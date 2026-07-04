//! ITAR classification.
//!
//! Implements detection of US Munitions List (USML) categories I-XXI and
//! assigns an [`ExportClassification`] level (`Uncontrolled` / `Ear99` /
//! `Ccl` / `Itar`). Detection is pattern-based: each USML category ships
//! with a small set of conservative controlled-term patterns derived from
//! the public USML text in 22 CFR § 121.1. Operators extend the catalog
//! via `config/compliance/itar.toml`.
//!
//! The classifier is conservative: any Probable or Explicit USML hit
//! promotes the query to [`ExportClassification::Itar`]. Ambiguous
//! (Possible-only) content is promoted to `Itar` when
//! [`ItarDetectorConfig::default_to_itar_on_ambiguity`] is `true` (the
//! default), satisfying the most-restrictive rule. The
//! [`jurisdiction`](crate::jurisdiction) module wraps this output and
//! applies country-based routing rules.
//!
//! Audit safety: every matched substring is hashed (Blake3, 32 hex
//! chars). The raw match is never retained on the [`UsmlHit`] so audit
//! logs cannot leak controlled technical text.

use std::collections::HashMap;

use regex::Regex;
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// The 21 USML categories defined in 22 CFR § 121.1.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UsmlCategory {
    /// Cat I — Firearms, Close Assault Weapons, Combat Shotguns.
    Firearms,
    /// Cat II — Guns and Armament.
    Armament,
    /// Cat III — Ammunition / Ordnance.
    Ammunition,
    /// Cat IV — Launch Vehicles, Guided Missiles, Ballistic Missiles,
    /// Rockets, Torpedoes, Bombs, and Mines.
    LaunchVehiclesAndMissiles,
    /// Cat V — Explosives and Energetic Materials, Propellants,
    /// Incendiary Agents.
    Explosives,
    /// Cat VI — Surface Vessels of War and Special Naval Equipment.
    NavalVessels,
    /// Cat VII — Ground Vehicles.
    GroundVehicles,
    /// Cat VIII — Aircraft and Related Articles.
    Aircraft,
    /// Cat IX — Military Training Equipment and Training.
    MilitaryTraining,
    /// Cat X — Personal Protective Equipment.
    PersonalProtective,
    /// Cat XI — Military Electronics.
    MilitaryElectronics,
    /// Cat XII — Fire Control, Laser, Imaging, and Guidance Equipment.
    FireControl,
    /// Cat XIII — Materials and Miscellaneous Articles.
    Materials,
    /// Cat XIV — Toxicological Agents (Chemical, Biological) and
    /// Associated Equipment.
    Toxicological,
    /// Cat XV — Spacecraft and Related Articles.
    Spacecraft,
    /// Cat XVI — Nuclear Weapons Related Articles.
    NuclearWeapons,
    /// Cat XVII — Classified Articles, Technical Data, Defense Services
    /// Not Otherwise Enumerated.
    Classified,
    /// Cat XVIII — Directed Energy Weapons.
    DirectedEnergy,
    /// Cat XIX — Gas Turbine Engines and Associated Equipment.
    GasTurbineEngines,
    /// Cat XX — Submersible Vessels and Related Articles.
    Submersibles,
    /// Cat XXI — Articles, Technical Data, and Defense Services Not
    /// Otherwise Enumerated.
    Miscellaneous,
}

impl UsmlCategory {
    /// Wire-format identifier for audit emission.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Firearms => "usml_i_firearms",
            Self::Armament => "usml_ii_armament",
            Self::Ammunition => "usml_iii_ammunition",
            Self::LaunchVehiclesAndMissiles => "usml_iv_launch_and_missiles",
            Self::Explosives => "usml_v_explosives",
            Self::NavalVessels => "usml_vi_naval_vessels",
            Self::GroundVehicles => "usml_vii_ground_vehicles",
            Self::Aircraft => "usml_viii_aircraft",
            Self::MilitaryTraining => "usml_ix_military_training",
            Self::PersonalProtective => "usml_x_personal_protective",
            Self::MilitaryElectronics => "usml_xi_military_electronics",
            Self::FireControl => "usml_xii_fire_control",
            Self::Materials => "usml_xiii_materials",
            Self::Toxicological => "usml_xiv_toxicological",
            Self::Spacecraft => "usml_xv_spacecraft",
            Self::NuclearWeapons => "usml_xvi_nuclear_weapons",
            Self::Classified => "usml_xvii_classified",
            Self::DirectedEnergy => "usml_xviii_directed_energy",
            Self::GasTurbineEngines => "usml_xix_gas_turbine_engines",
            Self::Submersibles => "usml_xx_submersibles",
            Self::Miscellaneous => "usml_xxi_miscellaneous",
        }
    }

    /// Roman-numeral category number ("I" through "XXI").
    pub fn roman(self) -> &'static str {
        match self {
            Self::Firearms => "I",
            Self::Armament => "II",
            Self::Ammunition => "III",
            Self::LaunchVehiclesAndMissiles => "IV",
            Self::Explosives => "V",
            Self::NavalVessels => "VI",
            Self::GroundVehicles => "VII",
            Self::Aircraft => "VIII",
            Self::MilitaryTraining => "IX",
            Self::PersonalProtective => "X",
            Self::MilitaryElectronics => "XI",
            Self::FireControl => "XII",
            Self::Materials => "XIII",
            Self::Toxicological => "XIV",
            Self::Spacecraft => "XV",
            Self::NuclearWeapons => "XVI",
            Self::Classified => "XVII",
            Self::DirectedEnergy => "XVIII",
            Self::GasTurbineEngines => "XIX",
            Self::Submersibles => "XX",
            Self::Miscellaneous => "XXI",
        }
    }
}

/// Export-control classification level.
///
/// Variants are ordered least- to most-restrictive so `>=` can gate.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum ExportClassification {
    /// No export control concern detected.
    #[default]
    Uncontrolled,
    /// EAR99 — subject to EAR but no specific ECCN.
    Ear99,
    /// CCL — listed on the Commerce Control List with an ECCN.
    Ccl,
    /// USML — listed on the United States Munitions List (ITAR).
    Itar,
}

impl ExportClassification {
    /// Wire-format identifier.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Uncontrolled => "uncontrolled",
            Self::Ear99 => "ear99",
            Self::Ccl => "ccl",
            Self::Itar => "itar",
        }
    }
}

/// Detection confidence tier for an ITAR hit.
///
/// Variants are declared weakest-to-strongest so derived `Ord` gives
/// `Possible < Probable < Explicit`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ItarConfidence {
    /// Weak signal — keyword overlap with controlled terms.
    Possible,
    /// Pattern is likely controlled in a defense context but ambiguous.
    Probable,
    /// Pattern is highly specific to controlled technical data.
    Explicit,
}

/// One detected USML category hit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct UsmlHit {
    /// Which USML category matched.
    pub category: UsmlCategory,
    /// Confidence tier of this match.
    pub confidence: ItarConfidence,
    /// Byte span `(start, end_exclusive)` in the original text.
    pub span: (usize, usize),
    /// Blake3 hash (first 32 hex chars) of the matched substring. Audit
    /// logs must never store the raw text.
    pub matched_hash: String,
}

/// Aggregate of every USML hit plus the derived export classification.
#[derive(Debug, Clone, Default, Serialize)]
pub struct ItarReport {
    /// Final export classification level after applying confidence rules.
    pub classification: ExportClassification,
    /// All hits in order of occurrence.
    pub hits: Vec<UsmlHit>,
    /// Per-category counts.
    pub by_category: HashMap<UsmlCategory, usize>,
    /// Highest confidence observed.
    pub highest_confidence: Option<ItarConfidence>,
    /// True when the report fell through to `Itar` by the ambiguity rule
    /// rather than from a Probable/Explicit hit.
    pub defaulted_to_itar_on_ambiguity: bool,
}

impl ItarReport {
    /// True when the report has any USML hit.
    pub fn has_any(&self) -> bool {
        !self.hits.is_empty()
    }

    /// True when classification is `Itar`.
    pub fn has_itar(&self) -> bool {
        self.classification == ExportClassification::Itar
    }

    /// Total hits across all categories.
    pub fn total_hits(&self) -> usize {
        self.hits.len()
    }
}

/// ITAR detection errors (build-time only).
#[derive(Debug, Error)]
pub enum ItarError {
    /// A pattern in the catalog failed to compile.
    #[error("invalid ITAR pattern for {category:?}: {source}")]
    InvalidPattern {
        /// Category whose pattern failed.
        category: UsmlCategory,
        /// Underlying regex error.
        source: regex::Error,
    },
}

/// Detector tunables.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ItarDetectorConfig {
    /// Lowest confidence to surface in the report. Defaults to
    /// `Possible` (most coverage).
    #[serde(default = "default_min_confidence")]
    pub min_confidence: ItarConfidence,
    /// When true and ALL hits are Possible-only, the classification is
    /// promoted to `Itar` per the most-restrictive rule. Defaults to
    /// `true`.
    #[serde(default = "default_default_to_itar")]
    pub default_to_itar_on_ambiguity: bool,
}

fn default_min_confidence() -> ItarConfidence {
    ItarConfidence::Possible
}

fn default_default_to_itar() -> bool {
    true
}

impl Default for ItarDetectorConfig {
    fn default() -> Self {
        Self {
            min_confidence: default_min_confidence(),
            default_to_itar_on_ambiguity: default_default_to_itar(),
        }
    }
}

struct CompiledPattern {
    category: UsmlCategory,
    confidence: ItarConfidence,
    regex: Regex,
}

/// ITAR / USML detector. Build once at startup, reuse across requests.
pub struct ItarDetector {
    config: ItarDetectorConfig,
    patterns: Vec<CompiledPattern>,
}

impl ItarDetector {
    /// Build a detector with a custom config.
    pub fn new(config: ItarDetectorConfig) -> Result<Self, ItarError> {
        let raw = baseline_patterns();
        let mut compiled = Vec::with_capacity(raw.len());
        for (category, confidence, pattern) in raw {
            let regex = Regex::new(pattern)
                .map_err(|source| ItarError::InvalidPattern { category, source })?;
            compiled.push(CompiledPattern {
                category,
                confidence,
                regex,
            });
        }
        Ok(Self {
            config,
            patterns: compiled,
        })
    }

    /// Default detector with the baseline pattern catalog.
    pub fn baseline() -> Self {
        Self::new(ItarDetectorConfig::default()).expect("baseline ITAR patterns must compile")
    }

    /// Scan a query and produce an [`ItarReport`].
    pub fn scan(&self, text: &str) -> ItarReport {
        let mut hits: Vec<UsmlHit> = Vec::new();
        let mut by_category: HashMap<UsmlCategory, usize> = HashMap::new();
        let mut highest_confidence: Option<ItarConfidence> = None;

        for pattern in &self.patterns {
            if pattern.confidence < self.config.min_confidence {
                continue;
            }
            for found in pattern.regex.find_iter(text) {
                hits.push(UsmlHit {
                    category: pattern.category,
                    confidence: pattern.confidence,
                    span: (found.start(), found.end()),
                    matched_hash: hash_match(found.as_str()),
                });
                *by_category.entry(pattern.category).or_insert(0) += 1;
                highest_confidence = Some(match highest_confidence {
                    Some(prev) => prev.max(pattern.confidence),
                    None => pattern.confidence,
                });
            }
        }
        hits.sort_by_key(|h| h.span.0);

        let (classification, defaulted) =
            derive_classification(highest_confidence, self.config.default_to_itar_on_ambiguity);

        ItarReport {
            classification,
            hits,
            by_category,
            highest_confidence,
            defaulted_to_itar_on_ambiguity: defaulted,
        }
    }
}

/// Classification rule:
///   - No hits → Uncontrolled
///   - Any Probable/Explicit hit → Itar
///   - Only Possible hits → Itar if `default_to_itar_on_ambiguity` else
///     Uncontrolled
fn derive_classification(
    highest: Option<ItarConfidence>,
    default_on_ambiguity: bool,
) -> (ExportClassification, bool) {
    match highest {
        None => (ExportClassification::Uncontrolled, false),
        Some(ItarConfidence::Explicit) | Some(ItarConfidence::Probable) => {
            (ExportClassification::Itar, false)
        }
        Some(ItarConfidence::Possible) => {
            if default_on_ambiguity {
                (ExportClassification::Itar, true)
            } else {
                (ExportClassification::Uncontrolled, false)
            }
        }
    }
}

fn hash_match(s: &str) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(s.as_bytes());
    hasher.finalize().to_hex().to_string()[..32].to_string()
}

/// Baseline USML pattern catalog. Tuples are `(category, confidence, pattern)`.
///
/// The shipped patterns are intentionally narrow. Operators extend the
/// catalog through `config/compliance/itar.toml`. Each pattern matches
/// case-insensitively where appropriate using `(?i)`.
fn baseline_patterns() -> Vec<(UsmlCategory, ItarConfidence, &'static str)> {
    use ItarConfidence::*;
    use UsmlCategory::*;
    vec![
        // Cat I — Firearms.
        (
            Firearms,
            Explicit,
            r"(?i)\b(?:machine\s+gun|automatic\s+rifle|combat\s+shotgun|sniper\s+rifle)\b",
        ),
        (
            Firearms,
            Probable,
            r"(?i)\b(?:M4\s+carbine|AR-15\s+select-fire)\b",
        ),
        // Cat II — Guns and Armament.
        (
            Armament,
            Explicit,
            r"(?i)\b(?:howitzer|mortar|naval\s+gun|tank\s+gun|cannon\s+105mm)\b",
        ),
        // Cat III — Ammunition.
        (
            Ammunition,
            Probable,
            r"(?i)\b(?:armor[\s-]piercing|tracer\s+round|depleted\s+uranium\s+round)\b",
        ),
        // Cat IV — Launch vehicles and missiles.
        (
            LaunchVehiclesAndMissiles,
            Explicit,
            r"(?i)\b(?:ballistic\s+missile|cruise\s+missile|ICBM|guided\s+missile|MIRV)\b",
        ),
        (
            LaunchVehiclesAndMissiles,
            Probable,
            r"(?i)\b(?:warhead|torpedo|naval\s+mine|guidance\s+kit)\b",
        ),
        // Cat V — Explosives.
        (
            Explosives,
            Explicit,
            r"(?i)\b(?:RDX|HMX|PETN|composition\s+C-4|TATP)\b",
        ),
        (
            Explosives,
            Probable,
            r"(?i)\b(?:plastic\s+explosive|shaped\s+charge|incendiary\s+agent)\b",
        ),
        // Cat VI — Naval vessels.
        (
            NavalVessels,
            Explicit,
            r"(?i)\b(?:aircraft\s+carrier|destroyer|frigate|naval\s+combat\s+system)\b",
        ),
        // Cat VII — Ground vehicles.
        (
            GroundVehicles,
            Explicit,
            r"(?i)\b(?:main\s+battle\s+tank|Abrams\s+M1|Bradley\s+IFV|armored\s+personnel\s+carrier)\b",
        ),
        // Cat VIII — Aircraft.
        (
            Aircraft,
            Explicit,
            r"(?i)\b(?:F-22|F-35|B-2\s+bomber|stealth\s+aircraft|attack\s+helicopter)\b",
        ),
        (
            Aircraft,
            Probable,
            r"(?i)\b(?:military\s+aircraft|combat\s+aircraft|UAV\s+military|drone\s+strike)\b",
        ),
        // Cat IX — Military training.
        (
            MilitaryTraining,
            Possible,
            r"(?i)\b(?:combat\s+simulator|military\s+training\s+system|war\s+game\s+system)\b",
        ),
        // Cat X — Personal protective.
        (
            PersonalProtective,
            Probable,
            r"(?i)\b(?:body\s+armor\s+level\s+IV|ballistic\s+plate|combat\s+helmet)\b",
        ),
        // Cat XI — Military electronics.
        (
            MilitaryElectronics,
            Explicit,
            r"(?i)\b(?:AESA\s+radar|electronic\s+warfare\s+suite|SIGINT\s+receiver|TEMPEST\s+shielding)\b",
        ),
        (
            MilitaryElectronics,
            Probable,
            r"(?i)\b(?:phased[\s-]array\s+radar|military\s+jammer|crypto\s+module\s+Type\s+1)\b",
        ),
        // Cat XII — Fire control / laser / guidance.
        (
            FireControl,
            Explicit,
            r"(?i)\b(?:fire\s+control\s+system|laser\s+designator|inertial\s+navigation\s+unit|IMU\s+military)\b",
        ),
        (
            FireControl,
            Probable,
            r"(?i)\b(?:night[\s-]vision\s+military|thermal\s+weapon\s+sight)\b",
        ),
        // Cat XIII — Materials.
        (
            Materials,
            Probable,
            r"(?i)\b(?:maraging\s+steel|radar[\s-]absorbing\s+material|stealth\s+coating)\b",
        ),
        // Cat XIV — Toxicological.
        (
            Toxicological,
            Explicit,
            r"(?i)\b(?:VX\s+nerve\s+agent|sarin|mustard\s+gas|anthrax\s+weaponized|chemical\s+weapon)\b",
        ),
        // Cat XV — Spacecraft.
        (
            Spacecraft,
            Explicit,
            r"(?i)\b(?:military\s+satellite|SIGINT\s+satellite|missile\s+warning\s+satellite|spacecraft\s+propulsion\s+military)\b",
        ),
        (
            Spacecraft,
            Probable,
            r"(?i)\b(?:satellite\s+bus\s+military|launch\s+vehicle\s+upper\s+stage)\b",
        ),
        // Cat XVI — Nuclear weapons.
        (
            NuclearWeapons,
            Explicit,
            r"(?i)\b(?:nuclear\s+warhead|fissile\s+pit|thermonuclear\s+device|implosion\s+lens)\b",
        ),
        // Cat XVII — Classified.
        (
            Classified,
            Probable,
            r"(?i)\b(?:classified\s+defense\s+article|SAP\s+program|special\s+access\s+program)\b",
        ),
        // Cat XVIII — Directed energy.
        (
            DirectedEnergy,
            Explicit,
            r"(?i)\b(?:directed\s+energy\s+weapon|high[\s-]energy\s+laser\s+weapon|HEL\s+weapon|microwave\s+weapon)\b",
        ),
        // Cat XIX — Gas turbine engines (military).
        (
            GasTurbineEngines,
            Probable,
            r"(?i)\b(?:F119\s+engine|F135\s+engine|military\s+turbofan|afterburner\s+module)\b",
        ),
        // Cat XX — Submersibles.
        (
            Submersibles,
            Explicit,
            r"(?i)\b(?:nuclear\s+submarine|attack\s+submarine|submarine\s+launched\s+missile|SLBM)\b",
        ),
        // Cat XXI — Miscellaneous controlled / catchall.
        (
            Miscellaneous,
            Possible,
            r"(?i)\b(?:USML|ITAR[\s-]controlled|defense\s+article)\b",
        ),
        // Generic technical-data indicators across categories.
        (
            Miscellaneous,
            Possible,
            r"(?i)\b(?:technical\s+data\s+package|defense\s+services|controlled\s+technology)\b",
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn det() -> ItarDetector {
        ItarDetector::baseline()
    }

    #[test]
    fn test_neutral_text_uncontrolled() {
        let r = det().scan("Tell me about the history of rainfall.");
        assert_eq!(r.classification, ExportClassification::Uncontrolled);
        assert!(!r.has_any());
    }

    #[test]
    fn test_missile_text_classified_itar() {
        let r = det().scan("Design parameters for an ICBM guided missile warhead.");
        assert!(r.has_itar());
        assert!(
            r.by_category
                .contains_key(&UsmlCategory::LaunchVehiclesAndMissiles)
        );
    }

    #[test]
    fn test_explosives_explicit() {
        let r = det().scan("Synthesis route for RDX and HMX.");
        assert_eq!(r.classification, ExportClassification::Itar);
        assert!(r.by_category.contains_key(&UsmlCategory::Explosives));
        assert_eq!(r.highest_confidence, Some(ItarConfidence::Explicit));
    }

    #[test]
    fn test_aircraft_f35_explicit() {
        let r = det().scan("Compare the F-22 and F-35 stealth profiles.");
        assert_eq!(r.classification, ExportClassification::Itar);
        assert!(r.by_category.contains_key(&UsmlCategory::Aircraft));
    }

    #[test]
    fn test_nuclear_warhead_explicit() {
        let r = det().scan("Implosion lens geometry for a nuclear warhead.");
        assert_eq!(r.classification, ExportClassification::Itar);
        assert!(r.by_category.contains_key(&UsmlCategory::NuclearWeapons));
    }

    #[test]
    fn test_chemical_weapon_explicit() {
        let r = det().scan("Production pathway for VX nerve agent.");
        assert_eq!(r.classification, ExportClassification::Itar);
        assert!(r.by_category.contains_key(&UsmlCategory::Toxicological));
    }

    #[test]
    fn test_directed_energy_explicit() {
        let r = det().scan("Beam profile of a high-energy laser weapon.");
        assert_eq!(r.classification, ExportClassification::Itar);
        assert!(r.by_category.contains_key(&UsmlCategory::DirectedEnergy));
    }

    #[test]
    fn test_submarine_explicit() {
        let r = det().scan("Acoustic signature of an attack submarine.");
        assert_eq!(r.classification, ExportClassification::Itar);
        assert!(r.by_category.contains_key(&UsmlCategory::Submersibles));
    }

    #[test]
    fn test_ambiguous_possible_defaults_to_itar() {
        let r = det().scan("This appears to be a defense article reference.");
        assert_eq!(r.classification, ExportClassification::Itar);
        assert!(r.defaulted_to_itar_on_ambiguity);
        assert_eq!(r.highest_confidence, Some(ItarConfidence::Possible));
    }

    #[test]
    fn test_ambiguous_can_be_disabled() {
        let cfg = ItarDetectorConfig {
            min_confidence: ItarConfidence::Possible,
            default_to_itar_on_ambiguity: false,
        };
        let d = ItarDetector::new(cfg).unwrap();
        let r = d.scan("This appears to be a defense article reference.");
        assert_eq!(r.classification, ExportClassification::Uncontrolled);
        assert!(!r.defaulted_to_itar_on_ambiguity);
    }

    #[test]
    fn test_min_confidence_filter_drops_possible() {
        let cfg = ItarDetectorConfig {
            min_confidence: ItarConfidence::Probable,
            default_to_itar_on_ambiguity: true,
        };
        let d = ItarDetector::new(cfg).unwrap();
        let r = d.scan("Defense article note only.");
        // Only Possible-tier hits exist for this text → filtered out.
        assert!(!r.has_any());
        assert_eq!(r.classification, ExportClassification::Uncontrolled);
    }

    #[test]
    fn test_matched_text_is_hashed_never_raw() {
        let r = det().scan("Procurement of RDX charges.");
        let hit = r.hits.first().expect("expected at least one hit");
        assert_eq!(hit.matched_hash.len(), 32);
        assert_ne!(hit.matched_hash.to_lowercase(), "rdx");
    }

    #[test]
    fn test_hits_sorted_by_span() {
        let r = det().scan("RDX then F-22 then VX nerve agent details.");
        for pair in r.hits.windows(2) {
            assert!(pair[0].span.0 <= pair[1].span.0);
        }
    }

    #[test]
    fn test_roman_numeral_lookup() {
        assert_eq!(UsmlCategory::Firearms.roman(), "I");
        assert_eq!(UsmlCategory::NuclearWeapons.roman(), "XVI");
        assert_eq!(UsmlCategory::Miscellaneous.roman(), "XXI");
    }

    #[test]
    fn test_export_classification_ordering() {
        assert!(ExportClassification::Itar > ExportClassification::Ccl);
        assert!(ExportClassification::Ccl > ExportClassification::Ear99);
        assert!(ExportClassification::Ear99 > ExportClassification::Uncontrolled);
    }
}
