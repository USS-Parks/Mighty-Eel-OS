//! Server-side cloud-credential grants (plan B1/B2).
//!
//! The public credential-exchange API takes a tenant-scoped `grant_id`, never a
//! raw cloud identity (AF-004: the broker used to accept a caller-selected AWS
//! role ARN). A [`GrantStore`] resolves `(tenant, grant_id)` to an approved
//! cloud identity + scope; a missing or cross-tenant grant denies. Production
//! loads signed / OpenBao-custodied mappings; dev/tests use [`StaticGrants`].

use std::collections::HashMap;

/// An approved cloud-credential grant. The `role_arn` (and future scope fields)
/// are server-side truth — the caller never submits them.
#[derive(Debug, Clone)]
pub struct CloudGrant {
    /// Tenant this grant belongs to.
    pub tenant_id: String,
    /// Opaque grant identifier the caller references.
    pub grant_id: String,
    /// The approved AWS role ARN the broker may assume for this grant.
    pub role_arn: String,
    /// Optional region override; `None` uses the broker default.
    pub region: Option<String>,
    /// Optional maximum credential TTL (seconds) for this grant.
    pub max_ttl_secs: Option<u64>,
}

/// Resolve a `(tenant, grant_id)` to an approved [`CloudGrant`]. `None` denies.
pub trait GrantStore: Send + Sync {
    /// The grant for `grant_id` within `tenant_id`, if one is approved.
    fn grant_for(&self, tenant_id: &str, grant_id: &str) -> Option<CloudGrant>;
}

/// In-memory grant set for development and tests.
#[derive(Debug, Clone, Default)]
pub struct StaticGrants {
    by_key: HashMap<(String, String), CloudGrant>,
}

impl StaticGrants {
    /// Empty store — every grant is denied until one is added.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Add or replace a grant.
    #[must_use]
    pub fn with(mut self, grant: CloudGrant) -> Self {
        self.by_key
            .insert((grant.tenant_id.clone(), grant.grant_id.clone()), grant);
        self
    }

    /// Convenience: a single dev grant mapping `grant_id` → `role_arn` for `tenant`.
    #[must_use]
    pub fn single_dev(
        tenant_id: impl Into<String>,
        grant_id: impl Into<String>,
        role_arn: impl Into<String>,
    ) -> Self {
        Self::new().with(CloudGrant {
            tenant_id: tenant_id.into(),
            grant_id: grant_id.into(),
            role_arn: role_arn.into(),
            region: None,
            max_ttl_secs: None,
        })
    }
}

impl GrantStore for StaticGrants {
    fn grant_for(&self, tenant_id: &str, grant_id: &str) -> Option<CloudGrant> {
        self.by_key
            .get(&(tenant_id.to_string(), grant_id.to_string()))
            .cloned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_only_the_right_tenant_and_grant() {
        let s = StaticGrants::single_dev("t-a", "g1", "arn:aws:iam::111:role/x");
        assert_eq!(
            s.grant_for("t-a", "g1").unwrap().role_arn,
            "arn:aws:iam::111:role/x"
        );
        // Wrong grant id, or right grant under the wrong tenant → denied.
        assert!(s.grant_for("t-a", "g2").is_none());
        assert!(s.grant_for("t-b", "g1").is_none());
    }
}
