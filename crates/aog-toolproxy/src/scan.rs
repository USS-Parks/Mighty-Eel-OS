//! Egress scanning of tool results (T5).
//!
//! A tool result re-enters the model's context from **outside** the trust
//! boundary (a web page, a file, a database row, another tool). Before the model
//! ever sees it, the proxy scans that output and **redacts** any secret, PHI, or
//! ITAR-controlled span it carries — one-way, so the raw value is gone from the
//! context the model reads. This blocks two harms at once: a leaked credential
//! (e.g. an AWS key) that a hijacked agent could exfiltrate, and controlled data
//! (PHI / ITAR) entering an un-cleared context.
//!
//! This is deliberately **not** the G8 egress-tokenization path. G8 tokenizes
//! *outbound* spans reversibly so a cloud provider sees placeholders and the
//! response detokenizes on return. T5 scans *inbound* tool output and redacts
//! **irreversibly** — the model has no legitimate need for the raw secret (the
//! tool that needed it already ran), so nothing is kept to restore.
//!
//! Detection reuses the mai-compliance `PhiDetector` + `ItarDetector` (the plan's
//! "reuse phi/itar detectors") and adds a focused, dependency-light secret scanner
//! for the credential shapes those detectors do not cover (AWS keys, GitHub /
//! OpenAI / Slack tokens, PEM private-key blocks). Only span **counts + kind
//! labels** are surfaced for the receipt — never the redacted value.

use base64::Engine as _;
use mai_compliance::{
    ItarConfidence, ItarDetector, ItarDetectorConfig, PhiConfidence, PhiDetector, PhiDetectorConfig,
};
use serde_json::Value;

const MAX_SCAN_BYTES: usize = 1_048_576;
const MAX_SCAN_NODES: usize = 100_000;
const MAX_SCAN_DEPTH: usize = 64;
const MAX_STRING_BYTES: usize = 262_144;
const MAX_ENCODED_CHARS: usize = 131_072;

/// The result of scanning a tool output: the redacted value plus the kind label
/// of every redacted span (metadata only — the raw values never appear here).
pub struct ScanOutcome {
    /// The output with every detected secret/PHI/ITAR span replaced by a
    /// `[REDACTED:<kind>]` marker. Byte-identical to the input when nothing hit.
    pub output: Value,
    /// One kind label per redacted span, in document order (e.g.
    /// `["secret_aws_key", "phi_ssn"]`). Safe to receipt.
    pub redactions: Vec<String>,
    /// A deterministic work-limit or structural violation. Violating output is
    /// quarantined as JSON null and never returned to the model.
    pub violation: Option<ScanViolation>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScanViolation {
    Bytes,
    Nodes,
    Depth,
    StringBytes,
    SensitiveObjectKey,
}

impl ScanViolation {
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Bytes => "egress_limit_bytes",
            Self::Nodes => "egress_limit_nodes",
            Self::Depth => "egress_limit_depth",
            Self::StringBytes => "egress_limit_string",
            Self::SensitiveObjectKey => "egress_sensitive_object_key",
        }
    }
}

#[derive(Default)]
struct ScanBudget {
    bytes: usize,
    nodes: usize,
}

impl ScanOutcome {
    /// Number of redacted spans.
    #[must_use]
    pub fn count(&self) -> u32 {
        u32::try_from(self.redactions.len()).unwrap_or(u32::MAX)
    }

    /// True when nothing was redacted (the output is unchanged).
    #[must_use]
    pub fn is_clean(&self) -> bool {
        self.redactions.is_empty()
    }
}

/// One span to redact and its kind label.
struct Finding {
    start: usize,
    end: usize,
    kind: String,
}

/// The egress scanner: PHI + ITAR (mai-compliance) + a focused secret scanner,
/// run over every string in a tool result. Object keys are scanned and sensitive
/// keys quarantine the result. Total bytes, nodes, depth, string length, and
/// standard-base64/hex decoding work are bounded by the constants above.
///
/// The PHI/ITAR confidence floor is **Probable** — the weakest "Possible"
/// keyword-only tier is skipped so ordinary tool output is not mangled by
/// single-word matches, while SSN-shape PHI (Explicit) and real controlled data
/// (Probable/Explicit) are still caught. Secret detection is always high-precision
/// and always on.
pub struct EgressScanner {
    phi: PhiDetector,
    itar: ItarDetector,
}

impl Default for EgressScanner {
    fn default() -> Self {
        Self::baseline()
    }
}

impl EgressScanner {
    /// The default scanner: baseline PHI + ITAR patterns at a Probable floor.
    #[must_use]
    pub fn baseline() -> Self {
        let phi = PhiDetector::new(PhiDetectorConfig {
            min_confidence: PhiConfidence::Probable,
        })
        .expect("baseline PHI patterns compile");
        let itar = ItarDetector::new(ItarDetectorConfig {
            min_confidence: ItarConfidence::Probable,
            default_to_itar_on_ambiguity: false,
        })
        .expect("baseline ITAR patterns compile");
        Self { phi, itar }
    }

    /// Scan a tool result, returning the redacted value + the redacted-span kinds.
    #[must_use]
    pub fn scan_result(&self, output: &Value) -> ScanOutcome {
        let mut budget = ScanBudget::default();
        match self.redact_value(output, 0, &mut budget) {
            Ok((output, redactions)) => ScanOutcome {
                output,
                redactions,
                violation: None,
            },
            Err(violation) => ScanOutcome {
                output: Value::Null,
                redactions: vec![violation.label().to_string()],
                violation: Some(violation),
            },
        }
    }

    /// Scan one free-text channel (including tool errors and receipt metadata).
    pub fn scan_text(&self, text: &str) -> Result<(String, Vec<String>), ScanViolation> {
        if text.len() > MAX_STRING_BYTES {
            return Err(ScanViolation::StringBytes);
        }
        Ok(self.redact_str(text))
    }

    /// Recurse through keys and values within deterministic work limits.
    fn redact_value(
        &self,
        value: &Value,
        depth: usize,
        budget: &mut ScanBudget,
    ) -> Result<(Value, Vec<String>), ScanViolation> {
        if depth > MAX_SCAN_DEPTH {
            return Err(ScanViolation::Depth);
        }
        budget.nodes = budget.nodes.saturating_add(1);
        if budget.nodes > MAX_SCAN_NODES {
            return Err(ScanViolation::Nodes);
        }
        match value {
            Value::String(s) => {
                if s.len() > MAX_STRING_BYTES {
                    return Err(ScanViolation::StringBytes);
                }
                budget.bytes = budget.bytes.saturating_add(s.len());
                if budget.bytes > MAX_SCAN_BYTES {
                    return Err(ScanViolation::Bytes);
                }
                let (redacted, kinds) = self.redact_str(s);
                Ok((Value::String(redacted), kinds))
            }
            Value::Array(items) => {
                let mut kinds = Vec::new();
                let mut out = Vec::with_capacity(items.len());
                for item in items {
                    let (redacted, item_kinds) = self.redact_value(item, depth + 1, budget)?;
                    kinds.extend(item_kinds);
                    out.push(redacted);
                }
                Ok((Value::Array(out), kinds))
            }
            Value::Object(map) => {
                let mut kinds = Vec::new();
                let mut out = serde_json::Map::with_capacity(map.len());
                for (key, val) in map {
                    budget.bytes = budget.bytes.saturating_add(key.len());
                    if budget.bytes > MAX_SCAN_BYTES {
                        return Err(ScanViolation::Bytes);
                    }
                    let (_, key_kinds) = self.redact_str(key);
                    if !key_kinds.is_empty() {
                        return Err(ScanViolation::SensitiveObjectKey);
                    }
                    let (redacted, value_kinds) = self.redact_value(val, depth + 1, budget)?;
                    kinds.extend(value_kinds);
                    out.insert(key.clone(), redacted);
                }
                Ok((Value::Object(out), kinds))
            }
            other => Ok((other.clone(), Vec::new())),
        }
    }

    /// Collect every finding (PHI, ITAR, secret) in a string and redact them.
    fn redact_str(&self, text: &str) -> (String, Vec<String>) {
        let mut findings: Vec<Finding> = Vec::new();
        for hit in self.phi.scan(text).hits {
            findings.push(Finding {
                start: hit.span.0,
                end: hit.span.1,
                kind: format!("phi_{}", hit.identifier.as_str()),
            });
        }
        for hit in self.itar.scan(text).hits {
            findings.push(Finding {
                start: hit.span.0,
                end: hit.span.1,
                kind: "itar".to_string(),
            });
        }
        scan_secrets(text, &mut findings);
        self.scan_encoded(text, &mut findings);
        redact_with(text, findings)
    }

    fn scan_encoded(&self, text: &str, findings: &mut Vec<Finding>) {
        let trimmed = text.trim();
        if trimmed.len() < 16 || trimmed.len() > MAX_ENCODED_CHARS {
            return;
        }
        let start = text.find(trimmed).unwrap_or(0);
        let decoded = if trimmed.len().is_multiple_of(2)
            && trimmed.bytes().all(|byte| byte.is_ascii_hexdigit())
        {
            hex::decode(trimmed).ok()
        } else {
            base64::engine::general_purpose::STANDARD
                .decode(trimmed)
                .ok()
        };
        let Some(decoded) = decoded else {
            return;
        };
        let Ok(decoded) = std::str::from_utf8(&decoded) else {
            return;
        };
        let mut decoded_findings = Vec::new();
        for hit in self.phi.scan(decoded).hits {
            decoded_findings.push(format!("phi_{}", hit.identifier.as_str()));
        }
        if !self.itar.scan(decoded).hits.is_empty() {
            decoded_findings.push("itar".to_string());
        }
        let mut secrets = Vec::new();
        scan_secrets(decoded, &mut secrets);
        decoded_findings.extend(secrets.into_iter().map(|finding| finding.kind));
        if let Some(kind) = decoded_findings.first() {
            findings.push(Finding {
                start,
                end: start + trimmed.len(),
                kind: format!("encoded_{kind}"),
            });
        }
    }
}

/// Apply a set of findings to `text`, returning the redacted string and the kinds
/// actually redacted (document order). Overlapping spans are resolved leftmost-and-
/// widest-first; replacements are applied end-to-start so earlier offsets stay valid.
fn redact_with(text: &str, mut findings: Vec<Finding>) -> (String, Vec<String>) {
    if findings.is_empty() {
        return (text.to_string(), Vec::new());
    }
    // Leftmost first; on a tie the wider span wins.
    findings.sort_by(|a, b| a.start.cmp(&b.start).then(b.end.cmp(&a.end)));

    // Keep a non-overlapping set.
    let mut chosen: Vec<Finding> = Vec::new();
    let mut last_end = 0usize;
    for f in findings {
        if f.end > f.start && f.start >= last_end {
            last_end = f.end;
            chosen.push(f);
        }
    }

    let mut out = text.to_string();
    let mut kinds_rev: Vec<String> = Vec::with_capacity(chosen.len());
    for f in chosen.iter().rev() {
        if f.end <= out.len() && out.is_char_boundary(f.start) && out.is_char_boundary(f.end) {
            out.replace_range(f.start..f.end, &format!("[REDACTED:{}]", f.kind));
            kinds_rev.push(f.kind.clone());
        }
    }
    kinds_rev.reverse();
    (out, kinds_rev)
}

// ── Secret scanner (dependency-light, high-precision) ────────────────────────

/// Find every secret span in `text`, appending a [`Finding`] for each.
fn scan_secrets(text: &str, out: &mut Vec<Finding>) {
    scan_aws(text, out);
    scan_prefixed(text, "ghp_", 36, 36, is_alnum, "secret_github_pat", out);
    scan_prefixed(text, "sk-", 20, 64, is_token, "secret_openai_key", out);
    for prefix in ["xoxb-", "xoxp-", "xoxa-", "xoxr-", "xoxs-"] {
        scan_prefixed(text, prefix, 10, 64, is_token, "secret_slack_token", out);
    }
    scan_pem(text, out);
}

fn is_alnum(b: u8) -> bool {
    b.is_ascii_alphanumeric()
}

fn is_token(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'-' || b == b'_'
}

fn is_aws_body(b: u8) -> bool {
    b.is_ascii_uppercase() || b.is_ascii_digit()
}

/// AWS access-key IDs: an `AKIA` (long-term) or `ASIA` (STS) prefix followed by
/// exactly 16 upper-case alphanumerics, on a word boundary.
fn scan_aws(text: &str, out: &mut Vec<Finding>) {
    let bytes = text.as_bytes();
    for prefix in ["AKIA", "ASIA"] {
        let pb = prefix.as_bytes();
        let mut i = 0usize;
        while i + 20 <= bytes.len() {
            let boundary = i == 0 || !is_alnum(bytes[i - 1]);
            if boundary
                && &bytes[i..i + 4] == pb
                && bytes[i + 4..i + 20].iter().all(|&b| is_aws_body(b))
            {
                out.push(Finding {
                    start: i,
                    end: i + 20,
                    kind: "secret_aws_key".to_string(),
                });
                i += 20;
                continue;
            }
            i += 1;
        }
    }
}

/// A `prefix` on a word boundary followed by a run of `min_tail..=max_tail` chars
/// matching `pred` — the shape of a prefixed API token.
fn scan_prefixed(
    text: &str,
    prefix: &str,
    min_tail: usize,
    max_tail: usize,
    pred: fn(u8) -> bool,
    kind: &'static str,
    out: &mut Vec<Finding>,
) {
    let bytes = text.as_bytes();
    let pb = prefix.as_bytes();
    let plen = pb.len();
    let mut i = 0usize;
    while i + plen <= bytes.len() {
        let boundary = i == 0 || !is_alnum(bytes[i - 1]);
        if boundary && &bytes[i..i + plen] == pb {
            let body_start = i + plen;
            let mut j = body_start;
            while j < bytes.len() && j - body_start < max_tail && pred(bytes[j]) {
                j += 1;
            }
            if j - body_start >= min_tail {
                out.push(Finding {
                    start: i,
                    end: j,
                    kind: kind.to_string(),
                });
                i = j;
                continue;
            }
        }
        i += 1;
    }
}

/// PEM private-key blocks: `-----BEGIN ... PRIVATE KEY----- … -----END ... KEY-----`.
/// Redacts the whole block; if no footer is present, redacts through the header.
fn scan_pem(text: &str, out: &mut Vec<Finding>) {
    const BEGIN: &str = "-----BEGIN";
    const HEADER_END: &str = "PRIVATE KEY-----";
    const FOOTER_END: &str = "KEY-----";
    let mut from = 0usize;
    while let Some(rel) = text[from..].find(BEGIN) {
        let start = from + rel;
        let Some(hrel) = text[start..].find(HEADER_END) else {
            break;
        };
        let after_header = start + hrel + HEADER_END.len();
        if let Some(frel) = text[after_header..].find(FOOTER_END) {
            let end = after_header + frel + FOOTER_END.len();
            out.push(Finding {
                start,
                end,
                kind: "secret_private_key".to_string(),
            });
            from = end;
        } else {
            out.push(Finding {
                start,
                end: after_header,
                kind: "secret_private_key".to_string(),
            });
            from = after_header;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scanner() -> EgressScanner {
        EgressScanner::baseline()
    }

    fn s(v: &str) -> Value {
        Value::String(v.to_string())
    }

    #[test]
    fn benign_output_is_unchanged() {
        let out = scanner().scan_result(&s("The capital of France is Paris."));
        assert!(out.is_clean());
        assert_eq!(out.output, s("The capital of France is Paris."));
        assert_eq!(out.count(), 0);
    }

    #[test]
    fn aws_key_is_redacted() {
        // The canonical 20-char AWS example key.
        let out = scanner().scan_result(&s("key=AKIAIOSFODNN7EXAMPLE done"));
        let text = out.output.as_str().unwrap();
        assert!(
            !text.contains("AKIAIOSFODNN7EXAMPLE"),
            "raw key gone: {text}"
        );
        assert!(
            text.contains("[REDACTED:secret_aws_key]"),
            "redacted: {text}"
        );
        assert!(text.starts_with("key="));
        assert!(text.ends_with(" done"));
        assert!(out.redactions.iter().any(|k| k == "secret_aws_key"));
    }

    #[test]
    fn asia_temp_key_is_redacted() {
        let out = scanner().scan_result(&s("ASIAY34FZKBOKMUTVV7A"));
        assert!(
            !out.output
                .as_str()
                .unwrap()
                .contains("ASIAY34FZKBOKMUTVV7A")
        );
        assert_eq!(out.count(), 1);
    }

    #[test]
    fn ssn_phi_is_redacted() {
        let out = scanner().scan_result(&s("patient SSN 123-45-6789 on file"));
        let text = out.output.as_str().unwrap();
        assert!(!text.contains("123-45-6789"), "raw SSN gone: {text}");
        assert!(text.contains("[REDACTED:phi_"), "redacted PHI: {text}");
        assert!(out.redactions.iter().any(|k| k.starts_with("phi_")));
    }

    #[test]
    fn github_and_openai_tokens_are_redacted() {
        let ghp = format!("token {} end", "ghp_".to_string() + &"a".repeat(36));
        let out = scanner().scan_result(&s(&ghp));
        assert!(
            out.output
                .as_str()
                .unwrap()
                .contains("[REDACTED:secret_github_pat]")
        );

        let sk = format!("OPENAI={}", "sk-".to_string() + &"b".repeat(40));
        let out = scanner().scan_result(&s(&sk));
        assert!(
            out.output
                .as_str()
                .unwrap()
                .contains("[REDACTED:secret_openai_key]")
        );
    }

    #[test]
    fn short_sk_prefix_is_not_a_false_positive() {
        // "task-force" contains "sk-" but the tail is far shorter than a real key.
        let out = scanner().scan_result(&s("the task-force convened"));
        assert!(out.is_clean(), "no false positive: {:?}", out.redactions);
    }

    #[test]
    fn pem_private_key_block_is_redacted() {
        let pem = "-----BEGIN RSA PRIVATE KEY-----\nMIIBderp\n-----END RSA PRIVATE KEY-----";
        let out = scanner().scan_result(&s(pem));
        let text = out.output.as_str().unwrap();
        assert!(!text.contains("MIIBderp"), "key body gone: {text}");
        assert!(text.contains("[REDACTED:secret_private_key]"));
    }

    #[test]
    fn redacts_string_values_nested_in_json() {
        let v = serde_json::json!({
            "rows": [
                { "note": "AWS AKIAIOSFODNN7EXAMPLE here" },
                { "note": "nothing sensitive" }
            ]
        });
        let out = scanner().scan_result(&v);
        let dumped = serde_json::to_string(&out.output).unwrap();
        assert!(
            !dumped.contains("AKIAIOSFODNN7EXAMPLE"),
            "nested key redacted: {dumped}"
        );
        assert_eq!(out.count(), 1);
        // Object keys and benign values are preserved.
        assert!(dumped.contains("\"note\""));
        assert!(dumped.contains("nothing sensitive"));
    }

    #[test]
    fn multiple_distinct_secrets_all_redacted() {
        let out = scanner().scan_result(&s("aws AKIAIOSFODNN7EXAMPLE and ssn 123-45-6789 both"));
        assert!(out.count() >= 2, "both spans found: {:?}", out.redactions);
        let text = out.output.as_str().unwrap();
        assert!(!text.contains("AKIAIOSFODNN7EXAMPLE"));
        assert!(!text.contains("123-45-6789"));
    }

    #[test]
    fn non_string_scalars_pass_through() {
        let v = serde_json::json!({ "count": 3, "ok": true, "ratio": 1.5 });
        let out = scanner().scan_result(&v);
        assert!(out.is_clean());
        assert_eq!(out.output, v);
    }

    #[test]
    fn reg_lsf_029_sensitive_object_key_quarantines_the_result() {
        let v = serde_json::json!({ "AKIAIOSFODNN7EXAMPLE": "value" });
        let out = scanner().scan_result(&v);
        assert_eq!(out.output, Value::Null);
        assert_eq!(out.violation, Some(ScanViolation::SensitiveObjectKey));
    }

    #[test]
    fn base64_and_hex_encoded_secrets_are_redacted() {
        let secret = "AKIAIOSFODNN7EXAMPLE";
        let encoded = base64::engine::general_purpose::STANDARD.encode(secret);
        let out = scanner().scan_result(&s(&encoded));
        assert!(!out.output.as_str().unwrap().contains(&encoded));
        assert!(out.redactions[0].starts_with("encoded_secret_"));

        let encoded = hex::encode(secret);
        let out = scanner().scan_result(&s(&encoded));
        assert!(!out.output.as_str().unwrap().contains(&encoded));
        assert!(out.redactions[0].starts_with("encoded_secret_"));
    }

    #[test]
    fn reg_lsd_010_depth_node_and_byte_limits_quarantine_deterministically() {
        let mut deep = Value::Null;
        for _ in 0..=MAX_SCAN_DEPTH {
            deep = Value::Array(vec![deep]);
        }
        assert_eq!(
            scanner().scan_result(&deep).violation,
            Some(ScanViolation::Depth)
        );

        let many = Value::Array(vec![Value::Null; MAX_SCAN_NODES]);
        assert_eq!(
            scanner().scan_result(&many).violation,
            Some(ScanViolation::Nodes)
        );

        let large = Value::Array(vec![s(&"x".repeat(220_000)); 5]);
        assert_eq!(
            scanner().scan_result(&large).violation,
            Some(ScanViolation::Bytes)
        );

        assert!(matches!(
            scanner().scan_text(&"x".repeat(MAX_STRING_BYTES + 1)),
            Err(ScanViolation::StringBytes)
        ));
    }
}
