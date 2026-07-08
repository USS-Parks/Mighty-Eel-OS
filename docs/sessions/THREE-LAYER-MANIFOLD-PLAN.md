# Three-Layer Security Manifold — Implementation Plan & Prompt Roster

<!--
  Island Mountain Mighty Eel OS — MAI/Lamprey Appliance
  Three-Layer Security Manifold (Ring-1 ↔ Ring-3) with OpenBao Trust Core
  Authored and reviewed by Basho Parks, copyright 2026
-->

## Overview

| Layer | Ring | Component | Status |
|-------|------|-----------|--------|
| 1 | **Ring-1** | mai-api application (trust exchange, policy runtime, WAL audit, AEAD sealer) | `c8055ea` — wired |
| 2 | **Ring-1→Ring-3** | OpenBao bridge client (AppRole auth, KV lookup, Transit claim signing) | `c8055ea` — wired |
| 3 | **Ring-3** | OpenBao Trust Core (KV v2, Transit, PKI, AppRole, ACL policy, file audit) | `c8055ea` — running |

**Gaps:** Revocation sync (dormant), mTLS bridge (dormant), bundle signing unification (dormant), operational hardening (missing).

---

## Gate Checklist (every prompt)

Before opening a PR, every task MUST pass:

| Gate | Command | Must |
|------|---------|------|
| **Check** | `cargo check --workspace` | no errors |
| **Clippy** | `cargo clippy --workspace -- -D warnings -A clippy::pedantic` | no errors |
| **Fmt** | `cargo fmt --check` | no diffs |
| **Test** | `cargo test --workspace` | 0 failures |
| **Unsafe** | `grep -rn "unsafe" mai-core/` | 0 results |
| **Production Guard** | `cargo test -p mai-api --test production_guard` | 41/41 checks, 0 failures |
| **Secrets** | `rg --no-heading "(secret|token|password)\s*=\s*['\"][^$]" deployment/` | 0 results |

---

## TIER 1 — Revocation Sync (Kill-Switch Activation)

**Goal:** Make `lamprey-revocation-signer` authoritative. When an operator revokes a credential in OpenBao, the mai-api policy runtime sees the revocation within the refresh interval (not 15-minute token TTL).

**Files touched:** `ship_profile.rs`, `openbao_client.rs`, `server.rs`, `handlers/trust.rs`, `state.rs`, `profile-production.toml`, `trust_cache.rs` (mai-compliance)

**Sub-tasks (5 prompts):**

### Prompt 1.1 — Ship Profile `[openbao]` Section

**Input:** Read `mai-api/src/ship_profile.rs` and `mai-api/src/openbao_client.rs`.

**Task:**
Add an `OpenbaoConfig` struct to `ShipProfile`, deserialized from a new `[openbao]` TOML section. Move all non-secret bridge configuration from `OpenBaoBridgeConfig::staging()` hardcoded defaults into the profile.

New `ShipProfile` field:
```rust
pub openbao: Option<OpenbaoConfig>,
```

New struct (with serde `Deserialize`, kebab-case TOML):
```rust
pub struct OpenbaoConfig {
    pub address: String,                          // e.g. "http://localhost:8200"
    pub role_id: String,                          // AppRole role_id
    pub timeout_secs: Option<u64>,                // default 10

    #[serde(default)]
    pub transit: TransitKeysConfig,
    #[serde(default)]
    pub kv: KvPathConfig,
    #[serde(default)]
    pub pki: PkiRoleConfig,
    #[serde(default)]
    pub trust_refresh: TrustRefreshConfig,
}

pub struct TransitKeysConfig {
    pub claim_signer_key: String,        // default "lamprey-claim-signer"
    pub bundle_signer_key: String,       // default "lamprey-bundle-signer"
    pub revocation_signer_key: String,   // default "lamprey-revocation-signer"
}

pub struct KvPathConfig {
    pub tenant_path: String,             // default "kv/tenants"
    pub revocation_path: String,         // default "kv/revocations"
}

pub struct PkiRoleConfig {
    pub role: String,                    // default "mai-appliance"
}

pub struct TrustRefreshConfig {
    pub enabled: bool,                   // default true
    pub interval_secs: u64,             // default 300 (5 min)
}
```

**Verification:**
1. `cargo check --workspace` passes
2. `cargo test -p mai-api --lib` passes (existing ship_profile tests)
3. Write a unit test: `#[test] fn deserialize_openbao_section()` in `ship_profile.rs` that parses a TOML string with `[openbao]` and asserts all fields
4. Update `profile-production.toml` with the new `[openbao]` section
5. Production guard: if `exchange_mode` is `OpenBaoBridge` but `openbao` section is `None`, add a prod check (`PROD-TRUST-103`) that fails

**Gate:** `cargo check`, `cargo clippy`, `cargo fmt --check`, `cargo test --workspace`, no unsafe, no secrets

---

### Prompt 1.2 — OpenBaoBridgeConfig from Profile

**Input:** Read `mai-api/src/ship_profile.rs`, `mai-api/src/openbao_client.rs`, `mai-api/src/server.rs` (the `apply_ship_profile` function).

**Task:**
1. Add `OpenBaoBridgeConfig::from_profile(profile: &OpenbaoConfig) -> Result<Self, OpenBaoBridgeError>` that constructs the config from the TOML section
2. Add `with_timeout()`, `with_secret_id()`, `with_wrapped_secret_id()` builder methods (secrets still come from env, never from profile TOML)
3. Move `transit_key_names` into the config struct so `sign_claim()` and new methods use config fields instead of hardcoded `"lamprey-claim-signer"` etc.
4. In `server.rs` `apply_ship_profile()`: replace `OpenBaoBridgeConfig::staging()` with `OpenBaoBridgeConfig::from_profile(&profile.openbao.as_ref()...)` + env var overrides
5. Keep `::staging()` as a fallback for when no profile is loaded (local dev)

The bridge client methods should reference `self.config.transit.claim_signer_key` etc. instead of hardcoded string literals.

**Verification:**
1. `cargo check --workspace` passes
2. `cargo test -p mai-api --lib` — all existing tests pass
3. Unit test: `from_profile_populates_all_fields()` — construct a profile TOML, build config, assert every field
4. Unit test: `staging_fallback_works()` — `OpenBaoBridgeConfig::staging()` still works when `MAI_OPENBAO_ADDR` is set
5. Integration test: boot with `profile-production.toml`, verify `exchange_token` still works (regression)

**Gate:** `cargo check`, `cargo clippy`, `cargo fmt --check`, `cargo test -p mai-api`, no unsafe, no secrets

---

### Prompt 1.3 — Revocation Snapshot Fetch + Background Refresh Loop

**Input:** Read `mai-api/src/openbao_client.rs`, `mai-compliance/src/trust_cache.rs`, `mai-compliance/src/bundle.rs` (SignedPolicyBundle), `mai-api/src/server.rs` (`apply_ship_profile`), `mai-api/src/state.rs`.

**Task — Part A: Bridge client revocation methods:**

Add to `OpenBaoBridgeClient`:
```rust
/// Fetch signed revocation snapshots from OpenBao KV for a tenant.
/// Returns a Vec<RevocationSnapshot> deserialized from kv/data/revocations/{tenant_id}.
pub async fn fetch_revocation_snapshots(
    &self,
    tenant_id: &str,
) -> Result<Vec<RevocationSnapshot>, OpenBaoBridgeError>

/// Sign a revocation list payload with the revocation-signer Transit key.
/// Returns the base64-encoded signature.
pub async fn sign_revocation_payload(
    &self,
    client_token: &str,
    payload: &[u8],
) -> Result<String, OpenBaoBridgeError>
```

Implementation details:
- `fetch_revocation_snapshots`: AppRole login → GET `kv/data/{revocation_path}/{tenant_id}` → deserialize `data.data` into `Vec<RevocationSnapshot>`. Handle empty/missing path gracefully (return empty vec).
- `sign_revocation_payload`: POST `transit/sign/{revocation_signer_key}` with base64 payload

**Task — Part B: Background refresh loop:**

In `apply_ship_profile()` (server.rs), when `exchange_mode == OpenBaoBridge` and `trust_refresh.enabled`:
1. Clone `Arc<RwLock<LocalTrustCache>>` and `OpenBaoBridgeClient`
2. `tokio::spawn` an async loop:
   - Sleep `interval_secs`
   - Call `bridge.fetch_revocation_snapshots("tribal-health-demo")` (for now, single tenant; follow-up: iterate known tenants)
   - Acquire write lock on trust cache
   - For each snapshot: `cache.revocations.insert(snapshot.claim_id.clone(), snapshot)`
   - Set `cache.last_refresh_secs = now`
   - Drop lock
   - On error: log warning via `tracing::warn!`, do NOT panic, do NOT clear cache (never-clobber invariant)
3. Add `tracing::info!` on first successful refresh and `tracing::warn!` on consecutive failures

**Task — Part C: claim issuance reflects revocation:**

In `issue_claim()`, after building the claim JSON, query the trust cache (passed as parameter or accessed via `AppState`) for `revocation_status(claim_id)`. Replace hardcoded `"valid"` with the actual status from the cache.

Signature change: `issue_claim()` now accepts `&Arc<RwLock<LocalTrustCache>>` (or `Option<&Arc<RwLock<LocalTrustCache>>>` for test backwards-compat).

**Verification:**
1. `cargo check --workspace` passes
2. Write unit test: `fetch_revocation_snapshots_returns_empty_for_missing_path()` — mock HTTP
3. Write unit test: `fetch_revocation_snapshots_deserializes_snapshots()` — mock HTTP response with sample JSON
4. Write unit test: `background_refresh_updates_cache()` — use `LocalTrustCache::default()`, insert snapshots via manual refresh call (calls `record_refresh`), verify `revocation_status()` reflects them
5. Write integration test in `mai-api/tests/trust_production.rs`: `openbao_bridge_claim_reflects_revocation()` — put a revocation snapshot in the cache, call `issue_claim`, assert `revocation_status` is `"revoked"` not `"valid"`
6. Existing tests pass (regression)

**Gate:** `cargo check`, `cargo clippy`, `cargo fmt --check`, `cargo test --workspace`, no unsafe, no secrets

---

### Prompt 1.4 — Force Refresh Endpoint + Airgap Resilience

**Input:** Read `mai-api/src/handlers/trust.rs`, `mai-api/src/server.rs`, `mai-api/src/state.rs`.

**Task — Part A: `POST /v1/trust/refresh` endpoint:**

New handler in `handlers/trust.rs`:
```rust
pub async fn force_refresh(
    State(state): State<AppState>,
) -> Result<Json<TrustRefreshResponse>, AppError>
```

Behavior:
1. If `state.openbao_bridge.is_none()`, return `503` with `mode: "no-bridge"`
2. Call `bridge.fetch_revocation_snapshots("tribal-health-demo")` (singleton for now)
3. Acquire write lock on `state.trust_cache`
4. Insert each snapshot via `cache.record_revocations(snapshots)` (new method, see below)
5. Drop lock
6. Return `TrustRefreshResponse { snapshots_ingested: N, refreshed_at: unix_epoch }`

New method on `LocalTrustCache`:
```rust
pub fn record_revocations(&mut self, snapshots: Vec<RevocationSnapshot>, now_secs: u64) {
    for snap in snapshots {
        self.revocations.insert(snap.claim_id.clone(), snap);
    }
    self.last_refresh_secs = Some(now_secs);
}
```

**Task — Part B: Airgap/Circuit-Breaker Resilience:**

In the background refresh loop and in `force_refresh`:
1. Track consecutive OpenBao failures with an `Arc<AtomicU32>` on `AppState` (new field: `pub openbao_consecutive_failures: Arc<AtomicU32>`)
2. On success: reset to 0
3. On failure: increment, log at `warn!` after 3 consecutive, log at `error!` after 10
4. Add `openbao_health` field to `GET /v1/trust/status` response: `"connected"` (0 failures), `"degraded"` (1-2), `"disconnected"` (3+)

**Verification:**
1. `cargo test -p mai-api --lib` — handler unit tests
2. Integration test: `force_refresh_endpoint_with_no_bridge_returns_503()` — app without bridge, call `POST /v1/trust/refresh`, assert 503
3. Integration test: `force_refresh_updates_cache()` — app with bridge (mocked or real OpenBao), put revocation in KV, call refresh, query revocation_status, assert `"revoked"`
4. Integration test: `consecutive_failures_counter_increments()` — mock failing bridge, call refresh 3 times, assert `/v1/trust/status` shows `"degraded"`
5. Production guard: existing 41 checks still pass

**Gate:** `cargo check`, `cargo clippy`, `cargo fmt --check`, `cargo test --workspace`, no unsafe, no secrets

---

### Prompt 1.5 — End-to-End Revocation Integration Test + Staging Deploy

**Input:** Read `deployment/openbao-staging/ir-respond.ps1`, `deployment/openbao-staging/start-openbao.ps1`, `mai-api/tests/trust_production.rs`.

**Task — Part A: E2E integration test:**

Add to `mai-api/tests/trust_production.rs`:

```rust
#[tokio::test]
async fn e2e_revocation_flow_claim_issue_revoke_refresh_verify() {
    // 1. Start with fresh cache
    // 2. Issue a claim via bridge client — assert revocation_status == "valid"
    // 3. Write revocation snapshot to OpenBao KV (or simulate in cache)
    // 4. Force refresh trust cache
    // 5. Query revocation_status for that claim_id — assert "revoked"
}
```

This test requires OpenBao running. Gate it behind a feature flag `#[cfg(feature = "openbao-e2e")]` so CI doesn't need Docker. Document how to run: `cargo test -p mai-api --test trust_production e2e_revocation --features openbao-e2e -- --nocapture`

**Task — Part B: Update staging scripts:**

1. `start-openbao.ps1`: add KV path `kv/revocations/tribal-health-demo` with initial empty revocation list
2. `ir-respond.ps1`: add subcommand `revoke-claim <claim_id>` that writes `{"claim_id": "...", "status": "Revoked", "recorded_at_secs": ...}` to `kv/revocations/tribal-health-demo`

**Task — Part C: Update profile-production.toml:**

Add the `[openbao]` section with all defaults populated. Ensure `trust_refresh.enabled = true`.

**Task — Part D: Manual staging verification:**

Start the full stack, verify:
1. Boot mai-api with `profile-production.toml` → 41/41 production guard passes
2. Issue a claim via `exchange_token` → returns `revocation_status: "valid"`
3. Run `ir-respond.ps1 revoke-claim <claim_id>`
4. Call `POST /v1/trust/refresh`
5. Query `GET /v1/trust/revocation_status?claim_id=<claim_id>` → returns `"revoked"`
6. Issue a new claim for same subject → `revocation_status: "revoked"`

**Gate:** `cargo check`, `cargo clippy`, `cargo fmt --check`, `cargo test --workspace` (skip e2e unless feature flag), no unsafe, no secrets, manual staging walkthrough passes

---

## TIER 2 — mTLS Bridge (Ring-1→Ring-3 Transport Binding)

**Goal:** The bridge from mai-api to OpenBao uses mutual TLS with a client certificate issued by OpenBao's PKI, binding the appliance's hardware identity to the transport layer. A stolen AppRole bearer token is no longer sufficient to impersonate the appliance.

**Files touched:** `openbao_client.rs`, `Cargo.toml` (mai-api), `start-openbao.ps1`, `profile-production.toml`

**Sub-tasks (3 prompts):**

### Prompt 2.1 — PKI Certificate Issuance Client

**Input:** Read `mai-api/src/openbao_client.rs`, `mai-api/Cargo.toml`.

**Task:**
Add to `OpenBaoBridgeClient`:
```rust
/// Request a client certificate from OpenBao PKI for mTLS.
/// Returns (certificate_pem, private_key_pem, ca_chain_pem).
pub async fn issue_appliance_cert(
    &self,
    client_token: &str,
    common_name: &str,
    ttl: &str,           // e.g. "24h"
) -> Result<IssuedCertificate, OpenBaoBridgeError>

pub struct IssuedCertificate {
    pub certificate: String,       // PEM
    pub private_key: String,       // PEM
    pub ca_chain: Vec<String>,     // PEM chain
    pub serial_number: String,
    pub expiration: i64,           // unix epoch
}
```

Implementation:
- POST `/v1/pki/issue/{pki_role}` with `{"common_name": cn, "ttl": ttl}`
- Deserialize response, extract `data.certificate`, `data.private_key`, `data.ca_chain`, `data.serial_number`, `data.expiration`
- The PKI role and pki mount path come from `self.config.pki`

**Verification:**
1. `cargo check --workspace` passes
2. Unit test: mock HTTP response with sample PKI issue JSON, assert `IssuedCertificate` fields populated
3. Unit test: error path — PKI role not found → `OpenBaoBridgeError::Protocol`

**Gate:** `cargo check`, `cargo clippy`, `cargo fmt --check`, `cargo test -p mai-api --lib`, no unsafe, no secrets

---

### Prompt 2.2 — mTLS reqwest Client Construction

**Input:** Read `mai-api/src/openbao_client.rs`, `mai-api/Cargo.toml`, workspace `Cargo.toml`.

**Task:**
1. Add workspace dependency: `rustls = "0.23"`, `rustls-pemfile = "2"` (or `native-tls` if `reqwest` default features use it — check `Cargo.toml`)
2. Actually, `reqwest` 0.12 defaults to `native-tls`. We can use `reqwest::Identity::from_pem()` and `reqwest::Certificate::from_pem()` with native-tls. No extra deps needed.

Add a builder to `OpenBaoBridgeClient`:
```rust
impl OpenBaoBridgeClient {
    /// Build a new client with mTLS enabled using an issued certificate.
    /// `ca_cert_pem` — the OpenBao PKI CA certificate (PEM) to trust.
    /// `client_cert_pem` — the issued appliance certificate (PEM).
    /// `client_key_pem` — the private key for the issued certificate (PEM).
    pub fn new_with_mtls(
        config: Arc<OpenBaoBridgeConfig>,
        ca_cert_pem: &str,
        client_cert_pem: &str,
        client_key_pem: &str,
    ) -> Result<Self, OpenBaoBridgeError>
}
```

Implementation:
- `reqwest::Identity::from_pem(format!("{client_cert_pem}\n{client_key_pem}").as_bytes())` — client identity
- `reqwest::Certificate::from_pem(ca_cert_pem.as_bytes())` — trusted CA
- `reqwest::Client::builder().add_root_certificate(ca).identity(identity).timeout(config.timeout).build()`
- Error handling: invalid PEM → `OpenBaoBridgeError::Config(...)`

3. Add a `use_tls: bool` flag + `ca_cert_path` and `client_cert_path` fields to `OpenbaoConfig` (profile TOML):
```toml
[openbao.tls]
enabled = true
ca_cert_path = "/etc/mai/certs/openbao-ca.pem"
client_cert_path = "/etc/mai/certs/appliance.pem"
```

4. In `apply_ship_profile()`: if `openbao.tls.enabled`, read cert files from disk and call `new_with_mtls()`. If files don't exist, log warning and fall back to plain HTTP.

5. Update the `address` field to support `https://` — currently hardcoded `http://`.

**Verification:**
1. `cargo check --workspace` passes
2. Unit test: `new_with_mtls_parses_valid_pem()` — generate self-signed cert in test, build client, assert `Ok`
3. Unit test: `new_with_mtls_rejects_invalid_pem()` — assert `Err`
4. Defer full mTLS e2e until OpenBao Docker has TLS listener configured (Prompt 2.3)

**Gate:** `cargo check`, `cargo clippy`, `cargo fmt --check`, `cargo test --workspace`, no unsafe, no secrets in source

---

### Prompt 2.3 — OpenBao Docker TLS Listener + E2E mTLS Test

**Input:** Read `deployment/openbao-staging/start-openbao.ps1`.

**Task — Part A: Dockerfile/listener config:**

OpenBao dev mode with TLS requires one of:
- Option A (easier): Add TLS listener to `BAO_LOCAL_CONFIG` JSON in `start-openbao.ps1`, generate self-signed cert on host, mount it in
- Option B: Use a reverse proxy (nginx/caddy) in a docker-compose — more realistic for production

For staging, do Option A:
1. `start-openbao.ps1` generates a self-signed CA cert + server cert before starting the container
2. Mount certs into container at `/vault/certs/`
3. `BAO_LOCAL_CONFIG` includes `listener.tcp.tls_cert_file` and `listener.tcp.tls_key_file`
4. Bridge address changes from `http://localhost:8200` to `https://localhost:8200`

**Task — Part B: E2E mTLS integration test:**

```rust
#[cfg(feature = "openbao-e2e")]
#[tokio::test]
async fn e2e_mtls_bridge_connects_with_client_cert() {
    // 1. OpenBaoBridgeClient::new() — plain HTTP login
    // 2. Issue appliance cert via PKI
    // 3. Build mTLS client with issued cert
    // 4. Call login() via mTLS client — assert success
}
```

**Task — Part C: Update staging documentation:**

Add to `openbao-connection.toml` (docs-only) a section documenting mTLS setup.

**Verification:**
1. `start-openbao.ps1` starts OpenBao with TLS listener
2. `curl -k https://localhost:8200/v1/sys/health` returns `{"initialized":true,"sealed":false}`
3. E2E test passes with feature flag
4. Production guard: if `openbao.tls.enabled` but `ca_cert_path` doesn't exist on disk, add `PROD-TRUST-104` that fails

**Gate:** `cargo check`, `cargo clippy`, `cargo fmt --check`, `cargo test --workspace`, no unsafe, no secrets in source, manual TLS curl check

---

## TIER 3 — Bundle Signing Unification (Runtime Policy via Transit)

**Goal:** Runtime policy bundles (not boot bundles) are signed by `lamprey-bundle-signer` in OpenBao Transit and verified by the trust cache refresh loop. The boot bundle stays offline ML-DSA for airgap root-of-trust. This adds a live, HSM-backed second signature path for policy updates after boot.

**Files touched:** `openbao_client.rs`, `server.rs`, `trust_cache.rs`, `start-openbao.ps1`, `handlers/trust.rs`

**Sub-tasks (2 prompts):**

### Prompt 3.1 — Bundle Fetch + Sign via Transit

**Input:** Read `mai-api/src/openbao_client.rs`, `mai-compliance/src/bundle.rs` (SignedPolicyBundle, PolicyBundlePayload), `mai-compliance/src/trust_cache.rs`.

**Task:**
Add to `OpenBaoBridgeClient`:
```rust
/// Fetch a signed policy bundle from OpenBao KV.
pub async fn fetch_signed_bundle(
    &self,
    tenant_id: &str,
) -> Result<SignedPolicyBundle, OpenBaoBridgeError>

/// Sign a policy bundle payload with the bundle-signer Transit key.
pub async fn sign_bundle_payload(
    &self,
    client_token: &str,
    payload_json: &str,
) -> Result<TransitSignData, OpenBaoBridgeError>
```

Implementation:
- `fetch_signed_bundle`: login → GET `kv/data/{revocation_path}/{tenant_id}/bundle` (or dedicated `kv/data/bundles/{tenant_id}`) → deserialize `data.data` into `SignedPolicyBundle`. Return `Protocol("no bundle found")` if empty/missing.
- `sign_bundle_payload`: POST `transit/sign/{bundle_signer_key}` with base64-encoded payload

This uses the PROVISIONED-BUT-DORMANT `lamprey-bundle-signer` key.

**Verification:**
1. `cargo check --workspace` passes
2. Unit test: mock HTTP for `fetch_signed_bundle` with sample bundle JSON
3. Unit test: `fetch_signed_bundle_returns_error_for_missing_path()`

**Gate:** `cargo check`, `cargo clippy`, `cargo fmt --check`, `cargo test -p mai-api --lib`, no unsafe

---

### Prompt 3.2 — Trust Cache Signed Refresh from OpenBao

**Input:** Read `mai-api/src/server.rs`, `mai-compliance/src/trust_cache.rs`, `mai-api/src/state.rs`.

**Task:**
Extend the background refresh loop (from Prompt 1.3) to also fetch and verify signed policy bundles:

1. After fetching revocation snapshots, call `bridge.fetch_signed_bundle(tenant_id)`
2. Call `cache.record_signed_refresh(&bundle, verifier.as_ref(), Some(tenant_id), now_secs)`
3. The `BundleVerifier` trait object is already on `AppState` as `Arc<dyn BundleVerifier + Send + Sync>`
4. This uses the existing `record_signed_refresh()` method which verifies signature, checks expiry, checks tenant match — zero new trust cache code needed
5. On verification failure: log `warn!`, leave cache untouched (never-clobber invariant)
6. On success: log `info!` with bundle version

**Verification:**
1. `cargo check --workspace` passes
2. Unit test in trust_cache.rs: `record_signed_refresh_rejects_expired_bundle()` — existing or new
3. Integration test: put a signed bundle in OpenBao KV (via `bao kv put`), start mai-api, wait for refresh loop, query `/v1/trust/status` — assert `bundle_version` is populated
4. Integration test: put a bundle with bad signature → refresh → cache untouched, `bundle_version` unchanged

**Gate:** `cargo check`, `cargo clippy`, `cargo fmt --check`, `cargo test --workspace`, no unsafe

---

## TIER 4 — Operational Hardening

**Goal:** Operators can health-check, rotate, and force-refresh the trust subsystem from the API dashboard without shell access to the deployment host.

**Files touched:** `handlers/trust.rs`, `server.rs`, `state.rs`, `openbao_client.rs`, `ir-respond.ps1`

**Sub-tasks (3 prompts):**

### Prompt 4.1 — OpenBao Health Check Endpoint

**Input:** Read `mai-api/src/handlers/trust.rs`, `mai-api/src/openbao_client.rs`, `mai-api/src/state.rs`.

**Task:**
Add `GET /v1/trust/openbao_health` that probes OpenBao and returns:
```json
{
  "openbao_reachable": true,
  "sealed": false,
  "mounts_healthy": {"kv": true, "transit": true, "pki": true, "approle": true},
  "demo_tenant_exists": true,
  "claim_signer_key_exists": true,
  "audit_enabled": true,
  "latency_ms": 12,
  "consecutive_failures": 0,
  "last_successful_probe_secs": 1716755600
}
```

Implementation:
- `bridge.health_check()` — calls `GET /v1/sys/health`, `GET /v1/sys/mounts`, `GET /v1/sys/auth`, reads KV tenant, checks transit key exists
- Return structured JSON, not raw OpenBao errors
- Time each call and report `latency_ms`
- Pull `consecutive_failures` from `AppState.openbao_consecutive_failures`
- Update `AppState` with `last_successful_health_probe: Arc<AtomicI64>` (unix epoch, -1 = never)

**Verification:**
1. `cargo test -p mai-api --lib` — handler test with mocked bridge
2. Integration test: with running OpenBao, `GET /v1/trust/openbao_health` returns 200 and `openbao_reachable: true`
3. Integration test: with OpenBao stopped, returns 503 and `openbao_reachable: false`

**Gate:** `cargo check`, `cargo clippy`, `cargo fmt --check`, `cargo test --workspace`, no unsafe

---

### Prompt 4.2 — Credential Rotation Hot-Reload

**Input:** Read `mai-api/src/openbao_client.rs`, `mai-api/src/server.rs`, `mai-api/src/state.rs`.

**Task:**
Add `POST /v1/admin/rotate-credentials` that rotates the AppRole secret_id without restarting:

1. New handler: `handlers/trust.rs` → `rotate_appliance_credentials()`
2. Requires API key auth with `admin` scope
3. Calls OpenBao: `POST /v1/auth/approle/role/mai-appliance/secret-id` to generate new secret_id
4. Builds a new `OpenBaoBridgeClient` with the new secret_id
5. Swaps `state.openbao_bridge` atomically: wrap in `Arc<RwLock<Option<OpenBaoBridgeClient>>>` instead of `Option<OpenBaoBridgeClient>`
6. Returns new accessor (not secret_id — the secret is set server-side)

Wait — currently `AppState.openbao_bridge` is `Option<OpenBaoBridgeClient>` which derives `Clone`. To support hot-swap, wrap it in `Arc<RwLock<Option<OpenBaoBridgeClient>>>`.

Actually, a simpler approach: `AppState.openbao_bridge` becomes `Arc<tokio::sync::RwLock<Option<OpenBaoBridgeClient>>>`. The `OpenBaoBridgeClient` itself is cheap to clone (it's just `reqwest::Client` + `Arc<Config>`).

The handler:
1. Authenticate via admin API key
2. Call OpenBao to generate new secret_id
3. Build new `OpenBaoBridgeClient::new(config.with_secret_id(new_secret))`
4. `*state.openbao_bridge.write().await = Some(new_bridge)`
5. Revoke the old secret_id accessor (stored from prior rotation)
6. Return `204 No Content`

**Verification:**
1. `cargo test -p mai-api --lib` — handler test
2. Integration test: issue several claims → rotate credentials → issue more claims → all succeed
3. Integration test: rotate with invalid admin key → 401

**Gate:** `cargo check`, `cargo clippy`, `cargo fmt --check`, `cargo test --workspace`, no unsafe

---

### Prompt 4.3 — Server-Side TLS Discussion (No Code)

**Task:** Document the TLS termination architecture decision in `docs/tls-architecture.md`:

- The mai-api application binds plain TCP (`axum::serve(listener, router)` on `0.0.0.0:8443`)
- The ship profile's `tls_mode = "reverse-proxy-required"` means TLS is an external responsibility
- For staging: no reverse proxy yet — direct plain HTTP is acceptable
- For production: recommend nginx/caddy with Let's Encrypt or internal CA
- The `TlsConfig` struct in `ship_profile.rs` is dead code; document whether to remove it or implement it in a follow-up

**Gate:** Docs only — no code changes, no tests needed

---

## Execution Order

```
Phase 1: Foundation
├── 1.1  Ship Profile [openbao] section
├── 1.2  OpenBaoBridgeConfig from Profile
└── 1.3  Revocation fetch + refresh loop + claim reflection
    └── GIT CHECKPOINT: commit + push

Phase 2: Revocation Complete
├── 1.4  Force refresh endpoint + airgap resilience
├── 1.5  E2E regression + staging deploy
└── └── GIT CHECKPOINT: commit + push — TIER 1 DONE

Phase 3: mTLS
├── 2.1  PKI certificate issuance client
├── 2.2  mTLS reqwest client construction
├── 2.3  OpenBao Docker TLS listener + E2E
└── └── GIT CHECKPOINT: commit + push — TIER 2 DONE

Phase 4: Bundle + Ops
├── 3.1  Bundle fetch + sign via Transit
├── 3.2  Trust cache signed refresh loop
├── 4.1  OpenBao health endpoint
├── 4.2  Credential rotation hot-reload
├── 4.3  TLS architecture doc
└── └── GIT CHECKPOINT: commit + push — TIERS 3+4 DONE
```

## CI Regression Guard

After each phase, run the full suite:
```powershell
cargo check --workspace
cargo clippy --workspace -- -D warnings -A clippy::pedantic
cargo fmt --check
cargo test --workspace
```

The `production_guard` test must remain at 41/41 checks with 0 failures. If a new tier adds guard rules, update the count in the test assertion.

## Environment Setup (before any prompt)

```powershell
# Set the live secret for mai-api
$env:MAI_OPENBAO_SECRET_ID = 'b89104a5-3e84-7995-4871-a8bca4c7fc3f'

# Ensure OpenBao is running with audit
Push-Location "deployment/openbao-staging"
.\start-openbao.ps1
Pop-Location

# Verify
.\ir-respond.ps1 health-check
```
