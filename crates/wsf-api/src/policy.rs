//! Server-side tenant issuance policy (plan A3).
//!
//! Issuance authority — which roles a principal may receive, the classification
//! ceiling, the budget ceiling, and the model allowlist — is *server-side*
//! truth keyed by the authenticated principal's tenant. The caller supplies
//! bounded intent only ([`crate::IssueReq`]); the handler intersects that intent
//! with the tenant's policy. A request for anything the policy does not grant is
//! denied, never silently honored.

use std::collections::{BTreeSet, HashMap};

use fabric_contracts::{Budget, Classification};

/// The issuance authority a tenant confers on its principals.
#[derive(Debug, Clone)]
pub struct TenantIssuancePolicy {
    /// Tenant this policy governs.
    pub tenant_id: String,
    /// Roles a principal in this tenant may be granted. A requested role outside
    /// this set is refused.
    pub grantable_roles: BTreeSet<String>,
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
    /// ceiling, and no model restriction. Never a production policy source.
    #[must_use]
    pub fn single_dev(tenant_id: impl Into<String>, roles: &[&str]) -> Self {
        let tenant_id = tenant_id.into();
        Self::new().with(TenantIssuancePolicy {
            tenant_id,
            grantable_roles: roles.iter().map(ToString::to_string).collect(),
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
}
