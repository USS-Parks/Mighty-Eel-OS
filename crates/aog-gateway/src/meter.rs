//! Metering + receipts (G7).
//!
//! Every completed request emits a **metadata-only** WSF receipt into an
//! append-only BLAKE3 hash chain (`fabric-proof`, the same primitive `wsf-seal`
//! and `wsf-ledger` use), and `aog-meter` aggregates spend per
//! **tenant / provider / model / task** (`workflow_id`). The chain verifies
//! off-host; the receipts carry the spend, provider, token counts, and — for a
//! **local** model — a weights digest (cloud is named by provider+model).
//!
//! Provider usage is untrusted evidence. Both completion paths compute a local
//! lower-bound estimate and reconcile each field to the greater of that estimate
//! and the provider report. A missing, zero, or implausibly low positive report
//! therefore cannot suppress accounting. Streams settle through [`StreamMeter`]
//! when the SSE generator drops — terminal frame, provider error, or client
//! disconnect alike. Every call is metered.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use axum::http::HeaderMap;
use chrono::Utc;
use fabric_proof::{ChainLink, GENESIS_HASH, canonical_hash, chain_link, verify_chain};
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::ResolvedContext;
use crate::app::DispatchReservation;
use crate::provider::{StreamChunk, Usage};
use crate::route::{GatewayRoute, route_header};

/// A per-model price (cents per 1000 tokens, input / output).
#[derive(Debug, Clone, Copy)]
pub struct Price {
    pub input_per_1k_cents: u64,
    pub output_per_1k_cents: u64,
}

/// Cost model: a default plus per-model overrides. A `local` provider is free
/// (on-prem compute); cloud providers bill per token.
#[derive(Debug, Clone, Default)]
pub struct PriceBook {
    default: Option<Price>,
    per_model: HashMap<String, Price>,
}

impl PriceBook {
    /// A baseline book: a modest default cloud price + a couple of well-known
    /// per-model prices. Everything is configurable; these are just non-zero.
    #[must_use]
    pub fn baseline() -> Self {
        let mut per_model = HashMap::new();
        per_model.insert(
            "gpt-4o-mini".to_string(),
            Price {
                input_per_1k_cents: 15,
                output_per_1k_cents: 60,
            },
        );
        per_model.insert(
            "claude-3-5-sonnet".to_string(),
            Price {
                input_per_1k_cents: 300,
                output_per_1k_cents: 1500,
            },
        );
        Self {
            default: Some(Price {
                input_per_1k_cents: 50,
                output_per_1k_cents: 150,
            }),
            per_model,
        }
    }

    /// Cost in cents for a call. Local providers are free; cloud bills per token.
    #[must_use]
    pub fn cost(&self, provider: &str, model: &str, input: u32, output: u32) -> u64 {
        if provider == "local" {
            return 0;
        }
        let Some(p) = self.per_model.get(model).copied().or(self.default) else {
            return 0;
        };
        (u64::from(input) * p.input_per_1k_cents / 1000)
            + (u64::from(output) * p.output_per_1k_cents / 1000)
    }
}

/// True for a zero span count — lets a non-tokenized receipt serialize (and hash)
/// exactly as it did before G8, so existing receipt chains are unaffected.
#[allow(clippy::trivially_copy_pass_by_ref)]
fn is_zero(n: &u32) -> bool {
    *n == 0
}

/// A metadata-only receipt for one completed request.
#[derive(Debug, Clone, Serialize)]
pub struct GatewayReceipt {
    pub request_id: String,
    pub at: String,
    pub tenant_id: String,
    pub subject_hash: String,
    pub token_id: String,
    pub provider: String,
    pub model: String,
    pub route: String,
    pub classification: String,
    pub policy: String,
    pub input_tokens: u32,
    pub output_tokens: u32,
    pub spend_cents: u64,
    /// G8: the local lower bound, untrusted provider evidence, and authoritative
    /// usage used for budget/spend settlement. Optional only so historical and
    /// hand-built pre-G8 receipt fixtures retain their byte shape.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage_reconciliation: Option<UsageReconciliation>,
    /// Set for a local model (the cloud identity is provider+model).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_weights_digest: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workflow_id: Option<String>,
    /// G8: count of sensitive spans tokenized on cloud egress for this request
    /// (0 = none; omitted when zero so a non-tokenized receipt is byte-identical
    /// to its pre-G8 shape). The placeholder→original map itself never leaves the
    /// gateway and is never receipted — only this count is.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub tokenized_spans: u32,
}

/// Auditable reconciliation of provider usage against the gateway's local
/// lower bound. `final_usage` is authoritative for receipts, spend, and budget
/// settlement; `provider_reported` remains evidence rather than authority.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct UsageReconciliation {
    pub local_estimate: Usage,
    pub provider_reported: Usage,
    pub final_usage: Usage,
}

/// Estimate tokens from UTF-8 bytes at the gateway's conservative four-byte
/// policy, rounding up and saturating instead of wrapping on extreme inputs.
#[must_use]
pub fn estimate_text_tokens(text: &str) -> u32 {
    estimate_output_tokens(text.len())
}

/// Estimate output tokens from an observed byte count. Empty output is zero;
/// every non-empty partial group of four bytes counts as one token.
#[must_use]
pub fn estimate_output_tokens(bytes: usize) -> u32 {
    let tokens = bytes.saturating_add(3) / 4;
    u32::try_from(tokens).unwrap_or(u32::MAX)
}

/// Reconcile provider-controlled counts with a locally observed lower bound.
/// A high report remains visible and is conservatively charged; a missing,
/// zero, low, or contradictory report cannot reduce either field below local.
#[must_use]
pub fn reconcile_usage(local_estimate: Usage, provider_reported: Usage) -> UsageReconciliation {
    UsageReconciliation {
        local_estimate,
        provider_reported,
        final_usage: Usage {
            input_tokens: local_estimate
                .input_tokens
                .max(provider_reported.input_tokens),
            output_tokens: local_estimate
                .output_tokens
                .max(provider_reported.output_tokens),
        },
    }
}

/// Reconcile one non-stream completion from the request and response text that
/// actually crossed the provider boundary. The request side is at least one
/// token; an empty response is allowed to remain zero.
#[must_use]
pub fn reconcile_completion_usage(
    input_text: &str,
    output_text: &str,
    provider_reported: Usage,
) -> UsageReconciliation {
    reconcile_usage(
        Usage {
            input_tokens: estimate_text_tokens(input_text).max(1),
            output_tokens: estimate_text_tokens(output_text),
        },
        provider_reported,
    )
}

/// Aggregated spend for one (tenant, provider, model, task) group.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct TaskUsage {
    pub tenant_id: String,
    pub provider: String,
    pub model: String,
    pub workflow_id: Option<String>,
    pub calls: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub spend_cents: u64,
}

/// Append-only receipt ledger: a BLAKE3 hash chain (`fabric-proof`) over
/// metadata-only receipts, mirroring `wsf-seal`'s `ReceiptChain`.
#[derive(Debug)]
pub struct ReceiptLedger {
    links: Vec<ChainLink>,
    receipts: Vec<GatewayReceipt>,
    last_hash: [u8; 32],
    /// Chain-intact flag, maintained in O(1) by `append` (each link extends the
    /// running head by construction). The unauthenticated `/v1/status` read
    /// consults this instead of re-walking the chain from genesis under the
    /// receipt lock the completion path needs, so an unauthenticated status
    /// flood cannot force an O(n) walk while holding that lock. The full O(n)
    /// re-derivation stays available via [`verify`](Self::verify).
    verified: bool,
}

impl Default for ReceiptLedger {
    fn default() -> Self {
        Self::new()
    }
}

impl ReceiptLedger {
    #[must_use]
    pub fn new() -> Self {
        Self {
            links: Vec::new(),
            receipts: Vec::new(),
            last_hash: GENESIS_HASH,
            verified: true,
        }
    }

    /// Append a receipt; returns the new chain head (hex).
    pub fn append(&mut self, receipt: GatewayReceipt) -> String {
        let value = serde_json::to_value(&receipt).expect("receipt serializes");
        let entry_hash = canonical_hash(&value).expect("canonical hash of receipt");
        let previous_hash = self.last_hash;
        self.links.push(ChainLink {
            previous_hash,
            entry_hash,
        });
        self.last_hash = chain_link(&previous_hash, &entry_hash);
        self.receipts.push(receipt);
        // O(1) maintenance of the chain-intact invariant: the link just pushed
        // extends the prior head, so the chain remains verified. A future change
        // that broke append's chaining would flip this; the full walk in
        // `verify` remains the tamper check.
        self.verified = self.verified
            && self
                .links
                .last()
                .is_some_and(|l| l.previous_hash == previous_hash);
        hex::encode(self.last_hash)
    }

    /// The next request id (monotonic within this ledger).
    #[must_use]
    pub fn next_id(&self) -> String {
        format!("rcpt-{}", self.receipts.len())
    }

    /// Verify the receipt chain is unbroken from genesis — the full O(n) walk.
    /// Use for an explicit tamper audit; the read-hot `/v1/status` path reads the
    /// O(1) [`chain_verified`](Self::chain_verified) instead.
    #[must_use]
    pub fn verify(&self) -> bool {
        verify_chain(&self.links).is_ok()
    }

    /// The O(1) chain-intact flag maintained by `append`. The unauthenticated
    /// `/v1/status` endpoint reads this so a status flood cannot force an O(n)
    /// chain walk while holding the receipt lock the completion path contends
    /// for. For a full genesis-to-head re-derivation, use
    /// [`verify`](Self::verify).
    #[must_use]
    pub fn chain_verified(&self) -> bool {
        self.verified
    }

    #[must_use]
    pub fn head_hex(&self) -> String {
        hex::encode(self.last_hash)
    }

    #[must_use]
    pub fn receipts(&self) -> &[GatewayReceipt] {
        &self.receipts
    }

    /// Aggregate spend per (tenant, provider, model, task), sorted for stability.
    #[must_use]
    pub fn aggregate(&self) -> Vec<TaskUsage> {
        let mut groups: HashMap<(String, String, String, String), TaskUsage> = HashMap::new();
        for r in &self.receipts {
            let wf = r.workflow_id.clone().unwrap_or_default();
            let key = (r.tenant_id.clone(), r.provider.clone(), r.model.clone(), wf);
            let e = groups.entry(key).or_insert_with(|| TaskUsage {
                tenant_id: r.tenant_id.clone(),
                provider: r.provider.clone(),
                model: r.model.clone(),
                workflow_id: r.workflow_id.clone(),
                calls: 0,
                input_tokens: 0,
                output_tokens: 0,
                spend_cents: 0,
            });
            e.calls += 1;
            e.input_tokens += u64::from(r.input_tokens);
            e.output_tokens += u64::from(r.output_tokens);
            e.spend_cents += r.spend_cents;
        }
        let mut out: Vec<TaskUsage> = groups.into_values().collect();
        out.sort_by(|a, b| {
            (&a.tenant_id, &a.provider, &a.model, &a.workflow_id).cmp(&(
                &b.tenant_id,
                &b.provider,
                &b.model,
                &b.workflow_id,
            ))
        });
        out
    }

    /// Aggregate spend for a single tenant — the caller's own view. Tenant scope
    /// is mandatory for ordinary callers so one tenant cannot read another's
    /// provider/model/spend estate.
    #[must_use]
    pub fn aggregate_for_tenant(&self, tenant_id: &str) -> Vec<TaskUsage> {
        self.aggregate()
            .into_iter()
            .filter(|u| u.tenant_id == tenant_id)
            .collect()
    }

    /// Total cost (cents) for one task across every call in the chain.
    #[must_use]
    pub fn cost_per_task(&self, workflow_id: &str) -> u64 {
        self.receipts
            .iter()
            .filter(|r| r.workflow_id.as_deref() == Some(workflow_id))
            .map(|r| r.spend_cents)
            .sum()
    }
}

/// The context for recording one completed request.
pub struct Completion<'a> {
    pub ctx: &'a ResolvedContext,
    pub provider: &'a str,
    pub model: &'a str,
    pub route: &'a GatewayRoute,
    pub allowed_cloud: bool,
    pub usage: Usage,
    pub usage_reconciliation: UsageReconciliation,
    pub workflow_id: Option<String>,
    /// G8: sensitive spans tokenized on cloud egress (0 when local or nothing hit).
    pub tokenized_spans: u32,
}

/// The caller-supplied task id from the `x-aog-workflow` header, if present —
/// what `aog-meter` aggregates cost-per-task by.
#[must_use]
pub fn workflow_from(headers: &HeaderMap) -> Option<String> {
    headers
        .get("x-aog-workflow")
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// A stand-in weights digest for a local model (`sha256(model id)`); the real
/// weights hash comes from the model loader. `None` for cloud providers.
#[must_use]
pub fn local_weights_digest(provider: &str, model: &str) -> Option<String> {
    (provider == "local")
        .then(|| format!("sha256:{}", hex::encode(Sha256::digest(model.as_bytes()))))
}

/// Build + append a receipt for a completed request; returns the chain head.
pub fn record(ledger: &Mutex<ReceiptLedger>, prices: &PriceBook, c: &Completion) -> String {
    let spend = prices.cost(
        c.provider,
        c.model,
        c.usage.input_tokens,
        c.usage.output_tokens,
    );
    // Recover from a poisoned lock instead of panicking: a prior panic in another
    // request must not wedge the per-completion receipt path (audit D4). The region
    // is a single next_id + append, so a recovered guard is consistent.
    let mut guard = ledger.lock().unwrap_or_else(|e| e.into_inner());
    let receipt = GatewayReceipt {
        request_id: guard.next_id(),
        at: Utc::now().to_rfc3339(),
        tenant_id: c.ctx.tenant_id.clone(),
        subject_hash: c.ctx.token.subject_hash.clone(),
        token_id: c.ctx.token.token_id.clone(),
        provider: c.provider.to_string(),
        model: c.model.to_string(),
        route: route_header(c.route.route).to_string(),
        classification: c.route.classification.clone(),
        policy: if c.allowed_cloud { "allow" } else { "deny" }.to_string(),
        input_tokens: c.usage.input_tokens,
        output_tokens: c.usage.output_tokens,
        spend_cents: spend,
        usage_reconciliation: Some(c.usage_reconciliation),
        model_weights_digest: local_weights_digest(c.provider, c.model),
        workflow_id: c.workflow_id.clone(),
        tokenized_spans: c.tokenized_spans,
    };
    guard.append(receipt)
}

/// Accounting guard for one **streamed** completion: the SSE
/// generator owns it, feeds every frame through [`observe`](Self::observe), and
/// settlement — receipt (G7) + budget decrement (G9) — happens exactly once in
/// `Drop`. Keying settlement to the guard's drop (not a terminal frame) means a
/// client that disconnects mid-stream, a provider that errors mid-stream, and a
/// clean `[DONE]` all meter alike: an early hang-up cannot dodge the budget.
///
/// Construct it literally (like [`Completion`]) in the surface handler with the
/// accumulators zeroed.
pub struct StreamMeter {
    pub receipts: Arc<Mutex<ReceiptLedger>>,
    pub prices: Arc<PriceBook>,
    pub gateway: Arc<crate::Gateway>,
    pub ctx: ResolvedContext,
    pub provider: String,
    pub model: String,
    pub route: GatewayRoute,
    pub allowed_cloud: bool,
    pub workflow_id: Option<String>,
    /// Conservative local lower bound for request input usage.
    pub input_estimate: u32,
    /// Provider-reported usage, folded per-field across frames (reports are
    /// cumulative; Anthropic splits input onto `message_start` and output onto
    /// `message_delta`, OpenAI reports both on one terminal usage frame).
    pub reported: Usage,
    /// Accumulated delta byte length for the output-side local lower bound.
    pub delta_bytes: usize,
    /// Authority reserved before provider stream creation. Drop reconciles the
    /// observed/fallback usage and releases the unused portion exactly once.
    pub(crate) reservation: Option<DispatchReservation>,
}

impl StreamMeter {
    /// Revalidate current revocation before one more streamed provider frame is
    /// released to the client.
    pub async fn authorize_continuation(&self) -> Result<(), crate::GatewayError> {
        self.gateway
            .authorize_current(&self.ctx.token, Utc::now())
            .await
    }

    /// Fold one stream frame into the running account.
    pub fn observe(&mut self, chunk: &StreamChunk) {
        if let Some(u) = chunk.usage {
            self.reported.input_tokens = self.reported.input_tokens.max(u.input_tokens);
            self.reported.output_tokens = self.reported.output_tokens.max(u.output_tokens);
        }
        self.delta_bytes = self.delta_bytes.saturating_add(chunk.delta.len());
    }
}

impl Drop for StreamMeter {
    /// Settle the streamed call, however the stream ended: append the receipt
    /// and accrue spend against the token's attenuation lineage (T5), exactly
    /// as the non-stream path does. Provider usage is evidence only; the final
    /// per-field count cannot fall below the request/delta lower bound.
    fn drop(&mut self) {
        let reconciliation = reconcile_usage(
            Usage {
                input_tokens: self.input_estimate,
                output_tokens: estimate_output_tokens(self.delta_bytes),
            },
            self.reported,
        );
        let usage = reconciliation.final_usage;
        if let Some(reservation) = self.reservation.take() {
            let _ = reservation.commit_usage(fabric_token::spend::Spent {
                tokens: u64::from(usage.input_tokens)
                    .saturating_add(u64::from(usage.output_tokens)),
                usd_cents: self.prices.cost(
                    &self.provider,
                    &self.model,
                    usage.input_tokens,
                    usage.output_tokens,
                ),
                tool_calls: 1,
            });
        }
        record(
            &self.receipts,
            &self.prices,
            &Completion {
                ctx: &self.ctx,
                provider: &self.provider,
                model: &self.model,
                route: &self.route,
                allowed_cloud: self.allowed_cloud,
                usage,
                usage_reconciliation: reconciliation,
                workflow_id: self.workflow_id.clone(),
                // The stream path refuses a cloud dispatch that would need span
                // tokenization, so a streamed receipt never carries spans.
                tokenized_spans: 0,
            },
        );
        self.gateway.record_spend(
            fabric_token::lineage_key(&self.ctx.token),
            u64::from(usage.input_tokens) + u64::from(usage.output_tokens),
            self.prices.cost(
                &self.provider,
                &self.model,
                usage.input_tokens,
                usage.output_tokens,
            ),
            1,
        );
    }
}

/// Shared fixtures for the streamed-metering tests here and in the surface
/// modules — compiled only for this crate's own test builds.
#[cfg(test)]
pub(crate) mod testkit {
    use std::sync::{Arc, Mutex};

    use fabric_contracts::{
        Attenuation, Budget, Classification, RevocationStatus, Route, Signature, TrustToken,
    };
    use fabric_token::spend::LocalSpendLedger;
    use wsf_bridge::{OpenBaoAuth, OpenBaoConfig};

    use super::{PriceBook, ReceiptLedger, StreamMeter};
    use crate::provider::{StreamChunk, Usage};
    use crate::route::{GatewayRoute, RouteSource};
    use crate::{Gateway, GatewayConfig, ResolvedContext};

    pub(crate) fn budgeted_token(id: &str) -> TrustToken {
        TrustToken {
            token_id: id.to_string(),
            issued_at: "2026-07-03T00:00:00Z".into(),
            expires_at: "2099-01-01T00:00:00Z".into(),
            issuer: "wsf-bridge".into(),
            trust_bundle_version: "2026.07.v2".into(),
            tenant_id: "tenant-a".into(),
            subject_id: None,
            subject_hash: "hmac:abc".into(),
            service_identity: None,
            identity_id: None,
            roles: vec![],
            compliance_scopes: vec![],
            allowed_routes: vec![],
            allowed_models: vec![],
            max_data_classification: Classification::Restricted,
            country: None,
            person_type: None,
            offline_mode: false,
            revocation_status: RevocationStatus::Valid,
            budget: Some(Budget {
                token_cap: 20,
                ..Default::default()
            }),
            attenuation: Attenuation::default(),
            signature: Signature {
                alg: String::new(),
                key_id: String::new(),
                value: String::new(),
            },
        }
    }

    pub(crate) fn test_route() -> GatewayRoute {
        GatewayRoute {
            route: Route::CloudAllowed,
            classification: "public".into(),
            reason: "test".into(),
            source: RouteSource::Classified,
            denied: false,
        }
    }

    /// A gateway that never reaches OpenBao (nothing resolves in these tests);
    /// only its spend ledger is exercised, via the injected handle.
    pub(crate) fn offline_gateway(spend: Arc<LocalSpendLedger>) -> Arc<Gateway> {
        Arc::new(
            Gateway::new(
                OpenBaoAuth::new(OpenBaoConfig::new("http://127.0.0.1:1", "r", "s"))
                    .expect("offline openbao client builds"),
                GatewayConfig {
                    token_public_key: vec![],
                    virtual_key_kv_prefix: "kv/data/aog/virtual-keys".into(),
                },
            )
            .with_spend_ledger(spend),
        )
    }

    /// A [`StreamMeter`] over fresh ledgers: lineage key `tok-stream`, provider
    /// `openai`, model `gpt-4o-mini`, input estimate 25.
    pub(crate) fn stream_meter(
        receipts: &Arc<Mutex<ReceiptLedger>>,
        spend: &Arc<LocalSpendLedger>,
    ) -> StreamMeter {
        StreamMeter {
            receipts: receipts.clone(),
            prices: Arc::new(PriceBook::baseline()),
            gateway: offline_gateway(spend.clone()),
            ctx: ResolvedContext {
                token: budgeted_token("tok-stream"),
                tenant_id: "tenant-a".into(),
            },
            provider: "openai".into(),
            model: "gpt-4o-mini".into(),
            route: test_route(),
            allowed_cloud: true,
            workflow_id: Some("task-s".into()),
            input_estimate: 25,
            reported: Usage::default(),
            delta_bytes: 0,
            reservation: None,
        }
    }

    pub(crate) fn delta(text: &str) -> StreamChunk {
        StreamChunk {
            delta: text.to_string(),
            done: false,
            finish_reason: None,
            usage: None,
        }
    }

    pub(crate) fn usage_frame(input: u32, output: u32) -> StreamChunk {
        StreamChunk {
            delta: String::new(),
            done: false,
            finish_reason: None,
            usage: Some(Usage {
                input_tokens: input,
                output_tokens: output,
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::testkit::{budgeted_token, delta, stream_meter, test_route, usage_frame};
    use super::*;
    use fabric_contracts::Budget;
    use fabric_token::spend::{LocalSpendLedger, SpendLedger};

    use crate::budget_exhausted;

    fn receipt(wf: &str, provider: &str, spend: u64) -> GatewayReceipt {
        GatewayReceipt {
            request_id: String::new(),
            at: String::new(),
            tenant_id: "tenant-a".to_string(),
            subject_hash: "h".to_string(),
            token_id: "t".to_string(),
            provider: provider.to_string(),
            model: "m".to_string(),
            route: "cloud_allowed".to_string(),
            classification: "public".to_string(),
            policy: "allow".to_string(),
            input_tokens: 10,
            output_tokens: 5,
            spend_cents: spend,
            usage_reconciliation: None,
            model_weights_digest: None,
            workflow_id: Some(wf.to_string()),
            tokenized_spans: 0,
        }
    }

    #[test]
    fn poisoned_receipt_ledger_recovers_instead_of_wedging() {
        // D4: a panic while another request held the receipt-ledger lock must not
        // wedge the ledger for every later request. The hot-path sites now recover
        // the guard via `unwrap_or_else(|e| e.into_inner())` instead of `.expect(..)`,
        // and the locked region (next_id + append) leaves consistent state on unwind.
        let ledger = Mutex::new(ReceiptLedger::new());
        ledger
            .lock()
            .unwrap()
            .append(receipt("task-1", "openai", 7));

        // Poison the lock: panic while the guard is held.
        let poisoned = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _g = ledger.lock().unwrap();
            panic!("simulated request panic under the receipt lock");
        }));
        assert!(poisoned.is_err(), "the closure panicked as set up");
        assert!(
            ledger.is_poisoned(),
            "precondition: the lock is now poisoned"
        );

        // The hardened access pattern still yields a usable guard, and the
        // pre-panic receipt is intact — no wedge, no data loss.
        let guard = ledger.lock().unwrap_or_else(|e| e.into_inner());
        assert!(guard.verify(), "recovered chain still verifies");
        assert_eq!(guard.receipts().len(), 1, "the pre-panic receipt survived");
        assert_eq!(guard.cost_per_task("task-1"), 7);
    }

    #[test]
    fn cost_per_task_sums_a_multi_call_chain_and_chain_verifies() {
        let mut led = ReceiptLedger::new();
        led.append(receipt("task-1", "openai", 12));
        led.append(receipt("task-1", "openai", 8));
        led.append(receipt("task-2", "anthropic", 100));
        assert_eq!(
            led.cost_per_task("task-1"),
            20,
            "cost-per-task sums the chain"
        );
        assert_eq!(led.cost_per_task("task-2"), 100);
        assert!(led.verify(), "receipt chain verifies");

        let agg = led.aggregate();
        assert_eq!(agg.len(), 2, "two (tenant,provider,model,task) groups");
        let t1 = agg
            .iter()
            .find(|a| a.workflow_id.as_deref() == Some("task-1"))
            .unwrap();
        assert_eq!(t1.calls, 2);
        assert_eq!(t1.spend_cents, 20);
    }

    #[test]
    fn aggregate_for_tenant_isolates_tenants() {
        let mut a = receipt("task-1", "openai", 10);
        a.tenant_id = "tenant-a".to_string();
        let mut b = receipt("task-2", "anthropic", 999);
        b.tenant_id = "tenant-b".to_string();
        let mut led = ReceiptLedger::new();
        led.append(a);
        led.append(b);

        let a_view = led.aggregate_for_tenant("tenant-a");
        assert_eq!(a_view.len(), 1, "tenant-a sees only its own group");
        assert!(a_view.iter().all(|u| u.tenant_id == "tenant-a"));
        assert!(
            a_view.iter().all(|u| u.provider != "anthropic"),
            "tenant-a must not learn tenant-b's provider/spend"
        );
        assert_eq!(a_view.iter().map(|u| u.spend_cents).sum::<u64>(), 10);
        // Tenant A's scoped view is invariant to tenant B's activity.
        assert_eq!(
            led.aggregate().len(),
            2,
            "global view still has both groups"
        );
    }

    #[test]
    fn tampering_breaks_the_chain() {
        let mut led = ReceiptLedger::new();
        led.append(receipt("t", "openai", 1));
        led.append(receipt("t", "openai", 2));
        assert!(led.verify());
        // Corrupt a link's entry hash.
        led.links[0].entry_hash[0] ^= 0xff;
        assert!(!led.verify(), "a tampered chain fails verification");
    }

    #[test]
    fn chain_verified_flag_matches_full_walk_on_a_long_chain() {
        // The O(1) flag the unauthenticated `/v1/status` read consults agrees
        // with the full O(n) walk across a long append-only chain — so status
        // never has to walk the chain under the receipt lock.
        let mut led = ReceiptLedger::new();
        assert!(led.chain_verified(), "empty chain is verified");
        for i in 0..1000 {
            led.append(receipt("t", "openai", i));
        }
        assert!(led.chain_verified(), "flag stays true across a long chain");
        assert_eq!(
            led.chain_verified(),
            led.verify(),
            "the O(1) status flag matches the full O(n) walk"
        );
    }

    #[test]
    fn pricebook_local_is_free_cloud_bills() {
        let pb = PriceBook::baseline();
        assert_eq!(pb.cost("local", "llama3", 1000, 1000), 0, "local is free");
        // gpt-4o-mini: 15/1k in + 60/1k out over 1000/500 → 15 + 30 = 45.
        assert_eq!(pb.cost("openai", "gpt-4o-mini", 1000, 500), 45);
        // local weights digest is recorded; cloud has none.
        assert!(local_weights_digest("local", "llama3").is_some());
        assert!(local_weights_digest("openai", "gpt-4o-mini").is_none());
    }

    #[test]
    fn authoritative_usage_never_falls_below_the_local_policy() {
        let input = "i".repeat(100);
        let output = "o".repeat(32);
        let local = Usage {
            input_tokens: 25,
            output_tokens: 8,
        };
        let fixtures = [
            ("missing-normalized", Usage::default(), local),
            ("explicit-zero", Usage::default(), local),
            (
                "positive-low",
                Usage {
                    input_tokens: 1,
                    output_tokens: 1,
                },
                local,
            ),
            (
                "provider-high",
                Usage {
                    input_tokens: 1000,
                    output_tokens: 500,
                },
                Usage {
                    input_tokens: 1000,
                    output_tokens: 500,
                },
            ),
            (
                "contradictory-fields",
                Usage {
                    input_tokens: 1,
                    output_tokens: 500,
                },
                Usage {
                    input_tokens: 25,
                    output_tokens: 500,
                },
            ),
        ];

        for (name, provider, expected) in fixtures {
            let reconciled = reconcile_completion_usage(&input, &output, provider);
            assert_eq!(
                reconciled.local_estimate, local,
                "{name}: estimate receipted"
            );
            assert_eq!(
                reconciled.provider_reported, provider,
                "{name}: provider evidence receipted"
            );
            assert_eq!(reconciled.final_usage, expected, "{name}: safe settlement");
        }

        assert_eq!(estimate_text_tokens(""), 0);
        assert_eq!(estimate_text_tokens("a"), 1);
        assert_eq!(estimate_text_tokens("abcde"), 2, "partial groups round up");
    }

    #[test]
    fn non_stream_receipt_records_estimate_provider_and_final_usage() {
        let ledger = Mutex::new(ReceiptLedger::new());
        let prices = PriceBook::baseline();
        let ctx = ResolvedContext {
            token: budgeted_token("tok-non-stream"),
            tenant_id: "tenant-a".to_string(),
        };
        let route = test_route();
        let reconciliation = reconcile_completion_usage(
            &"i".repeat(100),
            &"o".repeat(32),
            Usage {
                input_tokens: 1,
                output_tokens: 1,
            },
        );

        record(
            &ledger,
            &prices,
            &Completion {
                ctx: &ctx,
                provider: "openai",
                model: "gpt-4o-mini",
                route: &route,
                allowed_cloud: true,
                usage: reconciliation.final_usage,
                usage_reconciliation: reconciliation,
                workflow_id: Some("g8-fixture".to_string()),
                tokenized_spans: 0,
            },
        );

        let receipt = ledger.lock().unwrap().receipts()[0].clone();
        assert_eq!(receipt.input_tokens, 25);
        assert_eq!(receipt.output_tokens, 8);
        assert_eq!(
            receipt.usage_reconciliation,
            Some(UsageReconciliation {
                local_estimate: Usage {
                    input_tokens: 25,
                    output_tokens: 8,
                },
                provider_reported: Usage {
                    input_tokens: 1,
                    output_tokens: 1,
                },
                final_usage: Usage {
                    input_tokens: 25,
                    output_tokens: 8,
                },
            })
        );
        assert!(ledger.lock().unwrap().verify(), "receipt chain verifies");
    }

    #[test]
    fn stream_meter_settles_on_drop_without_a_terminal_frame() {
        // A client that hangs up mid-stream (no terminal frame, no usage
        // frame) is still receipted and budget-decremented when the SSE
        // generator drops — an early disconnect cannot dodge the meter. With a
        // usage-silent provider the fallbacks apply: the request-text input
        // estimate + ~4 chars/token of streamed deltas.
        let receipts = Arc::new(Mutex::new(ReceiptLedger::new()));
        let spend = Arc::new(LocalSpendLedger::default());
        let mut meter = stream_meter(&receipts, &spend);
        meter.observe(&delta("hell"));
        meter.observe(&delta("o wo"));
        drop(meter); // simulated disconnect: the stream never finished

        let led = receipts.lock().unwrap();
        assert_eq!(led.receipts().len(), 1, "the aborted stream is receipted");
        let r = &led.receipts()[0];
        assert_eq!(
            r.input_tokens, 25,
            "input falls back to the request estimate"
        );
        assert_eq!(r.output_tokens, 2, "output falls back to delta chars / 4");
        assert!(led.verify(), "receipt chain verifies");

        // 27 tokens accrued against the lineage key: the 20-token cap is now
        // exhausted, so the next pre-flight resolve refuses this key.
        let mut b = Budget {
            token_cap: 20,
            ..Default::default()
        };
        spend.fold("tok-stream", &mut b);
        assert!(
            budget_exhausted(&b),
            "streamed spend crosses the cap → pre-flight refuses"
        );
    }

    #[test]
    fn stream_meter_reconciles_late_split_usage_without_undercharging() {
        // Reports can arrive late and contradict earlier fields (the Anthropic
        // shape splits input and output across frames). Per-field maxima preserve
        // the strongest evidence, then local delta accounting supplies the floor.
        let receipts = Arc::new(Mutex::new(ReceiptLedger::new()));
        let spend = Arc::new(LocalSpendLedger::default());
        let mut meter = stream_meter(&receipts, &spend);
        meter.observe(&usage_frame(1000, 1));
        meter.observe(&delta("0123456789abcdef"));
        meter.observe(&usage_frame(1, 500));
        drop(meter);

        let led = receipts.lock().unwrap();
        let r = &led.receipts()[0];
        assert_eq!(
            r.input_tokens, 1000,
            "reported input wins over the estimate"
        );
        assert_eq!(
            r.output_tokens, 500,
            "reported output wins over delta chars"
        );
        // gpt-4o-mini baseline: 1000×15/1k + 500×60/1k = 45 cents.
        assert_eq!(r.spend_cents, 45);
        let reconciliation = r
            .usage_reconciliation
            .expect("stream receipt carries estimate, evidence, and final usage");
        assert_eq!(
            reconciliation.local_estimate,
            Usage {
                input_tokens: 25,
                output_tokens: 4,
            }
        );
        assert_eq!(
            reconciliation.provider_reported,
            Usage {
                input_tokens: 1000,
                output_tokens: 500,
            }
        );
        assert_eq!(
            reconciliation.final_usage,
            Usage {
                input_tokens: 1000,
                output_tokens: 500,
            }
        );

        // The full reported 1500 tokens accrued against the lineage key.
        let mut b = Budget {
            token_cap: 1500,
            ..Default::default()
        };
        spend.fold("tok-stream", &mut b);
        assert!(budget_exhausted(&b), "1500/1500 is exhausted");
    }

    #[test]
    fn positive_low_stream_report_cannot_suppress_observed_output() {
        let receipts = Arc::new(Mutex::new(ReceiptLedger::new()));
        let spend = Arc::new(LocalSpendLedger::default());
        let mut meter = stream_meter(&receipts, &spend);
        meter.observe(&usage_frame(1, 1));
        meter.observe(&delta("0123456789abcdef"));
        drop(meter);

        let led = receipts.lock().unwrap();
        let receipt = &led.receipts()[0];
        assert_eq!(
            receipt.input_tokens, 25,
            "local input estimate is the floor"
        );
        assert_eq!(
            receipt.output_tokens, 4,
            "positive provider output cannot suppress observed bytes"
        );
        assert_eq!(
            receipt.usage_reconciliation.unwrap().provider_reported,
            Usage {
                input_tokens: 1,
                output_tokens: 1,
            }
        );
    }
}
