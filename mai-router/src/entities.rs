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
use unicode_normalization::UnicodeNormalization;
use unicode_normalization::char::is_combining_mark;

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
        // Normalize (case-fold + compatibility decomposition + mark strip +
        // homoglyph fold + whitespace collapse) with a byte-offset map back to the
        // ORIGINAL text, so obfuscated terms are caught (audit G7) and `find_all`
        // still reports spans in original coordinates even though the fold changed
        // byte lengths.
        let (haystack, offsets) = normalize_with_offsets(text);
        let mut matches = Vec::new();
        find_all(
            &haystack,
            &offsets,
            &self.medical,
            EntityKind::Medical,
            text,
            &mut matches,
        );
        find_all(
            &haystack,
            &offsets,
            &self.tribal,
            EntityKind::Tribal,
            text,
            &mut matches,
        );
        find_all(
            &haystack,
            &offsets,
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

/// Normalize `text` for obfuscation-resistant matching, returning the folded
/// string plus a byte-offset map: for each byte index `i` of the folded string,
/// `offsets[i]` is the byte offset in the ORIGINAL text of the char that produced
/// it, and `offsets[folded.len()]` is `text.len()`. A match at folded `[hs, he)`
/// maps to original `(offsets[hs], offsets[he])`, so spans stay in original
/// coordinates regardless of how much the fold changed byte lengths (audit G7).
///
/// Per-char pipeline: lowercase -> compatibility decomposition (folds full-width
/// `ｐ`, ligatures `ﬁ`, and splits accents into base + combining mark) -> drop
/// combining marks (strips diacritics; defeats mark-splicing) -> map common
/// Cyrillic/Greek homoglyphs to their Latin lookalike. Runs of Unicode whitespace
/// collapse to a single ASCII space. Over-folding only ever *widens* detection
/// (fail-safe: a false hit routes local), never hides a term.
fn normalize_with_offsets(text: &str) -> (String, Vec<usize>) {
    let mut folded = String::with_capacity(text.len());
    let mut offsets = Vec::with_capacity(text.len() + 1);
    let mut buf = [0u8; 4];
    let mut prev_was_space = false;
    for (ob, ch) in text.char_indices() {
        if ch.is_whitespace() {
            // Collapse a run of whitespace to one ASCII space.
            if !prev_was_space {
                offsets.push(ob);
                folded.push(' ');
                prev_was_space = true;
            }
            continue;
        }
        prev_was_space = false;
        for lc in ch.to_lowercase() {
            for nf in std::iter::once(lc).nfkd() {
                if is_combining_mark(nf) {
                    continue;
                }
                let mapped = fold_homoglyph(nf);
                let s = mapped.encode_utf8(&mut buf);
                for _ in 0..s.len() {
                    offsets.push(ob);
                }
                folded.push_str(s);
            }
        }
    }
    offsets.push(text.len());
    (folded, offsets)
}

/// Map a curated set of common Cyrillic/Greek homoglyphs to their Latin
/// lookalike (lowercase). Not exhaustive — the widely-abused confusables that
/// spoof the Latin letters in the compliance vocabulary. Non-homoglyph chars
/// pass through unchanged.
fn fold_homoglyph(c: char) -> char {
    match c {
        // Cyrillic -> Latin
        'а' => 'a',
        'в' => 'b',
        'е' | 'ё' => 'e',
        'к' => 'k',
        'м' => 'm',
        'н' => 'h',
        'о' => 'o',
        'р' => 'p',
        'с' => 'c',
        'т' => 't',
        'у' => 'y',
        'х' => 'x',
        'і' | 'ї' => 'i',
        'ј' => 'j',
        'ѕ' => 's',
        // Greek -> Latin
        'ο' => 'o',
        'α' => 'a',
        'ρ' => 'p',
        'ι' => 'i',
        'ν' => 'v',
        'τ' => 't',
        'κ' => 'k',
        'μ' => 'm',
        'χ' => 'x',
        'ε' => 'e',
        other => other,
    }
}

fn find_all(
    haystack: &str,
    offsets: &[usize],
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
            let h_start = start + pos;
            let h_end = h_start + needle.len();
            // Map the folded-haystack span back to original-text coordinates.
            let (o_start, o_end) = (offsets[h_start], offsets[h_end]);
            let matched_slice = original.get(o_start..o_end).unwrap_or("");
            out.push(EntityMatch {
                kind,
                span: (o_start, o_end),
                matched_hash: hash_match(matched_slice),
                confidence: 1.0,
            });
            start = h_end;
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
    fn match_span_is_original_coordinates_after_length_changing_fold() {
        // Audit G7: 'İ' (U+0130, 2 bytes) lowercases to "i̇" (3 bytes), so the folded
        // haystack and the original diverge in byte length. The reported span must
        // index the ORIGINAL text, not the folded haystack.
        let s = EntityScanner::baseline();
        let text = "İ patient today";
        let m = s
            .scan(text)
            .into_iter()
            .find(|m| m.kind == EntityKind::Medical)
            .expect("'patient' detected");
        let (a, b) = m.span;
        assert_eq!(
            &text[a..b],
            "patient",
            "span must slice the original 'patient'"
        );
    }

    #[test]
    fn obfuscated_terms_are_detected_after_normalization() {
        // Audit G7: normalization catches homoglyph / full-width / diacritic /
        // whitespace obfuscation of the compliance vocabulary.
        let s = EntityScanner::baseline();

        // Full-width Latin: ｐｒｅｓｃｒｉｐｔｉｏｎ -> prescription.
        let full_width: String = "prescription"
            .chars()
            .map(|c| char::from_u32(c as u32 - 0x61 + 0xFF41).unwrap())
            .collect();
        let fw = s.scan(&full_width);
        assert!(
            fw.iter().any(|m| m.kind == EntityKind::Medical),
            "full-width 'prescription' must be detected"
        );
        // Span still indexes the (heavily length-changed) original validly.
        let m = fw.iter().find(|m| m.kind == EntityKind::Medical).unwrap();
        assert!(
            full_width.get(m.span.0..m.span.1).is_some(),
            "span must be valid original-coordinate bytes"
        );

        // Diacritics: pátîent -> patient (compat-decompose + strip marks).
        assert!(
            s.scan("p\u{00e1}t\u{00ee}ent")
                .iter()
                .any(|m| m.kind == EntityKind::Medical),
            "accented 'patient' must be detected"
        );

        // Cyrillic homoglyphs: р(er) + h + і(dotted i) -> phi.
        assert!(
            s.scan("\u{0440}h\u{0456}")
                .iter()
                .any(|m| m.kind == EntityKind::Medical),
            "homoglyph 'phi' must be detected"
        );

        // Collapsed whitespace: two non-breaking spaces -> single space.
        assert!(
            s.scan("medical\u{00a0}\u{00a0}record")
                .iter()
                .any(|m| m.kind == EntityKind::Medical),
            "'medical record' with obfuscated whitespace must be detected"
        );
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
