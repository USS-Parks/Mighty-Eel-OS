//! Technical-data classifier.
//!
//! Generic detector for content shaped like engineering / technical
//! data, independent of the USML category catalog. The output is a
//! [`TechDataAssessment`] with three axes:
//!
//! - **Drawings** — references to CAD drawings, blueprints, schematics,
//!   ICDs, drawing numbers, and revision blocks.
//! - **Specifications** — material specs, tolerances, MIL/AS/ASTM
//!   standard references, GD&T callouts, materials-of-construction
//!   tables.
//! - **Design methodology** — descriptions of how a defense article is
//!   built (assembly procedures, manufacturing sequence, integration).
//!
//! The detector is heuristic-only; an optional ML / Python-subprocess
//! classifier can be plugged in later by implementing the
//! [`TechDataClassifier`] trait. The heuristic implementation is the
//! "air-gap friendly fallback" called out in the prompt:
//! no model weights, no network, single-pass regex sweep.
//!
//! Confidence levels follow the same `Possible / Probable / Explicit`
//! convention as the surrounding modules so downstream rule engines
//! can gate uniformly.

use regex::Regex;
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// One of the three technical-data signal kinds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TechDataSignal {
    /// Drawing references — schematics, CAD, blueprint, ICD.
    Drawing,
    /// Engineering specifications — standards, tolerances, materials.
    Specification,
    /// Design methodology / manufacturing description.
    DesignMethodology,
}

impl TechDataSignal {
    /// Wire-format identifier.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Drawing => "drawing",
            Self::Specification => "specification",
            Self::DesignMethodology => "design_methodology",
        }
    }
}

/// Confidence tier for a tech-data signal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TechDataConfidence {
    /// Weak keyword overlap.
    Possible,
    /// Likely technical content.
    Probable,
    /// Highly specific technical data marker.
    Explicit,
}

/// One detected signal match.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TechDataHit {
    /// Signal kind.
    pub signal: TechDataSignal,
    /// Confidence tier.
    pub confidence: TechDataConfidence,
    /// Byte span `(start, end_exclusive)`.
    pub span: (usize, usize),
    /// Blake3 hash (first 32 hex) of the matched substring. Audit logs
    /// must never store the raw text.
    pub matched_hash: String,
}

/// Aggregate assessment for a query.
#[derive(Debug, Clone, Default, Serialize)]
pub struct TechDataAssessment {
    /// All matched signals in source order.
    pub hits: Vec<TechDataHit>,
    /// Highest confidence observed.
    pub highest_confidence: Option<TechDataConfidence>,
    /// True when the text contains at least one Probable or Explicit
    /// hit. This is the gating signal the jurisdiction module should
    /// consult to escalate ambiguous content.
    pub looks_like_tech_data: bool,
}

impl TechDataAssessment {
    /// True when any hit was found.
    pub fn has_any(&self) -> bool {
        !self.hits.is_empty()
    }

    /// Total hits.
    pub fn total_hits(&self) -> usize {
        self.hits.len()
    }
}

/// Build-time errors.
#[derive(Debug, Error)]
pub enum TechDataError {
    /// A pattern in the catalog failed to compile.
    #[error("invalid tech-data pattern for {signal:?}: {source}")]
    InvalidPattern {
        signal: TechDataSignal,
        source: regex::Error,
    },
}

/// Classifier trait. The default implementation
/// ([`HeuristicTechDataClassifier`]) is regex-based and ships in this
/// crate. Operators wanting an ML pipeline can implement this trait,
/// e.g. wrapping a Python subprocess, and swap it into the policy
/// runtime.
pub trait TechDataClassifier: Send + Sync {
    /// Run the classifier over `text` and return an assessment.
    fn assess(&self, text: &str) -> TechDataAssessment;
}

struct CompiledPattern {
    signal: TechDataSignal,
    confidence: TechDataConfidence,
    regex: Regex,
}

/// Heuristic implementation. Build once, reuse across requests.
pub struct HeuristicTechDataClassifier {
    patterns: Vec<CompiledPattern>,
}

impl HeuristicTechDataClassifier {
    /// Build a classifier from the baseline catalog.
    pub fn new() -> Result<Self, TechDataError> {
        let raw = baseline_patterns();
        let mut compiled = Vec::with_capacity(raw.len());
        for (signal, confidence, pattern) in raw {
            let regex = Regex::new(pattern)
                .map_err(|source| TechDataError::InvalidPattern { signal, source })?;
            compiled.push(CompiledPattern {
                signal,
                confidence,
                regex,
            });
        }
        Ok(Self { patterns: compiled })
    }

    /// Infallible convenience wrapper.
    pub fn baseline() -> Self {
        Self::new().expect("baseline tech-data patterns must compile")
    }
}

impl Default for HeuristicTechDataClassifier {
    fn default() -> Self {
        Self::baseline()
    }
}

impl TechDataClassifier for HeuristicTechDataClassifier {
    fn assess(&self, text: &str) -> TechDataAssessment {
        let mut hits: Vec<TechDataHit> = Vec::new();
        let mut highest: Option<TechDataConfidence> = None;
        for pattern in &self.patterns {
            for found in pattern.regex.find_iter(text) {
                hits.push(TechDataHit {
                    signal: pattern.signal,
                    confidence: pattern.confidence,
                    span: (found.start(), found.end()),
                    matched_hash: hash_match(found.as_str()),
                });
                highest = Some(match highest {
                    Some(prev) => prev.max(pattern.confidence),
                    None => pattern.confidence,
                });
            }
        }
        hits.sort_by_key(|h| h.span.0);
        let looks_like_tech_data = highest.is_some_and(|c| c >= TechDataConfidence::Probable);
        TechDataAssessment {
            hits,
            highest_confidence: highest,
            looks_like_tech_data,
        }
    }
}

fn hash_match(s: &str) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(s.as_bytes());
    hasher.finalize().to_hex().to_string()[..32].to_string()
}

/// Baseline pattern catalog.
fn baseline_patterns() -> Vec<(TechDataSignal, TechDataConfidence, &'static str)> {
    use TechDataConfidence::*;
    use TechDataSignal::*;
    vec![
        // ---- Drawings ----
        // Explicit drawing-number prefix (DWG, CDRL, ICD).
        (
            Drawing,
            Explicit,
            r"(?i)\b(?:DWG|drawing\s+no\.?|drawing\s+number|ICD|CDRL)\s*[#:-]?\s*[A-Z0-9-]{4,}\b",
        ),
        // Revision markers: REV A, Rev. 03, Sheet 2 of 5.
        (
            Drawing,
            Probable,
            r"(?i)\b(?:rev(?:ision)?\.?\s*[A-Z0-9]{1,3}|sheet\s+\d+\s+of\s+\d+)\b",
        ),
        // CAD / schematic / blueprint mentions.
        (
            Drawing,
            Probable,
            r"(?i)\b(?:CAD\s+(?:model|file|drawing)|schematic\s+diagram|blueprint|technical\s+drawing|exploded\s+view)\b",
        ),
        // Possible: bare "drawing".
        (
            Drawing,
            Possible,
            r"(?i)\b(?:drawing|schematic|blueprint)\b",
        ),
        // ---- Specifications ----
        // MIL-STD / MIL-PRF / MIL-DTL / MIL-HDBK references.
        (
            Specification,
            Explicit,
            r"(?i)\bMIL[-\s]?(?:STD|PRF|DTL|HDBK|SPEC)[-\s]?\d{2,5}[A-Z]?\b",
        ),
        // AS / SAE / NAS / ASTM / AMS / ANSI standard references.
        (
            Specification,
            Explicit,
            r"\b(?:AS|SAE|NAS|ASTM|AMS|ANSI)\s?[-]?\s?[A-Z]?\d{2,5}[A-Z]?\b",
        ),
        // GD&T callouts: ASME Y14.5.
        (Specification, Explicit, r"\bASME\s+Y14(?:\.\d+)?\b"),
        // Tolerance callouts: ±0.005 in, +/- 0.1 mm.
        (
            Specification,
            Probable,
            r"(?:±|\+/-|\+-)\s*\d+(?:\.\d+)?\s*(?:mm|in|inch|um|micron)\b",
        ),
        // Material designations: 6061-T6, 7075-T6, 17-4 PH, Inconel 718.
        (
            Specification,
            Probable,
            r"(?i)\b(?:\d{4}[-]T\d|17[-]4\s+PH|Inconel\s+\d{3}|Ti[-\s]?6Al[-\s]?4V|maraging\s+\d{3})\b",
        ),
        // Generic spec phrasing.
        (
            Specification,
            Possible,
            r"(?i)\b(?:engineering\s+specification|design\s+specification|materials?\s+of\s+construction)\b",
        ),
        // ---- Design methodology ----
        // Explicit: assembly drawing, manufacturing process plan.
        (
            DesignMethodology,
            Explicit,
            r"(?i)\b(?:manufacturing\s+process\s+plan|assembly\s+procedure|build[\s-]to[\s-]print|process\s+work\s+instructions?)\b",
        ),
        // Probable: integration / assembly / fabrication described.
        (
            DesignMethodology,
            Probable,
            r"(?i)\b(?:assembly\s+sequence|fabrication\s+procedure|integration\s+procedure|qualification\s+test\s+plan|acceptance\s+test\s+procedure)\b",
        ),
        // Probable: design-data / configuration management terms.
        (
            DesignMethodology,
            Probable,
            r"(?i)\b(?:design\s+data\s+package|TDP|technical\s+data\s+package|configuration\s+management\s+plan)\b",
        ),
        // Possible: bare "assembly", "fabrication".
        (
            DesignMethodology,
            Possible,
            r"(?i)\b(?:assembly|fabrication|manufacturing\s+method)\b",
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cls() -> HeuristicTechDataClassifier {
        HeuristicTechDataClassifier::baseline()
    }

    #[test]
    fn test_neutral_text_has_no_signal() {
        let a = cls().assess("Tell me about ocean currents.");
        assert!(!a.has_any());
        assert!(!a.looks_like_tech_data);
        assert!(a.highest_confidence.is_none());
    }

    #[test]
    fn test_drawing_number_explicit() {
        let a = cls().assess("See DWG 12345-A revision 02 for details.");
        let hit = a.hits.iter().find(|h| {
            h.signal == TechDataSignal::Drawing && h.confidence == TechDataConfidence::Explicit
        });
        assert!(hit.is_some());
        assert!(a.looks_like_tech_data);
    }

    #[test]
    fn test_mil_std_explicit() {
        let a = cls().assess("Surface finish per MIL-STD-810H.");
        let hit = a.hits.iter().find(|h| {
            h.signal == TechDataSignal::Specification
                && h.confidence == TechDataConfidence::Explicit
        });
        assert!(hit.is_some());
        assert!(a.looks_like_tech_data);
    }

    #[test]
    fn test_astm_standard_explicit() {
        let a = cls().assess("Conforms to ASTM A36 carbon steel grade.");
        assert!(
            a.hits
                .iter()
                .any(|h| h.signal == TechDataSignal::Specification)
        );
    }

    #[test]
    fn test_tolerance_callout_probable() {
        let a = cls().assess("Maintain dimension ±0.005 in over the bore.");
        assert!(
            a.hits
                .iter()
                .any(|h| h.signal == TechDataSignal::Specification)
        );
        assert!(a.looks_like_tech_data);
    }

    #[test]
    fn test_material_alloy_designation() {
        let a = cls().assess("Machined from 6061-T6 aluminum stock.");
        assert!(
            a.hits
                .iter()
                .any(|h| h.signal == TechDataSignal::Specification)
        );
    }

    #[test]
    fn test_manufacturing_process_plan_explicit_methodology() {
        let a = cls().assess("Refer to the manufacturing process plan section 4.2.");
        let hit = a.hits.iter().find(|h| {
            h.signal == TechDataSignal::DesignMethodology
                && h.confidence == TechDataConfidence::Explicit
        });
        assert!(hit.is_some());
    }

    #[test]
    fn test_tdp_acronym_methodology() {
        let a = cls().assess("Customer must deliver the full TDP at PDR.");
        assert!(
            a.hits
                .iter()
                .any(|h| h.signal == TechDataSignal::DesignMethodology)
        );
    }

    #[test]
    fn test_possible_only_does_not_promote() {
        let a = cls().assess("the drawing is on the wall");
        assert!(a.has_any());
        // Highest is Possible → looks_like_tech_data must be false.
        assert!(!a.looks_like_tech_data);
        assert_eq!(a.highest_confidence, Some(TechDataConfidence::Possible));
    }

    #[test]
    fn test_cad_schematic_promotes_to_probable() {
        let a = cls().assess("Attached CAD drawing of the harness.");
        assert!(a.looks_like_tech_data);
    }

    #[test]
    fn test_hits_sorted_by_span_start() {
        let a = cls().assess("DWG 1234-A then MIL-STD-810H then assembly procedure noted.");
        for pair in a.hits.windows(2) {
            assert!(pair[0].span.0 <= pair[1].span.0);
        }
    }

    #[test]
    fn test_matched_text_is_hashed() {
        let a = cls().assess("Surface finish per MIL-STD-810H.");
        let hit = a.hits.first().expect("expected at least one hit");
        assert_eq!(hit.matched_hash.len(), 32);
        assert_ne!(hit.matched_hash, "MIL-STD-810H");
    }

    #[test]
    fn test_assembly_procedure_probable_methodology() {
        let a = cls().assess("Follow the assembly sequence outlined in section 3.");
        assert!(
            a.hits
                .iter()
                .any(|h| h.signal == TechDataSignal::DesignMethodology)
        );
        assert!(a.looks_like_tech_data);
    }

    #[test]
    fn test_classifier_trait_object_dispatch() {
        let cls: Box<dyn TechDataClassifier> = Box::new(HeuristicTechDataClassifier::baseline());
        let a = cls.assess("DWG 9999-X revision A");
        assert!(a.looks_like_tech_data);
    }
}
