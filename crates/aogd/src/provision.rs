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
use aog_wire::tls::{NodeIdentityContract, NodeTls};
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
const RAFT_CA_DER: &str = "raft_ca_der";
const RAFT_CERT_DER: &str = "raft_cert_der";
const RAFT_KEY_DER: &str = "raft_key_der";

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

/// Read and validate a per-node Raft TLS record from OpenBao. The record keeps
/// private material separate from the estate-wide application trust record and
/// contains base64 DER fields `raft_ca_der`, `raft_cert_der`, and
/// `raft_key_der`. Errors never include field contents.
///
/// # Errors
/// [`DaemonError::Config`] if login/read fails or the identity contract rejects
/// the material.
pub async fn node_tls_from_openbao(
    trust: &OpenBaoTrust,
    path: &str,
    contract: &NodeIdentityContract,
) -> Result<NodeTls, DaemonError> {
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
        .get_kv_data(&token, path)
        .await
        .map_err(|e| DaemonError::Config(format!("openbao node TLS read: {e}")))?;
    assemble_node_tls(&record, contract)
}

/// Load DER files and validate them against the node identity contract before
/// the daemon can bind a listener.
///
/// # Errors
/// [`DaemonError::Config`] names the unreadable file or rejected public
/// contract without logging private-key bytes.
pub fn node_tls_from_files(
    ca_path: &std::path::Path,
    cert_path: &std::path::Path,
    key_path: &std::path::Path,
    contract: &NodeIdentityContract,
) -> Result<NodeTls, DaemonError> {
    let ca = std::fs::read(ca_path)
        .map_err(|e| DaemonError::Config(format!("read estate CA DER: {e}")))?;
    let cert = std::fs::read(cert_path)
        .map_err(|e| DaemonError::Config(format!("read node certificate DER: {e}")))?;
    let key = std::fs::read(key_path)
        .map_err(|e| DaemonError::Config(format!("read node private-key DER: {e}")))?;
    NodeTls::for_node_der(vec![ca], vec![cert], key, contract)
        .map_err(|e| DaemonError::Config(format!("node TLS identity: {e}")))
}

/// Assemble a node TLS record without exposing decoded key material.
///
/// # Errors
/// [`DaemonError::Config`] if a field is absent/malformed or the certificate
/// violates the node identity contract.
pub fn assemble_node_tls(
    record: &serde_json::Value,
    contract: &NodeIdentityContract,
) -> Result<NodeTls, DaemonError> {
    NodeTls::for_node_der(
        vec![field(record, RAFT_CA_DER)?],
        vec![field(record, RAFT_CERT_DER)?],
        field(record, RAFT_KEY_DER)?,
        contract,
    )
    .map_err(|e| DaemonError::Config(format!("node TLS identity: {e}")))
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
    use std::process::Command;
    use std::time::Duration;

    fn openssl(args: &[&str]) {
        let output = Command::new("openssl")
            .args(args)
            .output()
            .expect("openssl on PATH");
        assert!(
            output.status.success(),
            "openssl {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn node_tls_record() -> serde_json::Value {
        let dir = std::env::temp_dir().join("aogd-node-tls-record");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let ca = dir.join("ca.pem");
        let ca_key = dir.join("ca.key.pem");
        let cert = dir.join("node.pem");
        let key = dir.join("node.key.pem");
        let csr = dir.join("node.csr");
        let ext = dir.join("node.ext");
        let ca_der = dir.join("ca.der");
        let cert_der = dir.join("node.der");
        let key_der = dir.join("node.key.der");
        std::fs::write(
            &ext,
            "[ v3 ]\nsubjectAltName = DNS:cp1, URI:spiffe://loom/node/1\nextendedKeyUsage = serverAuth,clientAuth\nbasicConstraints = CA:FALSE\n",
        )
        .unwrap();
        openssl(&[
            "req",
            "-x509",
            "-newkey",
            "ec",
            "-pkeyopt",
            "ec_paramgen_curve:prime256v1",
            "-nodes",
            "-keyout",
            ca_key.to_str().unwrap(),
            "-out",
            ca.to_str().unwrap(),
            "-subj",
            "/CN=estate-ca",
            "-days",
            "36500",
            "-addext",
            "basicConstraints=critical,CA:TRUE",
            "-addext",
            "keyUsage=critical,keyCertSign,cRLSign",
        ]);
        openssl(&[
            "req",
            "-newkey",
            "ec",
            "-pkeyopt",
            "ec_paramgen_curve:prime256v1",
            "-nodes",
            "-keyout",
            key.to_str().unwrap(),
            "-out",
            csr.to_str().unwrap(),
            "-subj",
            "/CN=node-1",
        ]);
        openssl(&[
            "x509",
            "-req",
            "-in",
            csr.to_str().unwrap(),
            "-CA",
            ca.to_str().unwrap(),
            "-CAkey",
            ca_key.to_str().unwrap(),
            "-CAcreateserial",
            "-out",
            cert.to_str().unwrap(),
            "-days",
            "36500",
            "-extfile",
            ext.to_str().unwrap(),
            "-extensions",
            "v3",
        ]);
        openssl(&[
            "x509",
            "-in",
            ca.to_str().unwrap(),
            "-outform",
            "DER",
            "-out",
            ca_der.to_str().unwrap(),
        ]);
        openssl(&[
            "x509",
            "-in",
            cert.to_str().unwrap(),
            "-outform",
            "DER",
            "-out",
            cert_der.to_str().unwrap(),
        ]);
        openssl(&[
            "pkcs8",
            "-topk8",
            "-nocrypt",
            "-in",
            key.to_str().unwrap(),
            "-outform",
            "DER",
            "-out",
            key_der.to_str().unwrap(),
        ]);
        json!({
            RAFT_CA_DER: BASE64.encode(std::fs::read(ca_der).unwrap()),
            RAFT_CERT_DER: BASE64.encode(std::fs::read(cert_der).unwrap()),
            RAFT_KEY_DER: BASE64.encode(std::fs::read(key_der).unwrap()),
        })
    }

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

    #[test]
    fn openbao_node_tls_record_binds_the_exact_node_identity() {
        let record = node_tls_record();
        let contract =
            NodeIdentityContract::new(1, "https://cp1:4600", Duration::from_secs(3600)).unwrap();
        assert!(assemble_node_tls(&record, &contract).is_ok());

        let wrong_node =
            NodeIdentityContract::new(2, "https://cp1:4600", Duration::from_secs(3600)).unwrap();
        assert!(
            assemble_node_tls(&record, &wrong_node).is_err(),
            "an OpenBao node-1 record must not provision node 2"
        );

        let mut missing_key = record;
        missing_key.as_object_mut().unwrap().remove(RAFT_KEY_DER);
        let err = assemble_node_tls(&missing_key, &contract)
            .err()
            .expect("missing private key must fail closed");
        assert!(err.to_string().contains(RAFT_KEY_DER));
    }
}
