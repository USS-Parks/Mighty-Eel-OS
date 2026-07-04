//! PHI detection for all 18 HIPAA Safe Harbor identifiers.
//!
//! Each identifier has one or more regex patterns and is detected with a
//! `PhiConfidence` tier (`Explicit` / `Probable` / `Possible`). The detector
//! returns a `PhiReport` aggregating every hit, the highest confidence
//! observed, and per-identifier counts so callers can drive BAA decisions
//! and de-identification without re-scanning the text.
//!
//! Patterns are intentionally compiled once at startup; per-request work is
//! a linear sweep through the compiled set. The performance budget is
//! < 10ms per query and is verified by `tests/phi_perf.rs`.

use std::collections::HashMap;

use regex::Regex;
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// The 18 HIPAA Safe Harbor identifiers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PhiIdentifier {
    Name,
    GeographicSubdivision,
    Date,
    PhoneNumber,
    FaxNumber,
    EmailAddress,
    SocialSecurityNumber,
    MedicalRecordNumber,
    HealthPlanBeneficiaryNumber,
    AccountNumber,
    CertificateLicenseNumber,
    VehicleIdentifier,
    DeviceIdentifier,
    Url,
    IpAddress,
    BiometricIdentifier,
    PhotographicImage,
    OtherUniqueIdentifier,
}

impl PhiIdentifier {
    /// Wire-format string for audit emission.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Name => "name",
            Self::GeographicSubdivision => "geographic_subdivision",
            Self::Date => "date",
            Self::PhoneNumber => "phone_number",
            Self::FaxNumber => "fax_number",
            Self::EmailAddress => "email_address",
            Self::SocialSecurityNumber => "ssn",
            Self::MedicalRecordNumber => "medical_record_number",
            Self::HealthPlanBeneficiaryNumber => "health_plan_beneficiary_number",
            Self::AccountNumber => "account_number",
            Self::CertificateLicenseNumber => "certificate_license_number",
            Self::VehicleIdentifier => "vehicle_identifier",
            Self::DeviceIdentifier => "device_identifier",
            Self::Url => "url",
            Self::IpAddress => "ip_address",
            Self::BiometricIdentifier => "biometric_identifier",
            Self::PhotographicImage => "photographic_image",
            Self::OtherUniqueIdentifier => "other_unique_identifier",
        }
    }
}

/// Detection confidence for a PHI hit.
///
/// Variants are declared from weakest to strongest so the derived `Ord`
/// makes `Possible < Probable < Explicit`. Callers can use `>=` to gate
/// on a minimum confidence level.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PhiConfidence {
    /// Weak signal — keyword or shape match only.
    Possible,
    /// Pattern is likely PHI in a medical context but could be benign.
    Probable,
    /// Pattern is highly specific to PHI (e.g., SSN-shape, ICD-10).
    Explicit,
}

/// One detected PHI instance.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PhiHit {
    /// Which of the 18 categories matched.
    pub identifier: PhiIdentifier,
    /// Confidence tier.
    pub confidence: PhiConfidence,
    /// Byte span `(start, end_exclusive)` in the original text.
    pub span: (usize, usize),
    /// Blake3 hash of the matched substring (32 hex chars). Audit logs
    /// must never store the raw text.
    pub matched_hash: String,
}

/// Aggregate of every hit in a single query.
#[derive(Debug, Clone, Default, Serialize)]
pub struct PhiReport {
    /// All hits in order of occurrence.
    pub hits: Vec<PhiHit>,
    /// Highest confidence seen anywhere in the text.
    pub highest_confidence: Option<PhiConfidence>,
    /// Per-identifier counts.
    pub by_identifier: HashMap<PhiIdentifier, usize>,
}

impl PhiReport {
    /// Convenience: total hits across all identifiers.
    pub fn total_hits(&self) -> usize {
        self.hits.len()
    }

    /// True when at least one Explicit-confidence hit was seen.
    pub fn has_explicit(&self) -> bool {
        self.highest_confidence == Some(PhiConfidence::Explicit)
    }

    /// True when any PHI was seen at all.
    pub fn has_any(&self) -> bool {
        !self.hits.is_empty()
    }
}

/// Tunables for the detector. Operators can raise the minimum confidence
/// floor for noisy domains.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PhiDetectorConfig {
    /// Lowest confidence to emit. Defaults to `Possible` (catch everything).
    #[serde(default = "default_min_confidence")]
    pub min_confidence: PhiConfidence,
}

fn default_min_confidence() -> PhiConfidence {
    PhiConfidence::Possible
}

impl Default for PhiDetectorConfig {
    fn default() -> Self {
        Self {
            min_confidence: default_min_confidence(),
        }
    }
}

/// PHI detection errors.
#[derive(Debug, Error)]
pub enum PhiError {
    /// Pattern compilation failed at detector build time.
    #[error("invalid PHI pattern for {identifier:?}: {source}")]
    InvalidPattern {
        /// Identifier whose pattern failed to compile.
        identifier: PhiIdentifier,
        /// Underlying regex error.
        source: regex::Error,
    },
}

/// Compiled pattern entry.
struct CompiledPattern {
    identifier: PhiIdentifier,
    confidence: PhiConfidence,
    regex: Regex,
}

/// Production PHI detector. Build once at startup, reuse across requests.
pub struct PhiDetector {
    config: PhiDetectorConfig,
    patterns: Vec<CompiledPattern>,
}

impl PhiDetector {
    /// Build a detector with custom config.
    pub fn new(config: PhiDetectorConfig) -> Result<Self, PhiError> {
        let raw_patterns = baseline_patterns();
        let mut compiled = Vec::with_capacity(raw_patterns.len());
        for (identifier, confidence, pattern) in raw_patterns {
            let regex = Regex::new(pattern)
                .map_err(|source| PhiError::InvalidPattern { identifier, source })?;
            compiled.push(CompiledPattern {
                identifier,
                confidence,
                regex,
            });
        }
        Ok(Self {
            config,
            patterns: compiled,
        })
    }

    /// Default detector with the baseline pattern set.
    pub fn baseline() -> Self {
        Self::new(PhiDetectorConfig::default()).expect("baseline PHI patterns must compile")
    }

    /// Scan a query and produce a full PHI report.
    pub fn scan(&self, text: &str) -> PhiReport {
        let mut report = PhiReport::default();
        for pattern in &self.patterns {
            if pattern.confidence < self.config.min_confidence {
                continue;
            }
            for found in pattern.regex.find_iter(text) {
                report.hits.push(PhiHit {
                    identifier: pattern.identifier,
                    confidence: pattern.confidence,
                    span: (found.start(), found.end()),
                    matched_hash: hash_match(found.as_str()),
                });
                *report.by_identifier.entry(pattern.identifier).or_insert(0) += 1;
                report.highest_confidence = Some(match report.highest_confidence {
                    Some(existing) => existing.max(pattern.confidence),
                    None => pattern.confidence,
                });
            }
        }
        report.hits.sort_by_key(|h| h.span.0);
        report
    }
}

fn hash_match(s: &str) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(s.as_bytes());
    hasher.finalize().to_hex().to_string()[..32].to_string()
}

/// Baseline pattern catalog. Tuples are
/// `(identifier, confidence, pattern)`.
///
/// The shipped patterns are intentionally conservative. Operators tighten
/// or extend them through the deployment-specific config (see
/// `config/compliance/hipaa.toml`).
fn baseline_patterns() -> Vec<(PhiIdentifier, PhiConfidence, &'static str)> {
    use PhiConfidence::*;
    use PhiIdentifier::*;
    vec![
        // 1. Name — clinical-prefix heuristic only (Explicit name detection
        //    is a future deeper module).
        (
            Name,
            Possible,
            r"\b(?:Dr|Mr|Mrs|Ms|Miss|Patient)\.?\s+[A-Z][a-z]+\b",
        ),
        // 2. Geographic subdivision — US ZIP+4 / state abbreviations.
        (GeographicSubdivision, Probable, r"\b\d{5}(?:-\d{4})?\b"),
        // 3. Date — multiple shapes: MM/DD/YYYY, YYYY-MM-DD.
        (Date, Probable, r"\b\d{1,2}/\d{1,2}/\d{2,4}\b"),
        (Date, Probable, r"\b\d{4}-\d{2}-\d{2}\b"),
        // 4. Phone number (US).
        (
            PhoneNumber,
            Explicit,
            r"\b\(?\d{3}\)?[\s.-]?\d{3}[\s.-]?\d{4}\b",
        ),
        // 5. Fax number — same shape as phone, contextual cue word.
        (
            FaxNumber,
            Probable,
            r"(?i)\bfax[\s:#-]+\(?\d{3}\)?[\s.-]?\d{3}[\s.-]?\d{4}\b",
        ),
        // 6. Email.
        (
            EmailAddress,
            Explicit,
            r"\b[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Za-z]{2,}\b",
        ),
        // 7. SSN.
        (SocialSecurityNumber, Explicit, r"\b\d{3}-\d{2}-\d{4}\b"),
        // 8. Medical record number — `MRN` prefix or `MRN: nnn`.
        (MedicalRecordNumber, Explicit, r"(?i)\bMRN[\s:#-]*\d{4,}\b"),
        // 9. Health plan beneficiary number — `HPB:` / `Member ID` prefix.
        (
            HealthPlanBeneficiaryNumber,
            Probable,
            r"(?i)\b(?:HPB|Member\s+ID|Plan\s+#)[\s:#-]*[A-Z0-9-]{5,}\b",
        ),
        // 10. Account number — `Acct` / `Account #`.
        (
            AccountNumber,
            Probable,
            r"(?i)\b(?:Acct|Account)\s*#?\s*[A-Z0-9-]{4,}\b",
        ),
        // 11. Certificate / license — `License #`, DEA shape.
        (
            CertificateLicenseNumber,
            Probable,
            r"(?i)\b(?:License|DEA|NPI)\s*#?\s*[A-Z0-9-]{6,}\b",
        ),
        // 12. Vehicle identifier — VIN (17 alphanumeric, no I/O/Q).
        (VehicleIdentifier, Explicit, r"\b[A-HJ-NPR-Z0-9]{17}\b"),
        // 13. Device identifier — `Serial #`, `UDI:` prefix.
        (
            DeviceIdentifier,
            Possible,
            r"(?i)\b(?:Serial|UDI|Device\s+ID)[\s:#-]*[A-Z0-9-]{4,}\b",
        ),
        // 14. URL.
        (Url, Explicit, r#"\bhttps?://[^\s<>"']+"#),
        // 15. IP address (IPv4 only — IPv6 deferred).
        (IpAddress, Explicit, r"\b(?:\d{1,3}\.){3}\d{1,3}\b"),
        // 16. Biometric identifier — keyword markers.
        (
            BiometricIdentifier,
            Possible,
            r"(?i)\b(?:fingerprint|iris\s+scan|retina\s+scan|voice\s+print|biometric)\b",
        ),
        // 17. Photographic image — keyword markers.
        (
            PhotographicImage,
            Possible,
            r"(?i)\b(?:photo(?:graph)?|x-ray|MRI|CT\s+scan|ultrasound)\s+(?:of|showing)\b",
        ),
        // 18. Other unique identifier — generic catchall for `ID:` followed
        //     by 6+ alphanumeric chars.
        (
            OtherUniqueIdentifier,
            Possible,
            r"(?i)\bID[\s:#-]+[A-Z0-9]{6,}\b",
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn det() -> PhiDetector {
        PhiDetector::baseline()
    }

    #[test]
    fn test_no_phi_in_neutral_text() {
        let r = det().scan("Tell me about clouds.");
        assert!(!r.has_any());
        assert!(r.highest_confidence.is_none());
    }

    #[test]
    fn test_ssn_detected_as_explicit() {
        let r = det().scan("subject SSN is 123-45-6789");
        assert!(r.has_explicit());
        assert!(
            r.by_identifier
                .contains_key(&PhiIdentifier::SocialSecurityNumber)
        );
    }

    #[test]
    fn test_email_detected_as_explicit() {
        let r = det().scan("Contact alice@example.com");
        assert!(r.has_explicit());
        assert!(r.by_identifier.contains_key(&PhiIdentifier::EmailAddress));
    }

    #[test]
    fn test_phone_detected() {
        let r = det().scan("Call 415-555-1212 tonight");
        assert!(r.has_any());
        assert!(r.by_identifier.contains_key(&PhiIdentifier::PhoneNumber));
    }

    #[test]
    fn test_url_detected() {
        let r = det().scan("see https://example.com/page for details");
        assert!(r.has_explicit());
        assert!(r.by_identifier.contains_key(&PhiIdentifier::Url));
    }

    #[test]
    fn test_ip_address_detected() {
        let r = det().scan("Server reachable at 10.0.0.42");
        assert!(r.by_identifier.contains_key(&PhiIdentifier::IpAddress));
    }

    #[test]
    fn test_mrn_detected_as_explicit() {
        let r = det().scan("Patient MRN: 8472901 admitted");
        assert!(
            r.by_identifier
                .contains_key(&PhiIdentifier::MedicalRecordNumber)
        );
        assert!(r.has_explicit());
    }

    #[test]
    fn test_dr_name_is_only_possible() {
        let r = det().scan("Dr. Smith examined the patient");
        let hit = r.hits.iter().find(|h| h.identifier == PhiIdentifier::Name);
        assert!(hit.is_some());
        assert_eq!(hit.unwrap().confidence, PhiConfidence::Possible);
    }

    #[test]
    fn test_dates_detected() {
        let r = det().scan("admission 03/14/2026 discharge 2026-03-20");
        let dates: usize = *r.by_identifier.get(&PhiIdentifier::Date).unwrap_or(&0);
        assert_eq!(dates, 2);
    }

    #[test]
    fn test_zip_code_detected_as_geographic() {
        let r = det().scan("clinic in 94105 area");
        assert!(
            r.by_identifier
                .contains_key(&PhiIdentifier::GeographicSubdivision)
        );
    }

    #[test]
    fn test_min_confidence_filter_drops_possible() {
        let cfg = PhiDetectorConfig {
            min_confidence: PhiConfidence::Probable,
        };
        let d = PhiDetector::new(cfg).unwrap();
        // Pure-Possible-only text should produce no hits at Probable floor.
        let r = d.scan("Dr. Smith was helpful");
        assert!(!r.has_any());
    }

    #[test]
    fn test_hits_are_sorted_by_span_start() {
        let r = det().scan("SSN 123-45-6789 email a@b.co phone 415-555-1212");
        for pair in r.hits.windows(2) {
            assert!(pair[0].span.0 <= pair[1].span.0);
        }
    }

    #[test]
    fn test_matched_text_is_hashed_never_raw() {
        let r = det().scan("123-45-6789");
        assert_eq!(r.hits.len(), 1);
        assert_eq!(r.hits[0].matched_hash.len(), 32);
        assert_ne!(r.hits[0].matched_hash, "123-45-6789");
    }

    #[test]
    fn test_report_summary_fields() {
        let r = det().scan("email a@b.co SSN 123-45-6789 SSN 999-88-7777");
        assert_eq!(r.total_hits(), 3);
        assert_eq!(r.by_identifier[&PhiIdentifier::SocialSecurityNumber], 2);
        assert_eq!(r.by_identifier[&PhiIdentifier::EmailAddress], 1);
        assert_eq!(r.highest_confidence, Some(PhiConfidence::Explicit));
    }

    #[test]
    fn test_vin_explicit() {
        let r = det().scan("VIN 1HGCM82633A123456 sold");
        assert!(
            r.by_identifier
                .contains_key(&PhiIdentifier::VehicleIdentifier)
        );
    }
}
