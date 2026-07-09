//! Trust Manifold projection types.
//!
//! Defines the [`TrustContext`] struct consumed by every Lamprey
//! component onward, along with the supporting enums
//! (service identity, compliance scope, allowed route, data
//! classification, revocation status). The wire-level claim that
//! produces a `TrustContext` is defined in `docs/compliance/TRUST-MANIFOLD.md`
//! §4; this module is the in-memory decision-time projection.
//!
//! The split is deliberate:
//!
//! - A **claim** is the signed JSON artefact that crosses the network
//!   between the Lamprey Trust Bridge and the appliance. It is
//!   verified once at the trust-cache boundary.
//! - A **TrustContext** is the flat Rust struct the policy runtime
//!   passes around after verification. It carries fields drawn from
//!   the claim plus two appliance-state fields ([`TrustContext::offline_mode`]
//!   and [`TrustContext::revocation_status`]) that come from the local
//!   trust cache, not from the claim itself.
//!
//! (signed bundle verification) and (local trust
//! cache) land, callers construct `TrustContext` directly from mock
//! values via [`TrustContext::for_local_dev`] or
//! [`TrustContext::strict_local_only`]. will replace these
//! construction sites with calls into the verified-claim pipeline.

use std::collections::BTreeSet;

use mai_core::airgap::ConnectivityState;
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Tenant identifier. Stable, lowercase kebab-case.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct TenantId(String);

impl TenantId {
    /// Construct a tenant id. Rejects empty strings and obvious shape
    /// violations. Stricter normalisation when the
    /// claim verifier is wired.
    pub fn new(id: impl Into<String>) -> Result<Self, TrustContextError> {
        let raw = id.into();
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return Err(TrustContextError::InvalidTenantId(raw));
        }
        Ok(Self(trimmed.to_string()))
    }

    /// Borrowed view.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// In-memory subject identifier. Never logged in raw form — the audit
/// path must use [`SubjectHash`].
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SubjectId(String);

impl SubjectId {
    /// Construct from a raw subject identifier.
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    /// Borrowed view. Intended for in-process routing only — do NOT
    /// pass this into any audit, log, or telemetry sink.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// HMAC of [`SubjectId`] with a per-tenant key. Safe to log.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SubjectHash(String);

impl SubjectHash {
    /// Wrap a pre-computed hash string.
    pub fn new(h: impl Into<String>) -> Self {
        Self(h.into())
    }

    /// Borrowed view.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// One of the nine service identities defined in
/// `docs/compliance/SERVICE-IDENTITY.md` §2.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ServiceIdentity {
    /// HTTP / gRPC ingress on the appliance.
    MaiApi,
    /// Inference placement.
    MaiScheduler,
    /// Backend adapter lifecycle.
    MaiAdapterManager,
    /// Request classification + sensitivity scoring.
    LampreyRouter,
    /// HIPAA / ITAR-EAR / OCAP policy evaluation.
    LampreyPolicy,
    /// Tamper-evident audit log writer.
    LampreyAudit,
    /// Read-only dashboard backend.
    LampreyDashboard,
    /// Bundle and revocation snapshot fetcher.
    LocalTrustCache,
    /// Metadata-only audit sync upstream (cloud-side).
    AuditCorrelationService,
}

impl ServiceIdentity {
    /// Wire-format identifier (also the kebab-case the OpenBao policy
    /// path uses).
    pub fn as_str(self) -> &'static str {
        match self {
            Self::MaiApi => "mai-api",
            Self::MaiScheduler => "mai-scheduler",
            Self::MaiAdapterManager => "mai-adapter-manager",
            Self::LampreyRouter => "lamprey-router",
            Self::LampreyPolicy => "lamprey-policy",
            Self::LampreyAudit => "lamprey-audit",
            Self::LampreyDashboard => "lamprey-dashboard",
            Self::LocalTrustCache => "local-trust-cache",
            Self::AuditCorrelationService => "audit-correlation-service",
        }
    }
}

/// Compliance domains the policy runtime is permitted to evaluate
/// against. Absence of a scope means "must not evaluate" — not
/// "no concern". See `docs/compliance/SERVICE-IDENTITY.md` §4.3.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ComplianceScope {
    /// HIPAA engine may evaluate.
    Hipaa,
    /// ITAR / EAR jurisdiction may evaluate.
    ItarEar,
    /// OCAP may evaluate.
    Ocap,
}

impl ComplianceScope {
    /// Wire-format identifier.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Hipaa => "hipaa",
            Self::ItarEar => "itar_ear",
            Self::Ocap => "ocap",
        }
    }
}

/// Hard routing ceiling carried on every claim.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AllowedRoute {
    /// Even uncontrolled content stays local.
    LocalOnly,
    /// Local first; cloud route allowed only after classification passes.
    LocalPreferred,
    /// No route ceiling from the trust layer (compliance still applies).
    CloudAllowed,
}

impl AllowedRoute {
    /// Wire-format identifier.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::LocalOnly => "local_only",
            Self::LocalPreferred => "local_preferred",
            Self::CloudAllowed => "cloud_allowed",
        }
    }
}

/// Data classification ceiling. Ordered least- to most-sensitive so
/// `>=` can be used as a gate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DataClassification {
    /// Public information; no restriction.
    Public,
    /// Internal-only material.
    Internal,
    /// Restricted (HIPAA-style sensitive).
    Restricted,
    /// Export-controlled (ITAR/EAR-relevant).
    Controlled,
    /// Top-tier secret material.
    Secret,
}

impl DataClassification {
    /// Wire-format identifier.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Public => "public",
            Self::Internal => "internal",
            Self::Restricted => "restricted",
            Self::Controlled => "controlled",
            Self::Secret => "secret",
        }
    }
}

/// Local revocation status carried into the decision frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RevocationStatus {
    /// In current snapshot and not listed as revoked.
    Valid,
    /// Explicitly revoked by the bridge.
    Revoked,
    /// Snapshot is past soft expiry but before hard expiry.
    Stale,
    /// No fresh snapshot is available.
    Unknown,
}

impl RevocationStatus {
    /// Wire-format identifier.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Valid => "valid",
            Self::Revoked => "revoked",
            Self::Stale => "stale",
            Self::Unknown => "unknown",
        }
    }
}

/// Errors at construction time.
#[derive(Debug, Error)]
pub enum TrustContextError {
    /// Tenant id did not pass shape validation.
    #[error("invalid tenant id: '{0}'")]
    InvalidTenantId(String),
}

/// Decision-time projection of a verified Lamprey claim plus local
/// appliance state. Every Lamprey component onward
/// takes a `&TrustContext` on its decision path.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrustContext {
    /// Tenant the subject belongs to.
    pub tenant_id: TenantId,
    /// In-memory subject identifier. Never logged.
    pub subject_id: SubjectId,
    /// HMAC of the subject id; safe to log.
    pub subject_hash: SubjectHash,
    /// Application-defined RBAC roles.
    #[serde(default)]
    pub roles: BTreeSet<String>,
    /// Compliance engines this claim authorises the runtime to apply.
    #[serde(default)]
    pub compliance_scopes: BTreeSet<ComplianceScope>,
    /// Hard routing ceiling.
    #[serde(default)]
    pub allowed_routes: BTreeSet<AllowedRoute>,
    /// Tenant- or subject-specific model allowlist. Empty = no restriction.
    #[serde(default)]
    pub allowed_models: BTreeSet<String>,
    /// Highest classification this subject is authorised for.
    pub max_data_classification: DataClassification,
    /// Present iff this is a service-to-service claim.
    #[serde(default)]
    pub service_identity: Option<ServiceIdentity>,
    /// Version of the policy bundle this claim was issued against.
    pub trust_bundle_version: String,
    /// Globally unique claim id; audit correlation key.
    pub claim_id: String,
    /// Appliance connectivity state. Upgraded from `offline_mode: bool`
    /// / — the canonical enum lives in
    /// [`mai_core::airgap::ConnectivityState`]. Use [`Self::offline_mode`]
    /// for the legacy boolean view.
    #[serde(default = "default_connectivity")]
    pub connectivity: ConnectivityState,
    /// Appliance state: revocation snapshot lookup result.
    pub revocation_status: RevocationStatus,
}

/// Default connectivity used when deserialising legacy claims that
/// predate the field. `Connected` was the historical implicit
/// default for everything except the explicit `offline_mode: true`
/// case, which now maps to `Degraded` at construction sites.
fn default_connectivity() -> ConnectivityState {
    ConnectivityState::Connected
}

impl TrustContext {
    /// Permissive local-dev context. Use for tests and bring-up work
    /// before the verified-claim pipeline lands. Disposable.
    pub fn for_local_dev() -> Self {
        let mut scopes = BTreeSet::new();
        scopes.insert(ComplianceScope::Hipaa);
        scopes.insert(ComplianceScope::ItarEar);
        scopes.insert(ComplianceScope::Ocap);
        let mut routes = BTreeSet::new();
        routes.insert(AllowedRoute::CloudAllowed);
        routes.insert(AllowedRoute::LocalPreferred);
        routes.insert(AllowedRoute::LocalOnly);
        Self {
            tenant_id: TenantId::new("local-dev").expect("static id"),
            subject_id: SubjectId::new("local-dev-subject"),
            subject_hash: SubjectHash::new("hmac:local-dev"),
            roles: BTreeSet::new(),
            compliance_scopes: scopes,
            allowed_routes: routes,
            allowed_models: BTreeSet::new(),
            max_data_classification: DataClassification::Secret,
            service_identity: None,
            trust_bundle_version: "local-dev".to_string(),
            claim_id: "local-dev-claim".to_string(),
            connectivity: ConnectivityState::Connected,
            revocation_status: RevocationStatus::Valid,
        }
    }

    /// Local-only context with all scopes enabled. Useful for testing
    /// the allowed-route ceiling path.
    pub fn strict_local_only() -> Self {
        let mut ctx = Self::for_local_dev();
        ctx.allowed_routes.clear();
        ctx.allowed_routes.insert(AllowedRoute::LocalOnly);
        ctx
    }

    /// Convenience: true when the claim authorises the runtime to
    /// evaluate the given compliance scope.
    pub fn allows_scope(&self, scope: ComplianceScope) -> bool {
        self.compliance_scopes.contains(&scope)
    }

    /// Backwards-compatible accessor for the legacy `offline_mode` flag.
    /// Returns true for anything that is not [`ConnectivityState::Connected`].
    #[must_use]
    pub fn offline_mode(&self) -> bool {
        self.connectivity.is_offline_mode()
    }

    /// Convenience: true when the claim permits a cloud route. A cloud
    /// route requires:
    ///   - connectivity that permits cloud routes (Connected or Degraded)
    ///   - the `CloudAllowed` ceiling
    ///   - a `Valid` revocation status
    pub fn permits_cloud_route(&self) -> bool {
        self.connectivity.permits_cloud_route()
            && self.allowed_routes.contains(&AllowedRoute::CloudAllowed)
            && self.revocation_status == RevocationStatus::Valid
    }

    /// True when the connectivity state forces local-only execution
    /// regardless of the routing ceiling (Expired or AirGapped).
    #[must_use]
    pub fn requires_local_only(&self) -> bool {
        self.connectivity.requires_local_only()
    }

    /// True when revocation status disqualifies the claim outright.
    pub fn is_revoked(&self) -> bool {
        matches!(self.revocation_status, RevocationStatus::Revoked)
    }

    /// True when the bridge claim is missing a fresh revocation
    /// snapshot. The policy runtime treats this as `revoked` for
    /// ITAR content and `stale` for uncontrolled content
    /// (see SERVICE-IDENTITY.md §4.5).
    pub fn revocation_unknown(&self) -> bool {
        matches!(self.revocation_status, RevocationStatus::Unknown)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tenant_id_rejects_empty() {
        assert!(TenantId::new("").is_err());
        assert!(TenantId::new("   ").is_err());
        assert!(TenantId::new("bay-area-pediatrics").is_ok());
    }

    #[test]
    fn test_service_identity_strings() {
        assert_eq!(ServiceIdentity::LampreyRouter.as_str(), "lamprey-router");
        assert_eq!(ServiceIdentity::MaiApi.as_str(), "mai-api");
        assert_eq!(
            ServiceIdentity::AuditCorrelationService.as_str(),
            "audit-correlation-service"
        );
    }

    #[test]
    fn test_compliance_scope_strings() {
        assert_eq!(ComplianceScope::Hipaa.as_str(), "hipaa");
        assert_eq!(ComplianceScope::ItarEar.as_str(), "itar_ear");
        assert_eq!(ComplianceScope::Ocap.as_str(), "ocap");
    }

    #[test]
    fn test_data_classification_ordering() {
        assert!(DataClassification::Public < DataClassification::Internal);
        assert!(DataClassification::Internal < DataClassification::Restricted);
        assert!(DataClassification::Restricted < DataClassification::Controlled);
        assert!(DataClassification::Controlled < DataClassification::Secret);
    }

    #[test]
    fn test_local_dev_context_is_permissive() {
        let ctx = TrustContext::for_local_dev();
        assert!(ctx.allows_scope(ComplianceScope::Hipaa));
        assert!(ctx.allows_scope(ComplianceScope::ItarEar));
        assert!(ctx.allows_scope(ComplianceScope::Ocap));
        assert!(ctx.permits_cloud_route());
        assert!(!ctx.is_revoked());
        assert_eq!(ctx.max_data_classification, DataClassification::Secret);
    }

    #[test]
    fn test_strict_local_only_blocks_cloud() {
        let ctx = TrustContext::strict_local_only();
        assert!(!ctx.permits_cloud_route());
        assert!(ctx.allowed_routes.contains(&AllowedRoute::LocalOnly));
        assert_eq!(ctx.allowed_routes.len(), 1);
    }

    #[test]
    fn test_offline_mode_blocks_cloud_route_even_when_allowed() {
        let mut ctx = TrustContext::for_local_dev();
        // Degraded keeps the cloud route open (cached validation still works).
        ctx.connectivity = ConnectivityState::Degraded;
        assert!(ctx.permits_cloud_route());
        // StaleNotExpired forbids the cloud route.
        ctx.connectivity = ConnectivityState::StaleNotExpired;
        assert!(!ctx.permits_cloud_route());
        assert!(ctx.offline_mode());
        // AirGapped forbids the cloud route and forces local-only.
        ctx.connectivity = ConnectivityState::AirGapped;
        assert!(!ctx.permits_cloud_route());
        assert!(ctx.requires_local_only());
    }

    #[test]
    fn test_expired_connectivity_requires_local_only() {
        let mut ctx = TrustContext::for_local_dev();
        ctx.connectivity = ConnectivityState::Expired;
        assert!(ctx.requires_local_only());
        assert!(!ctx.permits_cloud_route());
        assert!(ctx.offline_mode());
    }

    #[test]
    fn test_revoked_disqualifies_cloud_route() {
        let mut ctx = TrustContext::for_local_dev();
        ctx.revocation_status = RevocationStatus::Revoked;
        assert!(ctx.is_revoked());
        assert!(!ctx.permits_cloud_route());
    }

    #[test]
    fn test_unknown_revocation_is_distinct_from_revoked() {
        let mut ctx = TrustContext::for_local_dev();
        ctx.revocation_status = RevocationStatus::Unknown;
        assert!(ctx.revocation_unknown());
        assert!(!ctx.is_revoked());
    }

    #[test]
    fn test_service_identity_optional_for_humans() {
        let ctx = TrustContext::for_local_dev();
        assert!(ctx.service_identity.is_none());
    }

    #[test]
    fn test_serde_roundtrip() {
        let ctx = TrustContext::for_local_dev();
        let json = serde_json::to_string(&ctx).expect("serialise");
        let back: TrustContext = serde_json::from_str(&json).expect("deserialise");
        assert_eq!(ctx, back);
    }

    #[test]
    fn test_scope_missing_detected() {
        let mut ctx = TrustContext::for_local_dev();
        ctx.compliance_scopes.remove(&ComplianceScope::ItarEar);
        assert!(!ctx.allows_scope(ComplianceScope::ItarEar));
        assert!(ctx.allows_scope(ComplianceScope::Hipaa));
    }
}
