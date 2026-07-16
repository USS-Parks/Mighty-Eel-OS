//! G8 gate — tokenization on egress.
//!
//! Exercises the exact public `tokenize` API the OpenAI (G3) and Anthropic (G4)
//! surfaces run around a cloud dispatch: a cloud-bound request has its sensitive
//! spans swapped for placeholders (**the cloud provider sees placeholders only**),
//! the model response is **detokenized inside the boundary**, and the event is
//! **receipted** (span count on a metadata-only, chain-verifying receipt). The
//! placeholder→original map never leaves the gateway.
//!
//! The full handler wiring (auth → classify → policy → G8 → dispatch → receipt) is
//! additionally exercised by the env-gated `openai_surface`/`anthropic_surface`
//! suites against live OpenBao; this suite pins the G8 contract with no infra.

use aog_gateway::meter::{GatewayReceipt, ReceiptLedger};
use aog_gateway::provider::{ChatMessage, Role};
use aog_gateway::tokenize;
use mai_compliance::PhiDetector;

fn user(content: &str) -> ChatMessage {
    ChatMessage {
        role: Role::User,
        content: content.to_string(),
    }
}

/// The egress → dispatch → restore round-trip a surface performs: the cloud
/// provider sees placeholders only, and the client gets the original back.
#[test]
fn cloud_provider_sees_placeholders_and_response_is_detokenized() {
    let detector = PhiDetector::baseline();
    let egress = tokenize::egress(
        &detector,
        true, // target_cloud
        &[user("Patient SSN 123-45-6789 — summarize the chart")],
    );

    // What actually egresses to the cloud provider.
    let dispatched = &egress.messages[0].content;
    assert!(
        !dispatched.contains("123-45-6789"),
        "the cloud provider must never see the raw SSN: {dispatched}"
    );
    assert!(
        dispatched.contains("[AOG:ssn:"),
        "a placeholder egresses instead: {dispatched}"
    );
    assert_eq!(egress.span_count(), 1);

    // The model echoes the placeholder it was handed; the gateway detokenizes on
    // return, inside the boundary — the client sees the original span restored.
    let model_reply = format!("Summary for {} is attached.", egress.map[0].0);
    let restored = tokenize::restore(&model_reply, &egress.map);
    assert_eq!(restored, "Summary for 123-45-6789 is attached.");
    assert!(!restored.contains("[AOG:"));
}

/// An on-prem (local) dispatch is never tokenized — there is no egress to protect.
#[test]
fn local_dispatch_is_never_tokenized() {
    let detector = PhiDetector::baseline();
    let egress = tokenize::egress(&detector, false, &[user("SSN 123-45-6789")]);
    assert_eq!(egress.messages[0].content, "SSN 123-45-6789");
    assert_eq!(egress.span_count(), 0);
}

/// The tokenization is receipted: the count lands on a metadata-only receipt, the
/// chain verifies, and a zero-count receipt stays byte-identical to its pre-G8
/// shape (the field is omitted), so existing chains are undisturbed.
#[test]
fn tokenization_is_receipted_and_backward_compatible() {
    let mut ledger = ReceiptLedger::new();
    let head0 = ledger.append(receipt("rcpt-0", 0));
    let head1 = ledger.append(receipt("rcpt-1", 2));
    assert_ne!(head0, head1);
    assert!(
        ledger.verify(),
        "the chain verifies with a tokenized receipt"
    );
    assert_eq!(
        ledger.receipts()[1].tokenized_spans,
        2,
        "the span count is receipted"
    );

    // Metadata-only: the receipt carries a count, never the placeholder map or the
    // raw values. A zero-count receipt omits the field entirely.
    let json0 = serde_json::to_string(&ledger.receipts()[0]).unwrap();
    let json1 = serde_json::to_string(&ledger.receipts()[1]).unwrap();
    assert!(
        !json0.contains("tokenized_spans"),
        "a zero-count receipt omits the field (unchanged pre-G8 shape): {json0}"
    );
    assert!(
        json1.contains("\"tokenized_spans\":2"),
        "a tokenized receipt records the count: {json1}"
    );
}

fn receipt(request_id: &str, tokenized_spans: u32) -> GatewayReceipt {
    GatewayReceipt {
        request_id: request_id.to_string(),
        at: "2026-07-03T00:00:00Z".to_string(),
        tenant_id: "tenant-a".to_string(),
        subject_hash: "h".to_string(),
        token_id: "tok".to_string(),
        provider: "openai".to_string(),
        model: "gpt-4o-mini".to_string(),
        route: "cloud_allowed".to_string(),
        classification: "restricted".to_string(),
        policy: "allow".to_string(),
        input_tokens: 12,
        output_tokens: 20,
        spend_cents: 3,
        usage_reconciliation: None,
        model_weights_digest: None,
        workflow_id: None,
        tokenized_spans,
    }
}
