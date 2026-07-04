//! `aog-gateway` binary — serves the AOG estate model gateway (appliance / D1).
//!
//! Reads its OpenBao AppRole creds + the WSF trust anchor (base64) from the
//! environment (the `wsf-seed` step writes them). Registers a local
//! OpenAI-compatible backend, plus cloud OpenAI / Anthropic when keyed, and
//! serves in the configured mode (`shadow` by default — the M1 posture).
#![allow(clippy::print_stdout, clippy::print_stderr)]

use std::sync::Arc;

use aog_gateway::app::{AppState, ModelMap, Target};
use aog_gateway::policy::PolicyMode;
use aog_gateway::provider::Registry;
use aog_gateway::provider::anthropic::AnthropicProvider;
use aog_gateway::provider::openai::OpenAiProvider;
use aog_gateway::{Gateway, GatewayConfig};
use base64::Engine;
use wsf_bridge::{OpenBaoAuth, OpenBaoConfig};

fn env(key: &str) -> Result<String, String> {
    std::env::var(key).map_err(|_| format!("missing required env {key}"))
}
fn env_or(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_string())
}

fn main() {
    // ML-DSA-87 + reqwest/hyper futures use large stack frames; run the runtime
    // on a generous stack so this holds on Windows (~1 MB default main stack) too.
    let worker = std::thread::Builder::new()
        .stack_size(16 * 1024 * 1024)
        .spawn(|| {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("tokio runtime")
                .block_on(run())
        })
        .expect("spawn runtime thread");
    if let Err(e) = worker.join().expect("runtime thread panicked") {
        eprintln!("aog-gateway: {e}");
        std::process::exit(1);
    }
}

async fn run() -> Result<(), String> {
    let sub = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "serve".to_string());
    if sub != "serve" {
        return Err(format!("unknown subcommand '{sub}' (expected: serve)"));
    }

    let addr = env("AOG_OPENBAO_ADDR")?;
    let role_id = env("AOG_OPENBAO_ROLE_ID")?;
    let secret_id = env("AOG_OPENBAO_SECRET_ID")?;
    let anchor = base64::engine::general_purpose::STANDARD
        .decode(env("AOG_TOKEN_ANCHOR")?)
        .map_err(|e| format!("AOG_TOKEN_ANCHOR not base64: {e}"))?;
    let vk_prefix = env_or("AOG_VIRTUAL_KEY_PREFIX", "kv/data/aog/virtual-keys");
    let listen = env_or("AOG_LISTEN", "0.0.0.0:8080");
    let mode_str = env_or("AOG_MODE", "shadow");
    let mode = PolicyMode::parse(&mode_str).ok_or_else(|| format!("bad AOG_MODE '{mode_str}'"))?;

    let openbao = OpenBaoAuth::new(OpenBaoConfig::new(&addr, role_id, secret_id))
        .map_err(|e| format!("openbao config: {e}"))?;
    let gateway = Arc::new(Gateway::new(
        openbao,
        GatewayConfig {
            token_public_key: anchor,
            virtual_key_kv_prefix: vk_prefix,
        },
    ));

    // Providers: always a local OpenAI-compatible backend; cloud providers when keyed.
    let local_base = env_or("AOG_LOCAL_BASE", "http://mock-llm:8000");
    let mut registry = Registry::new();
    registry.register(Arc::new(OpenAiProvider::local(local_base)));
    let mut models = ModelMap::new()
        .route("demo", Target::new("local", "demo"))
        .default_target(Target::new("local", "demo"));

    if let Ok(key) = std::env::var("AOG_OPENAI_KEY")
        && !key.is_empty()
    {
        let base = env_or("AOG_OPENAI_BASE", "https://api.openai.com");
        registry.register(Arc::new(OpenAiProvider::new("openai", base, key)));
        models = models
            .route("gpt-4o-mini", Target::new("openai", "gpt-4o-mini"))
            .route("gpt-4o", Target::new("openai", "gpt-4o"));
    }
    if let Ok(key) = std::env::var("AOG_ANTHROPIC_KEY")
        && !key.is_empty()
    {
        let base = env_or("AOG_ANTHROPIC_BASE", "https://api.anthropic.com");
        registry.register(Arc::new(AnthropicProvider::new(base, key)));
        models = models.route(
            "claude-3-5-sonnet",
            Target::new("anthropic", "claude-3-5-sonnet"),
        );
    }

    let state = AppState::new(gateway, Arc::new(registry), Arc::new(models)).with_mode(mode);

    // Merge the inference surfaces (OpenAI + Anthropic) with the auth skeleton
    // (`/healthz`, `/v1/preflight`).
    let app = aog_gateway::surface_openai::router(state.clone())
        .merge(aog_gateway::surface_anthropic::router(state.clone()))
        .merge(aog_gateway::http::router(state.gateway.clone()));

    let listener = tokio::net::TcpListener::bind(&listen)
        .await
        .map_err(|e| format!("bind {listen}: {e}"))?;
    println!("aog-gateway serving on {listen} (mode={mode_str})");
    axum::serve(listener, app)
        .await
        .map_err(|e| format!("serve: {e}"))?;
    Ok(())
}
