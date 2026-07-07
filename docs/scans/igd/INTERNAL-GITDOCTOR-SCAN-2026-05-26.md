# Internal GitDoctor Scan — Live Repo Audit

**Scope:** `mai/` working tree at HEAD = `ee6eb13` (origin/main, clean sync)
**Date:** 2026-05-26
**Scanner:** Claude Opus 4.7 (1M ctx), running native PowerShell + cargo + ruff + mypy + gitleaks + cargo-deny + cargo-audit + pip-audit
**Reference:** comparable to the John Dougherty external GitDoctor scan (2026-05-24, overall 52/100) and the Memorial Day local scan (2026-05-25, overall 59/100)

---

## TL;DR — Overall **86 / 100**

A material improvement over both prior scans. The codebase is in a strong release-candidate posture: zero null-byte corruption, zero null-trailed truncation, zero `todo!`/`unimplemented!` in src, full Python type-check pass, history clean of secrets. The remaining gaps are **two well-defined Rust security advisories with known upstream fixes**, **one uncommitted staging artifact that contains a real-looking Vault service token in an unignored path**, **3 cosmetic clippy `collapsible_if` errors in `mai-vault`** that block `clippy -D warnings`, and **systematic non-compliance with the project's stated commit co-author rule** (50 of last 50 commits missing the footer; 165 of 303 total).

### Per-category scores

| Category          | Score | Direction vs Dougherty | Notes |
|-------------------|------:|:----------------------:|-------|
| Vibe              | 88    | ↑↑                     | Tooling, structure, and intent all coherent. Roadmap (Plan v2, Lamprey roadmap, RC2 evidence) is well-curated. |
| Production        | 82    | ↑                      | 34/34 config-level production gates pass via `lamprey-mai-ship-validate.exe`; 7 deferred runtime checks documented. |
| Code Quality      | 78    | ↑↑                     | mypy strict clean (9 SDK + 117 adapter source files). 3 clippy errors. 1 real `TODO` in `mai-adapters/src/manager.rs:586`. |
| Error Handling    | 84    | ↑                      | All `panic!` occurrences are inside `#[cfg(test)]` assertion arms. `unwrap`/`expect` widely used but mostly in tests; zero `todo!`/`unimplemented!` in src. |
| Security          | 70    | ↓ vs Memorial Day      | 2 unpatched RustSec vulns (idna 0.4.0, pyo3 0.22.6); 1 staging-artifact secret in unignored path; gitleaks history clean. |
| Testing           | 90    | ↑↑                     | Workspace `cargo check` passes offline; mypy strict pass on SDK + adapters; ruff clean on all Python; previous evidence (RC1 freeze) reports 1717 pass / 0 fail. |
| Docs              | 92    | =                      | 193 markdown files / 40 576 lines. Operator runbooks present (17). RC2 evidence pack complete. |
| Architecture      | 88    | =                      | Workspace cleanly split: `mai-core`/`mai-router`/`mai-scheduler`/`mai-compliance`/`mai-api`/`mai-vault`/`mai-sdk-rs`/`mai-agent`/`mai-adapters`/`mai-hil` + Python `adapters/`+`mai-sdk-python`+`compliance-dashboard`+`apps/`. |
| Scalability       | 78    | ↑                      | Composer P99 1.5 µs, audit 9 003 events/s, report gen 16.7 ms (S46 evidence). Adapter completion matrix shows 8 backends. |
| DevOps            | 78    | ↑                      | CI gates documented; pre-commit hook live; `verify-tree.sh` runnable. Co-author footer rule enforced in plan but not in commits. |

---

## 1 · Repo Inventory

| Metric                  | Value |
|-------------------------|-------|
| Tracked files           | **881** |
| Tracked bytes           | **20.8 MB** |
| Commits                 | **303** (single author `USS-Parks`, 2026-05-15 → 2026-05-26 — 11 days) |
| Branch / sync           | `main`, **0 commits ahead, 0 behind** `origin/main` |
| Rust source             | **262 `.rs`** files, **97 856** lines |
| Python source           | **244 `.py`** files, **41 053** lines |
| Markdown                | **193 `.md`** files, **40 576** lines |
| Toml                    | 60 |
| Shell / PowerShell      | 18 / 9 |
| Total code              | **138 909** lines (Rust + Python) |
| Largest tracked asset   | `docs/assets/lamprey-mai-logo.png` (2.13 MB) |

**Uncommitted state on top of HEAD:**

```
 M Cargo.lock
 M Cargo.toml
?? deployment/openbao-staging/
?? tools/gen-trust-staging/
```

The two `M` and the two `??` paths together form a single in-flight workstream — a new `tools/gen-trust-staging` crate that synthesizes ML-DSA-87 keys and signs a policy bundle for the (untracked) `deployment/openbao-staging/` config. The crate is **not formatted** (`cargo fmt --check` fails only on this file) and is **not gitignored**, so it is at risk of being staged accidentally. See finding **H-1**.

---

## 2 · Findings by Severity

### HIGH (3)

#### H-1 — Real-looking Vault service token sitting in `deployment/openbao-staging/openbao-connection.toml`, parent dir not gitignored
- **File:** `deployment/openbao-staging/openbao-connection.toml:10` (untracked)
- **Detector:** `gitleaks detect --no-git` rule `vault-service-token`, entropy 4.32
- **State:** `git check-ignore -v` returns nothing for the file; `.gitignore` covers `target/`, Python caches, `.env`, `node_modules/`, etc., but **not `deployment/openbao-staging/`** or any sibling staging directory. Existing tracked `deployment/*/profile.toml` files are real config, so the directory itself cannot simply be wildcarded.
- **Risk:** Any future `git add deployment/` (or naive `git add -A`, which CLAUDE.md already prohibits) commits the token to origin.
- **Recommendation:** Add `deployment/openbao-staging/` (and any other `*-staging/` patterns) to `.gitignore` *now*; rotate the token after; consider `.gitignore` whitelist style for `deployment/`.

#### H-2 — RustSec **RUSTSEC-2025-0020**: PyO3 0.22.6 buffer overflow in `PyString::from_object`
- **Detector:** `cargo audit` + `cargo deny check`
- **Path:** `pyo3 v0.22.6 → mai-adapters v0.1.0 → mai-api v0.1.0`
- **Fix:** upgrade `pyo3` to **≥ 0.24.1** (`cargo update -p pyo3`). PyO3 0.25 will change the API to take `&CStr`; pin accordingly.
- **Exposure:** the adapter bridge uses PyO3 to talk to Python sidecars; any path that calls `PyString::from_object` with non-NUL-terminated input is exploitable.

#### H-3 — RustSec **RUSTSEC-2024-0421**: `idna` 0.4.0 Punycode confusion (privilege escalation)
- **Detector:** `cargo audit` + `cargo deny check`
- **Fix:** upgrade transitively to `idna ≥ 1.0.3` (most ergonomically via `url ≥ 2.5.4`).
- **Exposure:** wherever host-name comparison is part of an authorization check (auth allow-lists, trust-anchor host matching), an attacker can mask names as `xn--…` labels.

### MEDIUM (4)

#### M-1 — 3 `clippy::collapsible_if` errors in `mai-vault` block `clippy -D warnings`
- **Files:**
  - `mai-vault/src/file_dev.rs:107` (3 nested ifs over models dir)
  - `mai-vault/src/file_dev.rs:153` (json snapshot scan, 2× — nested twice)
- **Detector:** `cargo clippy --workspace -- -D warnings -A clippy::pedantic`
- **Fix:** collapse via `&& let` chains as suggested by clippy. `cargo check --workspace` already passes; only the `-D warnings` lint gate fails.

#### M-2 — Co-author commit footer is the project rule but **165 of 303 commits (54 %) are missing it**; **50 of the last 50 commits** are missing it
- **Reference:** `JOHN-REMEDIATION-ROSTER.md` line 9 declares the rule mandatory ("every commit, no exceptions"). Memory note `feedback_commit_coauthor.md` says the same.
- **Pattern in current HEAD:** subject-only commits (`docs: RC2 production validation evidence — all gates GO`) with no body, no co-author line.
- **First commit that contains the footer:** `80ab1b4 LCH-1: desktop icon for lamprey-mai.exe + halved ASCII banner`. The footer adoption rate is high in the LCH-/SCAN-/J- families and near-zero in `docs:`-prefixed commits.
- **Risk:** governance signal lost; future audit cannot reliably attribute work; the rule loses force the more it is broken.
- **Recommendation:** install a `commit-msg` hook that rejects commits without the line; backfill is not worth rewriting history for.

#### M-3 — Format/lint debt in the in-flight `tools/gen-trust-staging` crate
- **File:** `tools/gen-trust-staging/src/main.rs` (untracked, ~105 lines).
- **Detector:** `cargo fmt --all -- --check` (rustfmt diff in `use` groupings and one builder chain).
- **Recommendation:** run `cargo fmt -p gen-trust-staging` before staging; the `Session-Worktree.ps1` workflow makes this trivial.

#### M-4 — Local pip CVEs (CVE-2026-3219, CVE-2026-6357) surfaced by `pip-audit`
- **Scope:** the **system pip (26.0.1)**, not a project dependency.
- **Fix:** the project ships nothing affected, but `pip install -U pip` on developer machines clears the noise from `pip-audit` runs and brings the build-image baseline current.

### LOW (5)

#### L-1 — 3 source files missing trailing newline
- `apps/compliance-routed/main.py`, `apps/operator/main.py`, `apps/operator/tests/test_smoke.py`
- POSIX hygiene; not enforced by any current hook.

#### L-2 — One real `TODO` in committed source
- `mai-adapters/src/manager.rs:586` — "Track in-flight request count per adapter". (Already documented in `docs/KNOWN-ISSUES.md`.)
- Plus 3 other in-source `TODO`s in `mai-core/src/models/usb.rs:161` and `mai-scheduler/src/default.rs:394/399/402`, all session-pinned and pre-disclosed in `KNOWN-ISSUES.md`.

#### L-3 — Unmaintained transitive dep `proc-macro-error 1.0.4` (RUSTSEC-2024-0370)
- **Path:** `proc-macro-error → validator_derive 0.16.0 → validator 0.16.1 → mai-api`
- **Fix:** upgrade `validator` to 0.18+ (drops `proc-macro-error`). Informational only — no security exposure, just an unmaintained crate warning.

#### L-4 — One stale `.claude/skills/safe-edit/SKILL.md` tracked in `mai/`
- Harmless; project policy permits `.claude/CLAUDE.md` to be tracked.

#### L-5 — `Cargo.lock` and `Cargo.toml` are dirty in the working tree
- Tied to in-flight `gen-trust-staging` (see H-1 / M-3). Not a defect per se; flagged so the next operator does not commit them outside the appropriate session.

---

## 3 · What Was Verified PASSING

| Gate                                                | Result |
|-----------------------------------------------------|--------|
| `cargo check --workspace --offline`                 | **PASS** (exit 0) |
| `cargo fmt --check` on **tracked** tree              | PASS (fails only on untracked `gen-trust-staging`) |
| `cargo clippy --workspace -- -D warnings -A pedantic`| FAIL on 3 collapsible_if in `mai-vault` (M-1); rest of workspace clean |
| Integrity scan (803 files: `.rs/.py/.toml/.json/.js/.ts/.md/.sh/.ps1/.yml/.yaml`) | **0 null-byte FAILs / 0 brace-imbalance FAILs** |
| Trailing-newline scan (759 source files)            | 3 misses (L-1); 756 clean |
| `ruff check adapters/ mai-sdk-python/ apps/`        | **All checks passed** |
| `mypy --strict mai-sdk-python/src/`                 | **Success: no issues found in 9 source files** |
| `mypy adapters/`                                    | **Success: no issues found in 117 source files** |
| `gitleaks detect` (full git history, 475 commits)   | **no leaks found** |
| `gitleaks detect --no-git` (working tree)           | 1 leak in untracked staging file (H-1) |
| `cargo audit`                                       | 2 vulns (H-2 + H-3), 1 unmaintained (L-3); 3 advisories pre-ignored (RUSTSEC-2025-0144, -2024-0384, -2024-0436) |
| `cargo deny check`                                  | Bans OK, licenses OK, sources OK; advisories FAILED (H-2, H-3, L-3) |
| `todo!` / `unimplemented!` in `src/`                | **0 occurrences** |
| `unsafe { … }` blocks                               | Confined to `tools/mai-launcher/src/{main,splash}.rs` (Win32 FFI — expected) |
| Local sync vs `origin/main`                         | clean — 0 ahead / 0 behind |
| Pre-existing test footprint (RC1 freeze evidence)   | 1717 pass / 0 fail / 2 ignored across `cargo test --workspace` + Python + dashboard scaffolds |
| DOUGHERTY lane (J-01..J-26 + J-10b)                 | **CLOSED** — 28 J-tagged commits landed, evidenced by `9d68ab0 J-15: DOUGHERTY lane response doc + lane closure` and `a072634 Ship adapter hardening updates` (covers J-23/J-26 per `efe1576`) |

> Note: an initial `cargo test --workspace --no-run` was blocked by `os error 5 (Access is denied)` on `target/debug/lamprey-mai-api.exe` because **PID 1220** (`lamprey-mai-api`) was running. Re-running with `--exclude mai-api` then compiled the entire rest of the workspace's test crates **clean (exit 0)**. The locked `mai-api` test binary is an environmental artifact of having the dev server running, not a defect.

---

## 4 · Comparison to Prior Scans

| Dimension                    | Dougherty external (2026-05-24) | Memorial Day local (2026-05-25) | **This scan (2026-05-26)** |
|------------------------------|--------------------------------:|--------------------------------:|---------------------------:|
| Overall                      | 52                              | 59                              | **86** |
| Critical findings            | 0                               | 0                               | 0 |
| High findings                | 5 H (per remediation plan)       | 5 (mostly false-positive)       | **3** (1 secret in untracked, 2 RustSec) |
| Failed checks                | 9 of 50                         | 24 of 58                        | (different scanner; see §3) |
| `todo!`/`unimplemented!` src | nonzero (Issue 15)              | flagged via QUA-003 heuristic   | **0** |
| Adapter coverage             | 1 backend                       | 8 backends                      | **8 backends** (J-18..J-26) |
| mai-sdk-rs HTTP/SSE          | `todo!()` stubs (Issue 15)      | open                            | **CLOSED** (J-16, J-17) |
| pyo3 vulnerability           | n/a                             | not surfaced                    | **OPEN** (H-2 — new since 0.22.6 was wired in `0118ecc`) |

The Memorial Day scanner's "HIGH" findings (SEC-003 SQL injection, SEC-004 hardcoded secrets, SEC-016 state-changing GETs) were almost entirely **pattern-match false positives** — matching the word "select" in docstrings, environment-variable *names* (not values), and `DELETE` route doc-comments. The internal scan reported here uses real tools (`gitleaks`, `cargo audit`, `cargo deny`, `mypy --strict`, `cargo clippy -D warnings`) and the residual findings are all defensible.

---

## 5 · Recommended Next Actions (priority-ordered)

1. **Now, before any further `git add`:** add `deployment/openbao-staging/` to `.gitignore`; rotate the token at line 10 of the untracked `openbao-connection.toml`. (H-1)
2. **Next session:** `cargo update -p pyo3 --precise 0.24.1` (or whatever PyO3 0.25 lands at) and `cargo update -p url` to lift idna ≥ 1.0.3. Re-run `cargo audit` + `cargo deny check` to confirm green. (H-2, H-3)
3. **Same session as #2:** install a `commit-msg` hook that requires the canonical co-author line. Optional: a CI gate. (M-2)
4. **Same session as #2:** apply clippy's `collapsible_if` fixes in `mai-vault/src/file_dev.rs` so `clippy -D warnings` is green workspace-wide. (M-1)
5. **Opportunistic:** upgrade `validator` to 0.18 to drop the unmaintained `proc-macro-error` transitive dep. (L-3)
6. **Cosmetic:** add trailing newlines to the 3 `apps/` files; consider an `editorconfig` + a pre-commit hook for it. (L-1)
7. **In-flight `tools/gen-trust-staging`:** decide if this is RC-12 or post-RC work; either way `cargo fmt` it before staging, and commit with the co-author footer per the project rule. (M-3)

---

## 6 · Caveats / What This Scan Did NOT Do

- Did **not** run `cargo test --workspace` (running mai-api binary held the artifact lock); the most recent green run is the RC1 freeze evidence (1717/0/2).
- Did **not** run the 72 h burn-in driver (`.integrity/scripts/burn-in-72h.sh`).
- Did **not** exercise GPU paths, real Vault backends, real trust anchors, or any of the 7 deferred SHIP runtime checks.
- Did **not** run `pytest` end-to-end (only the static Python gates).
- Did **not** scan the `Lamprey-MAI-RC2/` bundle or `Island-Mountain-RC1-release/` artifacts in the workspace root — those are output, not source.
- Did **not** evaluate the untracked `tools/gen-trust-staging` crate against `cargo check` (it would be picked up once `Cargo.toml` change is committed).

---

*Internal GitDoctor Scan — 2026-05-26 — Authored and reviewed by Basho Parks, copyright 2026*
