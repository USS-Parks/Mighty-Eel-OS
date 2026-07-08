//! WSF principal contract (plan A1) — see `contracts/identity.md`.
//!
//! A [`WsfPrincipal`] is the **authenticated** answer to *who is calling*,
//! established by the transport authenticator ([`crate` consumer] `wsf-api`'s
//! authenticator seam, plan A2) from a verified credential — mTLS client cert,
//! workload token, or an explicit local-dev credential. It is deliberately
//! **not** the same thing as an [`Identity`](crate::Identity) claim carried in a
//! request body: a caller may *assert* an identity in JSON, but only the
//! authenticator may *establish* a principal.
//!
//! # Why this type does not implement `Deserialize`
//!
//! The single most damaging WSF finding (AF-002) was that privileged routes
//! read `tenant_id` / `subject_id` / `roles` straight from request JSON, so any
//! caller could self-assign authority. The structural fix is a type the wire
//! layer *cannot* manufacture: `WsfPrincipal` derives [`Serialize`] (so it can
//! be written into receipts and audit records) but **not** `Deserialize`.
//!
//! Because of that, none of these compile — the boundary is enforced by the
//! type system, not by review vigilance:
//!
//! ```compile_fail
//! use fabric_contracts::WsfPrincipal;
//! // axum's `Json<T>` / `serde_json::from_str::<T>` both require `T: Deserialize`.
//! let _p: WsfPrincipal = serde_json::from_str("{}").unwrap();
//! ```
//!
//! ```compile_fail
//! use fabric_contracts::WsfPrincipal;
//! fn assert_de<T: serde::de::DeserializeOwned>() {}
//! assert_de::<WsfPrincipal>(); // WsfPrincipal: DeserializeOwned is not satisfied
//! ```
//!
//! The only way to obtain one is [`WsfPrincipal::establish`], which the
//! authenticator calls after it has verified a credential.

use serde::{Deserialize, Serialize};

use crate::identity::IdentityKind;

/// How strongly the calling principal was authenticated. Ordered weakest →
/// strongest; production policy (plan A2/V1) rejects [`AuthStrength::LocalDev`].
///
/// This value enum is `Deserialize` (config/credential parsing needs it); the
/// no-JSON boundary applies to [`WsfPrincipal`] as a whole, not its fields.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthStrength {
    /// Explicit local-dev credential. Never valid under the production profile.
    LocalDev,
    /// Bearer workload token (e.g. OpenBao AppRole secret-id, SPIFFE JWT-SVID).
    WorkloadToken,
    /// Mutual TLS with a verified client-certificate chain.
    MutualTls,
}

impl AuthStrength {
    /// Whether this strength is acceptable outside local development.
    #[must_use]
    pub fn is_production_grade(self) -> bool {
        !matches!(self, AuthStrength::LocalDev)
    }
}

/// The plane a principal is authenticated *for*. A token minted for one
/// audience must not verify at another (plan T1/A4 audience binding).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Audience {
    /// The WSF trust-fabric control plane (token issue / attenuate / seal).
    Wsf,
    /// The AOG gateway.
    Aog,
    /// The MAI inference API.
    Mai,
}

/// The authenticated principal behind a privileged call.
///
/// Constructed only by [`WsfPrincipal::establish`] from a verified credential;
/// never deserialized from a request. Carries no cleartext subject — only the
/// pseudonymized `subject_hash`, matching the token/receipt contracts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct WsfPrincipal {
    /// Stable identifier of the authenticated principal (e.g. SPIFFE ID,
    /// certificate subject, or AppRole role name).
    pub principal_id: String,
    /// What kind of principal this is.
    pub kind: IdentityKind,
    /// Tenant the credential is bound to. Server-authoritative — the caller
    /// cannot influence it.
    pub tenant_id: String,
    /// Pseudonymized subject. Empty for pure workload principals.
    pub subject_hash: String,
    /// Service identity for workload/service principals, if any.
    pub service_identity: Option<String>,
    /// How strongly the principal was authenticated.
    pub auth_strength: AuthStrength,
    /// Plane this principal is authenticated for.
    pub audience: Audience,
    /// Correlation id threaded into every receipt this principal produces.
    pub correlation_id: String,
    /// RFC3339 instant the authenticator established this principal.
    pub authenticated_at: String,
    /// Private witness: makes `WsfPrincipal { .. }` literal construction
    /// impossible outside this module, so [`establish`](Self::establish) is the
    /// *only* constructor even within the crate's own consumers.
    _sealed: Sealed,
}

/// Zero-size construction witness. Private field ⇒ no external struct literal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct Sealed;

/// Everything the authenticator has proven about a credential, minus the
/// derived plumbing (`correlation_id`, `authenticated_at`) that `establish`
/// stamps. Grouping the proven facts keeps the constructor call-site honest.
#[derive(Debug, Clone)]
pub struct AuthenticatedFacts {
    pub principal_id: String,
    pub kind: IdentityKind,
    pub tenant_id: String,
    pub subject_hash: String,
    pub service_identity: Option<String>,
    pub auth_strength: AuthStrength,
    pub audience: Audience,
}

impl WsfPrincipal {
    /// Establish a principal from facts a credential authenticator has already
    /// verified. This is the sole constructor; the wire layer has no path to it.
    #[must_use]
    pub fn establish(
        facts: AuthenticatedFacts,
        correlation_id: impl Into<String>,
        authenticated_at: impl Into<String>,
    ) -> Self {
        Self {
            principal_id: facts.principal_id,
            kind: facts.kind,
            tenant_id: facts.tenant_id,
            subject_hash: facts.subject_hash,
            service_identity: facts.service_identity,
            auth_strength: facts.auth_strength,
            audience: facts.audience,
            correlation_id: correlation_id.into(),
            authenticated_at: authenticated_at.into(),
            _sealed: Sealed,
        }
    }

    /// Whether this principal may act on the given plane.
    #[must_use]
    pub fn is_for(&self, audience: Audience) -> bool {
        self.audience == audience
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts() -> AuthenticatedFacts {
        AuthenticatedFacts {
            principal_id: "spiffe://mai/wsf/issuer".into(),
            kind: IdentityKind::Workload,
            tenant_id: "tenant-a".into(),
            subject_hash: "blake3:deadbeef".into(),
            service_identity: Some("issuer".into()),
            auth_strength: AuthStrength::MutualTls,
            audience: Audience::Wsf,
        }
    }

    #[test]
    fn establish_is_the_only_constructor_and_stamps_derived_fields() {
        let p = WsfPrincipal::establish(facts(), "corr-1", "2026-07-07T00:00:00Z");
        assert_eq!(p.tenant_id, "tenant-a");
        assert_eq!(p.correlation_id, "corr-1");
        assert_eq!(p.authenticated_at, "2026-07-07T00:00:00Z");
        assert!(p.is_for(Audience::Wsf));
        assert!(!p.is_for(Audience::Aog));
    }

    #[test]
    fn principal_serializes_for_receipts_without_leaking_cleartext_subject() {
        let p = WsfPrincipal::establish(facts(), "corr-2", "2026-07-07T00:00:00Z");
        let json = serde_json::to_value(&p).unwrap();
        // Serializable (receipts/audit need it) …
        assert_eq!(json["tenant_id"], "tenant-a");
        assert_eq!(json["auth_strength"], "mutual_tls");
        assert_eq!(json["audience"], "wsf");
        // … but only the hash, never a cleartext subject id, and no witness leak
        // beyond an empty struct.
        assert_eq!(json["subject_hash"], "blake3:deadbeef");
        assert!(json.get("subject_id").is_none());
    }

    #[test]
    fn auth_strength_orders_and_gates_production() {
        assert!(AuthStrength::MutualTls > AuthStrength::LocalDev);
        assert!(AuthStrength::WorkloadToken.is_production_grade());
        assert!(AuthStrength::MutualTls.is_production_grade());
        assert!(!AuthStrength::LocalDev.is_production_grade());
    }
}
