# MAI Ship Hardening Plan

> **STATUS — CLOSED (2026-05-23)**
> All 17 SHIP sessions landed (SHIP-01..SHIP-17). Lane is complete; no further SHIP work queued. Only follow-up surfaced post-close is mai-sdk-rs HTTP client `todo!()` stubs (no in-tree consumer; not lane-blocking). Kept as the canonical record of what hardening covered; current build work is in the DOUGHERTY lane — see `mai/docs/dougherty/JOHN-REMEDIATION-PLAN.md`.

**Project:** Island Mountain Mighty Eel OS / MAI / Lamprey  
**Purpose:** Convert the current substantial local-first MAI/Lamprey stack from demo-safe and acquisition-ready into a no-caveats shippable product posture.  
**Audience:** Claude Code, Codex, implementation agents, release engineers, security reviewers.  
**Status:** Execution plan — **CLOSED 2026-05-23**.  
**Last updated:** 2026-05-23.

---

## 0. Executive Objective

The current MAI stack has real architecture: scheduler, adapters, REST/gRPC API, auth, rate limiting, compliance policy modules, audit chain primitives, Python/Rust SDKs, demos, integration tests, deployment profiles, and acquisition documentation.

The remaining problem is not "build the product from scratch." The remaining problem is production hardening:

1. Remove demo-safe defaults from all production startup paths.
2. Persist critical trust, audit, vault, model, and registry state.
3. Package the appliance as something an operator can install, run, monitor, back up, restore, and upgrade.
4. Make the production profile fail closed when any stub, no-op verifier, in-memory critical store, dev token, or insecure bind setting is active.
5. Add release gates that prove the product survives hardware burn-in, restarts, degradation, and recovery.

The target outcome is a `ship` profile with a single validation command that answers:

> Can this MAI node be shipped to a regulated customer without relying on demo shortcuts?

That command must return non-zero if the answer is anything other than yes.

---

## 1. Non-Negotiable Ship Criteria

Do not call the product shippable until every item below is true.

### 1.1 Runtime Safety

- `mai-api` production startup never constructs `StubVault`.
- `mai-api` production startup never constructs `MemoryAuditWriter`.
- `AppState` production startup never defaults to `AcceptAllBundleVerifier`.
- Compliance audit storage never uses `NullSealer` in production.
- Dashboard never accepts `dashboard-dev` in production.
- `POST /v1/auth/exchange_token` cannot mint synthetic local-dev tokens in production.
- Production profile refuses `allow_internal_profile_header = true`.
- Production profile refuses loopback-only "demo" trust settings unless explicitly named `local-dev`.
- Production profile refuses missing persistent paths for audit, vault, trust cache, model registry, and reports.
- Production profile refuses unsigned or unverifiable policy bundles.

### 1.2 Persistence

- API audit entries survive process restart.
- Compliance audit WAL survives process restart.
- Vault master material is initialized once, sealed, and recoverable under documented operator procedure.
- Trust bundles survive restart and are re-verified on boot.
- Model registry state survives restart.
- Compliance reports and certification metadata survive restart.
- Rotation, retention, pruning, and integrity verification behavior is documented and tested.

### 1.3 Packaging

- There is a real installable artifact or deterministic package build path.
- Linux service units exist for `mai-api` and any required companion processes.
- Config files have a production directory layout.
- First-boot key and secret bootstrap is deterministic, auditable, and does not leak secrets to persistent logs.
- Upgrade procedure preserves state and rolls back safely.

### 1.4 Recovery

- Backup and restore is implemented and tested for:
  - vault storage
  - API audit log
  - compliance audit WAL
  - trust bundle cache
  - model registry
  - report output
  - auth key config
- A restore drill proves a fresh node can recover from backup and pass integrity checks.
- Audit chain verification runs after restore.
- Trust bundle verification runs after restore.

### 1.5 Observability

- Structured logs are persisted with rotation.
- Metrics are exported in a machine-readable format.
- Health endpoints distinguish ready, live, degraded, and unsafe production states.
- Alerts exist for:
  - audit write failure
  - audit chain break
  - trust bundle stale/expired
  - production verifier missing
  - vault unavailable
  - adapter process crash loop
  - scheduler no healthy backend
  - disk capacity nearing retention failure
  - rate limit abuse
  - air-gap violation

### 1.6 Release Gates

- CI fails on Rust compile, clippy, format, tests.
- Python lint and type checks are enforced, not advisory.
- No-GPU integration runs on every PR.
- GPU integration runs on self-hosted runners before release.
- Benchmark regression check runs before release.
- 72-hour burn-in runs before release.
- `ship-profile-validate` runs before release.
- Release artifacts include build metadata, config checksum, test evidence, and known deferrals.

---

## 2. Workstream Overview

Execute in this order. Later workstreams assume earlier workstreams are complete.

| Workstream | Name | Outcome |
|---:|---|---|
| 1 | Production Profile Contract | A canonical `ship` profile and config contract exists. |
| 2 | Persistent Vault Wiring | `mai-api` boots with real vault storage outside test/dev. |
| 3 | Persistent Audit Wiring | API and compliance audit survive restart and verify. |
| 4 | Fail-Closed Startup Guard | Production refuses all demo-safe defaults. |
| 5 | Trust Bridge Production Mode | Token exchange and bundle verification are real or disabled. |
| 6 | Packaging and Service Layout | Installable/service-managed product shape exists. |
| 7 | Backup, Restore, and DR | Operators can recover state and prove integrity. |
| 8 | Observability and Alerting | Logs, metrics, health, and alerts are production-grade. |
| 9 | Release Gates and Burn-In | CI and hardware validation become release blockers. |
| 10 | Documentation and Runbooks | Operators can install, run, recover, and verify. |
| 11 | Final Ship Gate | One command proves the node has no unsafe defaults. |

---

## 3. Workstream 1: Production Profile Contract

### Goal

Create a canonical production profile that is distinct from `local-dev`, `cloud-trust-core`, `local-mai-node`, and `airgap-demo`. Name it `ship` or `production-local-node`. This profile defines what a customer-running node must provide.

### New Files

- `deployment/ship/README.md`
- `deployment/ship/profile.toml`
- `config/production.example.toml`
- `config/ship-validator.toml`
- `docs/SHIP-PROFILE.md`

### Expected Profile Fields

Add or standardize fields for:

```toml
[profile]
name = "ship"
mode = "production"
allow_demo_defaults = false
fail_closed = true

[paths]
state_dir = "/var/lib/mai"
config_dir = "/etc/mai"
log_dir = "/var/log/mai"
run_dir = "/run/mai"
backup_dir = "/var/backups/mai"

[vault]
backend = "zfs"
root = "/var/lib/mai/vault"
require_sealed_master_key = true
require_pqc = true
allow_stub = false

[audit]
api_writer = "wal"
compliance_writer = "wal"
wal_dir = "/var/lib/mai/audit"
require_hash_chain = true
require_pqc_checkpoints = true
require_encryption_at_rest = true
allow_memory_writer = false
allow_null_sealer = false

[trust]
bundle_cache_dir = "/var/lib/mai/trust"
verifier = "ml-dsa"
allow_accept_all_verifier = false
allow_local_dev_exchange = false
require_trust_anchor = true
require_bundle_on_boot = true

[auth]
auth_keys_path = "/etc/mai/auth_keys.toml"
allow_internal_profile_header = false
require_nonempty_key_store = true

[dashboard]
enabled = true
allow_default_admin_token = false

[network]
bind_address = "127.0.0.1"
tls_mode = "reverse-proxy-required"
require_forwarded_proto_header = false

[observability]
log_format = "json"
log_rotation = true
metrics_exporter = "prometheus"
alerts_enabled = true
```

### Implementation Tasks

1. Add a production profile file under `deployment/ship/`.
2. Extend server config loading so `MAI_PROFILE=ship` selects strict production behavior.
3. Add a typed representation of production profile flags if no suitable type exists.
4. Ensure `local-dev` remains convenient, but all demo defaults are explicitly scoped to local-dev only.
5. Add docs that explain the difference between:
   - `local-dev`: developer bring-up
   - `airgap-demo`: demo posture
   - `local-mai-node`: integration posture
   - `ship`: customer production posture

### Acceptance Tests

Add tests that assert:

- `ship` profile parses.
- `ship` profile has `fail_closed = true`.
- `ship` profile rejects missing persistent paths.
- `ship` profile rejects missing trust anchor.
- `ship` profile rejects missing audit WAL path.
- `ship` profile rejects `allow_demo_defaults = true`.

Suggested test locations:

- `mai-api/tests/ship_profile.rs`
- `mai-api/src/config.rs` unit tests

### Done When

- Running `cargo test -p mai-api ship_profile` passes.
- `deployment/ship/README.md` gives an operator enough context to understand what the profile enforces.

---

## 4. Workstream 2: Persistent Vault Wiring

### Goal

Replace `StubVault` in `mai-api` startup with a profile-selected real vault for production. `StubVault` may remain for tests and explicit local-dev mode only.

### Current Risk

`mai-api/src/server.rs` constructs `StubVault` during server bootstrap. That means the running server does not use the real `mai-vault` path even though real vault code exists.

### Implementation Tasks

1. Add a `VaultBackendConfig` to server config:

```rust
pub enum VaultBackendKind {
    Stub,
    Zfs,
    FileDev,
}

pub struct VaultRuntimeConfig {
    pub backend: VaultBackendKind,
    pub root: PathBuf,
    pub require_pqc: bool,
    pub require_tpm_seal: bool,
    pub allow_stub: bool,
}
```

2. Add a `build_vault(config: &ServerConfig) -> Result<Box<dyn VaultInterface>, ServerError>` helper.
3. In local-dev:
   - Allow `StubVault` only if the profile explicitly says `allow_stub = true`.
4. In ship:
   - Refuse `StubVault`.
   - Require vault root.
   - Require first-boot initialization or existing sealed vault.
   - Require signature verification provider.
5. Wire `mai-vault::ZfsVault` or the closest current concrete vault type into `ModelRegistry`.
6. Add clear startup log lines:
   - backend type
   - vault root
   - sealed/unsealed status
   - PQC provider mode
   - never log secret material
7. Update `docs/KNOWN-ISSUES.md` after implementation to remove or revise the bootstrap stub caveat.

### First-Boot Behavior

Production first boot should:

1. Detect empty vault root.
2. Run vault initialization.
3. Create master keypair.
4. Seal key material.
5. Initialize storage directories.
6. Write a first-boot record to audit.
7. Refuse to continue if initialization partially fails.

Do not silently fall back to a stub after any production vault error.

### Acceptance Tests

Add tests for:

- `ship` profile refuses `StubVault`.
- local-dev may use `StubVault`.
- production vault path must be configured.
- production vault initialization creates required directories.
- registry can list/load model metadata using real vault boundary.
- startup fails if production vault is unavailable.

Suggested locations:

- `mai-api/tests/vault_bootstrap.rs`
- `mai-vault/tests/first_boot.rs`

### Done When

- `rg "StubVault" mai-api/src/server.rs` shows it is used only in test/local-dev-specific code paths.
- A production config cannot boot with `StubVault`.

---

## 5. Workstream 3: Persistent Audit Wiring

### Goal

Persist both API audit and compliance audit records to disk by default under the ship profile, with hash-chain verification and optional PQC checkpoint signatures.

### Current Risk

`mai-api` uses `MemoryAuditWriter` in server startup. Compliance audit has WAL support, but defaults and production wiring need enforcement. Critical audit evidence must not evaporate on process exit.

### Implementation Tasks

1. Create a persistent API audit writer if one does not already exist:

```rust
pub struct WalAuditWriter {
    wal_path: PathBuf,
    chain_state_path: PathBuf,
    signer: Option<Arc<dyn AuditSigner>>,
}
```

2. Implement the existing `AuditWriter` trait for it.
3. Store entries as JSONL or another append-only format.
4. Persist chain tail:
   - previous hash
   - last sequence number
   - last checkpoint signature
5. On startup:
   - replay WAL
   - verify chain
   - refuse production boot if verification fails
6. Add WAL rotation:
   - daily or size-based
   - keep chain continuity across rotations
7. Add retention metadata:
   - default seven years for HIPAA-aligned posture
   - pruning command must verify checkpoint boundaries first
8. Wire compliance audit store with:
   - real WAL path
   - real sealer
   - configured retention
   - checkpoint signer if available
9. Replace `NullSealer` in ship mode with vault-backed AEAD sealer.
10. Add audit write failure policy:
    - production inference must fail closed if audit append fails for regulated request
    - health endpoint reports unsafe/degraded audit state

### Config Shape

```toml
[audit.api]
writer = "wal"
path = "/var/lib/mai/audit/api"
rotate = "daily"
fail_closed_on_write_error = true

[audit.compliance]
writer = "wal"
path = "/var/lib/mai/audit/compliance"
sealer = "vault-aead"
checkpoint_signing = "ml-dsa"
retention_days = 2555
fail_closed_on_write_error = true
```

### Acceptance Tests

Add tests for:

- API audit survives restart.
- Compliance audit survives restart.
- Chain verification succeeds after restart.
- Tampered WAL fails verification.
- Missing WAL directory fails production startup.
- Audit write failure causes regulated request refusal.
- `ship` profile rejects `MemoryAuditWriter`.
- `ship` profile rejects `NullSealer`.

Suggested locations:

- `mai-api/tests/audit_persistence.rs`
- `mai-compliance/tests/audit_wal.rs`
- `mai-api/tests/ship_fail_closed.rs`

### Done When

- `docs/DEPLOYMENT.md` no longer says default API audit is in-memory for production.
- `MemoryAuditWriter` remains available only for tests/dev.

---

## 6. Workstream 4: Fail-Closed Startup Guard

### Goal

Create one centralized production guard that scans runtime config and constructed components before the server begins listening.

### New Module

- `mai-api/src/production_guard.rs`

### Guard Responsibilities

The guard must reject ship-mode startup if any of these are true:

- `StubVault` selected.
- `MemoryAuditWriter` selected.
- `AcceptAllBundleVerifier` selected.
- `NullSealer` selected.
- `POST /v1/auth/exchange_token` configured in local-dev synthetic mode.
- `dashboard-dev` token present while dashboard is enabled.
- `allow_internal_profile_header = true`.
- No API keys configured.
- Bind address is public and TLS/reverse-proxy mode is not explicitly configured.
- Trust anchor missing.
- Trust bundle missing or unverifiable.
- Audit WAL missing or unverifiable.
- Vault path missing or uninitialized.
- Compliance policy config missing.
- Production profile has `allow_demo_defaults = true`.

### Implementation Pattern

Add a typed report:

```rust
pub struct ProductionReadinessReport {
    pub profile: String,
    pub checks: Vec<ProductionCheck>,
}

pub struct ProductionCheck {
    pub id: &'static str,
    pub severity: CheckSeverity,
    pub status: CheckStatus,
    pub message: String,
    pub remediation: String,
}
```

Expose it through:

- Startup guard: fails before bind.
- CLI command: `mai-api validate --profile deployment/ship/profile.toml`
- HTTP endpoint for admins: `GET /v1/system/production-readiness`

In production, the HTTP endpoint must never reveal secrets.

### Acceptance Tests

Add tests for each forbidden configuration. Each test should assert a specific check ID, not just "startup failed."

Suggested check IDs:

- `PROD-VAULT-001`
- `PROD-AUDIT-001`
- `PROD-TRUST-001`
- `PROD-AUTH-001`
- `PROD-DASH-001`
- `PROD-NET-001`
- `PROD-POLICY-001`

### Done When

- There is no scattered production safety logic hidden in random modules.
- One guard produces human-readable and machine-readable readiness output.

---

## 7. Workstream 5: Trust Bridge Production Mode

### Goal

Keep local-dev trust ergonomics, but prevent production from using synthetic token exchange or accept-all verification.

### Implementation Tasks

1. Add `TrustExchangeMode`:

```rust
pub enum TrustExchangeMode {
    LocalDevSynthetic,
    OpenBaoBridge,
    Disabled,
}
```

2. In `ship` mode:
   - allow only `OpenBaoBridge` or `Disabled`
   - reject `LocalDevSynthetic`
3. Implement `OpenBaoBridge` client abstraction:
   - endpoint URL
   - mTLS/client identity config placeholder if local appliance calls bridge directly
   - request/response schema already fixed by BF-6 contract
4. Add timeout and retry policy.
5. Add explicit offline behavior:
   - if bridge unreachable but valid bundle exists, use local trust cache according to policy
   - if bundle expired, fail closed
6. Wire real `MlDsaBundleVerifier` in production.
7. Load trust anchors from configured path.
8. Verify bundle on boot before accepting requests.
9. Add revocation snapshot loading and expiry enforcement.
10. Add audit entries for:
    - bridge exchange success
    - bridge exchange failure
    - bundle accepted
    - bundle rejected
    - revocation snapshot accepted/rejected
    - offline/degraded transition

### Acceptance Tests

Add tests for:

- ship mode rejects local-dev token exchange.
- ship mode rejects accept-all bundle verifier.
- valid signed bundle boots.
- tampered bundle blocks boot.
- expired bundle boots only into restricted mode if policy allows.
- missing trust anchor blocks boot.
- OpenBao bridge timeout does not leak prompt/completion data.
- trust events are audit-correlated.

Suggested locations:

- `mai-api/tests/trust_production.rs`
- `mai-compliance/tests/trust_cache_production.rs`

### Done When

- `/v1/auth/exchange_token` behavior is profile-dependent.
- Production cannot mint synthetic claims.

---

## 8. Workstream 6: Packaging and Service Layout

### Goal

Make MAI installable and service-managed.

### Required Artifacts

Add:

- `packaging/debian/`
- `packaging/systemd/mai-api.service`
- `packaging/systemd/mai-dashboard.service`
- `packaging/systemd/mai-adapter-manager.service` if separate process management is needed
- `packaging/systemd/mai-healthcheck.timer`
- `packaging/systemd/mai-healthcheck.service`
- `packaging/scripts/preinstall.sh`
- `packaging/scripts/postinstall.sh`
- `packaging/scripts/preremove.sh`
- `packaging/scripts/postremove.sh`
- `packaging/README.md`
- `scripts/build-package.sh`
- `scripts/build-package.ps1` if Windows build orchestration is useful

### Filesystem Layout

Production install should use:

```text
/usr/bin/mai-api
/usr/bin/mai-ship-validate
/usr/lib/mai/adapters/
/usr/lib/mai/compliance-dashboard/
/etc/mai/
/etc/mai/auth_keys.toml
/etc/mai/profile.toml
/etc/mai/policies/
/etc/mai/trust-anchors/
/var/lib/mai/
/var/lib/mai/vault/
/var/lib/mai/audit/
/var/lib/mai/trust/
/var/lib/mai/models/
/var/lib/mai/reports/
/var/log/mai/
/run/mai/
```

### systemd Requirements

`mai-api.service` should:

- run as dedicated `mai` user
- avoid root unless hardware access requires a documented group
- set `WorkingDirectory=/var/lib/mai`
- set `Environment=MAI_PROFILE=ship`
- set config path explicitly
- restart on failure with sane backoff
- limit privileges:
  - `NoNewPrivileges=true`
  - `PrivateTmp=true`
  - `ProtectSystem=strict` if compatible
  - `ReadWritePaths=/var/lib/mai /var/log/mai /run/mai`
- set file descriptor limits appropriate for concurrent requests
- direct logs to journald and/or JSON file sink

### Package Build Requirements

The package build should:

1. Build release binaries.
2. Vendor or install Python dashboard dependencies deterministically.
3. Include default config templates, not live secrets.
4. Include systemd units.
5. Include migration scripts.
6. Include package metadata with git commit and build time.
7. Run `mai-ship-validate --offline --package-root <staging-dir>` before producing package.

### Acceptance Tests

Add tests/scripts for:

- package builds from clean checkout.
- package contains required files.
- service file passes `systemd-analyze verify` on Linux.
- config templates parse.
- postinstall creates directories with correct permissions.
- uninstall does not delete customer data unless explicitly purged.

### Done When

- A release engineer can produce an installable artifact without hand-copying files.

---

## 9. Workstream 7: Backup, Restore, and Disaster Recovery

### Goal

Provide operator-grade backup and restore for every piece of critical state.

### New Tools

- `tools/mai-admin/src/main.rs` or `tools/admin/mai_admin.py`
- Commands:
  - `mai-admin backup create`
  - `mai-admin backup verify`
  - `mai-admin restore plan`
  - `mai-admin restore apply`
  - `mai-admin audit verify`
  - `mai-admin trust verify`
  - `mai-admin vault status`

### Backup Contents

Each backup must include:

- manifest
- version metadata
- git/build version
- profile name
- config checksums
- vault snapshot reference or encrypted vault export
- API audit WAL
- compliance audit WAL
- trust bundles and revocation snapshots
- trust anchors public material
- model registry metadata
- report output
- auth key hashes, never raw keys
- migration version

### Backup Manifest Shape

```json
{
  "backup_id": "mai-backup-2026-05-23T12-00-00Z",
  "created_at": "2026-05-23T12:00:00Z",
  "mai_version": "...",
  "profile": "ship",
  "components": [
    {
      "name": "api_audit_wal",
      "path": "audit/api/current.jsonl",
      "sha3_256": "...",
      "entry_count": 12345,
      "last_entry_hash": "..."
    }
  ],
  "signatures": {
    "manifest_mldsa": "..."
  }
}
```

### Restore Requirements

Restore must:

1. Refuse to overwrite live state unless `--force` and service is stopped.
2. Verify manifest signature.
3. Verify file checksums.
4. Verify audit chains.
5. Verify trust bundles.
6. Verify vault seal state.
7. Reconstruct directory permissions.
8. Produce a restore report.
9. Run `mai-ship-validate` after restore.

### DR Drills

Add scripted drills:

- restore to empty node
- restore after audit WAL tamper attempt
- restore after missing trust bundle
- restore after model registry metadata loss
- restore after interrupted backup
- restore from previous package version then migrate

### Acceptance Tests

Add integration tests:

- create backup from test fixture state
- verify backup
- restore into temp state dir
- boot API against restored state
- query audit and trust status after restore
- tampered backup is rejected

Suggested locations:

- `tests/recovery/`
- `mai-api/tests/recovery_boot.rs`

### Done When

- A clean machine can recover a node from backup and pass ship validation.

---

## 10. Workstream 8: Observability and Alerting

### Goal

Make production state visible, machine-readable, and actionable.

### Logging

Implement or document:

- JSON logs by default in ship profile.
- Log rotation:
  - journald settings, or
  - file appender with rotation.
- Redaction:
  - no raw API keys
  - no prompts
  - no completions
  - no embeddings
  - no raw PHI
  - no OpenBao tokens
- Correlation IDs in all request logs.
- Audit write result in logs without duplicating audit payload.

### Metrics

Expose Prometheus-compatible endpoint or documented metrics JSON:

- requests_total
- request_duration_ms
- auth_failures_total
- rate_limited_total
- audit_write_failures_total
- audit_chain_status
- trust_bundle_age_seconds
- trust_bundle_signature_status
- trust_connectivity_state
- scheduler_queue_depth
- scheduler_decision_latency_us
- adapter_health
- adapter_restart_count
- gpu_memory_used_bytes
- kv_cache_used_bytes
- policy_decisions_total by module/decision
- compliance_report_generation_total
- backup_success_total
- backup_failure_total

### Health Semantics

Define:

- live: process is running
- ready: can serve allowed traffic
- degraded: can serve restricted traffic
- unsafe: production invariant violated, should not serve

Add endpoints:

- `GET /v1/health/live`
- `GET /v1/health/ready`
- `GET /v1/health/production`
- `GET /v1/metrics`

### Alerts

Add alert rules as config and docs:

- `AuditWriteFailure`
- `AuditChainBroken`
- `TrustBundleExpired`
- `TrustBundleStale`
- `ProductionGuardViolation`
- `VaultUnavailable`
- `AdapterCrashLoop`
- `NoHealthyInferenceBackend`
- `AirGapViolation`
- `DiskNearFull`
- `BackupFailed`
- `PolicyReloadFailed`
- `DashboardDefaultToken`

### Acceptance Tests

Add tests for:

- metrics endpoint does not expose secrets.
- health ready fails when audit writer fails.
- health production fails when production guard fails.
- alert emitted on trust bundle expiry.
- alert emitted on audit verification failure.
- logs include correlation ID.
- logs do not include prompt text from test request.

### Done When

- Operators can monitor MAI without scraping ad hoc logs.

---

## 11. Workstream 9: Release Gates and Burn-In

### Goal

Turn current test scripts into enforceable release gates.

### CI Changes

Update `.github/workflows/ci.yml`:

1. Remove `continue-on-error: true` from `mypy`.
2. Add package build job.
3. Add ship validator job.
4. Add audit persistence tests.
5. Add recovery tests.
6. Add production guard tests.
7. Keep no-GPU tests on PR.
8. Enable scheduled nightly no-GPU burn-in.

### GPU Runner Workflow

Create:

- `.github/workflows/gpu-release.yml`

This should run on self-hosted GPU runners:

- Scout config boot
- Ranger config boot
- NVIDIA path
- AMD path if runner exists
- real adapter inference
- streaming inference
- scheduler placement under load
- KV pressure
- batch pressure
- adapter crash/restart
- thermal/degraded behavior if supported
- benchmark compare

### 72-Hour Burn-In

Create or extend:

- `scripts/burn-in-72h.sh`
- `scripts/burn-in-72h.ps1`

The burn-in should:

1. Start packaged service.
2. Run mixed workload.
3. Include policy-triggering prompts without logging payload content.
4. Exercise trust degradation.
5. Exercise adapter restart.
6. Exercise backup during load.
7. Exercise restore in side environment.
8. Capture metrics.
9. Capture final ship validation.
10. Emit signed burn-in report.

### Benchmark Gates

Define thresholds:

- p50/p95/p99 scheduler decision latency
- p50/p95/p99 API latency for no-op and real adapter paths
- streaming first-token latency
- maximum memory growth over 72 hours
- adapter crash recovery time
- audit append latency
- policy decision latency

### Acceptance Tests

- CI fails if ship validator fails.
- CI fails if package cannot build.
- CI fails if Python type check fails.
- GPU workflow produces artifact report.
- 72-hour script can run in shorter `--smoke` mode for CI sanity.

### Done When

- Release cannot be cut without passing explicit ship gates.

---

## 12. Workstream 10: Documentation and Runbooks

### Goal

Make the product operable by someone who did not build it.

### Required Docs

Add or update:

- `docs/SHIP-PROFILE.md`
- `docs/INSTALL.md`
- `docs/FIRST-BOOT.md`
- `docs/OPERATIONS.md`
- `docs/BACKUP-RESTORE.md`
- `docs/OBSERVABILITY.md`
- `docs/RELEASE-GATES.md`
- `docs/SECURITY-PRODUCTION.md`
- `docs/TRUST-BRIDGE-PRODUCTION.md`
- `docs/AUDIT-RETENTION.md`
- `docs/UPGRADE-ROLLBACK.md`
- `docs/INCIDENT-RESPONSE.md`

### Runbooks

Create concise operator runbooks:

- First boot and key capture.
- Rotate API key.
- Rotate trust anchor.
- Install new policy bundle.
- Verify audit chain.
- Generate compliance report.
- Back up node.
- Restore node.
- Recover from failed upgrade.
- Adapter crash loop.
- Trust bundle expired.
- Audit WAL tamper detected.
- Air-gap violation.
- Disk almost full.

### Documentation Rules

Every production doc must clearly distinguish:

- local-dev behavior
- demo behavior
- ship behavior

No production doc may instruct operators to use:

- `dashboard-dev`
- `AcceptAllBundleVerifier`
- synthetic token exchange
- in-memory audit
- stub vault
- null sealer

### Done When

- A new operator can install, validate, back up, restore, and monitor the node using docs only.

---

## 13. Workstream 11: Final Ship Gate

### Goal

Create one command that tells the truth.

### Command

Prefer:

```bash
mai-ship-validate --profile /etc/mai/profile.toml
```

or:

```bash
cargo run -p mai-api -- validate --profile deployment/ship/profile.toml
```

### Required Checks

The validator must check:

#### Config

- profile is `ship`
- `fail_closed = true`
- required paths exist
- path permissions are acceptable
- no dev dashboard token
- no internal profile header
- nonempty auth key store

#### Vault

- real backend selected
- vault initialized
- vault sealed/unsealed state valid
- PQC provider available
- signature verification path works

#### Audit

- persistent writer configured
- WAL path exists
- WAL append test succeeds
- chain verifies
- checkpoint signature verifies if configured
- sealer is not null

#### Trust

- real verifier configured
- trust anchors loaded
- current bundle verifies
- revocation snapshot state known
- local-dev exchange disabled

#### Compliance

- policy modules load
- policy composer works
- HIPAA/ITAR/EAR/OCAP configs parse
- deny-wins conflict resolution test passes

#### API

- REST routes build
- gRPC routes build
- health routes respond locally if server is running
- auth rejects missing token
- rate limiter configured

#### Packaging

- build metadata present
- service units present if installed
- migration version known

#### Recovery

- latest backup exists or policy says not required yet
- backup verifies if present
- restore drill artifact exists for release builds

### Output

Human-readable:

```text
MAI Ship Validation: FAIL

[FAIL] PROD-AUDIT-001: API audit writer is MemoryAuditWriter.
       Remediation: configure audit.api.writer = "wal" and set audit.api.path.

[PASS] PROD-AUTH-001: API key store is nonempty.
...
```

Machine-readable:

```bash
mai-ship-validate --json
```

### Exit Codes

- `0`: ship-ready
- `1`: validation failed
- `2`: config unreadable
- `3`: state unreadable
- `4`: internal validator error

### Done When

- Release process refuses to proceed unless validator exits `0`.

---

## 14. Suggested Execution Sessions

Claude Code should implement this in bounded sessions. Each session must leave the repo buildable.

### Session SHIP-01: Production Profile Skeleton

- Add `deployment/ship/`.
- Add profile parsing tests.
- Add docs for profile modes.
- No behavior changes yet except parsing.

### Session SHIP-02: Production Guard Core

- Add `production_guard.rs`.
- Add report structs and check IDs.
- Add config-only checks.
- Add CLI validation entry if easy.

### Session SHIP-03: Vault Builder

- Add `build_vault`.
- Keep local-dev behavior unchanged.
- Add ship-mode rejection for `StubVault`.
- Wire real vault where current interfaces allow.

### Session SHIP-04: API Audit WAL

- Implement persistent API audit writer.
- Add replay and verify on startup.
- Add tamper tests.

### Session SHIP-05: Compliance Audit Sealer

- Wire compliance audit WAL from production profile.
- Replace `NullSealer` in ship mode.
- Add sealer rejection check.

### Session SHIP-06: Trust Production Mode

- Add trust exchange mode.
- Reject local-dev synthetic exchange in ship mode.
- Wire ML-DSA verifier and trust anchors into ship startup.

### Session SHIP-07: Convergence + Readiness Endpoint and Validator CLI

In practice this session split into two slices once SHIP-02..SHIP-06
had landed. The convergence slice (the wiring step that retired the
demo defaults from the live startup path) was the highest-risk piece
and was executed first.

**Slice A — Bootstrap convergence + runtime guard wiring (done 2026-05-23, commit `48c7d2e`):**

- `MaiServer::with_ship_profile(path)` + `MAI_SHIP_PROFILE` env var.
- `MaiServer::run()` now branches on the resolved profile:
  - `vault_builder::build_vault(&profile)` replaces `StubVault`.
  - `WalAuditWriter::open(WalAuditConfig::for_dir(&profile.audit.wal_dir))`
    replaces `MemoryAuditWriter`.
  - `sealer_builder::build_sealer(&profile)` drives
    `ComplianceAuditLog::builder().sealer(...).build()` via
    `AppState::with_compliance_audit`.
  - `trust_builder::build_trust_components(&profile).bundle_verifier`
    replaces `AcceptAllBundleVerifier` via
    `AppState::with_bundle_verifier`.
  - In production mode (`require_bundle_on_boot=true`) the server
    calls `trust_builder::verify_boot_bundle` before the readiness
    gate; failure surfaces as `ServerError::Init`.
- New public types `RuntimeChecks` + `RuntimeOutcome` and
  `ProductionReadinessReport::evaluate_with_runtime` + `apply_runtime`
  flip the deferred runtime checks `PROD-VAULT-100`, `PROD-AUDIT-100`,
  `PROD-AUDIT-101`, `PROD-TRUST-100`, `PROD-AUTH-100`,
  `PROD-POLICY-001` from `Deferred` to `Pass` / `Fail` at startup.
- The server returns `ServerError::Init` (with the rendered report)
  on any Critical Fail and never reaches `bind()`.
- 4 new integration tests in `mai-api/tests/ship_convergence.rs`
  + 5 new unit tests in `production_guard.rs`. All 177 mai-api
  lib tests + ~108 integration tests pass; clippy + fmt clean.

**Slice B — Readiness endpoint + standalone CLI (pending):**

- Expose the runtime readiness report at
  `GET /v1/system/production-readiness` (admin-only). The
  serializer (`ProductionReadinessReport::to_json`) already exists;
  this is wiring an admin route, plus a small handler that
  re-runs `evaluate_with_runtime` over the introspection collected
  in `apply_ship_profile`.
- Add the standalone `mai-ship-validate` binary that accepts a
  `--profile <PATH>` (and optional `--state-dir <PATH>` so the
  runtime checks can be exercised offline) and emits human / JSON
  output with §13 exit codes.
- Profile-aware `handlers/trust.rs::exchange_token` switching on
  the `TrustExchangeMode` collected in `apply_ship_profile`:
  return 404/410 on `Disabled`, forward to OpenBao on
  `OpenBaoBridge`, mint synthetic only on `LocalDevSynthetic`.
  Currently the handler always mints synthetic regardless of
  profile.

### Session SHIP-08: Packaging

- Add systemd units.
- Add package staging scripts.
- Add package validation.

### Session SHIP-09: Backup Tooling

- Add backup create/verify.
- Add backup manifest.
- Add checksums/signature hooks.

### Session SHIP-10: Restore Tooling

- Add restore plan/apply.
- Add restore drill tests.
- Validate after restore.

**Status (2026-05-23): done.** `tools/mai-admin/src/restore.rs`
(`plan_restore` / `apply_restore`, two-phase: plan is read-only and
verifies signature + per-component sha3 + WAL chain on the backup
side; apply refuses populated targets without `--force`, recomputes
sha3 *after* each write, replays the WAL chain in the restored tree,
drops `source-manifest.json` + `restore-report.json` witnesses).
`mai-admin restore plan` / `mai-admin restore apply` CLI with §13
exit codes. New `RestoreError` enum (`ManifestMissing`,
`TargetNotEmpty`, `UnsignedManifest`, `SignatureFailed`,
`SourceDigestMismatch`, `TargetDigestMismatch`, `SourceMissing`,
`AuditChainBroken`, `AuditChainLastMismatch`, and IO/serde
passthroughs). Integration suite at
`tools/mai-admin/tests/restore_e2e.rs` (20 tests) covers the §9.5
DR drills end-to-end: WAL tamper, missing trust bundle, missing
model registry, signed-manifest tamper — every drill asserts the
target stays empty after a failed plan. Round-trip drills
(`restored_tree_passes_audit_chain_replay`,
`restored_tree_re_backs_up_to_byte_identical_state`) prove the
restored tree is byte-faithful and re-verifies clean. Gates:
`cargo test -p mai-admin` 64 pass (29 lib + 15 backup_e2e + 20
restore_e2e); `cargo clippy -p mai-admin --tests --bins
-- -D warnings -A clippy::pedantic` clean;
`cargo fmt -p mai-admin -- --check` clean. Landed in
commit `0fe5f59` on `origin/main`.

### Session SHIP-11: Observability

- Add metrics endpoint/export shape.
- Add health readiness semantics.
- Add alert rule config.

### Session SHIP-12: CI Enforcement

- Enforce mypy.
- Add package build job.
- Add ship validator job.
- Add nightly no-GPU burn-in.

### Session SHIP-13: GPU Release Workflow

- Add self-hosted GPU workflow.
- Add benchmark thresholds.
- Add artifacts.

**Status (2026-05-23): done.** `.github/workflows/gpu-release.yml`
(5 jobs on `[self-hosted, gpu, mai-release]`),
`config/gpu-release-thresholds.toml` (8 required benchmarks +
regression policy), `scripts/gpu-release-bundle.{sh,ps1}` (signed-shape
release manifest + tar.gz bundle), `tests/benchmarks/bench_compare.py`
extended with a `gate` subcommand (exit codes 0..5 per failure mode),
`tools/gpu_release_tests/` (84 pytest cases, 73 cross-platform +
11 POSIX-only). See `.github/workflows/gpu-release-README.md`.

### Session SHIP-14: 72-Hour Burn-In

- Extend burn-in scripts.
- Add signed burn-in report.
- Add smoke mode.

### Session SHIP-15: Production Docs and Runbooks

- Add all operator docs.
- Remove stale "production wires later" language where now implemented.

**Status (2026-05-23): done.** Added 11 thematic operator docs
under `mai/docs/` (`INSTALL.md`, `FIRST-BOOT.md`, `OPERATIONS.md`,
`BACKUP-RESTORE.md`, `OBSERVABILITY.md`, `RELEASE-GATES.md`,
`SECURITY-PRODUCTION.md`, `TRUST-BRIDGE-PRODUCTION.md`,
`AUDIT-RETENTION.md`, `UPGRADE-ROLLBACK.md`,
`INCIDENT-RESPONSE.md`). Added 14 named-failure runbooks under
`mai/docs/runbooks/` covering the §12 list (first boot, rotate
API key, rotate trust anchor, install policy bundle, verify
audit chain, generate compliance report, back up, restore,
recover from failed upgrade, adapter crash loop, trust bundle
expired, audit WAL tamper, air-gap violation, disk almost full),
plus `runbooks/README.md` as the index. Every alert in
`OBSERVABILITY.md` maps to exactly one runbook; every failing
`PROD-*` check in `RELEASE-GATES.md` maps to a runbook. Stale
"production wires later" language removed from
`mai/deployment/ship/README.md`, `mai/docs/SHIP-PROFILE.md`
status table, `mai/packaging/README.md` Future work section, and
`mai-api/src/audit_wal.rs:323-325`. `mai/docs/INDEX.md` updated
with a new "Operator Production Docs (SHIP-15)" section linking
all new docs and the runbook index. No source code changes
required; doc-only session. Gates: subagent integrity scan over
26 new files (zero null-byte delta, balanced fences, no
truncated tails), `git diff --stat` shows no >50% deletions on
edited existing files. Acceptance per §12: "A new operator can
install, validate, back up, restore, and monitor the node using
docs only" — INSTALL.md + the runbook index satisfy this end to
end.

### Session SHIP-16: Final Audit Pass

- Search for `StubVault`, `MemoryAuditWriter`, `AcceptAllBundleVerifier`, `NullSealer`, `dashboard-dev`, `local-dev token stub`, `placeholder`, `deferred`.
- Classify each remaining occurrence as test/dev/doc/future hardware.
- Update `KNOWN-ISSUES.md`.
- Run all no-GPU gates.

**Status (2026-05-23): done.** §15 grep sweep ran against the
production crate roots declared in `config/forbidden-terms.toml`; every
hit is classified in the new issue 14 of `docs/KNOWN-ISSUES.md`. The
sweep surfaced one previously-undocumented production-safety gap
(`load_auth_state` ignores `profile.auth.auth_keys_path` — see issue
13) and one historical workspace artefact (`mai-sdk-rs` HTTP client
methods are `todo!()` stubs — see issue 15); both are flagged for
follow-up but do not block the ship lane because the production guard
plus the operator docs (`docs/FIRST-BOOT.md`,
`docs/SECURITY-PRODUCTION.md`) keep the misconfiguration off the
default path and the Rust SDK has no in-tree consumer.

`config/forbidden-terms.toml` allowed_paths drain: every listed path
still legitimately contains its term (type definitions, builders, or
the `production_guard.rs` rejection wiring), so the allowlist length
does not shrink in this pass. The scanner reports `PASS (204 files,
6 terms, 0 disallowed hits)` against `main`.

SHIP-12 mypy adapter override shrink: `pyproject.toml` split the
single `adapters.*` block into a production-adapter override and a
tests-only override. Three error codes (`index`, `type-arg`,
`arg-type`) and one dead code (`unused-ignore`) were dropped from the
production block. Two now-unused `# type: ignore[union-attr]` comments
in `adapters/ollama/adapter.py` were removed alongside. `mypy --strict
mai-sdk-python/src/` and `mypy adapters/` both remain green; all 10
`tools/ship12_tests/test_ci_enforcement.py` regression tests pass.

No-GPU gates run: see commit body for the gate matrix and the exact
exit codes for `cargo check`, `cargo clippy`, `cargo fmt`,
`cargo test`, `ruff`, `mypy --strict mai-sdk-python/src/`, `mypy
adapters/`, and `python3 scripts/ci_forbidden_terms.py`.

### Session SHIP-17: Auth Bypass Consistency Guard

- Close `KNOWN-ISSUES.md` issue 13: `load_auth_state` must read
  `profile.auth.auth_keys_path` instead of the hard-coded
  `AUTH_KEYS_CONFIG_PATH` constant.
- Refuse the first-boot path under `ProfileMode::Production`; fail
  closed with `ServerError::Init` instead of silently flipping
  `allow_internal_profile_header = true` on a freshly-generated key.
- Add a deferred runtime check `PROD-AUTH-101` that cross-checks the
  runtime `ApiKeyStore.allow_internal_profile_header` against the
  profile field `PROD-AUTH-002` inspects, and flips Deferred → Fail
  when they diverge.
- Regression coverage: integration tests over the public guard API
  plus unit tests against `load_auth_state` directly.

**Status (2026-05-23): done.** Commit `6e027db` (+441/-36 across 6
files). `load_auth_state(profile: Option<&ShipProfile>) -> Result<AuthState, ServerError>`
in `mai-api/src/server.rs` now resolves the keys path from the parsed
profile; under `Production` a missing or unloadable file is fatal
(`ServerError::Init`) and the first-boot fallback is forbidden. Under
non-production modes first-boot still runs but
`store.allow_internal_profile_header` mirrors the profile field, so
the runtime state cannot diverge from what the static guard checked.
With no profile at all (legacy bring-up) the dev default of `true`
survives so existing tests + local-dev runs are unaffected.

New `RuntimeChecks::auth_internal_bypass_consistent` field plus
deferred check `PROD-AUTH-101` registered in
`production_guard::register_auth_checks` and wired through
`apply_runtime`. The new outcome is computed in
`MaiServer::apply_ship_profile` (live boot path) and in
`mai-api/src/bin/mai_ship_validate.rs` (offline validator) so both
agree about consistency.

Test footprint after SHIP-17: 194 mai-api lib + 136 mai-api
integration = 330 passing across 20 test binaries (+6 added by
SHIP-17, 0 regressions). New file
`mai-api/tests/auth_bypass_consistency.rs` (3 integration tests
covering deferred-without-runtime, pass-when-consistent, and the
fail-blocks-ship-ready Issue 13 scenario). Two new unit tests in
`server.rs` cover the production fail-closed path and the
non-production mirror-the-profile-field path. The existing
`test_load_auth_state_no_config` was updated to match the new
signature but keeps its legacy-bring-up assertions.

Gates run: `cargo check -p mai-api --tests` clean, `cargo fmt -p
mai-api --check` clean, `cargo test -p mai-api` 330 passing 0
failing, `.integrity/scripts/verify-tree.sh` PASS over all 6 touched
files, pre-commit integrity hook PASS. Docs follow-up (this entry,
`KNOWN-ISSUES.md` status flip, `SHIP-PROFILE.md` table row, session
log entry) lands as a separate commit per the code/docs split
direction.

---

## 15. Search Terms for Every Session

At the start and end of each hardening session, run targeted searches for:

```text
StubVault
MemoryAuditWriter
AcceptAllBundleVerifier
NullSealer
dashboard-dev
local-dev token stub
LocalDevSynthetic
allow_internal_profile_header = true
placeholder
stub
deferred
out of scope
production wires
operator's responsibility
TODO
FIXME
unimplemented!
todo!
```

Every hit must be categorized:

- acceptable in tests
- acceptable in local-dev docs
- acceptable for future hardware stubs
- unacceptable in production path
- stale documentation

Unacceptable production-path hits block release.

---

## 16. Definition of Done for the Whole Hardening Lane

The hardening lane is complete only when all commands below pass in the intended environments.

### Developer Machine

```bash
cargo check --workspace
cargo fmt --check
cargo clippy --workspace -- -D warnings -A clippy::pedantic
cargo test --workspace
python -m pytest adapters/ mai-sdk-python/ tools/ compliance-dashboard/ apps/
ruff check adapters/ mai-sdk-python/ compliance-dashboard/ apps/
mypy --strict adapters/ mai-sdk-python/src/
```

### Package Build

```bash
scripts/build-package.sh
mai-ship-validate --package-root target/package-staging --profile deployment/ship/profile.toml
```

### Installed Node

```bash
mai-ship-validate --profile /etc/mai/profile.toml
systemctl status mai-api
curl -f http://127.0.0.1:8420/v1/health/live
curl -f http://127.0.0.1:8420/v1/health/ready
```

### Recovery

```bash
mai-admin backup create --out /var/backups/mai/test
mai-admin backup verify /var/backups/mai/test
mai-admin restore plan /var/backups/mai/test --target /tmp/mai-restore
mai-admin restore apply /var/backups/mai/test --target /tmp/mai-restore
mai-ship-validate --state-dir /tmp/mai-restore --profile /tmp/mai-restore/etc/profile.toml
```

### Release Hardware

```bash
scripts/burn-in.sh --output results/release-smoke
scripts/burn-in-72h.sh --output results/release-72h
mai-ship-validate --profile /etc/mai/profile.toml
```

---

## 17. Final Product Standard

MAI is shippable when:

- The default production path is persistent.
- The default production path is cryptographically verified.
- The default production path is observable.
- The default production path is recoverable.
- The default production path fails closed.
- Every demo shortcut is impossible to activate accidentally in production.
- A skeptical customer can install it, break it, restart it, restore it, and verify the audit trail without trusting the vendor's word.

That is the difference between a strong architecture and a sea-worthy product.
