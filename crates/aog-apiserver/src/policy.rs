//! K7 — admission policy: deny-wins compliance over HIPAA/ITAR/OCAP plus per-kind
//! resource authority.
//!
//! A mutation is refused when the caller's token does not carry the authority the
//! resource asserts. Two checks, both fail-closed (doctrine D7):
//!
//!  1. **Per-kind resource authority.** A resource that declares a classification
//!     ceiling above the token's `max_data_classification` is denied — you cannot
//!     govern data more sensitive than your own authority.
//!  2. **Compliance, deny-wins.** For each compliance regime a resource declares
//!     (`compliance_scopes`), the token must hold that scope. The verdict is
//!     folded by the **mai-compliance `PolicyComposer`** (any regime's deny wins,
//!     the same engine the data-path gateway uses), so the control plane and the
//!     data plane share one composition contract.
//!
//! Evaluated locally from the token the front door (K6) already verified — no
//! OpenBao round-trip.

use aog_estate::{Kind, ResourceObject};
use fabric_contracts::{Classification, ComplianceScope};
use mai_compliance::{
    ComplianceReason, ComposerConfig, Destination, ModuleDecision, ModuleId, PolicyComposer,
};

use crate::admission::Principal;
use crate::error::ApiError;

/// The admission policy engine — the deny-wins compliance composer plus the
/// per-kind authority rule.
pub struct AdmissionPolicy {
    composer: PolicyComposer,
}

impl AdmissionPolicy {
    /// The baseline policy: all three compliance modules enabled
    /// (OCAP > ITAR > HIPAA), matching the data-path gateway's default.
    #[must_use]
    pub fn baseline() -> Self {
        Self {
            composer: PolicyComposer::new(ComposerConfig::default()),
        }
    }

    /// Evaluate a mutation against policy. `Ok(())` admits; a violation is an
    /// [`ApiError::Forbidden`] carrying the specific reason(s).
    ///
    /// # Errors
    /// [`ApiError::Forbidden`] when the resource asserts authority the token does
    /// not hold (classification over-reach, or an unheld compliance regime).
    pub fn evaluate(&self, object: &ResourceObject, principal: &Principal) -> Result<(), ApiError> {
        // The system principal (internal controllers, later phases) carries no
        // token; it is trusted and skips policy. API requests always carry one.
        let Some(token) = &principal.token else {
            return Ok(());
        };
        let facts = policy_facts(object);

        // 1. Per-kind resource authority: classification ceiling <= token max.
        if let Some(ceiling) = facts.classification_ceiling
            && ceiling > token.max_data_classification
        {
            return Err(ApiError::Forbidden(format!(
                "{} classification ceiling {:?} exceeds the token's authority {:?}",
                object.kind(),
                ceiling,
                token.max_data_classification
            )));
        }

        // 2. Compliance, deny-wins over the regimes the resource declares.
        if facts.compliance_scopes.is_empty() {
            return Ok(());
        }
        let decisions: Vec<ModuleDecision> = facts
            .compliance_scopes
            .iter()
            .map(|scope| {
                scope_decision(
                    *scope,
                    token.compliance_scopes.contains(scope),
                    object.kind(),
                )
            })
            .collect();
        let aggregate = self.composer.compose(decisions.iter().cloned());
        if aggregate.allowed {
            Ok(())
        } else {
            let reason = decisions
                .iter()
                .filter(|d| !d.allowed)
                .flat_map(|d| d.reasons.iter().map(|r| r.summary.clone()))
                .collect::<Vec<_>>()
                .join("; ");
            Err(ApiError::Forbidden(reason))
        }
    }
}

/// The authority a resource asserts, extracted per kind.
struct PolicyFacts {
    classification_ceiling: Option<Classification>,
    compliance_scopes: Vec<ComplianceScope>,
}

fn policy_facts(object: &ResourceObject) -> PolicyFacts {
    match object {
        ResourceObject::Tenant(r) => PolicyFacts {
            classification_ceiling: Some(r.spec.classification_ceiling),
            compliance_scopes: r.spec.compliance_scopes.clone(),
        },
        ResourceObject::Workload(r) => PolicyFacts {
            classification_ceiling: Some(r.spec.classification_ceiling),
            compliance_scopes: Vec::new(),
        },
        ResourceObject::Node(r) => PolicyFacts {
            classification_ceiling: Some(r.spec.attestation_floor),
            compliance_scopes: Vec::new(),
        },
        ResourceObject::Capability(r) => PolicyFacts {
            classification_ceiling: Some(r.spec.max_classification),
            compliance_scopes: Vec::new(),
        },
        _ => PolicyFacts {
            classification_ceiling: None,
            compliance_scopes: Vec::new(),
        },
    }
}

fn module_of(scope: ComplianceScope) -> ModuleId {
    match scope {
        ComplianceScope::Hipaa => ModuleId::Hipaa,
        ComplianceScope::ItarEar => ModuleId::Itar,
        ComplianceScope::Ocap => ModuleId::Ocap,
    }
}

fn scope_decision(scope: ComplianceScope, held: bool, kind: Kind) -> ModuleDecision {
    let module = module_of(scope);
    let (allowed, route) = if held {
        (true, Destination::Cloud)
    } else {
        (false, Destination::Local)
    };
    let summary = if held {
        format!("{kind} is within the token's {} authority", module.as_str())
    } else {
        format!(
            "{kind} requires {} compliance authority the token does not hold",
            module.as_str()
        )
    };
    ModuleDecision {
        module,
        allowed,
        route,
        flags: Vec::new(),
        reasons: vec![ComplianceReason::new(module, None, summary)],
    }
}
