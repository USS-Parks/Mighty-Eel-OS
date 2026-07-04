//! Rule-based sensitivity classifier.
//!
//! Inspects query text against a configurable set of regex patterns and
//! returns the highest matched classification level. Patterns are loaded
//! from TOML so deployments can extend the pattern set without code changes.
//!
//! The default pattern set ships with examples for PII, PHI, classified
//! terms, and tribal identifiers — operators are expected to tune them.

use std::collections::BTreeMap;

use regex::Regex;
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Sensitivity classification levels, ordered low-to-high.
///
/// `Ord` is derived so callers can take the maximum across multiple inputs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Classification {
    /// No sensitive content detected.
    Public,
    /// Internal-only content (organizational, not regulatory).
    Internal,
    /// Personally identifiable but not regulated (names, emails).
    Sensitive,
    /// Regulatory categories (PHI, financial, export-controlled).
    Regulated,
    /// National security or life-safety classification.
    Critical,
}

impl Classification {
    /// Wire-format string for audit log emission.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Public => "public",
            Self::Internal => "internal",
            Self::Sensitive => "sensitive",
            Self::Regulated => "regulated",
            Self::Critical => "critical",
        }
    }
}

/// Errors from classifier construction.
#[derive(Debug, Error)]
pub enum ClassifierError {
    /// Regex failed to compile.
    #[error("invalid pattern for {level:?}: {source}")]
    InvalidPattern {
        /// Classification level the bad pattern was for.
        level: Classification,
        /// Underlying regex error.
        source: regex::Error,
    },
}

/// Per-level pattern set loaded from TOML.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ClassifierConfig {
    /// Patterns whose match elevates classification to `Internal`.
    #[serde(default)]
    pub internal: Vec<String>,
    /// Patterns whose match elevates classification to `Sensitive`.
    #[serde(default)]
    pub sensitive: Vec<String>,
    /// Patterns whose match elevates classification to `Regulated`.
    #[serde(default)]
    pub regulated: Vec<String>,
    /// Patterns whose match elevates classification to `Critical`.
    #[serde(default)]
    pub critical: Vec<String>,
}

impl ClassifierConfig {
    /// Ship-with-product baseline patterns. Operators are expected to
    /// extend or replace these via TOML config.
    pub fn baseline() -> Self {
        Self {
            internal: vec![
                r"(?i)\binternal[\s-]use\b".to_string(),
                r"(?i)\bconfidential\b".to_string(),
            ],
            sensitive: vec![
                // Email
                r"\b[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Za-z]{2,}\b".to_string(),
                // US phone
                r"\b\d{3}-\d{3}-\d{4}\b".to_string(),
            ],
            regulated: vec![
                // US SSN
                r"\b\d{3}-\d{2}-\d{4}\b".to_string(),
                // Common PHI markers
                r"(?i)\b(patient|diagnosis|prescription|medical[\s-]record)\b".to_string(),
                // ICD-10 code shape (rough)
                r"\b[A-TV-Z][0-9][0-9AB]\.?[0-9A-TV-Z]{0,4}\b".to_string(),
            ],
            critical: vec![
                r"(?i)\b(top[\s-]secret|classified|noforn)\b".to_string(),
                r"(?i)\bitar[\s-]controlled\b".to_string(),
            ],
        }
    }
}

/// Trait for classifiers. Lets callers swap in an ML variant in air-gapped
/// builds without rewriting the router pipeline.
pub trait SensitivityClassifier: Send + Sync {
    /// Inspect `text` and return the highest classification level it triggers.
    fn classify(&self, text: &str) -> Classification;
}

/// Default rule-based classifier built from `ClassifierConfig`.
pub struct RuleBasedClassifier {
    internal: Vec<Regex>,
    sensitive: Vec<Regex>,
    regulated: Vec<Regex>,
    critical: Vec<Regex>,
}

impl RuleBasedClassifier {
    /// Compile a classifier from a config.
    pub fn new(config: &ClassifierConfig) -> Result<Self, ClassifierError> {
        Ok(Self {
            internal: compile(&config.internal, Classification::Internal)?,
            sensitive: compile(&config.sensitive, Classification::Sensitive)?,
            regulated: compile(&config.regulated, Classification::Regulated)?,
            critical: compile(&config.critical, Classification::Critical)?,
        })
    }

    /// Default classifier with the baseline pattern set.
    pub fn baseline() -> Self {
        Self::new(&ClassifierConfig::baseline()).expect("baseline patterns must compile")
    }

    /// Diagnostic: which level matched for which pattern.
    /// Useful for explaining a routing decision in audit logs.
    pub fn explain(&self, text: &str) -> BTreeMap<Classification, Vec<String>> {
        let mut hits: BTreeMap<Classification, Vec<String>> = BTreeMap::new();
        for (level, set) in [
            (Classification::Internal, &self.internal),
            (Classification::Sensitive, &self.sensitive),
            (Classification::Regulated, &self.regulated),
            (Classification::Critical, &self.critical),
        ] {
            for re in set {
                if re.is_match(text) {
                    hits.entry(level).or_default().push(re.as_str().to_string());
                }
            }
        }
        hits
    }
}

impl SensitivityClassifier for RuleBasedClassifier {
    fn classify(&self, text: &str) -> Classification {
        let mut level = Classification::Public;
        // Check highest-severity sets first so we can return early.
        for re in &self.critical {
            if re.is_match(text) {
                return Classification::Critical;
            }
        }
        for re in &self.regulated {
            if re.is_match(text) {
                level = Classification::Regulated;
            }
        }
        if level >= Classification::Regulated {
            return level;
        }
        for re in &self.sensitive {
            if re.is_match(text) {
                level = Classification::Sensitive;
            }
        }
        if level >= Classification::Sensitive {
            return level;
        }
        for re in &self.internal {
            if re.is_match(text) {
                level = Classification::Internal;
            }
        }
        level
    }
}

fn compile(patterns: &[String], level: Classification) -> Result<Vec<Regex>, ClassifierError> {
    let mut out = Vec::with_capacity(patterns.len());
    for pat in patterns {
        let re =
            Regex::new(pat).map_err(|source| ClassifierError::InvalidPattern { level, source })?;
        out.push(re);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_public_text_returns_public() {
        let c = RuleBasedClassifier::baseline();
        assert_eq!(
            c.classify("What is the capital of France?"),
            Classification::Public
        );
    }

    #[test]
    fn test_email_triggers_sensitive() {
        let c = RuleBasedClassifier::baseline();
        assert_eq!(
            c.classify("contact alice@example.com for details"),
            Classification::Sensitive,
        );
    }

    #[test]
    fn test_ssn_triggers_regulated() {
        let c = RuleBasedClassifier::baseline();
        assert_eq!(
            c.classify("the SSN is 123-45-6789"),
            Classification::Regulated,
        );
    }

    #[test]
    fn test_phi_marker_triggers_regulated() {
        let c = RuleBasedClassifier::baseline();
        assert_eq!(
            c.classify("the patient was given a prescription"),
            Classification::Regulated,
        );
    }

    #[test]
    fn test_classified_marker_short_circuits_to_critical() {
        let c = RuleBasedClassifier::baseline();
        assert_eq!(
            c.classify("this document is TOP SECRET / NOFORN"),
            Classification::Critical,
        );
    }

    #[test]
    fn test_classification_ordering() {
        assert!(Classification::Critical > Classification::Regulated);
        assert!(Classification::Regulated > Classification::Sensitive);
        assert!(Classification::Sensitive > Classification::Internal);
        assert!(Classification::Internal > Classification::Public);
    }

    #[test]
    fn test_invalid_pattern_errors() {
        let bad = ClassifierConfig {
            sensitive: vec!["(unclosed".to_string()],
            ..ClassifierConfig::default()
        };
        let result = RuleBasedClassifier::new(&bad);
        assert!(matches!(
            result,
            Err(ClassifierError::InvalidPattern { .. })
        ));
    }

    #[test]
    fn test_explain_returns_matched_pattern_strings() {
        let c = RuleBasedClassifier::baseline();
        let hits = c.explain("contact alice@example.com");
        assert!(hits.contains_key(&Classification::Sensitive));
    }

    #[test]
    fn test_empty_config_classifies_everything_public() {
        let c = RuleBasedClassifier::new(&ClassifierConfig::default()).unwrap();
        assert_eq!(
            c.classify("contact alice@example.com"),
            Classification::Public
        );
    }
}
