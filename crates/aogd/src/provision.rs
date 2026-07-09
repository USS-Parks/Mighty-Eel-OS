//! Provision the daemon's trust material from OpenBao.
//!
//! `Sealer::generate()` mints a *fresh, ephemeral* signer per process
//! and uses a fixed placeholder data key; the anchor arrives as a raw env hex.
//! That is the kernel default the `aog-apiserver` seal explicitly flags as the
//! Phase-W OpenBao seam. This module is that seam for `aogd`: at startup a node
//! logs in to OpenBao (AppRole) and reads ONE KV-v2 trust record, then assembles
//! from it both halves of the authenticated CRUD surface —
//!
//!   * the **Authenticator**'s WSF anchor (the ML-DSA-87 public key every
//!     presented token must verify under), and
//!   * the **Sealer**'s field-seal data key + child-mint signer.
//!
//! Custodying this material centrally makes it *stable* (sealed state stays
//! openable across a restart) and *shared* (any node verifies any node's child
//! tokens). The daemon never holds the anchor's secret — only the public key it
//! checks against. Missing or malformed material fails closed (doctrine I-4).
//!
//! The trust record is a KV-v2 object of four base64 fields:
//! `anchor_pubkey`, `seal_data_key`, `seal_signer_pk`, `seal_signer_sk`.

use aog_apiserver::auth::Authenticator;
use aog_apiserver::seal::Sealer;
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use fabric_crypto::providers::RustCryptoMlDsa87;
use wsf_bridge::{OpenBaoAuth, OpenBaoConfig};

use crate::{DaemonError, OpenBaoTrust};

/// The trust material a Loom node needs to serve the authenticated CRUD surface.
pub struct TrustMaterial {
    /// Verifies presented trust tokens against the WSF anchor.
    pub authenticator: Authenticator,
    /// Field-seals sensitive spec fields and mints attenuated child tokens.
    pub sealer: Sealer,
}

/// KV-v2 field names in the trust record (all base64).
const ANCHOR: &str = "anchor_pubkey";
const SEAL_DATA_KEY: &str = "seal_data_key";
const SEAL_SIGNER_PK: &str = "seal_signer_pk";
const SEAL_SIGNER_SK: &str = "seal_signer_sk";

/// The `key_id` the assembled seal signer carries — same label the kernel
/// default (`Sealer::generate`) uses, so the two are drop-in interchangeable.
const SEAL_KEY_ID: &str = "aog-apiserver-cp";

/// Log in to OpenBao (AppRole) and read the trust record at `trust.trust_path`,
/// then assemble the anchor + sealer from it.
///
/// # Errors
/// [`DaemonError::Config`] if OpenBao is unreachable, the credential is rejected,
/// the record is absent, or its material is malformed.
pub async fn from_openbao(trust: &OpenBaoTrust) -> Result<TrustMaterial, DaemonError> {
    let client = OpenBaoAuth::new(OpenBaoConfig::new(
        &trust.address,
        &trust.role_id,
        &trust.secret_id,
    ))
    .map_err(|e| DaemonError::Config(format!("openbao client: {e}")))?;
    let token = client
        .login()
        .await
        .map_err(|e| DaemonError::Config(format!("openbao login: {e}")))?;
    let record = client
        .get_kv_data(&token, &trust.trust_path)
        .await
        .map_err(|e| DaemonError::Config(format!("openbao trust read: {e}")))?;
    assemble(&record)
}

/// Assemble trust material from a KV-v2 `data.data` record. Pure — the offline
/// gate drives it directly, no OpenBao round-trip.
///
/// # Errors
/// [`DaemonError::Config`] if a field is absent, not base64, or the wrong length.
pub fn assemble(record: &serde_json::Value) -> Result<TrustMaterial, DaemonError> {
    let anchor = field(record, ANCHOR)?;
    let data_key: [u8; 32] = field(record, SEAL_DATA_KEY)?
        .as_slice()
        .try_into()
        .map_err(|_| DaemonError::Config(format!("{SEAL_DATA_KEY} must be 32 bytes")))?;
    let signer = RustCryptoMlDsa87::from_keypair(
        SEAL_KEY_ID,
        field(record, SEAL_SIGNER_PK)?,
        field(record, SEAL_SIGNER_SK)?,
    )
    .map_err(|e| DaemonError::Config(format!("seal signer: {e}")))?;
    Ok(TrustMaterial {
        authenticator: Authenticator::new(anchor),
        sealer: Sealer::new(data_key, signer),
    })
}

/// Decode a required base64 field of the trust record.
fn field(record: &serde_json::Value, name: &str) -> Result<Vec<u8>, DaemonError> {
    let encoded = record
        .get(name)
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| DaemonError::Config(format!("trust record missing {name}")))?;
    BASE64
        .decode(encoded.trim())
        .map_err(|e| DaemonError::Config(format!("{name} base64: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Duration as ChronoDuration, Utc};
    use fabric_contracts::{Attenuation, Classification, RevocationStatus, Signature, TrustToken};
    use fabric_crypto::Signer;
    use serde_json::json;

    /// A `base64(json(TrustToken))` `x-wsf-token` header value under `signer`.
    fn token_header(signer: &RustCryptoMlDsa87) -> String {
        let now = Utc::now();
        let token = TrustToken {
            token_id: "tok-vh5bc".to_owned(),
            issued_at: now.to_rfc3339(),
            expires_at: (now + ChronoDuration::hours(1)).to_rfc3339(),
            issuer: "wsf-bridge".to_owned(),
            trust_bundle_version: "2026.07.loom".to_owned(),
            tenant_id: "tenant-loom".to_owned(),
            subject_id: None,
            subject_hash: "hmac:loom".to_owned(),
            service_identity: Some("aogctl".to_owned()),
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
        let minted = fabric_token::issue(token, signer).unwrap();
        BASE64.encode(serde_json::to_vec(&minted).unwrap())
    }

    fn headers(value: &str) -> axum::http::HeaderMap {
        let mut h = axum::http::HeaderMap::new();
        h.insert("x-wsf-token", value.parse().unwrap());
        h
    }

    /// A well-formed trust record built around a caller-held anchor + seal signer.
    fn record(anchor: &RustCryptoMlDsa87, seal_pk: &[u8], seal_sk: &[u8]) -> serde_json::Value {
        json!({
            ANCHOR: BASE64.encode(anchor.public_key()),
            SEAL_DATA_KEY: BASE64.encode([7u8; 32]),
            SEAL_SIGNER_PK: BASE64.encode(seal_pk),
            SEAL_SIGNER_SK: BASE64.encode(seal_sk),
        })
    }

    #[test]
    fn assemble_wires_anchor_and_seal_signer() {
        let anchor = RustCryptoMlDsa87::generate("unit-anchor").unwrap();
        let (seal_pk, seal_sk) = RustCryptoMlDsa87::keypair().unwrap();
        let material = assemble(&record(&anchor, &seal_pk, &seal_sk)).unwrap();

        // The seal signer is the provisioned one (shared across the estate).
        assert_eq!(material.sealer.public_key(), seal_pk.as_slice());

        // The anchor is wired: a token minted under it authenticates.
        assert!(
            material
                .authenticator
                .authenticate(&headers(&token_header(&anchor)))
                .is_ok(),
            "a token under the provisioned anchor must authenticate"
        );

        // A token under a DIFFERENT anchor is refused (the anchor binding holds).
        let rogue = RustCryptoMlDsa87::generate("rogue-anchor").unwrap();
        assert!(
            material
                .authenticator
                .authenticate(&headers(&token_header(&rogue)))
                .is_err(),
            "a token under a rogue anchor must be refused"
        );
    }

    #[test]
    fn assemble_fails_closed_on_bad_material() {
        let anchor = RustCryptoMlDsa87::generate("unit-anchor").unwrap();
        let (seal_pk, seal_sk) = RustCryptoMlDsa87::keypair().unwrap();

        // A missing field.
        let mut missing = record(&anchor, &seal_pk, &seal_sk);
        missing.as_object_mut().unwrap().remove(SEAL_SIGNER_SK);
        assert!(
            assemble(&missing).is_err(),
            "missing field must fail closed"
        );

        // A short (31-byte) data key.
        let mut short = record(&anchor, &seal_pk, &seal_sk);
        short[SEAL_DATA_KEY] = json!(BASE64.encode([7u8; 31]));
        assert!(assemble(&short).is_err(), "short data key must fail closed");

        // A truncated seal secret key.
        let mut bad_key = record(&anchor, &seal_pk, &seal_sk);
        bad_key[SEAL_SIGNER_SK] = json!(BASE64.encode(&seal_sk[..100]));
        assert!(
            assemble(&bad_key).is_err(),
            "wrong-length seal key must fail closed"
        );
    }
}
