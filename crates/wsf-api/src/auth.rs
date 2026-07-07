//! `wsf-api::auth` — front-door authentication for privileged WSF issuance.
//!
//! Token *issuance* mints authority from nothing, so it cannot present a trust
//! token as its own authority — that is precisely what it is asking for. It must
//! instead prove *who is asking* with a server-verifiable identity, and the
//! tenant / subject / roles of the issued token are copied from that verified
//! principal, never from the request body (AF-002).
//!
//! Production authenticates a signed workload-identity assertion
//! (`fabric_contracts::Identity`, ML-DSA-verified under the identity anchor) and
//! maps it, via server-side role policy, to a [`WsfPrincipal`]. Local development
//! uses an explicit [`DevAuthenticator`] the production binary never constructs.
//! With no authenticator wired the default is [`DenyAllAuthenticator`] — the
//! privileged plane fails closed rather than minting tokens for anyone.

use std::collections::HashMap;

use axum::http::HeaderMap;
use base64::Engine;
use chrono::{DateTime, Utc};
use fabric_contracts::{Identity, WsfPrincipal};
use fabric_crypto::providers::MlDsa87Verifier;

/// Header carrying a base64-encoded JSON signed [`Identity`] assertion.
pub const IDENTITY_HEADER: &str = "x-wsf-identity";
/// Header the dev authenticator reads: a base64-encoded JSON [`WsfPrincipal`].
pub const DEV_PRINCIPAL_HEADER: &str = "x-wsf-dev-principal";

/// Why authentication was refused. Every failure resolves toward *less* privilege
/// and leaks nothing about why (no identity oracle).
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum AuthError {
    /// Missing, malformed, unverifiable, or expired identity.
    #[error("unauthenticated")]
    Unauthenticated,
    /// Authenticated, but not permitted for this request (e.g. wrong audience).
    #[error("forbidden")]
    Forbidden,
}

/// Produces a verified [`WsfPrincipal`] for a privileged request, or refuses.
pub trait WsfAuthenticator: Send + Sync {
    /// Authenticate the request headers into a principal at time `now`.
    ///
    /// # Errors
    /// [`AuthError`] when identity is missing, malformed, unverifiable, expired,
    /// or not permitted.
    fn authenticate(
        &self,
        headers: &HeaderMap,
        now: DateTime<Utc>,
    ) -> Result<WsfPrincipal, AuthError>;
}

fn decode_header<T: serde::de::DeserializeOwned>(
    headers: &HeaderMap,
    name: &str,
) -> Result<T, AuthError> {
    let raw = headers
        .get(name)
        .and_then(|v| v.to_str().ok())
        .ok_or(AuthError::Unauthenticated)?;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(raw.trim())
        .map_err(|_| AuthError::Unauthenticated)?;
    serde_json::from_slice(&bytes).map_err(|_| AuthError::Unauthenticated)
}

/// Production authenticator: verifies a signed workload-identity assertion under
/// the identity anchor, checks expiry, and maps it to a principal via server-side
/// role policy. The caller's request body is never an identity authority.
pub struct SignedIdentityAuthenticator {
    identity_anchor_pk: Vec<u8>,
    audience: String,
    /// Identity key (`service_identity`, else `subject_id`) → authorized roles.
    role_grants: HashMap<String, Vec<String>>,
}

impl SignedIdentityAuthenticator {
    /// A new authenticator anchored on `identity_anchor_pk`, stamping `audience`.
    #[must_use]
    pub fn new(identity_anchor_pk: Vec<u8>, audience: impl Into<String>) -> Self {
        Self {
            identity_anchor_pk,
            audience: audience.into(),
            role_grants: HashMap::new(),
        }
    }

    /// Builder: authorize `roles` for the identity keyed by `identity_key`.
    #[must_use]
    pub fn with_role_grant(mut self, identity_key: impl Into<String>, roles: Vec<String>) -> Self {
        self.role_grants.insert(identity_key.into(), roles);
        self
    }

    fn roles_for(&self, identity: &Identity) -> Vec<String> {
        let key = identity
            .service_identity
            .clone()
            .unwrap_or_else(|| identity.subject_id.clone());
        self.role_grants.get(&key).cloned().unwrap_or_default()
    }
}

impl WsfAuthenticator for SignedIdentityAuthenticator {
    fn authenticate(
        &self,
        headers: &HeaderMap,
        now: DateTime<Utc>,
    ) -> Result<WsfPrincipal, AuthError> {
        let identity: Identity = decode_header(headers, IDENTITY_HEADER)?;
        // 1. The assertion must verify under the identity anchor.
        fabric_identity::verify(&identity, &MlDsa87Verifier, &self.identity_anchor_pk)
            .map_err(|_| AuthError::Unauthenticated)?;
        // 2. Expiry — fail closed on an unparseable or past `expires_at`.
        let exp = DateTime::parse_from_rfc3339(&identity.expires_at)
            .map_err(|_| AuthError::Unauthenticated)?
            .with_timezone(&Utc);
        if exp <= now {
            return Err(AuthError::Unauthenticated);
        }
        // 3. A verified assertion must still name a tenant + subject.
        if identity.tenant_id.is_empty() || identity.subject_id.is_empty() {
            return Err(AuthError::Forbidden);
        }
        // 4. Roles come from server-side policy, never the caller.
        Ok(WsfPrincipal {
            roles: self.roles_for(&identity),
            tenant_id: identity.tenant_id,
            subject_id: identity.subject_id,
            service_identity: identity.service_identity,
            audience: self.audience.clone(),
            auth_method: "workload-identity".to_string(),
            credential_id: identity.signature.key_id,
            correlation_id: identity.identity_id,
        })
    }
}

/// A fail-closed authenticator that refuses every request — the safe default when
/// no real authenticator is configured. Production must wire one explicitly.
pub struct DenyAllAuthenticator;

impl WsfAuthenticator for DenyAllAuthenticator {
    fn authenticate(&self, _: &HeaderMap, _: DateTime<Utc>) -> Result<WsfPrincipal, AuthError> {
        Err(AuthError::Unauthenticated)
    }
}

/// Local-development authenticator: trusts a base64 [`WsfPrincipal`] header. The
/// production binary NEVER constructs it; its use is an explicit dev opt-in.
pub struct DevAuthenticator {
    audience: String,
}

impl DevAuthenticator {
    /// A dev authenticator stamping `audience` (and `auth_method = "dev"`).
    #[must_use]
    pub fn new(audience: impl Into<String>) -> Self {
        Self {
            audience: audience.into(),
        }
    }
}

impl WsfAuthenticator for DevAuthenticator {
    fn authenticate(
        &self,
        headers: &HeaderMap,
        _now: DateTime<Utc>,
    ) -> Result<WsfPrincipal, AuthError> {
        let mut principal: WsfPrincipal = decode_header(headers, DEV_PRINCIPAL_HEADER)?;
        principal.audience = self.audience.clone();
        principal.auth_method = "dev".to_string();
        Ok(principal)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fabric_contracts::{IdentityKind, Signature};
    use fabric_crypto::Signer;
    use fabric_crypto::providers::RustCryptoMlDsa87;

    fn identity(tenant: &str, subject: &str, expires_at: &str) -> Identity {
        Identity {
            identity_id: "id_1".into(),
            kind: IdentityKind::Workload,
            tenant_id: tenant.into(),
            subject_id: subject.into(),
            subject_hash: String::new(),
            service_identity: Some("aog-gateway".into()),
            spiffe_id: String::new(),
            pki_cert_fingerprint: String::new(),
            parent_id: None,
            issued_at: "2026-07-03T00:00:00Z".into(),
            expires_at: expires_at.into(),
            signature: Signature {
                alg: String::new(),
                key_id: String::new(),
                value: String::new(),
            },
        }
    }

    fn header_for(id: &Identity) -> HeaderMap {
        let mut h = HeaderMap::new();
        let b64 = base64::engine::general_purpose::STANDARD.encode(serde_json::to_vec(id).unwrap());
        h.insert(IDENTITY_HEADER, b64.parse().unwrap());
        h
    }

    #[test]
    fn signed_identity_yields_principal_with_policy_roles() {
        let anchor = RustCryptoMlDsa87::generate("identity-anchor").unwrap();
        let id = fabric_identity::mint(
            identity("baap", "clinician-42", "2099-01-01T00:00:00Z"),
            &anchor,
        )
        .unwrap();
        let auth = SignedIdentityAuthenticator::new(anchor.public_key().to_vec(), "wsf")
            .with_role_grant("aog-gateway", vec!["clinician".into()]);
        let p = auth.authenticate(&header_for(&id), Utc::now()).unwrap();
        assert_eq!(p.tenant_id, "baap");
        assert_eq!(p.subject_id, "clinician-42");
        assert_eq!(p.roles, vec!["clinician".to_string()]); // from policy, not the caller
        assert_eq!(p.auth_method, "workload-identity");
    }

    #[test]
    fn missing_identity_is_unauthenticated() {
        let anchor = RustCryptoMlDsa87::generate("k").unwrap();
        let auth = SignedIdentityAuthenticator::new(anchor.public_key().to_vec(), "wsf");
        assert_eq!(
            auth.authenticate(&HeaderMap::new(), Utc::now()),
            Err(AuthError::Unauthenticated)
        );
    }

    #[test]
    fn wrong_anchor_key_is_rejected() {
        let anchor = RustCryptoMlDsa87::generate("k").unwrap();
        let attacker = RustCryptoMlDsa87::generate("evil").unwrap();
        let id = fabric_identity::mint(identity("baap", "s", "2099-01-01T00:00:00Z"), &attacker)
            .unwrap();
        let auth = SignedIdentityAuthenticator::new(anchor.public_key().to_vec(), "wsf");
        assert_eq!(
            auth.authenticate(&header_for(&id), Utc::now()),
            Err(AuthError::Unauthenticated)
        );
    }

    #[test]
    fn expired_identity_is_rejected() {
        let anchor = RustCryptoMlDsa87::generate("k").unwrap();
        let id =
            fabric_identity::mint(identity("baap", "s", "2020-01-01T00:00:00Z"), &anchor).unwrap();
        let auth = SignedIdentityAuthenticator::new(anchor.public_key().to_vec(), "wsf");
        assert_eq!(
            auth.authenticate(&header_for(&id), Utc::now()),
            Err(AuthError::Unauthenticated)
        );
    }

    #[test]
    fn unknown_identity_gets_no_roles() {
        let anchor = RustCryptoMlDsa87::generate("k").unwrap();
        let id =
            fabric_identity::mint(identity("baap", "s", "2099-01-01T00:00:00Z"), &anchor).unwrap();
        // No role grant for this identity → fail-closed empty roles.
        let auth = SignedIdentityAuthenticator::new(anchor.public_key().to_vec(), "wsf");
        let p = auth.authenticate(&header_for(&id), Utc::now()).unwrap();
        assert!(p.roles.is_empty());
    }

    #[test]
    fn deny_all_refuses_everything() {
        assert_eq!(
            DenyAllAuthenticator.authenticate(&HeaderMap::new(), Utc::now()),
            Err(AuthError::Unauthenticated)
        );
    }
}
