//! E2 live gate — per-tenant Transit key isolation. A data key wrapped under
//! tenant A's key cannot be unwrapped under tenant B's key, even at the OpenBao
//! crypto layer (independent of the E4 app-layer tenant check).
//!
//! Env-gated on `WSF_OPENBAO_ADDR`.
#![allow(clippy::print_stdout, clippy::print_stderr)]

use reqwest::{Client, Method};
use serde_json::{Value, json};

const BASE_KEY: &str = "e2-dek";

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
) -> (u16, String) {
    let url = format!("{addr}/v1/{path}");
    let mut rb = c.request(m, &url).header("X-Vault-Token", tok);
    if let Some(b) = body {
        rb = rb
            .header("Content-Type", "application/json")
            .body(b.to_string());
    }
    let resp = rb.send().await.expect("openbao req");
    let status = resp.status().as_u16();
    (status, resp.text().await.unwrap_or_default())
}

#[tokio::test]
async fn per_tenant_transit_keys_isolate_wrapped_material() {
    let Some(addr) = openbao_addr() else {
        eprintln!("SKIP live_tenant_keys: set WSF_OPENBAO_ADDR (E2 live gate)");
        return;
    };
    let c = Client::new();
    let tok = root_token();
    bao(
        &c,
        &addr,
        &tok,
        Method::POST,
        "sys/mounts/transit",
        Some(json!({"type":"transit"})),
    )
    .await;
    for tenant in ["tenant-a", "tenant-b"] {
        bao(
            &c,
            &addr,
            &tok,
            Method::POST,
            &format!("transit/keys/{BASE_KEY}-{tenant}"),
            Some(json!({"type":"aes256-gcm96"})),
        )
        .await;
    }

    // Wrap a data key under tenant A's key.
    let plaintext_b64 = base64_std(b"tenant-a-data-key-material-000000");
    let (_, enc) = bao(
        &c,
        &addr,
        &tok,
        Method::POST,
        &format!("transit/encrypt/{BASE_KEY}-tenant-a"),
        Some(json!({ "plaintext": plaintext_b64 })),
    )
    .await;
    let enc: Value = serde_json::from_str(&enc).expect("encrypt json");
    let ciphertext = enc["data"]["ciphertext"]
        .as_str()
        .expect("ciphertext")
        .to_string();
    assert!(ciphertext.starts_with("vault:v1:"));

    // Unwrapping the SAME ciphertext under tenant B's key fails (400) — the
    // material is cryptographically bound to tenant A's key.
    let (status_b, body_b) = bao(
        &c,
        &addr,
        &tok,
        Method::POST,
        &format!("transit/decrypt/{BASE_KEY}-tenant-b"),
        Some(json!({ "ciphertext": ciphertext.clone() })),
    )
    .await;
    assert!(
        status_b >= 400,
        "cross-tenant transit decrypt must fail, got {status_b}: {body_b}"
    );

    // Unwrapping under tenant A's own key succeeds.
    let (status_a, body_a) = bao(
        &c,
        &addr,
        &tok,
        Method::POST,
        &format!("transit/decrypt/{BASE_KEY}-tenant-a"),
        Some(json!({ "ciphertext": ciphertext })),
    )
    .await;
    assert_eq!(status_a, 200, "same-tenant decrypt succeeds: {body_a}");

    println!(
        "E2 live gate PASSED against {addr}: per-tenant Transit keys isolate wrapped material"
    );
}

fn base64_std(bytes: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(bytes)
}
