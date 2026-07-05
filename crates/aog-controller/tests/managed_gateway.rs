//! X2 gate — "an existing OpenAI/Anthropic client is unaffected across the
//! cutover" to Loom management, against **live** OpenBao and the **real**
//! `aog-gateway` OpenAI surface (A3.2 no-mock-only).
//!
//! A real gateway serves an OpenAI-wire chat; the client completes it. The
//! gateway is then brought under management — declared as a `Workload`, bound by
//! a `Placement`, and reconciled to `Ready` by the `WorkloadController` probing
//! its live `/healthz`. The **same** client request is byte-identical
//! afterward: management touches the estate, never the data path.
#![allow(clippy::print_stderr)]

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use aog_apiserver::AppState;
use aog_apiserver::auth::Authenticator;
use aog_apiserver::seal::Sealer;
use aog_controller::{
    AlwaysLeader, Controller, EstateClient, HttpWorkloadProbe, Reconciler, SyncStats,
    WorkloadController,
};
use aog_estate::{
    Kind, NodeSpec, Phase, PlacementSpec, Resource, ResourceObject, WorkloadKind, WorkloadSpec,
};
use aog_gateway::app::{AppState as GwState, ModelMap, Target};
use aog_gateway::provider::Registry;
use aog_gateway::provider::openai::OpenAiProvider;
use aog_gateway::{Gateway, GatewayConfig};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::Utc;
use fabric_contracts::{
    Attenuation, Budget, Classification, RevocationStatus, Signature, TrustToken,
};
use fabric_crypto::Signer;
use fabric_crypto::providers::RustCryptoMlDsa87;
use reqwest::{Client, Method};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio::net::TcpListener;
use wsf_bridge::{OpenBaoAuth, OpenBaoConfig};

const ROLE: &str = "loom-x2";
const KV_PREFIX: &str = "kv/data/aog/virtual-keys";
const VK: &str = "vk_x2";
const WL: &str = "aog-gateway";

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
    let policy = r#"path "kv/data/aog/virtual-keys/*" { capabilities = ["create","read","update","delete"] }"#;
    bao(
        c,
        addr,
        tok,
        Method::PUT,
        "sys/policies/acl/loom-x2",
        Some(json!({ "policy": policy })),
    )
    .await;
    bao(
        c,
        addr,
        tok,
        Method::POST,
        &format!("auth/approle/role/{ROLE}"),
        Some(json!({"token_policies":"default,loom-x2","token_ttl":"15m"})),
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

fn in_budget_token(signer: &RustCryptoMlDsa87) -> TrustToken {
    let now = Utc::now();
    let t = TrustToken {
        token_id: "tok_x2".to_string(),
        issued_at: now.to_rfc3339(),
        expires_at: (now + chrono::Duration::minutes(15)).to_rfc3339(),
        issuer: "wsf-trust-bridge".to_string(),
        trust_bundle_version: "2026.07.loom".to_string(),
        tenant_id: "tenant-a".to_string(),
        subject_id: None,
        subject_hash: "hmac-sha256:demo".to_string(),
        service_identity: None,
        identity_id: None,
        roles: vec![],
        compliance_scopes: vec![],
        allowed_routes: vec![],
        allowed_models: vec![],
        max_data_classification: Classification::Restricted,
        country: None,
        person_type: None,
        offline_mode: false,
        revocation_status: RevocationStatus::Valid,
        budget: Some(Budget {
            token_cap: 1_000_000,
            ..Default::default()
        }),
        attenuation: Attenuation::default(),
        signature: Signature {
            alg: String::new(),
            key_id: String::new(),
            value: String::new(),
        },
    };
    fabric_token::issue(t, signer).unwrap()
}

fn key_path(virtual_key: &str) -> String {
    format!(
        "{KV_PREFIX}/{}",
        hex::encode(Sha256::digest(virtual_key.as_bytes()))
    )
}

/// The mock OpenAI upstream the gateway dispatches to.
async fn upstream(Json(_body): Json<Value>) -> Response {
    Json(json!({
        "model": "upstream-x",
        "choices": [{"message": {"content": "Hello from the gateway"}, "finish_reason": "stop"}],
        "usage": {"prompt_tokens": 5, "completion_tokens": 4}
    }))
    .into_response()
}

async fn spawn(app: Router) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    base
}

fn fresh_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(name);
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

fn quiet(s: SyncStats) -> bool {
    s.enqueued == 0 && s.drained == 0 && s.processed == 0
}
fn idle<R: Reconciler>(c: &Controller<R>) -> bool {
    c.queue_len() == 0 && c.delayed_len() == 0
}
async fn settle<R: Reconciler>(c: &mut Controller<R>) {
    for _ in 0..200 {
        let now = Instant::now();
        let s = c.sync(now).await.unwrap();
        if quiet(s) && idle(c) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    panic!("controller did not settle");
}

/// Send the OpenAI-wire chat and return (status, assistant content).
async fn chat(http: &Client, base: &str) -> (u16, String) {
    let resp = http
        .post(format!("{base}/v1/chat/completions"))
        .bearer_auth(VK)
        .json(&json!({"model": "gpt-4o-mini", "messages": [{"role": "user", "content": "hi"}]}))
        .send()
        .await
        .unwrap();
    let status = resp.status().as_u16();
    let v: Value = resp.json().await.unwrap();
    (
        status,
        v["choices"][0]["message"]["content"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
    )
}

#[tokio::test]
async fn an_openai_client_is_unaffected_across_the_cutover_to_management() {
    let Some(addr) = openbao_addr() else {
        eprintln!(
            "SKIP an_openai_client_is_unaffected_across_the_cutover_to_management: WSF_OPENBAO_ADDR unset (X2 live gate)"
        );
        return;
    };
    let c = Client::new();
    let (role_id, secret_id) = provision(&c, &addr, &root_token()).await;

    // Seed an in-budget virtual key the gateway resolves.
    let anchor = RustCryptoMlDsa87::generate("loom-x2-anchor").unwrap();
    let openbao = OpenBaoAuth::new(OpenBaoConfig::new(&addr, role_id, secret_id)).unwrap();
    let vault = openbao.login().await.expect("login");
    openbao
        .put_kv_data(
            &vault,
            &key_path(VK),
            json!({ "token": in_budget_token(&anchor) }),
        )
        .await
        .expect("seed key");

    // The real gateway: mock upstream + surface router + a /healthz probe target.
    let upstream_base = spawn(Router::new().route("/v1/chat/completions", post(upstream))).await;
    let mut registry = Registry::new();
    registry.register(Arc::new(OpenAiProvider::new(
        "openai",
        upstream_base,
        "unused",
    )));
    let gateway = Arc::new(Gateway::new(
        openbao,
        GatewayConfig {
            token_public_key: anchor.public_key().to_vec(),
            virtual_key_kv_prefix: KV_PREFIX.to_string(),
        },
    ));
    let models = ModelMap::new().route("gpt-4o-mini", Target::new("openai", "upstream-x"));
    let state = GwState::new(gateway, Arc::new(registry), Arc::new(models));
    let gw_base =
        spawn(aog_gateway::surface_openai::router(state).route("/healthz", get(|| async { "ok" })))
            .await;
    let http = Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .unwrap();

    // ── Before management: the client completes a chat.
    let (status_before, content_before) = chat(&http, &gw_base).await;
    assert_eq!(status_before, 200, "chat works before management");
    assert_eq!(content_before, "Hello from the gateway");

    // ── Bring the gateway under Loom management.
    let estate = AppState::bootstrap(
        1,
        fresh_dir("loom-x2-estate"),
        Authenticator::new(anchor.public_key().to_vec()),
        Sealer::generate().unwrap(),
    )
    .await
    .unwrap();
    let client = EstateClient::new(estate.admission(), estate.reader());
    client
        .ensure_created(ResourceObject::Node(Resource::new(
            "gw-node-1",
            NodeSpec {
                ring: 1,
                attestation_floor: Classification::Restricted,
                attestation: aog_estate::AttestationProfile::default(),
                capacity: aog_estate::Capacity::default(),
            },
        )))
        .await
        .unwrap();
    client
        .ensure_created(ResourceObject::Workload(Resource::new(
            WL,
            WorkloadSpec {
                workload_kind: WorkloadKind::Gateway,
                replicas: 1,
                ring: 1,
                classification_ceiling: Classification::Restricted,
                image: None,
                command: vec![],
                capability: None,
            },
        )))
        .await
        .unwrap();
    // The Placement the scheduler (Phase S) will mint; here it stands in for it.
    client
        .ensure_created(ResourceObject::Placement(Resource::new(
            "gw-place",
            PlacementSpec {
                workload: WL.to_string(),
                node: "gw-node-1".to_string(),
                token_id: String::new(),
            },
        )))
        .await
        .unwrap();

    let probe = Arc::new(
        HttpWorkloadProbe::new(HashMap::from([(
            WL.to_string(),
            format!("{gw_base}/healthz"),
        )]))
        .unwrap(),
    );
    let mut controller = Controller::new(
        "workload",
        estate.informer("Workload/"),
        WorkloadController::new(client.clone(), probe),
        Arc::new(AlwaysLeader),
    );
    settle(&mut controller).await;

    // ── Managed: Ready, placed, its live replica reflected.
    let Some(ResourceObject::Workload(managed)) = client.get(Kind::Workload, WL).await.unwrap()
    else {
        panic!("workload missing");
    };
    let status = managed.status.expect("status");
    assert_eq!(status.phase, Phase::Ready, "the gateway workload is Ready");
    assert_eq!(status.ready_replicas, 1, "its live replica is reflected");
    assert_eq!(
        status.placements,
        vec!["gw-node-1".to_string()],
        "its placement is reflected"
    );

    // ── Across the cutover: the same client request is byte-identical.
    let (status_after, content_after) = chat(&http, &gw_base).await;
    assert_eq!(
        status_after, status_before,
        "status unchanged across cutover"
    );
    assert_eq!(
        content_after, content_before,
        "response unchanged across cutover"
    );
}
