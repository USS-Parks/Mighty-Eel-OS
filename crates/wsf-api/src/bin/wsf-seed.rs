//! `wsf-seed` — one-shot appliance init (D1).
//!
//! Provisions a dev OpenBao (AppRole + KV + Transit + policy), mints the
//! persistent ML-DSA trust-anchor keypairs, issues a demo trust token, seeds a
//! demo virtual key (`vk_demo` → the signed token) for the AOG gateway, and
//! writes a shared env file the `wsf-api` + `aog-gateway` containers source. It
//! is idempotent enough to re-run against a fresh dev OpenBao.
#![allow(clippy::print_stdout, clippy::print_stderr)]

use std::sync::Arc;

use base64::Engine;
use fabric_contracts::Budget;
use fabric_crypto::providers::RustCryptoMlDsa87;
use reqwest::{Client, Method};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use wsf_bridge::{BridgeConfig, IssueTokenRequest, OpenBaoAuth, OpenBaoConfig, TrustBridge};

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_string())
}
fn b64e(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

async fn bao(
    c: &Client,
    addr: &str,
    token: &str,
    method: Method,
    path: &str,
    body: Option<Value>,
) -> Result<String, String> {
    let url = format!("{addr}/v1/{path}");
    let mut rb = c.request(method, &url).header("X-Vault-Token", token);
    if let Some(b) = body {
        rb = rb
            .header("Content-Type", "application/json")
            .body(b.to_string());
    }
    let resp = rb
        .send()
        .await
        .map_err(|e| format!("openbao {path}: {e}"))?;
    Ok(resp.text().await.unwrap_or_default())
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
        eprintln!("wsf-seed: {e}");
        std::process::exit(1);
    }
}

#[allow(clippy::too_many_lines)]
async fn run() -> Result<(), String> {
    let addr = env_or("WSF_OPENBAO_ADDR", "http://openbao:8200");
    let root = env_or("WSF_OPENBAO_TOKEN", "root");
    let role = env_or("WSF_APPROLE_NAME", "appliance");
    let role_id = env_or("WSF_OPENBAO_ROLE_ID", "appliance");
    let secret_id = env_or("WSF_OPENBAO_SECRET_ID", "appliance-secret");
    let transit_key = env_or("WSF_TRANSIT_KEY", "appliance-dek");
    let cred_path = env_or("WSF_BROKER_CRED_PATH", "kv/data/broker/aws-root");
    let bundle = env_or("WSF_BUNDLE_VERSION", "2026.07.appliance");
    let tenant = env_or("WSF_DEMO_TENANT", "demo-tenant");
    let vk = env_or("WSF_DEMO_VIRTUAL_KEY", "vk_demo");
    let vk_prefix = env_or("AOG_VIRTUAL_KEY_PREFIX", "kv/data/aog/virtual-keys");
    let out = env_or("WSF_SEED_OUT", "/seed/appliance.env");

    let c = Client::new();

    // Wait for OpenBao to accept connections (dev mode is unsealed on start).
    let mut up = false;
    for _ in 0..60 {
        if bao(&c, &addr, &root, Method::GET, "sys/health", None)
            .await
            .is_ok()
        {
            up = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_secs(1));
    }
    if !up {
        return Err(format!("openbao at {addr} never became reachable"));
    }

    // Mounts (ignore "already enabled" errors on re-run).
    let _ = bao(
        &c,
        &addr,
        &root,
        Method::POST,
        "sys/auth/approle",
        Some(json!({"type":"approle"})),
    )
    .await;
    let _ = bao(
        &c,
        &addr,
        &root,
        Method::POST,
        "sys/mounts/kv",
        Some(json!({"type":"kv","options":{"version":"2"}})),
    )
    .await;
    let _ = bao(
        &c,
        &addr,
        &root,
        Method::POST,
        "sys/mounts/transit",
        Some(json!({"type":"transit"})),
    )
    .await;
    bao(
        &c,
        &addr,
        &root,
        Method::POST,
        &format!("transit/keys/{transit_key}"),
        Some(json!({"type":"aes256-gcm96"})),
    )
    .await?;

    // Policy: read tenants/broker/aog KV + use the transit key.
    let policy = format!(
        "path \"kv/data/tenants/*\" {{ capabilities=[\"read\"] }}\n\
         path \"kv/data/broker/*\" {{ capabilities=[\"read\"] }}\n\
         path \"kv/data/aog/*\" {{ capabilities=[\"read\"] }}\n\
         path \"transit/encrypt/{transit_key}\" {{ capabilities=[\"update\"] }}\n\
         path \"transit/decrypt/{transit_key}\" {{ capabilities=[\"update\"] }}"
    );
    bao(
        &c,
        &addr,
        &root,
        Method::PUT,
        "sys/policies/acl/appliance",
        Some(json!({ "policy": policy })),
    )
    .await?;

    // AppRole with a fixed role_id + secret_id so the compose knows them ahead of time.
    bao(
        &c,
        &addr,
        &root,
        Method::POST,
        &format!("auth/approle/role/{role}"),
        Some(
            json!({"token_policies":"default,appliance","token_ttl":"20m",
                    "secret_id_num_uses":0,"secret_id_ttl":"0"}),
        ),
    )
    .await?;
    bao(
        &c,
        &addr,
        &root,
        Method::POST,
        &format!("auth/approle/role/{role}/role-id"),
        Some(json!({ "role_id": role_id })),
    )
    .await?;
    bao(
        &c,
        &addr,
        &root,
        Method::POST,
        &format!("auth/approle/role/{role}/custom-secret-id"),
        Some(json!({ "secret_id": secret_id })),
    )
    .await?;

    // Demo tenant (its attributes drive the token's routes / classification / scopes).
    let attrs = json!({
        "tenant_id": tenant, "display_name": "Appliance Demo Tenant",
        "compliance_scopes": ["hipaa"],
        "default_allowed_routes": ["local_only","cloud_allowed"],
        "max_data_classification": "restricted"
    });
    bao(
        &c,
        &addr,
        &root,
        Method::POST,
        &format!("kv/data/tenants/{tenant}"),
        Some(json!({ "data": { "attributes": attrs.to_string() } })),
    )
    .await?;

    // Broker root creds (demo / Moto).
    bao(
        &c,
        &addr,
        &root,
        Method::POST,
        &cred_path,
        Some(json!({ "data": { "access_key_id": "demo", "secret_access_key": "demo" } })),
    )
    .await?;

    // Mint the persistent trust-anchor keypairs + a subject-HMAC key.
    let (bridge_pk, bridge_sk) = RustCryptoMlDsa87::keypair().map_err(|e| e.to_string())?;
    let (seal_pk, seal_sk) = RustCryptoMlDsa87::keypair().map_err(|e| e.to_string())?;
    let (ledger_pk, ledger_sk) = RustCryptoMlDsa87::keypair().map_err(|e| e.to_string())?;
    let hmac = Sha256::digest([bridge_sk.as_slice(), b"subject-hmac"].concat()).to_vec();

    // Issue the demo token through the bridge (verifies the AppRole + tenant wiring).
    let bridge_signer = Arc::new(
        RustCryptoMlDsa87::from_keypair("wsf-bridge", bridge_pk.clone(), bridge_sk.clone())
            .map_err(|e| e.to_string())?,
    );
    let ob = OpenBaoAuth::new(OpenBaoConfig::new(
        &addr,
        role_id.clone(),
        secret_id.clone(),
    ))
    .map_err(|e| e.to_string())?;
    let bridge = TrustBridge::new(ob, bridge_signer, BridgeConfig::new(bundle, hmac.clone()));
    let request = IssueTokenRequest::new(
        tenant.clone(),
        "appliance-operator",
        vec!["operator".to_string()],
    )
    .with_budget(Budget {
        token_cap: 5_000_000,
        tokens_spent: 0,
        usd_cap_cents: 50_000,
        usd_spent_cents: 0,
        tool_call_cap: 10_000,
        tool_calls_spent: 0,
    })
    .with_models(vec!["demo".to_string(), "gpt-4o-mini".to_string()]);
    let token = bridge
        .issue_token(&request)
        .await
        .map_err(|e| format!("issue demo token: {e}"))?;

    // Seed the demo virtual key → signed token (the gateway resolves it).
    let key_hash = hex::encode(Sha256::digest(vk.as_bytes()));
    let token_value = serde_json::to_value(&token).map_err(|e| e.to_string())?;
    bao(
        &c,
        &addr,
        &root,
        Method::POST,
        &format!("{vk_prefix}/{key_hash}"),
        Some(json!({ "data": { "token": token_value } })),
    )
    .await?;

    // Write the shared env file the services source before exec.
    let env_body = format!(
        "WSF_OPENBAO_ADDR={addr}\n\
         WSF_OPENBAO_ROLE_ID={role_id}\n\
         WSF_OPENBAO_SECRET_ID={secret_id}\n\
         WSF_SUBJECT_HMAC_KEY={hmac_hex}\n\
         WSF_TRANSIT_KEY={transit_key}\n\
         WSF_BROKER_CRED_PATH={cred_path}\n\
         WSF_BRIDGE_PK={bridge_pk_b64}\n\
         WSF_BRIDGE_SK={bridge_sk_b64}\n\
         WSF_SEAL_PK={seal_pk_b64}\n\
         WSF_SEAL_SK={seal_sk_b64}\n\
         WSF_LEDGER_PK={ledger_pk_b64}\n\
         WSF_LEDGER_SK={ledger_sk_b64}\n\
         AOG_OPENBAO_ADDR={addr}\n\
         AOG_OPENBAO_ROLE_ID={role_id}\n\
         AOG_OPENBAO_SECRET_ID={secret_id}\n\
         AOG_TOKEN_ANCHOR={bridge_pk_b64}\n\
         AOG_VIRTUAL_KEY_PREFIX={vk_prefix}\n",
        hmac_hex = hex::encode(&hmac),
        bridge_pk_b64 = b64e(&bridge_pk),
        bridge_sk_b64 = b64e(&bridge_sk),
        seal_pk_b64 = b64e(&seal_pk),
        seal_sk_b64 = b64e(&seal_sk),
        ledger_pk_b64 = b64e(&ledger_pk),
        ledger_sk_b64 = b64e(&ledger_sk),
    );
    if let Some(dir) = std::path::Path::new(&out).parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("mkdir {}: {e}", dir.display()))?;
    }
    std::fs::write(&out, env_body).map_err(|e| format!("write {out}: {e}"))?;

    // Also dump the demo token so an operator can paste it into the console login
    // (`docker compose exec wsf-api cat /seed/demo-token.json`).
    let token_path = std::path::Path::new(&out).with_file_name("demo-token.json");
    let _ = std::fs::write(
        &token_path,
        serde_json::to_string_pretty(&token).unwrap_or_default(),
    );

    println!(
        "wsf-seed: OK — provisioned OpenBao, wrote {out}; tenant={tenant}, virtual_key={vk}, token_id={}",
        token.token_id
    );
    Ok(())
}
