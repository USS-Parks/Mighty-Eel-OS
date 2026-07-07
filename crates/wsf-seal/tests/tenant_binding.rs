//! AF-003 (offline): unseal refuses a cross-tenant / cross-owner token and an
//! unbound (v1) envelope BEFORE any Transit decrypt — the tenant/owner binding is
//! checked before OpenBao is consulted, so no live stack is needed.

use std::sync::Arc;

use chrono::Utc;
use fabric_contracts::{
    Attenuation, Classification, Envelope, Label, RevocationStatus, Signature, TrustToken,
};
use fabric_crypto::Signer;
use fabric_crypto::providers::RustCryptoMlDsa87;
use fabric_envelope::{EnvelopeBinding, ThreadSpec, seal_envelope};
use wsf_bridge::{OpenBaoAuth, OpenBaoConfig};
use wsf_seal::{SealError, SealService, SealServiceConfig, UnsealRequest};

fn dummy_openbao() -> OpenBaoAuth {
    OpenBaoAuth::new(OpenBaoConfig::new("http://127.0.0.1:1", "r", "s")).unwrap()
}

fn token(anchor: &RustCryptoMlDsa87, tenant: &str, subject_hash: &str) -> TrustToken {
    let t = TrustToken {
        token_id: "tok".into(),
        issued_at: "2026-07-03T00:00:00Z".into(),
        expires_at: "2099-01-01T00:00:00Z".into(),
        issuer: "wsf-bridge".into(),
        trust_bundle_version: "v".into(),
        tenant_id: tenant.into(),
        subject_id: None,
        subject_hash: subject_hash.into(),
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
        budget: None,
        attenuation: Attenuation::default(),
        signature: Signature {
            alg: String::new(),
            key_id: String::new(),
            value: String::new(),
        },
    };
    fabric_token::issue(t, anchor).unwrap()
}

fn bound_envelope(tenant: &str, owner: &str) -> Envelope {
    let label = Label {
        classification: Classification::Restricted,
        compliance_scopes: vec![],
        origin: "test".into(),
        permitted_ops: vec![],
        permitted_destinations: vec![],
        detected_entities: vec![],
    };
    let signer = RustCryptoMlDsa87::generate("env-signer").unwrap();
    seal_envelope(
        "env-1",
        b"secret",
        &[3u8; 32],
        "ref",
        label,
        ThreadSpec {
            authorizing_token_id: "tokB".into(),
            previous_hash: "blake3:0".into(),
            created_at: "2026-07-03T00:00:00Z".into(),
            binding: EnvelopeBinding {
                tenant_id: tenant.into(),
                owner_subject_hash: owner.into(),
                audience: String::new(),
            },
        },
        &signer,
    )
    .unwrap()
}

fn service(anchor: &RustCryptoMlDsa87) -> SealService {
    SealService::new(
        dummy_openbao(),
        Arc::new(RustCryptoMlDsa87::generate("seal").unwrap()),
        SealServiceConfig {
            transit_key: "k".into(),
            token_public_key: anchor.public_key().to_vec(),
        },
    )
}

#[tokio::test]
async fn cross_tenant_unseal_is_refused_before_transit() {
    let anchor = RustCryptoMlDsa87::generate("anchor").unwrap();
    let svc = service(&anchor);
    let env = bound_envelope("tenant-b", "hmac:b");
    let tok = token(&anchor, "tenant-a", "hmac:a"); // different tenant
    let err = svc
        .unseal(
            UnsealRequest {
                token: tok,
                envelope: env,
            },
            Utc::now(),
        )
        .await
        .unwrap_err();
    assert!(
        matches!(err, SealError::Unauthorized(_)),
        "cross-tenant unseal must be refused, got {err:?}"
    );
}

#[tokio::test]
async fn cross_owner_same_tenant_unseal_is_refused() {
    let anchor = RustCryptoMlDsa87::generate("anchor").unwrap();
    let svc = service(&anchor);
    let env = bound_envelope("tenant-a", "hmac:owner");
    let tok = token(&anchor, "tenant-a", "hmac:other"); // same tenant, other owner
    let err = svc
        .unseal(
            UnsealRequest {
                token: tok,
                envelope: env,
            },
            Utc::now(),
        )
        .await
        .unwrap_err();
    assert!(matches!(err, SealError::Unauthorized(_)));
}

#[tokio::test]
async fn unbound_v1_envelope_is_refused() {
    let anchor = RustCryptoMlDsa87::generate("anchor").unwrap();
    let svc = service(&anchor);
    let env = bound_envelope("", ""); // unbound legacy envelope
    let tok = token(&anchor, "tenant-a", "hmac:a");
    let err = svc
        .unseal(
            UnsealRequest {
                token: tok,
                envelope: env,
            },
            Utc::now(),
        )
        .await
        .unwrap_err();
    assert!(matches!(err, SealError::Unauthorized(_)));
}

#[tokio::test]
async fn owner_token_passes_binding_and_reaches_transit() {
    // Same tenant + owner → past the binding check → fails at the dummy OpenBao
    // (an OpenBao error, NOT Unauthorized): the legitimate owner is allowed.
    let anchor = RustCryptoMlDsa87::generate("anchor").unwrap();
    let svc = service(&anchor);
    let env = bound_envelope("tenant-a", "hmac:a");
    let tok = token(&anchor, "tenant-a", "hmac:a");
    let err = svc
        .unseal(
            UnsealRequest {
                token: tok,
                envelope: env,
            },
            Utc::now(),
        )
        .await
        .unwrap_err();
    assert!(
        matches!(err, SealError::OpenBao(_)),
        "owner passes the binding; fails at transit, got {err:?}"
    );
}
