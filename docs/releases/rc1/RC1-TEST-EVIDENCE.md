# RC1 Test Evidence

> **STATUS — CLOSED (2026-05-23)**
> Historical record of the RC-05 evidence pass against freeze `dceaabc`. Grand total: 1717 pass / 0 fail / 2 ignored. RC1 bundle shipped to outside tester (RC-09 closed `e2d9ea6`). Any re-ship under DOUGHERTY → RC-11 will produce a new RC2 evidence doc; this file is preserved as the literal record of what ran on the RC-05 host. Do not edit retroactively.

**Project:** Lamprey MAI
**Release:** RC1 (Tester Bundle)
**Date of run:** 2026-05-23
**Freeze commit:** `dceaabc` (SHIP-17 hotfix)
**HEAD at run time:** `fa0df32` (RC-04 docs commit; 4 commits ahead of
the freeze — RC-01..RC-04 are docs-only; same test surface as
building at `dceaabc`)
**Plan reference:** `docs/COGENT-DEPLOYMENT-ROADMAP.md` Session RC-05
**Artifacts directory:** `test-evidence/rc-05/`

This document is a literal record of what was actually executed on
this host. It is not a narrative summary of historical test
footprints, and the numbers below are not pasted from prior session
notes — they were captured from the runs whose logs live in
`test-evidence/rc-05/`. Where prior docs (READY.md, SHIP-HARDENING-PLAN.md)
cite different headroom numbers, those came from different runs at
different times; both can be correct.

The "What was not run on this host" section (§7) is as important as
the pass counts. Read it.

---

## 1. Machine Fingerprint

| Field | Value |
|---|---|
| Host kernel | `MINGW64_NT-10.0-26200 Fire-Starter 3.6.7-fb42d713.x86_64 2026-03-29 11:44 UTC` |
| OS | Windows 11 Home, build 26200 |
| Shell | MSYS / MINGW64 (Git Bash) |
| Architecture | x86_64 |
| Rust toolchain | `rustc 1.95.0 (59807616e 2026-04-14)` |
| Cargo | `cargo 1.95.0 (f2d3ce0bd 2026-03-21)` |
| LLVM (rustc) | 22.1.2 |
| Rust target triple | `x86_64-pc-windows-msvc` |
| Python | 3.14.4 |
| pytest | 9.0.3 |
| GPU | none present on host; `nvidia-smi` unavailable. mai-api falls back to flat topology (single line WARN in startup logs). |
| Free disk (workspace volume) | 652 GB at run start |
| Working tree at run start | clean (only untracked: prior RC outputs in `docs/`); confirmed by `git status --short` immediately before each command |
| HEAD | `fa0df3254dc3d7e6b2b9e597fdcb70d72e6fccc4` |

## 2. Commands Executed (Index)

| # | Command | Purpose | Log |
|---|---|---|---|
| 2.1 | `cargo test --workspace --no-fail-fast` | Full Rust test surface (lib + integration + doc) | `cargo-test-workspace.log` |
| 2.2 | `cargo test --release -p mai-compliance --test compliance_perf -- --nocapture` | Release-mode perf budgets with measured numbers | `compliance-perf-release.log` |
| 2.3 | `python -m pytest mai-sdk-python/tests/ -v` (with `PYTHONPATH=mai-sdk-python/src`) | Python SDK | `python-sdk-tests.log` |
| 2.4 | `python -m pytest compliance-dashboard/tests/ -v` | Python dashboard | `python-dashboard-tests.log` |
| 2.5 | `python -m pytest apps/<name>/tests/` for each of six apps | Application scaffold smoke tests | `python-scaffold-tests.log` |

Each command's log file lives under `test-evidence/rc-05/`. The per-command
start/end timestamps and exit codes live in
`{command}-start.txt`, `{command}-end.txt`, `{command}-exit.txt` next to
the log where applicable.

## 3. Results

### 3.1 `cargo test --workspace --no-fail-fast`

| Field | Value |
|---|---|
| Started | 2026-05-23 21:09:24 PDT |
| Finished | 2026-05-23 21:14:54 PDT |
| Wall clock | 5 m 30 s |
| Exit code | 0 |
| Test binaries reported | 44 with non-zero results (49 `Running` + 11 `Doc-tests` lines; binaries with 0 tests omitted from the totals table) |
| Passed | **1 539** |
| Failed | **0** |
| Ignored | **2** (both doc tests under `mai-scheduler`, see §8) |

Per-binary breakdown (`cargo-test-workspace-summary.txt` is the
machine-readable version of the same data):

| Binary | Pass | Fail | Ign |
|---|---:|---:|---:|
| mai_adapters (unit) | 28 | 0 | 0 |
| mai_adapters `tests/e2e_integration.rs` | 14 | 0 | 0 |
| mai_admin (unit) | 29 | 0 | 0 |
| mai_admin `tests/backup_e2e.rs` | 15 | 0 | 0 |
| mai_admin `tests/restore_e2e.rs` | 20 | 0 | 0 |
| mai_agent (unit) | 65 | 0 | 0 |
| mai_agent `tests/rag_pipeline_test.rs` | 4 | 0 | 0 |
| mai_agent `tests/task_lifecycle_test.rs` | 7 | 0 | 0 |
| mai_agent `tests/tool_calling_test.rs` | 5 | 0 | 0 |
| mai_api (unit, lib) | 194 | 0 | 0 |
| mai_api (unit, main) | 1 | 0 | 0 |
| mai_ship_validate (unit) | 5 | 0 | 0 |
| mai_api `tests/audit_wal.rs` | 7 | 0 | 0 |
| mai_api `tests/auth_bypass_consistency.rs` | 3 | 0 | 0 |
| mai_api `tests/auth_gate_a.rs` | 6 | 0 | 0 |
| mai_api `tests/compliance_integration.rs` | 17 | 0 | 0 |
| mai_api `tests/grpc_integration.rs` | 4 | 0 | 0 |
| mai_api `tests/http_integration.rs` | 7 | 0 | 0 |
| mai_api `tests/production_guard.rs` | 6 | 0 | 0 |
| mai_api `tests/sealer_bootstrap.rs` | 7 | 0 | 0 |
| mai_api `tests/ship_07b_endpoints.rs` | 7 | 0 | 0 |
| mai_api `tests/ship_11_observability.rs` | 13 | 0 | 0 |
| mai_api `tests/ship_convergence.rs` | 4 | 0 | 0 |
| mai_api `tests/ship_profile.rs` | 3 | 0 | 0 |
| mai_api `tests/streaming_integration.rs` | 5 | 0 | 0 |
| mai_api `tests/system_integration.rs` | 7 | 0 | 0 |
| mai_api `tests/trust_production.rs` | 25 | 0 | 0 |
| mai_api `tests/vault_bootstrap.rs` | 9 | 0 | 0 |
| mai_compliance (unit) | 331 | 0 | 0 |
| mai_compliance `tests/compliance_demos.rs` | 6 | 0 | 0 |
| mai_compliance `tests/compliance_perf.rs` (debug) | 3 | 0 | 0 |
| mai_compliance `tests/phi_perf.rs` | 1 | 0 | 0 |
| mai_core (unit) | 192 | 0 | 0 |
| mai_core `tests/integration_lifecycle.rs` | 4 | 0 | 0 |
| mai_hil (unit) | 7 | 0 | 0 |
| mai_router (unit) | 62 | 0 | 0 |
| mai_router `tests/baseline_policy_load.rs` | 4 | 0 | 0 |
| mai_router `tests/latency_budget.rs` | 1 | 0 | 0 |
| mai_scheduler (unit) | 324 | 0 | 0 |
| mai_scheduler `tests/gate_c_session33.rs` | 8 | 0 | 0 |
| mai_scheduler `tests/topology_integration.rs` | 16 | 0 | 0 |
| mai_scheduler (doc tests) | 0 | 0 | 2 |
| mai_sdk_rs (unit) | 8 | 0 | 0 |
| mai_vault (unit) | 55 | 0 | 0 |
| **Total** | **1 539** | **0** | **2** |

### 3.2 `cargo test --release -p mai-compliance --test compliance_perf -- --nocapture`

| Field | Value |
|---|---|
| Started | 2026-05-23 21:22 (release compile + run) |
| Finished | 2026-05-23 21:23 PDT |
| Wall clock | ~1 m 12 s (1 m 06 s release compile + 0.06 s test) |
| Exit code | 0 |
| Passed | 3 | Failed | 0 | Ignored | 0 |

Measured (this run, this host):

| Budget test | Budget | Measured |
|---|---|---|
| `composer_p99_under_5ms` | < 5 ms | **600 ns** (P50 400 ns / P95 500 ns / P99 600 ns over 5 000 samples) |
| `audit_append_throughput_over_1000_per_sec` | > 1 000/s | **127 929/s** (2 000 entries in 15.63 ms) |
| `report_generation_under_10_seconds` | < 10 s | **1.588 ms** (200 seed entries, 30-day window, JSON format) |

Comparison to `docs/acquisition/READY.md` (S46-era release run; *different
run, not this one*): composer P99 there was 1.5 µs, audit throughput
9 003/s, report 16.7 ms. Both runs satisfy the same budgets with
multi-thousand-x headroom. The variance reflects machine state on the
respective days, not changes in code performance characteristics.

### 3.3 Python SDK — `python -m pytest mai-sdk-python/tests/ -v`

| Field | Value |
|---|---|
| Started | 2026-05-23 21:09:34 PDT |
| Finished | 2026-05-23 21:09:54 PDT |
| Wall clock | 19.68 s |
| Exit code | 0 |
| Passed | **94** | Failed | 0 | Ignored | 0 |

PYTHONPATH was set to `mai-sdk-python/src` per the SDK's package layout.

### 3.4 Python dashboard — `python -m pytest compliance-dashboard/tests/ -v`

| Field | Value |
|---|---|
| Started | 2026-05-23 21:10:05 PDT |
| Finished | 2026-05-23 21:10:10 PDT |
| Wall clock | 5.78 s |
| Exit code | 0 |
| Passed | **20** | Failed | 0 | Ignored | 0 |

### 3.5 Application scaffolds — `python -m pytest apps/<app>/tests/` (one app at a time)

Per-app smoke runs (separate invocations because the scaffolds share
the `tests.test_smoke` module name and collide if invoked together):

| App | Pass | Wall clock |
|---|---:|---:|
| compliance-routed | 11 | 4.67 s |
| local-secure-inference | 6 | 4.87 s |
| openbao-trust-demo | 17 | 5.63 s |
| operator | 12 | 5.56 s |
| rag-reference | 6 | 3.84 s |
| tribal-sovereignty | 9 | 2.90 s |
| **Total** | **61** | **27.47 s** (sum) |

All six exit 0. Started 2026-05-23 21:10:22 PDT; last finished ~21:10:50 PDT.

## 4. Grand Totals (this run)

| Surface | Pass | Fail | Ignored |
|---|---:|---:|---:|
| `cargo test --workspace` | 1 539 | 0 | 2 |
| `cargo test --release -p mai-compliance --test compliance_perf` | 3 | 0 | 0 |
| Python SDK | 94 | 0 | 0 |
| Python dashboard | 20 | 0 | 0 |
| Python scaffolds (6 apps) | 61 | 0 | 0 |
| **Grand total** | **1 717** | **0** | **2** |

(The compliance_perf release run's 3 tests overlap with the workspace
run's debug pass of the same test file, so a deduplicated "unique
tests exercised" count would be 1 714. The 1 717 number above counts
the perf re-run separately because that is what was actually
executed.)

## 5. Build Side-Effects

`cargo test --workspace` builds the entire workspace in debug profile
before running tests. After the run, `target/debug/` carried the
freshly built test binaries and incrementally re-built crate
artefacts. The release perf re-run compiled `mai-compliance` and its
deps once into `target/release/` (the `mai-api` release binary from
RC-03 was already present there; cargo reused unchanged artefacts).
Total `target/` footprint after RC-05 is well above the 54 GB
recorded at the start of RC-03 work, dominated by the test binaries
and `tests/` rmeta files. `target/` cleanup is a packaging concern
(RC1-PACKAGE-MANIFEST.md §4.1 excludes it from the bundle entirely).

## 6. The 2 Ignored Doc Tests

Both are in `mai-scheduler` and are marked `ignore` in the doc
comment itself:

```
test mai-scheduler\src\batch\mod.rs - batch (line 24) ... ignored
test mai-scheduler\src\topology\mod.rs - topology (line 20) ... ignored
```

Neither is a runtime skip — they are developer-marked ignores in the
source. Not failures; not deferrals; not RC1 blockers.

## 7. What Was NOT Run on This Host

This section is deliberately explicit. RC-05 evidence is what got
executed. Anything below was not exercised in this run and must not
be claimed as proven by this evidence.

### 7.1 SHIP-14 — 72-hour burn-in evidence

- **What landed in code:** the burn-in driver
  (`scripts/burn-in-72h.sh`, `scripts/burn-in-72h.ps1`), the ML-DSA
  report signer, and the smoke-mode plumbing.
- **What was exercised by this run:** the report-signing crypto path
  via `mai-compliance` unit tests; the driver scripts' own smoke
  mode is not invoked by `cargo test --workspace`.
- **What was NOT done in RC-05:** a full 72-hour endurance run on
  this host. No signed burn-in report exists from this RC-05 run.
  The SHIP-14 implementation is present; production-grade endurance
  evidence is pending and is not part of RC1.

### 7.2 GPU runtime paths

- `nvidia-smi` is unavailable on this host; the daemon's startup
  logs an explicit WARN
  (`nvidia-smi unavailable, using flat topology`) and uses a flat
  topology default.
- All GPU-touching code paths exercised in this run were the
  CPU-side scheduler / topology logic and its unit tests. No actual
  CUDA driver, no NVLink discovery, no measured-throughput
  per-device routing.
- GPU runtime validation is not part of RC-05 evidence.

### 7.3 Linux glibc target

- The Rust toolchain in use targeted `x86_64-pc-windows-msvc`.
- The Linux `x86_64-unknown-linux-gnu` cross-compile was not
  executed.
- The ship-profile deployment (systemd, `/etc/mai/`,
  `/var/lib/mai/`) is Linux-shaped per `docs/packaging/` and the
  SHIP-08 layout. None of that was exercised on this host.
- RC1 v2 prebuilt binaries from RC-03 (`lamprey-mai-api.exe`,
  `lamprey-mai-ship-validate.exe`) are Windows MSVC only.

### 7.4 Real secrets / real vault

- All vault-touching tests in this run used `StubVault` (the
  bring-up default) or the in-memory `NullSealer` (the audit-store
  default).
- No hardware TPM, no OpenBao, no production HSM was exercised.
- The vault crypto unit tests (PQC, AEAD, KDF) passed in the
  workspace run; the integration with a real vault provider is not
  RC-05 evidence.

### 7.5 Real regulated data

- All HIPAA, ITAR/EAR, and OCAP test scenarios used synthetic
  fixtures and the public demo scripts under
  `docs/acquisition/demos/`.
- No real PHI, no real ITAR-controlled technical data, no real
  tribal records were processed.
- The compliance demo tests prove the engine reaches the documented
  decisions for the synthetic inputs. They do not prove the engine
  is approved for any specific regulated workload on any specific
  customer site.

### 7.6 Network exposure

- The smoke test from RC-03 bound `mai-api` to `127.0.0.1:8420`
  only. No external network adapter, no TLS terminator, no reverse
  proxy was tested.
- The air-gap startup verification ran and reported `compliant`;
  that means the daemon-side check passed, not that an external
  air-gap auditor verified the host.

### 7.7 `mai-sdk-rs` HTTP client

- `KNOWN-ISSUES.md` Issue 15 records that `mai-sdk-rs` HTTP client
  methods are `todo!()` stubs.
- The 8 unit tests under `mai_sdk_rs` exercise types and helpers,
  not live HTTP calls.
- An end-to-end Rust SDK ↔ live `mai-api` round-trip was not part
  of RC-05.

### 7.8 Fresh-machine rehearsal

- This run was executed on the build host, not a clean tester
  machine. Cargo target caches, OS toolchain state, and prior
  artefacts were all warm.
- Session RC-06 is the dedicated fresh-machine rehearsal session.
  RC-05 does not substitute for it.

## 8. Acceptance Checklist (RC-05)

| Criterion | Status |
|---|---|
| Rust workspace tests run | §3.1 — `cargo test --workspace --no-fail-fast`, exit 0, 1 539 pass / 0 fail / 2 ignored |
| Compliance demo tests run | §3.1 (included in workspace run as `compliance_demos.rs`, 6 pass) |
| Python SDK tests run | §3.3 — 94 pass |
| Dashboard tests run | §3.4 — 20 pass |
| Scaffold tests run (if time permits) | §3.5 — 61 pass across 6 apps |
| Logs saved under a clean evidence folder | `test-evidence/rc-05/` (this commit) |
| Pass/fail status summarized in one markdown file | this document |
| Exact commands, dates, machine, results recorded | §1 + §2 + §3 |
| Failures documented as blockers or known deferrals | no failures in this run; deferrals enumerated in §7 |

## 9. Artifacts (`test-evidence/rc-05/`)

| File | Purpose |
|---|---|
| `cargo-test-workspace.log` | Full stdout/stderr from `cargo test --workspace` |
| `cargo-test-workspace-summary.txt` | Per-binary pass/fail/ignored breakdown (parsed from the log) |
| `cargo-test-workspace-start.txt`, `-end.txt`, `-exit.txt` | Timestamps + exit code |
| `compliance-perf-release.log` | Release-mode `compliance_perf` output incl. measured numbers |
| `compliance-perf-release-start.txt`, `-end.txt`, `-exit.txt` | Timestamps + exit code |
| `python-sdk-tests.log` | `pytest -v` output for the SDK |
| `python-dashboard-tests.log` | `pytest -v` output for the dashboard |
| `python-scaffold-tests.log` | Per-app `pytest` output (6 invocations concatenated) |

All logs are plain text and check in under `test-evidence/rc-05/`.

## 10. Next Session

Session RC-06 (Fresh Machine Rehearsal) copies the RC1 package to a
clean directory or clean machine, follows `README-FIRST.md` from
scratch without reading the rest of the repo, runs the API + at
least one Trust Manifold dry-run + at least one compliance demo
test, and documents every missing dependency or confusing step. RC1
is not considered ready for outside testers until RC-06 passes.
