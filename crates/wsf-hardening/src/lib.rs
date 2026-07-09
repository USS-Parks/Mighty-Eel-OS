//! `wsf-hardening` — production readiness for the WSF trust plane.
//!
//! Two load-bearing pieces:
//!
//!   * [`KeyRing`] — a set of trust-anchor public keys accepted during a
//!     signing-key rotation. New tokens sign with the current key; tokens signed
//!     by a still-listed previous key keep verifying — so a key rotation has
//!     **zero downtime** (add the new key, migrate signers, then retire the old).
//!   * [`production_guard`] — rejects **dev fixtures** in a production deployment
//!     (the dev root token, plaintext HTTP to OpenBao, a weak/uniform HMAC key),
//!     closing the "OpenBao proven only against dev" debt by making dev config
//!     fail closed in production.
//!
//! The bridge and every WSF service are already **stateless per call** (each does
//! its own OpenBao login), so horizontal scale is a topology concern — see
//! `deployment/wsf-ha/` and `docs/architecture/WSF-HA.md`.

use fabric_contracts::TrustToken;
use fabric_crypto::Verifier;

/// A rotation ring of trust-anchor public keys (current first, older behind).
#[derive(Debug, Clone, Default)]
pub struct KeyRing {
    keys: Vec<Vec<u8>>,
}

impl KeyRing {
    /// A ring seeded with the current signing-anchor public key.
    #[must_use]
    pub fn new(current: Vec<u8>) -> Self {
        Self {
            keys: vec![current],
        }
    }

    /// Begin a rotation: `new_current` becomes the current key; the previous key
    /// stays accepted until [`retire_oldest`](Self::retire_oldest).
    pub fn rotate_in(&mut self, new_current: Vec<u8>) {
        self.keys.insert(0, new_current);
    }

    /// Close a rotation window: drop the oldest key (never empties the ring).
    pub fn retire_oldest(&mut self) {
        if self.keys.len() > 1 {
            self.keys.pop();
        }
    }

    /// The current signing-anchor public key.
    #[must_use]
    pub fn current(&self) -> Option<&[u8]> {
        self.keys.first().map(Vec::as_slice)
    }

    /// Number of accepted keys (1 outside a rotation window).
    #[must_use]
    pub fn len(&self) -> usize {
        self.keys.len()
    }

    /// Whether the ring is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }

    /// Verify a token against **any** key in the ring (the zero-downtime path).
    ///
    /// # Errors
    /// [`fabric_token::TokenError`] if no key verifies (or the token is revoked).
    pub fn verify_token(
        &self,
        token: &TrustToken,
        verifier: &dyn Verifier,
    ) -> Result<(), fabric_token::TokenError> {
        let mut last = fabric_token::TokenError::InvalidSignature;
        for key in &self.keys {
            match fabric_token::verify(token, verifier, key) {
                Ok(()) => return Ok(()),
                Err(e) => last = e,
            }
        }
        Err(last)
    }
}

/// Deployment mode. The guard is a no-op in [`DeployMode::Dev`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeployMode {
    /// Local/dev; guard disabled.
    Dev,
    /// Production; dev fixtures are rejected.
    Production,
}

/// The deployment configuration the guard inspects.
#[derive(Debug, Clone)]
pub struct DeploymentConfig {
    /// Dev or Production.
    pub mode: DeployMode,
    /// OpenBao address (must be `https://` in production).
    pub openbao_address: String,
    /// OpenBao auth token/marker (the dev `root` token is rejected in production).
    pub openbao_token: String,
    /// Subject-pseudonymization HMAC key (must be ≥32 bytes and not uniform).
    pub subject_hmac_key: Vec<u8>,
}

/// A single production-guard violation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GuardViolation {
    /// Stable machine code.
    pub code: &'static str,
    /// Human-readable detail.
    pub detail: String,
}

fn is_uniform(bytes: &[u8]) -> bool {
    bytes
        .first()
        .is_some_and(|first| bytes.iter().all(|b| b == first))
}

/// Inspect a deployment config and return the dev-fixture violations that would
/// block production (empty = production-ready). A no-op in [`DeployMode::Dev`].
#[must_use]
pub fn production_guard(cfg: &DeploymentConfig) -> Vec<GuardViolation> {
    let mut violations = Vec::new();
    if cfg.mode != DeployMode::Production {
        return violations;
    }
    if cfg.openbao_address.starts_with("http://") || !cfg.openbao_address.starts_with("https://") {
        violations.push(GuardViolation {
            code: "insecure_transport",
            detail: "OpenBao address must be https:// in production".to_string(),
        });
    }
    if cfg.openbao_token == "root" || cfg.openbao_token.is_empty() {
        violations.push(GuardViolation {
            code: "dev_root_token",
            detail: "the dev root token is not allowed in production".to_string(),
        });
    }
    if cfg.subject_hmac_key.len() < 32 {
        violations.push(GuardViolation {
            code: "weak_hmac_key",
            detail: "subject HMAC key must be at least 32 bytes".to_string(),
        });
    } else if is_uniform(&cfg.subject_hmac_key) {
        violations.push(GuardViolation {
            code: "dev_hmac_key",
            detail: "subject HMAC key is a dev fixture (uniform bytes)".to_string(),
        });
    }
    violations
}

/// Guard, returning `Err` with the violations if the config is not production-ready.
///
/// # Errors
/// The list of [`GuardViolation`]s when any dev fixture is present.
pub fn assert_production_ready(cfg: &DeploymentConfig) -> Result<(), Vec<GuardViolation>> {
    let violations = production_guard(cfg);
    if violations.is_empty() {
        Ok(())
    } else {
        Err(violations)
    }
}

/// The Loom control-plane facts the prod guard checks beyond the base
/// [`DeploymentConfig`]: the Raft voter count (a single-node quorum is not HA)
/// and whether the policy bundles it would serve are signed.
#[derive(Debug, Clone)]
pub struct LoomDeployment<'a> {
    /// The base WSF deployment config (OpenBao transport/token, HMAC key).
    pub config: &'a DeploymentConfig,
    /// Number of Raft voters in the control plane — must be `>= 3` in production.
    pub voter_count: usize,
    /// Whether every policy bundle to be served is signed.
    pub bundles_signed: bool,
}

/// The Loom production guard: the base WSF dev-fixture guard
/// ([`production_guard`]) **plus** Loom's HA and signed-bundle requirements —
/// reject a single-node quorum and an unsigned bundle in production. Empty =
/// production-ready. A no-op in [`DeployMode::Dev`].
#[must_use]
pub fn loom_production_guard(deployment: &LoomDeployment) -> Vec<GuardViolation> {
    let mut violations = production_guard(deployment.config);
    if deployment.config.mode != DeployMode::Production {
        return violations;
    }
    if deployment.voter_count < 3 {
        violations.push(GuardViolation {
            code: "single_node_quorum",
            detail: format!(
                "production consensus needs at least 3 voters; found {}",
                deployment.voter_count
            ),
        });
    }
    if !deployment.bundles_signed {
        violations.push(GuardViolation {
            code: "unsigned_bundle",
            detail: "an unsigned policy bundle is not allowed in production".to_string(),
        });
    }
    violations
}

/// Guard the Loom deployment, returning `Err` with the violations if it is not
/// production-ready.
///
/// # Errors
/// The list of [`GuardViolation`]s when any dev fixture, a single-node quorum, or
/// an unsigned bundle is present.
pub fn assert_loom_production_ready(
    deployment: &LoomDeployment,
) -> Result<(), Vec<GuardViolation>> {
    let violations = loom_production_guard(deployment);
    if violations.is_empty() {
        Ok(())
    } else {
        Err(violations)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fabric_contracts::{Attenuation, Classification, RevocationStatus, Signature, TrustToken};
    use fabric_crypto::Signer;
    use fabric_crypto::providers::{MlDsa87Verifier, RustCryptoMlDsa87};

    fn token(signer: &RustCryptoMlDsa87) -> TrustToken {
        let now = chrono::Utc::now();
        let t = TrustToken {
            token_id: "tok_hard".to_string(),
            issued_at: now.to_rfc3339(),
            expires_at: (now + chrono::Duration::hours(1)).to_rfc3339(),
            issuer: "wsf-trust-bridge".to_string(),
            trust_bundle_version: "2026.07.03".to_string(),
            tenant_id: "t".to_string(),
            subject_id: None,
            subject_hash: "h".to_string(),
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
        fabric_token::issue(t, signer).unwrap()
    }

    #[test]
    fn key_rotation_is_zero_downtime() {
        let key_a = RustCryptoMlDsa87::generate("A").unwrap();
        let key_b = RustCryptoMlDsa87::generate("B").unwrap();
        let tok_a = token(&key_a);
        let tok_b = token(&key_b);

        // Before rotation: only A accepted.
        let mut ring = KeyRing::new(key_a.public_key().to_vec());
        assert_eq!(ring.len(), 1);
        ring.verify_token(&tok_a, &MlDsa87Verifier).unwrap();
        assert!(ring.verify_token(&tok_b, &MlDsa87Verifier).is_err());

        // Mid-rotation: both A (old) and B (new) verify — zero downtime.
        ring.rotate_in(key_b.public_key().to_vec());
        assert_eq!(ring.len(), 2);
        assert_eq!(ring.current(), Some(key_b.public_key()));
        ring.verify_token(&tok_a, &MlDsa87Verifier).unwrap();
        ring.verify_token(&tok_b, &MlDsa87Verifier).unwrap();

        // After the window closes: only B accepted.
        ring.retire_oldest();
        assert_eq!(ring.len(), 1);
        ring.verify_token(&tok_b, &MlDsa87Verifier).unwrap();
        assert!(ring.verify_token(&tok_a, &MlDsa87Verifier).is_err());
    }

    #[test]
    fn production_guard_rejects_dev_fixtures() {
        // The exact dev fixtures used across the live tests.
        let dev = DeploymentConfig {
            mode: DeployMode::Production,
            openbao_address: "http://127.0.0.1:8250".to_string(),
            openbao_token: "root".to_string(),
            subject_hmac_key: vec![7u8; 32],
        };
        let violations = production_guard(&dev);
        let codes: Vec<_> = violations.iter().map(|v| v.code).collect();
        assert!(codes.contains(&"insecure_transport"));
        assert!(codes.contains(&"dev_root_token"));
        assert!(codes.contains(&"dev_hmac_key"));
        assert!(assert_production_ready(&dev).is_err());
    }

    #[test]
    fn production_guard_passes_a_hardened_config() {
        let mut key = [0u8; 32];
        rand::RngCore::fill_bytes(&mut rand::thread_rng(), &mut key);
        let prod = DeploymentConfig {
            mode: DeployMode::Production,
            openbao_address: "https://openbao.internal:8200".to_string(),
            openbao_token: "s.9fJ2k...approle-derived".to_string(),
            subject_hmac_key: key.to_vec(),
        };
        assert!(production_guard(&prod).is_empty());
        assert!(assert_production_ready(&prod).is_ok());
    }

    #[test]
    fn guard_is_noop_in_dev_mode() {
        let dev = DeploymentConfig {
            mode: DeployMode::Dev,
            openbao_address: "http://127.0.0.1:8250".to_string(),
            openbao_token: "root".to_string(),
            subject_hmac_key: vec![7u8; 32],
        };
        assert!(production_guard(&dev).is_empty());
    }

    #[test]
    fn short_key_flagged() {
        let cfg = DeploymentConfig {
            mode: DeployMode::Production,
            openbao_address: "https://ob:8200".to_string(),
            openbao_token: "real".to_string(),
            subject_hmac_key: vec![1u8; 16],
        };
        assert!(
            production_guard(&cfg)
                .iter()
                .any(|v| v.code == "weak_hmac_key")
        );
    }
}
