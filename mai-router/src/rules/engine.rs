//! Rule engine.
//!
//! Programmable compliance rules layered on top of the router
//! primitives. A `Rule` carries a name, integer priority, a `Condition`
//! (boolean tree over fields), an `Action`, and an `AuditLevel`. Rules
//! evaluate against a `FactSet` derived from the request + the classifier
//! and entity scanner outputs.
//!
//! All applicable rules fire; the highest-priority action wins. Ties
//! prefer the more restrictive action (Deny > Reroute > Flag > Allow).

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::classifier::Classification;
use crate::entities::EntityKind;
use crate::router::RouteRequest;

/// How a rule hit should be surfaced to the audit log.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuditLevel {
    /// Suppress from the audit log.
    None,
    /// Informational note.
    Info,
    /// Warn-level event.
    Warn,
    /// Critical event — operator should be paged.
    Critical,
}

/// Where a `Reroute` action sends the request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RerouteTarget {
    /// Force local MAI inference.
    Local,
    /// Force cloud routing (operator override).
    Cloud,
}

/// What a rule does when its condition matches.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Action {
    /// Explicitly allow the request through.
    Allow,
    /// Reject the request with a stable code.
    Deny {
        /// Human-readable reason for audit.
        reason: String,
        /// Stable code for programmatic checks.
        code: String,
    },
    /// Force the request to a specific target.
    Reroute {
        /// Where to send the request.
        to: RerouteTarget,
        /// Human-readable reason for audit.
        reason: String,
    },
    /// Allow but flag for audit attention.
    Flag {
        /// Reason recorded in the audit log.
        reason: String,
    },
}

impl Action {
    /// Restrictiveness rank — higher == more restrictive. Used when two
    /// rules tie on priority.
    pub fn restrictiveness(&self) -> u8 {
        match self {
            Self::Deny { .. } => 4,
            Self::Reroute {
                to: RerouteTarget::Local,
                ..
            } => 3,
            Self::Reroute {
                to: RerouteTarget::Cloud,
                ..
            } => 2,
            Self::Flag { .. } => 1,
            Self::Allow => 0,
        }
    }
}

/// Comparison operator for a field matcher.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Operator {
    Equals,
    NotEquals,
    Contains,
    GreaterEqual,
    LessEqual,
    In,
}

/// Right-hand-side value for a field matcher.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Value {
    /// String literal.
    Str(String),
    /// Integer literal.
    Int(i64),
    /// Boolean literal.
    Bool(bool),
    /// List of strings (use with `In`).
    List(Vec<String>),
}

/// Boolean condition tree.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Condition {
    /// Compare a named field on the `FactSet` to a literal.
    Match {
        /// Field name (see `FactSet` for the supported set).
        field: String,
        /// Comparison operator.
        op: Operator,
        /// Right-hand value.
        value: Value,
    },
    /// AND combination.
    All {
        /// All sub-conditions must match.
        all: Vec<Condition>,
    },
    /// OR combination.
    Any {
        /// At least one sub-condition must match.
        any: Vec<Condition>,
    },
    /// NOT combination.
    Not {
        /// Inverts the inner condition.
        not: Box<Condition>,
    },
}

/// One compliance rule.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rule {
    /// Human-readable rule identifier — included in audit hits.
    pub name: String,
    /// Higher numeric value wins ties at the resolution step.
    #[serde(default)]
    pub priority: i32,
    /// Boolean condition tree evaluated against the `FactSet`.
    pub condition: Condition,
    /// Action to take when the condition matches.
    pub action: Action,
    /// Audit emission level when this rule fires.
    #[serde(default = "default_audit_level")]
    pub audit_level: AuditLevel,
}

fn default_audit_level() -> AuditLevel {
    AuditLevel::Info
}

/// Per-evaluation facts the rule engine inspects. Built once per request by
/// the pipeline, then queried by every `Match` condition. Fields exposed:
///
/// - `classification` (Str): `public` / `internal` / `sensitive` / `regulated` / `critical`
/// - `role` (Str)
/// - `profile_id` (Str)
/// - `estimated_tokens` (Int)
/// - `has_entity.medical` / `.tribal` / `.export_controlled` (Bool)
/// - `upstream_flags` (List<Str>)
#[derive(Debug, Clone)]
pub struct FactSet {
    fields: HashMap<String, FactValue>,
}

#[derive(Debug, Clone)]
enum FactValue {
    Str(String),
    Int(i64),
    Bool(bool),
    List(Vec<String>),
}

impl FactSet {
    /// Build a fact set from a request + classifier and entity outputs.
    pub fn from_request(
        request: &RouteRequest,
        classification: Classification,
        entity_kinds: &[EntityKind],
    ) -> Self {
        let mut fields: HashMap<String, FactValue> = HashMap::new();
        fields.insert(
            "classification".into(),
            FactValue::Str(classification.as_str().to_string()),
        );
        fields.insert("role".into(), FactValue::Str(request.role.clone()));
        fields.insert(
            "profile_id".into(),
            FactValue::Str(request.profile_id.clone()),
        );
        fields.insert(
            "estimated_tokens".into(),
            FactValue::Int(i64::from(request.estimated_tokens)),
        );
        fields.insert(
            "has_entity.medical".into(),
            FactValue::Bool(entity_kinds.contains(&EntityKind::Medical)),
        );
        fields.insert(
            "has_entity.tribal".into(),
            FactValue::Bool(entity_kinds.contains(&EntityKind::Tribal)),
        );
        fields.insert(
            "has_entity.export_controlled".into(),
            FactValue::Bool(entity_kinds.contains(&EntityKind::ExportControlled)),
        );
        fields.insert(
            "upstream_flags".into(),
            FactValue::List(request.upstream_flags.clone()),
        );
        Self { fields }
    }
}

/// Errors raised while evaluating rules.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum RuleError {
    /// Field referenced by a condition does not exist on the fact set.
    #[error("unknown fact field '{0}'")]
    UnknownField(String),
    /// Operator/value type mismatch (e.g. GreaterEqual on a string).
    #[error("type mismatch for field '{field}' with op {op:?}")]
    TypeMismatch {
        /// The field that mismatched.
        field: String,
        /// The operator the rule used.
        op: Operator,
    },
}

/// One rule that fired during evaluation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RuleHit {
    /// Name of the rule.
    pub name: String,
    /// Priority at evaluation time.
    pub priority: i32,
    /// Action attached to the rule.
    pub action: Action,
    /// Audit emission level.
    pub audit_level: AuditLevel,
}

/// Evaluate a slice of rules against the facts and return every rule that
/// fired, sorted by priority desc then restrictiveness desc.
pub fn evaluate(rules: &[Rule], facts: &FactSet) -> Result<Vec<RuleHit>, RuleError> {
    let mut hits = Vec::new();
    for rule in rules {
        if eval_condition(&rule.condition, facts)? {
            hits.push(RuleHit {
                name: rule.name.clone(),
                priority: rule.priority,
                action: rule.action.clone(),
                audit_level: rule.audit_level,
            });
        }
    }
    hits.sort_by(|a, b| {
        b.priority
            .cmp(&a.priority)
            .then_with(|| b.action.restrictiveness().cmp(&a.action.restrictiveness()))
    });
    Ok(hits)
}

/// Return the winning action: the highest-priority hit, with restrictiveness
/// breaking ties.
pub fn resolve(hits: &[RuleHit]) -> Option<&RuleHit> {
    hits.first()
}

fn eval_condition(cond: &Condition, facts: &FactSet) -> Result<bool, RuleError> {
    match cond {
        Condition::Match { field, op, value } => eval_match(field, op, value, facts),
        Condition::All { all } => {
            for c in all {
                if !eval_condition(c, facts)? {
                    return Ok(false);
                }
            }
            Ok(true)
        }
        Condition::Any { any } => {
            for c in any {
                if eval_condition(c, facts)? {
                    return Ok(true);
                }
            }
            Ok(false)
        }
        Condition::Not { not } => Ok(!eval_condition(not, facts)?),
    }
}

fn eval_match(
    field: &str,
    op: &Operator,
    value: &Value,
    facts: &FactSet,
) -> Result<bool, RuleError> {
    let fact = facts
        .fields
        .get(field)
        .ok_or_else(|| RuleError::UnknownField(field.to_string()))?;
    let mismatch = || RuleError::TypeMismatch {
        field: field.to_string(),
        op: op.clone(),
    };
    match (fact, op, value) {
        (FactValue::Str(s), Operator::Equals, Value::Str(v)) => Ok(s == v),
        (FactValue::Str(s), Operator::NotEquals, Value::Str(v)) => Ok(s != v),
        (FactValue::Str(s), Operator::Contains, Value::Str(v)) => Ok(s.contains(v.as_str())),
        (FactValue::Str(s), Operator::In, Value::List(vs)) => Ok(vs.iter().any(|x| x == s)),
        (FactValue::Int(i), Operator::Equals, Value::Int(v)) => Ok(i == v),
        (FactValue::Int(i), Operator::NotEquals, Value::Int(v)) => Ok(i != v),
        (FactValue::Int(i), Operator::GreaterEqual, Value::Int(v)) => Ok(i >= v),
        (FactValue::Int(i), Operator::LessEqual, Value::Int(v)) => Ok(i <= v),
        (FactValue::Bool(b), Operator::Equals, Value::Bool(v)) => Ok(b == v),
        (FactValue::List(xs), Operator::Contains, Value::Str(v)) => Ok(xs.iter().any(|s| s == v)),
        _ => Err(mismatch()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req() -> RouteRequest {
        RouteRequest {
            query: "test".to_string(),
            estimated_tokens: 100,
            profile_id: "alice".to_string(),
            role: "adult".to_string(),
            upstream_flags: vec!["phi-hint".to_string()],
        }
    }

    fn facts() -> FactSet {
        FactSet::from_request(&req(), Classification::Regulated, &[EntityKind::Medical])
    }

    fn match_field(field: &str, op: Operator, value: Value) -> Condition {
        Condition::Match {
            field: field.to_string(),
            op,
            value,
        }
    }

    #[test]
    fn test_match_string_equals() {
        let f = facts();
        let cond = match_field(
            "classification",
            Operator::Equals,
            Value::Str("regulated".to_string()),
        );
        assert!(eval_condition(&cond, &f).unwrap());
    }

    #[test]
    fn test_match_bool_true_field() {
        let f = facts();
        let cond = match_field("has_entity.medical", Operator::Equals, Value::Bool(true));
        assert!(eval_condition(&cond, &f).unwrap());
    }

    #[test]
    fn test_match_int_ge() {
        let f = facts();
        let cond = match_field("estimated_tokens", Operator::GreaterEqual, Value::Int(50));
        assert!(eval_condition(&cond, &f).unwrap());
    }

    #[test]
    fn test_unknown_field_errors() {
        let f = facts();
        let cond = match_field("nope.nope", Operator::Equals, Value::Str("x".into()));
        assert!(matches!(
            eval_condition(&cond, &f),
            Err(RuleError::UnknownField(_)),
        ));
    }

    #[test]
    fn test_type_mismatch_errors() {
        let f = facts();
        let cond = match_field("classification", Operator::GreaterEqual, Value::Int(1));
        assert!(matches!(
            eval_condition(&cond, &f),
            Err(RuleError::TypeMismatch { .. }),
        ));
    }

    #[test]
    fn test_combinators_and_or_not() {
        let f = facts();
        let phi = match_field("has_entity.medical", Operator::Equals, Value::Bool(true));
        let admin = match_field("role", Operator::Equals, Value::Str("admin".to_string()));
        assert!(
            !eval_condition(
                &Condition::All {
                    all: vec![phi.clone(), admin.clone()],
                },
                &f,
            )
            .unwrap()
        );
        assert!(
            eval_condition(
                &Condition::Any {
                    any: vec![phi.clone(), admin.clone()],
                },
                &f,
            )
            .unwrap()
        );
        assert!(!eval_condition(&Condition::Not { not: Box::new(phi) }, &f).unwrap());
    }

    #[test]
    fn test_evaluate_returns_priority_sorted_hits() {
        let f = facts();
        let rules = vec![
            Rule {
                name: "low".into(),
                priority: 10,
                condition: match_field("has_entity.medical", Operator::Equals, Value::Bool(true)),
                action: Action::Flag {
                    reason: "phi".into(),
                },
                audit_level: AuditLevel::Info,
            },
            Rule {
                name: "high".into(),
                priority: 100,
                condition: match_field(
                    "classification",
                    Operator::Equals,
                    Value::Str("regulated".into()),
                ),
                action: Action::Deny {
                    reason: "phi-cloud".into(),
                    code: "HIPAA-PHI-DENY".into(),
                },
                audit_level: AuditLevel::Warn,
            },
        ];
        let hits = evaluate(&rules, &f).unwrap();
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].name, "high");
        let winner = resolve(&hits).unwrap();
        assert_eq!(winner.name, "high");
    }

    #[test]
    fn test_evaluate_tie_breaks_by_restrictiveness() {
        let f = facts();
        let rules = vec![
            Rule {
                name: "lenient".into(),
                priority: 50,
                condition: match_field(
                    "classification",
                    Operator::Equals,
                    Value::Str("regulated".into()),
                ),
                action: Action::Flag {
                    reason: "flag".into(),
                },
                audit_level: AuditLevel::Info,
            },
            Rule {
                name: "strict".into(),
                priority: 50,
                condition: match_field(
                    "classification",
                    Operator::Equals,
                    Value::Str("regulated".into()),
                ),
                action: Action::Deny {
                    reason: "deny".into(),
                    code: "TEST".into(),
                },
                audit_level: AuditLevel::Warn,
            },
        ];
        let hits = evaluate(&rules, &f).unwrap();
        assert_eq!(resolve(&hits).unwrap().name, "strict");
    }

    #[test]
    fn test_in_operator_against_list_value() {
        let f = facts();
        let cond = match_field(
            "role",
            Operator::In,
            Value::List(vec!["admin".into(), "adult".into()]),
        );
        assert!(eval_condition(&cond, &f).unwrap());
    }

    #[test]
    fn test_list_field_contains() {
        let f = facts();
        let cond = match_field(
            "upstream_flags",
            Operator::Contains,
            Value::Str("phi-hint".into()),
        );
        assert!(eval_condition(&cond, &f).unwrap());
    }
}
