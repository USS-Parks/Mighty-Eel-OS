# MAI Integration Test Coverage Map (Session 34)

This document maps the 16 integration coverage areas required by
`BUILD-EXECUTION-PLAN.md` Session 34 to the test files that exercise them.
It is the Gate C evidence the integration suite is real, runs consistently,
and that gaps are documented honestly.

## Coverage Matrix

| Area | Test file(s) | Status |
|------|--------------|:------:|
| Inference path | `mai-api/tests/http_integration.rs::test_chat_completions_no_model`, `test_embeddings_endpoint_routes`; `mai-core/tests/integration_lifecycle.rs::test_full_request_lifecycle` | ✓ |
| Streaming | `mai-api/tests/streaming_integration.rs` (5 tests: SSE delivery, heartbeat, done, concurrency, non-streaming fallback) | ✓ |
| SDK (Python) | `mai-sdk-python` unit + adapter integration via `pytest tools/ adapters/` (114 tests) | ✓ |
| SDK (Rust) | `mai-sdk-rs` lib tests (8): config defaults, auth headers, retryable errors | ✓ |
| Auth | `mai-api/tests/auth_gate_a.rs` (6 acceptance tests: missing/invalid/valid/rate-limit/spoofing/exempt-health); `mai-api/tests/http_integration.rs::test_admin_endpoint_rejected_for_adult`; `mai-api/tests/grpc_integration.rs::test_grpc_auth_rejects_unprivileged` | ✓ |
| Vault | `mai-core/tests/integration_lifecycle.rs::test_full_request_lifecycle` exercises the `VaultInterface` boundary; vault crypto (Session 27) lands its own integration tests | ◐ |
| Air-gap | `mai-api/tests/system_integration.rs::session34_air_gap_*` (3 tests: consistent state, anomaly detection, fail-safe Unknown) | ✓ |
| Scheduler placement | `mai-scheduler/tests/gate_c_session33.rs` (8 Gate C acceptance tests); `mai-scheduler/tests/topology_integration.rs` | ✓ |
| KV cache pressure | `mai-scheduler` lib tests (KV eviction, guards, triggers, soft eviction round-trip) — unit-level integration; full-stack pressure exercised by `tools/simulator/replay_compare.py` | ✓ |
| Batching | `mai-scheduler` lib tests in `batch::` (admission, builder, preemption); end-to-end batching under load is part of Session 35 burn-in | ◐ |
| Power states | `mai-core/tests/integration_lifecycle.rs::test_power_state_full_cycle` (state machine); `mai-api/tests/system_integration.rs::session34_power_transition_via_api_walks_full_cycle` (HTTP API drives the cycle) | ✓ |
| Sentinel promotion | Covered by the same two tests above (the promote action drives Sentinel → FullInference); sentinel module unit tests in `mai-core/src/sentinel/` | ✓ |
| Model lifecycle | `mai-core/tests/integration_lifecycle.rs::test_hotswap_adapter_replacement`; `mai-core/src/models/install.rs::tests`; `mai-core/src/models/lifecycle.rs::tests` (load/benchmark/unload round-trip) | ✓ |
| OTA update | `mai-core/src/models/update.rs::tests` (4 tests: differential download, license validation, resumable, no-device-identity); `mai-api/src/handlers/updates.rs::tests` (background download progress) | ✓ |
| Metrics | `mai-api/tests/http_integration.rs` (telemetry endpoints exist and route); `mai-core/tests/integration_lifecycle.rs::test_scheduler_metrics_under_load`; `mai-scheduler/src/metrics/` lib tests | ✓ |
| Error handling | `mai-api/tests/http_integration.rs::test_error_format_spec` (MAI-XXXX code format); auth rejection tests across `auth_gate_a.rs` | ✓ |
| Shutdown behavior | `mai-core/tests/integration_lifecycle.rs::test_power_state_full_cycle` includes SystemShutdown trigger; `mai-api/tests/system_integration.rs::session34_power_transition_via_api_walks_full_cycle` drives shutdown via the HTTP API | ✓ |

Legend: ✓ covered · ◐ partial (unit-level + simulator, full-stack deferred to burn-in) · ✗ not covered

## Phase 1 Exit Criteria

The eight Phase 1 exit criteria from `MAI-BUILD-PROMPT-ROSTER-v2.md` Session 34
split into two groups by what they can be honestly verified on:

### Verifiable in CI / developer machines

| Criterion | Coverage |
|-----------|----------|
| `test_air_gap_complete` | `session34_air_gap_*` tests (3 cases against a mock SwitchReader) |
| `test_family_profiles_isolation` | `session34_family_profiles_isolation_matrix` |
| `test_zero_data_leak` | `session34_audit_schema_does_not_carry_inference_content` (structural assertion that `AuditEntry` exposes no field that could hold prompts or responses) |
| `test_onboarding_path` | Partial — model lifecycle tests cover the install→load→inference sequence; full <10-minute boot timing is a burn-in measurement |

### Deferred to Session 35 burn-in

| Criterion | Why deferred |
|-----------|--------------|
| `test_scout_config_boots` | Requires a real RTX 4090 / Ollama / Qwen3-14B stack and ≥60-second boot timing |
| `test_ranger_config_boots` | Requires dual-H100 / vLLM tensor parallel / Qwen3-70B |
| `test_two_gpu_configs` | Requires both NVIDIA and AMD hardware |
| `test_72_hour_stability` | Time-dependent; runs as part of Session 35 burn-in |

These deferrals are not gaps — they are tests that can only be honestly run
on the target hardware. Session 35 builds the burn-in scripts that exercise
them as part of deployment validation.

## Running the Integration Suite

```bash
# Full workspace test (covers all of the above except the SDK Python suite)
cargo test --workspace

# Python integration tests
python -m pytest tools/ adapters/

# Targeted Gate C / Session 34 surface
cargo test -p mai-scheduler --test gate_c_session33   # Session 33 Gate C
cargo test -p mai-api --test auth_gate_a              # Session 26 Gate A
cargo test -p mai-api --test system_integration       # Session 34
```

## Gate C Acceptance Map

The four BUILD-EXECUTION-PLAN.md Gate C criteria for the integration suite:

| Criterion | Evidence |
|-----------|----------|
| Integration suite runs consistently | All test files above pass on `cargo test --workspace`. Test counts last verified: 324 mai-scheduler lib, 121 mai-api lib, 186 mai-core lib, plus ~50 integration tests in `mai-*/tests/` (114 Python tests). |
| Failures are actionable | Tests assert on specific status codes, named fields, and use descriptive panic messages. MAI-XXXX error codes are spec-defined. |
| Major endpoints have test coverage | HTTP (8 routes tested), gRPC (4 tests), SSE streaming (5 tests), WebSocket (skeleton), power/profile/audit admin endpoints. |
| Critical paths under realistic conditions | Auth rate-limit burst (Gate A), trace-driven multi-policy simulation (Session 32), soft-eviction round-trip + preemption (Session 33), HTTP-level power state walk (Session 34). |
| Known failing tests documented | None currently. Hardware-dependent Phase 1 tests are documented as Session 35 burn-in scope, not "failing." |
