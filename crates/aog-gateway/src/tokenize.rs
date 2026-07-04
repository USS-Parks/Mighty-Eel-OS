//! Tokenization on egress (G8).
//!
//! When a request is dispatched to a **cloud** provider, sensitive spans in the
//! outbound messages are swapped for unique placeholders *before* egress — so the
//! cloud provider only ever sees placeholders — and the model's response is
//! detokenized inside the trust boundary on return.
//!
//! Detection reuses the mai-compliance PHI detector (the same span source
//! `deid::Redactor` is built on). `deid`'s own redaction is deliberately one-way
//! (every SSN → `[PHI:ssn]`), which cannot be reversed; G8 needs a *reversible*
//! swap, so it gives each span a unique placeholder (`[AOG:<kind>:<n>]`) and keeps
//! the originals in a request-scoped map used only on the return path, inside the
//! boundary. The map never leaves the gateway and is never receipted (only its
//! span count is — receipts stay metadata-only).
//!
//! Scope: the **non-stream** completion path, where the full request and response
//! are in hand — matching G7's metering scope. Streaming egress tokenization must
//! reassemble placeholders across SSE chunk boundaries and is a follow-on.

use axum::http::HeaderValue;
use axum::response::Response;
use mai_compliance::PhiDetector;

use crate::provider::ChatMessage;

/// The outbound side of an egress round-trip: the (possibly) tokenized messages
/// plus the request-scoped restore map (`placeholder → original span`).
pub struct Egress {
    /// Messages to dispatch — tokenized when cloud-bound and sensitive, else the
    /// originals unchanged.
    pub messages: Vec<ChatMessage>,
    /// `placeholder → original` for every swapped span; empty when nothing was
    /// tokenized. Consumed by [`restore`] on the response path; never egresses.
    pub map: Vec<(String, String)>,
}

impl Egress {
    /// The number of sensitive spans swapped for placeholders (what a receipt
    /// records — a count, never the values).
    #[must_use]
    pub fn span_count(&self) -> u32 {
        u32::try_from(self.map.len()).unwrap_or(u32::MAX)
    }

    /// Pass the messages through unchanged (local dispatch, or no detector hit).
    fn identity(messages: &[ChatMessage]) -> Self {
        Self {
            messages: messages.to_vec(),
            map: Vec::new(),
        }
    }
}

/// A placeholder for the nth detected span of a given kind. The index makes each
/// placeholder unique within a request so [`restore`] is unambiguous.
fn placeholder(kind: &str, n: usize) -> String {
    format!("[AOG:{kind}:{n}]")
}

/// Tokenize sensitive spans for cloud egress. Returns the originals unchanged when
/// `target_cloud` is false (on-prem never needs tokenization) or when no spans are
/// detected. Detection uses `detector`; the swap is reversible via the returned map.
#[must_use]
pub fn egress(detector: &PhiDetector, target_cloud: bool, messages: &[ChatMessage]) -> Egress {
    if !target_cloud {
        return Egress::identity(messages);
    }
    let mut map: Vec<(String, String)> = Vec::new();
    let messages = messages
        .iter()
        .map(|m| ChatMessage {
            role: m.role,
            content: tokenize_text(detector, &m.content, &mut map),
        })
        .collect();
    Egress { messages, map }
}

/// Swap each detected span in `text` for a unique placeholder, recording
/// `(placeholder, original)` into `map`. Spans are replaced from the end so earlier
/// byte offsets stay valid; a malformed/out-of-range span is skipped, never panics.
fn tokenize_text(detector: &PhiDetector, text: &str, map: &mut Vec<(String, String)>) -> String {
    let report = detector.scan(text);
    if report.hits.is_empty() {
        return text.to_string();
    }
    let mut hits = report.hits.clone();
    hits.sort_by_key(|h| std::cmp::Reverse(h.span.0));
    let mut out = String::from(text);
    for hit in &hits {
        let (start, end) = hit.span;
        if end > out.len() || !out.is_char_boundary(start) || !out.is_char_boundary(end) {
            continue;
        }
        let ph = placeholder(hit.identifier.as_str(), map.len() + 1);
        let original = out[start..end].to_string();
        out.replace_range(start..end, &ph);
        map.push((ph, original));
    }
    out
}

/// Restore original spans in `text` (a model response) from the egress `map`,
/// inside the trust boundary. A no-op when the map is empty.
#[must_use]
pub fn restore(text: &str, map: &[(String, String)]) -> String {
    if map.is_empty() {
        return text.to_string();
    }
    let mut out = text.to_string();
    for (ph, original) in map {
        out = out.replace(ph.as_str(), original);
    }
    out
}

/// Tag a response with the G8 egress-tokenization count (`x-aog-tokenized`) when
/// any spans were tokenized — the observable half of "both events receipted",
/// alongside the receipt's `tokenized_spans`. Mirrors the `x-aog-*` header surface.
#[must_use]
pub fn tag(mut resp: Response, spans: u32) -> Response {
    if spans > 0
        && let Ok(v) = HeaderValue::from_str(&spans.to_string())
    {
        resp.headers_mut().insert("x-aog-tokenized", v);
    }
    resp
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::Role;

    fn detector() -> PhiDetector {
        PhiDetector::baseline()
    }

    fn user(content: &str) -> ChatMessage {
        ChatMessage {
            role: Role::User,
            content: content.to_string(),
        }
    }

    #[test]
    fn local_dispatch_is_identity() {
        // target_cloud = false → never tokenized, even with PHI present.
        let e = egress(&detector(), false, &[user("SSN 123-45-6789")]);
        assert_eq!(e.messages[0].content, "SSN 123-45-6789");
        assert_eq!(e.span_count(), 0);
    }

    #[test]
    fn benign_cloud_text_untouched() {
        let e = egress(&detector(), true, &[user("What is the capital of France?")]);
        assert_eq!(e.messages[0].content, "What is the capital of France?");
        assert_eq!(e.span_count(), 0);
    }

    #[test]
    fn cloud_egress_swaps_placeholders_and_hides_the_raw_span() {
        let e = egress(&detector(), true, &[user("patient SSN 123-45-6789 please")]);
        let sent = &e.messages[0].content;
        assert!(
            !sent.contains("123-45-6789"),
            "the cloud provider must never see the raw SSN: {sent}"
        );
        assert!(
            sent.contains("[AOG:ssn:"),
            "a placeholder is substituted: {sent}"
        );
        assert_eq!(e.span_count(), 1);
        // surrounding text is preserved.
        assert!(sent.starts_with("patient "));
        assert!(sent.ends_with(" please"));
    }

    #[test]
    fn response_detokenizes_inside_the_boundary() {
        let e = egress(&detector(), true, &[user("email alice@example.com now")]);
        // The model echoes the placeholder it was given.
        let ph = &e.map[0].0;
        let model_reply = format!("I will contact {ph} shortly.");
        let restored = restore(&model_reply, &e.map);
        assert_eq!(restored, "I will contact alice@example.com shortly.");
    }

    #[test]
    fn multiple_spans_get_distinct_reversible_placeholders() {
        let e = egress(
            &detector(),
            true,
            &[user("SSN 123-45-6789 and SSN 987-65-4321")],
        );
        assert_eq!(e.span_count(), 2, "two distinct spans");
        let phs: Vec<&String> = e.map.iter().map(|(p, _)| p).collect();
        assert_ne!(phs[0], phs[1], "each span gets a unique placeholder");
        // Both restore correctly, in any order they appear in the reply.
        let reply = format!("{} then {}", e.map[1].0, e.map[0].0);
        let restored = restore(&reply, &e.map);
        assert!(restored.contains("987-65-4321"));
        assert!(restored.contains("123-45-6789"));
        assert!(!restored.contains("[AOG:"));
    }

    #[test]
    fn spans_across_messages_share_one_map() {
        let e = egress(
            &detector(),
            true,
            &[user("SSN 123-45-6789"), user("email a@b.co")],
        );
        assert_eq!(e.span_count(), 2, "spans from both messages are collected");
        assert!(!e.messages[0].content.contains("123-45-6789"));
        assert!(!e.messages[1].content.contains("a@b.co"));
    }

    #[test]
    fn restore_is_noop_without_a_map() {
        assert_eq!(restore("nothing to restore", &[]), "nothing to restore");
    }
}
