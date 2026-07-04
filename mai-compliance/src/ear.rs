//! EAR / Commerce Control List classification.
//!
//! Implements detection of Export Administration Regulations (EAR)
//! controlled content. Two kinds of signal are extracted:
//!
//! - **ECCN matches**: Export Control Classification Numbers shaped
//!   `<digit><letter A-E><digit><digit><digit>` (e.g. `5A002`, `3D001`).
//!   A direct ECCN reference is the strongest signal.
//! - **CCL category keywords**: textual indicators that map a query to
//!   one of the 10 CCL categories (0–9) without a specific ECCN.
//!
//! Output is an [`EarReport`] with a derived [`EarClassification`]
//! (`Uncontrolled` / `Ear99` / `Ccl`). The neighbouring
//! [`itar`](crate::itar) module handles USML/ITAR; the
//! [`jurisdiction`](crate::jurisdiction) module composes both and
//! resolves overall export-control posture.
//!
//! License exceptions (ENC, TSU, GBS, …) and de minimis content
//! percentages are extracted as informational signals attached to the
//! report. Final routing decisions belong to the jurisdiction module.

use std::collections::HashMap;

use regex::Regex;
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// The ten CCL categories defined in 15 CFR § 738 Supplement No. 1.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CclCategory {
    /// 0 — Nuclear materials, facilities, equipment.
    Nuclear,
    /// 1 — Materials, chemicals, microorganisms, toxins.
    MaterialsChemBio,
    /// 2 — Materials processing.
    MaterialsProcessing,
    /// 3 — Electronics.
    Electronics,
    /// 4 — Computers.
    Computers,
    /// 5 — Telecommunications and information security.
    TelecomCrypto,
    /// 6 — Sensors and lasers.
    SensorsAndLasers,
    /// 7 — Navigation and avionics.
    NavigationAvionics,
    /// 8 — Marine.
    Marine,
    /// 9 — Aerospace and propulsion.
    AerospacePropulsion,
}

impl CclCategory {
    /// Wire-format identifier for audit emission.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Nuclear => "ccl_0_nuclear",
            Self::MaterialsChemBio => "ccl_1_materials_chem_bio",
            Self::MaterialsProcessing => "ccl_2_materials_processing",
            Self::Electronics => "ccl_3_electronics",
            Self::Computers => "ccl_4_computers",
            Self::TelecomCrypto => "ccl_5_telecom_crypto",
            Self::SensorsAndLasers => "ccl_6_sensors_lasers",
            Self::NavigationAvionics => "ccl_7_navigation_avionics",
            Self::Marine => "ccl_8_marine",
            Self::AerospacePropulsion => "ccl_9_aerospace_propulsion",
        }
    }

    /// Numeric category digit (0–9).
    pub fn digit(self) -> char {
        match self {
            Self::Nuclear => '0',
            Self::MaterialsChemBio => '1',
            Self::MaterialsProcessing => '2',
            Self::Electronics => '3',
            Self::Computers => '4',
            Self::TelecomCrypto => '5',
            Self::SensorsAndLasers => '6',
            Self::NavigationAvionics => '7',
            Self::Marine => '8',
            Self::AerospacePropulsion => '9',
        }
    }

    /// Parse the leading digit of an ECCN to a CCL category.
    pub fn from_digit(d: char) -> Option<Self> {
        match d {
            '0' => Some(Self::Nuclear),
            '1' => Some(Self::MaterialsChemBio),
            '2' => Some(Self::MaterialsProcessing),
            '3' => Some(Self::Electronics),
            '4' => Some(Self::Computers),
            '5' => Some(Self::TelecomCrypto),
            '6' => Some(Self::SensorsAndLasers),
            '7' => Some(Self::NavigationAvionics),
            '8' => Some(Self::Marine),
            '9' => Some(Self::AerospacePropulsion),
            _ => None,
        }
    }
}

/// CCL product group (the second character of an ECCN).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProductGroup {
    /// A — Equipment, assemblies, components.
    Equipment,
    /// B — Test, inspection, production equipment.
    TestEquipment,
    /// C — Materials.
    Materials,
    /// D — Software.
    Software,
    /// E — Technology.
    Technology,
}

impl ProductGroup {
    /// Parse the group letter to an enum.
    pub fn from_letter(c: char) -> Option<Self> {
        match c.to_ascii_uppercase() {
            'A' => Some(Self::Equipment),
            'B' => Some(Self::TestEquipment),
            'C' => Some(Self::Materials),
            'D' => Some(Self::Software),
            'E' => Some(Self::Technology),
            _ => None,
        }
    }

    /// Single-character wire format.
    pub fn letter(self) -> char {
        match self {
            Self::Equipment => 'A',
            Self::TestEquipment => 'B',
            Self::Materials => 'C',
            Self::Software => 'D',
            Self::Technology => 'E',
        }
    }
}

/// EAR-derived classification level. Ordered least → most restrictive.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum EarClassification {
    /// No EAR concern detected.
    #[default]
    Uncontrolled,
    /// EAR99 — subject to EAR but no specific ECCN.
    Ear99,
    /// CCL — direct ECCN or strong category-keyword match.
    Ccl,
}

impl EarClassification {
    /// Wire-format identifier.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Uncontrolled => "uncontrolled",
            Self::Ear99 => "ear99",
            Self::Ccl => "ccl",
        }
    }
}

/// One parsed ECCN.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EccnHit {
    /// Raw ECCN string (uppercased), e.g. `"5A002"`.
    pub eccn: String,
    /// CCL category derived from the leading digit.
    pub category: CclCategory,
    /// Product group derived from the second character.
    pub group: ProductGroup,
    /// Byte span `(start, end_exclusive)` in the original text.
    pub span: (usize, usize),
}

/// One CCL category keyword hit (no specific ECCN).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CclKeywordHit {
    /// CCL category matched by the keyword.
    pub category: CclCategory,
    /// Byte span `(start, end_exclusive)`.
    pub span: (usize, usize),
    /// Blake3 hash (first 32 hex) of the matched substring.
    pub matched_hash: String,
}

/// A detected license exception (informational only).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LicenseException {
    /// Exception identifier (e.g. `"ENC"`, `"TSU"`, `"GBS"`).
    pub code: String,
    /// Byte span.
    pub span: (usize, usize),
}

/// De-minimis-style content percentage extracted from text (e.g.
/// "less than 25% controlled content").
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DeMinimisIndicator {
    /// Reported percentage value (0.0 – 100.0).
    pub percent: f64,
    /// True when the percentage is below the standard 25% threshold
    /// (10% for some country groups). Decision-makers should still
    /// apply the deployment-specific rule.
    pub below_default_threshold: bool,
    /// Byte span.
    pub span: (usize, usize),
}

/// Aggregate EAR analysis of a query.
#[derive(Debug, Clone, Default, Serialize)]
pub struct EarReport {
    /// Final EAR classification level.
    pub classification: EarClassification,
    /// Parsed ECCN hits.
    pub eccn_hits: Vec<EccnHit>,
    /// Keyword-only CCL hits.
    pub keyword_hits: Vec<CclKeywordHit>,
    /// Detected license exceptions.
    pub license_exceptions: Vec<LicenseException>,
    /// De minimis indicators extracted from text.
    pub de_minimis: Vec<DeMinimisIndicator>,
    /// Hit counts per CCL category (combined across ECCN + keyword).
    pub by_category: HashMap<CclCategory, usize>,
}

impl EarReport {
    /// True when any EAR signal was detected.
    pub fn has_any(&self) -> bool {
        !(self.eccn_hits.is_empty() && self.keyword_hits.is_empty())
    }

    /// True when classification is `Ccl`.
    pub fn has_ccl(&self) -> bool {
        self.classification == EarClassification::Ccl
    }

    /// True when at least one license exception was extracted.
    pub fn has_license_exception(&self) -> bool {
        !self.license_exceptions.is_empty()
    }

    /// True when at least one de minimis indicator is below the default
    /// 25% threshold.
    pub fn de_minimis_below_threshold(&self) -> bool {
        self.de_minimis.iter().any(|d| d.below_default_threshold)
    }
}

/// EAR detection errors (build-time only).
#[derive(Debug, Error)]
pub enum EarError {
    /// A keyword pattern failed to compile.
    #[error("invalid EAR pattern for {category:?}: {source}")]
    InvalidPattern {
        category: CclCategory,
        source: regex::Error,
    },
    /// The ECCN scan regex failed to compile.
    #[error("ECCN scan regex failed: {0}")]
    InvalidEccnRegex(#[from] regex::Error),
}

/// Tunables.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EarDetectorConfig {
    /// De minimis threshold (percent). Defaults to 25.0%.
    #[serde(default = "default_threshold")]
    pub de_minimis_threshold: f64,
    /// When true and no specific ECCN or keyword matches but the text
    /// references export control generically, classify as `Ear99`.
    /// Defaults to `true`.
    #[serde(default = "default_ear99_on_generic")]
    pub ear99_on_generic_reference: bool,
}

fn default_threshold() -> f64 {
    25.0
}

fn default_ear99_on_generic() -> bool {
    true
}

impl Default for EarDetectorConfig {
    fn default() -> Self {
        Self {
            de_minimis_threshold: default_threshold(),
            ear99_on_generic_reference: default_ear99_on_generic(),
        }
    }
}

struct CompiledKeyword {
    category: CclCategory,
    regex: Regex,
}

/// EAR / CCL detector.
pub struct EarDetector {
    config: EarDetectorConfig,
    eccn_regex: Regex,
    license_regex: Regex,
    de_minimis_regex: Regex,
    generic_regex: Regex,
    keywords: Vec<CompiledKeyword>,
}

impl EarDetector {
    /// Build a detector with custom config.
    pub fn new(config: EarDetectorConfig) -> Result<Self, EarError> {
        // ECCN shape: digit, A-E, three digits, optional .alphanumeric.
        // Word boundary requires the leading digit and trailing alphanum
        // be surrounded by non-word characters.
        let eccn_regex = Regex::new(r"\b([0-9])([A-Ea-e])([0-9]{3})(?:\.[A-Za-z0-9]{1,4})?\b")?;
        let license_regex = Regex::new(
            r"(?i)\blicense\s+exception\s+(ENC|TSU|GBS|CIV|GOV|TMP|RPL|BAG|AVS|APR|STA|GFT|TSR)\b",
        )?;
        let de_minimis_regex = Regex::new(
            r"(?i)\b(?:less\s+than|under|below|at\s+most|<=?)\s*([0-9]{1,3}(?:\.[0-9]+)?)\s*%(?:\s+(?:controlled|US|U\.S\.))?",
        )?;
        let generic_regex = Regex::new(
            r"(?i)\b(?:EAR(?:99|\s+controlled)|export\s+control(?:led)?|Commerce\s+Control\s+List|dual[\s-]use\s+technology)\b",
        )?;

        let raw_keywords = baseline_keywords();
        let mut keywords = Vec::with_capacity(raw_keywords.len());
        for (category, pattern) in raw_keywords {
            let regex = Regex::new(pattern)
                .map_err(|source| EarError::InvalidPattern { category, source })?;
            keywords.push(CompiledKeyword { category, regex });
        }

        Ok(Self {
            config,
            eccn_regex,
            license_regex,
            de_minimis_regex,
            generic_regex,
            keywords,
        })
    }

    /// Default detector with baseline catalog.
    pub fn baseline() -> Self {
        Self::new(EarDetectorConfig::default()).expect("baseline EAR patterns must compile")
    }

    /// Scan a query and produce an [`EarReport`].
    pub fn scan(&self, text: &str) -> EarReport {
        let mut report = EarReport::default();

        // 1. ECCNs.
        for caps in self.eccn_regex.captures_iter(text) {
            let whole = caps.get(0).unwrap();
            let cat_char = caps[1].chars().next().unwrap();
            let group_char = caps[2].chars().next().unwrap();
            let (Some(category), Some(group)) = (
                CclCategory::from_digit(cat_char),
                ProductGroup::from_letter(group_char),
            ) else {
                continue;
            };
            report.eccn_hits.push(EccnHit {
                eccn: whole.as_str().to_uppercase(),
                category,
                group,
                span: (whole.start(), whole.end()),
            });
            *report.by_category.entry(category).or_insert(0) += 1;
        }

        // 2. CCL keyword hits.
        for keyword in &self.keywords {
            for found in keyword.regex.find_iter(text) {
                report.keyword_hits.push(CclKeywordHit {
                    category: keyword.category,
                    span: (found.start(), found.end()),
                    matched_hash: hash_match(found.as_str()),
                });
                *report.by_category.entry(keyword.category).or_insert(0) += 1;
            }
        }

        // 3. License exceptions.
        for caps in self.license_regex.captures_iter(text) {
            let whole = caps.get(0).unwrap();
            let code = caps[1].to_uppercase();
            report.license_exceptions.push(LicenseException {
                code,
                span: (whole.start(), whole.end()),
            });
        }

        // 4. De minimis indicators.
        for caps in self.de_minimis_regex.captures_iter(text) {
            let whole = caps.get(0).unwrap();
            let Ok(percent) = caps[1].parse::<f64>() else {
                continue;
            };
            report.de_minimis.push(DeMinimisIndicator {
                percent,
                below_default_threshold: percent < self.config.de_minimis_threshold,
                span: (whole.start(), whole.end()),
            });
        }

        // 5. Classification.
        report.classification = if !report.eccn_hits.is_empty() || !report.keyword_hits.is_empty() {
            EarClassification::Ccl
        } else if self.config.ear99_on_generic_reference && self.generic_regex.is_match(text) {
            EarClassification::Ear99
        } else {
            EarClassification::Uncontrolled
        };

        // 6. Sort hits by start span for deterministic audit output.
        report.eccn_hits.sort_by_key(|h| h.span.0);
        report.keyword_hits.sort_by_key(|h| h.span.0);
        report.license_exceptions.sort_by_key(|h| h.span.0);
        report.de_minimis.sort_by_key(|h| h.span.0);

        report
    }
}

fn hash_match(s: &str) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(s.as_bytes());
    hasher.finalize().to_hex().to_string()[..32].to_string()
}

/// Baseline CCL keyword catalog. Tuples are `(category, pattern)`. Each
/// pattern is matched case-insensitively via the `(?i)` prefix. Operators
/// extend via `config/compliance/ear.toml`.
fn baseline_keywords() -> Vec<(CclCategory, &'static str)> {
    use CclCategory::*;
    vec![
        // 0 — Nuclear.
        (
            Nuclear,
            r"(?i)\b(?:uranium\s+enrichment|plutonium\s+reprocessing|nuclear\s+reactor\s+design|heavy\s+water\s+production)\b",
        ),
        // 1 — Materials, chemicals, microorganisms, toxins.
        (
            MaterialsChemBio,
            r"(?i)\b(?:CW\s+precursor|biological\s+agent|select\s+agent|toxin\s+production)\b",
        ),
        // 2 — Materials processing.
        (
            MaterialsProcessing,
            r"(?i)\b(?:isostatic\s+press|robotic\s+machining\s+center|composite\s+filament[\s-]winding)\b",
        ),
        // 3 — Electronics.
        (
            Electronics,
            r"(?i)\b(?:radiation[\s-]hardened\s+chip|rad[\s-]hard\s+ASIC|MMIC\s+amplifier|GaN\s+power\s+device)\b",
        ),
        // 4 — Computers.
        (
            Computers,
            r"(?i)\b(?:HPC\s+cluster\s+military|supercomputer\s+\d+\s*petaflops|peak\s+performance\s+\d+\s*PetaFLOPs)\b",
        ),
        // 5 — Telecom and information security.
        (
            TelecomCrypto,
            r"(?i)\b(?:strong\s+cryptography|AES[\s-]256\s+module|public[\s-]key\s+infrastructure\s+military|quantum[\s-]safe\s+VPN)\b",
        ),
        // 6 — Sensors and lasers.
        (
            SensorsAndLasers,
            r"(?i)\b(?:infrared\s+focal[\s-]plane\s+array|LIDAR\s+military|hyperspectral\s+sensor|high[\s-]power\s+laser\s+industrial)\b",
        ),
        // 7 — Navigation and avionics.
        (
            NavigationAvionics,
            r"(?i)\b(?:GPS[\s-]denied\s+navigation|inertial\s+measurement\s+unit|fiber[\s-]optic\s+gyroscope|ring[\s-]laser\s+gyro)\b",
        ),
        // 8 — Marine.
        (
            Marine,
            r"(?i)\b(?:sonar\s+towed\s+array|underwater\s+vehicle\s+autonomous|submarine\s+communications\s+buoy)\b",
        ),
        // 9 — Aerospace and propulsion.
        (
            AerospacePropulsion,
            r"(?i)\b(?:hypersonic\s+vehicle|scramjet|solid\s+rocket\s+motor|liquid\s+propellant\s+engine|UAV\s+endurance)\b",
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn det() -> EarDetector {
        EarDetector::baseline()
    }

    #[test]
    fn test_neutral_text_uncontrolled() {
        let r = det().scan("Tell me about ocean currents.");
        assert_eq!(r.classification, EarClassification::Uncontrolled);
        assert!(!r.has_any());
    }

    #[test]
    fn test_eccn_5a002_parses_to_telecom_crypto() {
        let r = det().scan("Item classified as ECCN 5A002 strong crypto module.");
        assert!(r.has_ccl());
        assert_eq!(r.eccn_hits.len(), 1);
        let hit = &r.eccn_hits[0];
        assert_eq!(hit.eccn, "5A002");
        assert_eq!(hit.category, CclCategory::TelecomCrypto);
        assert_eq!(hit.group, ProductGroup::Equipment);
    }

    #[test]
    fn test_eccn_3a001_parses_to_electronics() {
        let r = det().scan("Rad-hard ASIC under 3A001.b.2.");
        assert_eq!(r.classification, EarClassification::Ccl);
        let hit = r.eccn_hits.iter().find(|h| h.eccn.starts_with("3A001"));
        assert!(hit.is_some());
        assert_eq!(hit.unwrap().category, CclCategory::Electronics);
    }

    #[test]
    fn test_eccn_software_group_d() {
        let r = det().scan("Software 5D002 controlled cryptographic toolkit.");
        let hit = r.eccn_hits.iter().find(|h| h.eccn == "5D002");
        assert!(hit.is_some());
        assert_eq!(hit.unwrap().group, ProductGroup::Software);
    }

    #[test]
    fn test_eccn_technology_group_e() {
        let r = det().scan("Tech 9E003 covers gas turbine technology.");
        let hit = r.eccn_hits.iter().find(|h| h.eccn == "9E003");
        assert!(hit.is_some());
        assert_eq!(hit.unwrap().group, ProductGroup::Technology);
        assert_eq!(hit.unwrap().category, CclCategory::AerospacePropulsion);
    }

    #[test]
    fn test_keyword_only_promotes_to_ccl() {
        let r = det().scan("Drawing of a fiber-optic gyroscope assembly.");
        assert_eq!(r.classification, EarClassification::Ccl);
        assert!(r.by_category.contains_key(&CclCategory::NavigationAvionics));
    }

    #[test]
    fn test_keyword_nuclear() {
        let r = det().scan("Uranium enrichment cascade design notes.");
        assert_eq!(r.classification, EarClassification::Ccl);
        assert!(r.by_category.contains_key(&CclCategory::Nuclear));
    }

    #[test]
    fn test_generic_ear_promotes_to_ear99() {
        let r = det().scan("This is an EAR99 item with no specific listing.");
        assert_eq!(r.classification, EarClassification::Ear99);
        assert!(!r.has_ccl());
    }

    #[test]
    fn test_license_exception_enc_parsed() {
        let r = det().scan("Shipped under License Exception ENC.");
        assert!(r.has_license_exception());
        assert_eq!(r.license_exceptions[0].code, "ENC");
    }

    #[test]
    fn test_de_minimis_below_threshold() {
        let r = det().scan("Contains less than 10% US-origin controlled content.");
        assert!(!r.de_minimis.is_empty());
        assert!(r.de_minimis_below_threshold());
        assert!((r.de_minimis[0].percent - 10.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_de_minimis_at_or_above_threshold() {
        let r = det().scan("Under 30% controlled content present.");
        assert!(!r.de_minimis.is_empty());
        assert!(!r.de_minimis[0].below_default_threshold);
    }

    #[test]
    fn test_ccl_category_digit_roundtrip() {
        for c in [
            CclCategory::Nuclear,
            CclCategory::Electronics,
            CclCategory::TelecomCrypto,
            CclCategory::AerospacePropulsion,
        ] {
            assert_eq!(CclCategory::from_digit(c.digit()), Some(c));
        }
    }

    #[test]
    fn test_product_group_roundtrip() {
        for g in [
            ProductGroup::Equipment,
            ProductGroup::TestEquipment,
            ProductGroup::Materials,
            ProductGroup::Software,
            ProductGroup::Technology,
        ] {
            assert_eq!(ProductGroup::from_letter(g.letter()), Some(g));
        }
    }

    #[test]
    fn test_classification_ordering() {
        assert!(EarClassification::Ccl > EarClassification::Ear99);
        assert!(EarClassification::Ear99 > EarClassification::Uncontrolled);
    }

    #[test]
    fn test_eccn_uppercased_in_hit() {
        let r = det().scan("entry 5a002 lowercase form");
        assert_eq!(r.eccn_hits[0].eccn, "5A002");
    }

    #[test]
    fn test_multiple_eccns_sorted_by_span() {
        let r = det().scan("Reference 3A001 then 5D002 then 9E003.");
        for pair in r.eccn_hits.windows(2) {
            assert!(pair[0].span.0 < pair[1].span.0);
        }
    }
}
