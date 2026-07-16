//! The mutating `/admin/*` surface refuses a caller without the admin role.
//!
//! The daemon serves the admin API behind the front-door authenticator once an
//! anchor is provisioned. Three postures against `POST /admin/write`:
//!
//! * no token — 401 (authentication, fail-closed),
//! * a VALID token without the `aog-admin` role — 403 (authorization: the
//!   denial this test exists to assert),
//! * a valid `aog-admin` token — past the trust gate; on this unformed
//!   single node the write then fails 503 ("no leader"), proving the 403
//!   above was the role check and not a blanket refusal.
//!
//! The read-only `/admin/leader` stays open by design (the conformance
//! harness polls it before any trust material exists).

use std::path::PathBuf;
use std::time::{Duration, Instant};

use aogd::admin::AOG_ADMIN_ROLE;
use aogd::{Config, Daemon, Op, Precondition};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use chrono::{Duration as ChronoDuration, Utc};
use fabric_contracts::{Attenuation, Classification, RevocationStatus, Signature, TrustToken};
use fabric_crypto::Signer;
use fabric_crypto::providers::RustCryptoMlDsa87;
use tokio::net::TcpListener;

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(name);
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

/// A `base64(json(TrustToken))` header value carrying `roles`, signed under
/// `signer`.
fn token_header(signer: &RustCryptoMlDsa87, roles: &[&str]) -> String {
    let now = Utc::now();
    let token = TrustToken {
        token_id: "tok-admin-auth".to_owned(),
        issued_at: now.to_rfc3339(),
        expires_at: (now + ChronoDuration::hours(1)).to_rfc3339(),
        issuer: "wsf-bridge".to_owned(),
        trust_bundle_version: "2026.07.loom".to_owned(),
        tenant_id: "tenant-loom".to_owned(),
        subject_id: None,
        subject_hash: "hmac:loom".to_owned(),
        service_identity: Some("aogctl".to_owned()),
        identity_id: None,
        roles: roles.iter().map(ToString::to_string).collect(),
        compliance_scopes: vec![],
        allowed_routes: vec![],
        allowed_models: vec![],
        max_data_classification: Classification::Restricted,
        country: None,
        person_type: None,
        offline_mode: false,
        revocation_status: RevocationStatus::Valid,
        budget: None,
        attenuation: Attenuation::default(),
        signature: Signature {
            alg: String::new(),
            key_id: String::new(),
            value: String::new(),
        },
    };
    let minted = fabric_token::issue(token, signer).unwrap();
    BASE64.encode(serde_json::to_vec(&minted).unwrap())
}

async fn await_health(http: &reqwest::Client, base: &str) -> bool {
    let start = Instant::now();
    loop {
        if let Ok(r) = http.get(format!("{base}/healthz")).send().await
            && r.status().is_success()
        {
            return true;
        }
        if start.elapsed() >= Duration::from_secs(5) {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

fn write_op() -> Op {
    Op::Put {
        key: "Workload/admin-auth".to_owned(),
        value: b"v1".to_vec(),
        expected: Precondition::Any,
    }
}

#[tokio::test]
async fn admin_mutation_without_admin_role_is_403() {
    let anchor = RustCryptoMlDsa87::generate("admin-auth-anchor").unwrap();

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let config = Config {
        node_id: 1,
        data_dir: scratch("loom-admin-auth-aogd"),
        listen: addr,
        advertise: format!("http://{addr}"),
        anchor_pubkey: Some(anchor.public_key().to_vec()),
        openbao: None,
        node_tls: None,
        allow_insecure_admin: false,
    };
    let daemon = Daemon::start(config).await.unwrap();
    let app = daemon.app();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

    let base = format!("http://{addr}");
    let http = reqwest::Client::new();
    assert!(
        await_health(&http, &base).await,
        "daemon health never came up"
    );

    let url = format!("{base}/admin/write");

    // No token -> 401: the mutating admin surface authenticates first.
    let r = http.post(&url).json(&write_op()).send().await.unwrap();
    assert_eq!(
        r.status().as_u16(),
        401,
        "an unauthenticated admin mutation must be refused"
    );

    // An authenticated NON-admin token -> 403: the role gate holds.
    let r = http
        .post(&url)
        .header("x-wsf-token", token_header(&anchor, &["operator"]))
        .json(&write_op())
        .send()
        .await
        .unwrap();
    assert_eq!(
        r.status().as_u16(),
        403,
        "a valid token without the admin role must be refused with 403"
    );
    let body = r.text().await.unwrap();
    assert!(
        body.contains(AOG_ADMIN_ROLE),
        "the denial names the required role, got: {body}"
    );

    // The aog-admin role passes the trust gate; on this unformed single node
    // the write then fails 503 (no leader) — availability, not auth. This
    // pins that the 403 above was the role check, not a blanket refusal.
    let r = http
        .post(&url)
        .header("x-wsf-token", token_header(&anchor, &[AOG_ADMIN_ROLE]))
        .json(&write_op())
        .send()
        .await
        .unwrap();
    assert_eq!(
        r.status().as_u16(),
        503,
        "an admin-role write on an unformed node fails on availability, never auth"
    );

    // The read-only leader view stays open by design.
    let r = http
        .get(format!("{base}/admin/leader"))
        .send()
        .await
        .unwrap();
    assert_eq!(
        r.status().as_u16(),
        200,
        "the read-only /admin/leader stays open for the harness"
    );
}
