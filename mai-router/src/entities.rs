//! Compliance entity detection.
//!
//! Dictionary-based recognition of entities that influence routing decisions:
//! medical terminology (PHI / HIPAA), tribal identifiers (OCAP), and
//! export-controlled technical terms (ITAR/EAR).
//!
//! This is intentionally not a probabilistic NLP model — air-gapped
//! deployments cannot ship a multi-megabyte model file with the binary.
//! The dictionary is loadable from TOML so each tenant can extend the
//! shipped baseline for their domain.

use serde::{Deserialize, Serialize};

/// Category of compliance entity detected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntityKind {
    /// HIPAA-relevant medical term (PHI marker).
    Medical,
    /// Tribal data sovereignty identifier (OCAP-relevant).
    Tribal,
    /// ITAR/EAR export-controlled technical term.
    ExportControlled,
}

impl EntityKind {
    /// Wire-format string for audit emission.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Medical => "medical",
            Self::Tribal => "tribal",
            Self::ExportControlled => "export_controlled",
        }
    }
}

/// One detected entity match.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct EntityMatch {
    /// Category.
    pub kind: EntityKind,
    /// Byte span (start, end_exclusive) within the query text.
    pub span: (usize, usize),
    /// Blake3 hash of the matched substring — never the raw text, so audit
    /// logs cannot regenerate the query.
    pub matched_hash: String,
    /// Confidence in [0.0, 1.0]. Dictionary hits are 1.0; future ML-driven
    /// matches would lower this.
    pub confidence: f64,
}

/// Dictionary of compliance entities, loadable from TOML.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct EntityDictionary {
    /// Medical / PHI vocabulary.
    #[serde(default)]
    pub medical: Vec<String>,
    /// Tribal / OCAP vocabulary.
    #[serde(default)]
    pub tribal: Vec<String>,
    /// Export-controlled / ITAR vocabulary.
    #[serde(default)]
    pub export_controlled: Vec<String>,
}

impl EntityDictionary {
    /// Ship-with-product baseline vocabulary. Operators are expected to
    /// extend or replace these via TOML config.
    pub fn baseline() -> Self {
        Self {
            medical: vec![
                "diagnosis".into(),
                "prescription".into(),
                "medical record".into(),
                "patient".into(),
                "hospital".into(),
                "phi".into(),
            ],
            tribal: vec![
                "treaty".into(),
                "tribal".into(),
                "sacred site".into(),
                "ocap".into(),
                "indigenous data".into(),
            ],
            export_controlled: vec![
                "itar".into(),
                "ear99".into(),
                "controlled technical data".into(),
                "munitions list".into(),
                "missile technology".into(),
            ],
        }
    }
}

/// Scanner over a fixed dictionary. Construction normalizes terms to
/// lowercase once so per-request scanning is a simple case-insensitive
/// substring search.
pub struct EntityScanner {
    medical: Vec<String>,
    tribal: Vec<String>,
    export_controlled: Vec<String>,
}

impl EntityScanner {
    /// Build a scanner from a dictionary.
    pub fn new(dict: &EntityDictionary) -> Self {
        Self {
            medical: lower(&dict.medical),
            tribal: lower(&dict.tribal),
            export_controlled: lower(&dict.export_controlled),
        }
    }

    /// Default scanner using the baseline dictionary.
    pub fn baseline() -> Self {
        Self::new(&EntityDictionary::baseline())
    }

    /// Scan `text` and return every dictionary hit. Matches preserve order
    /// of first occurrence within the text.
    pub fn scan(&self, text: &str) -> Vec<EntityMatch> {
        let haystack = text.to_lowercase();
        let mut matches = Vec::new();
        find_all(
            &haystack,
            &self.medical,
            EntityKind::Medical,
            text,
            &mut matches,
        );
        find_all(
            &haystack,
            &self.tribal,
            EntityKind::Tribal,
            text,
            &mut matches,
        );
        find_all(
            &haystack,
            &self.export_controlled,
            EntityKind::ExportControlled,
            text,
            &mut matches,
        );
        matches.sort_by_key(|m| m.span.0);
        matches
    }
}

fn lower(terms: &[String]) -> Vec<String> {
    terms.iter().map(|t| t.to_lowercase()).collect()
}

fn find_all(
    haystack: &str,
    needles: &[String],
    kind: EntityKind,
    original: &str,
    out: &mut Vec<EntityMatch>,
) {
    for needle in needles {
        if needle.is_empty() {
            continue;
        }
        let mut start = 0;
        while let Some(pos) = haystack[start..].find(needle.as_str()) {
            let absolute = start + pos;
            let end = absolute + needle.len();
            let matched_slice = original.get(absolute..end).unwrap_or("");
            out.push(EntityMatch {
                kind,
                span: (absolute, end),
                matched_hash: hash_match(matched_slice),
                confidence: 1.0,
            });
            start = end;
        }
    }
}

fn hash_match(text: &str) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(text.as_bytes());
    hasher.finalize().to_hex().to_string()[..32].to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_no_matches_in_unrelated_text() {
        let s = EntityScanner::baseline();
        assert!(s.scan("The weather is nice today.").is_empty());
    }

    #[test]
    fn test_medical_term_detected() {
        let s = EntityScanner::baseline();
        let matches = s.scan("The patient received a prescription for...");
        assert!(matches.iter().any(|m| m.kind == EntityKind::Medical));
    }

    #[test]
    fn test_tribal_term_detected() {
        let s = EntityScanner::baseline();
        let matches = s.scan("Per the treaty, sacred site access is restricted.");
        assert!(matches.iter().any(|m| m.kind == EntityKind::Tribal));
    }

    #[test]
    fn test_export_controlled_term_detected() {
        let s = EntityScanner::baseline();
        let matches = s.scan("Item is ITAR controlled technical data.");
        assert!(
            matches
                .iter()
                .any(|m| m.kind == EntityKind::ExportControlled)
        );
    }

    #[test]
    fn test_matched_text_is_hashed_never_raw() {
        let s = EntityScanner::baseline();
        let matches = s.scan("patient");
        assert_eq!(matches.len(), 1);
        // 32 hex chars from blake3 truncated.
        assert_eq!(matches[0].matched_hash.len(), 32);
        assert_ne!(matches[0].matched_hash, "patient");
    }

    #[test]
    fn test_matches_sorted_by_span_start() {
        let s = EntityScanner::baseline();
        let text = "The patient referenced a treaty about ITAR.";
        let matches = s.scan(text);
        for pair in matches.windows(2) {
            assert!(pair[0].span.0 <= pair[1].span.0);
        }
    }

    #[test]
    fn test_case_insensitive_matching() {
        let s = EntityScanner::baseline();
        assert!(!s.scan("PATIENT records").is_empty());
        assert!(!s.scan("patient records").is_empty());
    }

    #[test]
    fn test_empty_needle_skipped() {
        let dict = EntityDictionary {
            medical: vec![String::new(), "patient".into()],
            ..EntityDictionary::default()
        };
        let s = EntityScanner::new(&dict);
        let m = s.scan("the patient is stable");
        assert_eq!(m.len(), 1);
    }
}
