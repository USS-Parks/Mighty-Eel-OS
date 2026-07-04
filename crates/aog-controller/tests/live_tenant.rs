//! R3 gate — provision → issue → deprovision → revoked-everywhere, against a
//! **live** OpenBao (A3.2 no-mock-only closure; mock-only does not satisfy
//! this path).
//!
//! Env-gated like the W1 live suite: runs only when `WSF_OPENBAO_ADDR` is set
//! (e.g. `docker run -e BAO_DEV_ROOT_TOKEN_ID=root -p 8200:8200
//! openbao/openbao`); returns cleanly otherwise so the offline lane stays
//! green. Self-bootstraps AppRole + KV + an admin policy from the dev root
//! token. `WSF_OPENBAO_TOKEN` overrides the root token (default `root`).
#![allow(clippy::print_stderr)]

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use aog_apiserver::AppState;
use aog_apiserver::admission::{AdmissionRequest, Principal, Verb};
use aog_apiserver::auth::{Authenticator, TOKEN_HEADER};
use aog_apiserver::seal::Sealer;
use aog_controller::{
    AlwaysLeader, Controller, EstateClient, GarbageCollector, OPENBAO_FINALIZER, Reconciler,
    RevocationIndexer, SyncStats, TENANT_FINALIZER, TenantProvisioner, TenantTeardown,
};
use aog_estate::{CapabilitySpec, Kind, Resource, ResourceObject, TenantSpec};
use axum::http::HeaderMap;
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use chrono::Utc;
use fabric_contracts::{
    Attenuation, Budget, Classification, ComplianceScope, RevocationStatus, Signature, TrustToken,
};
use fabric_crypto::Signer;
use fabric_crypto::providers::{MlDsa87Verifier, RustCryptoMlDsa87};
use fabric_revocation::RevocationSnapshot;
use reqwest::{Client, Method};
use serde_json::json;
use wsf_bridge::{BridgeConfig, IssueTokenRequest, OpenBaoAuth, OpenBaoConfig, TrustBridge};
use wsf_tenants::{TenantAdmin, TenantAdminConfig, TenantRecord};

const ROLE: &str = "loom-r3-admin";
const TENANT: &str = "loom-acme";

fn openbao_addr() -> Option<String> {
    std::env::var("WSF_OPENBAO_ADDR").ok()
}

fn root_token() -> String {
    std::env::var("WSF_OPENBAO_TOKEN").unwrap_or_else(|_| "root".to_string())
}

async fn bao(
    c: &Client,
    addr: &str,
    tok: &str,
    method: Method,
    path: &str,
    body: Option<serde_json::Value>,
) -> reqwest::Response {
    let url = format!("{addr}/v1/{path}");
    let mut rb = c.request(method, &url).header("X-Vault-Token", tok);
    if let Some(b) = body {
        rb = rb.json(&b);
    }
    rb.send().await.expect("openbao request")
}

/// Bootstrap AppRole + KV + an admin policy (tenants read/write, tenant
/// metadata delete, revocations write) from the dev root token.
async fn bootstrap(c: &Client, addr: &str, tok: &str) -> (String, String) {
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

    let policy = r#"
path "kv/data/tenants/*"     { capabilities = ["create", "read", "update"] }
path "kv/metadata/tenants/*" { capabilities = ["delete"] }
path "kv/data/revocations/*" { capabilities = ["create", "read", "update"] }
"#;
    bao(
        c,
        addr,
        tok,
        Method::PUT,
        "sys/policies/acl/loom-r3-admin",
        Some(json!({ "policy": policy })),
    )
    .await;
    bao(
        c,
        addr,
        tok,
        Method::POST,
        &format!("auth/approle/role/{ROLE}"),
        Some(json!({"token_policies":"default,loom-r3-admin","token_ttl":"15m"})),
    )
    .await;

    let rid: serde_json::Value = bao(
        c,
        addr,
        tok,
        Method::GET,
        &format!("auth/approle/role/{ROLE}/role-id"),
        None,
    )
    .await
    .json()
    .await
    .expect("role-id json");
    let role_id = rid["data"]["role_id"].as_str().expect("role_id").to_owned();
    let sid: serde_json::Value = bao(
        c,
        addr,
        tok,
        Method::POST,
        &format!("auth/approle/role/{ROLE}/secret-id"),
        Some(json!({})),
    )
    .await
    .json()
    .await
    .expect("secret-id json");
    let secret_id = sid["data"]["secret_id"]
        .as_str()
        .expect("secret_id")
        .to_owned();
    (role_id, secret_id)
}

fn fresh_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(name);
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

fn mint(signer: &RustCryptoMlDsa87, tenant: &str, id: &str) -> TrustToken {
    let now = Utc::now();
    let token = TrustToken {
        token_id: id.to_owned(),
        issued_at: now.to_rfc3339(),
        expires_at: (now + chrono::Duration::hours(1)).to_rfc3339(),
        issuer: "wsf-bridge".to_owned(),
        trust_bundle_version: "2026.07.loom".to_owned(),
        tenant_id: tenant.to_owned(),
        subject_id: None,
        subject_hash: format!("hmac:{tenant}:{id}"),
        service_identity: Some("aogctl".to_owned()),
        identity_id: None,
        roles: vec![],
        compliance_scopes: vec![ComplianceScope::Hipaa],
        allowed_routes: vec![],
        allowed_models: vec![],
        max_data_classification: Classification::Restricted,
        country: None,
        person_type: None,
        offline_mode: false,
        revocation_status: RevocationStatus::Valid,
        budget: None,
        attenuation: Attenuation::default(),
        signature: Signature {
            alg: String::new(),
            key_id: String::new(),
            value: String::new(),
        },
    };
    fabric_token::issue(token, signer).unwrap()
}

fn headers_for(token: &TrustToken) -> HeaderMap {
    let mut headers = HeaderMap::new();
    let value = BASE64.encode(serde_json::to_vec(token).unwrap());
    headers.insert(TOKEN_HEADER, value.parse().unwrap());
    headers
}

fn quiet(stats: SyncStats) -> bool {
    stats.enqueued == 0 && stats.drained == 0 && stats.processed == 0
}

fn idle<R: Reconciler>(c: &Controller<R>) -> bool {
    c.queue_len() == 0 && c.delayed_len() == 0
}

async fn settle4<A: Reconciler, B: Reconciler, C: Reconciler, D: Reconciler>(
    a: &mut Controller<A>,
    b: &mut Controller<B>,
    c: &mut Controller<C>,
    d: &mut Controller<D>,
) {
    for _ in 0..300 {
        let now = Instant::now();
        let (sa, sb, sc, sd) = (
            a.sync(now).await.unwrap(),
            b.sync(now).await.unwrap(),
            c.sync(now).await.unwrap(),
            d.sync(now).await.unwrap(),
        );
        if quiet(sa)
            && quiet(sb)
            && quiet(sc)
            && quiet(sd)
            && idle(a)
            && idle(b)
            && idle(c)
            && idle(d)
        {
            return;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    panic!("controllers did not settle within 300 rounds");
}

#[tokio::test]
async fn provision_issue_deprovision_revoked_everywhere_live() {
    let Some(addr) = openbao_addr() else {
        eprintln!(
            "SKIP provision_issue_deprovision_revoked_everywhere_live: WSF_OPENBAO_ADDR unset (R3 live gate)"
        );
        return;
    };
    let http = Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .unwrap();
    let root = root_token();
    let (role_id, secret_id) = bootstrap(&http, &addr, &root).await;

    // One trust anchor signs everything: tokens, and the deprovision snapshot.
    let anchor = Arc::new(RustCryptoMlDsa87::generate("loom-r3-anchor").unwrap());
    let admin = Arc::new(TenantAdmin::new(
        OpenBaoAuth::new(OpenBaoConfig::new(
            &addr,
            role_id.clone(),
            secret_id.clone(),
        ))
        .unwrap(),
        Arc::clone(&anchor) as Arc<dyn Signer>,
        TenantAdminConfig::new(),
    ));

    // The estate, front-doored on the same anchor.
    let state = AppState::bootstrap(
        1,
        fresh_dir("loom-r3-live"),
        Authenticator::new(anchor.public_key().to_vec()),
        Sealer::generate().unwrap(),
    )
    .await
    .unwrap();
    let client = EstateClient::new(state.admission(), state.reader());

    // Declare the tenant + a scoped capability as an authenticated operator,
    // so admission mints scoped child tokens (what deprovision will revoke).
    let operator = Principal::authenticated(mint(&anchor, "ops", "tok-ops"));
    let tenant = Resource::new(
        TENANT,
        TenantSpec {
            display_name: "Loom Acme Health".to_owned(),
            ring: 1,
            classification_ceiling: Classification::Restricted,
            compliance_scopes: vec![ComplianceScope::Hipaa],
            subject_hmac_rotation_days: 0,
        },
    );
    state
        .admission()
        .admit(
            AdmissionRequest {
                verb: Verb::Create,
                kind: Kind::Tenant,
                name: TENANT.to_owned(),
                object: Some(ResourceObject::Tenant(tenant)),
            },
            &operator,
        )
        .await
        .unwrap();
    let mut cap = Resource::new(
        "loom-acme-cap",
        CapabilitySpec {
            budget: Budget::default(),
            caveats: vec![],
            allowed_routes: vec![],
            allowed_models: vec![],
            max_classification: Classification::Restricted,
            ttl_seconds: 300,
        },
    );
    cap.metadata.tenant = Some(TENANT.to_owned());
    state
        .admission()
        .admit(
            AdmissionRequest {
                verb: Verb::Create,
                kind: Kind::Capability,
                name: "loom-acme-cap".to_owned(),
                object: Some(ResourceObject::Capability(cap)),
            },
            &operator,
        )
        .await
        .unwrap();

    // The Phase-R controllers, provisioner included.
    let mut provisioner = Controller::new(
        "tenant-provisioner",
        state.informer("Tenant/"),
        TenantProvisioner::new(client.clone(), Arc::clone(&admin)),
        Arc::new(AlwaysLeader),
    );
    let mut teardown = Controller::new(
        "tenant-teardown",
        state.informer("Tenant/"),
        TenantTeardown::new(client.clone()).with_requeue(Duration::from_millis(1)),
        Arc::new(AlwaysLeader),
    );
    let mut gc = Controller::new(
        "gc",
        state.informer(""),
        GarbageCollector::new(client.clone()),
        Arc::new(AlwaysLeader),
    );
    let mut indexer = Controller::new(
        "revocation-indexer",
        state.informer("RevocationIntent/"),
        RevocationIndexer::new(client.clone(), state.authenticator().live_revocation()),
        Arc::new(AlwaysLeader),
    );
    settle4(&mut provisioner, &mut teardown, &mut gc, &mut indexer).await;

    // ── Provisioned: the record is live in OpenBao, the status reflects it.
    let record = admin.get(TENANT).await.expect("record provisioned live");
    assert_eq!(record.tenant_id, TENANT);
    assert_eq!(record.max_data_classification, "restricted");
    assert!(record.compliance_scopes.contains(&"hipaa".to_owned()));
    assert_eq!(record.subject_hmac_key.len(), 64, "32-byte hex HMAC key");
    let Some(ResourceObject::Tenant(estate_tenant)) =
        client.get(Kind::Tenant, TENANT).await.unwrap()
    else {
        panic!("tenant missing from estate");
    };
    let status = estate_tenant.status.clone().expect("status written");
    assert_eq!(
        status.openbao_path.as_deref(),
        Some("kv/data/tenants/loom-acme")
    );
    assert!(
        estate_tenant
            .metadata
            .finalizers
            .contains(&OPENBAO_FINALIZER.to_owned())
            && estate_tenant
                .metadata
                .finalizers
                .contains(&TENANT_FINALIZER.to_owned()),
        "both lifecycle finalizers guard the tenant"
    );

    // ── Issue: the record is usable — a real token minted through the bridge.
    let bridge = TrustBridge::new(
        OpenBaoAuth::new(OpenBaoConfig::new(&addr, role_id, secret_id)).unwrap(),
        Arc::clone(&anchor) as Arc<dyn Signer>,
        BridgeConfig::new("2026.07.loom", vec![9u8; 32]).with_locale("US", "us_person"),
    );
    let issued = bridge
        .issue_token(&IssueTokenRequest::new(
            TENANT,
            "analyst-7",
            vec!["analyst".to_owned()],
        ))
        .await
        .expect("issue against the live record");
    assert_eq!(issued.tenant_id, TENANT);

    // ── Rotation: doctor the record's rotation stamp into the deep past, wake
    // the provisioner, and the subject-HMAC key is re-minted (spec window 0 →
    // the 90-day default, long past for 2020).
    let old_key = record.subject_hmac_key.clone();
    let doctored = TenantRecord {
        hmac_rotated_at: "2020-01-01T00:00:00Z".to_owned(),
        ..record
    };
    bao(
        &http,
        &addr,
        &root,
        Method::POST,
        &format!("kv/data/tenants/{TENANT}"),
        Some(json!({ "data": { "attributes": serde_json::to_string(&doctored).unwrap() } })),
    )
    .await;
    provisioner.enqueue(&format!("Tenant/{TENANT}"));
    settle4(&mut provisioner, &mut teardown, &mut gc, &mut indexer).await;
    let rotated = admin.get(TENANT).await.expect("record still live");
    assert_ne!(
        rotated.subject_hmac_key, old_key,
        "subject-HMAC key rotated"
    );
    assert_ne!(rotated.hmac_rotated_at, "2020-01-01T00:00:00Z");

    // ── Deprovision: delete the tenant; the estate tears down and OpenBao is
    // cleaned + a signed revocation snapshot lands on the poll path.
    client.delete(Kind::Tenant, TENANT).await.unwrap();
    settle4(&mut provisioner, &mut teardown, &mut gc, &mut indexer).await;

    assert!(
        client.get(Kind::Tenant, TENANT).await.unwrap().is_none(),
        "estate tenant finalized away"
    );
    assert!(
        client.list(Kind::Capability).await.unwrap().is_empty(),
        "no dangling capability"
    );
    assert!(
        admin.get(TENANT).await.is_err(),
        "OpenBao record deprovisioned"
    );
    assert!(
        bridge
            .issue_token(&IssueTokenRequest::new(TENANT, "analyst-8", vec![]))
            .await
            .is_err(),
        "issuance root is gone: no new tokens for a deprovisioned tenant"
    );

    // The persisted snapshot: signed by the anchor, revoking the control-plane
    // token ids enumerated from the tenant's estate objects.
    let resp: serde_json::Value = bao(
        &http,
        &addr,
        &root,
        Method::GET,
        &format!("kv/data/revocations/{TENANT}"),
        None,
    )
    .await
    .json()
    .await
    .expect("revocation snapshot json");
    let snapshot: RevocationSnapshot =
        serde_json::from_value(resp["data"]["data"].clone()).expect("snapshot deserializes");
    fabric_revocation::verify(&snapshot, &MlDsa87Verifier, anchor.public_key())
        .expect("snapshot signature verifies off-host");
    assert!(
        !snapshot.revoked_tokens.is_empty(),
        "the snapshot revokes the tenant's enumerable control-plane tokens"
    );

    // ── Revoked everywhere (kernel leg): the tenant's tokens die at the front
    // door while an unrelated tenant's pass.
    let acme_token = mint(&anchor, TENANT, "tok-acme-live");
    assert!(
        state
            .authenticator()
            .authenticate(&headers_for(&acme_token))
            .is_err(),
        "post-deprovision, the tenant's token is refused"
    );
    let bystander = mint(&anchor, "globex", "tok-globex");
    assert!(
        state
            .authenticator()
            .authenticate(&headers_for(&bystander))
            .is_ok(),
        "an unrelated tenant is untouched"
    );
}
