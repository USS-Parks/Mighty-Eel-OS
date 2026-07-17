# Mighty Eel MAI Repository Audit — Stem to Stern

**Audit date:** 2026-07-07  
**Repository:** `USS-Parks/Mighty-Eel-OS` (renamed from `USS-Parks/im-mighty-eel-mai` on 2026-07-16)
**Worktree:** `C:\Users\17076\Documents\Claude\Mighty Eel OS\mai`  
**Revision:** `6ffaaeeea0a83c7fa071e114183cfa60c5898703`  
**Branch:** `cleanup/artifact-audit`  
**Disposition:** **STOP-SHIP pending security and truthful-runtime remediation**

## 1. Executive finding

The repository is substantial, test-rich, and mechanically healthier than its current release posture suggests: Rust formatting and compilation pass; 1,831 Rust tests pass; 1,221 Python tests pass when the SDK source path is established; both Python mypy gates pass; 23 console tests pass after a clean install; Cargo deny and npm audit pass; and the full no-slop integrity gate passes.

It is not ready to be represented as a production appliance. Twenty-four validated security findings remain open (11 high, 13 medium), including unauthenticated authority issuance, caller-selected cloud roles, fail-open or bypassable policy paths, tenant-crossing analytics and receipts, incomplete package/restore authentication, plaintext model storage, and readiness checks that can certify constructed rather than proven controls. Several public product surfaces also return plausible success while performing a placeholder operation. Release governance is materially stale: the canonical workspace include is missing, status front doors disagree, secret-scanner policy no longer matches the moved tree, the React console is not gated in CI, and 165 local Markdown links are broken across 45 tracked documents.

The correct next move is a single dependency-ordered remediation lane, not another feature lane. The companion canonical roster is `REPOSITORY-STEM-TO-STERN-PSPR-2026-07-07.md`.

## 2. Scope and method

The audit covered all 1,087 tracked files and the live worktree state, including:

- 35 Rust workspace members and 353 tracked Rust files;
- 245 Python files, the SDK, adapters, tools, demos, dashboard, and deployment helpers;
- the React/TypeScript Sovereignty Console;
- API, gRPC, WebSocket, OpenAI-compatible, Anthropic-compatible, WSF, AOG, vault, package, restore, update, and credential-broker boundaries;
- deployment, packaging, systemd, OpenBao staging, CI, integrity hooks, dependency policy, secret scanning, and release documentation;
- 215 tracked Markdown documents and their local link graph;
- the existing deterministic security scan and its full-file receipts.

Live OpenBao/AWS/GCP/Azure/ZFS/TPM/GPU production environments were not available. Findings requiring those systems remain open until live closure evidence exists.

## 3. Gate results

| Gate | Result | Evidence |
|---|---:|---|
| `cargo fmt --check` | PASS | No diff |
| `cargo check --workspace` | PASS | Workspace compiled |
| `cargo test --workspace` | PASS | 1,831 passed; 2 ignored; 137 suites |
| `cargo clippy --workspace -- -D warnings -A clippy::pedantic` | **FAIL** | `mai-core/src/cache.rs:109`, `doc_lazy_continuation` |
| Full Python suite with `PYTHONPATH=mai-sdk-python/src` | PASS | 1,221 passed; 89 skipped |
| Full Python suite without SDK installation/path | **FAIL** | Four application suites fail collection with `ModuleNotFoundError: mai` |
| Ruff, whole repository | **FAIL** | Three findings in `deployment/appliance/mock-llm/app.py` |
| mypy SDK strict | PASS | 9 source files |
| mypy adapters | PASS | 117 source files |
| Bandit | **FAIL** | One medium all-interface bind in the mock LLM |
| Console typecheck | PASS | After `npm ci` |
| Console tests | PASS | 23 tests in 8 files |
| `npm audit --audit-level=moderate` | PASS | 0 vulnerabilities |
| `cargo audit` | PASS with warning | `proc-macro-error2 2.0.1` is unmaintained; warning is allowed |
| Cargo deny advisories/bans/licenses | PASS with warnings | Duplicate dependency graph and two unused license allowances |
| Gitleaks worktree scan | **FAIL** | 17 findings; policy/path drift requires classification |
| Detect-secrets raw worktree scan | **FAIL** | Fixtures, docs, tracked staging material, and ignored local keys require a reviewed baseline |
| No-slop full tracked-tree scan | PASS | No CANON §11 violations |
| Tracked Markdown local-link audit | **FAIL** | 165 broken links in 45 files |

## 4. Validated security findings

The authoritative source/control/sink traces are in the repository-root `AUDIT_REPORT.md`. They are reproduced here as the release-level closure index.

| ID | Severity | Finding |
|---|---:|---|
| SEC-01 | High | AWS credential exchange accepts a caller-selected role ARN |
| SEC-02 | High | Unauthenticated WSF token issuance grants caller-selected authority |
| SEC-03 | High | AOG defaults to non-blocking shadow policy mode |
| SEC-04 | High | Legacy OpenAI completions bypass compliance routing and accounting |
| SEC-05 | High | Appliance composition publishes development OpenBao with a known root token |
| SEC-06 | High | Model package signature authenticates weights but not manifest identity |
| SEC-07 | High | Attenuation signs attacker-constructed child tokens without authenticating the parent |
| SEC-08 | High | Envelope unseal is not bound to tenant, subject, audience, or policy |
| SEC-09 | High | gRPC trusts caller-authored administrator metadata |
| SEC-10 | High | OpenAI streaming bypasses egress tokenization, metering, and receipts |
| SEC-11 | High | Anthropic streaming bypasses egress tokenization, metering, and receipts |
| SEC-12 | Medium | Restore accepts unsigned or unverified manifests by default |
| SEC-13 | Medium | Manifest-derived model ID can escape the vault root |
| SEC-14 | Medium | AOG revocation checking fails open when a snapshot is absent |
| SEC-15 | Medium | ROI endpoint computes recommendations from every tenant |
| SEC-16 | Medium | Restore component paths can escape backup and target roots |
| SEC-17 | Medium | Production readiness certifies a merely constructed vault |
| SEC-18 | Medium | Usage endpoint returns aggregates for every tenant |
| SEC-19 | Medium | AWS credentials can outlive remaining WSF token authority |
| SEC-20 | Medium | WSF receipt queries are unauthenticated and cross-tenant |
| SEC-21 | Medium | Vault snapshot and rollback APIs report success without ZFS operations |
| SEC-22 | Medium | ZFS vault stores and loads model weights as plaintext |
| SEC-23 | Medium | Production-like deployment images use mutable tags |
| SEC-24 | Medium | Revocation snapshots lack freshness, scope, and anti-rollback enforcement |

### Security disposition

These are release blockers. Unit coverage and prior hardening do not compensate for authority derived from caller-controlled fields, fail-open control state, bypass routes, or success reports unsupported by runtime proof. Any one of SEC-01 through SEC-11 is sufficient to block production release; the combined set requires architectural convergence before local fixes are accepted.

## 5. Functional truth findings

### FUN-01 — Model load can report `Loaded` without placing weights

`mai-core/src/registry.rs:386-393` loads bytes and then marks the registry entry `Loaded`; the code explicitly records that weights are not passed to the adapter/HIL placement path. This violates the meaning of the state and can produce false operational readiness.

**Required closure:** define a transactional load contract, perform backend placement, prove liveness, and only then publish `Loaded`; failure must roll back state and resources.

### FUN-02 — WebSocket inference completes immediately with zero output

`mai-api/src/streaming/ws.rs:572-589` documents the intended scheduler/token flow but currently removes the active request and returns `inference.complete` with zero tokens.

**Required closure:** route through the same authenticated, policy-governed, metered inference pipeline as REST and prove cancellation, backpressure, and receipt parity.

### FUN-03 — gRPC streaming and embeddings are placeholder implementations

`mai-api/src/grpc/inference.rs:230-231` emits a single completion chunk rather than real adapter streaming. Lines 348-352 construct empty embedding results rather than invoking an embedding backend.

**Required closure:** implement protocol parity through one shared application service and add cross-protocol equivalence tests.

### FUN-04 — Registry, profile, and model metadata endpoints fabricate or omit source-of-truth data

- `mai-api/src/grpc/registry.rs:97-100` calls a model count a scan.
- `mai-api/src/handlers/system.rs:353-411` returns only the caller or fabricates an `unknown` profile for admin lookups.
- `mai-api/src/handlers/models.rs:124-130` hardcodes safety/default metadata.
- `mai-vault/src/profiles.rs:52-53` uses interim JSON persistence instead of the claimed SQLite store.

**Required closure:** create typed source-of-truth repositories, remove fabricated values, and make unavailable capabilities return an explicit unsupported/unavailable error until real.

### FUN-05 — Adapter stream ownership is not concurrency-safe by contract

`mai-adapters/src/manager.rs:260-266` consumes the IPC receiver directly for one request and records that it cannot re-wrap it for other callers. Even if current tests pass, the ownership model is not a safe basis for concurrent multiplexed inference.

**Required closure:** introduce request-ID demultiplexing with one receiver owner, bounded per-request channels, cancellation, timeout cleanup, and stress tests.

### FUN-06 — Hot-swap still depends on a second legacy scheduler

`mai-api/src/server.rs:270-278` constructs a legacy scheduler solely for `HotSwapManager` while the server otherwise uses the new scheduler. Split scheduling authority risks state divergence during load, pause, and resume.

**Required closure:** migrate hot-swap to the canonical scheduler trait and prove one authoritative lifecycle state machine.

## 6. Build, CI, supply-chain, and scanner findings

### BLD-01 — The required Clippy gate is red

The workspace lint command fails at `mai-core/src/cache.rs:109`. The change is trivial, but the release consequence is not: a documented hard gate is currently failing.

### BLD-02 — Whole-repository Python lint/security scope is red

`deployment/appliance/mock-llm/app.py` lacks type annotations and binds to `0.0.0.0`. The bind may be intentional inside a container, but it must be explicit, configuration-bound, documented as test-only, and narrowly suppressed if retained.

### BLD-03 — Full Python test execution is environment-dependent

The repository-wide suite passes only after setting `PYTHONPATH=mai-sdk-python/src` or installing the package. The project does not provide one canonical command that establishes this automatically, so a clean checkout can fail during collection despite healthy code.

### CI-01 — The existing console is absent from CI

`.github/workflows/ci.yml` still says the TypeScript/Vitest job belongs to a future phase even though `console/` now contains a real application and 23 tests. Local typecheck and tests pass after `npm ci`, but regressions are not gated.

### SUP-01 — Secret-scanner policy has drifted and can mask tracked staging files

The Gitleaks evidence allowlist still targets `docs/LOCAL-GITDOCTOR-*`, while the files moved to `docs/scans/`. More importantly, `.gitleaks.toml` broadly excludes `deployment/*-staging/` on the assertion that those files are untracked, but nine files under `deployment/openbao-staging/` are tracked. A path-wide exclusion can therefore hide a real future secret in committed staging configuration.

The current 17 Gitleaks hits appear dominated by examples, contract fields, test fixtures, and moved evidence output, but they must be individually adjudicated. Ignored local private keys under `deployment/openbao-staging/openbao-tls/` are not tracked; they still require restrictive ACLs, lifecycle cleanup, and a scanner rule that distinguishes ignored local material from committed content.

### SUP-02 — Dependency policy passes but carries acknowledged debt

`cargo audit` reports the allowed unmaintained `proc-macro-error2 2.0.1` through `validator_derive`; Cargo deny reports a large duplicate graph and two unused license allowances. These are not immediate vulnerabilities, but release evidence should not call the dependency graph warning-free.

## 7. Documentation and governance findings

### DOC-01 — The tracked documentation graph has 165 broken local links

The failures span 45 files, including `README.md`, `docs/INDEX.md`, `docs/HANDOFF.md`, operations, compliance, acquisition, runbooks, RC1 records, and SDK quickstart material. Many are systematic reorganization errors: links were not rebased when documents moved under `docs/product`, `docs/scans`, `docs/operations`, or `docs/compliance`.

This is operationally significant. Broken runbook and incident links can delay recovery; broken acquisition links undermine diligence; broken status links make obsolete plans look current.

### GOV-01 — The workspace governance include is missing

Root `AGENTS.md` imports `@RTK.md`, but no `RTK.md` exists in the workspace. The remaining `.rtk/filters.toml` is not a substitute for the missing instruction source. Agents therefore cannot prove they loaded the complete governing rules.

### GOV-02 — Status front doors disagree

Root instructions describe DOUGHERTY as active and cite old paths. The live `docs/INDEX.md` says DOUGHERTY and RC-11 are closed, labels GITDOCTOR/IGD in flight, and also points to a July security-remediation lane. Its heading says “Current build state (2026-05-26)” despite July changes. This must be resolved into one dated state ledger.

### GOV-03 — Competing untracked security rosters create canonical ambiguity

The worktree contains `SECURITY-REMEDIATION-PSPR.md` and `REPOSITORY-SECURITY-REMEDIATION-PSPR-2026-07-07.md`, both untracked and overlapping. The larger roster covers 32 prompts; the smaller uses a different phase taxonomy. Neither alone covers the functional, CI, scanner, and documentation findings in this report.

**Required closure:** adopt the companion stem-to-stern roster as the sole top-level execution authority. Retain prior security documents as evidence or explicitly mark them superseded; do not execute parallel remediation taxonomies.

## 8. Coverage limitations and deferred proof

- No live production OpenBao, cloud STS, ZFS, TPM, air-gap NIC, GPU, or appliance hardware was exercised in this audit.
- Existing live-integration suites were reviewed but not rerun because their external services were unavailable.
- Python integration tests marked for real backends and hardware remained skipped.
- A local ignored staging directory contains generated key material; values were not reproduced in this report.
- Static scanners produce known fixture/document false positives; only source/control/sink-validated security findings are counted in SEC-01 through SEC-24.

These limitations reduce release assurance; they do not reduce the severity of findings already established from reachable code and deployment configuration.

## 9. Strengths worth preserving

- `unsafe_code = "forbid"` is enforced workspace-wide.
- Rust formatting, compilation, and 1,831 tests are green.
- Python strict typing gates are green for the SDK and configured adapters.
- Full Python behavior is strong once the environment contract is satisfied.
- The console is testable and currently green after a deterministic install.
- Cargo deny and npm audit are green.
- The repository has meaningful integrity hooks, CODEOWNERS, release tooling, live-integration suites, and extensive operational documentation.
- The existing security report contains unusually detailed source/control/sink traces and provides a credible remediation baseline.

The remediation program should preserve these strengths and converge duplicated paths rather than rewrite working subsystems gratuitously.

## 10. Release decision

**Decision: NO-GO / STOP-SHIP.**

Release may be reconsidered only after:

1. all 11 high security findings are closed with adversarial regression tests;
2. all 13 medium security findings are fixed or explicitly accepted by a named risk owner with expiry (production claims may not contradict an acceptance);
3. placeholder-success product surfaces are implemented or removed from advertised/production routes;
4. every required local and CI gate is green from a clean checkout;
5. scanner policy is narrowed and all findings are adjudicated;
6. the documentation link graph and status ledger are coherent;
7. live trust-plane, storage, restore, package, appliance, and hardware evidence is captured; and
8. an independent re-scan finds no unresolved high or critical issue.

Execution authority and exact acceptance criteria are in `REPOSITORY-STEM-TO-STERN-PSPR-2026-07-07.md`.
