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

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use fabric_contracts::TrustToken;
use fabric_crypto::providers::MlDsa87Verifier;
use fabric_revocation::{CurrentRevocationError, MonotonicRevocationStore, RevocationSnapshot};
use fabric_token::spend::{LocalSpendLedger, SpendLedger, Spent};
use sha2::{Digest, Sha256};
use tokio::sync::{Semaphore, SemaphorePermit};
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
    /// Authentication admission is at its configured concurrency/rate bound.
    #[error("authentication admission limited")]
    AdmissionLimited,
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

/// Bounds for unauthenticated virtual-key resolution work.
#[derive(Debug, Clone)]
pub struct GatewayAdmissionConfig {
    /// Maximum simultaneous OpenBao virtual-key resolutions.
    pub max_concurrent: usize,
    /// Maximum newly admitted resolutions in one fixed window.
    pub max_per_window: usize,
    /// Fixed-window duration for the resolution rate ceiling.
    pub window: Duration,
    /// How long a confirmed unknown-key hash may be denied locally.
    pub negative_ttl: Duration,
    /// Maximum number of unknown-key hashes retained.
    pub negative_capacity: usize,
}

impl Default for GatewayAdmissionConfig {
    fn default() -> Self {
        Self {
            max_concurrent: 32,
            max_per_window: 128,
            window: Duration::from_secs(1),
            negative_ttl: Duration::from_secs(1),
            negative_capacity: 1_024,
        }
    }
}

struct AdmissionControl {
    concurrency: Semaphore,
    config: GatewayAdmissionConfig,
    state: Mutex<AdmissionState>,
}

struct AdmissionState {
    window_started: Instant,
    admitted_in_window: usize,
    in_flight: HashSet<String>,
    negative: HashMap<String, Instant>,
    negative_order: VecDeque<(String, Instant)>,
}

enum AdmissionRejection {
    CachedUnknown,
    Limited,
}

struct AdmissionLease<'a> {
    control: &'a AdmissionControl,
    key_hash: String,
    cache_unknown: bool,
    _permit: SemaphorePermit<'a>,
}

impl AdmissionControl {
    fn new(mut config: GatewayAdmissionConfig) -> Self {
        config.max_concurrent = config.max_concurrent.max(1);
        config.max_per_window = config.max_per_window.max(1);
        config.window = config.window.max(Duration::from_millis(1));
        config.negative_ttl = config.negative_ttl.max(Duration::from_millis(1));
        config.negative_capacity = config.negative_capacity.max(1);
        Self {
            concurrency: Semaphore::new(config.max_concurrent),
            config,
            state: Mutex::new(AdmissionState {
                window_started: Instant::now(),
                admitted_in_window: 0,
                in_flight: HashSet::new(),
                negative: HashMap::new(),
                negative_order: VecDeque::new(),
            }),
        }
    }

    fn begin(&self, key_hash: String) -> Result<AdmissionLease<'_>, AdmissionRejection> {
        let permit = self
            .concurrency
            .try_acquire()
            .map_err(|_| AdmissionRejection::Limited)?;
        let now = Instant::now();
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        while state
            .negative_order
            .front()
            .is_some_and(|(_, expires)| *expires <= now)
        {
            if let Some((expired_hash, expired_at)) = state.negative_order.pop_front()
                && state.negative.get(&expired_hash) == Some(&expired_at)
            {
                state.negative.remove(&expired_hash);
            }
        }
        if state
            .negative
            .get(&key_hash)
            .is_some_and(|expires| *expires > now)
        {
            return Err(AdmissionRejection::CachedUnknown);
        }
        if state.in_flight.contains(&key_hash) {
            return Err(AdmissionRejection::Limited);
        }
        if now.duration_since(state.window_started) >= self.config.window {
            state.window_started = now;
            state.admitted_in_window = 0;
        }
        if state.admitted_in_window >= self.config.max_per_window {
            return Err(AdmissionRejection::Limited);
        }
        state.admitted_in_window += 1;
        state.in_flight.insert(key_hash.clone());
        drop(state);
        Ok(AdmissionLease {
            control: self,
            key_hash,
            cache_unknown: false,
            _permit: permit,
        })
    }

    #[cfg(test)]
    fn sizes(&self) -> (usize, usize) {
        let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        (state.negative.len(), state.in_flight.len())
    }
}

impl AdmissionLease<'_> {
    fn cache_unknown(&mut self) {
        self.cache_unknown = true;
    }
}

impl Drop for AdmissionLease<'_> {
    fn drop(&mut self) {
        let mut state = self.control.state.lock().unwrap_or_else(|e| e.into_inner());
        state.in_flight.remove(&self.key_hash);
        if self.cache_unknown {
            let expires = Instant::now() + self.control.config.negative_ttl;
            state.negative.insert(self.key_hash.clone(), expires);
            state
                .negative_order
                .push_back((self.key_hash.clone(), expires));
            while state.negative.len() > self.control.config.negative_capacity {
                if let Some((oldest_hash, oldest_expiry)) = state.negative_order.pop_front()
                    && state.negative.get(&oldest_hash) == Some(&oldest_expiry)
                {
                    state.negative.remove(&oldest_hash);
                }
            }
        }
    }
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
    revocation: GatewayRevocation,
    admission: AdmissionControl,
}

enum GatewayRevocation {
    DevelopmentDisabled,
    Required {
        path: String,
        store: Arc<RwLock<MonotonicRevocationStore>>,
    },
}

/// Whether a budget has any dimension exhausted (a cap of 0 = that axis unused).
#[must_use]
pub fn budget_exhausted(budget: &fabric_contracts::Budget) -> bool {
    (budget.token_cap > 0 && budget.tokens_spent >= budget.token_cap)
        || (budget.usd_cap_cents > 0 && budget.usd_spent_cents >= budget.usd_cap_cents)
        || (budget.tool_call_cap > 0 && budget.tool_calls_spent >= budget.tool_call_cap)
}

impl Gateway {
    /// Assemble an explicit development/test gateway with revocation disabled.
    /// Production callers must use [`Self::new_production`].
    #[must_use]
    pub fn new(openbao: OpenBaoAuth, config: GatewayConfig) -> Self {
        Self {
            openbao,
            config,
            spend: Arc::new(LocalSpendLedger::default()),
            revocation: GatewayRevocation::DevelopmentDisabled,
            admission: AdmissionControl::new(GatewayAdmissionConfig::default()),
        }
    }

    /// Assemble a production gateway and load its mandatory initial revocation
    /// snapshot before returning. Listener construction must happen only after
    /// this succeeds.
    pub async fn new_production(
        openbao: OpenBaoAuth,
        config: GatewayConfig,
        revocation_path: impl Into<String>,
    ) -> Result<Self, GatewayError> {
        let revocation_path = revocation_path.into();
        if revocation_path.trim().is_empty() {
            return Err(GatewayError::Unauthorized(
                "mandatory production revocation path is empty".to_string(),
            ));
        }
        let gateway = Self {
            openbao,
            config,
            spend: Arc::new(LocalSpendLedger::default()),
            revocation: GatewayRevocation::Required {
                path: revocation_path,
                store: Arc::new(RwLock::new(MonotonicRevocationStore::new())),
            },
            admission: AdmissionControl::new(GatewayAdmissionConfig::default()),
        };
        gateway.refresh_revocation().await?;
        gateway.ensure_current_revocation(Utc::now())?;
        Ok(gateway)
    }

    #[cfg(test)]
    pub(crate) fn with_test_revocation_store(
        mut self,
        store: Arc<RwLock<MonotonicRevocationStore>>,
    ) -> Self {
        self.revocation = GatewayRevocation::Required {
            path: String::new(),
            store,
        };
        self
    }

    /// Swap the runtime spend ledger — e.g. a shared ledger for a horizontally
    /// scaled estate. The default is the single-process [`LocalSpendLedger`];
    /// this changes no data-path API (X2).
    #[must_use]
    pub fn with_spend_ledger(mut self, spend: Arc<dyn SpendLedger>) -> Self {
        self.spend = spend;
        self
    }

    /// Override unauthenticated virtual-key admission bounds.
    #[must_use]
    pub fn with_admission_config(mut self, config: GatewayAdmissionConfig) -> Self {
        self.admission = AdmissionControl::new(config);
        self
    }

    async fn refresh_revocation(&self) -> Result<(), GatewayError> {
        let GatewayRevocation::Required { path, store } = &self.revocation else {
            return Ok(());
        };
        if path.is_empty() {
            return Ok(());
        }
        let vault_token = self.openbao.login().await?;
        let data = match self.openbao.get_kv_data(&vault_token, path).await {
            Ok(data) => data,
            Err(wsf_bridge::OpenBaoError::NotFound(_)) => {
                return Err(GatewayError::Unauthorized(
                    "revocation snapshot unavailable (fail-closed)".to_string(),
                ));
            }
            Err(error) => return Err(GatewayError::OpenBao(error)),
        };
        let value = data
            .get("snapshot")
            .cloned()
            .ok_or_else(|| GatewayError::Unauthorized("malformed revocation record".to_string()))?;
        let candidate: RevocationSnapshot = serde_json::from_value(value)
            .map_err(|error| GatewayError::Unauthorized(format!("revocation snapshot: {error}")))?;
        let mut held = store.write().map_err(|_| {
            GatewayError::Unauthorized("revocation store lock poisoned".to_string())
        })?;
        if held.current() != Some(&candidate) {
            held.advance(candidate, &MlDsa87Verifier, &self.config.token_public_key)
                .map_err(|error| {
                    GatewayError::Unauthorized(format!("revocation snapshot rejected: {error}"))
                })?;
        }
        Ok(())
    }

    fn authorize_held(&self, token: &TrustToken, now: DateTime<Utc>) -> Result<(), GatewayError> {
        let GatewayRevocation::Required { store, .. } = &self.revocation else {
            return Ok(());
        };
        let held = store.read().map_err(|_| {
            GatewayError::Unauthorized("revocation store lock poisoned".to_string())
        })?;
        held.authorize(token, now).map_err(map_revocation_error)
    }

    fn ensure_current_revocation(&self, now: DateTime<Utc>) -> Result<(), GatewayError> {
        let GatewayRevocation::Required { store, .. } = &self.revocation else {
            return Ok(());
        };
        let held = store.read().map_err(|_| {
            GatewayError::Unauthorized("revocation store lock poisoned".to_string())
        })?;
        held.ensure_current(now)
            .map(|_| ())
            .map_err(map_revocation_error)
    }

    /// Refresh and apply current revocation immediately before a privileged
    /// provider step or stream continuation.
    pub async fn authorize_current(
        &self,
        token: &TrustToken,
        now: DateTime<Utc>,
    ) -> Result<(), GatewayError> {
        self.refresh_revocation().await?;
        self.authorize_held(token, now)
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
        validate_virtual_key(virtual_key)?;
        let key_hash = hex::encode(Sha256::digest(virtual_key.as_bytes()));
        let mut admission = match self.admission.begin(key_hash.clone()) {
            Ok(admission) => admission,
            Err(AdmissionRejection::CachedUnknown) => return Err(GatewayError::UnknownKey),
            Err(AdmissionRejection::Limited) => return Err(GatewayError::AdmissionLimited),
        };
        let vault_token = self.openbao.login().await?;
        let path = format!("{}/{key_hash}", self.config.virtual_key_kv_prefix);
        let data = match self.openbao.get_kv_data(&vault_token, &path).await {
            Ok(d) => d,
            Err(wsf_bridge::OpenBaoError::NotFound(_)) => {
                admission.cache_unknown();
                return Err(GatewayError::UnknownKey);
            }
            Err(e) => return Err(GatewayError::OpenBao(e)),
        };
        let token_value = data.get("token").cloned().ok_or(GatewayError::UnknownKey)?;
        let token: TrustToken = serde_json::from_value(token_value)
            .map_err(|e| GatewayError::Malformed(e.to_string()))?;
        drop(admission);

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
        self.authorize_current(&token, now).await?;

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

fn validate_virtual_key(virtual_key: &str) -> Result<(), GatewayError> {
    if virtual_key.is_empty()
        || virtual_key.len() > 128
        || !virtual_key
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.' | b'~'))
    {
        return Err(GatewayError::Unauthorized(
            "malformed virtual key".to_string(),
        ));
    }
    Ok(())
}

/// A revocation snapshot is stale once `now` is at or past its `expires_at`
/// (F2-N3). A snapshot whose expiry cannot be parsed is treated as stale
/// (fail-closed) rather than trusted indefinitely.
fn map_revocation_error(error: CurrentRevocationError) -> GatewayError {
    match error {
        CurrentRevocationError::Revoked(_) => GatewayError::Revoked,
        other => GatewayError::Unauthorized(format!("revocation state: {other}")),
    }
}

#[cfg(test)]
fn snapshot_is_stale(snapshot: &RevocationSnapshot, now: DateTime<Utc>) -> bool {
    DateTime::parse_from_rfc3339(&snapshot.expires_at)
        .map_or(true, |expires| now >= expires.with_timezone(&Utc))
}

#[cfg(test)]
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
    use std::sync::atomic::{AtomicUsize, Ordering};

    use axum::{
        Json, Router,
        extract::State,
        http::StatusCode,
        routing::{get, post},
    };
    use fabric_contracts::Budget;
    use serde_json::json;
    use wsf_bridge::OpenBaoConfig;

    #[derive(Clone, Default)]
    struct BackendCounts {
        logins: Arc<AtomicUsize>,
        reads: Arc<AtomicUsize>,
    }

    async fn mock_login(State(counts): State<BackendCounts>) -> Json<serde_json::Value> {
        counts.logins.fetch_add(1, Ordering::SeqCst);
        Json(json!({
            "auth": {"client_token": "test-token", "lease_duration": 60}
        }))
    }

    async fn mock_missing(
        State(counts): State<BackendCounts>,
    ) -> (StatusCode, Json<serde_json::Value>) {
        counts.reads.fetch_add(1, Ordering::SeqCst);
        (StatusCode::NOT_FOUND, Json(json!({"errors": []})))
    }

    async fn mock_unknown_backend() -> (String, BackendCounts) {
        let counts = BackendCounts::default();
        let app = Router::new()
            .route("/v1/auth/approle/login", post(mock_login))
            .route("/v1/{*path}", get(mock_missing))
            .with_state(counts.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        (address, counts)
    }

    fn admission_gateway(address: &str, admission: GatewayAdmissionConfig) -> Gateway {
        Gateway::new(
            OpenBaoAuth::new(OpenBaoConfig::new(address, "role", "secret")).unwrap(),
            GatewayConfig {
                token_public_key: vec![],
                virtual_key_kv_prefix: "kv/data/aog/virtual-keys".to_string(),
            },
        )
        .with_admission_config(admission)
    }

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

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn adversarial_bearers_bound_backend_calls_and_memory() {
        let (address, counts) = mock_unknown_backend().await;
        let admission = GatewayAdmissionConfig {
            max_concurrent: 8,
            max_per_window: 16,
            window: Duration::from_secs(60),
            negative_ttl: Duration::from_secs(60),
            negative_capacity: 8,
        };
        let gateway = Arc::new(admission_gateway(&address, admission.clone()));

        let malformed = (0..500)
            .map(|index| {
                let gateway = Arc::clone(&gateway);
                tokio::spawn(async move {
                    gateway
                        .resolve_and_check(&format!("not a key {index}"), Utc::now())
                        .await
                })
            })
            .collect::<Vec<_>>();
        for task in malformed {
            let error = task.await.unwrap().unwrap_err();
            assert!(matches!(error, GatewayError::Unauthorized(_)));
        }
        assert_eq!(counts.logins.load(Ordering::SeqCst), 0);
        assert_eq!(counts.reads.load(Ordering::SeqCst), 0);

        let repeated = (0..500)
            .map(|_| {
                let gateway = Arc::clone(&gateway);
                tokio::spawn(
                    async move { gateway.resolve_and_check("vk_repeated", Utc::now()).await },
                )
            })
            .collect::<Vec<_>>();
        for task in repeated {
            let error = task.await.unwrap().unwrap_err();
            assert!(matches!(
                error,
                GatewayError::UnknownKey | GatewayError::AdmissionLimited
            ));
            assert!(matches!(
                crate::http::to_http(&error).0,
                StatusCode::UNAUTHORIZED | StatusCode::TOO_MANY_REQUESTS
            ));
        }
        assert_eq!(counts.logins.load(Ordering::SeqCst), 1);
        assert_eq!(counts.reads.load(Ordering::SeqCst), 1);
        assert!(matches!(
            gateway.resolve_and_check("vk_repeated", Utc::now()).await,
            Err(GatewayError::UnknownKey)
        ));
        assert_eq!(counts.reads.load(Ordering::SeqCst), 1);

        let distinct_gateway = Arc::new(admission_gateway(&address, admission));
        let initial_logins = counts.logins.load(Ordering::SeqCst);
        let initial_reads = counts.reads.load(Ordering::SeqCst);
        let distinct = (0..500)
            .map(|index| {
                let gateway = Arc::clone(&distinct_gateway);
                tokio::spawn(async move {
                    gateway
                        .resolve_and_check(&format!("vk_distinct_{index}"), Utc::now())
                        .await
                })
            })
            .collect::<Vec<_>>();
        for task in distinct {
            let error = task.await.unwrap().unwrap_err();
            assert!(matches!(
                error,
                GatewayError::UnknownKey | GatewayError::AdmissionLimited
            ));
        }
        let admitted_logins = counts.logins.load(Ordering::SeqCst) - initial_logins;
        let admitted_reads = counts.reads.load(Ordering::SeqCst) - initial_reads;
        assert!(admitted_reads > 0);
        assert!(admitted_reads <= 16);
        assert_eq!(admitted_logins, admitted_reads);
        let (negative_entries, in_flight) = distinct_gateway.admission.sizes();
        assert!(negative_entries <= 8);
        assert_eq!(in_flight, 0);
    }
}
