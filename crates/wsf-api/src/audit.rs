//! Global-auditor authorization (plan L2 remainder).
//!
//! Tenant scoping is the default and only posture for `/v1/receipts`.
//! A **global auditor** is the one server-designated exception: a principal the
//! operator has explicitly enrolled — by authenticated `principal_id`, never by
//! anything the caller submits — who may read receipts across tenants and
//! export the signed evidence pack. Production backs this with signed /
//! OpenBao-custodied enrollment; dev and tests use [`StaticAuditors`].

use std::collections::HashSet;

use fabric_contracts::WsfPrincipal;

/// Decide whether an authenticated principal is a global auditor.
pub trait AuditorStore: Send + Sync {
    /// Is this principal enrolled as a global auditor?
    fn is_global_auditor(&self, principal: &WsfPrincipal) -> bool;
}

/// In-memory auditor enrollment for development and tests.
#[derive(Debug, Clone, Default)]
pub struct StaticAuditors {
    principal_ids: HashSet<String>,
}

impl StaticAuditors {
    /// No auditors — every cross-tenant read and export is denied (the safe
    /// default for every deployment that has not explicitly enrolled one).
    #[must_use]
    pub fn none() -> Self {
        Self::default()
    }

    /// Enroll `principal_id` as a global auditor.
    #[must_use]
    pub fn with(mut self, principal_id: impl Into<String>) -> Self {
        self.principal_ids.insert(principal_id.into());
        self
    }
}

impl AuditorStore for StaticAuditors {
    fn is_global_auditor(&self, principal: &WsfPrincipal) -> bool {
        self.principal_ids.contains(&principal.principal_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fabric_contracts::{Audience, AuthStrength, AuthenticatedFacts, IdentityKind};

    fn principal(id: &str) -> WsfPrincipal {
        WsfPrincipal::establish(
            AuthenticatedFacts {
                principal_id: id.to_string(),
                kind: IdentityKind::Human,
                tenant_id: "tenant-a".to_string(),
                subject_hash: "hmac:x".to_string(),
                service_identity: None,
                roles: vec!["auditor".to_string()],
                token_lineage: None,
                auth_strength: AuthStrength::MutualTls,
                audience: Audience::Wsf,
            },
            "corr-1",
            "2026-07-07T00:00:00Z",
        )
    }

    #[test]
    fn only_enrolled_principals_are_auditors() {
        let auditors = StaticAuditors::none().with("auditor-1");
        assert!(auditors.is_global_auditor(&principal("auditor-1")));
        assert!(!auditors.is_global_auditor(&principal("local-dev")));
        assert!(!StaticAuditors::none().is_global_auditor(&principal("auditor-1")));
    }
}
