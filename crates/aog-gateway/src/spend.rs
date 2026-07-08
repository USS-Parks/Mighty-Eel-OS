//! X1 — the production [`LeaseStore`]: an atomic shared budget record in
//! OpenBao KV v2, guarded by compare-and-set. Concurrent replicas race their
//! CAS writes; the loser re-reads and retries, so the total leased can never
//! exceed the cap — the property the whole shared-budget contract stands on.
//!
//! Record shape at `<prefix>/<key>`: `{ "leased": { tokens, usd_cents,
//! tool_calls } }` — cumulative grants, created lazily by the first
//! acquisition. The per-replica slice arithmetic lives in
//! `fabric_token::spend::LeasedSpendLedger`; this store only claims slices
//! atomically.

use fabric_contracts::Budget;
use fabric_token::spend::{LeaseStore, SpendError, Spent};
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};
use serde_json::json;
use wsf_bridge::OpenBaoAuth;

/// CAS retry ceiling — past this the pool is treated as failed (deny; I-4).
const MAX_CAS_ATTEMPTS: u32 = 32;

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
struct LeasedRecord {
    #[serde(default)]
    tokens: u64,
    #[serde(default)]
    usd_cents: u64,
    #[serde(default)]
    tool_calls: u32,
}

/// The OpenBao-KV-CAS lease store.
pub struct OpenBaoLeaseStore {
    openbao: OpenBaoAuth,
    http: reqwest::Client,
    /// KV-v2 data prefix for shared budget records (e.g. `kv/data/spend`).
    prefix: String,
}

impl OpenBaoLeaseStore {
    /// # Errors
    /// [`SpendError`] if the HTTP client cannot be built.
    pub fn new(openbao: OpenBaoAuth, prefix: impl Into<String>) -> Result<Self, SpendError> {
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .build()
            .map_err(|e| SpendError(e.to_string()))?;
        Ok(Self {
            openbao,
            http,
            prefix: prefix.into(),
        })
    }

    /// Read the record + its KV version (absent → zero record, version 0).
    async fn read(&self, key: &str) -> Result<(LeasedRecord, u64), SpendError> {
        let token = self
            .openbao
            .login()
            .await
            .map_err(|e| SpendError(e.to_string()))?;
        let url = format!("{}/v1/{}/{key}", self.openbao.address(), self.prefix);
        let resp = self
            .http
            .get(url)
            .header("X-Vault-Token", token)
            .send()
            .await
            .map_err(|e| SpendError(e.to_string()))?;
        if resp.status() == StatusCode::NOT_FOUND {
            return Ok((LeasedRecord::default(), 0));
        }
        if !resp.status().is_success() {
            return Err(SpendError(format!("lease record read: {}", resp.status())));
        }
        let body: serde_json::Value = resp.json().await.map_err(|e| SpendError(e.to_string()))?;
        let record = body["data"]["data"]["leased"]
            .as_object()
            .map_or_else(LeasedRecord::default, |_| {
                serde_json::from_value(body["data"]["data"]["leased"].clone()).unwrap_or_default()
            });
        let version = body["data"]["metadata"]["version"].as_u64().unwrap_or(0);
        Ok((record, version))
    }

    /// CAS-write the record at `expected` version. Ok(true) = committed;
    /// Ok(false) = version conflict (retry).
    async fn write(
        &self,
        key: &str,
        record: LeasedRecord,
        expected: u64,
    ) -> Result<bool, SpendError> {
        let token = self
            .openbao
            .login()
            .await
            .map_err(|e| SpendError(e.to_string()))?;
        let url = format!("{}/v1/{}/{key}", self.openbao.address(), self.prefix);
        let body = json!({
            "data": { "leased": record },
            "options": { "cas": expected },
        });
        let resp = self
            .http
            .post(url)
            .header("X-Vault-Token", token)
            .json(&body)
            .send()
            .await
            .map_err(|e| SpendError(e.to_string()))?;
        if resp.status().is_success() {
            return Ok(true);
        }
        // KV v2 answers 400 to a check-and-set version mismatch.
        if resp.status() == StatusCode::BAD_REQUEST {
            return Ok(false);
        }
        Err(SpendError(format!("lease record write: {}", resp.status())))
    }
}

fn grant_axis(cap: u64, leased: u64, want: u64) -> u64 {
    if cap == 0 {
        want // unmetered axis
    } else {
        want.min(cap.saturating_sub(leased))
    }
}

impl LeaseStore for OpenBaoLeaseStore {
    async fn acquire(&self, key: &str, cap: &Budget, want: Spent) -> Result<Spent, SpendError> {
        for _ in 0..MAX_CAS_ATTEMPTS {
            let (leased, version) = self.read(key).await?;
            let grant = Spent {
                tokens: grant_axis(cap.token_cap, leased.tokens, want.tokens),
                usd_cents: grant_axis(cap.usd_cap_cents, leased.usd_cents, want.usd_cents),
                tool_calls: u32::try_from(grant_axis(
                    u64::from(cap.tool_call_cap),
                    u64::from(leased.tool_calls),
                    u64::from(want.tool_calls),
                ))
                .unwrap_or(u32::MAX),
            };
            if grant.is_zero() {
                return Ok(grant); // pool dry — nothing to commit
            }
            let next = LeasedRecord {
                tokens: leased.tokens.saturating_add(grant.tokens),
                usd_cents: leased.usd_cents.saturating_add(grant.usd_cents),
                tool_calls: leased.tool_calls.saturating_add(grant.tool_calls),
            };
            if self.write(key, next, version).await? {
                return Ok(grant);
            }
            // CAS conflict: another replica won the version — re-read.
        }
        Err(SpendError(format!(
            "lease CAS contention exceeded {MAX_CAS_ATTEMPTS} attempts"
        )))
    }
}
