//! `aog-gateway` — the AOG estate model gateway (data-path).
//!
//! One endpoint in front of every model. A caller presents an opaque **virtual
//! key**; the gateway resolves it to a **WSF trust token** (scope + budget),
//! verifies the token off-host (ML-DSA), and refuses over-budget requests
//! **pre-flight** — before any model is touched. Provider dispatch (G2), the
//! OpenAI/Anthropic surfaces (G3/G4), classify+route (G5), policy modes (G6),
//! metering (G7), and the budget kill-switch (G9) layer on top of this skeleton.
//!
//! Virtual keys map to tokens in OpenBao KV (`<prefix>/<sha256(key)>` →
//! `{ "token": <TrustToken> }`), so a key is a revocable pointer at a signed
//! token, never a standing secret.

pub mod app;
pub mod http;
pub mod meter;
pub mod policy;
pub mod provider;
pub mod route;
pub mod surface_anthropic;
pub mod surface_openai;
pub mod tokenize;

use std::collections::HashMap;
use std::sync::Mutex;

use chrono::{DateTime, Utc};
use fabric_contracts::{Budget, TrustToken};
use fabric_crypto::providers::MlDsa87Verifier;
use fabric_revocation::RevocationSnapshot;
use sha2::{Digest, Sha256};
use wsf_bridge::OpenBaoAuth;

/// Failures on the gateway's auth / resolution path.
#[derive(Debug, thiserror::Error)]
pub enum GatewayError {
    /// The virtual key does not resolve to a token.
    #[error("unknown virtual key")]
    UnknownKey,
    /// The resolved token failed verification / is expired / revoked.
    #[error("unauthorized: {0}")]
    Unauthorized(String),
    /// The token's budget is exhausted — reject pre-flight.
    #[error("budget exhausted")]
    BudgetExhausted,
    /// The stored virtual-key record was malformed.
    #[error("malformed key record: {0}")]
    Malformed(String),
    /// An OpenBao interaction failed.
    #[error("openbao: {0}")]
    OpenBao(#[from] wsf_bridge::OpenBaoError),
    /// The token (or its subject) is named in the current revocation snapshot —
    /// the kill switch. Rejected regardless of signature validity or budget.
    #[error("token revoked")]
    Revoked,
}

/// The resolved request context — a verified, in-budget token + its tenant.
#[derive(Debug, Clone)]
pub struct ResolvedContext {
    /// The verified trust token behind the virtual key.
    pub token: TrustToken,
    /// The owning tenant (from the token).
    pub tenant_id: String,
}

/// Static configuration.
#[derive(Debug, Clone)]
pub struct GatewayConfig {
    /// WSF trust-anchor public key (verifies the resolved tokens).
    pub token_public_key: Vec<u8>,
    /// KV-v2 data prefix mapping key hashes to tokens (`kv/data/aog/virtual-keys`).
    pub virtual_key_kv_prefix: String,
}

/// One token's accumulated runtime spend (G9). The signed token carries the caps
/// and a spend baseline; actual usage accrues here — in the gateway — across a
/// session, so the pre-flight check enforces cumulative budget.
#[derive(Debug, Default, Clone, Copy)]
struct Spent {
    tokens: u64,
    usd_cents: u64,
    tool_calls: u32,
}

/// Thread-safe `token_id` → accumulated [`Spent`]. Appliance-scoped (in-memory,
/// single gateway); an OpenBao-backed, HA-shared counter is the follow-on.
#[derive(Debug, Default)]
struct SpendLedger {
    inner: Mutex<HashMap<String, Spent>>,
}

impl SpendLedger {
    /// Add one completed call's usage to a token's running total (saturating).
    fn add(&self, token_id: &str, tokens: u64, usd_cents: u64, tool_calls: u32) {
        let mut g = self.inner.lock().expect("spend ledger lock");
        let e = g.entry(token_id.to_string()).or_default();
        e.tokens = e.tokens.saturating_add(tokens);
        e.usd_cents = e.usd_cents.saturating_add(usd_cents);
        e.tool_calls = e.tool_calls.saturating_add(tool_calls);
    }

    /// Fold a token's accumulated runtime spend into a budget's `*_spent` counters
    /// (saturating), so the pre-flight sees session-cumulative usage. Unknown token
    /// folds nothing.
    fn apply(&self, token_id: &str, budget: &mut Budget) {
        let g = self.inner.lock().expect("spend ledger lock");
        if let Some(s) = g.get(token_id) {
            budget.tokens_spent = budget.tokens_spent.saturating_add(s.tokens);
            budget.usd_spent_cents = budget.usd_spent_cents.saturating_add(s.usd_cents);
            budget.tool_calls_spent = budget.tool_calls_spent.saturating_add(s.tool_calls);
        }
    }
}

/// The gateway's auth + resolution core.
pub struct Gateway {
    openbao: OpenBaoAuth,
    config: GatewayConfig,
    /// G9 per-token runtime spend (session-cumulative budget enforcement).
    spend: SpendLedger,
    /// G9 kill switch: KV path to the signed revocation snapshot. Empty (the
    /// default) disables the check — no snapshot source configured.
    revocation_kv_path: String,
}

/// Whether a budget has any dimension exhausted (a cap of 0 = that axis unused).
#[must_use]
pub fn budget_exhausted(budget: &fabric_contracts::Budget) -> bool {
    (budget.token_cap > 0 && budget.tokens_spent >= budget.token_cap)
        || (budget.usd_cap_cents > 0 && budget.usd_spent_cents >= budget.usd_cap_cents)
        || (budget.tool_call_cap > 0 && budget.tool_calls_spent >= budget.tool_call_cap)
}

impl Gateway {
    /// Assemble a gateway from an OpenBao client and config.
    #[must_use]
    pub fn new(openbao: OpenBaoAuth, config: GatewayConfig) -> Self {
        Self {
            openbao,
            config,
            spend: SpendLedger::default(),
            revocation_kv_path: String::new(),
        }
    }

    /// Set the KV path the kill switch reads the signed revocation snapshot from
    /// (e.g. `kv/data/aog/revocation`). Empty (the default) disables the check.
    #[must_use]
    pub fn with_revocation_path(mut self, path: impl Into<String>) -> Self {
        self.revocation_kv_path = path.into();
        self
    }

    /// Resolve a virtual key to a verified, in-budget [`ResolvedContext`].
    ///
    /// # Errors
    /// [`GatewayError::UnknownKey`] if the key is unmapped, [`GatewayError::Unauthorized`]
    /// if the token fails verification / expiry, [`GatewayError::BudgetExhausted`]
    /// (pre-flight) if the budget has no room, or an OpenBao/parse error.
    pub async fn resolve_and_check(
        &self,
        virtual_key: &str,
        now: DateTime<Utc>,
    ) -> Result<ResolvedContext, GatewayError> {
        let vault_token = self.openbao.login().await?;
        let key_hash = hex::encode(Sha256::digest(virtual_key.as_bytes()));
        let path = format!("{}/{key_hash}", self.config.virtual_key_kv_prefix);
        let data = match self.openbao.get_kv_data(&vault_token, &path).await {
            Ok(d) => d,
            Err(wsf_bridge::OpenBaoError::NotFound(_)) => return Err(GatewayError::UnknownKey),
            Err(e) => return Err(GatewayError::OpenBao(e)),
        };
        let token_value = data.get("token").cloned().ok_or(GatewayError::UnknownKey)?;
        let token: TrustToken = serde_json::from_value(token_value)
            .map_err(|e| GatewayError::Malformed(e.to_string()))?;

        // Verify off-host + expiry.
        fabric_token::verify(&token, &MlDsa87Verifier, &self.config.token_public_key)
            .map_err(|e| GatewayError::Unauthorized(e.to_string()))?;
        if fabric_token::is_expired(&token, now)
            .map_err(|e| GatewayError::Unauthorized(e.to_string()))?
        {
            return Err(GatewayError::Unauthorized("token expired".to_string()));
        }

        // Kill switch (G9): consult the signed revocation snapshot. A revoked token
        // or subject halts the session's next call. No snapshot at the path = nothing
        // revoked (fail-open on absence); a present-but-invalid snapshot fails closed.
        if !self.revocation_kv_path.is_empty() {
            match self
                .openbao
                .get_kv_data(&vault_token, &self.revocation_kv_path)
                .await
            {
                Ok(d) => {
                    let snap = d.get("snapshot").cloned().ok_or_else(|| {
                        GatewayError::Unauthorized("malformed revocation record".to_string())
                    })?;
                    let snapshot: RevocationSnapshot =
                        serde_json::from_value(snap).map_err(|e| {
                            GatewayError::Unauthorized(format!("revocation snapshot: {e}"))
                        })?;
                    fabric_revocation::verify(
                        &snapshot,
                        &MlDsa87Verifier,
                        &self.config.token_public_key,
                    )
                    .map_err(|e| {
                        GatewayError::Unauthorized(format!("revocation snapshot signature: {e}"))
                    })?;
                    if snapshot.is_token_revoked(&token.token_id)
                        || snapshot.is_subject_revoked(&token.subject_hash)
                    {
                        return Err(GatewayError::Revoked);
                    }
                }
                Err(wsf_bridge::OpenBaoError::NotFound(_)) => {}
                Err(e) => return Err(GatewayError::OpenBao(e)),
            }
        }

        // Budget pre-flight (G1 static caps + G9 session-cumulative runtime spend).
        if let Some(b) = &token.budget {
            let mut effective = b.clone();
            self.spend.apply(&token.token_id, &mut effective);
            if budget_exhausted(&effective) {
                return Err(GatewayError::BudgetExhausted);
            }
        }

        let tenant_id = token.tenant_id.clone();
        Ok(ResolvedContext { token, tenant_id })
    }

    /// Record one completed call's usage against a token's budget (G9). The next
    /// [`resolve_and_check`](Self::resolve_and_check) for the same token folds in
    /// the cumulative spend and rejects pre-flight once a cap is reached — so
    /// budget exhaustion blocks a session mid-flight, not just at issue time.
    pub fn record_spend(&self, token_id: &str, tokens: u64, usd_cents: u64, tool_calls: u32) {
        self.spend.add(token_id, tokens, usd_cents, tool_calls);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fabric_contracts::Budget;

    #[test]
    fn budget_exhaustion_is_per_dimension() {
        // token dimension exhausted
        assert!(budget_exhausted(&Budget {
            token_cap: 1000,
            tokens_spent: 1000,
            ..Default::default()
        }));
        // usd dimension exhausted
        assert!(budget_exhausted(&Budget {
            usd_cap_cents: 500,
            usd_spent_cents: 500,
            ..Default::default()
        }));
        // room left on all set dimensions
        assert!(!budget_exhausted(&Budget {
            token_cap: 1000,
            tokens_spent: 10,
            usd_cap_cents: 500,
            usd_spent_cents: 1,
            ..Default::default()
        }));
        // an all-zero budget is "unused" (no axis enforced), not exhausted
        assert!(!budget_exhausted(&Budget::default()));
    }

    #[test]
    fn spend_ledger_accumulates_and_folds_per_token() {
        let led = SpendLedger::default();
        led.add("tok-1", 100, 5, 1);
        led.add("tok-1", 50, 3, 0);
        let mut b = Budget {
            token_cap: 1000,
            usd_cap_cents: 100,
            tool_call_cap: 10,
            ..Default::default()
        };
        led.apply("tok-1", &mut b);
        assert_eq!(b.tokens_spent, 150);
        assert_eq!(b.usd_spent_cents, 8);
        assert_eq!(b.tool_calls_spent, 1);
        // an unknown token folds nothing.
        let mut other = Budget {
            token_cap: 1000,
            ..Default::default()
        };
        led.apply("unknown", &mut other);
        assert_eq!(other.tokens_spent, 0);
    }

    #[test]
    fn accumulated_spend_exhausts_the_budget_mid_session() {
        // A token with room at issue time that runtime spend pushes over its cap.
        let led = SpendLedger::default();
        let base = Budget {
            token_cap: 200,
            ..Default::default()
        };
        led.add("t", 150, 0, 0);
        let mut b = base.clone();
        led.apply("t", &mut b);
        assert!(!budget_exhausted(&b), "150/200 still has room");
        // The next call tips it over — the pre-flight now rejects.
        led.add("t", 60, 0, 0);
        let mut b = base.clone();
        led.apply("t", &mut b);
        assert!(budget_exhausted(&b), "210/200 is exhausted mid-session");
    }
}
