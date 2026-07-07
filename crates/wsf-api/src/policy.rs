//! Server-side tenant issuance policy (plan A3).
//!
//! Issuance authority — which roles a principal may receive, the classification
//! ceiling, the budget ceiling, and the model allowlist — is *server-side*
//! truth keyed by the authenticated principal's tenant. The caller supplies
//! bounded intent only ([`crate::IssueReq`]); the handler intersects that intent
//! with the tenant's policy. A request for anything the policy does not grant is
//! denied, never silently honored.

use std::collections::{BTreeSet, HashMap};

use fabric_contracts::{Budget, Classification, IdentityKind};
use serde::Serialize;

/// How a token is being issued (plan A4). Determines which issuance permission
/// the tenant policy must grant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum IssuanceMode {
    /// A human/session principal minting a token for itself (a leaf token).
    SelfService,
    /// A workload/task principal minting a token to call another service.
    ServiceToService,
    /// Issuance carrying an administrative role (broad authority).
    Administrative,
}

impl IssuanceMode {
    /// Whether this mode mints tokens intended to be delegated/attenuated
    /// downstream (so the tenant must allow a non-zero delegation depth).
    #[must_use]
    pub fn is_delegation_capable(self) -> bool {
        matches!(
            self,
            IssuanceMode::ServiceToService | IssuanceMode::Administrative
        )
    }

    /// Stable label for receipts.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            IssuanceMode::SelfService => "self_service",
            IssuanceMode::ServiceToService => "service_to_service",
            IssuanceMode::Administrative => "administrative",
        }
    }
}

/// The issuance authority a tenant confers on its principals.
#[derive(Debug, Clone)]
pub struct TenantIssuancePolicy {
    /// Tenant this policy governs.
    pub tenant_id: String,
    /// Roles a principal in this tenant may be granted. A requested role outside
    /// this set is refused.
    pub grantable_roles: BTreeSet<String>,
    /// Roles that constitute administrative issuance. Requesting one classifies
    /// the issuance as [`IssuanceMode::Administrative`].
    pub admin_roles: BTreeSet<String>,
    /// Issuance modes this tenant permits (plan A4). A mode outside this set is
    /// refused with a deny receipt.
    pub permitted_modes: BTreeSet<IssuanceMode>,
    /// Maximum delegation depth for tokens minted here. `0` forbids delegation,
    /// so delegation-capable modes are refused. Enforced at the chain level in T4.
    pub max_delegation_depth: u32,
    /// Highest classification a token from this tenant may carry.
    pub max_classification: Classification,
    /// Per-token budget ceiling. A requested budget above any counter is refused;
    /// an unspecified budget is granted exactly this ceiling (never unlimited).
    pub max_budget: Budget,
    /// Model allowlist. Empty = unrestricted at this layer; non-empty = a
    /// requested model outside the list is refused.
    pub allowed_models: Vec<String>,
}

impl TenantIssuancePolicy {
    /// Whether `role` may be granted to a principal in this tenant.
    #[must_use]
    pub fn may_grant_role(&self, role: &str) -> bool {
        self.grantable_roles.contains(role)
    }

    /// Whether `model` may be attached (empty allowlist ⇒ unrestricted).
    #[must_use]
    pub fn allows_model(&self, model: &str) -> bool {
        self.allowed_models.is_empty() || self.allowed_models.iter().any(|m| m == model)
    }

    /// Classify the issuance (plan A4) from the principal kind and requested
    /// roles: an admin role ⇒ administrative; else a machine principal ⇒
    /// service-to-service; else self-service.
    #[must_use]
    pub fn classify(&self, kind: IdentityKind, requested_roles: &[String]) -> IssuanceMode {
        if requested_roles.iter().any(|r| self.admin_roles.contains(r)) {
            IssuanceMode::Administrative
        } else if matches!(kind, IdentityKind::Workload | IdentityKind::Task) {
            IssuanceMode::ServiceToService
        } else {
            IssuanceMode::SelfService
        }
    }

    /// Whether this tenant permits `mode` at all.
    #[must_use]
    pub fn permits_mode(&self, mode: IssuanceMode) -> bool {
        self.permitted_modes.contains(&mode)
    }
}

/// Lookup of issuance policy by tenant. Production loads signed / OpenBao-held
/// mappings (plan B2 extends this seam); dev/tests use [`StaticTenantPolicies`].
pub trait TenantPolicyStore: Send + Sync {
    /// The policy for `tenant_id`, or `None` if the tenant has none (⇒ deny).
    fn policy_for(&self, tenant_id: &str) -> Option<TenantIssuancePolicy>;
}

/// In-memory policy set for development and tests.
#[derive(Debug, Clone, Default)]
pub struct StaticTenantPolicies {
    by_tenant: HashMap<String, TenantIssuancePolicy>,
}

impl StaticTenantPolicies {
    /// Empty store — every tenant is denied until one is added.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Add or replace a tenant's policy.
    #[must_use]
    pub fn with(mut self, policy: TenantIssuancePolicy) -> Self {
        self.by_tenant.insert(policy.tenant_id.clone(), policy);
        self
    }

    /// Convenience: a single dev tenant granting `roles`, a default budget
    /// ceiling, no model restriction, all three issuance modes permitted, and a
    /// modest delegation depth. `admin` (if present in `roles`) is an admin role.
    /// Never a production policy source.
    #[must_use]
    pub fn single_dev(tenant_id: impl Into<String>, roles: &[&str]) -> Self {
        let tenant_id = tenant_id.into();
        let admin_roles = roles
            .iter()
            .filter(|r| **r == "admin")
            .map(ToString::to_string)
            .collect();
        Self::new().with(TenantIssuancePolicy {
            tenant_id,
            grantable_roles: roles.iter().map(ToString::to_string).collect(),
            admin_roles,
            permitted_modes: [
                IssuanceMode::SelfService,
                IssuanceMode::ServiceToService,
                IssuanceMode::Administrative,
            ]
            .into_iter()
            .collect(),
            max_delegation_depth: 3,
            max_classification: Classification::Restricted,
            max_budget: Budget {
                token_cap: 1_000_000,
                usd_cap_cents: 100_000,
                tool_call_cap: 10_000,
                ..Budget::default()
            },
            allowed_models: Vec::new(),
        })
    }
}

impl TenantPolicyStore for StaticTenantPolicies {
    fn policy_for(&self, tenant_id: &str) -> Option<TenantIssuancePolicy> {
        self.by_tenant.get(tenant_id).cloned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_tenant_has_no_policy() {
        let s = StaticTenantPolicies::single_dev("t-a", &["user"]);
        assert!(s.policy_for("t-a").is_some());
        assert!(s.policy_for("t-b").is_none());
    }

    #[test]
    fn role_and_model_gating() {
        let p = StaticTenantPolicies::single_dev("t", &["user", "clinician"])
            .policy_for("t")
            .unwrap();
        assert!(p.may_grant_role("clinician"));
        assert!(!p.may_grant_role("admin"));
        // empty allowlist ⇒ unrestricted
        assert!(p.allows_model("anything"));
    }

    #[test]
    fn issuance_mode_classification_matrix() {
        let p = StaticTenantPolicies::single_dev("t", &["user", "admin"])
            .policy_for("t")
            .unwrap();
        // Human/session principal, ordinary role → self-service (leaf).
        assert_eq!(
            p.classify(IdentityKind::Human, &["user".into()]),
            IssuanceMode::SelfService
        );
        assert_eq!(
            p.classify(IdentityKind::Session, &[]),
            IssuanceMode::SelfService
        );
        // Machine principals → service-to-service.
        assert_eq!(
            p.classify(IdentityKind::Workload, &["user".into()]),
            IssuanceMode::ServiceToService
        );
        assert_eq!(
            p.classify(IdentityKind::Task, &[]),
            IssuanceMode::ServiceToService
        );
        // An admin role wins regardless of principal kind.
        assert_eq!(
            p.classify(IdentityKind::Human, &["admin".into()]),
            IssuanceMode::Administrative
        );
        assert_eq!(
            p.classify(IdentityKind::Workload, &["user".into(), "admin".into()]),
            IssuanceMode::Administrative
        );
    }

    #[test]
    fn delegation_capability_and_mode_permission() {
        let p = StaticTenantPolicies::single_dev("t", &["user"])
            .policy_for("t")
            .unwrap();
        assert!(p.permits_mode(IssuanceMode::SelfService));
        assert!(p.max_delegation_depth > 0);
        assert!(!IssuanceMode::SelfService.is_delegation_capable());
        assert!(IssuanceMode::ServiceToService.is_delegation_capable());
        assert!(IssuanceMode::Administrative.is_delegation_capable());
    }
}
