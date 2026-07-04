//! Treaty-aware routing.
//!
//! Treaties are not policy strings — they are legal instruments
//! recognised in Canadian courts and increasingly in US tribal law.
//! When tribal data references a treaty, the routing engine must
//! apply that treaty's specific routing rules in addition to OCAP's
//! generic local-only default.
//!
//! This module:
//!
//! - Detects treaty references in source text (numbered Canadian
//!   treaties, year-prefixed treaties, named treaties).
//! - Looks up each detected treaty in a per-deployment registry of
//!   treaty routing obligations.
//! - Falls back to the most-restrictive rule when multiple treaties
//!   are referenced, matching the OCAP "respect first" default.
//!
//! The treaty registry itself is data, not code. Operators ship the
//! registry through `config/compliance/ocap.toml` (table
//! `[ocap.treaties]`); the in-crate baseline only knows how to
//! recognise treaty references, not how to route them. This separation
//! ensures the build does not enshrine any specific community's
//! interpretation of a treaty's data provisions.

use std::collections::{BTreeMap, BTreeSet};

use regex::Regex;
use serde::{Deserialize, Serialize};

/// Treaty identifier — a stable string assigned by the operator. The
/// in-crate baseline only emits identifiers for treaty *references it
/// recognised*; the routing rules for those identifiers come from the
/// registry, not from this code.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct TreatyId(String);

impl TreatyId {
    /// Build a treaty id from a non-empty string.
    pub fn new(id: impl Into<String>) -> Option<Self> {
        let raw = id.into();
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(Self(trimmed.to_string()))
        }
    }

    /// Borrowed view.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Per-treaty routing obligation. The fields are intentionally
/// minimal: this module's job is to surface the *fact* of a treaty
/// reference and the obligation level; the policy engine in
/// [`super::ocap_rules`] combines that with the OCAP defaults.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TreatyObligation {
    /// Treaty id (matches the detected hit).
    pub treaty: TreatyId,
    /// True when the treaty requires that data referencing it remain
    /// under the tribal nation's physical possession (the strictest
    /// interpretation, and the default).
    #[serde(default = "default_local_only")]
    pub requires_local_processing: bool,
    /// True when the treaty grants the tribal nation an explicit
    /// right of review before any external use. The policy engine
    /// treats this as `Quarantine` until consent is recorded.
    #[serde(default)]
    pub requires_consent_review: bool,
    /// Free-form note for the audit record. NOT a substitute for the
    /// treaty text; the operator is responsible for citing the actual
    /// provision in their compliance documentation.
    #[serde(default)]
    pub note: String,
}

fn default_local_only() -> bool {
    true
}

/// One detected treaty reference.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TreatyHit {
    /// Treaty identifier as emitted by the recogniser. The
    /// canonicalisation rule is documented in
    /// [`TreatyDetector::recognise`].
    pub treaty: TreatyId,
    /// Byte span `(start, end_exclusive)` in the source text.
    pub span: (usize, usize),
    /// Rule id (matches the in-crate recogniser identifier).
    pub rule_id: String,
}

/// Aggregate of every treaty reference in the source text plus the
/// effective obligation derived from the registry.
#[derive(Debug, Clone, Default, Serialize)]
pub struct TreatyReport {
    /// All hits in order of occurrence.
    pub hits: Vec<TreatyHit>,
    /// Distinct treaty ids referenced.
    pub treaties_referenced: BTreeSet<TreatyId>,
    /// True when at least one referenced treaty requires local
    /// processing. False only if every referenced treaty in the
    /// registry explicitly allows cloud routing AND there were no
    /// unknown (unregistered) treaty references.
    pub requires_local_processing: bool,
    /// True when at least one referenced treaty requires consent
    /// review before processing.
    pub requires_consent_review: bool,
    /// True when at least one referenced treaty was not in the
    /// registry. The policy engine treats unknown treaties as
    /// most-restrictive (local-only + consent-review).
    pub has_unknown_treaty: bool,
}

impl TreatyReport {
    /// True when any treaty was referenced.
    pub fn has_any(&self) -> bool {
        !self.hits.is_empty()
    }

    /// Total hits.
    pub fn total_hits(&self) -> usize {
        self.hits.len()
    }
}

/// Detector tunables.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TreatyDetectorConfig {
    /// Registry of known treaty obligations, keyed by [`TreatyId`].
    /// Operators populate this from `config/compliance/ocap.toml`.
    #[serde(default)]
    pub registry: BTreeMap<TreatyId, TreatyObligation>,
}

struct CompiledRule {
    rule_id: String,
    regex: Regex,
    /// Function that maps a match string to a [`TreatyId`]. The
    /// recogniser identifies the treaty; the registry then supplies
    /// the obligation.
    canonicalise: fn(&str) -> Option<TreatyId>,
}

/// Treaty reference detector + registry lookup.
pub struct TreatyDetector {
    config: TreatyDetectorConfig,
    rules: Vec<CompiledRule>,
}

impl TreatyDetector {
    /// Build a detector with the given config (registry).
    pub fn new(config: TreatyDetectorConfig) -> Self {
        Self {
            config,
            rules: baseline_rules(),
        }
    }

    /// Empty registry — every detected treaty will be reported as
    /// "unknown" and the report will flag local-only + consent-review.
    pub fn baseline() -> Self {
        Self::new(TreatyDetectorConfig::default())
    }

    /// Recognise treaty references in the given text and aggregate
    /// the obligations from the registry. The recogniser's canonical
    /// id rules are:
    ///
    /// - `Treaty N` (numbered) → id `"treaty_<N>"`
    /// - `YYYY Treaty` (year-prefixed) → id `"treaty_<YYYY>"`
    /// - Named treaty (e.g. `Jay Treaty`) → id matches the in-crate
    ///   recogniser id (operators register the same id).
    pub fn scan(&self, text: &str) -> TreatyReport {
        let mut hits: Vec<TreatyHit> = Vec::new();
        let mut referenced: BTreeSet<TreatyId> = BTreeSet::new();
        let mut requires_local = false;
        let mut requires_consent = false;
        let mut has_unknown = false;

        for rule in &self.rules {
            for found in rule.regex.find_iter(text) {
                if let Some(treaty) = (rule.canonicalise)(found.as_str()) {
                    hits.push(TreatyHit {
                        treaty: treaty.clone(),
                        span: (found.start(), found.end()),
                        rule_id: rule.rule_id.clone(),
                    });
                    referenced.insert(treaty);
                }
            }
        }
        hits.sort_by_key(|h| h.span.0);

        for id in &referenced {
            match self.config.registry.get(id) {
                Some(ob) => {
                    if ob.requires_local_processing {
                        requires_local = true;
                    }
                    if ob.requires_consent_review {
                        requires_consent = true;
                    }
                }
                None => {
                    // Unknown treaty → most-restrictive default.
                    has_unknown = true;
                    requires_local = true;
                    requires_consent = true;
                }
            }
        }

        TreatyReport {
            hits,
            treaties_referenced: referenced,
            requires_local_processing: requires_local,
            requires_consent_review: requires_consent,
            has_unknown_treaty: has_unknown,
        }
    }

    /// Recognise a single treaty reference. Returns the canonical
    /// [`TreatyId`] if the recogniser knows how to identify it.
    pub fn recognise(&self, text: &str) -> Option<TreatyId> {
        for rule in &self.rules {
            if let Some(m) = rule.regex.find(text)
                && let Some(id) = (rule.canonicalise)(m.as_str())
            {
                return Some(id);
            }
        }
        None
    }

    /// Number of registered treaty obligations.
    pub fn registered_count(&self) -> usize {
        self.config.registry.len()
    }
}

fn baseline_rules() -> Vec<CompiledRule> {
    vec![
        CompiledRule {
            rule_id: "treaty.numbered".to_string(),
            regex: Regex::new(r"(?i)\bTreaty\s+(?:No\.?\s*)?(1[0-1]|[1-9])\b")
                .expect("numbered-treaty regex"),
            canonicalise: canonicalise_numbered,
        },
        CompiledRule {
            rule_id: "treaty.year_prefixed".to_string(),
            regex: Regex::new(r"(?i)\b(1[78]\d{2})\s+Treaty\b").expect("year-treaty regex"),
            canonicalise: canonicalise_year_prefixed,
        },
        CompiledRule {
            rule_id: "treaty.jay".to_string(),
            regex: Regex::new(r"(?i)\bJay\s+Treaty\b").expect("jay-treaty regex"),
            canonicalise: |_| TreatyId::new("treaty_jay"),
        },
        CompiledRule {
            rule_id: "treaty.fort_laramie".to_string(),
            regex: Regex::new(r"(?i)\bFort\s+Laramie\s+Treaty\b").expect("fort-laramie regex"),
            canonicalise: |_| TreatyId::new("treaty_fort_laramie"),
        },
        CompiledRule {
            rule_id: "treaty.medicine_creek".to_string(),
            regex: Regex::new(r"(?i)\bMedicine\s+Creek\s+Treaty\b").expect("medicine-creek regex"),
            canonicalise: |_| TreatyId::new("treaty_medicine_creek"),
        },
    ]
}

fn canonicalise_numbered(matched: &str) -> Option<TreatyId> {
    // Take the trailing digits.
    let digits: String = matched.chars().filter(|c| c.is_ascii_digit()).collect();
    if digits.is_empty() {
        return None;
    }
    TreatyId::new(format!("treaty_{}", digits))
}

fn canonicalise_year_prefixed(matched: &str) -> Option<TreatyId> {
    let digits: String = matched
        .chars()
        .take_while(|c| c.is_ascii_digit() || c.is_whitespace())
        .filter(|c| c.is_ascii_digit())
        .collect();
    if digits.len() != 4 {
        return None;
    }
    TreatyId::new(format!("treaty_{}", digits))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn registry_with(entries: Vec<TreatyObligation>) -> TreatyDetectorConfig {
        let mut registry = BTreeMap::new();
        for ob in entries {
            registry.insert(ob.treaty.clone(), ob);
        }
        TreatyDetectorConfig { registry }
    }

    #[test]
    fn no_treaty_reference_returns_empty_report() {
        let det = TreatyDetector::baseline();
        let report = det.scan("Tell me about rainfall this week.");
        assert!(!report.has_any());
        assert!(!report.requires_local_processing);
        assert!(!report.has_unknown_treaty);
    }

    #[test]
    fn detects_numbered_treaty_and_flags_unknown() {
        let det = TreatyDetector::baseline();
        let report = det.scan("This data falls under Treaty 7.");
        assert!(report.has_any());
        assert!(report.has_unknown_treaty);
        assert!(report.requires_local_processing);
        assert!(report.requires_consent_review);
        assert!(
            report
                .treaties_referenced
                .contains(&TreatyId::new("treaty_7").unwrap())
        );
    }

    #[test]
    fn detects_year_prefixed_treaty() {
        let det = TreatyDetector::baseline();
        let report = det.scan("Provisions from the 1871 Treaty are binding.");
        assert!(report.has_any());
        assert!(
            report
                .treaties_referenced
                .contains(&TreatyId::new("treaty_1871").unwrap())
        );
    }

    #[test]
    fn detects_jay_treaty() {
        let det = TreatyDetector::baseline();
        let report = det.scan("Border crossings under the Jay Treaty are protected.");
        assert!(
            report
                .treaties_referenced
                .contains(&TreatyId::new("treaty_jay").unwrap())
        );
    }

    #[test]
    fn registered_treaty_uses_registry_rules() {
        let cfg = registry_with(vec![TreatyObligation {
            treaty: TreatyId::new("treaty_7").unwrap(),
            requires_local_processing: true,
            requires_consent_review: false,
            note: "test-only".to_string(),
        }]);
        let det = TreatyDetector::new(cfg);
        let report = det.scan("This data falls under Treaty 7.");
        assert!(report.requires_local_processing);
        assert!(!report.requires_consent_review);
        assert!(!report.has_unknown_treaty);
    }

    #[test]
    fn registered_treaty_can_allow_cloud_if_operator_chose() {
        // A deployment where the tribal authority has explicitly
        // approved cloud routing for a specific treaty: the report
        // surface should reflect that.
        let cfg = registry_with(vec![TreatyObligation {
            treaty: TreatyId::new("treaty_jay").unwrap(),
            requires_local_processing: false,
            requires_consent_review: false,
            note: "cloud-allowed by tribal authority approval".to_string(),
        }]);
        let det = TreatyDetector::new(cfg);
        let report = det.scan("Border crossings under the Jay Treaty are protected.");
        assert!(!report.requires_local_processing);
        assert!(!report.requires_consent_review);
        assert!(!report.has_unknown_treaty);
    }

    #[test]
    fn multiple_treaties_apply_most_restrictive() {
        // Treaty 7 is unknown → strict; Jay Treaty is registered as
        // cloud-allowed. The unknown one still drags the overall
        // outcome to strict.
        let cfg = registry_with(vec![TreatyObligation {
            treaty: TreatyId::new("treaty_jay").unwrap(),
            requires_local_processing: false,
            requires_consent_review: false,
            note: "".to_string(),
        }]);
        let det = TreatyDetector::new(cfg);
        let report = det.scan("Treaty 7 and the Jay Treaty both apply here.");
        assert!(report.requires_local_processing);
        assert!(report.requires_consent_review);
        assert!(report.has_unknown_treaty);
    }

    #[test]
    fn hits_are_ordered_by_span_start() {
        let det = TreatyDetector::baseline();
        let report = det.scan("Treaty 6, then the 1855 Treaty, then Treaty 11.");
        let starts: Vec<usize> = report.hits.iter().map(|h| h.span.0).collect();
        let mut sorted = starts.clone();
        sorted.sort_unstable();
        assert_eq!(starts, sorted);
        assert_eq!(report.hits.len(), 3);
    }

    #[test]
    fn recognise_single_reference() {
        let det = TreatyDetector::baseline();
        let id = det.recognise("Treaty 6").expect("recognised");
        assert_eq!(id.as_str(), "treaty_6");
    }

    #[test]
    fn treaty_id_rejects_empty_string() {
        assert!(TreatyId::new("").is_none());
        assert!(TreatyId::new("   ").is_none());
        assert!(TreatyId::new("treaty_x").is_some());
    }

    #[test]
    fn registered_count_reflects_registry_size() {
        let cfg = registry_with(vec![
            TreatyObligation {
                treaty: TreatyId::new("treaty_7").unwrap(),
                requires_local_processing: true,
                requires_consent_review: true,
                note: String::new(),
            },
            TreatyObligation {
                treaty: TreatyId::new("treaty_jay").unwrap(),
                requires_local_processing: false,
                requires_consent_review: false,
                note: String::new(),
            },
        ]);
        let det = TreatyDetector::new(cfg);
        assert_eq!(det.registered_count(), 2);
    }

    #[test]
    fn year_prefixed_only_matches_1700s_and_1800s() {
        let det = TreatyDetector::baseline();
        // 1900s should NOT match the year-prefixed rule.
        let report = det.scan("The 1985 Treaty is fictional.");
        assert!(!report.has_any());
    }
}
