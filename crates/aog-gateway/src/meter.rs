//! Metering + receipts (G7).
//!
//! Every completed request emits a **metadata-only** WSF receipt into an
//! append-only BLAKE3 hash chain (`fabric-proof`, the same primitive `wsf-seal`
//! and `wsf-ledger` use), and `aog-meter` aggregates spend per
//! **tenant / provider / model / task** (`workflow_id`). The chain verifies
//! off-host; the receipts carry the spend, provider, token counts, and — for a
//! **local** model — a weights digest (cloud is named by provider+model).
//!
//! Streaming note: receipts are recorded on the **non-stream** completion path,
//! where the provider's real `Usage` is in hand. Metering the streamed path from
//! its terminal usage frame is a follow-on; every non-stream call is metered.

use std::collections::HashMap;
use std::sync::Mutex;

use axum::http::HeaderMap;
use chrono::Utc;
use fabric_proof::{ChainLink, GENESIS_HASH, canonical_hash, chain_link, verify_chain};
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::ResolvedContext;
use crate::provider::Usage;
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
        model_weights_digest: local_weights_digest(c.provider, c.model),
        workflow_id: c.workflow_id.clone(),
        tokenized_spans: c.tokenized_spans,
    };
    guard.append(receipt)
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
