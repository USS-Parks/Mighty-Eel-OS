//! `aog-gateway` binary — serves the AOG estate model gateway (appliance / D1).
//!
//! Reads its OpenBao AppRole creds + the WSF trust anchor (base64) from the
//! environment (the `wsf-seed` step writes them). Registers a local
//! OpenAI-compatible backend, plus cloud OpenAI / Anthropic when keyed, and
//! serves in the configured mode (`enforce` by default — fail-closed; the
//! non-blocking `shadow`/`report_only` modes require `AOG_PROFILE=development`).
#![allow(clippy::print_stdout, clippy::print_stderr)]

use std::sync::Arc;

use aog_gateway::app::{AppState, ModelMap, Target};
use aog_gateway::policy::{Profile, resolve_mode};
use aog_gateway::posture::{ProviderEndpoint, enforce_startup_posture};
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

    // Fail-closed policy posture resolved FIRST — before any external dependency
    // (OpenBao) or listener bind. Production defaults to enforce and rejects the
    // non-blocking modes; shadow/report_only require AOG_PROFILE=development.
    let profile =
        Profile::parse(std::env::var("AOG_PROFILE").ok().as_deref()).map_err(|e| e.to_string())?;
    let mode = resolve_mode(profile, std::env::var("AOG_MODE").ok().as_deref())
        .map_err(|e| e.to_string())?;

    let revocation_path = env_or("AOG_REVOCATION_PATH", "kv/data/aog/revocation");
    let local_base = env_or("AOG_LOCAL_BASE", "http://127.0.0.1:8000");
    let openai_key = std::env::var("AOG_OPENAI_KEY")
        .ok()
        .filter(|key| !key.is_empty());
    let openai_base = env_or("AOG_OPENAI_BASE", "https://api.openai.com");
    let anthropic_key = std::env::var("AOG_ANTHROPIC_KEY")
        .ok()
        .filter(|key| !key.is_empty());
    let anthropic_base = env_or("AOG_ANTHROPIC_BASE", "https://api.anthropic.com");
    let mut provider_endpoints = vec![ProviderEndpoint {
        name: "local",
        base_url: &local_base,
        credentialed: false,
        local: true,
    }];
    if openai_key.is_some() {
        provider_endpoints.push(ProviderEndpoint {
            name: "openai",
            base_url: &openai_base,
            credentialed: true,
            local: false,
        });
    }
    if anthropic_key.is_some() {
        provider_endpoints.push(ProviderEndpoint {
            name: "anthropic",
            base_url: &anthropic_base,
            credentialed: true,
            local: false,
        });
    }
    enforce_startup_posture(profile, &revocation_path, &provider_endpoints)?;

    let addr = env("AOG_OPENBAO_ADDR")?;
    let role_id = env("AOG_OPENBAO_ROLE_ID")?;
    let secret_id = env("AOG_OPENBAO_SECRET_ID")?;
    let anchor = base64::engine::general_purpose::STANDARD
        .decode(env("AOG_TOKEN_ANCHOR")?)
        .map_err(|e| format!("AOG_TOKEN_ANCHOR not base64: {e}"))?;
    let vk_prefix = env_or("AOG_VIRTUAL_KEY_PREFIX", "kv/data/aog/virtual-keys");
    let listen = env_or("AOG_LISTEN", "127.0.0.1:8080");

    let openbao = OpenBaoAuth::new(OpenBaoConfig::new(&addr, role_id, secret_id))
        .map_err(|e| format!("openbao config: {e}"))?;
    let gateway = Arc::new(
        Gateway::new(
            openbao,
            GatewayConfig {
                token_public_key: anchor,
                virtual_key_kv_prefix: vk_prefix,
            },
        )
        .with_revocation_path(revocation_path),
    );

    // Providers: always a local OpenAI-compatible backend; cloud providers when keyed.
    let mut registry = Registry::new();
    registry.register(Arc::new(OpenAiProvider::local(local_base)));
    let mut models = ModelMap::new()
        .route("demo", Target::new("local", "demo"))
        .default_target(Target::new("local", "demo"));

    if let Some(key) = openai_key {
        registry.register(Arc::new(OpenAiProvider::new("openai", openai_base, key)));
        models = models
            .route("gpt-4o-mini", Target::new("openai", "gpt-4o-mini"))
            .route("gpt-4o", Target::new("openai", "gpt-4o"));
    }
    if let Some(key) = anthropic_key {
        registry.register(Arc::new(AnthropicProvider::new(anthropic_base, key)));
        models = models.route(
            "claude-3-5-sonnet",
            Target::new("anthropic", "claude-3-5-sonnet"),
        );
    }

    let state = AppState::new(gateway, Arc::new(registry), Arc::new(models))
        .with_mode(mode)
        .with_profile(profile);

    // Merge the inference surfaces (OpenAI + Anthropic) with the auth skeleton
    // (`/healthz`, `/v1/preflight`).
    let app = aog_gateway::surface_openai::router(state.clone())
        .merge(aog_gateway::surface_anthropic::router(state.clone()))
        .merge(aog_gateway::http::router(state.gateway.clone()));

    let listener = tokio::net::TcpListener::bind(&listen)
        .await
        .map_err(|e| format!("bind {listen}: {e}"))?;
    println!(
        "aog-gateway serving on {listen} (profile={}, mode={})",
        profile.header(),
        mode.header()
    );
    axum::serve(listener, app)
        .await
        .map_err(|e| format!("serve: {e}"))?;
    Ok(())
}
