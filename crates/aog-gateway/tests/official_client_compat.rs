//! LSH-G9 live compatibility matrix.
//!
//! A real OpenBao-backed production gateway is driven by the vendor-maintained
//! OpenAI and Anthropic Python SDKs against all five supported execution
//! surfaces. The upstream is deliberately adversarial: redirects, malformed and
//! oversized JSON, truncated SSE, and false usage share the same harness as the
//! valid two-tenant, route-denial, budget, and revocation cases.
#![allow(clippy::print_stdout, clippy::print_stderr)]

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use aog_gateway::app::{AppState, ModelMap, Target};
use aog_gateway::provider::Registry;
use aog_gateway::provider::anthropic::AnthropicProvider;
use aog_gateway::provider::openai::OpenAiProvider;
use aog_gateway::{Gateway, GatewayConfig};
use axum::body::Body;
use axum::extract::State;
use axum::http::header::{CONTENT_LENGTH, CONTENT_TYPE, LOCATION};
use axum::http::{HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::{Duration, Utc};
use fabric_contracts::{
    Attenuation, Budget, Classification, RevocationStatus, Route, Signature, TrustToken,
};
use fabric_crypto::Signer;
use fabric_crypto::providers::RustCryptoMlDsa87;
use fabric_revocation::RevocationSnapshot;
use reqwest::{Client, Method};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio::net::TcpListener;
use tokio::process::Command;
use wsf_bridge::{OpenBaoAuth, OpenBaoConfig};

const ROLE: &str = "aog-g9-official-clients";
const KV_PREFIX: &str = "kv/data/aog/virtual-keys";
const REVOCATION_PATH: &str = "kv/data/aog/g9-official-revocation";
const MAX_JSON_BODY: u64 = 8 * 1024 * 1024;

const OPENAI_SSE: &str = "\
data: {\"choices\":[{\"delta\":{\"content\":\"gateway-\"}}]}\n\n\
data: {\"choices\":[{\"delta\":{\"content\":\"ok\"}}]}\n\n\
data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n\
data: {\"choices\":[],\"usage\":{\"prompt_tokens\":20,\"completion_tokens\":20}}\n\n\
data: [DONE]\n\n";

const OPENAI_TRUNCATED_SSE: &str =
    "data: {\"choices\":[{\"delta\":{\"content\":\"partial\"}}]}\n\n";

const ANTHROPIC_SSE: &str = "\
event: message_start\n\
data: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":20,\"output_tokens\":0}}}\n\n\
event: content_block_delta\n\
data: {\"type\":\"content_block_delta\",\"delta\":{\"type\":\"text_delta\",\"text\":\"gateway-\"}}\n\n\
event: content_block_delta\n\
data: {\"type\":\"content_block_delta\",\"delta\":{\"type\":\"text_delta\",\"text\":\"ok\"}}\n\n\
event: message_delta\n\
data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":20}}\n\n\
event: message_stop\n\
data: {\"type\":\"message_stop\"}\n\n";

const ANTHROPIC_TRUNCATED_SSE: &str = "\
event: message_start\n\
data: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":1,\"output_tokens\":0}}}\n\n\
event: content_block_delta\n\
data: {\"type\":\"content_block_delta\",\"delta\":{\"type\":\"text_delta\",\"text\":\"partial\"}}\n\n";

#[derive(Clone)]
struct ProviderHarness {
    redirect_url: String,
    credential_sink_hits: Arc<AtomicUsize>,
}

fn openbao_addr() -> Option<String> {
    std::env::var("WSF_OPENBAO_ADDR").ok()
}

fn root_token() -> String {
    std::env::var("WSF_OPENBAO_TOKEN").unwrap_or_else(|_| "root".to_string())
}

async fn bao(
    client: &Client,
    addr: &str,
    token: &str,
    method: Method,
    path: &str,
    body: Option<Value>,
) -> String {
    let mut request = client
        .request(method, format!("{addr}/v1/{path}"))
        .header("X-Vault-Token", token);
    if let Some(body) = body {
        request = request.json(&body);
    }
    request
        .send()
        .await
        .expect("OpenBao request")
        .text()
        .await
        .unwrap_or_default()
}

async fn provision(client: &Client, addr: &str, token: &str) -> (String, String) {
    let _ = bao(
        client,
        addr,
        token,
        Method::POST,
        "sys/auth/approle",
        Some(json!({"type": "approle"})),
    )
    .await;
    let _ = bao(
        client,
        addr,
        token,
        Method::POST,
        "sys/mounts/kv",
        Some(json!({"type": "kv", "options": {"version": "2"}})),
    )
    .await;
    let policy = "path \"kv/data/aog/*\" { capabilities=[\"create\",\"update\",\"read\"] }";
    bao(
        client,
        addr,
        token,
        Method::PUT,
        &format!("sys/policies/acl/{ROLE}"),
        Some(json!({"policy": policy})),
    )
    .await;
    bao(
        client,
        addr,
        token,
        Method::POST,
        &format!("auth/approle/role/{ROLE}"),
        Some(json!({"token_policies": format!("default,{ROLE}"), "token_ttl": "15m"})),
    )
    .await;

    let role: Value = serde_json::from_str(
        &bao(
            client,
            addr,
            token,
            Method::GET,
            &format!("auth/approle/role/{ROLE}/role-id"),
            None,
        )
        .await,
    )
    .expect("role id JSON");
    let secret: Value = serde_json::from_str(
        &bao(
            client,
            addr,
            token,
            Method::POST,
            &format!("auth/approle/role/{ROLE}/secret-id"),
            Some(json!({})),
        )
        .await,
    )
    .expect("secret id JSON");
    (
        role["data"]["role_id"].as_str().unwrap().to_string(),
        secret["data"]["secret_id"].as_str().unwrap().to_string(),
    )
}

fn token(
    signer: &RustCryptoMlDsa87,
    token_id: &str,
    tenant_id: &str,
    token_cap: u64,
) -> TrustToken {
    let now = Utc::now();
    fabric_token::issue(
        TrustToken {
            token_id: token_id.to_string(),
            issued_at: now.to_rfc3339(),
            expires_at: (now + Duration::minutes(15)).to_rfc3339(),
            issuer: "wsf-trust-bridge".to_string(),
            trust_bundle_version: "2026.07.g9-official".to_string(),
            tenant_id: tenant_id.to_string(),
            subject_id: None,
            subject_hash: format!("hmac-sha256:{token_id}"),
            service_identity: None,
            identity_id: None,
            roles: vec![],
            compliance_scopes: vec![],
            allowed_routes: vec![Route::CloudAllowed],
            allowed_models: vec!["gpt-4o-mini".into(), "claude-3-5-sonnet".into()],
            max_data_classification: Classification::Restricted,
            country: None,
            person_type: None,
            offline_mode: false,
            revocation_status: RevocationStatus::Valid,
            budget: Some(Budget {
                token_cap,
                ..Budget::default()
            }),
            attenuation: Attenuation::default(),
            signature: Signature {
                alg: String::new(),
                key_id: String::new(),
                value: String::new(),
            },
        },
        signer,
    )
    .expect("issue G9 token")
}

fn key_path(virtual_key: &str) -> String {
    format!(
        "{KV_PREFIX}/{}",
        hex::encode(Sha256::digest(virtual_key.as_bytes()))
    )
}

async fn seed_key(openbao: &OpenBaoAuth, vault_token: &str, key: &str, trust_token: TrustToken) {
    openbao
        .put_kv_data(vault_token, &key_path(key), json!({"token": trust_token}))
        .await
        .expect("seed G9 virtual key");
}

async fn put_snapshot(addr: &str, client: &Client, token: &str, snapshot: &RevocationSnapshot) {
    bao(
        client,
        addr,
        token,
        Method::POST,
        REVOCATION_PATH,
        Some(json!({"data": {"snapshot": snapshot}})),
    )
    .await;
}

fn prompt(body: &Value) -> String {
    body.get("prompt")
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| body["messages"].to_string())
}

fn oversized_response() -> Response {
    let mut response = Response::new(Body::empty());
    response
        .headers_mut()
        .insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    response.headers_mut().insert(
        CONTENT_LENGTH,
        HeaderValue::from_str(&(MAX_JSON_BODY + 1).to_string()).unwrap(),
    );
    response
}

async fn openai_upstream(
    State(state): State<ProviderHarness>,
    Json(body): Json<Value>,
) -> Response {
    let prompt = prompt(&body);
    if prompt.contains("FAULT_REDIRECT") {
        return (
            StatusCode::TEMPORARY_REDIRECT,
            [(LOCATION, state.redirect_url)],
        )
            .into_response();
    }
    if prompt.contains("FAULT_MALFORMED") {
        return ([(CONTENT_TYPE, "application/json")], "not-json").into_response();
    }
    if prompt.contains("FAULT_OVERSIZED") {
        return oversized_response();
    }
    if body["stream"].as_bool().unwrap_or(false) {
        let body = if prompt.contains("FAULT_TRUNCATED") {
            OPENAI_TRUNCATED_SSE
        } else {
            OPENAI_SSE
        };
        return ([(CONTENT_TYPE, "text/event-stream")], body).into_response();
    }
    let (content, input, output) = if prompt.contains("FALSE_USAGE") {
        ("x".repeat(32), 1, 1)
    } else if prompt.contains("BUDGET") {
        ("ok".to_string(), 1, 1)
    } else {
        ("gateway-ok".to_string(), 20, 20)
    };
    Json(json!({
        "model": "upstream-openai",
        "choices": [{"message": {"content": content}, "finish_reason": "stop"}],
        "usage": {"prompt_tokens": input, "completion_tokens": output}
    }))
    .into_response()
}

async fn anthropic_upstream(
    State(_state): State<ProviderHarness>,
    Json(body): Json<Value>,
) -> Response {
    let prompt = prompt(&body);
    if prompt.contains("FAULT_MALFORMED") {
        return ([(CONTENT_TYPE, "application/json")], "not-json").into_response();
    }
    if body["stream"].as_bool().unwrap_or(false) {
        let body = if prompt.contains("FAULT_TRUNCATED") {
            ANTHROPIC_TRUNCATED_SSE
        } else {
            ANTHROPIC_SSE
        };
        return ([(CONTENT_TYPE, "text/event-stream")], body).into_response();
    }
    Json(json!({
        "model": "upstream-anthropic",
        "content": [{"type": "text", "text": "gateway-ok"}],
        "stop_reason": "end_turn",
        "usage": {"input_tokens": 20, "output_tokens": 20}
    }))
    .into_response()
}

async fn credential_sink(State(state): State<ProviderHarness>) -> StatusCode {
    state.credential_sink_hits.fetch_add(1, Ordering::SeqCst);
    StatusCode::NO_CONTENT
}

async fn spawn(app: Router) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    base
}

async fn run_official_clients(base: &str, phase: &str) {
    let python = std::env::var("AOG_G9_PYTHON").unwrap_or_else(|_| {
        if cfg!(windows) {
            "python".to_string()
        } else {
            "python3".to_string()
        }
    });
    let script = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("official_client_probe.py");
    let local_sdk = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("target")
        .join("g9-official-clients");
    let mut command = Command::new(python);
    command
        .arg(script)
        .env("AOG_G9_BASE_URL", base)
        .env("AOG_G9_PHASE", phase);
    if local_sdk.is_dir() {
        command.env("PYTHONPATH", local_sdk);
    }
    let output = command.output().await.expect("launch official SDK probe");
    assert!(
        output.status.success(),
        "official SDK phase {phase} failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    eprint!("{}", String::from_utf8_lossy(&output.stdout));
}

#[tokio::test]
async fn official_sdks_survive_the_adversarial_gateway_matrix() {
    let Some(addr) = openbao_addr() else {
        eprintln!(
            "SKIP official_sdks_survive_the_adversarial_gateway_matrix: \
             WSF_OPENBAO_ADDR unset (LSH-G9 live gate)"
        );
        return;
    };

    let client = Client::new();
    let root = root_token();
    let (role_id, secret_id) = provision(&client, &addr, &root).await;
    let anchor = RustCryptoMlDsa87::generate("aog-g9-official-anchor").unwrap();
    let openbao = OpenBaoAuth::new(OpenBaoConfig::new(&addr, role_id, secret_id)).unwrap();
    let vault_token = openbao.login().await.expect("OpenBao login");

    bao(
        &client,
        &addr,
        &root,
        Method::DELETE,
        &REVOCATION_PATH.replacen("/data/", "/metadata/", 1),
        None,
    )
    .await;

    seed_key(
        &openbao,
        &vault_token,
        "vk_g9_tenant_a",
        token(&anchor, "tok-g9-a", "tenant-a", 1_000_000),
    )
    .await;
    seed_key(
        &openbao,
        &vault_token,
        "vk_g9_tenant_b",
        token(&anchor, "tok-g9-b", "tenant-b", 1_000_000),
    )
    .await;
    seed_key(
        &openbao,
        &vault_token,
        "vk_g9_budget",
        token(&anchor, "tok-g9-budget", "tenant-a", 3),
    )
    .await;
    seed_key(
        &openbao,
        &vault_token,
        "vk_g9_revoke",
        token(&anchor, "tok-g9-revoke", "tenant-b", 1_000_000),
    )
    .await;

    let now = Utc::now();
    let baseline = RevocationSnapshot::new(
        "snap-g9-official-baseline",
        now.to_rfc3339(),
        (now + Duration::hours(1)).to_rfc3339(),
    )
    .with_sequence(1);
    let signed_baseline = fabric_revocation::sign(baseline, &anchor).unwrap();
    put_snapshot(&addr, &client, &root, &signed_baseline).await;

    let sink_hits = Arc::new(AtomicUsize::new(0));
    let placeholder_state = ProviderHarness {
        redirect_url: String::new(),
        credential_sink_hits: sink_hits.clone(),
    };
    let upstream_base = spawn(
        Router::new()
            .route("/v1/chat/completions", post(openai_upstream))
            .route("/v1/messages", post(anthropic_upstream))
            .route("/credential-sink", get(credential_sink))
            .with_state(placeholder_state.clone()),
    )
    .await;

    // Rebuild the upstream with its now-known credential-sink URL. The first
    // listener is intentionally dropped from use; no credentials are sent to it.
    let harness = ProviderHarness {
        redirect_url: format!("{upstream_base}/credential-sink"),
        credential_sink_hits: sink_hits.clone(),
    };
    let provider_base = spawn(
        Router::new()
            .route("/v1/chat/completions", post(openai_upstream))
            .route("/v1/messages", post(anthropic_upstream))
            .route("/credential-sink", get(credential_sink))
            .with_state(harness),
    )
    .await;

    let endpoint =
        aog_gateway::posture::ApprovedProviderEndpoint::loopback_fixture(&provider_base).unwrap();
    let mut registry = Registry::new();
    registry.register(Arc::new(OpenAiProvider::new(
        "openai",
        endpoint.clone(),
        "openai-provider-secret",
    )));
    registry.register(Arc::new(AnthropicProvider::new(
        endpoint,
        "anthropic-provider-secret",
    )));

    let gateway = Arc::new(
        Gateway::new_production(
            openbao,
            GatewayConfig {
                token_public_key: anchor.public_key().to_vec(),
                virtual_key_kv_prefix: KV_PREFIX.to_string(),
            },
            REVOCATION_PATH,
        )
        .await
        .expect("production gateway loads baseline revocation"),
    );
    let models = ModelMap::new()
        .route("gpt-4o-mini", Target::new("openai", "upstream-openai"))
        .route(
            "claude-3-5-sonnet",
            Target::new("anthropic", "upstream-anthropic"),
        );
    let state = AppState::new(gateway, Arc::new(registry), Arc::new(models));
    let receipts = state.receipts.clone();
    let app = aog_gateway::surface_openai::router(state.clone())
        .merge(aog_gateway::surface_anthropic::router(state));
    let gateway_base = spawn(app).await;

    run_official_clients(&gateway_base, "matrix").await;
    assert_eq!(
        sink_hits.load(Ordering::SeqCst),
        0,
        "redirect target never receives provider credentials"
    );

    {
        let ledger = receipts.lock().unwrap_or_else(|error| error.into_inner());
        assert!(
            ledger
                .aggregate_for_tenant("tenant-a")
                .iter()
                .any(|usage| usage.calls > 0),
            "tenant A has isolated usage"
        );
        assert!(
            ledger
                .aggregate_for_tenant("tenant-b")
                .iter()
                .any(|usage| usage.calls > 0),
            "tenant B has isolated usage"
        );
        let false_usage = ledger
            .receipts()
            .iter()
            .find(|receipt| receipt.workflow_id.as_deref() == Some("g9-false-usage"))
            .expect("false-usage call is receipted");
        let reconciliation = false_usage
            .usage_reconciliation
            .expect("false-usage receipt carries reconciliation");
        assert_eq!(reconciliation.provider_reported.output_tokens, 1);
        assert_eq!(reconciliation.local_estimate.output_tokens, 8);
        assert_eq!(reconciliation.final_usage.output_tokens, 8);
        assert!(ledger.verify(), "adversarial matrix receipt chain verifies");
    }

    let mut revoked = RevocationSnapshot::new(
        "snap-g9-official-revoked",
        now.to_rfc3339(),
        (now + Duration::hours(1)).to_rfc3339(),
    )
    .with_sequence(2);
    revoked.revoked_tokens.push("tok-g9-revoke".to_string());
    let signed_revoked = fabric_revocation::sign(revoked, &anchor).unwrap();
    put_snapshot(&addr, &client, &root, &signed_revoked).await;
    run_official_clients(&gateway_base, "revoked").await;

    eprintln!(
        "LSH-G9 live gate PASSED: official OpenAI + Anthropic SDKs, five surfaces, \
         two tenants, route deny, atomic budget, live revocation, redirect isolation, \
         bounded response faults, and authoritative false-usage reconciliation"
    );
}
