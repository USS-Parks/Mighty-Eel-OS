# May 25 Internal GitDoctor Live Findings

**Project:** Island Mountain Mighty Eel OS / MAI / Lamprey  
**Report date:** 2026-05-26  
**Planning baseline:** May 25, 2026 forward remediation  
**Workspace:** `C:\Users\17076\Documents\Claude\Island Mountain Mighty Eel OS\mai`  
**Branch state observed:** `main...origin/main`  
**Purpose:** record the live internal GitDoctor-style findings that remain after the GD75 remediation evidence pack, separating static scanner score from real command-level release readiness.

This report is intentionally blunt: the mapped GitDoctor-style scan is strong, but the live verification runner is not release-clean. The next release lane should treat those as two different truths.

---

## 1. Rescan Contract

This run used the GD75 rescan material as the contract for what a reviewer or external scanner should see:

- `docs/GITDOCTOR-75-RESCAN-INSTRUCTIONS.md`
- `docs/GITDOCTOR-75-EVIDENCE-PACK.md`

Those documents define the expected scan root as `mai/`, require the GD75 evidence pack and OpenAPI contract to be visible, and call out MAI's intentional localhost-bound, air-gapped appliance posture.

### Report Inputs

- Temp mapped scan markdown: `%TEMP%\opencode\May25-Internal-GitDoctor-Live-Scan.md`
- Temp mapped scan JSON: `%TEMP%\opencode\May25-Internal-GitDoctor-Live-Scan.json`
- Temp evidence runner markdown: `%TEMP%\opencode\May25-Internal-GitDoctor-Live-Evidence.md`
- Temp evidence runner JSON: `%TEMP%\opencode\May25-Internal-GitDoctor-Live-Evidence.json`
- GD75 rescan instructions: `docs/GITDOCTOR-75-RESCAN-INSTRUCTIONS.md`
- GD75 evidence pack: `docs/GITDOCTOR-75-EVIDENCE-PACK.md`

### Toolchain Observed

- `cargo 1.95.0`
- `rustc 1.95.0`
- `Python 3.14.4`
- `CARGO_NET_OFFLINE=true`

---

## 2. Executive Findings

### Truth A: mapped static scan is strong

The mapped GitDoctor-style scanner reports:

| Metric | Result |
|---|---:|
| Overall score | **93 / 100** |
| Checks | 58 |
| Passed | 54 |
| Failed | 4 |

Category scores:

| Category | Score | Passed | Failed |
|---|---:|---:|---:|
| Security | 100 | 16 | 0 |
| Testing | 100 | 6 | 0 |
| Configuration | 100 | 7 | 0 |
| Performance | 100 | 6 | 0 |
| Project Hygiene | 100 | 5 | 0 |
| Review Integrity | 88 | 7 | 1 |
| Code Quality | 70 | 7 | 3 |

### Truth B: live verification is not release-clean

The three-layer evidence runner reports:

| Layer | Pass | Fail | Skipped | Total |
|---|---:|---:|---:|---:|
| Adversarial scanner fixtures | 1 | 0 | 0 | 1 |
| Independent implementations | 3 | 11 | 1 | 15 |

Passing independent probes:

- `gitleaks` secret scan passed.
- `tokei` line-count scan passed.
- `scc` complexity scan passed.

Failing or skipped independent probes are listed in section 4. These are higher-priority release blockers than the mapped static scan score.

---

## 3. Mapped Static Findings

### QUA-001: God files over 300 lines

**Severity:** Medium  
**Finding count:** 12 files  
**Pattern:** mostly adapter and adapter-client files.

Evidence:

- `adapters/base.py` - 578 lines
- `adapters/ollama/adapter.py` - 318 lines
- `adapters/ollama/client.py` - 301 lines
- `adapters/onnxruntime/adapter.py` - 368 lines
- `adapters/onnxruntime/client.py` - 373 lines
- `adapters/openai_compat/adapter.py` - 465 lines
- `adapters/openai_compat/client.py` - 394 lines
- `adapters/runner.py` - 385 lines
- `adapters/sglang/adapter.py` - 387 lines
- `adapters/tensorrt/adapter.py` - 432 lines
- `adapters/tensorrt/client.py` - 401 lines
- `adapters/tgi/adapter.py` - 350 lines

Interpretation: not a ship stopper by itself, but it is the clearest remaining mapped code-quality drag. Prefer targeted extraction only where it reduces risk: shared adapter client helpers, common validation, or repeated backend response parsing.

### QUA-008: Modules with 15+ exports

**Severity:** Low  
**Finding count:** 8 modules  
**Pattern:** broad Rust modules with many public items.

Evidence:

- `mai-adapters/src/bridge.rs` - 27 exports
- `mai-agent/src/context.rs` - 19 exports
- `mai-agent/src/rag.rs` - 15 exports
- `mai-agent/src/stt.rs` - 15 exports
- `mai-agent/src/tasks.rs` - 23 exports
- `mai-agent/src/tools.rs` - 22 exports
- `mai-agent/src/types.rs` - 39 exports
- `mai-api/src/auth.rs` - 22 exports

Interpretation: this is mostly reviewability debt. It should be handled after the live verification blockers, unless a module is already being touched for a real bug.

### QUA-009: Deeply nested code

**Severity:** Medium  
**Finding count:** 8 locations  
**Pattern:** mostly adapter entry points and clients.

Evidence:

- `adapters/base.py:222`
- `adapters/exllamav2/adapter.py:70`
- `adapters/exllamav2/client.py:74`
- `adapters/llamacpp/adapter.py:71`
- `adapters/llamacpp/client.py:75`
- `adapters/mlx/adapter.py:93`
- `adapters/mlx/client.py:97`
- `adapters/ollama/adapter.py:89`

Interpretation: flatten only when it improves error handling or lifecycle clarity. Avoid churn that merely pleases the heuristic.

### REV-007: Duplicated boilerplate blocks

**Severity:** Low  
**Finding count:** 8 duplicate-block hits  
**Pattern:** HIL driver and shared type boilerplate.

Evidence:

- `mai-core/src/power/demotion.rs:12` duplicates a 6-line block in `mai-core/src/power/mod.rs:92`
- `mai-api/src/types.rs:183` duplicates a 6-line block in `mai-core/src/registry.rs:36`
- `mai-hil/src/drivers/amd.rs:125` duplicates a 6-line block in `mai-hil/src/drivers/cpu.rs:130`
- `mai-hil/src/drivers/amd.rs:126` duplicates a 6-line block in `mai-hil/src/drivers/cpu.rs:131`
- `mai-hil/src/drivers/amd.rs:127` duplicates a 6-line block in `mai-hil/src/drivers/cpu.rs:132`
- `mai-hil/src/drivers/amd.rs:128` duplicates a 6-line block in `mai-hil/src/drivers/cpu.rs:133`
- `mai-hil/src/drivers/amd.rs:147` duplicates a 6-line block in `mai-hil/src/drivers/cpu.rs:149`
- `mai-hil/src/drivers/amd.rs:148` duplicates a 6-line block in `mai-hil/src/drivers/cpu.rs:150`

Interpretation: this belongs in a review-integrity polish lane, not the immediate release gate, unless the duplicated blocks hide diverging behavior.

---

## 4. Live Verification Failures

### Rust workspace checks fail under offline Cargo

Affected probes:

- `IND-RS-001`: `cargo check --workspace`
- `IND-RS-002`: `cargo clippy --workspace -- -D warnings -A clippy::pedantic`
- `IND-RS-003`: `cargo test --workspace`
- `IND-RS-005`: `cargo deny check`

Failure shape:

```text
error: failed to select a version for the requirement `http = "^1.0.0"` (locked to 1.4.1)
candidate versions found which didn't match: 1.4.0
location searched: crates.io index
required by package `axum v0.8.9`
As a reminder, you're using offline mode (--offline)
```

Observed environment:

```text
CARGO_NET_OFFLINE=true
```

Interpretation: this is not yet evidence that Rust code is broken. It is evidence that the release verification environment cannot reproduce the locked dependency graph offline. That is a ship-readiness problem because offline/air-gapped reproducibility is part of the product story.

Required follow-up: either vendor the exact locked crate set, refresh the local Cargo cache with `http 1.4.1`, or document a deterministic preflight that proves the offline cache matches `Cargo.lock`.

### `cargo audit` cannot write advisory lock

Affected probe:

- `IND-RS-004`: `cargo audit`

Failure shape:

```text
failed to obtain lock file 'C:\Users\17076\.cargo\advisory-db..lock'
attempted to take an exclusive lock on a read-only path
```

Interpretation: this is an environment/cache-permission failure, not an advisory finding. It still blocks live release evidence because the audit tool cannot complete.

Required follow-up: move `CARGO_HOME` / advisory DB to a writable controlled path for evidence runs, or pre-stage a writable advisory database under the workspace/test temp root.

### Python repository test collection fails

Affected probe:

- `IND-PY-001`: `python -m pytest -q --ignore=target --ignore=results`

Failure shape:

- 29 collection errors.
- `ModuleNotFoundError` for:
  - `tests.e2e`
  - `tests.integration`
  - `tests.integrity`
- Permission errors under:
  - `py_tmp_dir`
  - `pytest_temp`

Important nuance: `tests/__init__.py`, `tests/e2e/__init__.py`, `tests/integration/__init__.py`, and `tests/integrity/__init__.py` are present. The failure likely comes from import-mode/path collisions plus stale temp directories being collected.

Required follow-up: make the repository-wide pytest command release-safe. At minimum, ignore local temp roots in the evidence runner and confirm `tool.pytest.ini_options.addopts = "--import-mode=importlib"` is actually loaded in the command environment.

### Python lint fails

Affected probe:

- `IND-PY-002`: `python -m ruff check .`

Failure count: 4.

Evidence:

- `tests/e2e/test_compliance_smoke.py:181` - `S110` try/except/pass
- `tests/e2e/test_compliance_smoke.py:191` - `SIM105` recommends `contextlib.suppress`
- `tests/e2e/test_compliance_smoke.py:193` - `S110` try/except/pass
- `tools/local_gitdoctor_scan.py:327` - `SIM103` can return condition directly

Interpretation: these are small, concrete hygiene fixes. They should be cleared before the next live report.

### Bandit fails on the same try/except/pass blocks

Affected probe:

- `IND-PY-003`: `python -m bandit -r . -f json -c pyproject.toml`

Failure shape:

- `tests/e2e/test_compliance_smoke.py:181` - `B110`
- `tests/e2e/test_compliance_smoke.py:193` - `B110`

Interpretation: this overlaps with the ruff failures. One small cleanup should clear both tools.

### `pip-audit` cannot create its cache directory

Affected probe:

- `IND-PY-004`: `python -m pip_audit`

Failure shape:

```text
PermissionError: [WinError 5] Access is denied: 'C:\Users\17076\AppData\Local\pip-audit'
```

Interpretation: environment/cache-permission failure. It does not prove a vulnerable dependency, but it blocks dependency-audit evidence.

Required follow-up: run `pip-audit` with a writable cache directory under the workspace temp root, or add an evidence-runner option that sets a controlled cache path.

### Secret scanning is split

Passing probe:

- `IND-SEC-001`: `gitleaks detect --source . --no-git --redact`

Failing probe:

- `IND-SEC-002`: `detect-secrets scan --all-files`

Failure shape:

```text
PermissionError: [WinError 5] Access is denied
```

Interpretation: `gitleaks` found no leaks, which is the more important signal. `detect-secrets` failed from Windows multiprocessing/pipe permissions, not from discovered secrets. Still, the live report should not call the secret-scan layer fully clean until the second tool either passes or is replaced with a deterministic single-process invocation.

### Docker lint skipped

Affected probe:

- `IND-DOC-001`: `hadolint Dockerfile`

Status:

```text
SKIPPED: tool not installed: hadolint
```

Interpretation: the Dockerfile may be fine, but this evidence run did not prove it. Install `hadolint`, run it in CI, or provide a containerized lint path.

### Complexity scan split

Passing probes:

- `IND-CPLX-001`: `tokei`
- `IND-CPLX-002`: `scc`

Failing probe:

- `IND-CPLX-003`: `radon`

Failure shape:

```text
timed out after 180s
```

Interpretation: this is not a complexity failure; it is a tool timeout on the repository scope. Narrow radon to production Python packages or increase timeout. Do not treat the timeout as evidence of excessive complexity without a completed run.

---

## 5. Workspace Hygiene Findings

Tracked stray files still exist:

- `12`
- `et HEAD`

These were confirmed as tracked files:

```text
100644 bd9899a40c961a6c97be5abd0143b5c11806e685 0  12
100644 81f4af285f69364ff8642eef640a65fb97f73bf8 0  et HEAD
```

Local temp directories still interfere with live scans:

- `py_tmp_dir`
- `pytest_temp`
- `pytest-cache-files-txhvvf0c`

Current ignore behavior observed:

- `.gitignore` ignores `pytest_temp/`
- `.gitignore` ignores `.tmp/`
- `.gitignore` ignores `results/`
- `.gitignore` ignores `target/`

It does not appear to cover every observed local temp artifact, and tracked files `12` / `et HEAD` are already in the index so ignore rules alone will not remove them from release packages.

---

## 6. Recommended Session Blocks

### LIVE-01: Land this live findings report

**Goal:** preserve the evidence before it gets blurred into the larger release plan.  
**Acceptance:** this file exists in `docs/`, has the live mapped score, has the independent probe failures, and does not claim ship readiness.

### LIVE-02: Fix Cargo offline reproducibility

**Goal:** make `cargo check`, `cargo clippy`, `cargo test`, and `cargo deny` runnable in the release evidence environment.  
**Work:** decide between vendoring, cache refresh, or controlled online preflight.  
**Acceptance:** the same commands pass with `CARGO_NET_OFFLINE=true`, or the release evidence docs explicitly require and verify a dependency-cache preparation step.

### LIVE-03: Clean Python collection path and temp-dir interference

**Goal:** make `python -m pytest -q --ignore=target --ignore=results` collect deterministically.  
**Work:** prevent `py_tmp_dir`, `pytest_temp`, and `pytest-cache-files-*` from collection; verify package/import mode.  
**Acceptance:** repository-wide pytest either passes or fails only on real test assertions, not collection/import/temp permission errors.

### LIVE-04: Clear ruff and bandit nits

**Goal:** remove the small Python hygiene failures.  
**Work:** replace `try/except/pass` in `tests/e2e/test_compliance_smoke.py`; simplify the scanner helper conditional in `tools/local_gitdoctor_scan.py`.  
**Acceptance:** `ruff check .` and `bandit -r . -f json -c pyproject.toml` pass or have only documented, intentional findings.

### LIVE-05: Fix audit-tool cache and permission behavior

**Goal:** make audit tools complete in a controlled local evidence environment.  
**Work:** set writable cache paths for `cargo audit` and `pip-audit`; make `detect-secrets` run without Windows multiprocessing permission errors.  
**Acceptance:** `cargo audit`, `pip-audit`, and `detect-secrets` either pass or produce actionable vulnerability findings.

### LIVE-06: Remove tracked stray files and harden ignore rules

**Goal:** prevent known debris from entering RC bundles.  
**Work:** remove tracked `12` and `et HEAD`; add ignore coverage for observed temp roots if missing.  
**Acceptance:** `git ls-files -- "12" "et HEAD"` returns nothing, and `git check-ignore` covers the recurring temp roots.

### LIVE-07: Prove Docker lint

**Goal:** turn Docker lint from skipped into evidence.  
**Work:** install `hadolint`, containerize it, or add a CI job that runs it.  
**Acceptance:** `hadolint Dockerfile` runs and its result is captured.

### LIVE-08: Rerun full evidence runner

**Goal:** replace this report's failing-probe table with a clean or consciously deferred table.  
**Work:** rerun the mapped scanner and evidence runner after LIVE-02..LIVE-07.  
**Acceptance:** independent probes are all PASS, SKIPPED-with-rationale, or FAIL-with-owner-session.

---

## 7. Verification Plan

After this report lands:

1. Confirm this file exists at `docs/MAY-25-INTERNAL-GITDOCTOR-LIVE-FINDINGS.md`.
2. Read back the last five lines and total line count.
3. Re-run the mapped scanner command from `docs/GITDOCTOR-75-RESCAN-INSTRUCTIONS.md`:

```powershell
python tools/local_gitdoctor_scan.py --root . --format markdown --fail-on none
```

4. Do not claim ship readiness until the independent evidence runner is passing or every failure is explicitly classified as environmental, fixed, or deferred.

---

## 8. Bottom Line

The mapped scanner result is encouraging: 93/100 with zero security, testing, configuration, performance, or project-hygiene failures.

The live evidence result is the harder truth: the release verification environment still cannot cleanly run Rust gates, repository-wide Python tests, Python lint/security scans, dependency audits, Docker lint, and one complexity probe. That is where the next sessions should go before RC2 or production-appliance claims.
