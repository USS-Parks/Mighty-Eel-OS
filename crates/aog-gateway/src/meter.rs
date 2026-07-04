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
#[derive(Debug, Default)]
pub struct ReceiptLedger {
    links: Vec<ChainLink>,
    receipts: Vec<GatewayReceipt>,
    last_hash: [u8; 32],
}

impl ReceiptLedger {
    #[must_use]
    pub fn new() -> Self {
        Self {
            links: Vec::new(),
            receipts: Vec::new(),
            last_hash: GENESIS_HASH,
        }
    }

    /// Append a receipt; returns the new chain head (hex).
    pub fn append(&mut self, receipt: GatewayReceipt) -> String {
        let value = serde_json::to_value(&receipt).expect("receipt serializes");
        let entry_hash = canonical_hash(&value).expect("canonical hash of receipt");
        self.links.push(ChainLink {
            previous_hash: self.last_hash,
            entry_hash,
        });
        self.last_hash = chain_link(&self.last_hash, &entry_hash);
        self.receipts.push(receipt);
        hex::encode(self.last_hash)
    }

    /// The next request id (monotonic within this ledger).
    #[must_use]
    pub fn next_id(&self) -> String {
        format!("rcpt-{}", self.receipts.len())
    }

    /// Verify the receipt chain is unbroken from genesis.
    #[must_use]
    pub fn verify(&self) -> bool {
        verify_chain(&self.links).is_ok()
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
    let mut guard = ledger.lock().expect("receipt ledger lock");
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
        }
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
