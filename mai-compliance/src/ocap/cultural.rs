//! Cultural sensitivity filter.
//!
//! Separate from [`super::tribal_data`]: that module identifies
//! tribal *governance metadata* (treaty, reserve, clan, nation), so
//! the engine knows tribal sovereignty applies. *This* module
//! identifies tribal *content* that should be held back for human
//! review even when the requesting actor has authority — sacred
//! knowledge, ceremonial language, and elder teachings.
//!
//! The default action when a cultural-sensitivity signal fires is
//! **deferred processing (quarantine)**, not refusal. The respectful
//! interpretation is: this content deserves a human-in-the-loop
//! review by the relevant cultural authority before the LLM operates
//! on it, regardless of whether the request is otherwise permitted.
//!
//! Audit safety: the matched substring is Blake3-hashed; the raw
//! match never appears in a [`CulturalHit`].

use std::collections::HashMap;

use regex::Regex;
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Category of cultural-sensitivity signal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CulturalSignal {
    /// Sacred or restricted knowledge.
    SacredKnowledge,
    /// Ceremonial content (active practice, not historical record).
    Ceremonial,
    /// Material attributed to an elder or knowledge keeper.
    ElderTeaching,
    /// Funerary, burial, or ancestral-remains content.
    Funerary,
    /// Restricted ethnographic content (anthropological records the
    /// originating community has not released).
    RestrictedEthnographic,
}

impl CulturalSignal {
    /// Wire-format identifier.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::SacredKnowledge => "sacred_knowledge",
            Self::Ceremonial => "ceremonial",
            Self::ElderTeaching => "elder_teaching",
            Self::Funerary => "funerary",
            Self::RestrictedEthnographic => "restricted_ethnographic",
        }
    }
}

/// Confidence tier on a single hit.
///
/// Variants are declared weakest-to-strongest so derived `Ord` gives
/// `Possible < Probable < Explicit`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CulturalConfidence {
    /// Weak signal; may be coincidental.
    Possible,
    /// Pattern is likely cultural in context but ambiguous.
    Probable,
    /// Pattern is unambiguous (named ceremony, explicit attribution).
    Explicit,
}

/// One cultural-sensitivity hit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CulturalHit {
    /// Which signal fired.
    pub signal: CulturalSignal,
    /// Confidence tier.
    pub confidence: CulturalConfidence,
    /// Byte span `(start, end_exclusive)` in the source text.
    pub span: (usize, usize),
    /// Blake3 (first 32 hex chars) of the matched substring. Audit
    /// logs MUST NOT store the raw match.
    pub matched_hash: String,
    /// Rule id.
    pub rule_id: String,
}

/// Aggregate of every cultural-sensitivity hit.
#[derive(Debug, Clone, Default, Serialize)]
pub struct CulturalReport {
    /// All hits in order of occurrence.
    pub hits: Vec<CulturalHit>,
    /// Per-signal counts.
    pub by_signal: HashMap<CulturalSignal, usize>,
    /// Highest confidence observed.
    pub highest_confidence: Option<CulturalConfidence>,
}

impl CulturalReport {
    /// True when any sensitivity signal fired.
    pub fn has_any(&self) -> bool {
        !self.hits.is_empty()
    }

    /// True when at least one Probable / Explicit hit fired.
    pub fn requires_review(&self) -> bool {
        self.highest_confidence
            .map(|c| c >= CulturalConfidence::Probable)
            .unwrap_or(false)
    }

    /// True when at least one Explicit hit fired (the strongest
    /// quarantine signal — content the originating community has
    /// flagged as restricted, not merely "potentially sensitive").
    pub fn requires_explicit_review(&self) -> bool {
        self.highest_confidence
            .map(|c| c == CulturalConfidence::Explicit)
            .unwrap_or(false)
    }

    /// True when any hit is of the given signal.
    pub fn has_signal(&self, signal: CulturalSignal) -> bool {
        self.by_signal.get(&signal).copied().unwrap_or(0) > 0
    }
}

/// Filter errors (build-time only).
#[derive(Debug, Error)]
pub enum CulturalFilterError {
    /// A pattern failed to compile.
    #[error("invalid cultural-filter pattern '{rule_id}': {source}")]
    InvalidPattern {
        /// Rule id that failed.
        rule_id: String,
        /// Underlying regex error.
        source: regex::Error,
    },
}

/// Filter tunables.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CulturalFilterConfig {
    /// Lowest confidence to surface. Defaults to `Probable` — the
    /// filter is intentionally less eager than the tribal-data
    /// detector, because the *consequence* of a hit here is to defer
    /// human-reviewable content; over-firing burdens reviewers without
    /// improving compliance.
    #[serde(default = "default_min_confidence")]
    pub min_confidence: CulturalConfidence,
}

fn default_min_confidence() -> CulturalConfidence {
    CulturalConfidence::Probable
}

impl Default for CulturalFilterConfig {
    fn default() -> Self {
        Self {
            min_confidence: default_min_confidence(),
        }
    }
}

struct CompiledPattern {
    signal: CulturalSignal,
    confidence: CulturalConfidence,
    regex: Regex,
    rule_id: String,
}

/// Cultural sensitivity filter.
pub struct CulturalFilter {
    config: CulturalFilterConfig,
    patterns: Vec<CompiledPattern>,
}

impl CulturalFilter {
    /// Build a filter with a custom config.
    pub fn new(config: CulturalFilterConfig) -> Result<Self, CulturalFilterError> {
        let raw = baseline_patterns();
        let mut compiled = Vec::with_capacity(raw.len());
        for (signal, confidence, rule_id, pattern) in raw {
            let regex =
                Regex::new(pattern).map_err(|source| CulturalFilterError::InvalidPattern {
                    rule_id: rule_id.to_string(),
                    source,
                })?;
            compiled.push(CompiledPattern {
                signal,
                confidence,
                regex,
                rule_id: rule_id.to_string(),
            });
        }
        Ok(Self {
            config,
            patterns: compiled,
        })
    }

    /// Default filter with the baseline pattern catalog.
    pub fn baseline() -> Self {
        Self::new(CulturalFilterConfig::default()).expect("baseline cultural patterns must compile")
    }

    /// Scan text and produce a [`CulturalReport`].
    pub fn scan(&self, text: &str) -> CulturalReport {
        let mut hits: Vec<CulturalHit> = Vec::new();
        let mut by_signal: HashMap<CulturalSignal, usize> = HashMap::new();
        let mut highest_confidence: Option<CulturalConfidence> = None;

        for pattern in &self.patterns {
            if pattern.confidence < self.config.min_confidence {
                continue;
            }
            for found in pattern.regex.find_iter(text) {
                hits.push(CulturalHit {
                    signal: pattern.signal,
                    confidence: pattern.confidence,
                    span: (found.start(), found.end()),
                    matched_hash: hash_match(found.as_str()),
                    rule_id: pattern.rule_id.clone(),
                });
                *by_signal.entry(pattern.signal).or_insert(0) += 1;
                highest_confidence = Some(match highest_confidence {
                    Some(prev) => prev.max(pattern.confidence),
                    None => pattern.confidence,
                });
            }
        }
        hits.sort_by_key(|h| h.span.0);

        CulturalReport {
            hits,
            by_signal,
            highest_confidence,
        }
    }

    /// Configuration accessor.
    pub fn config(&self) -> &CulturalFilterConfig {
        &self.config
    }
}

fn hash_match(s: &str) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(s.as_bytes());
    hasher.finalize().to_hex().to_string()[..32].to_string()
}

/// Baseline cultural-sensitivity pattern catalog. Tuples are
/// `(signal, confidence, rule_id, pattern)`.
///
/// Like the tribal-data baseline, these patterns reference *categories*
/// of sensitive content rather than any specific community's
/// vocabulary. Operators extend the catalog through
/// `config/compliance/ocap.toml`.
fn baseline_patterns() -> Vec<(
    CulturalSignal,
    CulturalConfidence,
    &'static str,
    &'static str,
)> {
    use CulturalConfidence::*;
    use CulturalSignal::*;
    vec![
        // Sacred knowledge.
        (
            SacredKnowledge,
            Explicit,
            "cultural.sacred.restricted",
            r"(?i)\b(?:restricted\s+(?:teaching|knowledge)s?|closed\s+ceremon(?:y|ies)|men[-\s]only\s+teachings?|women[-\s]only\s+teachings?)\b",
        ),
        (
            SacredKnowledge,
            Probable,
            "cultural.sacred.generic",
            r"(?i)\b(?:sacred\s+(?:teaching|knowledge|story|song)|spiritual\s+practice\s+restricted)\b",
        ),
        // Ceremonial.
        (
            Ceremonial,
            Explicit,
            "cultural.ceremony.named",
            r"(?i)\b(?:Sun\s*dance|Potlatch|Sweat\s+Lodge\s+ceremony|Pipe\s+Ceremony|Vision\s+Quest|Smudg(?:e|ing)\s+Ceremony|Naming\s+Ceremony|Coming[-\s]of[-\s]Age\s+Ceremony)\b",
        ),
        (
            Ceremonial,
            Probable,
            "cultural.ceremony.active",
            r"(?i)\b(?:ceremonial\s+(?:protocol|practice|preparation)|in\s+ceremony|during\s+the\s+ceremony)\b",
        ),
        // Elder teachings.
        (
            ElderTeaching,
            Explicit,
            "cultural.elder.attribution",
            r"(?i)\b(?:as\s+(?:told|shared|taught)\s+by\s+(?:our\s+)?(?:elder|knowledge\s+keeper|grandmother|grandfather)|elder\s+teaching|knowledge\s+keeper\s+(?:said|shared|taught))\b",
        ),
        (
            ElderTeaching,
            Probable,
            "cultural.elder.generic",
            r"(?i)\b(?:elder\s+wisdom|elder\s+counsel|elder\s+guidance|elder\s+story)\b",
        ),
        // Funerary / burial / ancestral remains.
        (
            Funerary,
            Explicit,
            "cultural.funerary.remains",
            r"(?i)\b(?:ancestral\s+remains|repatriation\s+(?:claim|request)|NAGPRA|burial\s+goods?|grave\s+goods?)\b",
        ),
        (
            Funerary,
            Probable,
            "cultural.funerary.generic",
            r"(?i)\b(?:burial\s+site|burial\s+ceremony|funeral\s+rites?|mourning\s+practice)\b",
        ),
        // Restricted ethnographic.
        (
            RestrictedEthnographic,
            Probable,
            "cultural.ethno.restricted",
            r"(?i)\b(?:restricted\s+ethnograph(?:y|ic)|unreleased\s+field\s+notes|community\s+embargoed\s+(?:material|records?))\b",
        ),
        (
            RestrictedEthnographic,
            Possible,
            "cultural.ethno.generic",
            r"(?i)\b(?:salvage\s+ethnography|colonial\s+era\s+anthropolog(?:y|ical)\s+record)\b",
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn neutral_text_produces_empty_report() {
        let f = CulturalFilter::baseline();
        let report = f.scan("Tell me about the weather this week.");
        assert!(!report.has_any());
        assert!(!report.requires_review());
    }

    #[test]
    fn detects_named_ceremony() {
        let f = CulturalFilter::baseline();
        let report = f.scan("Notes from the Pipe Ceremony last summer.");
        assert!(report.has_signal(CulturalSignal::Ceremonial));
        assert!(report.requires_explicit_review());
    }

    #[test]
    fn detects_sacred_knowledge() {
        let f = CulturalFilter::baseline();
        let report = f.scan("This is a sacred teaching shared in confidence.");
        assert!(report.has_signal(CulturalSignal::SacredKnowledge));
        assert!(report.requires_review());
    }

    #[test]
    fn detects_explicit_restricted_knowledge() {
        let f = CulturalFilter::baseline();
        let report = f.scan("These are women-only teachings, not for outside circulation.");
        assert!(report.has_signal(CulturalSignal::SacredKnowledge));
        assert!(report.requires_explicit_review());
    }

    #[test]
    fn detects_elder_attribution() {
        let f = CulturalFilter::baseline();
        let report = f.scan("As shared by our elder, the story begins this way.");
        assert!(report.has_signal(CulturalSignal::ElderTeaching));
        assert!(report.requires_explicit_review());
    }

    #[test]
    fn detects_funerary_content() {
        let f = CulturalFilter::baseline();
        let report = f.scan("Update the case file on ancestral remains repatriation.");
        assert!(report.has_signal(CulturalSignal::Funerary));
        assert!(report.requires_explicit_review());
    }

    #[test]
    fn nagpra_reference_is_funerary() {
        let f = CulturalFilter::baseline();
        let report = f.scan("Comply with NAGPRA reporting requirements.");
        assert!(report.has_signal(CulturalSignal::Funerary));
    }

    #[test]
    fn matched_text_is_hashed_not_stored() {
        let f = CulturalFilter::baseline();
        let report = f.scan("Notes from the Pipe Ceremony last summer.");
        let hit = report.hits.first().expect("hit");
        assert_eq!(hit.matched_hash.len(), 32);
        let serialised = serde_json::to_string(&hit).expect("serialise");
        assert!(!serialised.contains("Pipe Ceremony"));
        assert!(serialised.contains(&hit.matched_hash));
    }

    #[test]
    fn min_confidence_filters_possible_hits() {
        // Default is Probable; only the explicit `salvage ethnography`
        // probable would surface anyway. Drop to Possible to confirm.
        let f = CulturalFilter::new(CulturalFilterConfig {
            min_confidence: CulturalConfidence::Possible,
        })
        .unwrap();
        let report = f.scan("Reviewing salvage ethnography from the 1920s.");
        assert!(report.has_signal(CulturalSignal::RestrictedEthnographic));
    }

    #[test]
    fn possible_hits_dropped_when_min_is_probable() {
        let f = CulturalFilter::baseline();
        // "salvage ethnography" is Possible; default config drops it.
        let report = f.scan("Reviewing salvage ethnography from the 1920s.");
        assert!(!report.has_any());
    }

    #[test]
    fn hits_are_ordered_by_span_start() {
        let f = CulturalFilter::baseline();
        let report = f.scan(
            "Pipe Ceremony notes, then ancestral remains case file, then sacred teaching review.",
        );
        let starts: Vec<usize> = report.hits.iter().map(|h| h.span.0).collect();
        let mut sorted = starts.clone();
        sorted.sort_unstable();
        assert_eq!(starts, sorted);
    }

    #[test]
    fn requires_review_is_false_when_only_possible_hits() {
        // With min_confidence = Possible, the Possible-only salvage
        // ethnography fires but does NOT require review.
        let f = CulturalFilter::new(CulturalFilterConfig {
            min_confidence: CulturalConfidence::Possible,
        })
        .unwrap();
        let report = f.scan("Reviewing salvage ethnography from the 1920s.");
        assert!(report.has_any());
        assert!(!report.requires_review());
        assert!(!report.requires_explicit_review());
    }
}
