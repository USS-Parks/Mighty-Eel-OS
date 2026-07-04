//! Business Associate Agreement (BAA) enforcement.
//!
//! Given a `PhiReport` from the detector, the enforcer evaluates it
//! against the deployment's BAA posture and returns a `BaaDecision` with
//! enough detail for the audit log. Three baseline modes ship:
//!
//! - `Standard`: PHI must stay local. De-identified data may route to
//!   cloud (the de-id verification step is separate, in `deid`).
//! - `Strict`: any PHI hit at all forces local, regardless of confidence.
//! - `Custom`: the operator declares the maximum confidence level allowed
//!   on cloud and the set of identifier categories that must never leave.
//!
//! The enforcer is intentionally pure: it inspects the report, never the
//! original text. That keeps PHI out of audit log call stacks.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::phi::{PhiConfidence, PhiIdentifier, PhiReport};

/// BAA posture.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum BaaMode {
    /// PHI must stay local; de-identified data may route.
    Standard,
    /// Any PHI hit forces local routing.
    Strict,
    /// Operator-defined posture.
    Custom(CustomBaa),
}

/// Operator-defined BAA. PHI is allowed on cloud only when:
///
/// - the highest confidence is at or below `max_cloud_confidence`, AND
/// - none of the matched identifiers appear in `never_leave_local`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CustomBaa {
    /// Maximum PHI confidence permitted on cloud routes.
    #[serde(default = "default_max_confidence")]
    pub max_cloud_confidence: PhiConfidence,
    /// Identifier categories that must never leave the local boundary,
    /// regardless of confidence.
    #[serde(default)]
    pub never_leave_local: BTreeSet<PhiIdentifier>,
}

fn default_max_confidence() -> PhiConfidence {
    PhiConfidence::Possible
}

impl Default for CustomBaa {
    fn default() -> Self {
        Self {
            max_cloud_confidence: default_max_confidence(),
            never_leave_local: BTreeSet::new(),
        }
    }
}

/// Top-level BAA configuration loaded from TOML.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BaaConfig {
    /// Active posture.
    pub mode: BaaMode,
}

impl Default for BaaConfig {
    fn default() -> Self {
        Self {
            mode: BaaMode::Standard,
        }
    }
}

/// One BAA violation discovered while evaluating a report.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct BaaViolation {
    /// The identifier that violated the agreement.
    pub identifier: PhiIdentifier,
    /// Confidence of the offending hit.
    pub confidence: PhiConfidence,
    /// Human-readable reason for audit.
    pub reason: String,
}

/// Enforcer decision.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct BaaDecision {
    /// True when the request is allowed to proceed (to its current target).
    pub allowed: bool,
    /// Human-readable summary; always populated.
    pub reason: String,
    /// Every detected violation. Empty when `allowed` is true.
    pub violations: Vec<BaaViolation>,
}

/// BAA errors (config-level).
#[derive(Debug, Error)]
pub enum BaaError {
    /// Custom BAA referenced an unknown identifier name.
    #[error("invalid identifier name in custom BAA: '{0}'")]
    InvalidIdentifier(String),
}

/// Pure evaluator over a PHI report.
#[derive(Debug, Clone, Default)]
pub struct BaaEnforcer {
    config: BaaConfig,
}

impl BaaEnforcer {
    /// Build an enforcer with the given config.
    pub fn new(config: BaaConfig) -> Self {
        Self { config }
    }

    /// Convenience: the Standard-mode enforcer.
    pub fn standard() -> Self {
        Self::new(BaaConfig {
            mode: BaaMode::Standard,
        })
    }

    /// Convenience: the Strict-mode enforcer.
    pub fn strict() -> Self {
        Self::new(BaaConfig {
            mode: BaaMode::Strict,
        })
    }

    /// Evaluate a report against the active BAA mode.
    pub fn evaluate_for_cloud(&self, report: &PhiReport) -> BaaDecision {
        match &self.config.mode {
            BaaMode::Standard => self.eval_standard(report),
            BaaMode::Strict => self.eval_strict(report),
            BaaMode::Custom(custom) => self.eval_custom(report, custom),
        }
    }

    fn eval_standard(&self, report: &PhiReport) -> BaaDecision {
        if report.has_any() {
            let violations = report
                .hits
                .iter()
                .map(|hit| BaaViolation {
                    identifier: hit.identifier,
                    confidence: hit.confidence,
                    reason: format!(
                        "Standard BAA: PHI {} ({}) must remain local",
                        hit.identifier.as_str(),
                        confidence_str(hit.confidence),
                    ),
                })
                .collect::<Vec<_>>();
            BaaDecision {
                allowed: false,
                reason: "Standard BAA: PHI detected; must stay local".to_string(),
                violations,
            }
        } else {
            BaaDecision {
                allowed: true,
                reason: "Standard BAA: no PHI detected".to_string(),
                violations: Vec::new(),
            }
        }
    }

    fn eval_strict(&self, report: &PhiReport) -> BaaDecision {
        if report.has_any() {
            let violations = report
                .hits
                .iter()
                .map(|hit| BaaViolation {
                    identifier: hit.identifier,
                    confidence: hit.confidence,
                    reason: format!(
                        "Strict BAA: any PHI hit forces local ({} {})",
                        hit.identifier.as_str(),
                        confidence_str(hit.confidence),
                    ),
                })
                .collect::<Vec<_>>();
            BaaDecision {
                allowed: false,
                reason: "Strict BAA: any PHI present forces local routing".to_string(),
                violations,
            }
        } else {
            BaaDecision {
                allowed: true,
                reason: "Strict BAA: no PHI detected".to_string(),
                violations: Vec::new(),
            }
        }
    }

    fn eval_custom(&self, report: &PhiReport, custom: &CustomBaa) -> BaaDecision {
        let mut violations = Vec::new();
        for hit in &report.hits {
            if custom.never_leave_local.contains(&hit.identifier) {
                violations.push(BaaViolation {
                    identifier: hit.identifier,
                    confidence: hit.confidence,
                    reason: format!(
                        "Custom BAA: identifier {} is in never_leave_local set",
                        hit.identifier.as_str()
                    ),
                });
                continue;
            }
            if hit.confidence > custom.max_cloud_confidence {
                violations.push(BaaViolation {
                    identifier: hit.identifier,
                    confidence: hit.confidence,
                    reason: format!(
                        "Custom BAA: {} confidence above max_cloud_confidence",
                        hit.identifier.as_str()
                    ),
                });
            }
        }
        if violations.is_empty() {
            BaaDecision {
                allowed: true,
                reason: "Custom BAA: all detections within allowed confidence and category set"
                    .to_string(),
                violations,
            }
        } else {
            BaaDecision {
                allowed: false,
                reason: format!("Custom BAA: {} violation(s) detected", violations.len()),
                violations,
            }
        }
    }
}

fn confidence_str(c: PhiConfidence) -> &'static str {
    match c {
        PhiConfidence::Explicit => "explicit",
        PhiConfidence::Probable => "probable",
        PhiConfidence::Possible => "possible",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::phi::PhiDetector;

    fn report_for(text: &str) -> PhiReport {
        PhiDetector::baseline().scan(text)
    }

    #[test]
    fn test_standard_allows_no_phi() {
        let d = BaaEnforcer::standard().evaluate_for_cloud(&report_for("the sky is blue"));
        assert!(d.allowed);
        assert!(d.violations.is_empty());
    }

    #[test]
    fn test_standard_blocks_any_phi() {
        let d = BaaEnforcer::standard().evaluate_for_cloud(&report_for("SSN 123-45-6789"));
        assert!(!d.allowed);
        assert!(!d.violations.is_empty());
    }

    #[test]
    fn test_strict_blocks_even_low_confidence() {
        let d =
            BaaEnforcer::strict().evaluate_for_cloud(&report_for("Dr. Smith examined the patient"));
        assert!(!d.allowed);
    }

    #[test]
    fn test_custom_max_confidence_allows_below_threshold() {
        // Allow up to Probable. A Possible-only report passes.
        let enforcer = BaaEnforcer::new(BaaConfig {
            mode: BaaMode::Custom(CustomBaa {
                max_cloud_confidence: PhiConfidence::Probable,
                never_leave_local: BTreeSet::new(),
            }),
        });
        let d = enforcer.evaluate_for_cloud(&report_for("Dr. Smith helped"));
        assert!(d.allowed);
    }

    #[test]
    fn test_custom_blocks_explicit_above_threshold() {
        // Allow up to Probable. SSN is Explicit → blocked.
        let enforcer = BaaEnforcer::new(BaaConfig {
            mode: BaaMode::Custom(CustomBaa {
                max_cloud_confidence: PhiConfidence::Probable,
                never_leave_local: BTreeSet::new(),
            }),
        });
        let d = enforcer.evaluate_for_cloud(&report_for("SSN 123-45-6789"));
        assert!(!d.allowed);
    }

    #[test]
    fn test_custom_never_leave_local_takes_precedence() {
        let mut never = BTreeSet::new();
        never.insert(PhiIdentifier::EmailAddress);
        let enforcer = BaaEnforcer::new(BaaConfig {
            mode: BaaMode::Custom(CustomBaa {
                max_cloud_confidence: PhiConfidence::Explicit,
                never_leave_local: never,
            }),
        });
        let d = enforcer.evaluate_for_cloud(&report_for("contact alice@example.com"));
        assert!(!d.allowed);
        assert!(
            d.violations
                .iter()
                .any(|v| v.identifier == PhiIdentifier::EmailAddress)
        );
    }

    #[test]
    fn test_decision_reason_always_populated() {
        let d = BaaEnforcer::standard().evaluate_for_cloud(&report_for("hi"));
        assert!(!d.reason.is_empty());
    }

    #[test]
    fn test_violations_carry_identifier_and_confidence() {
        let d = BaaEnforcer::strict().evaluate_for_cloud(&report_for("SSN 123-45-6789"));
        let v = d.violations.first().unwrap();
        assert_eq!(v.identifier, PhiIdentifier::SocialSecurityNumber);
        assert_eq!(v.confidence, PhiConfidence::Explicit);
    }
}
