//! Pre-processing pipeline.
//!
//! Stitches the router primitives — classifier, entity scanner
//! budget tracker — together with the rule engine into a single
//! ordered evaluation:
//!
//! 1. Classify text
//! 2. Scan for entities
//! 3. Evaluate policy rules
//! 4. Budget check
//!   5. Resolve final decision
//!
//! Any stage can short-circuit. Per-stage microsecond timings are emitted
//! so operators can see where decision latency is spent.

use std::sync::Arc;
use std::time::Instant;

use serde::Serialize;
use thiserror::Error;

use crate::classifier::{Classification, SensitivityClassifier};
use crate::cost::{BudgetCheck, BudgetTracker};
use crate::entities::{EntityKind, EntityMatch, EntityScanner};
use crate::router::{RouteRequest, RouterConfig, RoutingDecision};
use crate::rules::{
    Action, AuditLevel, FactSet, PolicyModuleRegistry, RerouteTarget, RuleError, RuleHit, evaluate,
    resolve,
};

/// Per-stage timings in microseconds — useful for operator dashboards.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct StageMetrics {
    /// Classifier stage.
    pub classify_us: u64,
    /// Entity scan stage.
    pub entities_us: u64,
    /// Rule evaluation stage.
    pub policy_us: u64,
    /// Budget check stage.
    pub budget_us: u64,
    /// Total decision time.
    pub total_us: u64,
}

/// Compact audit record for a rule that fired.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PipelineRuleHit {
    /// Rule name.
    pub name: String,
    /// Audit level configured on the rule.
    pub audit_level: AuditLevel,
    /// Action the rule emitted.
    pub action: Action,
}

impl From<RuleHit> for PipelineRuleHit {
    fn from(hit: RuleHit) -> Self {
        Self {
            name: hit.name,
            audit_level: hit.audit_level,
            action: hit.action,
        }
    }
}

/// Result of a single pipeline pass.
#[derive(Debug, Clone, Serialize)]
pub struct PipelineResult {
    /// Final routing decision.
    pub decision: RoutingDecision,
    /// Classification observed for the query.
    pub classification: Classification,
    /// Entity kinds detected (one entry per unique kind).
    pub entity_kinds: Vec<EntityKind>,
    /// Every rule that fired, in evaluation order.
    pub rule_hits: Vec<PipelineRuleHit>,
    /// Per-stage timings.
    pub metrics: StageMetrics,
}

/// Pipeline errors.
#[derive(Debug, Error)]
pub enum PipelineError {
    /// Caller-provided request was invalid.
    #[error("invalid request: estimated_tokens must be > 0")]
    InvalidRequest,
    /// Rule evaluation failed.
    #[error(transparent)]
    RuleError(#[from] RuleError),
}

/// The pre-processing pipeline.
///
/// Build once at startup, reuse across requests. Holds Arc references so
/// the rule registry can be hot-reloaded without rebuilding the pipeline.
pub struct Pipeline {
    classifier: Arc<dyn SensitivityClassifier>,
    entities: Arc<EntityScanner>,
    modules: Arc<PolicyModuleRegistry>,
    budget: Arc<BudgetTracker>,
    config: RouterConfig,
}

impl Pipeline {
    /// Build a pipeline.
    pub fn new(
        classifier: Arc<dyn SensitivityClassifier>,
        entities: Arc<EntityScanner>,
        modules: Arc<PolicyModuleRegistry>,
        budget: Arc<BudgetTracker>,
        config: RouterConfig,
    ) -> Self {
        Self {
            classifier,
            entities,
            modules,
            budget,
            config,
        }
    }

    /// Evaluate one request through every stage.
    pub fn evaluate(&self, request: &RouteRequest) -> Result<PipelineResult, PipelineError> {
        if request.estimated_tokens == 0 {
            return Err(PipelineError::InvalidRequest);
        }
        let total_start = Instant::now();

        // 1. Classify
        let s = Instant::now();
        let classification = self.classifier.classify(&request.query);
        let classify_us = micros(s);

        // 2. Entities
        let s = Instant::now();
        let matches: Vec<EntityMatch> = self.entities.scan(&request.query);
        let mut entity_kinds: Vec<EntityKind> = matches.iter().map(|m| m.kind).collect();
        entity_kinds.sort_by_key(|k| *k as u8);
        entity_kinds.dedup();
        let entities_us = micros(s);

        // 3. Rule evaluation
        let s = Instant::now();
        let facts = FactSet::from_request(request, classification, &entity_kinds);
        let rules = self.modules.enabled_rules();
        let hits = evaluate(&rules, &facts)?;
        let policy_us = micros(s);

        // Resolve highest-priority rule hit (if any).
        let winning = resolve(&hits).cloned();
        let rule_hits: Vec<PipelineRuleHit> = hits.into_iter().map(PipelineRuleHit::from).collect();

        // If a rule wins, apply it before the default router precedence so
        // policy can override defaults (deny critical, force local, ...).
        if let Some(hit) = winning.as_ref()
            && let Some(decision) = action_to_decision(&hit.action, classification, &self.config)
        {
            let metrics = StageMetrics {
                classify_us,
                entities_us,
                policy_us,
                budget_us: 0,
                total_us: micros(total_start),
            };
            return Ok(PipelineResult {
                decision,
                classification,
                entity_kinds,
                rule_hits,
                metrics,
            });
        }

        // 4. Default precedence (mirrors DefaultRouter::route):
        //    deny floor → entity-forced local → cloud ceiling → budget.
        if classification >= self.config.deny_at {
            return Ok(PipelineResult {
                decision: RoutingDecision::Denied {
                    code: "ROUTER-DENY-CRITICAL".to_string(),
                    reason: format!(
                        "classification {} at or above deny floor {}",
                        classification.as_str(),
                        self.config.deny_at.as_str()
                    ),
                    classification,
                },
                classification,
                entity_kinds,
                rule_hits,
                metrics: StageMetrics {
                    classify_us,
                    entities_us,
                    policy_us,
                    budget_us: 0,
                    total_us: micros(total_start),
                },
            });
        }

        if entity_kinds.contains(&EntityKind::ExportControlled) {
            return Ok(PipelineResult {
                decision: RoutingDecision::Local {
                    reason: "export-controlled entity detected (ITAR/EAR baseline)".to_string(),
                    classification,
                },
                classification,
                entity_kinds,
                rule_hits,
                metrics: StageMetrics {
                    classify_us,
                    entities_us,
                    policy_us,
                    budget_us: 0,
                    total_us: micros(total_start),
                },
            });
        }
        if entity_kinds.contains(&EntityKind::Tribal) {
            return Ok(PipelineResult {
                decision: RoutingDecision::Local {
                    reason: "tribal data sovereignty (OCAP baseline)".to_string(),
                    classification,
                },
                classification,
                entity_kinds,
                rule_hits,
                metrics: StageMetrics {
                    classify_us,
                    entities_us,
                    policy_us,
                    budget_us: 0,
                    total_us: micros(total_start),
                },
            });
        }

        if classification >= self.config.cloud_classification_ceiling {
            return Ok(PipelineResult {
                decision: RoutingDecision::Local {
                    reason: format!(
                        "classification {} at or above cloud ceiling {}",
                        classification.as_str(),
                        self.config.cloud_classification_ceiling.as_str()
                    ),
                    classification,
                },
                classification,
                entity_kinds,
                rule_hits,
                metrics: StageMetrics {
                    classify_us,
                    entities_us,
                    policy_us,
                    budget_us: 0,
                    total_us: micros(total_start),
                },
            });
        }

        // 5. Budget
        let s = Instant::now();
        let budget_decision = self.budget.check(
            &request.profile_id,
            &request.role,
            u64::from(request.estimated_tokens),
        );
        let budget_us = micros(s);
        let mut force_local_for_budget = false;
        match budget_decision {
            Ok(BudgetCheck::HardCapExceeded { .. }) | Err(_) => {
                force_local_for_budget = true;
            }
            _ => {}
        }
        let decision = if force_local_for_budget {
            RoutingDecision::Local {
                reason: "cloud budget hard cap reached; forced local".to_string(),
                classification,
            }
        } else {
            RoutingDecision::Cloud {
                provider: self.config.default_cloud_provider,
                model: self.config.default_cloud_model.clone(),
                reason: format!(
                    "classification {} below cloud ceiling",
                    classification.as_str()
                ),
                classification,
            }
        };

        Ok(PipelineResult {
            decision,
            classification,
            entity_kinds,
            rule_hits,
            metrics: StageMetrics {
                classify_us,
                entities_us,
                policy_us,
                budget_us,
                total_us: micros(total_start),
            },
        })
    }
}

/// Convert a rule `Action` into a `RoutingDecision`. Returns `None` for
/// `Allow` and `Flag` actions — those let the default router precedence
/// take over.
fn action_to_decision(
    action: &Action,
    classification: Classification,
    config: &RouterConfig,
) -> Option<RoutingDecision> {
    match action {
        Action::Allow | Action::Flag { .. } => None,
        Action::Deny { reason, code } => Some(RoutingDecision::Denied {
            code: code.clone(),
            reason: reason.clone(),
            classification,
        }),
        Action::Reroute {
            to: RerouteTarget::Local,
            reason,
        } => Some(RoutingDecision::Local {
            reason: reason.clone(),
            classification,
        }),
        Action::Reroute {
            to: RerouteTarget::Cloud,
            reason,
        } => Some(RoutingDecision::Cloud {
            provider: config.default_cloud_provider,
            model: config.default_cloud_model.clone(),
            reason: reason.clone(),
            classification,
        }),
    }
}

#[allow(clippy::cast_possible_truncation)]
fn micros(start: Instant) -> u64 {
    start.elapsed().as_micros() as u64
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::classifier::RuleBasedClassifier;
    use crate::rules::engine::{Condition, Operator, Value};
    use crate::rules::{PolicyModule, Rule};

    fn pipeline_with(modules: PolicyModuleRegistry) -> Pipeline {
        Pipeline::new(
            Arc::new(RuleBasedClassifier::baseline()),
            Arc::new(EntityScanner::baseline()),
            Arc::new(modules),
            Arc::new(BudgetTracker::with_defaults()),
            RouterConfig::default(),
        )
    }

    fn req(query: &str) -> RouteRequest {
        RouteRequest {
            query: query.to_string(),
            estimated_tokens: 200,
            profile_id: "alice".to_string(),
            role: "adult".to_string(),
            upstream_flags: vec![],
        }
    }

    fn deny_phi_rule() -> Rule {
        Rule {
            name: "hipaa_phi_deny_cloud".into(),
            priority: 1_000,
            condition: Condition::All {
                all: vec![
                    Condition::Match {
                        field: "has_entity.medical".into(),
                        op: Operator::Equals,
                        value: Value::Bool(true),
                    },
                    Condition::Match {
                        field: "role".into(),
                        op: Operator::NotEquals,
                        value: Value::Str("admin".into()),
                    },
                ],
            },
            action: Action::Reroute {
                to: RerouteTarget::Local,
                reason: "HIPAA: PHI must stay local".into(),
            },
            audit_level: AuditLevel::Warn,
        }
    }

    #[test]
    fn test_public_query_flows_to_default_cloud() {
        let reg = PolicyModuleRegistry::new();
        let p = pipeline_with(reg);
        let r = p.evaluate(&req("What is the capital of France?")).unwrap();
        assert!(matches!(r.decision, RoutingDecision::Cloud { .. }));
        assert!(r.metrics.total_us < 100_000); // < 100ms is a sanity ceiling.
    }

    #[test]
    fn test_rule_reroute_overrides_default() {
        let reg = PolicyModuleRegistry::new();
        reg.install(PolicyModule {
            name: "hipaa".into(),
            enabled: true,
            rules: vec![deny_phi_rule()],
        });
        let p = pipeline_with(reg);
        let r = p
            .evaluate(&req("The patient was given a prescription"))
            .unwrap();
        match r.decision {
            RoutingDecision::Local { reason, .. } => assert!(reason.contains("HIPAA")),
            other => panic!("expected Local from HIPAA reroute, got {other:?}"),
        }
        assert!(!r.rule_hits.is_empty());
        assert_eq!(r.rule_hits[0].name, "hipaa_phi_deny_cloud");
    }

    #[test]
    fn test_higher_priority_rule_wins() {
        let reg = PolicyModuleRegistry::new();
        reg.install(PolicyModule {
            name: "hipaa".into(),
            enabled: true,
            rules: vec![
                Rule {
                    name: "low_priority_allow".into(),
                    priority: 10,
                    condition: Condition::Match {
                        field: "has_entity.medical".into(),
                        op: Operator::Equals,
                        value: Value::Bool(true),
                    },
                    action: Action::Allow,
                    audit_level: AuditLevel::Info,
                },
                Rule {
                    name: "high_priority_deny".into(),
                    priority: 100,
                    condition: Condition::Match {
                        field: "has_entity.medical".into(),
                        op: Operator::Equals,
                        value: Value::Bool(true),
                    },
                    action: Action::Deny {
                        reason: "phi".into(),
                        code: "HIPAA-DENY".into(),
                    },
                    audit_level: AuditLevel::Warn,
                },
            ],
        });
        let p = pipeline_with(reg);
        let r = p.evaluate(&req("patient diagnosis")).unwrap();
        match r.decision {
            RoutingDecision::Denied { code, .. } => assert_eq!(code, "HIPAA-DENY"),
            other => panic!("expected Denied, got {other:?}"),
        }
    }

    #[test]
    fn test_disabled_module_does_not_contribute() {
        let reg = PolicyModuleRegistry::new();
        reg.install(PolicyModule {
            name: "hipaa".into(),
            enabled: false,
            rules: vec![deny_phi_rule()],
        });
        let p = pipeline_with(reg);
        // Without the HIPAA module, PHI still goes Local — but for the
        // built-in cloud-ceiling reason, not the rule reason.
        let r = p
            .evaluate(&req("The patient was given a prescription"))
            .unwrap();
        match r.decision {
            RoutingDecision::Local { reason, .. } => {
                assert!(!reason.contains("HIPAA"));
                assert!(
                    reason.contains("cloud ceiling")
                        || reason.contains("OCAP")
                        || reason.contains("export")
                );
            }
            other => panic!("expected Local, got {other:?}"),
        }
    }

    #[test]
    fn test_stage_metrics_populated() {
        let reg = PolicyModuleRegistry::new();
        let p = pipeline_with(reg);
        let r = p.evaluate(&req("benign query")).unwrap();
        // Metrics are best-effort; just assert they were populated.
        assert!(r.metrics.total_us >= r.metrics.classify_us);
        assert!(r.metrics.total_us >= r.metrics.entities_us);
    }

    #[test]
    fn test_critical_classification_still_denied_even_with_allow_rule() {
        // A super-low-priority Allow rule must not bypass the Critical
        // deny floor.
        let reg = PolicyModuleRegistry::new();
        reg.install(PolicyModule {
            name: "permissive".into(),
            enabled: true,
            rules: vec![Rule {
                name: "always_allow".into(),
                priority: 1,
                condition: Condition::Match {
                    field: "role".into(),
                    op: Operator::Equals,
                    value: Value::Str("adult".into()),
                },
                action: Action::Allow,
                audit_level: AuditLevel::None,
            }],
        });
        let p = pipeline_with(reg);
        let r = p.evaluate(&req("TOP SECRET project alpha")).unwrap();
        // Allow returns None from action_to_decision, so default precedence
        // takes over and Critical hits the deny floor.
        assert!(matches!(r.decision, RoutingDecision::Denied { .. }));
    }
}
