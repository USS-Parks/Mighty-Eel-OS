//! Router core: trait, decision types, and the default composition.
//!
//! The `Router` trait is the public surface every caller programs against.
//! `DefaultRouter` is the production implementation that composes the
//! classifier, entity scanner, and budget tracker into a single
//! deterministic decision with an audit-grade reason string.

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use thiserror::Error;
use tracing::debug;

use crate::classifier::{Classification, SensitivityClassifier};
use crate::cost::{BudgetCheck, BudgetTracker};
use crate::entities::{EntityKind, EntityMatch, EntityScanner};

/// Cloud frontier-model providers the router may route to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CloudProvider {
    /// Anthropic Claude.
    Anthropic,
    /// OpenAI GPT family.
    OpenAi,
    /// Google Gemini family.
    Google,
}

impl CloudProvider {
    /// Wire-format string.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Anthropic => "anthropic",
            Self::OpenAi => "openai",
            Self::Google => "google",
        }
    }
}

/// What the router decided to do with the request.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub enum RoutingDecision {
    /// Process the request locally via the MAI inference engine.
    Local {
        /// Human-readable reason — emitted to audit.
        reason: String,
        /// Classification level observed.
        classification: Classification,
    },
    /// Route to a cloud provider.
    Cloud {
        /// Provider selected.
        provider: CloudProvider,
        /// Provider-side model name.
        model: String,
        /// Human-readable reason — emitted to audit.
        reason: String,
        /// Classification level observed.
        classification: Classification,
    },
    /// Reject the request entirely.
    Denied {
        /// Stable code for programmatic checks.
        code: String,
        /// Human-readable reason — emitted to audit.
        reason: String,
        /// Classification level observed (may be Critical).
        classification: Classification,
    },
}

impl RoutingDecision {
    /// Convenience: the audit reason string regardless of variant.
    pub fn reason(&self) -> &str {
        match self {
            Self::Local { reason, .. }
            | Self::Cloud { reason, .. }
            | Self::Denied { reason, .. } => reason,
        }
    }

    /// Convenience: the classification regardless of variant.
    pub fn classification(&self) -> Classification {
        match self {
            Self::Local { classification, .. }
            | Self::Cloud { classification, .. }
            | Self::Denied { classification, .. } => *classification,
        }
    }
}

/// Inputs the router needs to make a decision.
///
/// `query` carries the full request text — the classifier and entity scanner
/// must see it. The router itself never emits the raw text; downstream
/// audit / trace emission must hash or truncate it.
#[derive(Debug, Clone)]
pub struct RouteRequest {
    /// Full query text. Inspected by the classifier and entity scanner.
    pub query: String,
    /// Caller-estimated token count (prompt + max completion).
    pub estimated_tokens: u32,
    /// Profile identifier of the requester.
    pub profile_id: String,
    /// Role of the requester (admin, adult, child, ...).
    pub role: String,
    /// Caller-supplied sensitivity hints (e.g. upstream classifier
    /// already saw "PHI"). Combined with the router's own scan.
    pub upstream_flags: Vec<String>,
}

/// Errors that can prevent a router decision.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum RouterError {
    /// Estimated token count was zero — caller error.
    #[error("estimated_tokens must be > 0")]
    InvalidTokenEstimate,
}

/// Router configuration top-level shape, loaded from TOML.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RouterConfig {
    /// Default cloud provider for offload.
    #[serde(default = "default_provider")]
    pub default_cloud_provider: CloudProvider,
    /// Default cloud model identifier.
    #[serde(default = "default_cloud_model")]
    pub default_cloud_model: String,
    /// Sensitivity level at or above which the router refuses cloud routing.
    /// `Regulated` is the conservative default.
    #[serde(default = "default_cloud_ceiling")]
    pub cloud_classification_ceiling: Classification,
    /// Sensitivity level at which the router denies the request entirely
    /// (regardless of cloud or local). `Critical` is the default.
    #[serde(default = "default_deny_floor")]
    pub deny_at: Classification,
}

fn default_provider() -> CloudProvider {
    CloudProvider::Anthropic
}

fn default_cloud_model() -> String {
    "claude-sonnet-4-6".to_string()
}

fn default_cloud_ceiling() -> Classification {
    Classification::Regulated
}

fn default_deny_floor() -> Classification {
    Classification::Critical
}

impl Default for RouterConfig {
    fn default() -> Self {
        Self {
            default_cloud_provider: default_provider(),
            default_cloud_model: default_cloud_model(),
            cloud_classification_ceiling: default_cloud_ceiling(),
            deny_at: default_deny_floor(),
        }
    }
}

/// Public router contract. All decisions are synchronous; the implementation
/// must keep route() under the latency budget documented in `docs/`.
pub trait Router: Send + Sync {
    /// Decide where the request should go.
    fn route(&self, request: &RouteRequest) -> Result<RoutingDecision, RouterError>;
}

/// Production composition: classifier + entity scanner + budget tracker.
pub struct DefaultRouter {
    config: RouterConfig,
    classifier: Arc<dyn SensitivityClassifier>,
    entities: Arc<EntityScanner>,
    budget: Arc<BudgetTracker>,
}

impl DefaultRouter {
    /// Build a router from its constituent parts.
    pub fn new(
        config: RouterConfig,
        classifier: Arc<dyn SensitivityClassifier>,
        entities: Arc<EntityScanner>,
        budget: Arc<BudgetTracker>,
    ) -> Self {
        Self {
            config,
            classifier,
            entities,
            budget,
        }
    }

    /// Convenience: a router with all default components.
    pub fn with_defaults() -> Self {
        Self {
            config: RouterConfig::default(),
            classifier: Arc::new(crate::classifier::RuleBasedClassifier::baseline()),
            entities: Arc::new(EntityScanner::baseline()),
            budget: Arc::new(BudgetTracker::with_defaults()),
        }
    }
}

/// The classification floor implied by caller-supplied sensitivity hints (audit
/// G4). Hints only ever RAISE the floor; an unrecognized hint contributes
/// `Public` (no effect). Matched case-insensitively and by substring so common
/// spellings ("phi", "phi-hint", "itar-controlled") all land.
fn floor_from_upstream_flags(flags: &[String]) -> Classification {
    let mut floor = Classification::Public;
    for raw in flags {
        let f = raw.to_ascii_lowercase();
        let hint = if f.contains("itar") || f.contains("export") || f.contains("classified") {
            Classification::Critical
        } else if f.contains("phi") || f.contains("medical") || f.contains("regulated") {
            Classification::Regulated
        } else if f.contains("sensitive") || f.contains("pii") {
            Classification::Sensitive
        } else {
            Classification::Public
        };
        floor = floor.max(hint);
    }
    floor
}

impl Router for DefaultRouter {
    fn route(&self, request: &RouteRequest) -> Result<RoutingDecision, RouterError> {
        if request.estimated_tokens == 0 {
            return Err(RouterError::InvalidTokenEstimate);
        }

        // Honor caller-supplied sensitivity hints as a floor (audit G4): they only
        // ever RAISE the classification, never lower the router's own scan, so an
        // upstream "phi" hint the local scan missed still forces the request up the
        // ladder (and thus local / denied by the checks below).
        let classification = self
            .classifier
            .classify(&request.query)
            .max(floor_from_upstream_flags(&request.upstream_flags));
        let entity_hits: Vec<EntityMatch> = self.entities.scan(&request.query);
        let has_export_controlled = entity_hits
            .iter()
            .any(|m| m.kind == EntityKind::ExportControlled);
        let has_tribal = entity_hits.iter().any(|m| m.kind == EntityKind::Tribal);
        let has_medical = entity_hits.iter().any(|m| m.kind == EntityKind::Medical);

        // 1. Hard deny at or above the configured floor.
        if classification >= self.config.deny_at {
            return Ok(RoutingDecision::Denied {
                code: "ROUTER-DENY-CRITICAL".to_string(),
                reason: format!(
                    "classification {} at or above deny floor {}",
                    classification.as_str(),
                    self.config.deny_at.as_str()
                ),
                classification,
            });
        }

        // 2. Export-controlled, tribal, or medical/PHI data must stay local
        //    regardless of classification (Lamprey ITAR / OCAP / HIPAA baseline).
        //    The entity floor is independent of the classifier — a medical entity
        //    the classifier does not rate high (e.g. "hospital") must still stay
        //    local (audit G3). The policy runtime lets operators tighten further.
        if has_export_controlled {
            return Ok(RoutingDecision::Local {
                reason: "export-controlled entity detected (ITAR/EAR baseline)".to_string(),
                classification,
            });
        }
        if has_tribal {
            return Ok(RoutingDecision::Local {
                reason: "tribal data sovereignty (OCAP baseline)".to_string(),
                classification,
            });
        }
        if has_medical {
            return Ok(RoutingDecision::Local {
                reason: "medical/PHI entity detected (HIPAA baseline)".to_string(),
                classification,
            });
        }

        // 3. Above the cloud ceiling: must stay local.
        if classification >= self.config.cloud_classification_ceiling {
            return Ok(RoutingDecision::Local {
                reason: format!(
                    "classification {} at or above cloud ceiling {}",
                    classification.as_str(),
                    self.config.cloud_classification_ceiling.as_str()
                ),
                classification,
            });
        }

        // 4. Budget check. Failures or hard-cap force local routing.
        match self.budget.check(
            &request.profile_id,
            &request.role,
            u64::from(request.estimated_tokens),
        ) {
            Ok(BudgetCheck::HardCapExceeded { used, budget, .. }) => {
                debug!(profile = %request.profile_id, used, budget, "hard cap exceeded; forcing local");
                return Ok(RoutingDecision::Local {
                    reason: "cloud budget hard cap reached; forced local".to_string(),
                    classification,
                });
            }
            Ok(BudgetCheck::SoftCapReached { remaining, .. }) => {
                debug!(
                    profile = %request.profile_id,
                    remaining,
                    "soft cap reached; flagged but routing"
                );
            }
            Ok(BudgetCheck::Ok { .. }) => {}
            Err(_) => {
                return Ok(RoutingDecision::Local {
                    reason: "budget check error; forced local".to_string(),
                    classification,
                });
            }
        }

        // 5. Default: cloud route.
        Ok(RoutingDecision::Cloud {
            provider: self.config.default_cloud_provider,
            model: self.config.default_cloud_model.clone(),
            reason: format!(
                "classification {} below cloud ceiling",
                classification.as_str()
            ),
            classification,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::classifier::RuleBasedClassifier;

    fn router() -> DefaultRouter {
        DefaultRouter::with_defaults()
    }

    fn req(query: &str) -> RouteRequest {
        RouteRequest {
            query: query.to_string(),
            estimated_tokens: 100,
            profile_id: "alice".to_string(),
            role: "adult".to_string(),
            upstream_flags: vec![],
        }
    }

    #[test]
    fn test_public_query_routes_cloud() {
        let r = router();
        match r.route(&req("What is the capital of France?")).unwrap() {
            RoutingDecision::Cloud { .. } => {}
            other => panic!("expected Cloud, got {other:?}"),
        }
    }

    #[test]
    fn test_upstream_phi_hint_raises_floor() {
        // Audit G4: a caller's "phi" hint raises the classification floor, so a
        // query the local scan rates Public is still forced off the default cloud
        // route (to Local, or Denied if the floor is at/above the deny threshold).
        let r = router();
        let request = RouteRequest {
            query: "what is the capital of France?".to_string(), // benign -> Public locally
            estimated_tokens: 10,
            profile_id: "p".to_string(),
            role: "adult".to_string(),
            upstream_flags: vec!["phi-hint".to_string()],
        };
        match r.route(&request).unwrap() {
            RoutingDecision::Local { classification, .. } => {
                assert!(classification >= Classification::Regulated);
            }
            RoutingDecision::Denied { .. } => {}
            other => panic!("expected Local/Denied from the phi-hint floor, got {other:?}"),
        }
    }

    #[test]
    fn test_phi_query_routes_local() {
        let r = router();
        match r.route(&req("Patient has a new prescription")).unwrap() {
            RoutingDecision::Local { classification, .. } => {
                assert!(classification >= Classification::Regulated);
            }
            other => panic!("expected Local, got {other:?}"),
        }
    }

    #[test]
    fn test_critical_query_denied() {
        let r = router();
        match r.route(&req("TOP SECRET project alpha")).unwrap() {
            RoutingDecision::Denied { code, .. } => assert!(code.starts_with("ROUTER-DENY")),
            other => panic!("expected Denied, got {other:?}"),
        }
    }

    #[test]
    fn test_export_controlled_forces_local_regardless_of_classification() {
        let r = router();
        // No regex hit, but an entity-dictionary hit on "itar".
        match r.route(&req("ITAR question about widgets")).unwrap() {
            RoutingDecision::Denied { .. } => {
                // The regex baseline marks "itar controlled" as critical;
                // a bare "ITAR" by itself triggers the ExportControlled
                // entity. Either outcome is policy-correct (deny or local).
            }
            RoutingDecision::Local { reason, .. } => assert!(reason.contains("export-controlled")),
            other => panic!("unexpected decision {other:?}"),
        }
    }

    #[test]
    fn test_medical_entity_forces_local_below_ceiling() {
        // Audit G3: a medical/PHI entity forces local even when the classifier does
        // not rate the text high, mirroring export-controlled / tribal. "hospital"
        // is a medical entity but not a regulated-classifier pattern, so without the
        // entity floor this query would route to cloud.
        let r = router();
        match r.route(&req("is there a hospital nearby")).unwrap() {
            RoutingDecision::Local { reason, .. } => {
                assert!(
                    reason.contains("medical") || reason.contains("PHI"),
                    "expected a medical-entity reason, got: {reason}"
                );
            }
            other => panic!("expected Local for a medical entity, got {other:?}"),
        }
    }

    #[test]
    fn test_tribal_data_forces_local() {
        let r = router();
        match r.route(&req("sacred site mapping for the treaty")).unwrap() {
            RoutingDecision::Local { reason, .. } => assert!(reason.contains("tribal")),
            other => panic!("expected Local, got {other:?}"),
        }
    }

    #[test]
    fn test_hard_cap_forces_local() {
        let budget = Arc::new(BudgetTracker::with_defaults());
        budget.record("alice", 99_999);
        let r = DefaultRouter::new(
            RouterConfig::default(),
            Arc::new(RuleBasedClassifier::baseline()),
            Arc::new(EntityScanner::baseline()),
            budget,
        );
        let request = RouteRequest {
            estimated_tokens: 100,
            ..req("Just a benign question")
        };
        match r.route(&request).unwrap() {
            RoutingDecision::Local { reason, .. } => assert!(reason.contains("budget")),
            other => panic!("expected Local from hard cap, got {other:?}"),
        }
    }

    #[test]
    fn test_zero_tokens_errors() {
        let r = router();
        let mut request = req("hello");
        request.estimated_tokens = 0;
        assert_eq!(r.route(&request), Err(RouterError::InvalidTokenEstimate));
    }

    #[test]
    fn test_decision_carries_audit_reason() {
        let r = router();
        let d = r.route(&req("What time is it?")).unwrap();
        assert!(!d.reason().is_empty());
    }
}
