//! X3 gate — "tool calls + metering continue across the cutover; receipts
//! unbroken" — against **live** OpenBao, the **real** `aog-gateway` (whose
//! process carries the meter), and the **real** `aog-toolproxy` (A3.2
//! no-mock-only).
//!
//! Before the cutover, a client completes a metered chat through the gateway
//! (one meter receipt) and an agent brokers a tool call through the proxy (one
//! tool receipt). Both services are then brought under Loom management —
//! declared as `Workload`s (kinds `Gateway` and `Toolproxy`), bound by
//! `Placement`s, and reconciled to `Ready` by one `WorkloadController` probing
//! their live `/healthz`. The same chat and the same tool call succeed
//! afterward, and **both receipt chains verify unbroken end to end** —
//! management touches the estate, never the data path. Metering has no kind of
//! its own: it rides inside the gateway process, so the gateway workload
//! carries it and the meter's chain continuity is the metering proof.
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
use aog_toolproxy::{InvokeContext, MintedCredential, ToolExecutor, ToolProxy};
use async_trait::async_trait;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::Utc;
use fabric_contracts::{
    Attenuation, Budget, Classification, RevocationStatus, Signature, TrustToken,
};
use fabric_crypto::Signer;
use fabric_crypto::providers::RustCryptoMlDsa87;
use mai_agent::types::{ToolAccessRole, ToolCall, ToolDefinition, ToolResult};
use reqwest::{Client, Method};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio::net::TcpListener;
use wsf_bridge::{OpenBaoAuth, OpenBaoConfig};

const ROLE: &str = "loom-x3";
const KV_PREFIX: &str = "kv/data/aog/virtual-keys";
const VK: &str = "vk_x3";
const WL_GW: &str = "aog-gateway";
const WL_TP: &str = "aog-toolproxy";

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
        "sys/policies/acl/loom-x3",
        Some(json!({ "policy": policy })),
    )
    .await;
    bao(
        c,
        addr,
        tok,
        Method::POST,
        &format!("auth/approle/role/{ROLE}"),
        Some(json!({"token_policies":"default,loom-x3","token_ttl":"15m"})),
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
        token_id: "tok_x3".to_string(),
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

/// The mock OpenAI upstream the gateway dispatches to (usage present, so the
/// completion is metered and receipted).
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

struct EchoExecutor;

#[async_trait]
impl ToolExecutor for EchoExecutor {
    async fn execute(
        &self,
        _tool: &ToolDefinition,
        call: &ToolCall,
        _cred: Option<&MintedCredential>,
    ) -> ToolResult {
        ToolResult {
            call_id: call.call_id.clone(),
            tool_id: call.tool_id.clone(),
            success: true,
            output: call.arguments.clone(),
            error: None,
            duration_ms: 1,
        }
    }
}

fn echo_tool() -> ToolDefinition {
    ToolDefinition {
        id: "echo.say".to_string(),
        name: "echo.say".to_string(),
        description: "echoes its arguments".to_string(),
        parameters_schema: json!({ "type": "object" }),
        return_schema: None,
        has_side_effects: false,
        timeout: Duration::from_secs(5),
        required_role: ToolAccessRole::Guest,
        supports_parallel: false,
    }
}

fn echo_call(call_id: &str) -> ToolCall {
    ToolCall {
        call_id: call_id.to_string(),
        tool_id: "echo.say".to_string(),
        arguments: json!({ "say": "hello" }),
        chain_step: 0,
        parallel_group: None,
    }
}

fn invoke_ctx() -> InvokeContext {
    InvokeContext {
        session_id: "x3-session".to_string(),
        profile_id: "tok_x3".to_string(),
        role: ToolAccessRole::Guest,
        untrusted: false,
        system: None,
        estimated_cost_cents: 0,
    }
}

#[tokio::test]
async fn tool_calls_and_metering_continue_across_the_cutover_receipts_unbroken() {
    let Some(addr) = openbao_addr() else {
        eprintln!(
            "SKIP tool_calls_and_metering_continue_across_the_cutover_receipts_unbroken: WSF_OPENBAO_ADDR unset (X3 live gate)"
        );
        return;
    };
    let c = Client::new();
    let (role_id, secret_id) = provision(&c, &addr, &root_token()).await;

    // Seed an in-budget virtual key the gateway resolves.
    let anchor = RustCryptoMlDsa87::generate("loom-x3-anchor").unwrap();
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

    // The real gateway (its process carries the meter): mock upstream + surface
    // router + a /healthz probe target. Keep a handle on the meter's ledger.
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
    let meter = state.receipts.clone();
    let gw_base =
        spawn(aog_gateway::surface_openai::router(state).route("/healthz", get(|| async { "ok" })))
            .await;

    // The real toolproxy, and the /healthz surface of the process hosting it.
    let proxy = ToolProxy::new();
    proxy.register(echo_tool()).unwrap();
    let tp_base = spawn(Router::new().route("/healthz", get(|| async { "ok" }))).await;

    let http = Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .unwrap();

    // ── Before management: a metered chat and a brokered tool call succeed.
    let (chat_status_before, chat_content_before) = chat(&http, &gw_base).await;
    assert_eq!(chat_status_before, 200, "chat works before management");
    let meter_head_before = {
        let led = meter.lock().unwrap();
        assert_eq!(led.receipts().len(), 1, "the chat was metered");
        assert!(led.verify(), "meter chain verifies before cutover");
        led.head_hex()
    };
    let tool_before = proxy
        .invoke(&echo_call("x3-before"), &invoke_ctx(), &EchoExecutor)
        .await
        .expect("tool call before cutover");
    assert!(tool_before.success, "tool call works before management");
    assert_eq!(proxy.receipts().len(), 1, "the tool call was receipted");

    // ── Bring both services under Loom management.
    let estate = AppState::bootstrap(
        1,
        fresh_dir("loom-x3-estate"),
        Authenticator::new(anchor.public_key().to_vec()),
        Sealer::generate().unwrap(),
    )
    .await
    .unwrap();
    let client = EstateClient::new(estate.admission(), estate.reader());
    client
        .ensure_created(ResourceObject::Node(Resource::new(
            "x3-node-1",
            NodeSpec {
                ring: 1,
                attestation_floor: Classification::Restricted,
                attestation: aog_estate::AttestationProfile::default(),
                capacity: aog_estate::Capacity::default(),
            },
        )))
        .await
        .unwrap();
    for (name, kind) in [
        (WL_GW, WorkloadKind::Gateway),
        (WL_TP, WorkloadKind::Toolproxy),
    ] {
        client
            .ensure_created(ResourceObject::Workload(Resource::new(
                name,
                WorkloadSpec {
                    workload_kind: kind,
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
        client
            .ensure_created(ResourceObject::Placement(Resource::new(
                format!("{name}-place"),
                PlacementSpec {
                    workload: name.to_string(),
                    node: "x3-node-1".to_string(),
                    token_id: String::new(),
                },
            )))
            .await
            .unwrap();
    }

    let probe = Arc::new(
        HttpWorkloadProbe::new(HashMap::from([
            (WL_GW.to_string(), format!("{gw_base}/healthz")),
            (WL_TP.to_string(), format!("{tp_base}/healthz")),
        ]))
        .unwrap(),
    );
    let mut controller = Controller::new(
        "workload",
        estate.informer("Workload/"),
        WorkloadController::new(client.clone(), probe),
        Arc::new(AlwaysLeader),
    );
    settle(&mut controller).await;

    // ── Managed: both workloads Ready, placed, their live replicas reflected —
    // the Toolproxy kind is now managed exactly like the Gateway kind.
    for name in [WL_GW, WL_TP] {
        let Some(ResourceObject::Workload(managed)) =
            client.get(Kind::Workload, name).await.unwrap()
        else {
            panic!("workload {name} missing");
        };
        let status = managed.status.expect("status");
        assert_eq!(status.phase, Phase::Ready, "{name} is Ready");
        assert_eq!(status.ready_replicas, 1, "{name} replica reflected");
        assert_eq!(
            status.placements,
            vec!["x3-node-1".to_string()],
            "{name} placement reflected"
        );
    }

    // ── Across the cutover: the same chat is metered and the same tool call is
    // brokered, and both receipt chains verify unbroken end to end.
    let (chat_status_after, chat_content_after) = chat(&http, &gw_base).await;
    assert_eq!(chat_status_after, chat_status_before);
    assert_eq!(
        chat_content_after, chat_content_before,
        "chat unchanged across cutover"
    );
    {
        let led = meter.lock().unwrap();
        assert_eq!(led.receipts().len(), 2, "metering continued across cutover");
        assert!(
            led.verify(),
            "meter receipt chain unbroken across the cutover"
        );
        assert_ne!(
            led.head_hex(),
            meter_head_before,
            "the post-cutover completion extended the meter chain"
        );
    }

    let tool_after = proxy
        .invoke(&echo_call("x3-after"), &invoke_ctx(), &EchoExecutor)
        .await
        .expect("tool call after cutover");
    assert!(tool_after.success, "tool calls continue across cutover");
    assert_eq!(
        tool_after.output, tool_before.output,
        "tool result unchanged across cutover"
    );
    assert_eq!(proxy.receipts().len(), 2, "both tool calls receipted");
    assert!(
        proxy.verify_receipts(),
        "tool receipt chain unbroken across the cutover"
    );
}
