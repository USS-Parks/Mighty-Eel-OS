//! A2 gate — the transport authenticator rejects missing / malformed / expired
//! / wrong-audience / wrong-tenant credentials with 401/403 *before* any
//! handler runs, and admits a valid signed credential.
//!
//! Offline: exercises the [`WsfAuthenticator`] seam directly (no OpenBao). The
//! live-stack side of A2 rides the A5 gate.
#![allow(clippy::print_stdout, clippy::print_stderr)]

use axum::http::{HeaderMap, StatusCode, header};
use base64::Engine;
use chrono::{Duration, Utc};
use fabric_contracts::Audience;
use fabric_crypto::Signer;
use fabric_crypto::providers::{MlDsa87Verifier, RustCryptoMlDsa87};
use wsf_api::auth::{
    AuthError, WorkloadAuthenticator, WorkloadCredential, WsfAuthenticator, mint_credential,
};

fn header_for(cred: &WorkloadCredential) -> HeaderMap {
    let json = serde_json::to_vec(cred).unwrap();
    let b64 = base64::engine::general_purpose::STANDARD.encode(json);
    let mut h = HeaderMap::new();
    h.insert(
        header::AUTHORIZATION,
        format!("Workload {b64}").parse().unwrap(),
    );
    h
}

fn future() -> String {
    (Utc::now() + Duration::hours(1)).to_rfc3339()
}

#[test]
fn valid_credential_establishes_the_principal() {
    let authority = RustCryptoMlDsa87::generate("wsf-authority").unwrap();
    let auth = WorkloadAuthenticator::new(
        Box::new(MlDsa87Verifier),
        authority.public_key().to_vec(),
        Audience::Wsf,
    );
    let cred = mint_credential(
        &authority,
        "spiffe://mai/issuer",
        "tenant-a",
        "blake3:abc",
        Some("issuer".into()),
        Audience::Wsf,
        future(),
    );
    let p = auth
        .authenticate(&header_for(&cred))
        .expect("valid → principal");
    assert_eq!(p.tenant_id, "tenant-a");
    assert_eq!(p.principal_id, "spiffe://mai/issuer");
    assert!(p.is_for(Audience::Wsf));
    // Strength is server-decided, not caller-claimed.
    assert!(p.auth_strength.is_production_grade());
}

#[test]
fn missing_credential_is_401() {
    let authority = RustCryptoMlDsa87::generate("a").unwrap();
    let auth = WorkloadAuthenticator::new(
        Box::new(MlDsa87Verifier),
        authority.public_key().to_vec(),
        Audience::Wsf,
    );
    let err = auth.authenticate(&HeaderMap::new()).unwrap_err();
    assert_eq!(err, AuthError::MissingCredential);
    assert_eq!(err.status(), StatusCode::UNAUTHORIZED);
}

#[test]
fn malformed_credential_is_401() {
    let authority = RustCryptoMlDsa87::generate("a").unwrap();
    let auth = WorkloadAuthenticator::new(
        Box::new(MlDsa87Verifier),
        authority.public_key().to_vec(),
        Audience::Wsf,
    );
    // Wrong scheme.
    let mut h = HeaderMap::new();
    h.insert(header::AUTHORIZATION, "Bearer xyz".parse().unwrap());
    assert_eq!(
        auth.authenticate(&h).unwrap_err().status(),
        StatusCode::UNAUTHORIZED
    );
    // Right scheme, garbage body.
    let mut h2 = HeaderMap::new();
    h2.insert(
        header::AUTHORIZATION,
        "Workload !!!not-base64".parse().unwrap(),
    );
    assert_eq!(
        auth.authenticate(&h2).unwrap_err().status(),
        StatusCode::UNAUTHORIZED
    );
}

#[test]
fn forged_signature_is_401_untrusted() {
    // Credential signed by an *attacker* key, verified against the authority key.
    let authority = RustCryptoMlDsa87::generate("authority").unwrap();
    let attacker = RustCryptoMlDsa87::generate("attacker").unwrap();
    let auth = WorkloadAuthenticator::new(
        Box::new(MlDsa87Verifier),
        authority.public_key().to_vec(),
        Audience::Wsf,
    );
    let cred = mint_credential(
        &attacker,
        "spiffe://mai/evil",
        "tenant-a",
        "",
        None,
        Audience::Wsf,
        future(),
    );
    assert_eq!(
        auth.authenticate(&header_for(&cred)).unwrap_err(),
        AuthError::UntrustedCredential
    );
}

#[test]
fn tampered_field_after_signing_is_401_untrusted() {
    let authority = RustCryptoMlDsa87::generate("authority").unwrap();
    let auth = WorkloadAuthenticator::new(
        Box::new(MlDsa87Verifier),
        authority.public_key().to_vec(),
        Audience::Wsf,
    );
    let mut cred = mint_credential(
        &authority,
        "p",
        "tenant-a",
        "",
        None,
        Audience::Wsf,
        future(),
    );
    // Escalate tenant after the authority signed — signature no longer covers it.
    cred.tenant_id = "tenant-victim".into();
    assert_eq!(
        auth.authenticate(&header_for(&cred)).unwrap_err(),
        AuthError::UntrustedCredential
    );
}

#[test]
fn expired_credential_is_401() {
    let authority = RustCryptoMlDsa87::generate("authority").unwrap();
    let auth = WorkloadAuthenticator::new(
        Box::new(MlDsa87Verifier),
        authority.public_key().to_vec(),
        Audience::Wsf,
    );
    let past = (Utc::now() - Duration::hours(1)).to_rfc3339();
    let cred = mint_credential(&authority, "p", "tenant-a", "", None, Audience::Wsf, past);
    assert_eq!(
        auth.authenticate(&header_for(&cred)).unwrap_err(),
        AuthError::ExpiredCredential
    );
}

#[test]
fn wrong_audience_is_403() {
    let authority = RustCryptoMlDsa87::generate("authority").unwrap();
    // Ingress serves WSF; credential is minted for AOG.
    let auth = WorkloadAuthenticator::new(
        Box::new(MlDsa87Verifier),
        authority.public_key().to_vec(),
        Audience::Wsf,
    );
    let cred = mint_credential(
        &authority,
        "p",
        "tenant-a",
        "",
        None,
        Audience::Aog,
        future(),
    );
    let err = auth.authenticate(&header_for(&cred)).unwrap_err();
    assert!(matches!(err, AuthError::WrongAudience { .. }));
    assert_eq!(err.status(), StatusCode::FORBIDDEN);
}

#[test]
fn wrong_tenant_on_bound_ingress_is_403() {
    let authority = RustCryptoMlDsa87::generate("authority").unwrap();
    let auth = WorkloadAuthenticator::new(
        Box::new(MlDsa87Verifier),
        authority.public_key().to_vec(),
        Audience::Wsf,
    )
    .bound_to_tenant("tenant-a");
    // Validly signed, correct audience, but for a different tenant than this
    // single-tenant ingress admits.
    let cred = mint_credential(
        &authority,
        "p",
        "tenant-b",
        "",
        None,
        Audience::Wsf,
        future(),
    );
    let err = auth.authenticate(&header_for(&cred)).unwrap_err();
    assert_eq!(err, AuthError::WrongTenant);
    assert_eq!(err.status(), StatusCode::FORBIDDEN);
}
