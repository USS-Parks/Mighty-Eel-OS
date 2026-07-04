//! Medical entity enrichment.
//!
//! These detectors enrich routing and de-id decisions with clinical
//! domain knowledge:
//!
//! - **ICD-10**: format + range validation for diagnosis codes.
//! - **RxNorm**: small ship-with-product medication dictionary that
//!   operators can extend via TOML.
//! - **Lab values**: numeric value + unit parsing for common patterns.
//!
//! None of these claim to be exhaustive — a deployment will tune
//! dictionaries to its clinical specialty. The shipped baseline is meant
//! to exercise the wiring and to give an honest demo signal.

use std::collections::BTreeSet;

use regex::Regex;
use serde::{Deserialize, Serialize};

/// ICD-10 code validator.
///
/// Format: one letter (A-T or V-Z, no U), one digit, one alphanumeric
/// (0-9 or A/B), optional `.` and up to 4 more alphanumeric characters
/// (0-9 or A-T or V-Z).
#[derive(Debug)]
pub struct IcdValidator {
    shape: Regex,
}

impl Default for IcdValidator {
    fn default() -> Self {
        Self::new()
    }
}

impl IcdValidator {
    /// Build a validator.
    pub fn new() -> Self {
        let shape = Regex::new(r"^[A-TV-Z][0-9][0-9AB]\.?[0-9A-TV-Z]{0,4}$")
            .expect("ICD-10 regex must compile");
        Self { shape }
    }

    /// True when the input matches ICD-10 shape.
    pub fn is_valid(&self, code: &str) -> bool {
        let trimmed = code.trim();
        if trimmed.is_empty() {
            return false;
        }
        self.shape.is_match(trimmed)
    }

    /// Extract every plausible ICD-10 code from free text. Returns the
    /// matched substrings (trimmed) in order of occurrence.
    pub fn extract(&self, text: &str) -> Vec<String> {
        let scan = Regex::new(r"\b[A-TV-Z][0-9][0-9AB]\.?[0-9A-TV-Z]{0,4}\b")
            .expect("ICD-10 scan regex must compile");
        scan.find_iter(text)
            .map(|m| m.as_str().to_string())
            .collect()
    }
}

/// Medication dictionary derived from RxNorm-style names.
///
/// Ships with a small baseline; operators replace it with their formulary.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MedicationDictionary {
    /// Generic and brand medication names. Matched case-insensitively.
    #[serde(default)]
    pub medications: BTreeSet<String>,
}

impl MedicationDictionary {
    /// Ship-with-product baseline. Operators extend via TOML.
    pub fn baseline() -> Self {
        let medications = [
            "amoxicillin",
            "ibuprofen",
            "acetaminophen",
            "metformin",
            "lisinopril",
            "atorvastatin",
            "omeprazole",
            "albuterol",
            "warfarin",
            "insulin",
        ]
        .iter()
        .map(|s| (*s).to_string())
        .collect();
        Self { medications }
    }
}

/// One medication hit in scanned text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MedicationHit {
    /// Normalized (lowercase) medication name.
    pub name: String,
    /// Byte span in the original text.
    pub span: (usize, usize),
}

impl MedicationDictionary {
    /// Scan free text for medication mentions.
    pub fn scan(&self, text: &str) -> Vec<MedicationHit> {
        let lower = text.to_lowercase();
        let mut hits = Vec::new();
        for med in &self.medications {
            let needle = med.to_lowercase();
            if needle.is_empty() {
                continue;
            }
            let mut start = 0;
            while let Some(pos) = lower[start..].find(needle.as_str()) {
                let absolute = start + pos;
                let end = absolute + needle.len();
                hits.push(MedicationHit {
                    name: needle.clone(),
                    span: (absolute, end),
                });
                start = end;
            }
        }
        hits.sort_by_key(|h| h.span.0);
        hits
    }
}

/// A parsed lab value, e.g. `4.5 mg/dL`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct LabValue {
    /// Numeric value.
    pub value: f64,
    /// Unit string (e.g. `mg/dL`, `mmol/L`).
    pub unit: String,
    /// Byte span in the original text.
    pub span: (usize, usize),
}

/// Extract numeric lab values + units from free text.
///
/// Matches shapes like `4.5 mg/dL`, `120 mmHg`, `7.2 mEq/L`, `99.6 °F`.
pub fn parse_lab_values(text: &str) -> Vec<LabValue> {
    // Number followed by unit. Units may contain letters, /, %, °.
    let pattern = Regex::new(r"(?P<value>\d+(?:\.\d+)?)\s*(?P<unit>[A-Za-z°%][A-Za-z/%°]{0,9})")
        .expect("lab-value regex must compile");
    let mut out = Vec::new();
    for caps in pattern.captures_iter(text) {
        if let (Some(num), Some(unit)) = (caps.name("value"), caps.name("unit"))
            && let Ok(value) = num.as_str().parse::<f64>()
        {
            // Filter out obvious non-units like "the" — heuristic:
            // valid unit token contains a slash or starts with a
            // recognized prefix.
            let u = unit.as_str();
            if is_lab_unit(u) {
                out.push(LabValue {
                    value,
                    unit: u.to_string(),
                    span: (num.start(), unit.end()),
                });
            }
        }
    }
    out
}

fn is_lab_unit(s: &str) -> bool {
    const KNOWN: &[&str] = &[
        "mg", "mcg", "g", "kg", "lb", "lbs", "oz", "mg/dL", "mmol/L", "mEq/L", "mIU/L", "ng/mL",
        "pg/mL", "mmHg", "bpm", "kPa", "U/L", "uL", "mL", "L", "%", "°F", "°C", "F", "C",
    ];
    KNOWN.iter().any(|k| k.eq_ignore_ascii_case(s)) || s.contains('/')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_icd10_valid_format() {
        let v = IcdValidator::new();
        assert!(v.is_valid("E11.9")); // Type 2 diabetes
        assert!(v.is_valid("J45.40")); // Asthma
        assert!(v.is_valid("M54.5")); // Low back pain
        assert!(v.is_valid("A00")); // Cholera (3-char)
    }

    #[test]
    fn test_icd10_rejects_bad_input() {
        let v = IcdValidator::new();
        assert!(!v.is_valid("U07.1")); // U is excluded from first letter
        assert!(!v.is_valid("123"));
        assert!(!v.is_valid("ABCD"));
        assert!(!v.is_valid(""));
    }

    #[test]
    fn test_icd10_extract_from_text() {
        let v = IcdValidator::new();
        let codes = v.extract("Diagnoses include E11.9 and J45.40 today.");
        assert!(codes.iter().any(|c| c == "E11.9"));
        assert!(codes.iter().any(|c| c == "J45.40"));
    }

    #[test]
    fn test_medication_dictionary_detects_baseline_drug() {
        let d = MedicationDictionary::baseline();
        let hits = d.scan("Prescribed amoxicillin for the infection.");
        assert!(hits.iter().any(|h| h.name == "amoxicillin"));
    }

    #[test]
    fn test_medication_dictionary_case_insensitive() {
        let d = MedicationDictionary::baseline();
        let hits = d.scan("Take IBUPROFEN twice daily.");
        assert!(hits.iter().any(|h| h.name == "ibuprofen"));
    }

    #[test]
    fn test_medication_hits_sorted_by_span() {
        let d = MedicationDictionary::baseline();
        let hits = d.scan("amoxicillin then ibuprofen later");
        for pair in hits.windows(2) {
            assert!(pair[0].span.0 <= pair[1].span.0);
        }
    }

    #[test]
    fn test_lab_values_parsed_with_unit() {
        let values = parse_lab_values("Glucose 102 mg/dL after fasting");
        let hit = values
            .iter()
            .find(|v| (v.value - 102.0).abs() < f64::EPSILON);
        assert!(hit.is_some());
        assert_eq!(hit.unwrap().unit, "mg/dL");
    }

    #[test]
    fn test_lab_values_multiple_in_one_string() {
        let values = parse_lab_values("BP 120/80 mmHg, HR 72 bpm, Temp 98.6 °F");
        // Three hits expected: 80 mmHg (the slash version may swallow 120),
        // 72 bpm, 98.6 °F.
        assert!(values.iter().any(|v| v.unit.contains("mmHg")));
        assert!(values.iter().any(|v| v.unit == "bpm"));
        assert!(
            values
                .iter()
                .any(|v| v.unit.contains("°F") || v.unit == "F")
        );
    }

    #[test]
    fn test_lab_values_rejects_pure_word_units() {
        // "drink 8 glasses" should not parse as a lab value — "glasses"
        // is not a recognized unit.
        let values = parse_lab_values("drink 8 glasses today");
        assert!(values.iter().all(|v| v.unit != "glasses"));
    }
}
