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
pub mod posture;
pub mod provider;
pub mod recommend;
pub mod route;
pub mod spend;
pub mod surface_anthropic;
pub mod surface_openai;
pub mod tokenize;

use std::sync::Arc;

use chrono::{DateTime, Utc};
use fabric_contracts::TrustToken;
use fabric_crypto::providers::MlDsa87Verifier;
use fabric_revocation::RevocationSnapshot;
use fabric_token::spend::{LocalSpendLedger, SpendLedger, Spent};
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

/// The gateway's auth + resolution core.
pub struct Gateway {
    openbao: OpenBaoAuth,
    config: GatewayConfig,
    /// G9 per-token runtime spend (session-cumulative budget enforcement),
    /// behind the `fabric_token::spend::SpendLedger` trait (X1) so the ledger is
    /// swappable without touching the data-path API. Defaults to the
    /// single-process [`LocalSpendLedger`] (byte-for-byte the old behavior); X2
    /// makes it injectable so a horizontally-scaled deployment can supply a
    /// shared ledger. (The lease-based reserve flow uses a distinct `try_spend`
    /// API rather than `fold`/`add`; adopting it in the request path is
    /// scale-out work that lands with the node runtime running replicas, M3b.)
    spend: Arc<dyn SpendLedger>,
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
            spend: Arc::new(LocalSpendLedger::default()),
            revocation_kv_path: String::new(),
        }
    }

    /// Swap the runtime spend ledger — e.g. a shared ledger for a horizontally
    /// scaled estate. The default is the single-process [`LocalSpendLedger`];
    /// this changes no data-path API (X2).
    #[must_use]
    pub fn with_spend_ledger(mut self, spend: Arc<dyn SpendLedger>) -> Self {
        self.spend = spend;
        self
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

        // Kill switch (G9/F2): consult the signed revocation snapshot. When a
        // revocation path is configured, a missing snapshot fails CLOSED — an
        // operator that wired revocation must not be silently unprotected because
        // the snapshot is absent or was deleted (F2-N2). The verified snapshot is
        // then checked for freshness (F2-N3) and against the complete revocation
        // predicate — every dimension, not just token id + subject (F2-N1).
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
                    revocation_decision(&snapshot, &token, now)?;
                }
                Err(wsf_bridge::OpenBaoError::NotFound(_)) => {
                    return Err(GatewayError::Unauthorized(
                        "revocation snapshot unavailable (fail-closed)".to_string(),
                    ));
                }
                Err(e) => return Err(GatewayError::OpenBao(e)),
            }
        }

        // Budget pre-flight (G1 static caps + G9 session-cumulative runtime spend).
        // Metering is keyed by the attenuation lineage (T5) so sibling children
        // share one atomic counter and cannot each spend the parent's remaining.
        if let Some(b) = &token.budget {
            let mut effective = b.clone();
            self.spend
                .fold(fabric_token::lineage_key(&token), &mut effective);
            if budget_exhausted(&effective) {
                return Err(GatewayError::BudgetExhausted);
            }
        }

        let tenant_id = token.tenant_id.clone();
        Ok(ResolvedContext { token, tenant_id })
    }

    /// Record one completed call's usage against a token's budget (G9). `key`
    /// must be the token's [`fabric_token::lineage_key`] (T5) so sibling children
    /// accrue against one shared counter; the next
    /// [`resolve_and_check`](Self::resolve_and_check) folds the cumulative spend
    /// and rejects pre-flight once a cap is reached — so budget exhaustion blocks
    /// a session mid-flight, not just at issue time. (A root token's lineage key
    /// is its own id.)
    pub fn record_spend(&self, key: &str, tokens: u64, usd_cents: u64, tool_calls: u32) {
        self.spend.add(
            key,
            Spent {
                tokens,
                usd_cents,
                tool_calls,
            },
        );
    }
}

/// A revocation snapshot is stale once `now` is at or past its `expires_at`
/// (F2-N3). A snapshot whose expiry cannot be parsed is treated as stale
/// (fail-closed) rather than trusted indefinitely.
fn snapshot_is_stale(snapshot: &RevocationSnapshot, now: DateTime<Utc>) -> bool {
    match DateTime::parse_from_rfc3339(&snapshot.expires_at) {
        Ok(exp) => now >= exp.with_timezone(&Utc),
        Err(_) => true,
    }
}

/// The gateway's decision for a signature-verified revocation snapshot (F2).
/// Fails closed on a stale snapshot; denies on any revoked dimension via the
/// complete predicate (`RevocationSnapshot::revokes`).
fn revocation_decision(
    snapshot: &RevocationSnapshot,
    token: &TrustToken,
    now: DateTime<Utc>,
) -> Result<(), GatewayError> {
    if snapshot_is_stale(snapshot, now) {
        return Err(GatewayError::Unauthorized(
            "revocation snapshot is stale (past its freshness window)".to_string(),
        ));
    }
    if snapshot.revokes(token).is_some() {
        return Err(GatewayError::Revoked);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use fabric_contracts::Budget;

    fn now_t() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-07-04T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc)
    }

    fn snap(expires_at: &str) -> RevocationSnapshot {
        RevocationSnapshot::new("s1", "2026-07-03T00:00:00Z", expires_at)
    }

    fn tok(tenant: &str) -> TrustToken {
        use fabric_contracts::{Attenuation, Classification, RevocationStatus, Signature};
        TrustToken {
            token_id: "t".into(),
            issued_at: "2026-07-03T00:00:00Z".into(),
            expires_at: "2099-01-01T00:00:00Z".into(),
            issuer: "wsf-bridge".into(),
            trust_bundle_version: "2026.07.v2".into(),
            tenant_id: tenant.into(),
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
            revocation_status: RevocationStatus::Unknown,
            budget: None,
            attenuation: Attenuation::default(),
            signature: Signature {
                alg: String::new(),
                key_id: String::new(),
                value: String::new(),
            },
        }
    }

    #[test]
    fn stale_snapshot_detected_by_expiry() {
        let now = now_t();
        assert!(!snapshot_is_stale(&snap("2099-01-01T00:00:00Z"), now)); // fresh
        assert!(snapshot_is_stale(&snap("2026-07-03T00:00:00Z"), now)); // expired
        assert!(snapshot_is_stale(&snap("not-a-date"), now)); // unparseable -> fail-closed
    }

    #[test]
    fn revocation_decision_denies_stale_before_revokes() {
        // Fail-closed: a stale snapshot is Unauthorized regardless of contents.
        let s = snap("2026-07-03T00:00:00Z");
        assert!(matches!(
            revocation_decision(&s, &tok("baap"), now_t()),
            Err(GatewayError::Unauthorized(_))
        ));
    }

    #[test]
    fn revocation_decision_denies_revoked_tenant() {
        // F2-N1: a fresh snapshot revoking the token's TENANT denies — the
        // dimension the old token-id + subject-only check missed.
        let mut s = snap("2099-01-01T00:00:00Z");
        s.revoked_tenants.push("baap".into());
        assert!(matches!(
            revocation_decision(&s, &tok("baap"), now_t()),
            Err(GatewayError::Revoked)
        ));
    }

    #[test]
    fn revocation_decision_allows_clean_fresh_snapshot() {
        let s = snap("2099-01-01T00:00:00Z");
        assert!(revocation_decision(&s, &tok("baap"), now_t()).is_ok());
    }

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
    fn accumulated_spend_exhausts_the_budget_mid_session() {
        // A token with room at issue time that runtime spend pushes over its
        // cap. (Ledger mechanics themselves are covered in fabric-token::spend,
        // where the X1 promotion moved them.)
        let led = LocalSpendLedger::default();
        let base = Budget {
            token_cap: 200,
            ..Default::default()
        };
        led.add(
            "t",
            Spent {
                tokens: 150,
                ..Default::default()
            },
        );
        let mut b = base.clone();
        led.fold("t", &mut b);
        assert!(!budget_exhausted(&b), "150/200 still has room");
        // The next call tips it over — the pre-flight now rejects.
        led.add(
            "t",
            Spent {
                tokens: 60,
                ..Default::default()
            },
        );
        let mut b = base.clone();
        led.fold("t", &mut b);
        assert!(budget_exhausted(&b), "210/200 is exhausted mid-session");
    }
}
