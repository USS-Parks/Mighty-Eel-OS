//! AF-002 gate (offline): `/v1/tokens/issue` refuses an unauthenticated caller
//! *before* any token is minted, and the issued token's identity comes from the
//! verified principal — not the request body.
//!
//! No live OpenBao is needed: the authenticator middleware rejects an
//! unauthenticated request before the bridge is ever consulted, and an
//! authenticated one is proven to pass the gate (it then fails at the dummy
//! bridge, which is *past* the security boundary under test).

use std::sync::{Arc, Mutex};

use base64::Engine;
use fabric_contracts::WsfPrincipal;
use fabric_crypto::Signer;
use fabric_crypto::providers::RustCryptoMlDsa87;
use wsf_api::AppState;
use wsf_api::auth::{
    DEV_PRINCIPAL_HEADER, DenyAllAuthenticator, DevAuthenticator, WsfAuthenticator,
};
use wsf_bridge::{BridgeConfig, OpenBaoAuth, OpenBaoConfig, TrustBridge};
use wsf_broker::{AwsStsBroker, BrokerConfig};
use wsf_ledger::Ledger;
use wsf_seal::{SealService, SealServiceConfig};

fn dummy_openbao() -> OpenBaoAuth {
    OpenBaoAuth::new(OpenBaoConfig::new("http://127.0.0.1:1", "r", "s")).unwrap()
}

fn state(authenticator: Arc<dyn WsfAuthenticator>) -> AppState {
    let signer = Arc::new(RustCryptoMlDsa87::generate("gate-bridge").unwrap());
    let anchor = signer.public_key().to_vec();
    AppState {
        bridge: Arc::new(TrustBridge::new(
            dummy_openbao(),
            signer,
            BridgeConfig::new("v", vec![7u8; 32]),
        )),
        broker: Arc::new(AwsStsBroker::new(
            dummy_openbao(),
            reqwest::Client::new(),
            BrokerConfig::new("us-east-1", "http://127.0.0.1:1", "kv/data/broker/aws-root"),
        )),
        seal: Arc::new(SealService::new(
            dummy_openbao(),
            Arc::new(RustCryptoMlDsa87::generate("gate-seal").unwrap()),
            SealServiceConfig {
                transit_key: "k".to_string(),
                token_public_key: anchor.clone(),
            },
        )),
        ledger: Arc::new(Mutex::new(Ledger::new(Arc::new(
            RustCryptoMlDsa87::generate("gate-ledger").unwrap(),
        )))),
        token_public_key: Arc::new(anchor),
        authenticator,
    }
}

async fn serve(state: AppState) -> String {
    let app = wsf_api::router(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    base
}

#[tokio::test]
async fn issue_without_identity_is_401() {
    let base = serve(state(Arc::new(DenyAllAuthenticator))).await;
    let resp = reqwest::Client::new()
        .post(format!("{base}/v1/tokens/issue"))
        .json(&serde_json::json!({}))
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status().as_u16(),
        401,
        "unauthenticated issuance is refused"
    );
}

#[tokio::test]
async fn issue_with_verified_principal_passes_the_gate() {
    // The dev authenticator accepts a principal header, so the request gets PAST
    // the gate; the handler then cannot reach the dummy OpenBao bridge and returns
    // 502 — a failure *after* the security boundary, i.e. the gate let it through.
    let base = serve(state(Arc::new(DevAuthenticator::new("wsf")))).await;
    let principal = WsfPrincipal {
        tenant_id: "baap".into(),
        subject_id: "clinician-1".into(),
        service_identity: None,
        roles: vec!["clinician".into()],
        audience: "wsf".into(),
        auth_method: "dev".into(),
        credential_id: String::new(),
        correlation_id: String::new(),
    };
    let hdr =
        base64::engine::general_purpose::STANDARD.encode(serde_json::to_vec(&principal).unwrap());
    let resp = reqwest::Client::new()
        .post(format!("{base}/v1/tokens/issue"))
        .header(DEV_PRINCIPAL_HEADER, hdr)
        .json(&serde_json::json!({}))
        .send()
        .await
        .unwrap();
    assert_ne!(
        resp.status().as_u16(),
        401,
        "an authenticated request must pass the issuance gate"
    );
    assert_eq!(
        resp.status().as_u16(),
        502,
        "past the gate: the handler reached the (unreachable) bridge"
    );
}

#[tokio::test]
async fn receipts_without_identity_is_401() {
    let base = serve(state(Arc::new(DenyAllAuthenticator))).await;
    let resp = reqwest::Client::new()
        .get(format!("{base}/v1/receipts"))
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status().as_u16(),
        401,
        "unauthenticated receipt read is refused (AF-007)"
    );
}

#[tokio::test]
async fn receipts_are_tenant_scoped_to_the_principal() {
    // Two tenants' receipts in the ledger; a tenant-a principal sees only its own.
    let st = state(Arc::new(DevAuthenticator::new("wsf")));
    {
        let mut l = st.ledger.lock().unwrap();
        l.ingest(
            "wsf-bridge",
            serde_json::json!({"tenant_id":"tenant-a","token_id":"tok_a"}),
        )
        .unwrap();
        l.ingest(
            "wsf-bridge",
            serde_json::json!({"tenant_id":"tenant-b","token_id":"tok_b"}),
        )
        .unwrap();
    }
    let base = serve(st).await;
    let principal = WsfPrincipal {
        tenant_id: "tenant-a".into(),
        subject_id: "s".into(),
        service_identity: None,
        roles: vec![],
        audience: "wsf".into(),
        auth_method: "dev".into(),
        credential_id: String::new(),
        correlation_id: String::new(),
    };
    let hdr =
        base64::engine::general_purpose::STANDARD.encode(serde_json::to_vec(&principal).unwrap());
    let resp = reqwest::Client::new()
        .get(format!("{base}/v1/receipts"))
        .header(DEV_PRINCIPAL_HEADER, hdr)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let entries = body["entries"].as_array().unwrap();
    assert_eq!(entries.len(), 1, "tenant-a sees only its own receipt");
    assert_eq!(entries[0]["receipt"]["tenant_id"], "tenant-a");
}
