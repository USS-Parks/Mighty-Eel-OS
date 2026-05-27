# MAI Known Issues

**Project:** Island Mountain Model Abstraction Interface (MAI)
**Last Updated:** 2026-05-26 (IGD-10 added Issue #16: triaged 5 source-level Follow-up: markers as accept-and-tracked)

---

## Active Issues

### 1. Rust Toolchain Availability

**Severity:** Low (development workflow only)
**Affects:** Sessions 11a-11e, all future sessions
**Status:** Resolved in current workspace

This workspace currently has Cargo available (`cargo 1.95.0`). Earlier sessions ran in a sandbox without a Rust toolchain, so older handoff notes may say `cargo check`, `cargo clippy`, and `cargo fmt` could not run in-session. That limitation no longer applies here.

**Action:** Run the standard Rust gates in-session when practical: `cargo check --workspace`, `cargo clippy --workspace -- -D warnings -A clippy::pedantic`, and `cargo fmt --check`.

### 2. cargo fmt Drift

**Severity:** Low (cosmetic)
**Affects:** All Rust files
**Status:** Resolved 2026-05-21, monitor for new drift

Formatting drift accumulated across earlier sessions, then was resolved by the 2026-05-21 CI fix pass across 14 Rust files.

**Action:** Keep running `cargo fmt --check` before marking code sessions complete. If generated protobuf code causes conflicts, add `#[rustfmt::skip]` or exclude in `rustfmt.toml`. See docs/BUILD.md for details.

### 7. Axum 0.7 vs 0.8 Handler Trait Version Conflict

**Severity:** Medium (affects new 2+-extractor handlers that use body extractors)
**Affects:** `mai-api/src/handlers/models.rs` (install_model), `mai-api/src/routes.rs`
**Status:** Workaround in place (Session 24, 2026-05-21)

`tonic 0.12.3` transitively depends on `axum 0.7.9` while `mai-api` directly depends on `axum 0.8.9`. Both export a `Handler` trait. The compiler cannot resolve which `Handler` impl to use for async functions with 2+ extractors when `T3` is a `FromRequest` (body) type that exists in both versions (e.g. `Json<T>`, `Bytes`). Custom `FromRequest` types in `mai-api` also fail because the function type matches the generic pattern of both crate versions before where-clause checking.

**Workaround:** Register body-consuming routes via `post_service(service_fn(...))` (Tower `Service`) instead of `post(handler)` (axum `Handler`). See `routes.rs` and `install_handler_raw` in `handlers/models.rs`.

### 8. Phase 1 Hardware-Dependent Exit Criteria Deferred to Burn-In

**Severity:** Expected (hardware-only verification)
**Affects:** Gate C / Session 34 acceptance
**Status:** Documented 2026-05-22 (Session 35)

Four Phase 1 exit criteria from `MAI-BUILD-PROMPT-ROSTER-v2.md` require target hardware and cannot be verified in CI:

- `test_scout_config_boots` — needs 1x RTX 4090 + Ollama + Qwen3-14B and a <60s timing measurement.
- `test_ranger_config_boots` — needs 2x H100 + vLLM tensor parallel + Qwen3-70B and a <90s timing measurement.
- `test_two_gpu_configs` — needs both NVIDIA and AMD hardware.
- `test_72_hour_stability` — time-dependent.

**Action:** Run these as part of deployment validation on the target hardware. `scripts/burn-in.sh` emits a `phase1-deferred.txt` artifact per run that names them explicitly so the deferral is never silent. See `docs/INTEGRATION-COVERAGE.md` for the full coverage map.

### 9. Live OpenBao Bring-Up Deferred (per Plan Appendix A)

**Severity:** Low (BF-6 contract shipped; production swap is handler-body-only)
**Affects:** V2 sessions 26c (OpenBao HA dev deployment) and 27c (PKI / mTLS) — neither was executed live; both were absorbed into the BF-1..BF-7 contract lane per plan Appendix A §A.2.
**Status:** Documented deferral; not blocking S45 / S46 / Gate D.

The BF-1..BF-6 lane ships the Trust Manifold contract, schemas, ML-DSA-87 bundle verifier, local trust cache, BF-5 audit correlation, the four deployment profiles, and a local-dev token stub at `POST /v1/auth/exchange_token`. An acquirer wires their existing OpenBao deployment in by swapping the handler body for that endpoint — wire shape, claim schema, and bundle verification do not change. Live OpenBao deployment, OpenBao HA, and live PKI / mTLS bring-up belong to the acquirer's integration sequence (`docs/BUYER-INTEGRATION-GUIDE.md`), not to this build.

### 10. Hard-Coded Local-Dev Admin Token in Dashboard

**Severity:** Low (development-only default; not a production gate)
**Affects:** `compliance-dashboard/util.py`
**Status:** By design, documented at the BF-6 gate. The production_guard's `PROD-DASH-001` check rejects the dashboard-dev path at ship-profile parse time, so SHIP-07 convergence already blocks this default from reaching production startup.

The compliance dashboard admin gate uses `X-IM-Auth-Token: $MAI_DASHBOARD_ADMIN_TOKEN` (default `dashboard-dev`). The default value is for local development only — every shipped deployment profile recommends setting `MAI_DASHBOARD_ADMIN_TOKEN` to a real value before exposing the dashboard. Acquirer-side integration guides (`docs/BUYER-INTEGRATION-GUIDE.md` Step 6) call this out.

### 11. SHIP-07 remainder slice (admin endpoint + standalone CLI, closed)

**Severity:** Low (functional gate is already live inside `MaiServer::run()`; this is the network/binary exposure)
**Affects:** Operator tooling and packaging (SHIP-08).
**Status:** **Closed 2026-05-23 in SHIP-07-endpoint-and-cli** (commit `1f40413`; later docs may cite aggregate commit `7b746c0`).

SHIP-07 convergence (commit `48c7d2e`) wired all four builders into `MaiServer::run()` and the production_guard's six deferred runtime checks (`PROD-VAULT-100`, `PROD-AUDIT-100`, `PROD-AUDIT-101`, `PROD-TRUST-100`, `PROD-AUTH-100`, `PROD-POLICY-001`) now flip from Deferred to Pass / Fail via `ProductionReadinessReport::evaluate_with_runtime`. SHIP-07-endpoint-and-cli then landed the `GET /v1/system/production-readiness` admin route, standalone `mai-ship-validate` binary, and profile-aware `handlers/trust.rs::exchange_token` switch on `TrustExchangeMode`.

### 12. Duplicate `GENESIS_HASH` constant in `audit_wal.rs`

**Severity:** Trivial (guarded by inline `genesis_hash_matches_audit_module` test against drift)
**Affects:** `mai-api/src/audit_wal.rs:51`
**Status:** Carried forward from SHIP-07 convergence checklist.

`audit_wal.rs` duplicates `audit.rs::GENESIS_HASH` because SHIP-04 was committed in parallel with SHIP-05's `pub(crate)` promotion. The inline test will fail closed if the canonical value ever drifts. Safe to remove in a follow-up cleanup commit.

### 13. `load_auth_state` ignored ship-profile `auth.auth_keys_path` (SHIP-16 finding, closed in SHIP-17)

**Severity:** High (silent first-boot fallback in misconfigured production)
**Affects:** `mai-api/src/server.rs` (`load_auth_state`, `apply_ship_profile`), `mai-api/src/production_guard.rs` (`RuntimeChecks`, `PROD-AUTH-101`).
**Status:** **Closed 2026-05-23 in SHIP-17 (commit `6e027db`).** Originally surfaced by the SHIP-16 §15 grep sweep and deferred to a follow-up.

`load_auth_state()` hard-coded `config/auth_keys.toml` (relative to CWD) and never read `profile.auth.auth_keys_path` from the parsed ship profile. If a production ship profile pointed `auth.auth_keys_path` at a path that was not `config/auth_keys.toml` (e.g. the documented `/etc/mai/auth_keys.toml`), and the legacy CWD-relative file was absent, `load_auth_state` fell through to the first-boot path. That path printed a fresh admin key to stdout and set `store.allow_internal_profile_header = true` on the in-memory `ApiKeyStore`. The production guard's `PROD-AUTH-002` rejected `profile.auth.allow_internal_profile_header = true`, but it did NOT cross-check the runtime store. The middleware at `mai-api/src/auth.rs:478` honors the runtime store flag, so the internal-profile bypass would have become live for as long as the server stayed up. `PROD-AUTH-100` (runtime `auth_keys_nonempty`) still reported Pass because the freshly generated first-boot key kept `auth_key_count >= 1`.

**Mitigations that were in place before the fix:**
- Operators following `docs/FIRST-BOOT.md` and `docs/SECURITY-PRODUCTION.md` install `/etc/mai/auth_keys.toml` before first boot, so the first-boot fallback was not triggered on the default path.
- `PROD-AUTH-001` rejects an empty `auth.auth_keys_path`, so the misconfiguration only fired when the operator set a path that did not match `config/auth_keys.toml`.

**Fix landed in SHIP-17 (commit `6e027db`):**
1. `load_auth_state(profile: Option<&ShipProfile>) -> Result<AuthState, ServerError>` now reads `profile.auth.auth_keys_path` instead of the hard-coded constant. The no-profile bring-up path still uses `AUTH_KEYS_CONFIG_PATH` for back-compat with tests + local-dev.
2. Under `ProfileMode::Production`, a missing or unloadable keys file is fatal: `ServerError::Init` short-circuits before any socket binds. The first-boot path is forbidden in production. Under non-production modes, first-boot still runs but the runtime store's `allow_internal_profile_header` mirrors `profile.auth.allow_internal_profile_header` so it can never silently diverge from the value `PROD-AUTH-002` inspected.
3. New deferred runtime check `PROD-AUTH-101` cross-checks the runtime `ApiKeyStore` flag against the profile field. Wired through `RuntimeChecks::auth_internal_bypass_consistent`; computed in `MaiServer::apply_ship_profile` and in the `mai-ship-validate` binary so both the live boot path and the offline validator agree.
4. Regression coverage: `mai-api/tests/auth_bypass_consistency.rs` (3 integration tests over the public guard API) + two new unit tests in `server.rs` (production fail-closed, non-production mirror-the-profile-field). Test footprint after SHIP-17: 194 mai-api lib + 136 integration = 330 passing, +6 added by SHIP-17, 0 regressions.

### 14. SHIP-16 final audit pass — §15 classification (informational)

**Severity:** Informational (no change in shippability; documents the residue the audit pass left in place)
**Affects:** Documentation only.
**Status:** Closed 2026-05-23 (this is the SHIP-16 deliverable).

The SHIP-16 §15 grep sweep ran the term list from `docs/SHIP-HARDENING-PLAN.md` §15 against the production crate roots. Every remaining occurrence has been classified.

| Term | Live occurrences in production crate src/ | Classification |
|---|---|---|
| `StubVault` | 4 files (server.rs, lib.rs, production_guard.rs, vault_builder.rs) | dev/test fixture + rejection wiring; allowlisted in `config/forbidden-terms.toml`. Reachable only on the no-profile bring-up path (tests + local-dev). `PROD-VAULT-001/002/100` reject in production. |
| `MemoryAuditWriter` | 5 files (audit.rs, audit_wal.rs, lib.rs, production_guard.rs, server.rs) | dev/test fixture + rejection wiring; allowlisted. `PROD-AUDIT-001/002/100` reject in production. |
| `AcceptAllBundleVerifier` | 6 files (state.rs, trust_builder.rs, production_guard.rs, mai-compliance bundle.rs, trust_cache.rs, lib.rs) | bring-up default + rejection wiring; allowlisted. `PROD-TRUST-001/002/100` reject in production. |
| `NullSealer` | 8 files (mai-api lib.rs/production_guard.rs/sealer_builder.rs, mai-compliance audit/api.rs/mod.rs/sealer.rs/store.rs/lib.rs) | bring-up default + rejection wiring; allowlisted. `PROD-AUDIT-005/101` reject in production. |
| `dashboard-dev` | 1 file (production_guard.rs error string) | rejection wiring only. Dashboard default value lives in `compliance-dashboard/util.py`; covered by issue 10 above. |
| `LocalDevSynthetic` | 4 files (trust_builder.rs, state.rs, production_guard.rs, handlers/trust.rs) | enum variant + rejection wiring; `PROD-TRUST-003` rejects `trust.allow_local_dev_exchange = true` in production. |
| `allow_internal_profile_header = true` | 2 files (auth.rs:357 — `AuthState::local_trust` helper; server.rs:679 — first-boot fallback) | auth.rs:357 is a dev-only helper. server.rs:679 is the first-boot fallback flagged in issue 13 above. |
| `placeholder` | 9 files | Mix: (a) legitimate domain term in `mai-compliance/src/deid.rs` (the redaction string itself); (b) deferred-runtime-check wording in `production_guard.rs`; (c) `mai-core/src/sentinel/promotion.rs` user-visible "Processing your request..." string; (d) real stubs in `mai-api/src/grpc/registry.rs::scan_models`, `mai-api/src/grpc/inference.rs::Stream*`, `mai-api/src/streaming/ws.rs` (carried by issue 6 / Session 11e); (e) profile-metadata stubs in `mai-api/src/handlers/models.rs:129-131`. None block ship; gRPC + streaming placeholders carry forward per the comments in source. |
| `out of scope` | 10 files | Doc-comment scope statements ("Out of scope (SHIP-04):"); no source-level stubs. |
| `production wires` | 4 files (state.rs:64, state.rs:68, mai-compliance audit/store.rs:90, mai-compliance audit/api.rs:62) | Doc-comment contract statements ("default is X; production wires Y"). Not stale — they describe the rejection wiring that lives in `production_guard.rs`. |
| `operator's responsibility` | 1 file (tools/mai-admin/src/restore.rs:44) | Doc comment describing operator-owned packaging concern. |
| `TODO` | 3 files (`mai-adapters/src/manager.rs:586`, `mai-core/src/models/usb.rs:161`, `mai-scheduler/src/default.rs:394/399/402`) | Three carry session-pinned follow-ups (Session 19/22 metrics integration); one (`manager.rs`) is a known scheduler integration gap. None block ship. |
| `FIXME`, `unimplemented!` | 0 in src | clean. |
| `todo!()` (Rust macro that panics) | 0 calls | Cleared. The 17 calls previously in `mai-sdk-rs/src/lib.rs` were closed by `b281b55` (HTTP client, 14 sites) and `8d412c6` / J-17 (SSE streaming + resume, 3 sites). The crate is now a usable client; see Issue 15 below for the closing record. Keep `rg -n "todo!" mai-sdk-rs/src/lib.rs` in the J-lane verification checklist so the regression stays visible. |
| `deferred` | 12 files | Dominated by `production_guard.rs` (46 occurrences of the `CheckStatus::Deferred` enum variant — legitimate spec terminology); the rest are doc-comment scope statements. |

### 15. `mai-sdk-rs` HTTP client methods are `todo!()` stubs — **CLOSED**

**Severity:** Low (no in-tree consumer; Python SDK is the supported client; `mai-sdk-rs` is a workspace member but not a shipped runtime dependency)
**Affects (historical):** `mai-sdk-rs/src/lib.rs:768-887` — 17 `todo!("Session 11: …")` calls across the chat, completion, embedding, model, health, power, profile, audit, and SSE-stream surfaces.
**Status:** **CLOSED** 2026-05-24 by the DOUGHERTY lane. Closing record:

| Layer | Commit | What landed |
|:--|:--|:--|
| Plain-HTTP client + 14 method bodies | `b281b55` | `reqwest::Client` on `MaiClient`, `get_json` / `post_json` / `request_builder` helpers, error mapping via `api_error_from_body`, all of chat / complete / embed / structured / function_call / list_models / get_model / health / adapter_health / hardware_health / power_state / transition_power / get_profile / audit_log. |
| Plain-HTTP test coverage | `88fa06e` (J-16b) | `mai-sdk-rs/tests/http_client.rs`: 18 wiremock tests (14 happy-path + 4 edge cases: 401, 500, timeout, malformed JSON). |
| SSE streaming + resume primitive | J-17 (this lane) | `chat_stream` (buffered SSE parse via `ChatStreamHandle::from_sse_body`); new `chat_stream_resume(req, last_event_id)` that sets the `Last-Event-ID` header per the SSE spec. `mai-sdk-rs/tests/streaming.rs`: 7 wiremock tests covering order, last-event-id capture, DONE-only body, non-2xx mapping, malformed SSE, resume header on the wire, and resume non-2xx propagation. |

**Verification:** `grep -c 'todo!' mai-sdk-rs/src/lib.rs` returns 0; `cargo test -p mai-sdk-rs` runs the full integration surface (lib unit tests + 18 http_client + 7 streaming) green.

**What is still NOT here:** the buffered `ChatStreamHandle` waits for the full SSE response before yielding the first chunk. A true incremental stream (yield-as-they-arrive) would require a different design (`tokio_stream::Stream` over `Response::bytes_stream`) — that is out of scope for this closure and not blocking RC-10. Callers wanting incremental delivery should use the Python SDK or wrap the response body manually.

### 16. Source-level `Follow-up:` markers — triaged (IGD-10)

**Severity:** Low (metric reporting gaps; none block ship or compliance)
**Affects:** `mai-adapters/src/manager.rs:592`, `mai-core/src/models/usb.rs:161`, `mai-scheduler/src/default.rs:{394,399,402}`
**Status:** Accept + tracked here per the IGD-10 triage on 2026-05-26.

The 2026-05-26 internal GitDoctor scan (`docs/INTERNAL-GITDOCTOR-SCAN-2026-05-26.md`, M-2 finding) flagged 5 in-source `TODO` comments at the locations above. Between the scan and IGD-10 a parallel session converted the bare `// TODO` comments to `// Follow-up:` markers — the verification gate of `grep '\bTODO\b' **/src/*.rs` returns zero hits on the current HEAD. The underlying deferred work, however, was not implemented; this table tracks each item explicitly so the triage decision is recorded.

| # | File | Deferred work | Why deferred | Target session |
|---|---|---|---|---|
| 1 | `mai-adapters/src/manager.rs:592` | `adapter_in_flight()` returns 0; should track per-adapter in-flight request count | Scheduler uses its own internal tracking; the manager-side counter is for least-loaded routing visibility, not correctness | Post-RC2 scheduler-metrics session |
| 2 | `mai-core/src/models/usb.rs:161` | `fs_available_bytes()` returns 0; should call `GetDiskFreeSpaceExW` (Windows) / `statvfs` (POSIX) | Cross-platform API surface deserves its own session; downstream code already tolerates `0` defensively | Post-RC2 platform-support session |
| 3 | `mai-scheduler/src/default.rs:394` | `healthy_instances` always set to `total_instances`; should integrate adapter health probe | Pinned to Session 22 in-source; S22 closed without the integration; non-blocking for routing decisions | Post-RC2 scheduler-metrics session |
| 4 | `mai-scheduler/src/default.rs:399` | `avg_routing_latency_us` always 0; should track real routing latency | Pinned to Session 19 in-source; S19 closed without the integration; non-blocking | Post-RC2 scheduler-metrics session |
| 5 | `mai-scheduler/src/default.rs:402` | `topology_has_anomalies` always false; should wire to `MetricsRefresher` | Pinned to Session 19 in-source; S19 closed without the integration; non-blocking | Post-RC2 scheduler-metrics session |

**Owner:** Basho Parks (re-assign on session pickup).
**Deadline:** TBD (driven by post-RC2 sequencing in `docs/COGENT-DEPLOYMENT-ROADMAP.md`).
**Verification gate satisfied:** `grep '\bTODO\b' **/src/*.rs` returns zero hits on `main` HEAD.

Note: Issue #14's `TODO` row above is the 2026-05-23 SHIP-16 audit snapshot and uses the original `manager.rs:586` line number; commits since (notably `6d8bf8e` FileDev landing) shifted it to line 592. The numbers in this Issue #16 are current as of 2026-05-26.

---

## Deferred Items (Out of Scope)

These items are explicitly excluded from the current build. See PROJECT.md for scope boundaries.

- L6 UI (React dashboard, onboarding wizard)
- Remote support tunnel (network service, not MAI)
- Landfall Council (multi-user chat variant)
- Full L4 agent logic (RAG pipeline internals, tool implementations)
- Full L5 application logic (only scaffolds built in Session 16)
- TetraMem adapter implementation (stub interface only via HIL)
- Photonic adapter implementation (stub interface only via HIL)
- Audio/STT binary frame processing (acknowledged in WebSocket, deferred to Session 13)
- Tool calling execution (acknowledged in WebSocket, deferred to Session 13)

---

## Resolved Issues (Historical)

### BF-7 S30 Scaffold Absorption (RESOLVED)

**Resolved:** 2026-05-22 (BF-7, commit `e2d3791`)

BF-6 changed the SDK signature for `client.auth.exchange_token` from `(claim)` to `(subject_id, *, tenant_id, scopes)` and reshaped `TrustBundleStatus` (`bundle_version | None`, `last_refresh_secs`, `age_secs`, `connectivity`, `is_emergency_only`). The S30 scaffolds `apps/openbao-trust-demo/` and `apps/operator/` had been written against the old signature and silently regressed (6 + 2 = 8 failing tests). BF-7 absorbed the BF-6 wiring: updated both scaffolds, replaced `TrustNotProvisionedError` fallbacks with `MaiError` server-unreachable fallbacks, switched the operator trust panel to `client.trust.status()` for the consolidated dashboard view. Scaffold total 58 → 61 green.

### Issue #10 (BF-3 prerequisite) — RESOLVED 2026-05-22

BF-3 landed signed claim and policy bundle verification before Session 41 closed. The verification matrix now covers signed bundle success, invalid signature rejection, expired bundle behavior, tampered payload/metadata rejection, unknown anchor handling, tenant mismatch preservation, and HMAC subject hashing without raw subject leakage. Commit `9cbad83`.

### Session 03 Audit: FFI Blocking Issues (RESOLVED)

Three blocking FFI issues in the Backend Adapter Framework spec. All fixed during Session 03 audit. See SESSION-LOG-ARCHIVE-01.md for details.

### Session 10 CI: pytest Collection Failures (RESOLVED)

Missing `adapters/__init__.py` and AdapterBase constructor signature mismatch. Fixed 2026-05-17. See SESSION-LOG-ARCHIVE-02.md maintenance log.

### Issue #6: Registry scan_models Placeholder (RESOLVED in Session 15)

`ModelRegistry` had no `scan_models()` method and the gRPC `ScanModels` RPC returned an empty list. Session 15 added the real model scanning and discovery pipeline.

### Session 11d: Invented mai-core APIs (RESOLVED)

All 6 gRPC service files initially coded against non-existent APIs. All rewritten from scratch against verified interfaces during audit. See SESSION-LOG.md Session 11d notes.

### Session 11e: Proto Message Type Mismatches (RESOLVED)

Integration tests used `LoadModelRequest` (doesn't exist), empty `ListModelsRequest` (has profile_id field), ChatMessage with `tool_calls`/`tool_call_id` (proto only has role/content/name). All fixed during Audit Pass 1.

### Issue #3: Sglang Adapter self._raw_config (RESOLVED)

**Resolved:** 2026-05-19 (Adapter Contract Alignment maintenance session)

The Sglang adapter referenced `self._raw_config` in its `initialize()` method, but `AdapterBase` stores config as `self._config`. Fixed by changing to `self._config`. Confirmed via grep: no remaining references to `_raw_config` in the codebase.

### Issue #4: StubVault in Server Bootstrap (RESOLVED)

**Resolved:** Session 12, 2026-05-18; production wiring closed by SHIP-07 convergence on 2026-05-23 (commit `48c7d2e`).

The server used a `StubVault` placeholder. Real `ZfsVault` was added to the mai-vault crate in Session 12; SHIP-03 added the `build_vault` selector that rejects `StubVault` in production; SHIP-07 convergence made `MaiServer::run()` actually call the builder when `MAI_SHIP_PROFILE` is set. `StubVault` is now reachable only from the no-profile bring-up path (tests + local-dev without a ship profile) and is unconditionally rejected by the production guard (`PROD-VAULT-001`, `PROD-VAULT-002`, `PROD-VAULT-100`).

### SHIP-01..SHIP-07-convergence (RESOLVED)

**Resolved:** 2026-05-23.

The hardening lane's demo-default removal: `StubVault`, `MemoryAuditWriter`, `NullSealer`, and `AcceptAllBundleVerifier` are no longer reachable in production startup when `MAI_SHIP_PROFILE` is set or `MaiServer::with_ship_profile(path)` is called. `MaiServer::run()` constructs the real vault / WAL audit / sealer-backed compliance log / ML-DSA bundle verifier via the SHIP-03/04/05/06 builders, calls `verify_boot_bundle` (production-only), and refuses to bind sockets if `ProductionReadinessReport::evaluate_with_runtime` reports any Critical Fail. See `docs/SHIP-PROFILE.md` "What changes after SHIP-01" for the per-session enforcement table, and `docs/SHIP-HARDENING-PLAN.md` §SHIP-07 for the full convergence checklist with carried-forward items.

### Issue #5: Placeholder Token Producers in Streaming (RESOLVED)

**Resolved:** Session 14b, 2026-05-20

Streaming handlers previously used simulated token producers. Session 14b wired the real inference path end-to-end through AdapterManager, connecting adapter IPC output to the SSE streaming channel.

---

*Document derived from MAI-BUILD-PROMPT-ROSTER.md | 2026-05-15 | Island Mountain AI | Confidential*
