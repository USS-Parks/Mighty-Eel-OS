# Security Remediation DEVLOG

Execution log for the Repository Security Remediation PSPR (PSPR-00 … PSPR-31).
One receipt per prompt, appended in order. Evidence under
`test-evidence/security-remediation/PSPR-XX/`. Ledger: `SECURITY-REMEDIATION-LEDGER.md`.

**Workspace decision (recorded per PSPR §0.1):** Work is performed in the main `mai/`
checkout on branch `cleanup/artifact-audit` (HEAD `6ffaaee`, the audited revision), **not**
in a new `session/` worktree. Rationale: (1) PSPR §0.2 directs work in the standalone `mai/`
repository and does not require a worktree; (2) every *other* active session (LOOM-1..5, SEC-1,
SOV-1, node24) is already isolated in its own worktree with its own index, so the main checkout
is unshared and carries no cross-session staging-contamination risk; (3) the 157 GB warm build
cache and the untracked reference docs (`AUDIT_REPORT.md`, `docs/scans/threat_model.md`) live in
the main checkout. Mitigations honored: individual-file staging only (never `git add -A`),
`git diff` inspection before any stage, and **no commit/push without separate explicit approval**.
This diverges from the project `.claude/CLAUDE.md` worktree protocol; the divergence is a
process-hygiene choice, not a security-invariant downgrade.

---

## PSPR-00 — Baseline, ledger, and Clippy release gate

**Objective:** Reproducible remediation baseline; create the ledger; restore the known Clippy
release gate without changing product behavior. Closes the quality follow-up only; no security
finding is closed.

**Starting HEAD:** `6ffaaeeea0a83c7fa071e114183cfa60c5898703` (branch `cleanup/artifact-audit`)
**Ending HEAD:** `6ffaaeeea0a83c7fa071e114183cfa60c5898703` (no commit made — working-tree changes only)
**Snapshot digest:** `codex-security-snapshot/v1:sha256:b920522f2f117347053cfb8f0e35237868c1da3b9743ecd7549edb755bf7ddb4`

### Pre-change failing proof

`cargo clippy --workspace -- -D warnings -A clippy::pedantic` → **exit 101**:

```
error: doc list item without indentation
   --> mai-core\src\cache.rs:109:9
    = note: `-D clippy::doc-lazy-continuation` implied by `-D warnings`
error: could not compile `mai-core` (lib) due to 1 previous error
```

Evidence: `test-evidence/security-remediation/PSPR-00/02-cargo-clippy-BASELINE.txt`.

### Audit correction (finding surfaced during PSPR-00)

The audit recorded the Clippy gate failing at exactly one site. Because the gate aborts at the
first failing crate, further same-lint violations were masked. A forced full re-lint (bumped
mtime on all 353 workspace `.rs` files, then `cargo clippy --workspace -- -A clippy::pedantic`)
found **10** `doc_lazy_continuation` warnings, all in lib/bin, across three crates. All were
repaired as documentation-only edits (no `#[allow]`, no suppression). Recorded in the ledger Q-1
note. Evidence: `07-gate-scope.txt`.

> A first auto-fix attempt (`cargo clippy --fix --all-targets -- -A clippy::all -W …`) overreached:
> the workspace `[workspace.lints.clippy]` table kept other lints active and `--all-targets` pulled
> in test files, so clippy rewrote ~180 unrelated lines (control-flow inversions, `&raw` pointer
> syntax, format-arg inlining) across 48 files. **That was fully reverted** (`git checkout HEAD --`
> on the 48 `.rs` files, preserving the pre-existing `docs/INDEX.md` change) and replaced with the
> 10 surgical hand edits. Diff verified doc-comment-only.

### Files changed (why)

| File | Change | Why |
|:--|:--|:--|
| `mai-core/src/cache.rs` | +`- ` bullet on 1 doc line | restore list item |
| `mai-scheduler/src/scoring/mod.rs` | +`- ` bullets on 5 doc lines | restore list items |
| `mai-api/src/routes.rs` | +`- ` bullets on 2 doc lines | restore list items |
| `mai-api/src/server.rs` | indent 2 numbered sub-steps (`3b.`) | continuation of step 3 |

New tracking/evidence artifacts (non-product): `docs/sessions/SECURITY-REMEDIATION-LEDGER.md`,
`docs/sessions/SECURITY-REMEDIATION-DEVLOG.md`, `test-evidence/security-remediation/PSPR-00/*`.
Pre-existing untouched: `docs/INDEX.md` (` M`), `.opencode/`, `AUDIT_REPORT.md`, `docs/scans/*`.

### Verification tier (baseline, post-fix state)

| Gate | Result | Evidence |
|:--|:--|:--|
| `cargo fmt --check` | PASS (0) | `08-fmt-final.txt` |
| `cargo check --workspace` | PASS (0) | `10-cargo-check.txt` |
| `cargo clippy --workspace -- -D warnings -A clippy::pedantic` | PASS (0) after fix | `09-clippy-gate-final.txt` |
| `cargo test --workspace` | PASS — 1831 passed, 2 ignored, 137 suites, 0 failed | `16-cargo-test.txt` |
| `cargo audit` | PASS (0) | `11-cargo-audit.txt` |
| `cargo deny check` | PASS (0) | `12-cargo-deny.txt` |
| `ruff check .` | **FAIL (1)** — 3 pre-existing errors, all in `deployment/appliance/mock-llm/app.py` (ANN202, ANN002, S104) | `13-ruff.txt` |
| `mypy --strict mai-sdk-python/src/` | PASS (0) | `14-mypy-sdk.txt` |
| `mypy adapters/` | PASS (0) | `15-mypy-adapters.txt` |
| `pytest -q` | PASS — 1221 passed, 89 skipped (after `pip install -e mai-sdk-python`) | `17-pytest.txt` |

`ruff` FAIL is pre-existing debt in a mock test harness, out of PSPR-00 scope; the S104
bind-all-interfaces sits in the `deployment/appliance` tree that PSPR-01/24 will harden.
pytest initially aborted collection (`No module named 'mai'`); the `mai-sdk` package was
reinstalled editable — an environment setup step, not a product change.

### Live-service / infrastructure inventory

See ledger "Infrastructure availability" and `19-infra-inventory.txt`. Summary: Docker +
`openbao/openbao:latest` AVAILABLE; Moto INSTALLABLE; **live ZFS and TPM/PCR UNAVAILABLE**
(Windows host; WSL2 kernel lacks ZFS module). Consequences carried in the ledger for
AF-07A/07B (PSPR-20/29) and AF-06 (PSPR-21).

### Migration / rollback / compatibility / docs

No migrations. No API/behavior change. Rollback = `git checkout HEAD -- <4 files>`. Docs impact:
new ledger + DEVLOG added (this file).

### Residual risk / blocked work / next prompt

- Residual: none from PSPR-00 (doc-only). Ledger records all 24 findings + 2 deferred + 1 quality
  (Q-1 CLOSED).
- Blocked (infrastructure, not code): live-ZFS and TPM live proofs — carried, not resolved.
- **Next prompt:** PSPR-01 (emergency containment of the known-token dev OpenBao).

### Proposed commit scope (NOT executed — awaiting explicit approval)

`docs: restore clippy doc-lazy-continuation gate + add security-remediation ledger/devlog (PSPR-00)`
— stages only: `mai-core/src/cache.rs`, `mai-scheduler/src/scoring/mod.rs`, `mai-api/src/routes.rs`,
`mai-api/src/server.rs`, `docs/sessions/SECURITY-REMEDIATION-LEDGER.md`, this DEVLOG,
`test-evidence/security-remediation/PSPR-00/`. **No commit or push performed.**

---

## PSPR-01 — Emergency containment of the known-token dev OpenBao (AF-12)

**Objective:** Make the shipped/default appliance composition incapable of exposing a dev
OpenBao with a known root token. Containment only; AF-12 stays OPEN until PSPR-24 supplies the
production trust-root build and its live gate.

**Starting HEAD:** `6ffaaee` · **Ending HEAD:** `6ffaaee` (no commit) · Branch `cleanup/artifact-audit`

### Pre-change failing proof

`python deployment/appliance/validate_profile.py --profile production docker-compose.yml`
(original) → **exit 1, 7 violations**: `dev-mode`, `known-token`, `weak-token`,
`host-published-trust` (`8200:8200`), `credential-not-injected` ×2, `weak-credential`
(`WSF_OPENBAO_TOKEN: root`). Evidence: `PSPR-01/01-BEFORE-validate-production.txt`,
`00-BEFORE-effective-compose.txt`.

### Files changed (why)

| File | Change |
|:--|:--|
| `deployment/appliance/docker-compose.yml` | whole stack gated behind `demo` profile; trust core + all app ports bound to `127.0.0.1` only; dev root token + AppRole secret injected via `${VAR:?...}`; private `appliance` network; security header comment |
| `deployment/appliance/validate_profile.py` | **new** signature-based profile validator (`--profile production|demo`), PyYAML-parsed, exits non-zero on any violation |
| `deployment/appliance/fixtures/*.yml` | **new** 6 unsafe + 1 secure regression fixtures (one per rule) |
| `deployment/appliance/tests/{conftest.py,test_validate_profile.py}` | **new** pytest regression (10 cases) — runs in the default tier |
| `deployment/appliance/.env.example` | documents required demo secrets + `--profile demo` bring-up |
| `deployment/appliance/README.md` | bring-up now `cp .env.example .env` + `--profile demo`; OpenBao endpoint loopback-only |

### Verification / negative controls

| Check | Result | Evidence |
|:--|:--|:--|
| validator `--profile demo` on hardened compose | **OK** (0 violations) | `02-AFTER-validate-demo.txt` |
| validator `--profile production` on hardened compose | **REJECT** (dev-mode + host-published-trust) — demo is not production, by design | `03-AFTER-validate-production.txt` |
| `docker compose config` with secrets unset | **error before start** — `required variable OPENBAO_DEV_ROOT_TOKEN is missing` | `04-compose-config-no-secrets.txt` |
| bare `docker compose config --services` (no profile) | **NONE** — bare `up` starts nothing | (05) |
| `--profile demo` render | all 8 services; every host bind `host_ip: 127.0.0.1` (incl. trust port 8200) | `05-AFTER-compose-config-demo.txt` |
| 6 unsafe fixtures | each rejected with its specific rule | `07-pytest-fixtures.txt` |
| secure-production fixture | passes `--profile production` (0 violations) | `07-pytest-fixtures.txt` |
| `pytest deployment/appliance/tests/` | **10 passed** | `07-pytest-fixtures.txt` |
| `ruff check .` | 3 errors, all pre-existing `mock-llm/app.py` (baseline unchanged) | `06-ruff-appliance.txt` |

Live service: Docker 29.6.1 + `docker compose config` used to render/validate. A full
`docker compose --profile demo up` (image build ~10–30 min) and the secure-fixture health check
are deferred to PSPR-24/PSPR-30 (live deployment gate).

### Migration / rollback / compatibility / docs

Operators must now `cp .env.example .env`, set `OPENBAO_DEV_ROOT_TOKEN` + `WSF_OPENBAO_SECRET_ID`,
and run `docker compose --profile demo up`. Documented in README + `.env.example`. Rollback =
`git checkout HEAD -- deployment/appliance/`.

### Residual risk / blocked / next

- **AF-12 stays OPEN** (containment landed; production trust-root build + live gate = PSPR-24).
- Reserved data stores (`postgres`, `objectstore`) keep demo dev passwords, now env-overridable,
  loopback-only, demo-gated — non-trust-plane, out of AF-12 scope; noted for PSPR-24 hardening.
- Runtime rejection of a weak *injected* token (operator sets `OPENBAO_DEV_ROOT_TOKEN=root` in
  `.env`) is a runtime control for the `wsf-seed` binary, not a static-compose check — carried to
  PSPR-24.
- **Next prompt:** PSPR-02 (production AOG fails closed — eliminate implicit shadow mode).

### Proposed commit scope (NOT executed — awaiting explicit approval)

`security(appliance): contain dev OpenBao — profile-gate, loopback-bind, inject secrets + validator (PSPR-01)`
stages: `deployment/appliance/docker-compose.yml`, `validate_profile.py`, `fixtures/*`,
`tests/*`, `.env.example`, `README.md`, `test-evidence/security-remediation/PSPR-01/`.
**No commit or push performed.**

---

## PSPR-02 — Production AOG fails closed; eliminate implicit shadow (AF-13)

**Objective:** Missing or ambiguous configuration must never route regulated content through a
non-blocking policy mode. Root fix landed + unit/regression green; live startup-fail proof owed at
PSPR-28/31.

**Starting HEAD:** `6ffaaee` · **Ending HEAD:** `6ffaaee` (no commit) · Branch `cleanup/artifact-audit`

### Pre-change static proof

Three silent-shadow vectors (workspace grep confirms only these 3 files reference the mode):

1. `main.rs:62` — `let mode_str = env_or("AOG_MODE", "shadow");` (unset → shadow).
2. `policy.rs:32` — `PolicyMode` `#[default] Shadow` (any `default()` → shadow).
3. `app.rs:119` — `AppState::new(..)` hard-codes `mode: PolicyMode::Shadow`.

Any of the three yields non-blocking policy when configuration is absent.

### Files changed (why)

| File | Change |
|:--|:--|
| `policy.rs` | `#[default]` moved Shadow→**Enforce**; new `Profile` enum (default Production), `ModeError`, and testable `resolve_mode(profile, AOG_MODE)`; 7 frozen-matrix unit tests; module doc corrected |
| `app.rs` | `AppState::new` default `mode` Shadow→**Enforce**; new `profile` field (default Production) + `with_profile()` builder |
| `main.rs` | fail-closed profile+mode resolution **first in `run()`** (before OpenBao/bind); banner shows profile+mode |
| `surface_openai.rs` | `/v1/status` readiness now emits `profile` alongside `mode` |
| `tests/policy_modes.rs` | shadow state now explicit `.with_mode(Shadow)` (was relying on the old default) |
| `deployment/appliance/docker-compose.yml` | aog-gateway gets `AOG_PROFILE: development` (its `AOG_MODE: shadow` is now dev-only) |
| `deployment/appliance/.env.example` | documents fail-closed default + `AOG_PROFILE` |

### Fail-closed resolution (the invariant)

`resolve_mode`: absent/blank `AOG_MODE` → **Enforce** in every profile; `shadow`/`report_only`
are **development-only** and explicit — requesting either under `Production` is an error that
fails startup **before any dependency or bind**; unrecognized values error; case-insensitive but
never coerced to shadow. `Profile::parse`: absent/blank → **Production** (fail-safe).

### Verification / negative controls

| Check | Result |
|:--|:--|
| `cargo fmt --check` | PASS |
| `cargo clippy -p aog-gateway --all-targets -- -D warnings -A clippy::pedantic` | PASS |
| `cargo test -p aog-gateway` | **PASS** — 46 lib (incl. 7 new) + integration; 11 suites ok, 0 failed |
| `resolve_mode` matrix (absent/blank→enforce; shadow/report rejected in prod, accepted in dev; case-insensitive-never-coerce; unrecognized rejected) | PASS — `test-evidence/.../PSPR-02/05-test-final.txt`, `06-clippy-final2.txt` |
| `policy_modes` runtime (shadow never blocks; enforce blocks PHI→cloud 403) | PASS (unchanged) |
| appliance validator `--profile demo` after `AOG_PROFILE` add | OK |

Evidence: `test-evidence/security-remediation/PSPR-02/`.

### Migration / compatibility (behavior change — this IS the fix)

- A deployment that relied on the **implicit** shadow default now **enforces** (blocks
  classified→cloud). Intended.
- A deployment that set `AOG_MODE=shadow`/`report_only` **without** `AOG_PROFILE=development`
  now **fails to start** with a clear message. Operators add `AOG_PROFILE=development` to keep a
  non-blocking dev posture. Documented in `.env.example`.
- Readiness `/v1/status` now reports `profile` + `mode`.

### Residual risk / blocked / next

- **AF-13 live proof owed:** startup-abort against the real binary + OpenBao, and denied-dispatch
  end-to-end, land in PSPR-28. Logic is unit-proven and resolves before any dependency.
- "Policy backend unavailable" is N/A today (in-process `PolicyEngine::baseline()`); an external
  policy backend would add a readiness probe — noted for PSPR-11/28.
- **Next prompt:** PSPR-03 (authenticate WSF issuance; derive authority server-side).

### Proposed commit scope (NOT executed — awaiting explicit approval)

`security(aog): fail closed — enforce is the default, shadow/report are dev-only + explicit (PSPR-02)`
stages the 7 files above + `test-evidence/security-remediation/PSPR-02/`. **No commit or push performed.**

---

## PSPR-03 — Authenticate WSF issuance (AF-01) — CORE FIXED (live proof owed)

**Step 1:** full WSF route inventory + auth-classification matrix at
`test-evidence/security-remediation/PSPR-03/route-inventory.md`. Confirmed against
`crates/wsf-api/src/lib.rs`: all six privileged routes (`issue`, `attenuate`, `seal`, `unseal`,
`exchange`, `receipts`) are **unauthenticated** — `issue` (225–254) signs a token straight from
caller-authored `tenant_id/subject_id/roles/budget/allowed_models` with no `WsfPrincipal`.

**Core implemented (this session):** new `crates/wsf-api/src/auth.rs` (474 lines) —

- `Authenticator` trait + fail-closed `DenyAllAuthenticator` **default** (an unconfigured trust
  plane mints nothing); `StaticAuthenticator` dev/test seam.
- `WsfPrincipal` (server-side tenant/roles/audience/budget-ceiling/model-allowlist/permissions).
- `derive_issue_authority` — the body may only **narrow**: cross-tenant, role-elevation,
  budget-widening (incl. tool-call cap), model-widening, and a disallowed issuance kind are all
  **Forbidden before signing**. `IssuancePermissions` separates self/delegated/service/admin.
- Per-principal `RateLimiter` (fixed window).

Wired into `lib.rs`: the `issue` handler now authenticates → rate-limits → derives authority →
signs with the **server-derived** `IssueTokenRequest` (never a caller field) → receipts allow;
every rejection receipts a deny (metadata only, no token/credential material). `AppState` gained
`authenticator` + `rate_limiter`; `main.rs` wires the fail-closed default (TODO(basho): production
mTLS authenticator); the SDK (`client.rs`) gained `with_credential` (bearer). `IssueReq` gained an
optional `issuance_kind`.

**Verification:** `cargo clippy -p wsf-api --all-targets -- -D warnings -A clippy::pedantic` PASS;
`cargo test -p wsf-api --lib` **10 passed / 0 failed** — the adversarial matrix (anonymous,
unknown-credential, cross-tenant, role-elevation, budget-widening, model-widening, disallowed-kind)
plus narrowing-success, inherit-ceiling, and rate-limit bounds. `cargo fmt --check` PASS. Evidence:
`test-evidence/security-remediation/PSPR-03/{01-clippy.txt,02-auth-tests.txt}`.

**Still owed (tracked):** (a) extend authentication middleware to the *other* privileged routes —
they are the roots of AF-02/04/05/14 and get authenticated by their owning prompts (PSPR-04/08/09/16)
now that the `Authenticator` seam exists; (b) a concrete production mTLS/workload-identity
authenticator (currently the fail-closed `DenyAllAuthenticator` placeholder); (c) a route-conformance
CI test asserting every privileged route carries policy metadata; (d) `openapi.json` regeneration;
(e) live two-tenant OpenBao black-box proof (PSPR-28). **AF-01 core is CODE-FIXED** (anonymous
minting of arbitrary authority is closed at the issuance root); it stays open in the ledger until the
live proof lands.

### Proposed commit scope (NOT executed — awaiting explicit approval)

`security(wsf): authenticate issuance + server-derived, narrowing-only authority (PSPR-03 core)`
stages `crates/wsf-api/src/{auth.rs,lib.rs,client.rs,main.rs}`,
`crates/wsf-api/tests/live_api.rs`, `test-evidence/security-remediation/PSPR-03/`.
**No commit or push performed.**
