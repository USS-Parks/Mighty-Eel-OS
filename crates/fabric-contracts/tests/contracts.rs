//! Contract-level tests (Prompt 0.8): serde round-trip, MAI-claim compatibility,
//! and label-standalone readability. No crypto here — this crate is pure types.

use fabric_contracts::{
    Classification, Envelope, Identity, Label, Receipt, RevocationStatus, Route, TrustToken,
};

fn sample_token_json() -> serde_json::Value {
    serde_json::json!({
        "token_id": "tok_x", "issued_at": "2026-07-03T18:00:00Z",
        "expires_at": "2026-07-03T18:15:00Z", "issuer": "wsf-bridge",
        "trust_bundle_version": "2026.07.03.001", "tenant_id": "baap",
        "subject_hash": "hmac:abc", "service_identity": "aog-gateway",
        "identity_id": "id_x", "roles": ["clinician"], "compliance_scopes": ["hipaa"],
        "allowed_routes": ["local_only"], "allowed_models": ["llama-3-8b-instruct"],
        "max_data_classification": "restricted", "country": "US", "person_type": "us_person",
        "offline_mode": false, "revocation_status": "unknown",
        "budget": {"token_cap": 200000, "tokens_spent": 0, "usd_cap_cents": 500,
                   "usd_spent_cents": 0, "tool_call_cap": 25, "tool_calls_spent": 0},
        "attenuation": {"parent_id": null,
                        "caveats": [{"type": "route_ceiling", "value": "local_only"}]},
        "signature": {"alg": "ml-dsa-87", "key_id": "k1", "value": "b64"}
    })
}

#[test]
fn trust_token_round_trips() {
    let t: TrustToken = serde_json::from_value(sample_token_json()).unwrap();
    let back = serde_json::to_value(&t).unwrap();
    let t2: TrustToken = serde_json::from_value(back).unwrap();
    assert_eq!(t, t2);
    assert_eq!(t.allowed_routes, vec![Route::LocalOnly]);
    assert_eq!(t.max_data_classification, Classification::Restricted);
    assert_eq!(t.attenuation.caveats.len(), 1);
}

// The exact MAI claim from docs/compliance/TRUST-MANIFOLD.md §4.1.
const MAI_CLAIM: &str = r#"{
  "claim_id": "clm_2026-05-22T18-00-00Z_a7f3",
  "issued_at": "2026-05-22T18:00:00Z",
  "expires_at": "2026-05-22T18:15:00Z",
  "issuer": "lamprey-trust-bridge",
  "trust_bundle_version": "2026.05.22.001",
  "tenant_id": "bay-area-pediatrics",
  "subject_id": "user:alice@bayarea-peds.example",
  "subject_hash": "hmac:7c2f3a...",
  "service_identity": null,
  "roles": ["clinician", "supervising-pharmacist"],
  "compliance_scopes": ["hipaa"],
  "allowed_routes": ["local_only"],
  "allowed_models": ["llama-3-8b-instruct", "mistral-7b-q4"],
  "max_data_classification": "restricted",
  "country": "US",
  "person_type": "us_person",
  "offline_mode": false,
  "revocation_status": "unknown",
  "signature": { "alg": "ed25519", "key_id": "lamprey-bridge-2026-q2", "value": "base64..." }
}"#;

#[test]
fn mai_claim_deserializes_as_root_token() {
    let t: TrustToken = serde_json::from_str(MAI_CLAIM).unwrap();
    assert_eq!(t.token_id, "clm_2026-05-22T18-00-00Z_a7f3"); // via claim_id alias
    assert!(t.budget.is_none()); // budget enforcement off
    assert!(t.attenuation.parent_id.is_none());
    assert!(t.attenuation.caveats.is_empty());
    assert_eq!(t.revocation_status, RevocationStatus::Unknown);
    assert_eq!(
        t.subject_id.as_deref(),
        Some("user:alice@bayarea-peds.example")
    );
}

#[test]
fn label_is_readable_without_seal_or_thread() {
    // A label parses standalone — no sealed payload, no provenance thread needed.
    // This is the property AOG relies on for DSPM-informed routing.
    let label_json = serde_json::json!({
        "classification": "restricted",
        "compliance_scopes": ["hipaa"],
        "origin": "svc:aeneas-gateway",
        "permitted_ops": ["unseal_local"],
        "permitted_destinations": ["local_only"],
        "detected_entities": ["phi.mrn"]
    });
    let label: Label = serde_json::from_value(label_json).unwrap();
    assert_eq!(label.classification, Classification::Restricted);
    assert_eq!(label.permitted_destinations, vec![Route::LocalOnly]);
}

#[test]
fn identity_round_trips() {
    let v = serde_json::json!({
        "identity_id": "id_x", "kind": "workload", "tenant_id": "baap",
        "subject_id": "svc:aog", "subject_hash": "hmac:abc", "service_identity": "aog-gateway",
        "spiffe_id": "spiffe://im/t/baap/aog", "pki_cert_fingerprint": "sha256:x",
        "parent_id": null, "issued_at": "2026-07-03T18:00:00Z",
        "expires_at": "2026-07-03T18:15:00Z",
        "signature": {"alg": "ml-dsa-87", "key_id": "k1", "value": "b64"}
    });
    let id: Identity = serde_json::from_value(v).unwrap();
    let back: Identity = serde_json::from_value(serde_json::to_value(&id).unwrap()).unwrap();
    assert_eq!(id, back);
}

#[test]
fn receipt_and_envelope_round_trip() {
    let rv = serde_json::json!({
        "receipt_id": "rcp_1", "request_id": "req_1", "request_hash": "blake3:a",
        "previous_hash": "blake3:0", "routing_decision": "LocalOnly",
        "modules_applied": ["hipaa"], "flags": [], "reasons": ["hipaa.min_necessary"],
        "correlation": {"subject_hash": "hmac:x", "token_id": "tok_x",
                        "tenant_id": "baap", "offline_mode": false},
        "token_id": "tok_x", "provider": "local:vllm", "model_weights_digest": "blake3:w",
        "spend_cents": 3, "tokens_used": 1840, "recorded_at": "2026-07-03T18:00:05Z"
    });
    let r: Receipt = serde_json::from_value(rv).unwrap();
    let r2: Receipt = serde_json::from_value(serde_json::to_value(&r).unwrap()).unwrap();
    assert_eq!(r, r2);

    let ev = serde_json::json!({
        "envelope_id": "env_1",
        "seal": {"aead_alg": "AES-256-GCM", "data_key_wrapped": "openbao:transit:x",
                 "nonce": "n", "ciphertext": "c", "aad_hash": "blake3:a"},
        "label": {"classification": "restricted", "compliance_scopes": ["hipaa"],
                  "origin": "svc:x", "permitted_ops": ["unseal_local"],
                  "permitted_destinations": ["local_only"], "detected_entities": []},
        "thread": {"created_at": "2026-07-03T18:00:00Z", "authorizing_token_id": "tok_x",
                   "previous_hash": "blake3:0", "signatures": []}
    });
    let e: Envelope = serde_json::from_value(ev).unwrap();
    let e2: Envelope = serde_json::from_value(serde_json::to_value(&e).unwrap()).unwrap();
    assert_eq!(e, e2);
}
