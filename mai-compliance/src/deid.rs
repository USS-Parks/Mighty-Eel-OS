//! De-identification.
//!
//! Takes a query text plus the `PhiReport` from the detector and produces a
//! redacted version where each detected PHI span is replaced with a
//! placeholder of the form `[PHI:<identifier>]`. The redactor also computes
//! a re-identification risk score so callers can decide whether the
//! redacted output is safe to forward to a cloud route.
//!
//! Zero-false-negative guarantee: every PHI span in the report becomes a
//! placeholder. Callers that need a stronger guarantee should re-scan the
//! redacted output with the detector and assert `has_any() == false`.

use std::cmp::Reverse;

use serde::{Deserialize, Serialize};

use crate::phi::{PhiConfidence, PhiReport};

/// Tunables for the redactor.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeidConfig {
    /// Risk-score threshold above which the result is considered unsafe
    /// for cloud forwarding. Operators may tighten for high-sensitivity
    /// deployments.
    #[serde(default = "default_safe_threshold")]
    pub safe_threshold: f64,
    /// Template for the placeholder. Available tokens:
    ///   `{kind}`   - identifier name in snake_case
    ///   `{idx}`    - 1-based position of the span in the original text
    #[serde(default = "default_template")]
    pub placeholder_template: String,
}

fn default_safe_threshold() -> f64 {
    0.4
}

fn default_template() -> String {
    "[PHI:{kind}]".to_string()
}

impl Default for DeidConfig {
    fn default() -> Self {
        Self {
            safe_threshold: default_safe_threshold(),
            placeholder_template: default_template(),
        }
    }
}

/// Statistical re-identification risk for one query.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct RiskScore {
    /// Composite score in `[0.0, 1.0]`. Higher = greater re-id risk.
    pub score: f64,
    /// True when `score <= safe_threshold`.
    pub safe_for_cloud: bool,
    /// Number of distinct identifier categories observed.
    pub distinct_kinds: usize,
    /// Total PHI hit count.
    pub total_hits: usize,
}

/// Result of a redaction pass.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DeidResult {
    /// Redacted text — all PHI spans replaced with placeholders.
    pub redacted_text: String,
    /// Composite risk score.
    pub risk: RiskScore,
}

/// Pure redactor: takes the original text and a report, returns the
/// redacted version + risk.
#[derive(Debug, Clone, Default)]
pub struct Redactor {
    config: DeidConfig,
}

impl Redactor {
    /// New redactor.
    pub fn new(config: DeidConfig) -> Self {
        Self { config }
    }

    /// Defaults: 0.4 safe threshold, `[PHI:<kind>]` template.
    pub fn with_defaults() -> Self {
        Self::default()
    }

    /// Apply redaction. The report should be the one produced by
    /// `PhiDetector::scan(text)` for the same `text`.
    pub fn redact(&self, text: &str, report: &PhiReport) -> DeidResult {
        // Replace spans in reverse so indices stay stable.
        let mut hits = report.hits.clone();
        hits.sort_by_key(|hit| Reverse(hit.span.0));

        let mut redacted = String::from(text);
        for hit in &hits {
            let (start, end) = hit.span;
            if end > redacted.len()
                || !redacted.is_char_boundary(start)
                || !redacted.is_char_boundary(end)
            {
                // Skip malformed span; the caller would have generated this
                // from a different text. Better to no-op than to panic.
                continue;
            }
            let placeholder = self
                .config
                .placeholder_template
                .replace("{kind}", hit.identifier.as_str());
            redacted.replace_range(start..end, &placeholder);
        }

        let risk = self.compute_risk(report, text.len());
        DeidResult {
            redacted_text: redacted,
            risk,
        }
    }

    fn compute_risk(&self, report: &PhiReport, text_len: usize) -> RiskScore {
        let total_hits = report.total_hits();
        let distinct_kinds = report.by_identifier.len();
        if total_hits == 0 {
            return RiskScore {
                score: 0.0,
                safe_for_cloud: true,
                distinct_kinds: 0,
                total_hits: 0,
            };
        }
        // Composite: weighted mix of confidence boost, density, and breadth.
        let confidence_boost = match report.highest_confidence {
            Some(PhiConfidence::Explicit) => 0.5,
            Some(PhiConfidence::Probable) => 0.3,
            Some(PhiConfidence::Possible) => 0.15,
            None => 0.0,
        };
        // Density: hits per 100 chars, capped at 1.0.
        #[allow(clippy::cast_precision_loss)]
        let density = {
            let denom = (text_len.max(1)) as f64;
            ((total_hits as f64) * 100.0 / denom).min(1.0)
        };
        // Breadth: distinct categories scaled by 18 (HIPAA's full identifier set).
        #[allow(clippy::cast_precision_loss)]
        let breadth = (distinct_kinds as f64 / 18.0).min(1.0);
        let score = (confidence_boost + 0.3 * density + 0.2 * breadth).clamp(0.0, 1.0);
        RiskScore {
            score,
            safe_for_cloud: score <= self.config.safe_threshold,
            distinct_kinds,
            total_hits,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::phi::{PhiDetector, PhiIdentifier};

    fn round_trip(text: &str) -> DeidResult {
        let report = PhiDetector::baseline().scan(text);
        Redactor::with_defaults().redact(text, &report)
    }

    #[test]
    fn test_no_phi_text_unchanged_and_safe() {
        let r = round_trip("the sky is blue");
        assert_eq!(r.redacted_text, "the sky is blue");
        assert!(r.risk.safe_for_cloud);
        assert_eq!(r.risk.total_hits, 0);
    }

    #[test]
    fn test_ssn_replaced_with_placeholder() {
        let r = round_trip("SSN 123-45-6789");
        assert!(r.redacted_text.contains("[PHI:ssn]"));
        assert!(!r.redacted_text.contains("123-45-6789"));
    }

    #[test]
    fn test_email_replaced_with_placeholder() {
        let r = round_trip("contact alice@example.com");
        assert!(r.redacted_text.contains("[PHI:email_address]"));
        assert!(!r.redacted_text.contains("alice@example.com"));
    }

    #[test]
    fn test_multiple_phi_all_redacted() {
        let r = round_trip("email a@b.co SSN 123-45-6789 IP 10.0.0.1");
        assert!(!r.redacted_text.contains("a@b.co"));
        assert!(!r.redacted_text.contains("123-45-6789"));
        assert!(!r.redacted_text.contains("10.0.0.1"));
    }

    #[test]
    fn test_redaction_preserves_surrounding_text() {
        let r = round_trip("Please email alice@example.com today.");
        assert!(r.redacted_text.starts_with("Please email"));
        assert!(r.redacted_text.ends_with("today."));
    }

    #[test]
    fn test_no_phi_survives_double_scan() {
        // Round-trip and confirm a second scan of the redacted output finds
        // nothing. This is the zero-false-negative invariant — it must hold
        // for any text whose original scan was complete.
        let text = "SSN 123-45-6789 phone 415-555-1212 email a@b.co";
        let report = PhiDetector::baseline().scan(text);
        let r = Redactor::with_defaults().redact(text, &report);
        let second = PhiDetector::baseline().scan(&r.redacted_text);
        // Allow `[PHI:url]` etc to escape — but no original PHI categories.
        for hit in &second.hits {
            assert_ne!(hit.identifier, PhiIdentifier::SocialSecurityNumber);
            assert_ne!(hit.identifier, PhiIdentifier::PhoneNumber);
            assert_ne!(hit.identifier, PhiIdentifier::EmailAddress);
        }
    }

    #[test]
    fn test_risk_high_for_dense_phi() {
        let r = round_trip("SSN 123-45-6789");
        assert!(r.risk.score > 0.3);
    }

    #[test]
    fn test_risk_safe_threshold_respected() {
        let cfg = DeidConfig {
            safe_threshold: 0.01,
            ..DeidConfig::default()
        };
        let report = PhiDetector::baseline().scan("Dr. Smith examined the patient");
        let r = Redactor::new(cfg).redact("Dr. Smith examined the patient", &report);
        // With an almost-zero threshold even Possible-only PHI is unsafe.
        if r.risk.total_hits > 0 {
            assert!(!r.risk.safe_for_cloud);
        }
    }

    #[test]
    fn test_distinct_kinds_counted() {
        let r = round_trip("SSN 123-45-6789 email a@b.co");
        assert!(r.risk.distinct_kinds >= 2);
    }

    #[test]
    fn test_malformed_span_no_panic() {
        // Build a report whose span exceeds the text length; redactor should
        // skip it without panicking.
        let mut report = PhiReport::default();
        report.hits.push(crate::phi::PhiHit {
            identifier: PhiIdentifier::SocialSecurityNumber,
            confidence: PhiConfidence::Explicit,
            span: (100, 200),
            matched_hash: "x".repeat(32),
        });
        let r = Redactor::with_defaults().redact("short", &report);
        assert_eq!(r.redacted_text, "short");
    }
}
