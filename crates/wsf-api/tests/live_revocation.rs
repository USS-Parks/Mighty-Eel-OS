//! End-to-end revocation across services.
//!
//! Env-gated on `WSF_OPENBAO_ADDR` + `WSF_AWS_ENDPOINT`. Drives the full loop
//! over live OpenBao + Moto through the real HTTP API:
//!
//! 1. issue → verify → attenuate → seal → unseal → exchange all succeed with the revocation store
//!    engaged (a clean sequence-1 snapshot, distributed via OpenBao KV);
//! 2. a signed sequence-2 snapshot revoking the tenant is published to KV,
//!    fetched back, and advanced into the store the seal service and broker
//!    share;
//! 3. every privileged token consumer is denied (403) — no restart/cache clear;
//! 4. replaying the older clean snapshot is refused (anti-rollback) and
//!    the denials stand.
#![allow(clippy::print_stdout, clippy::print_stderr)]

use std::sync::{Arc, Mutex, RwLock};

use base64::Engine;
use fabric_contracts::Classification;
use fabric_crypto::Signer;
use fabric_crypto::providers::{MlDsa87Verifier, RustCryptoMlDsa87};
use fabric_revocation::{MonotonicRevocationStore, RevocationError, RevocationSnapshot};
use reqwest::{Client, Method};
use serde_json::{Value, json};
use wsf_api::client::{ClientError, WsfClient};
use wsf_api::{AppState, ExchangeReq, IssueReq, SealReq, UnsealReq};
use wsf_bridge::{BridgeConfig, IssueTokenRequest, OpenBaoAuth, OpenBaoConfig, TrustBridge};
use wsf_broker::{AwsStsBroker, BrokerConfig};
use wsf_ledger::Ledger;
use wsf_seal::{LabelSpec, SealService, SealServiceConfig};

const ROLE: &str = "wsf-r6-test";
const TENANT: &str = "wsf-r6-tenant";
const TENANT_B: &str = "wsf-r6-tenant-b";
const TRANSIT_KEY: &str = "wsf-r6-dek";
const CRED_PATH: &str = "kv/data/broker/aws-root";
const REV_PATH: &str = "revocation/r6-current";

fn openbao_addr() -> Option<String> {
    std::env::var("WSF_OPENBAO_ADDR").ok()
}
fn aws_endpoint() -> Option<String> {
    std::env::var("WSF_AWS_ENDPOINT").ok()
}
fn root_token() -> String {
    std::env::var("WSF_OPENBAO_TOKEN").unwrap_or_else(|_| "root".to_string())
}

async fn bao(
    c: &Client,
    addr: &str,
    tok: &str,
    m: Method,
    path: &str,
    body: Option<Value>,
) -> String {
    let url = format!("{addr}/v1/{path}");
    let mut rb = c.request(m, &url).header("X-Vault-Token", tok);
    if let Some(b) = body {
        rb = rb
            .header("Content-Type", "application/json")
            .body(b.to_string());
    }
    rb.send()
        .await
        .expect("openbao req")
        .text()
        .await
        .unwrap_or_default()
}

async fn provision(c: &Client, addr: &str, tok: &str) -> (String, String) {
    let _ = bao(
        c,
        addr,
        tok,
        Method::POST,
        "sys/auth/approle",
        Some(json!({"type":"approle"})),
    )
    .await;
    let _ = bao(
        c,
        addr,
        tok,
        Method::POST,
        "sys/mounts/kv",
        Some(json!({"type":"kv","options":{"version":"2"}})),
    )
    .await;
    let _ = bao(
        c,
        addr,
        tok,
        Method::POST,
        "sys/mounts/transit",
        Some(json!({"type":"transit"})),
    )
    .await;
    // Self-clean any stale snapshot from a previous run (KV-v2 metadata wipe).
    let _ = bao(
        c,
        addr,
        tok,
        Method::DELETE,
        &format!("kv/metadata/{REV_PATH}"),
        None,
    )
    .await;
    bao(
        c,
        addr,
        tok,
        Method::POST,
        &format!("transit/keys/{TRANSIT_KEY}-{TENANT}"),
        Some(json!({"type":"aes256-gcm96"})),
    )
    .await;
    bao(
        c,
        addr,
        tok,
        Method::POST,
        &format!("transit/keys/{TRANSIT_KEY}-{TENANT_B}"),
        Some(json!({"type":"aes256-gcm96"})),
    )
    .await;

    let policy = format!(
        "path \"kv/data/tenants/*\" {{ capabilities=[\"read\"] }}\npath \"kv/data/broker/*\" {{ capabilities=[\"read\"] }}\npath \"kv/data/revocation/*\" {{ capabilities=[\"read\"] }}\npath \"transit/encrypt/{TRANSIT_KEY}-*\" {{ capabilities=[\"update\"] }}\npath \"transit/decrypt/{TRANSIT_KEY}-*\" {{ capabilities=[\"update\"] }}"
    );
    bao(
        c,
        addr,
        tok,
        Method::PUT,
        &format!("sys/policies/acl/{ROLE}"),
        Some(json!({ "policy": policy })),
    )
    .await;
    let attrs_b = json!({
        "tenant_id": TENANT_B, "display_name": "WSF R6 Tenant B",
        "compliance_scopes": ["hipaa"], "default_allowed_routes": ["local_only"],
        "max_data_classification": "restricted"
    });
    bao(
        c,
        addr,
        tok,
        Method::POST,
        &format!("kv/data/tenants/{TENANT_B}"),
        Some(json!({ "data": { "attributes": attrs_b.to_string() } })),
    )
    .await;
    bao(
        c,
        addr,
        tok,
        Method::POST,
        &format!("auth/approle/role/{ROLE}"),
        Some(json!({"token_policies":format!("default,{ROLE}"),"token_ttl":"15m"})),
    )
    .await;

    let attrs = json!({
        "tenant_id": TENANT, "display_name": "WSF R6 Tenant",
        "compliance_scopes": ["hipaa"], "default_allowed_routes": ["local_only"],
        "max_data_classification": "restricted"
    });
    bao(
        c,
        addr,
        tok,
        Method::POST,
        &format!("kv/data/tenants/{TENANT}"),
        Some(json!({ "data": { "attributes": attrs.to_string() } })),
    )
    .await;
    bao(
        c,
        addr,
        tok,
        Method::POST,
        CRED_PATH,
        Some(json!({ "data": { "access_key_id": "test", "secret_access_key": "test" } })),
    )
    .await;

    let rid: Value = serde_json::from_str(
        &bao(
            c,
            addr,
            tok,
            Method::GET,
            &format!("auth/approle/role/{ROLE}/role-id"),
            None,
        )
        .await,
    )
    .expect("role-id json");
    let role_id = rid["data"]["role_id"]
        .as_str()
        .expect("role_id")
        .to_string();
    let sid: Value = serde_json::from_str(
        &bao(
            c,
            addr,
            tok,
            Method::POST,
            &format!("auth/approle/role/{ROLE}/secret-id"),
            Some(json!({})),
        )
        .await,
    )
    .expect("secret-id json");
    let secret_id = sid["data"]["secret_id"]
        .as_str()
        .expect("secret_id")
        .to_string();
    (role_id, secret_id)
}

/// Publish a signed snapshot to OpenBao KV (the distribution channel).
async fn publish_snapshot(c: &Client, addr: &str, tok: &str, snapshot: &RevocationSnapshot) {
    let payload = serde_json::to_string(snapshot).expect("snapshot json");
    bao(
        c,
        addr,
        tok,
        Method::POST,
        &format!("kv/data/{REV_PATH}"),
        Some(json!({ "data": { "snapshot": payload } })),
    )
    .await;
}

/// Fetch the current snapshot back from OpenBao KV, as a consumer would.
async fn fetch_snapshot(c: &Client, addr: &str, tok: &str) -> RevocationSnapshot {
    let raw = bao(
        c,
        addr,
        tok,
        Method::GET,
        &format!("kv/data/{REV_PATH}"),
        None,
    )
    .await;
    let v: Value = serde_json::from_str(&raw).expect("kv json");
    let payload = v["data"]["data"]["snapshot"]
        .as_str()
        .expect("snapshot field");
    serde_json::from_str(payload).expect("snapshot parse")
}

fn clean_snapshot(rev_anchor: &RustCryptoMlDsa87, sequence: u64) -> RevocationSnapshot {
    fabric_revocation::sign(
        RevocationSnapshot::new(
            format!("r6-snap-{sequence}"),
            "2026-07-07T00:00:00Z",
            "2027-01-01T00:00:00Z",
        )
        .with_sequence(sequence),
        rev_anchor,
    )
    .expect("sign snapshot")
}

fn assert_403(res: Result<impl std::fmt::Debug, ClientError>, leg: &str) {
    match res {
        Err(ClientError::Api { status, body }) => {
            assert_eq!(status, 403, "{leg}: expected 403, got {status}: {body}");
        }
        other => panic!("{leg}: expected 403 Api error, got {other:?}"),
    }
}

#[tokio::test]
async fn revocation_propagates_to_seal_and_broker_end_to_end() {
    let (Some(addr), Some(aws)) = (openbao_addr(), aws_endpoint()) else {
        eprintln!(
            "SKIP revocation_propagates_to_seal_and_broker_end_to_end: set WSF_OPENBAO_ADDR + WSF_AWS_ENDPOINT (live gate)"
        );
        return;
    };

    let c = Client::new();
    let root = root_token();
    let (role_id, secret_id) = provision(&c, &addr, &root).await;
    let ob = || {
        OpenBaoAuth::new(OpenBaoConfig::new(
            &addr,
            role_id.clone(),
            secret_id.clone(),
        ))
        .unwrap()
    };

    // Revocation anchor + the store BOTH services share.
    let rev_anchor = RustCryptoMlDsa87::generate("r6-rev-anchor").unwrap();
    let store = Arc::new(RwLock::new(MonotonicRevocationStore::new()));

    // Sequence 1 (clean) travels the real distribution channel: signed →
    // published to OpenBao KV → fetched back → verified + adopted.
    publish_snapshot(&c, &addr, &root, &clean_snapshot(&rev_anchor, 1)).await;
    let fetched = fetch_snapshot(&c, &addr, &root).await;
    assert_eq!(
        store
            .write()
            .unwrap()
            .advance(fetched, &MlDsa87Verifier, rev_anchor.public_key())
            .expect("adopt seq 1"),
        1
    );

    let bridge_signer = Arc::new(RustCryptoMlDsa87::generate("wsf-r6-bridge").unwrap());
    let anchor = bridge_signer.public_key().to_vec();
    let bridge = Arc::new(TrustBridge::new(
        ob(),
        bridge_signer.clone(),
        BridgeConfig::new("2026.07.07.r6", vec![6u8; 32])
            .with_token_ttl(std::time::Duration::from_secs(1_200)),
    ));
    let seal_signer = Arc::new(RustCryptoMlDsa87::generate("wsf-r6-seal").unwrap());
    let state = AppState {
        bridge: bridge.clone(),
        broker: Arc::new(
            AwsStsBroker::new(
                ob(),
                Client::new(),
                BrokerConfig::new("us-east-1", &aws, CRED_PATH),
            )
            .with_revocation_store(store.clone()),
        ),
        seal: Arc::new(
            SealService::new(
                ob(),
                seal_signer.clone(),
                SealServiceConfig {
                    transit_key: TRANSIT_KEY.to_string(),
                    token_public_key: anchor.clone(),
                },
            )
            .with_revocation_store(store.clone()),
        ),
        ledger: Arc::new(Mutex::new(Ledger::new(Arc::new(
            RustCryptoMlDsa87::generate("wsf-r6-ledger").unwrap(),
        )))),
        token_public_key: Arc::new(anchor.clone()),
        auth: Arc::new(wsf_api::auth::LocalDevAuthenticator::for_wsf(TENANT)),
        policy: Arc::new(wsf_api::policy::StaticTenantPolicies::single_dev(
            TENANT,
            &["clinician"],
        )),
        grants: Arc::new(wsf_api::grants::StaticGrants::single_dev(
            TENANT,
            "aws-readonly",
            "arn:aws:iam::000000000000:role/wsf-r6",
        )),
        auditors: Arc::new(wsf_api::audit::StaticAuditors::none()),
        revocation: wsf_api::RevocationEnforcement::required(store.clone()),
        attenuation: Arc::new(wsf_api::AttenuationState::new()),
    };

    let app = wsf_api::router(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let sdk = WsfClient::new(base);

    // A second real HTTP plane uses the same trust/revocation anchors but an
    // independently authenticated tenant. This proves revocation and envelope
    // behavior are tenant-partitioned, not merely single-fixture happy paths.
    let state_b = AppState {
        bridge: bridge.clone(),
        broker: Arc::new(
            AwsStsBroker::new(
                ob(),
                Client::new(),
                BrokerConfig::new("us-east-1", &aws, CRED_PATH),
            )
            .with_revocation_store(store.clone()),
        ),
        seal: Arc::new(
            SealService::new(
                ob(),
                seal_signer.clone(),
                SealServiceConfig {
                    transit_key: TRANSIT_KEY.to_string(),
                    token_public_key: anchor.clone(),
                },
            )
            .with_revocation_store(store.clone()),
        ),
        ledger: Arc::new(Mutex::new(Ledger::new(Arc::new(
            RustCryptoMlDsa87::generate("wsf-r6-ledger-b").unwrap(),
        )))),
        token_public_key: Arc::new(anchor.clone()),
        auth: Arc::new(wsf_api::auth::LocalDevAuthenticator::for_wsf(TENANT_B)),
        policy: Arc::new(wsf_api::policy::StaticTenantPolicies::single_dev(
            TENANT_B,
            &["clinician"],
        )),
        grants: Arc::new(wsf_api::grants::StaticGrants::single_dev(
            TENANT_B,
            "aws-readonly",
            "arn:aws:iam::000000000000:role/wsf-r6-b",
        )),
        auditors: Arc::new(wsf_api::audit::StaticAuditors::none()),
        revocation: wsf_api::RevocationEnforcement::required(store.clone()),
        attenuation: Arc::new(wsf_api::AttenuationState::new()),
    };
    let app_b = wsf_api::router(state_b);
    let listener_b = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let sdk_b = WsfClient::new(format!("http://{}", listener_b.local_addr().unwrap()));
    let server_b = tokio::spawn(async move { axum::serve(listener_b, app_b).await.unwrap() });

    // ---- Allow phase: everything works with the store engaged. ----
    let token = sdk
        .issue(&IssueReq {
            requested_roles: vec!["clinician".to_string()],
            requested_models: vec![],
            budget: None,
        })
        .await
        .expect("issue");
    assert!(sdk.verify(&token).await.expect("verify").valid);
    let child = sdk
        .attenuate(&token, &fabric_token::TokenRestrictions::new("r6-child"))
        .await
        .expect("attenuate before revocation");
    let second_subject = bridge
        .issue_token(&IssueTokenRequest::new(
            TENANT,
            "subject-two",
            vec!["clinician".to_string()],
        ))
        .await
        .expect("issue second subject");
    let label = LabelSpec {
        classification: Classification::Restricted,
        compliance_scopes: vec![],
        origin: "r6".to_string(),
        permitted_ops: vec!["unseal".to_string()],
        permitted_destinations: vec![],
        detected_entities: vec![],
    };
    let envelope = sdk
        .seal(&SealReq {
            token: child.clone(),
            plaintext_b64: base64::engine::general_purpose::STANDARD.encode(b"r6 payload"),
            label: label.clone(),
            envelope_id: "env-r6".to_string(),
        })
        .await
        .expect("seal succeeds before revocation");
    let plaintext = sdk
        .unseal(&UnsealReq {
            token: child.clone(),
            envelope: envelope.clone(),
        })
        .await
        .expect("unseal succeeds before revocation");
    assert_eq!(plaintext, b"r6 payload");
    assert_403(
        sdk.unseal(&UnsealReq {
            token: second_subject,
            envelope: envelope.clone(),
        })
        .await,
        "same-tenant second subject unseal",
    );
    let creds = sdk
        .exchange(&ExchangeReq {
            token: child.clone(),
            grant_id: "aws-readonly".to_string(),
        })
        .await
        .expect("exchange succeeds before revocation");
    assert!(!creds.access_key_id.is_empty());

    let token_b = sdk_b
        .issue(&IssueReq {
            requested_roles: vec!["clinician".to_string()],
            requested_models: vec![],
            budget: None,
        })
        .await
        .expect("tenant B issue");
    let envelope_b = sdk_b
        .seal(&SealReq {
            token: token_b.clone(),
            plaintext_b64: base64::engine::general_purpose::STANDARD.encode(b"tenant b payload"),
            label: LabelSpec {
                classification: Classification::Restricted,
                compliance_scopes: vec![],
                origin: "r6-b".to_string(),
                permitted_ops: vec!["unseal".to_string()],
                permitted_destinations: vec![],
                detected_entities: vec![],
            },
            envelope_id: "env-r6-b".to_string(),
        })
        .await
        .expect("tenant B seal");
    assert_403(
        sdk.verify(&token_b).await,
        "tenant A presenting tenant B token",
    );

    // ---- Revoke: sequence 2 names the tenant; consumers refresh from KV. ----
    let mut revoked =
        RevocationSnapshot::new("r6-snap-2", "2026-07-07T00:05:00Z", "2027-01-01T00:00:00Z")
            .with_sequence(2);
    revoked.revoked_tenants.push(TENANT.to_string());
    let revoked = fabric_revocation::sign(revoked, &rev_anchor).expect("sign seq 2");
    publish_snapshot(&c, &addr, &root, &revoked).await;
    let fetched = fetch_snapshot(&c, &addr, &root).await;
    assert_eq!(
        store
            .write()
            .unwrap()
            .advance(fetched, &MlDsa87Verifier, rev_anchor.public_key())
            .expect("adopt seq 2"),
        2
    );

    // Every privileged surface denies the still-signature-valid token now.
    assert_403(sdk.verify(&child).await, "verify after revocation");
    assert_403(
        sdk.attenuate(
            &child,
            &fabric_token::TokenRestrictions::new("r6-grandchild"),
        )
        .await,
        "attenuate after revocation",
    );
    assert_403(
        sdk.seal(&SealReq {
            token: child.clone(),
            plaintext_b64: base64::engine::general_purpose::STANDARD.encode(b"denied"),
            label,
            envelope_id: "env-r6-denied".to_string(),
        })
        .await,
        "seal after revocation",
    );
    assert_403(
        sdk.unseal(&UnsealReq {
            token: child.clone(),
            envelope: envelope.clone(),
        })
        .await,
        "unseal after revocation",
    );
    assert_403(
        sdk.exchange(&ExchangeReq {
            token: child.clone(),
            grant_id: "aws-readonly".to_string(),
        })
        .await,
        "exchange after revocation",
    );
    assert_403(
        sdk.issue(&IssueReq {
            requested_roles: vec!["clinician".to_string()],
            requested_models: vec![],
            budget: None,
        })
        .await,
        "issue after principal tenant revocation",
    );
    assert!(
        sdk_b.verify(&token_b).await.expect("tenant B verify").valid,
        "tenant B remains valid when only tenant A is revoked"
    );
    assert_eq!(
        sdk_b
            .unseal(&UnsealReq {
                token: token_b.clone(),
                envelope: envelope_b.clone(),
            })
            .await
            .expect("tenant B unseal after tenant A revocation"),
        b"tenant b payload"
    );
    assert!(
        !sdk_b
            .exchange(&ExchangeReq {
                token: token_b.clone(),
                grant_id: "aws-readonly".to_string(),
            })
            .await
            .expect("tenant B exchange after tenant A revocation")
            .access_key_id
            .is_empty()
    );

    // ---- anti-rollback: replaying the old clean snapshot is refused. ----
    let stale = clean_snapshot(&rev_anchor, 1);
    let err = store
        .write()
        .unwrap()
        .advance(stale, &MlDsa87Verifier, rev_anchor.public_key())
        .expect_err("rollback must be refused");
    assert!(
        matches!(
            err,
            RevocationError::Rollback {
                current: 2,
                candidate: 1
            }
        ),
        "got {err:?}"
    );
    assert_403(
        sdk.unseal(&UnsealReq {
            token: child.clone(),
            envelope: envelope.clone(),
        })
        .await,
        "unseal after rollback attempt",
    );

    // Process restart: rebuild revocation state solely from the signed
    // OpenBao-distributed sequence-2 snapshot, then prove the denial survives.
    let restart_store = Arc::new(RwLock::new(MonotonicRevocationStore::new()));
    let restart_snapshot = fetch_snapshot(&c, &addr, &root).await;
    restart_store
        .write()
        .unwrap()
        .advance(restart_snapshot, &MlDsa87Verifier, rev_anchor.public_key())
        .expect("restart adopts current snapshot");
    let restart_state = AppState {
        bridge: bridge.clone(),
        broker: Arc::new(
            AwsStsBroker::new(
                ob(),
                Client::new(),
                BrokerConfig::new("us-east-1", &aws, CRED_PATH),
            )
            .with_revocation_store(restart_store.clone()),
        ),
        seal: Arc::new(
            SealService::new(
                ob(),
                seal_signer,
                SealServiceConfig {
                    transit_key: TRANSIT_KEY.to_string(),
                    token_public_key: anchor.clone(),
                },
            )
            .with_revocation_store(restart_store.clone()),
        ),
        ledger: Arc::new(Mutex::new(Ledger::new(Arc::new(
            RustCryptoMlDsa87::generate("wsf-r6-ledger-restart").unwrap(),
        )))),
        token_public_key: Arc::new(anchor),
        auth: Arc::new(wsf_api::auth::LocalDevAuthenticator::for_wsf(TENANT)),
        policy: Arc::new(wsf_api::policy::StaticTenantPolicies::single_dev(
            TENANT,
            &["clinician"],
        )),
        grants: Arc::new(wsf_api::grants::StaticGrants::single_dev(
            TENANT,
            "aws-readonly",
            "arn:aws:iam::000000000000:role/wsf-r6",
        )),
        auditors: Arc::new(wsf_api::audit::StaticAuditors::none()),
        revocation: wsf_api::RevocationEnforcement::required(restart_store),
        attenuation: Arc::new(wsf_api::AttenuationState::new()),
    };
    let restart_app = wsf_api::router(restart_state);
    let restart_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let restart_sdk = WsfClient::new(format!("http://{}", restart_listener.local_addr().unwrap()));
    let restart_server =
        tokio::spawn(async move { axum::serve(restart_listener, restart_app).await.unwrap() });
    assert_403(restart_sdk.verify(&child).await, "verify after restart");

    server.abort();
    server_b.abort();
    restart_server.abort();
    eprintln!(
        "live gate PASSED against {addr} (+Moto {aws}): two tenants/two subjects isolated; revocation denied issue/verify/attenuate/seal/unseal/exchange; rollover, rollback refusal, and restart rehydration passed"
    );
}
