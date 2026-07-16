//! `wsf-bridge` — the WSF Trust Bridge (Ring 2), productized.
//!
//! Turns an authenticated OpenBao workload into a signed, budget-carrying WSF
//! trust token, and signs the two other trust artifacts a bridge is responsible
//! for: policy bundles and revocation snapshots. It is the Ring-1 (OpenBao Trust
//! Core) ↔ Ring-3 (appliance) bridge from `docs/compliance/TRUST-MANIFOLD.md`,
//! promoted from the MAI `openbao_client.rs` into a standalone, HA-ready library.
//!
//! ## Trust model
//! - **Identity comes from OpenBao.** The bridge authenticates the workload via
//!   AppRole (the "auth event") and reads the tenant's authorization envelope
//!   (compliance scopes, route ceiling, classification ceiling) from KV.
//! - **Signatures are pure-Rust ML-DSA-87** (`fabric-crypto`), never OpenBao
//!   Transit — so every token / bundle / revocation verifies **off-host** with a
//!   public key alone, and an air-gapped appliance can verify without reaching
//!   the core. (OSS OpenBao Transit has no GA post-quantum algorithm; Transit
//!   stays a pluggable custody seam for the day it ships one.)
//! - **Stateless:** every call performs its own OpenBao login, so the bridge
//!   scales horizontally with no shared session state.
//!
//! ```no_run
//! # async fn demo() -> Result<(), wsf_bridge::BridgeError> {
//! use std::sync::Arc;
//! use fabric_crypto::providers::RustCryptoMlDsa87;
//! use wsf_bridge::{BridgeConfig, IssueTokenRequest, OpenBaoAuth, OpenBaoConfig, TrustBridge};
//!
//! let openbao = OpenBaoAuth::new(OpenBaoConfig::new(
//!     "http://localhost:8200", "role-id", "secret-id",
//! ))?;
//! let signer = Arc::new(RustCryptoMlDsa87::generate("wsf-bridge-key")?);
//! let bridge = TrustBridge::new(openbao, signer, BridgeConfig::new("2026.07.03.001", vec![0u8; 32]));
//! let token = bridge
//!     .issue_token(&IssueTokenRequest::new("tenant-a", "subject-1", vec!["clinician".into()]))
//!     .await?;
//! # let _ = token;
//! # Ok(()) }
//! ```

mod error;
pub mod openbao;

pub use error::BridgeError;
pub use openbao::{OpenBaoAuth, OpenBaoConfig, OpenBaoError, TenantAttributes};

use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use fabric_contracts::{
    Attenuation, Budget, Classification, ComplianceScope, RevocationStatus, Route, Signature,
    TrustToken,
};
use fabric_crypto::{Signer, Verifier};
use fabric_revocation::RevocationSnapshot;
use serde::{Deserialize, Serialize};

/// Static configuration for a Trust Bridge.
#[derive(Debug, Clone)]
pub struct BridgeConfig {
    /// `issuer` stamped on every token (e.g. `wsf-trust-bridge`).
    pub issuer: String,
    /// `trust_bundle_version` stamped on every token.
    pub trust_bundle_version: String,
    /// Token lifetime; `expires_at = issued_at + token_ttl`.
    pub token_ttl: Duration,
    /// Subject-pseudonymization HMAC key (≥ `fabric_proof::MIN_TENANT_KEY_LEN`
    /// bytes). W9 makes this per-tenant and rotating; for now it is bridge-wide.
    pub subject_hmac_key: Vec<u8>,
    /// Optional `country` stamp (e.g. `US`).
    pub country: Option<String>,
    /// Optional `person_type` stamp (e.g. `us_person`).
    pub person_type: Option<String>,
}

impl BridgeConfig {
    /// A config with sane defaults (`wsf-trust-bridge` issuer, 15-minute TTL).
    #[must_use]
    pub fn new(trust_bundle_version: impl Into<String>, subject_hmac_key: Vec<u8>) -> Self {
        Self {
            issuer: "wsf-trust-bridge".to_string(),
            trust_bundle_version: trust_bundle_version.into(),
            token_ttl: Duration::from_secs(900),
            subject_hmac_key,
            country: None,
            person_type: None,
        }
    }

    /// Builder: override the token lifetime.
    #[must_use]
    pub fn with_token_ttl(mut self, ttl: Duration) -> Self {
        self.token_ttl = ttl;
        self
    }

    /// Builder: set the `country` / `person_type` stamps.
    #[must_use]
    pub fn with_locale(
        mut self,
        country: impl Into<String>,
        person_type: impl Into<String>,
    ) -> Self {
        self.country = Some(country.into());
        self.person_type = Some(person_type.into());
        self
    }
}

/// A request to issue a trust token for a subject within a tenant.
#[derive(Debug, Clone)]
pub struct IssueTokenRequest {
    /// Tenant whose authorization envelope bounds the token.
    pub tenant_id: String,
    /// Cleartext subject id — pseudonymized into `subject_hash`; never stored on
    /// the token.
    pub subject_id: String,
    /// Roles granted to the subject.
    pub roles: Vec<String>,
    /// Optional budget strand (spend ceilings).
    pub budget: Option<Budget>,
    /// Optional model allowlist (empty = unrestricted at this layer).
    pub allowed_models: Vec<String>,
    /// Server-authorized route ceiling. `None` preserves the bridge's legacy
    /// OpenBao tenant-envelope mapping; WSF API issuance always supplies it.
    pub allowed_routes: Option<Vec<Route>>,
    /// Server-authorized compliance scopes. `None` preserves the legacy bridge
    /// mapping; WSF API issuance always supplies it.
    pub compliance_scopes: Option<Vec<ComplianceScope>>,
    /// Server-authorized classification ceiling. `None` preserves the legacy
    /// bridge mapping; WSF API issuance always supplies it.
    pub max_data_classification: Option<Classification>,
    /// Service identity copied from authenticated server context, never the
    /// untrusted issue-token body.
    pub service_identity: Option<String>,
}

impl IssueTokenRequest {
    /// A minimal request: tenant + subject + roles, no budget or model allowlist.
    #[must_use]
    pub fn new(
        tenant_id: impl Into<String>,
        subject_id: impl Into<String>,
        roles: Vec<String>,
    ) -> Self {
        Self {
            tenant_id: tenant_id.into(),
            subject_id: subject_id.into(),
            roles,
            budget: None,
            allowed_models: Vec::new(),
            allowed_routes: None,
            compliance_scopes: None,
            max_data_classification: None,
            service_identity: None,
        }
    }

    /// Builder: attach a budget strand.
    #[must_use]
    pub fn with_budget(mut self, budget: Budget) -> Self {
        self.budget = Some(budget);
        self
    }

    /// Builder: restrict the model allowlist.
    #[must_use]
    pub fn with_models(mut self, models: Vec<String>) -> Self {
        self.allowed_models = models;
        self
    }

    /// Builder: attach the complete server-authorized token authority.
    #[must_use]
    pub fn with_authority(
        mut self,
        routes: Vec<Route>,
        scopes: Vec<ComplianceScope>,
        classification: Classification,
        service_identity: Option<String>,
    ) -> Self {
        self.allowed_routes = Some(routes);
        self.compliance_scopes = Some(scopes);
        self.max_data_classification = Some(classification);
        self.service_identity = service_identity;
        self
    }
}

/// Metadata-only correlation record for a token — safe to publish to the audit
/// ledger. Carries no cleartext subject and no payload (§2.2, "identity metadata
/// only"): a `subject_hash`, ids, and timestamps to correlate a token to its
/// downstream receipts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuditCorrelation {
    /// Token this record correlates.
    pub token_id: String,
    /// Owning tenant.
    pub tenant_id: String,
    /// Pseudonymized subject.
    pub subject_hash: String,
    /// Issuing bridge.
    pub issuer: String,
    /// Issue time (RFC3339).
    pub issued_at: String,
    /// Expiry (RFC3339).
    pub expires_at: String,
    /// Trust-bundle version in force at issue.
    pub trust_bundle_version: String,
}

/// The Trust Bridge: issues tokens and signs bundles / revocations.
pub struct TrustBridge {
    openbao: OpenBaoAuth,
    signer: Arc<dyn Signer>,
    config: BridgeConfig,
}

impl TrustBridge {
    /// Assemble a bridge from an OpenBao client, an ML-DSA signer, and config.
    #[must_use]
    pub fn new(openbao: OpenBaoAuth, signer: Arc<dyn Signer>, config: BridgeConfig) -> Self {
        Self {
            openbao,
            signer,
            config,
        }
    }

    /// The bridge's signing public key — hand this to off-host verifiers.
    #[must_use]
    pub fn public_key(&self) -> &[u8] {
        self.signer.public_key()
    }

    /// The bridge's signer (for callers that build their own signed artifacts).
    #[must_use]
    pub fn signer(&self) -> &dyn Signer {
        self.signer.as_ref()
    }

    /// The current trust-bundle version this bridge stamps on issued tokens.
    /// Used as the T6 "current" bundle so legacy (v1) tokens are refused
    /// attenuation.
    #[must_use]
    pub fn bundle_version(&self) -> &str {
        &self.config.trust_bundle_version
    }

    /// Issue a signed trust token: OpenBao auth event → tenant lookup → compose
    /// → ML-DSA sign. Stateless (its own login each call).
    ///
    /// # Errors
    /// [`BridgeError::OpenBao`] if auth or the tenant read fails,
    /// [`BridgeError::Config`] if a tenant attribute is unmappable, or a signing
    /// error.
    pub async fn issue_token(&self, req: &IssueTokenRequest) -> Result<TrustToken, BridgeError> {
        let vault_token = self.openbao.login().await?;
        let tenant = self
            .openbao
            .get_tenant(&vault_token, &req.tenant_id)
            .await?;
        let now = Utc::now();
        let token_id = format!("tok_{}", uuid::Uuid::new_v4());
        // Per-tenant HMAC key (W9) when the record carries one, else the bridge-wide key.
        let hmac_key = tenant
            .subject_hmac_key
            .as_ref()
            .and_then(|h| hex::decode(h).ok())
            .unwrap_or_else(|| self.config.subject_hmac_key.clone());
        let subject_hash = fabric_proof::hmac_subject(&hmac_key, &req.subject_id)?;
        let token = compose_token(&self.config, &tenant, req, token_id, subject_hash, now)?;
        Ok(fabric_token::issue(token, self.signer.as_ref())?)
    }

    /// Sign a policy-bundle payload with the bridge's ML-DSA key. The signature
    /// is over `BLAKE3(payload)` and verifies off-host with [`verify_bundle`].
    ///
    /// # Errors
    /// [`BridgeError::Crypto`] if signing fails.
    pub fn sign_bundle(&self, payload: &[u8]) -> Result<Signature, BridgeError> {
        let digest = blake3::hash(payload);
        let raw = self.signer.sign(digest.as_bytes())?;
        Ok(Signature {
            alg: self.signer.algorithm().to_string(),
            key_id: self.signer.key_id().to_string(),
            value: hex::encode(raw),
        })
    }

    /// Sign a revocation snapshot (delegates to `fabric-revocation`, same key).
    ///
    /// # Errors
    /// [`BridgeError::Revocation`] if signing fails.
    pub fn sign_revocation(
        &self,
        snapshot: RevocationSnapshot,
    ) -> Result<RevocationSnapshot, BridgeError> {
        Ok(fabric_revocation::sign(snapshot, self.signer.as_ref())?)
    }

    /// Extract the metadata-only correlation record for an issued token.
    #[must_use]
    pub fn audit_correlation(&self, token: &TrustToken) -> AuditCorrelation {
        AuditCorrelation {
            token_id: token.token_id.clone(),
            tenant_id: token.tenant_id.clone(),
            subject_hash: token.subject_hash.clone(),
            issuer: token.issuer.clone(),
            issued_at: token.issued_at.clone(),
            expires_at: token.expires_at.clone(),
            trust_bundle_version: token.trust_bundle_version.clone(),
        }
    }
}

/// Verify a policy-bundle signature off-host (public key only, no OpenBao).
#[must_use]
pub fn verify_bundle(
    payload: &[u8],
    signature: &Signature,
    verifier: &dyn Verifier,
    public_key: &[u8],
) -> bool {
    let digest = blake3::hash(payload);
    let Ok(raw) = hex::decode(&signature.value) else {
        return false;
    };
    verifier
        .verify(digest.as_bytes(), &raw, public_key)
        .unwrap_or(false)
}

/// Compose an unsigned trust token from a tenant's authorization envelope. Pure
/// (no I/O), so the mapping is unit-testable without OpenBao.
fn compose_token(
    config: &BridgeConfig,
    tenant: &TenantAttributes,
    req: &IssueTokenRequest,
    token_id: String,
    subject_hash: String,
    now: DateTime<Utc>,
) -> Result<TrustToken, BridgeError> {
    let expires = now
        + chrono::TimeDelta::from_std(config.token_ttl)
            .map_err(|e| BridgeError::Config(format!("token_ttl out of range: {e}")))?;
    let compliance_scopes = match &req.compliance_scopes {
        Some(scopes) => scopes.clone(),
        None => tenant
            .compliance_scopes
            .iter()
            .map(|s| parse_wire::<ComplianceScope>(s, "compliance scope"))
            .collect::<Result<Vec<_>, _>>()?,
    };
    let allowed_routes = match &req.allowed_routes {
        Some(routes) => routes.clone(),
        None => tenant
            .default_allowed_routes
            .iter()
            .map(|s| parse_wire::<Route>(s, "route"))
            .collect::<Result<Vec<_>, _>>()?,
    };
    let max_data_classification = match req.max_data_classification {
        Some(classification) => classification,
        None => parse_wire::<Classification>(&tenant.max_data_classification, "classification")?,
    };
    Ok(TrustToken {
        token_id,
        issued_at: now.to_rfc3339(),
        expires_at: expires.to_rfc3339(),
        issuer: config.issuer.clone(),
        trust_bundle_version: config.trust_bundle_version.clone(),
        tenant_id: tenant.tenant_id.clone(),
        subject_id: None, // pseudonymous: cleartext subject is never stored on the token
        subject_hash,
        service_identity: req.service_identity.clone(),
        identity_id: None,
        roles: req.roles.clone(),
        compliance_scopes,
        allowed_routes,
        allowed_models: req.allowed_models.clone(),
        max_data_classification,
        country: config.country.clone(),
        person_type: config.person_type.clone(),
        offline_mode: false,
        revocation_status: RevocationStatus::Valid,
        budget: req.budget.clone(),
        attenuation: Attenuation::default(),
        signature: Signature {
            alg: String::new(),
            key_id: String::new(),
            value: String::new(),
        },
    })
}

/// Map an OpenBao wire string to a contract enum via its serde representation,
/// failing closed (a `Config` error naming `kind`) on an unknown value.
fn parse_wire<T: serde::de::DeserializeOwned>(s: &str, kind: &str) -> Result<T, BridgeError> {
    serde_json::from_value(serde_json::Value::String(s.to_string()))
        .map_err(|_| BridgeError::Config(format!("unknown {kind}: {s}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use fabric_crypto::providers::{MlDsa87Verifier, RustCryptoMlDsa87};

    fn test_tenant() -> TenantAttributes {
        TenantAttributes {
            tenant_id: "tribal-health-demo".to_string(),
            display_name: "Tribal Health".to_string(),
            compliance_scopes: vec!["hipaa".to_string(), "ocap".to_string()],
            default_allowed_routes: vec!["local_only".to_string()],
            max_data_classification: "restricted".to_string(),
            subject_hmac_key: None,
        }
    }

    fn test_config() -> BridgeConfig {
        BridgeConfig::new("2026.07.03.001", vec![7u8; 32])
    }

    #[test]
    fn compose_maps_tenant_envelope() {
        let cfg = test_config();
        let req = IssueTokenRequest::new(
            "tribal-health-demo",
            "clinician-42",
            vec!["clinician".into()],
        );
        let subject_hash =
            fabric_proof::hmac_subject(&cfg.subject_hmac_key, &req.subject_id).unwrap();
        let tok = compose_token(
            &cfg,
            &test_tenant(),
            &req,
            "tok_x".to_string(),
            subject_hash.clone(),
            Utc::now(),
        )
        .unwrap();
        assert_eq!(tok.tenant_id, "tribal-health-demo");
        assert_eq!(
            tok.compliance_scopes,
            vec![ComplianceScope::Hipaa, ComplianceScope::Ocap]
        );
        assert_eq!(tok.allowed_routes, vec![Route::LocalOnly]);
        assert_eq!(tok.max_data_classification, Classification::Restricted);
        assert!(tok.subject_id.is_none());
        assert_eq!(tok.subject_hash, subject_hash);
        assert!(tok.subject_hash.starts_with(fabric_proof::HMAC_PREFIX));
    }

    #[test]
    fn compose_uses_server_authority_instead_of_broader_tenant_envelope() {
        let cfg = test_config();
        let req = IssueTokenRequest::new("tribal-health-demo", "service-a", vec!["worker".into()])
            .with_models(vec!["model-a".into()])
            .with_authority(
                vec![Route::LocalOnly],
                vec![ComplianceScope::Hipaa],
                Classification::Internal,
                Some("service-a".into()),
            );
        let tok = compose_token(
            &cfg,
            &test_tenant(),
            &req,
            "tok_policy".into(),
            "hmac:v1:test".into(),
            Utc::now(),
        )
        .unwrap();

        assert_eq!(tok.allowed_models, vec!["model-a"]);
        assert_eq!(tok.allowed_routes, vec![Route::LocalOnly]);
        assert_eq!(tok.compliance_scopes, vec![ComplianceScope::Hipaa]);
        assert_eq!(tok.max_data_classification, Classification::Internal);
        assert_eq!(tok.service_identity.as_deref(), Some("service-a"));
    }

    #[test]
    fn compose_rejects_unknown_scope() {
        let cfg = test_config();
        let mut tenant = test_tenant();
        tenant.compliance_scopes = vec!["hipaa".to_string(), "quantum".to_string()];
        let req = IssueTokenRequest::new("t", "s", vec![]);
        let err = compose_token(
            &cfg,
            &tenant,
            &req,
            "id".to_string(),
            "h".to_string(),
            Utc::now(),
        )
        .unwrap_err();
        assert!(matches!(err, BridgeError::Config(_)));
    }

    #[test]
    fn compose_issue_verify_roundtrip_offline() {
        let cfg = test_config();
        let signer = RustCryptoMlDsa87::generate("bridge-key").unwrap();
        let req = IssueTokenRequest::new(
            "tribal-health-demo",
            "clinician-42",
            vec!["clinician".into()],
        );
        let subject_hash =
            fabric_proof::hmac_subject(&cfg.subject_hmac_key, &req.subject_id).unwrap();
        let unsigned = compose_token(
            &cfg,
            &test_tenant(),
            &req,
            "tok_x".to_string(),
            subject_hash,
            Utc::now(),
        )
        .unwrap();
        let signed = fabric_token::issue(unsigned, &signer).unwrap();
        // off-host verify with the public key only
        fabric_token::verify(&signed, &MlDsa87Verifier, signer.public_key()).unwrap();
        // tamper → verify fails
        let mut tampered = signed.clone();
        tampered.tenant_id = "evil".to_string();
        assert!(fabric_token::verify(&tampered, &MlDsa87Verifier, signer.public_key()).is_err());
    }

    #[test]
    fn sign_and_verify_bundle_offline() {
        let signer = Arc::new(RustCryptoMlDsa87::generate("bundle-key").unwrap());
        let openbao = OpenBaoAuth::new(OpenBaoConfig::new("http://127.0.0.1:1", "r", "s")).unwrap();
        let bridge = TrustBridge::new(openbao, signer, test_config());
        let payload = b"policy-bundle-payload-v1";
        let sig = bridge.sign_bundle(payload).unwrap();
        assert!(verify_bundle(
            payload,
            &sig,
            &MlDsa87Verifier,
            bridge.public_key()
        ));
        assert!(!verify_bundle(
            b"tampered",
            &sig,
            &MlDsa87Verifier,
            bridge.public_key()
        ));
    }

    #[test]
    fn sign_revocation_verifies_offline() {
        let signer = Arc::new(RustCryptoMlDsa87::generate("rev-key").unwrap());
        let pubkey = signer.public_key().to_vec();
        let openbao = OpenBaoAuth::new(OpenBaoConfig::new("http://127.0.0.1:1", "r", "s")).unwrap();
        let bridge = TrustBridge::new(openbao, signer, test_config());
        let snap =
            RevocationSnapshot::new("snap-1", "2026-07-03T00:00:00Z", "2026-07-04T00:00:00Z");
        let signed = bridge.sign_revocation(snap).unwrap();
        fabric_revocation::verify(&signed, &MlDsa87Verifier, &pubkey).unwrap();
    }

    #[test]
    fn audit_correlation_is_metadata_only() {
        let signer = Arc::new(RustCryptoMlDsa87::generate("k").unwrap());
        let openbao = OpenBaoAuth::new(OpenBaoConfig::new("http://127.0.0.1:1", "r", "s")).unwrap();
        let bridge = TrustBridge::new(openbao, signer, test_config());
        let cfg = test_config();
        let req = IssueTokenRequest::new("tribal-health-demo", "clinician-42", vec![]);
        let subject_hash =
            fabric_proof::hmac_subject(&cfg.subject_hmac_key, &req.subject_id).unwrap();
        let tok = compose_token(
            &cfg,
            &test_tenant(),
            &req,
            "tok_x".to_string(),
            subject_hash.clone(),
            Utc::now(),
        )
        .unwrap();
        let corr = bridge.audit_correlation(&tok);
        assert_eq!(corr.token_id, "tok_x");
        assert_eq!(corr.subject_hash, subject_hash);
        // no cleartext subject anywhere in the correlation record
        let json = serde_json::to_string(&corr).unwrap();
        assert!(!json.contains("clinician-42"));
    }
}
