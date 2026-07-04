//! `wsf-api` binary — serves the WSF trust-plane REST API (appliance / D1).
//!
//! Reads its OpenBao AppRole creds + the persistent ML-DSA key material from the
//! environment. The one-shot `wsf-seed` binary provisions OpenBao, mints those
//! keys, and writes a shared env file the service container sources before exec.
#![allow(clippy::print_stdout, clippy::print_stderr)]

use std::sync::{Arc, Mutex};

use base64::Engine;
use fabric_crypto::providers::RustCryptoMlDsa87;
use wsf_api::AppState;
use wsf_bridge::{BridgeConfig, OpenBaoAuth, OpenBaoConfig, TrustBridge};
use wsf_broker::{AwsStsBroker, BrokerConfig};
use wsf_ledger::Ledger;
use wsf_seal::{SealService, SealServiceConfig};

fn env(key: &str) -> Result<String, String> {
    std::env::var(key).map_err(|_| format!("missing required env {key}"))
}
fn env_or(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_string())
}
fn b64(key: &str) -> Result<Vec<u8>, String> {
    base64::engine::general_purpose::STANDARD
        .decode(env(key)?)
        .map_err(|e| format!("{key} is not valid base64: {e}"))
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
        eprintln!("wsf-api: {e}");
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

    let addr = env("WSF_OPENBAO_ADDR")?;
    let role_id = env("WSF_OPENBAO_ROLE_ID")?;
    let secret_id = env("WSF_OPENBAO_SECRET_ID")?;
    let new_openbao = || {
        OpenBaoAuth::new(OpenBaoConfig::new(
            &addr,
            role_id.clone(),
            secret_id.clone(),
        ))
        .map_err(|e| format!("openbao config: {e}"))
    };

    let bridge_pk = b64("WSF_BRIDGE_PK")?;
    let bridge_sk = b64("WSF_BRIDGE_SK")?;
    let seal_pk = b64("WSF_SEAL_PK")?;
    let seal_sk = b64("WSF_SEAL_SK")?;
    let ledger_pk = b64("WSF_LEDGER_PK")?;
    let ledger_sk = b64("WSF_LEDGER_SK")?;
    let hmac =
        hex::decode(env("WSF_SUBJECT_HMAC_KEY")?).map_err(|e| format!("hmac not hex: {e}"))?;
    let anchor = bridge_pk.clone();

    let bridge_signer = Arc::new(
        RustCryptoMlDsa87::from_keypair("wsf-bridge", bridge_pk, bridge_sk)
            .map_err(|e| format!("bridge key: {e}"))?,
    );
    let seal_signer = Arc::new(
        RustCryptoMlDsa87::from_keypair("wsf-seal", seal_pk, seal_sk)
            .map_err(|e| format!("seal key: {e}"))?,
    );
    let ledger_signer = Arc::new(
        RustCryptoMlDsa87::from_keypair("wsf-ledger", ledger_pk, ledger_sk)
            .map_err(|e| format!("ledger key: {e}"))?,
    );

    let bundle = env_or("WSF_BUNDLE_VERSION", "2026.07.appliance");
    let transit_key = env_or("WSF_TRANSIT_KEY", "appliance-dek");
    let region = env_or("WSF_AWS_REGION", "us-east-1");
    let aws_endpoint = env_or("WSF_AWS_ENDPOINT", "https://sts.amazonaws.com");
    let cred_path = env_or("WSF_BROKER_CRED_PATH", "kv/data/broker/aws-root");
    let listen = env_or("WSF_LISTEN", "0.0.0.0:8300");

    let state = AppState {
        bridge: Arc::new(TrustBridge::new(
            new_openbao()?,
            bridge_signer,
            BridgeConfig::new(bundle, hmac),
        )),
        broker: Arc::new(AwsStsBroker::new(
            new_openbao()?,
            reqwest::Client::new(),
            BrokerConfig::new(region, aws_endpoint, cred_path),
        )),
        seal: Arc::new(SealService::new(
            new_openbao()?,
            seal_signer,
            SealServiceConfig {
                transit_key,
                token_public_key: anchor.clone(),
            },
        )),
        ledger: Arc::new(Mutex::new(Ledger::new(ledger_signer))),
        token_public_key: Arc::new(anchor),
    };

    let app = wsf_api::router(state);
    let listener = tokio::net::TcpListener::bind(&listen)
        .await
        .map_err(|e| format!("bind {listen}: {e}"))?;
    println!("wsf-api serving on {listen}");
    axum::serve(listener, app)
        .await
        .map_err(|e| format!("serve: {e}"))?;
    Ok(())
}
