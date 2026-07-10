//! Server-side cloud-credential grants (plan B1/B2).
//!
//! The public credential-exchange API takes a tenant-scoped `grant_id`, never a
//! raw cloud identity (the broker used to accept a caller-selected AWS
//! role ARN). A [`GrantStore`] resolves `(tenant, grant_id)` to an approved
//! cloud identity + scope; a missing or cross-tenant grant denies. Production
//! loads signed / OpenBao-custodied mappings; dev/tests use [`StaticGrants`].

use std::collections::HashMap;

/// The non-AWS cloud a grant may target. AWS uses the [`CloudGrant`] role
/// fields directly (the shipped exchange path); Azure and GCP carry their
/// server-resolved identity here so the Azure/GCP brokers get the same
/// grant-resolved scope AWS does — the caller still names only a `grant_id`,
/// never a raw Azure scope or GCP service account.
#[derive(Debug, Clone, Default)]
pub enum GrantCloud {
    /// AWS STS: the identity is the [`CloudGrant`] `role_arn` + `allowed_actions`.
    #[default]
    Aws,
    /// Azure AD: the approved OAuth scope.
    Azure {
        /// The approved Azure AD OAuth scope.
        scope: String,
    },
    /// GCP IAM Credentials: the approved service account + OAuth scopes.
    Gcp {
        /// The approved service account to impersonate.
        service_account: String,
        /// The approved downstream OAuth scopes.
        scopes: Vec<String>,
    },
}

/// An approved cloud-credential grant. Every field is server-side truth — the
/// caller never submits any of it (plan B1/B3).
#[derive(Debug, Clone)]
pub struct CloudGrant {
    /// Tenant this grant belongs to.
    pub tenant_id: String,
    /// Opaque grant identifier the caller references.
    pub grant_id: String,
    /// The approved AWS role ARN the broker may assume for this grant.
    pub role_arn: String,
    /// IAM actions the brokered session policy allows (plan B3 — least
    /// privilege). Empty approves nothing: the policy denies all.
    pub allowed_actions: Vec<String>,
    /// Optional `ExternalId` the role's trust policy requires
    /// (confused-deputy defense).
    pub external_id: Option<String>,
    /// Optional region override; `None` uses the broker default.
    pub region: Option<String>,
    /// Optional maximum credential TTL (seconds) for this grant.
    pub max_ttl_secs: Option<u64>,
    /// Which cloud this grant targets. Defaults to AWS (the role fields above).
    pub cloud: GrantCloud,
}

impl CloudGrant {
    /// Convert to the broker's [`wsf_broker::GrantScope`] — the shape the STS
    /// call binds (role, actions, region, external id, TTL ceiling).
    #[must_use]
    pub fn to_scope(&self) -> wsf_broker::GrantScope {
        wsf_broker::GrantScope {
            role_arn: self.role_arn.clone(),
            allowed_actions: self.allowed_actions.clone(),
            region: self.region.clone(),
            external_id: self.external_id.clone(),
            max_ttl_secs: self.max_ttl_secs.and_then(|v| i64::try_from(v).ok()),
        }
    }

    /// The Azure broker scope this grant resolves to, or `None` if the grant is
    /// not an Azure grant. The same tenant-scoped `grant_id` → server-side
    /// scope indirection AWS uses.
    #[must_use]
    pub fn to_azure_scope(&self) -> Option<wsf_broker::AzureGrantScope> {
        match &self.cloud {
            GrantCloud::Azure { scope } => Some(wsf_broker::AzureGrantScope::new(scope.clone())),
            _ => None,
        }
    }

    /// The GCP broker scope this grant resolves to, or `None` if the grant is
    /// not a GCP grant.
    #[must_use]
    pub fn to_gcp_scope(&self) -> Option<wsf_broker::GcpGrantScope> {
        match &self.cloud {
            GrantCloud::Gcp {
                service_account,
                scopes,
            } => Some(wsf_broker::GcpGrantScope {
                service_account: service_account.clone(),
                scopes: scopes.clone(),
            }),
            _ => None,
        }
    }
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

    /// Convenience: a single dev grant mapping `grant_id` → `role_arn` for
    /// `tenant`, approving a read-only S3 action pair (dev default — real
    /// grants list their actions explicitly).
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
            allowed_actions: vec!["s3:GetObject".to_string(), "s3:ListBucket".to_string()],
            external_id: None,
            region: None,
            max_ttl_secs: None,
            cloud: GrantCloud::Aws,
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

    #[test]
    fn scope_binds_grant_fields_and_bounded_actions() {
        // B3: the scope handed to the broker carries the grant's exact action
        // list (never a wildcard), external id, region, and TTL ceiling.
        let grant = CloudGrant {
            tenant_id: "t-a".to_string(),
            grant_id: "g1".to_string(),
            role_arn: "arn:aws:iam::111:role/x".to_string(),
            allowed_actions: vec!["s3:GetObject".to_string()],
            external_id: Some("wsf-ext-9".to_string()),
            region: Some("eu-central-1".to_string()),
            max_ttl_secs: Some(1200),
            cloud: GrantCloud::Aws,
        };
        let scope = grant.to_scope();
        assert_eq!(scope.role_arn, "arn:aws:iam::111:role/x");
        assert_eq!(scope.allowed_actions, vec!["s3:GetObject".to_string()]);
        assert_eq!(scope.external_id.as_deref(), Some("wsf-ext-9"));
        assert_eq!(scope.region.as_deref(), Some("eu-central-1"));
        assert_eq!(scope.max_ttl_secs, Some(1200));
        // An AWS grant resolves to no Azure/GCP scope.
        assert!(grant.to_azure_scope().is_none());
        assert!(grant.to_gcp_scope().is_none());

        // The dev grant approves a bounded read-only action set — no "*".
        let dev = StaticGrants::single_dev("t", "g", "arn:aws:iam::1:role/y")
            .grant_for("t", "g")
            .unwrap();
        assert!(!dev.allowed_actions.is_empty());
        assert!(dev.allowed_actions.iter().all(|a| a != "*"));
    }

    #[test]
    fn azure_and_gcp_grants_resolve_to_broker_scopes() {
        // An Azure/GCP grant resolves — server-side, from a tenant-scoped
        // grant_id — to the broker's scope type. The caller never names the
        // Azure scope or GCP service account; it names the grant_id.
        let azure = CloudGrant {
            tenant_id: "t-a".to_string(),
            grant_id: "az-blob".to_string(),
            role_arn: String::new(),
            allowed_actions: vec![],
            external_id: None,
            region: None,
            max_ttl_secs: None,
            cloud: GrantCloud::Azure {
                scope: "https://storage.azure.com/.default".to_string(),
            },
        };
        let az = azure.to_azure_scope().expect("azure grant resolves");
        assert_eq!(az.scope, "https://storage.azure.com/.default");
        assert!(azure.to_gcp_scope().is_none());

        let gcp = CloudGrant {
            tenant_id: "t-a".to_string(),
            grant_id: "gcp-storage".to_string(),
            role_arn: String::new(),
            allowed_actions: vec![],
            external_id: None,
            region: None,
            max_ttl_secs: None,
            cloud: GrantCloud::Gcp {
                service_account: "sa@proj.iam.gserviceaccount.com".to_string(),
                scopes: vec!["https://www.googleapis.com/auth/devstorage.read_only".to_string()],
            },
        };
        let g = gcp.to_gcp_scope().expect("gcp grant resolves");
        assert_eq!(g.service_account, "sa@proj.iam.gserviceaccount.com");
        assert_eq!(g.scopes.len(), 1);
        assert!(gcp.to_azure_scope().is_none());
    }
}
