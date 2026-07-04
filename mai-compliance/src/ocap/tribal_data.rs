//! Tribal identifier and traditional-knowledge detection.
//!
//! Pattern-based scanner that surfaces references to tribal nations,
//! reserves, clans, sacred sites, ceremonies, traditional medicines,
//! and elder-attributed material. The output is a
//! [`TribalDataReport`] consumed by [`super::ocap_rules`].
//!
//! Detection is intentionally conservative and configurable: every
//! deployment ships with the in-crate baseline dictionary, and the
//! tribal nation served by the appliance extends it through
//! `config/compliance/ocap.toml`. The shipped patterns are
//! deliberately *generic* — they reference categories of tribal
//! identifiers, not specific nations — so that the baseline doesn't
//! claim authority over any particular community's vocabulary.
//!
//! Audit safety: every matched substring is Blake3-hashed (first 32
//! hex chars). The raw match is never retained on the [`TribalHit`],
//! so audit logs cannot leak tribal-governance-restricted content.

use std::collections::HashMap;

use regex::Regex;
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Kind of tribal identifier surfaced by the detector.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TribalIdentifierKind {
    /// Treaty reference (e.g. "Treaty 7", "1871 Treaty").
    TreatyReference,
    /// Reserve, reservation, or rancheria name.
    Reserve,
    /// Clan / band / house designation.
    Clan,
    /// Sacred site (geographic reference flagged by tribal authority).
    SacredSite,
    /// Ceremony name (Sundance, Potlatch, etc.) or ceremonial language.
    Ceremony,
    /// Traditional knowledge: stories, medicines, ecological knowledge.
    TraditionalKnowledge,
    /// Content attributed to an elder or knowledge keeper.
    ElderAttribution,
    /// Generic tribal-nation reference (treaty signatory, federally
    /// recognised tribe / First Nation / band).
    NationReference,
}

impl TribalIdentifierKind {
    /// Wire-format identifier for audit emission.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::TreatyReference => "treaty_reference",
            Self::Reserve => "reserve",
            Self::Clan => "clan",
            Self::SacredSite => "sacred_site",
            Self::Ceremony => "ceremony",
            Self::TraditionalKnowledge => "traditional_knowledge",
            Self::ElderAttribution => "elder_attribution",
            Self::NationReference => "nation_reference",
        }
    }
}

/// Detection confidence tier.
///
/// Variants are declared weakest-to-strongest so derived `Ord` gives
/// `Possible < Probable < Explicit`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OcapConfidence {
    /// Weak signal — keyword overlap, may be coincidental.
    Possible,
    /// Pattern is likely tribal in context but could be benign.
    Probable,
    /// Pattern is unambiguous (explicit clan name, sacred-site name,
    /// elder attribution).
    Explicit,
}

/// One detected tribal-identifier hit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TribalHit {
    /// Which kind of identifier matched.
    pub kind: TribalIdentifierKind,
    /// Confidence tier of this match.
    pub confidence: OcapConfidence,
    /// Byte span `(start, end_exclusive)` in the source text.
    pub span: (usize, usize),
    /// Blake3 hash (first 32 hex chars) of the matched substring.
    /// Audit logs MUST NOT store the raw match.
    pub matched_hash: String,
    /// Identifier of the pattern that fired. Useful for tuning and for
    /// the audit log; the pattern itself is configuration, not secret.
    pub rule_id: String,
}

/// Aggregate of every tribal identifier hit.
#[derive(Debug, Clone, Default, Serialize)]
pub struct TribalDataReport {
    /// All hits in order of occurrence.
    pub hits: Vec<TribalHit>,
    /// Per-kind counts.
    pub by_kind: HashMap<TribalIdentifierKind, usize>,
    /// Highest confidence observed across all hits.
    pub highest_confidence: Option<OcapConfidence>,
}

impl TribalDataReport {
    /// True when the report has any hit at all.
    pub fn has_any(&self) -> bool {
        !self.hits.is_empty()
    }

    /// True when at least one Probable or Explicit hit fired (i.e. the
    /// content is likely tribal data, not merely keyword overlap).
    pub fn has_actionable(&self) -> bool {
        self.highest_confidence
            .map(|c| c >= OcapConfidence::Probable)
            .unwrap_or(false)
    }

    /// Total number of hits across all kinds.
    pub fn total_hits(&self) -> usize {
        self.hits.len()
    }

    /// True when any hit is of the given kind.
    pub fn has_kind(&self, kind: TribalIdentifierKind) -> bool {
        self.by_kind.get(&kind).copied().unwrap_or(0) > 0
    }
}

/// Detector errors (build-time only).
#[derive(Debug, Error)]
pub enum TribalDataError {
    /// A pattern failed to compile.
    #[error("invalid tribal-data pattern '{rule_id}': {source}")]
    InvalidPattern {
        /// Rule id that failed.
        rule_id: String,
        /// Underlying regex error.
        source: regex::Error,
    },
}

/// Detector tunables.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TribalDataDetectorConfig {
    /// Lowest confidence to surface in the report. Defaults to
    /// `Possible` (most coverage).
    #[serde(default = "default_min_confidence")]
    pub min_confidence: OcapConfidence,
    /// When true, Possible-only matches still promote the report to
    /// "has tribal data" status. Defaults to `false`: the OCAP policy
    /// engine wants explicit evidence to assert tribal governance,
    /// since false positives are a respect violation, not just an
    /// availability cost.
    #[serde(default)]
    pub possible_implies_tribal: bool,
}

fn default_min_confidence() -> OcapConfidence {
    OcapConfidence::Possible
}

impl Default for TribalDataDetectorConfig {
    fn default() -> Self {
        Self {
            min_confidence: default_min_confidence(),
            possible_implies_tribal: false,
        }
    }
}

struct CompiledPattern {
    kind: TribalIdentifierKind,
    confidence: OcapConfidence,
    regex: Regex,
    rule_id: String,
}

/// Tribal identifier scanner. Build once at startup, reuse across
/// requests.
pub struct TribalDataDetector {
    config: TribalDataDetectorConfig,
    patterns: Vec<CompiledPattern>,
}

impl TribalDataDetector {
    /// Build a detector with a custom config.
    pub fn new(config: TribalDataDetectorConfig) -> Result<Self, TribalDataError> {
        let raw = baseline_patterns();
        let mut compiled = Vec::with_capacity(raw.len());
        for (kind, confidence, rule_id, pattern) in raw {
            let regex = Regex::new(pattern).map_err(|source| TribalDataError::InvalidPattern {
                rule_id: rule_id.to_string(),
                source,
            })?;
            compiled.push(CompiledPattern {
                kind,
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

    /// Default detector with the baseline pattern catalog.
    pub fn baseline() -> Self {
        Self::new(TribalDataDetectorConfig::default())
            .expect("baseline tribal-data patterns must compile")
    }

    /// Extend the detector with operator-supplied patterns. Returns
    /// `Err` on the first invalid regex; the detector is left
    /// unchanged in that case.
    pub fn with_extra_patterns(
        mut self,
        extras: Vec<(TribalIdentifierKind, OcapConfidence, String, String)>,
    ) -> Result<Self, TribalDataError> {
        let mut compiled = Vec::with_capacity(extras.len());
        for (kind, confidence, rule_id, pattern) in extras {
            let regex = Regex::new(&pattern).map_err(|source| TribalDataError::InvalidPattern {
                rule_id: rule_id.clone(),
                source,
            })?;
            compiled.push(CompiledPattern {
                kind,
                confidence,
                regex,
                rule_id,
            });
        }
        self.patterns.extend(compiled);
        Ok(self)
    }

    /// Scan text and produce a [`TribalDataReport`].
    pub fn scan(&self, text: &str) -> TribalDataReport {
        let mut hits: Vec<TribalHit> = Vec::new();
        let mut by_kind: HashMap<TribalIdentifierKind, usize> = HashMap::new();
        let mut highest_confidence: Option<OcapConfidence> = None;

        for pattern in &self.patterns {
            if pattern.confidence < self.config.min_confidence {
                continue;
            }
            for found in pattern.regex.find_iter(text) {
                hits.push(TribalHit {
                    kind: pattern.kind,
                    confidence: pattern.confidence,
                    span: (found.start(), found.end()),
                    matched_hash: hash_match(found.as_str()),
                    rule_id: pattern.rule_id.clone(),
                });
                *by_kind.entry(pattern.kind).or_insert(0) += 1;
                highest_confidence = Some(match highest_confidence {
                    Some(prev) => prev.max(pattern.confidence),
                    None => pattern.confidence,
                });
            }
        }
        hits.sort_by_key(|h| h.span.0);

        TribalDataReport {
            hits,
            by_kind,
            highest_confidence,
        }
    }

    /// Configuration accessor.
    pub fn config(&self) -> &TribalDataDetectorConfig {
        &self.config
    }
}

fn hash_match(s: &str) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(s.as_bytes());
    hasher.finalize().to_hex().to_string()[..32].to_string()
}

/// Baseline pattern catalog. Tuples are `(kind, confidence, rule_id,
/// pattern)`.
///
/// The shipped patterns reference *categories* of tribal vocabulary,
/// not any specific nation's vocabulary. Operators MUST extend the
/// catalog with deployment-specific terms in
/// `config/compliance/ocap.toml`, and tribal-government deployments
/// are expected to review and approve the local extension before the
/// detector is enabled in production.
fn baseline_patterns() -> Vec<(
    TribalIdentifierKind,
    OcapConfidence,
    &'static str,
    &'static str,
)> {
    use OcapConfidence::*;
    use TribalIdentifierKind::*;
    vec![
        // Treaty references — numbered treaties and year-prefixed
        // treaties dominate the corpus.
        (
            TreatyReference,
            Explicit,
            "ocap.treaty.numbered",
            r"(?i)\bTreaty\s+(?:No\.?\s*)?(?:1[0-1]|[1-9])\b",
        ),
        (
            TreatyReference,
            Explicit,
            "ocap.treaty.year_prefixed",
            r"(?i)\b1[78]\d{2}\s+Treaty\b",
        ),
        (
            TreatyReference,
            Probable,
            "ocap.treaty.generic",
            r"(?i)\b(?:treaty\s+rights?|treaty\s+obligations?|treaty\s+signatory)\b",
        ),
        // Reserves / reservations / rancherias.
        (
            Reserve,
            Probable,
            "ocap.reserve.generic",
            r"(?i)\b(?:Indian\s+Reserve|First\s+Nations?\s+Reserve|tribal\s+reservation|rancheria|reservation\s+land)\b",
        ),
        (
            Reserve,
            Possible,
            "ocap.reserve.shortform",
            r"(?i)\b(?:on\s+rez|on[-\s]reserve|reservation\s+(?:residents?|members?))\b",
        ),
        // Clans / bands / houses.
        (
            Clan,
            Probable,
            "ocap.clan.generic",
            r"(?i)\b(?:clan\s+(?:mother|elder|chief)|matriarchal\s+clan|moiety|house\s+group)\b",
        ),
        (
            Clan,
            Possible,
            "ocap.clan.band",
            r"(?i)\b(?:band\s+council|band\s+member|hereditary\s+chief)\b",
        ),
        // Sacred sites.
        (
            SacredSite,
            Explicit,
            "ocap.sacred.explicit",
            r"(?i)\b(?:sacred\s+site|sacred\s+land|sacred\s+ground|burial\s+ground|sacred\s+mountain)\b",
        ),
        (
            SacredSite,
            Probable,
            "ocap.sacred.ceremonial_site",
            r"(?i)\b(?:ceremonial\s+(?:site|grounds?)|sweat\s+lodge\s+site|petroglyph\s+site)\b",
        ),
        // Ceremonies / ceremonial language.
        (
            Ceremony,
            Explicit,
            "ocap.ceremony.named",
            r"(?i)\b(?:Sun\s*dance|Potlatch|Sweat\s+Lodge|Pipe\s+Ceremony|Vision\s+Quest|Smudg(?:e|ing)\s+Ceremony)\b",
        ),
        (
            Ceremony,
            Probable,
            "ocap.ceremony.generic",
            r"(?i)\b(?:ceremonial\s+(?:protocol|knowledge|practice)|sacred\s+ceremony|traditional\s+ceremony)\b",
        ),
        // Traditional knowledge.
        (
            TraditionalKnowledge,
            Explicit,
            "ocap.tk.explicit",
            r"(?i)\b(?:traditional\s+(?:ecological\s+)?knowledge|TEK\b|Indigenous\s+knowledge\s+system)\b",
        ),
        (
            TraditionalKnowledge,
            Probable,
            "ocap.tk.medicine",
            r"(?i)\b(?:traditional\s+medicine|medicine\s+(?:plant|wheel|bundle)|herbal\s+medicine\s+teachings?)\b",
        ),
        (
            TraditionalKnowledge,
            Probable,
            "ocap.tk.story",
            r"(?i)\b(?:traditional\s+stor(?:y|ies)|creation\s+story|oral\s+tradition|origin\s+story)\b",
        ),
        // Elder attribution.
        (
            ElderAttribution,
            Explicit,
            "ocap.elder.explicit",
            r"(?i)\b(?:as\s+told\s+by\s+(?:elder|knowledge\s+keeper)|elder\s+teaching|elder\s+wisdom|knowledge\s+keeper\s+said)\b",
        ),
        (
            ElderAttribution,
            Probable,
            "ocap.elder.generic",
            r"(?i)\b(?:tribal\s+elder|community\s+elder|First\s+Nations?\s+elder)\b",
        ),
        // Nation reference (general — used to confirm tribal context
        // even when no specific governance keyword is present).
        (
            NationReference,
            Probable,
            "ocap.nation.generic",
            r"(?i)\b(?:First\s+Nations?|Indigenous\s+(?:peoples?|community)|tribal\s+(?:government|nation|council)|Native\s+American\s+nation)\b",
        ),
        (
            NationReference,
            Possible,
            "ocap.nation.federally_recognized",
            r"(?i)\b(?:federally\s+recognized\s+tribe|recognized\s+First\s+Nation|sovereign\s+tribal\s+nation)\b",
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_tribal_content_produces_empty_report() {
        let det = TribalDataDetector::baseline();
        let report = det.scan("Tell me about the weather in Phoenix.");
        assert!(!report.has_any());
        assert!(!report.has_actionable());
        assert_eq!(report.total_hits(), 0);
    }

    #[test]
    fn detects_numbered_treaty() {
        let det = TribalDataDetector::baseline();
        let report = det.scan("This data falls under Treaty 7.");
        assert!(report.has_kind(TribalIdentifierKind::TreatyReference));
        assert!(report.has_actionable());
        assert_eq!(report.highest_confidence, Some(OcapConfidence::Explicit));
    }

    #[test]
    fn detects_year_prefixed_treaty() {
        let det = TribalDataDetector::baseline();
        let report = det.scan("Provisions from the 1871 Treaty are still binding.");
        assert!(report.has_kind(TribalIdentifierKind::TreatyReference));
    }

    #[test]
    fn detects_reserve_reference() {
        let det = TribalDataDetector::baseline();
        let report = det.scan("Subjects living on-reserve were surveyed.");
        assert!(report.has_kind(TribalIdentifierKind::Reserve));
    }

    #[test]
    fn detects_sacred_site() {
        let det = TribalDataDetector::baseline();
        let report = det.scan("The path runs near a sacred burial ground.");
        assert!(report.has_kind(TribalIdentifierKind::SacredSite));
        assert_eq!(report.highest_confidence, Some(OcapConfidence::Explicit));
    }

    #[test]
    fn detects_named_ceremony() {
        let det = TribalDataDetector::baseline();
        let report = det.scan("Notes from the Pipe Ceremony last summer.");
        assert!(report.has_kind(TribalIdentifierKind::Ceremony));
        assert!(report.has_actionable());
    }

    #[test]
    fn detects_traditional_knowledge() {
        let det = TribalDataDetector::baseline();
        let report = det.scan("Document the traditional ecological knowledge of the region.");
        assert!(report.has_kind(TribalIdentifierKind::TraditionalKnowledge));
        assert!(report.has_actionable());
    }

    #[test]
    fn detects_elder_attribution() {
        let det = TribalDataDetector::baseline();
        let report = det.scan("As told by elder Mary, the creation story begins...");
        assert!(report.has_kind(TribalIdentifierKind::ElderAttribution));
        assert!(report.has_kind(TribalIdentifierKind::TraditionalKnowledge));
    }

    #[test]
    fn detects_nation_reference() {
        let det = TribalDataDetector::baseline();
        let report = det.scan("Meeting with a First Nations council on Tuesday.");
        assert!(report.has_kind(TribalIdentifierKind::NationReference));
    }

    #[test]
    fn matched_substring_is_hashed_not_stored() {
        let det = TribalDataDetector::baseline();
        let text = "This data falls under Treaty 7.";
        let report = det.scan(text);
        let hit = report.hits.first().expect("hit");
        assert_eq!(hit.matched_hash.len(), 32);
        // Audit safety: the hit must not carry the raw match.
        let serialized = serde_json::to_string(&hit).expect("serialise");
        assert!(!serialized.contains("Treaty 7"));
        assert!(serialized.contains(&hit.matched_hash));
    }

    #[test]
    fn min_confidence_drops_weaker_hits() {
        let cfg = TribalDataDetectorConfig {
            min_confidence: OcapConfidence::Explicit,
            possible_implies_tribal: false,
        };
        let det = TribalDataDetector::new(cfg).unwrap();
        // "tribal elder" is Probable; "as told by elder" is Explicit.
        let report = det.scan("A tribal elder reviewed the document.");
        assert!(
            report
                .hits
                .iter()
                .all(|h| h.confidence == OcapConfidence::Explicit),
            "expected only explicit hits; got {:?}",
            report.hits,
        );
    }

    #[test]
    fn extra_patterns_can_be_added() {
        let det = TribalDataDetector::baseline()
            .with_extra_patterns(vec![(
                TribalIdentifierKind::Clan,
                OcapConfidence::Explicit,
                "test.local.clan_example".to_string(),
                r"(?i)\bMyExampleClan\b".to_string(),
            )])
            .unwrap();
        let report = det.scan("Members of MyExampleClan attended.");
        assert!(report.has_kind(TribalIdentifierKind::Clan));
    }

    #[test]
    fn invalid_extra_pattern_returns_error() {
        let r = TribalDataDetector::baseline().with_extra_patterns(vec![(
            TribalIdentifierKind::Clan,
            OcapConfidence::Possible,
            "test.invalid".to_string(),
            r"[".to_string(),
        )]);
        assert!(matches!(r, Err(TribalDataError::InvalidPattern { .. })));
    }

    #[test]
    fn hits_are_sorted_by_span_start() {
        let det = TribalDataDetector::baseline();
        let report =
            det.scan("First, the Sundance, then a First Nations meeting, then Treaty 7 work.");
        let starts: Vec<usize> = report.hits.iter().map(|h| h.span.0).collect();
        let mut sorted = starts.clone();
        sorted.sort_unstable();
        assert_eq!(starts, sorted);
    }

    #[test]
    fn confidence_order_matches_strictness() {
        assert!(OcapConfidence::Possible < OcapConfidence::Probable);
        assert!(OcapConfidence::Probable < OcapConfidence::Explicit);
    }
}
