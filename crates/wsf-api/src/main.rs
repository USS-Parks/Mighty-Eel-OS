//! `wsf-api` binary — serves the WSF trust-plane REST API (appliance / D1).
//!
//! Reads its OpenBao AppRole creds + the persistent ML-DSA key material from the
//! environment. The one-shot `wsf-seed` binary provisions OpenBao, mints those
//! keys, and writes a shared env file the service container sources before exec.
#![allow(clippy::print_stdout, clippy::print_stderr)]

use std::net::ToSocketAddrs;
use std::sync::{Arc, Mutex, RwLock};

use base64::Engine;
use fabric_contracts::Audience;
use fabric_crypto::providers::{MlDsa87Verifier, RustCryptoMlDsa87};
use wsf_api::auth::{LocalDevAuthenticator, WorkloadAuthenticator, WsfAuthenticator};
use wsf_api::{AppState, RevocationEnforcement};
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

    let profile = wsf_api::posture::Profile::parse(std::env::var("WSF_PROFILE").ok().as_deref())?;

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
    // The privileged trust plane must not bind a public interface by default:
    // loopback unless an operator explicitly widens WSF_LISTEN behind an
    // authenticated ingress. The demo/appliance composes set WSF_LISTEN themselves.
    let listen = env_or("WSF_LISTEN", "127.0.0.1:8300");

    // P1 production startup posture. A public (non-loopback) bind must present a
    // real workload-credential authority key AND a hardened OpenBao/HMAC config,
    // or we refuse to start — the local-dev authenticator and dev fixtures never
    // answer a public interface. A loopback bind stays unrestricted (host-only).
    let resolved: Vec<std::net::SocketAddr> = listen
        .to_socket_addrs()
        .map_err(|e| format!("WSF_LISTEN '{listen}' did not resolve: {e}"))?
        .collect();
    let public_bind = wsf_api::posture::is_public_bind(&resolved);
    let workload_key = std::env::var("WSF_WORKLOAD_AUTHORITY_KEY").ok();
    let allow_insecure_development_bind =
        std::env::var("WSF_ALLOW_INSECURE_BIND").ok().as_deref() == Some("1");

    let revocation = if profile == wsf_api::posture::Profile::Production {
        let path = env_or("WSF_REVOCATION_PATH", "kv/data/revocation/current");
        let openbao = new_openbao()?;
        let vault_token = openbao
            .login()
            .await
            .map_err(|e| format!("load revocation login: {e}"))?;
        let data = openbao
            .get_kv_data(&vault_token, &path)
            .await
            .map_err(|e| format!("load mandatory revocation snapshot {path}: {e}"))?;
        let snapshot: fabric_revocation::RevocationSnapshot = serde_json::from_value(
            data.get("snapshot")
                .cloned()
                .ok_or_else(|| format!("mandatory revocation record {path} lacks snapshot"))?,
        )
        .map_err(|e| format!("decode mandatory revocation snapshot {path}: {e}"))?;
        let mut store = fabric_revocation::MonotonicRevocationStore::new();
        store
            .advance(snapshot, &MlDsa87Verifier, &anchor)
            .map_err(|e| format!("verify mandatory revocation snapshot {path}: {e}"))?;
        RevocationEnforcement::required(Arc::new(RwLock::new(store)))
    } else {
        RevocationEnforcement::development_disabled()
    };
    wsf_api::posture::enforce_startup_posture(
        profile,
        public_bind,
        workload_key.is_some(),
        revocation.required_store().is_some(),
        allow_insecure_development_bind,
        &wsf_hardening::DeploymentConfig {
            mode: wsf_hardening::DeployMode::Production,
            openbao_address: addr.clone(),
            openbao_token: secret_id.clone(),
            subject_hmac_key: hmac.clone(),
        },
    )?;

    // A2 authenticator: a workload-credential authority key selects the
    // production authenticator; without it we fall back to the explicit
    // local-dev principal — which the P1 posture above confines to a loopback bind.
    let authenticator: Arc<dyn WsfAuthenticator> = match workload_key {
        Some(k) => {
            let key = base64::engine::general_purpose::STANDARD
                .decode(k.trim())
                .map_err(|e| format!("WSF_WORKLOAD_AUTHORITY_KEY not base64: {e}"))?;
            let mut a = WorkloadAuthenticator::new(Box::new(MlDsa87Verifier), key, Audience::Wsf);
            if let Ok(t) = std::env::var("WSF_INGRESS_TENANT") {
                a = a.bound_to_tenant(t);
            }
            println!("wsf-api: workload-credential authenticator (audience=wsf)");
            Arc::new(a)
        }
        None => {
            let tenant = env_or("WSF_DEV_TENANT", "local-dev-tenant");
            println!("wsf-api: LOCAL-DEV authenticator (tenant={tenant}) — not for production");
            Arc::new(LocalDevAuthenticator::for_wsf(tenant))
        }
    };

    // A3 issuance policy. Production loads signed / OpenBao-held tenant mappings
    // (extended in B2); the dev fallback grants a small role set to the dev
    // tenant so the loopback surface is usable without inventing authority.
    let issuance_policy: Arc<dyn wsf_api::policy::TenantPolicyStore> = {
        let tenant = env_or("WSF_DEV_TENANT", "local-dev-tenant");
        Arc::new(wsf_api::policy::StaticTenantPolicies::single_dev(
            tenant,
            &["user", "clinician"],
        ))
    };

    // B1/B2 cloud grants. Production loads signed / OpenBao-custodied mappings;
    // the dev fallback maps a single grant id to a role ARN if configured.
    let cloud_grants: Arc<dyn wsf_api::grants::GrantStore> = {
        let tenant = env_or("WSF_DEV_TENANT", "local-dev-tenant");
        match (
            std::env::var("WSF_DEV_GRANT_ID"),
            std::env::var("WSF_DEV_GRANT_ROLE_ARN"),
        ) {
            (Ok(gid), Ok(arn)) => {
                Arc::new(wsf_api::grants::StaticGrants::single_dev(tenant, gid, arn))
            }
            _ => Arc::new(wsf_api::grants::StaticGrants::new()),
        }
    };

    let broker = AwsStsBroker::new(
        new_openbao()?,
        reqwest::Client::new(),
        BrokerConfig::new(region, aws_endpoint, cred_path),
    );
    let seal = SealService::new(
        new_openbao()?,
        seal_signer,
        SealServiceConfig {
            transit_key,
            token_public_key: anchor.clone(),
        },
    );
    let broker = match revocation.required_store() {
        Some(store) => broker.with_revocation_store(store),
        None => broker,
    };
    let seal = match revocation.required_store() {
        Some(store) => seal.with_revocation_store(store),
        None => seal,
    };

    let state = AppState {
        bridge: Arc::new(TrustBridge::new(
            new_openbao()?,
            bridge_signer,
            BridgeConfig::new(bundle, hmac),
        )),
        broker: Arc::new(broker),
        seal: Arc::new(seal),
        ledger: Arc::new(Mutex::new(Ledger::new(ledger_signer))),
        token_public_key: Arc::new(anchor),
        auth: authenticator,
        policy: issuance_policy,
        grants: cloud_grants,
        // L2: no global auditors unless explicitly enrolled (safe default).
        auditors: Arc::new(wsf_api::audit::StaticAuditors::none()),
        revocation,
        attenuation: Arc::new(wsf_api::AttenuationState::new()),
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
