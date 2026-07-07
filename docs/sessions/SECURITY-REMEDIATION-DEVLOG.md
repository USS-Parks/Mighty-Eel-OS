# Security Remediation DEVLOG

Initiative: MAI / WSF / AOG security remediation (Critical/High trust-boundary closure).
Repository: im-mighty-eel-mai. Worktree: `mai-worktrees/mai-SEC-1`, branch `session/SEC-1`.
Baseline: `6ffaaee` (`cleanup/artifact-audit`) — the 2026-07-05 audit commit.
Plan of record: [../scans/SECURITY-REMEDIATION-PSPR.md](../scans/SECURITY-REMEDIATION-PSPR.md).
Finding register: [../scans/SECURITY-REMEDIATION-FINDINGS.md](../scans/SECURITY-REMEDIATION-FINDINGS.md).

Each entry records objective, evidence, verify result, and commit SHA per the plan's
Appendix C. Red is reported as red; a skipped step is reported as skipped.

---

## Phase 0 — Containment and evidence freeze

### 0.1 — Remediation lane artifacts + baseline freeze

Objective: stand up the lane (plan tracked in-repo, DEVLOG, finding register, evidence
contract) and freeze a reproducible baseline — HEAD, worktree status, toolchain, gate results.

Environment (win32; `mai-SEC-1` worktree):
- rustc / cargo 1.96.1, rustfmt 1.9.0, clippy 0.1.96
- cargo-audit 0.22.1, cargo-deny 0.19.7
- Python 3.14.4, ruff 0.15.14, mypy 2.1.0, pytest 9.0.3
- gitleaks 8.30.1, detect-secrets 1.5.0
- Docker 29.6.1, Docker Compose v5.3.0 (live-gate infrastructure available)

Baseline HEAD `6ffaaee`; worktree clean at capture (fresh from baseline).

Baseline verify ladder — raw cargo, evidence under
`test-evidence/security-remediation/M0/baseline/`:
- `cargo fmt --check` ................................... PASS (exit 0)
- `cargo check --workspace` ............................. PASS (exit 0)
- `cargo clippy --workspace -- -D warnings -A pedantic`   FAIL (exit 101) — AQ-001:
  `clippy::doc_lazy_continuation`, "doc list item without indentation" at
  `mai-core/src/cache.rs:109`. Reproduced, not introduced; owned by Q1.
- `cargo test --workspace` ............................. PASS (exit 0; 0 failed, all suites)

Not exercised this prompt (captured when their phases open): `cargo audit`, `cargo deny`,
Python gates (ruff / mypy / pytest). AQ-002 (Python) is expected-red per the audit; owned
by Phase Q.

Artifacts:
- `docs/scans/SECURITY-REMEDIATION-PSPR.md` — canonical plan, tracked into SEC-1
- `docs/scans/SECURITY-REMEDIATION-FINDINGS.md` — finding register
- `docs/sessions/SECURITY-REMEDIATION-DEVLOG.md` — this log
- `test-evidence/security-remediation/{M0..M4}/` + `README.md` — evidence contract
- `docs/INDEX.md` — remediation-lane pointer added to the build-state block

Crate to finding map recorded for the trust-boundary phases:
- AF-001 `crates/fabric-token` (`fn attenuate`) + `crates/wsf-api` (attenuation route)
- AF-002 `crates/wsf-api` + `crates/fabric-identity` + `crates/wsf-bridge`
- AF-003 `crates/fabric-envelope` + `crates/wsf-seal`
- AF-004 `crates/wsf-broker`
- AF-005 `mai-vault`
- AF-006 `crates/fabric-revocation`
- AF-007 `crates/wsf-ledger`

Verify: fmt clean; check green; clippy red on the pre-existing AQ-001 only; tests green.
Commit: `21efec1`.

### 0.2 — Emergency WSF exposure containment

Objective: stop unauthenticated host access to the privileged WSF trust plane
before the real authentication fix (Phase A) — contain by network exposure, not
by new auth logic.

Found: the code default and all three deployment composes exposed the privileged
plane. `crates/wsf-api/src/main.rs` defaulted `WSF_LISTEN` to `0.0.0.0:8300`;
`deployment/wsf-ha` (production/HA) host-published wsf-api (8300) and openbao
(8200); `deployment/appliance` + `deployment/shadow` (dev-mode root-token demos)
published wsf-api / gateway / console / openbao on 0.0.0.0.

Changed:
- main.rs default bind 0.0.0.0:8300 -> 127.0.0.1:8300 (production fail-safe;
  explicit WSF_LISTEN widens it behind an ingress).
- wsf-ha: removed the wsf-api + openbao host ports; wsf-api sets
  WSF_LISTEN=0.0.0.0:8300 to bind the internal compose network for the LB only.
- appliance + shadow: all host ports loopback-bound (127.0.0.1); headers marked
  insecure opt-in demos.

Verify: fmt PASS; cargo check --workspace PASS; clippy -p wsf-api PASS (no new
lint; AQ-001 in mai-core still owned by Q1); test -p wsf-api PASS (0 crate-local
tests; workspace suite was green at baseline, re-run at the M0 close). YAML valid
for all three composes. Evidence: test-evidence/security-remediation/M0/containment/.

Gate: an unauthenticated host request cannot reach token issue/attenuate, seal/
unseal, credential exchange, or receipts in the production/HA posture — those
routes are no longer host-published (static + config proof). The live black-box
proof rides the Phase-A ingress gate (A5), once the authenticated ingress exists
and images are built.

Findings: AF-001/002/003/004/006/007 -> CONTAINED (network exposure removed;
root fixes land in their phases). AF-005 untouched (its contain step is 0.5).
AS-001 (floating images) not addressed here — owned by Q7.

Commit: `6daa146`.

### 0.3 — Route inventory + privilege matrix

Objective: a machine-readable inventory of every WSF/AOG/MAI entry point with its
auth/tenant/audit posture, plus a gate that fails when a new privileged route has
no policy row.

Surface (read-only enumeration cross-checked against the WSF router read): 79
production HTTP routes — wsf-api (9, the unauthenticated privileged plane),
aog-gateway (10, per-handler virtual-key `authorize()`), mai-api (60, global
`X-IM-Auth-Token` middleware + per-route `check_permission`; health/metrics
exempt) — plus ~20 mai-api gRPC methods, SSE (`/v1/chat/completions` stream),
WebSocket (`/v1/ws`, post-upgrade auth), and local CLI (mai-admin, mai-api
validate, wsf-seed).

Deliverables:
- `.integrity/route-policy.tsv` — 79-row machine-readable HTTP policy file,
  derived from the live route extraction so it is provably complete.
- `.integrity/scripts/route-policy-check.sh` — perl-based, multi-line aware,
  tests/benches/mocks excluded; asserts every source route has a policy row.
- `.integrity/hooks/pre-push` — invokes the gate next to the no-slop full scan.
- `docs/scans/SECURITY-ROUTE-INVENTORY.md` — human inventory + privilege matrix
  (HTTP/gRPC/SSE/WS/CLI), with the WSF no-auth findings and the
  `/v1/auth/exchange_token` stub + `/v1/ws` post-upgrade flags called out.

Verify: gate exit 0 on the current tree (79/79 declared). Negative control:
dropping the `/v1/tokens/attenuate` row makes the gate exit 1 and name the
undeclared route; restoring returns exit 0. Evidence:
`test-evidence/security-remediation/M0/route-inventory/`.

Notes: the gate is wired into GitHub Actions (`.github/workflows/ci.yml`,
config-check job) and the pre-push hook. The automated gate covers axum HTTP route
literals; gRPC/SSE/WS/CLI are inventoried but not yet auto-gated (F-phase).

Correction (2026-07-06): the original 0.3 wrap wrongly claimed no CI exists —
`.github/workflows/` holds 7 workflows (ci.yml, commit-msg-check, ship-validation,
supply-chain, gpu-release, lamprey-validation, pages) both at the `6ffaaee`
baseline and on main, including a live-OpenBao + Moto `wsf-live` gate. The error
came from a `Glob('.github/**/*')` that does not list dot-directories, asserted
without cross-checking `git ls-tree` / `ls` / `gh`. The route gate is now wired
into ci.yml (this commit) in addition to pre-push.

Commit: `edc9021`.

### 0.4 — Adversarial regression fixtures

Objective: freeze a deterministic regression identifier per finding, with a
quarantined harness asserting current vulnerable behavior (product tests flip to
repaired behavior in-phase). No `#[ignore]` (§0.5).

Reading fabric-token pinned AF-001 exactly: `attenuate()` checks monotonicity on
routes/models/classification/budget/expiry but never verifies the parent
signature and never constrains tenant_id, roles, service_identity, or
revocation_status. So a fabricated/unsigned/wrong-key parent yields a signed
child (signer oracle), a valid parent can be widened on roles/tenant, and a
revoked parent still attenuates (AF-006).

Delivered:
- `crates/fabric-token/Cargo.toml`: `security-regression` feature (off by default).
- `crates/fabric-token/tests/security_regression.rs`: 5 feature-gated fixtures
  (REG-AF-001 unsigned / wrong-key / role-widening / tenant-swap, REG-AF-006
  revoked-parent) asserting the current vulnerable behavior.
- `docs/scans/SECURITY-REGRESSION-REGISTRY.md`: deterministic id per finding
  (AF-001..007); AF-001 + AF-006 implemented, AF-002/003/004/007 reserved for A/E/B/L.

Verify: fmt PASS; harness `--features security-regression` PASS (5 fixtures);
default `cargo test -p fabric-token` runs 0 of them (quarantined); clippy PASS.
Evidence: `test-evidence/security-remediation/M0/adversarial-fixtures/`.

Gate: every AF finding has a deterministic regression identifier (registry).

Commit: `f678e11`.

---

## Phase T — Token primitive and attenuation repair (AF-001 Critical)

### T1–T4 — VerificationContext + parent-authenticated, fully-monotonic attenuation

Objective: close the attenuation signer-oracle and identity-widening (AF-001) at
the `fabric-token` primitive, and introduce the `VerificationContext` (T1) that
Phase R will consume across every privileged path.

Root cause (confirmed reading `fabric-token/src/lib.rs`): `attenuate(parent,
child, signer)` trusted the caller's `parent` without verifying its signature — a
signer oracle: a fabricated or attacker-key parent minted a valid child — and
constrained only routes/models/classification/budget/expiry, leaving tenant,
subject, roles, service identity, scopes, locale, offline_mode, and revocation
unchecked (a child could swap tenant, add roles, or descend from a revoked parent).

Contract (T1/T2):
- `fabric-contracts`: `Attenuation` gains a bounded lineage `depth`
  (`skip_serializing_if` when 0, so every pre-existing root-token signature is
  byte-identical and still verifies — proven by the unchanged issue/verify tests).
- `fabric-token`: new `VerificationContext { verifier, public_key, now,
  revocation }` + `verify_in_context` (signature + expiry + signed-snapshot
  revocation by token/subject/signing-key/bundle; the snapshot is itself verified
  under the anchor so a substituted/forged snapshot fails closed). Signature-only
  `verify` stays as the low-level primitive.

Attenuation (T3/T4) — `attenuate(parent, child, ctx, signer)` now:
1. authenticates the parent — reject on-token/snapshot revocation, verify the
   anchor signature (`ctx.public_key` may differ from the child `signer`: a kernel
   re-anchors a WSF-issued parent), reject if expired;
2. enforces monotonicity on EVERY axis — identity (tenant/issuer/bundle/subject/
   service/identity_id/country/person) equal, sets (roles/scopes/routes/models)
   subset, scalars (classification/budget/expiry) narrowing, `offline_mode` a
   one-way restriction;
3. bounds the lineage — parent_id bound, `depth+1 ≤ MAX_ATTENUATION_DEPTH` (16),
   no self-cycle.

Consumers migrated (one commit, tree always green — §0.3 contract-then-consumers):
- `wsf-api` `/v1/tokens/attenuate`: builds the ctx from the bridge anchor pubkey.
- `aog-apiserver` mutate stage (K8): `Sealer` gains the WSF anchor pubkey, wired in
  `AppState::from_raft` from the front-door authenticator (the ~20 `Sealer::generate`
  call sites are unchanged); `mint_child`/`scoped_child_token` authenticate the
  parent under it before minting. Added `Authenticator::token_public_key()`.
- Five `Attenuation { .. }` root-token literals (wsf-broker, aog-controller vkeys +
  scheduler, aog-node edge, a broker live test) gained `depth: 0`.

Regression (0.4 → repaired): the 5 fixtures flipped from asserting the vuln to
asserting rejection and left quarantine — they run in the default product suite
(the `security-regression` feature is removed). REG-AF-001-{unsigned,wrong-key} →
`ParentUnverified`; -role-widening → `AttenuationWidens{roles}`; -tenant-swap →
`AttenuationWidens{tenant_id}`; REG-AF-006-revoked-parent → `ParentRevoked`.

Verify (this Linux CI container; protoc installed):
- `cargo fmt --check` .................................. PASS
- `cargo check --workspace --all-targets` ............. PASS (exit 0 — all targets)
- `cargo clippy --workspace -- -D warnings -A pedantic`  PASS (exit 0)
- `cargo test -p fabric-token` ........................ PASS (5 regression + 8 unit
  + 4 spend)
- `cargo test -p aog-apiserver --lib`/`--test seal` ... PASS (3 + 2; the seal test
  drives mint_child end-to-end through admission)
- `cargo test -p {fabric-contracts,wsf-broker(16),aog-node(32),wsf-api,
  aog-controller --lib(50)}` .......................... PASS
- Note: `cargo test --workspace` exhausts this container's disk while compiling all
  ~40 crates' test binaries (rustc-LLVM ENOSPC, not a test failure); the tests that
  ran before it filled had zero failures. The full suite is CI-gated (more disk).

Findings: AF-001 CONTAINED → **FIXED** (root controls + focused/property proof; the
live T7 OpenBao black-box gate is deferred → PROVEN, needs the live lane). AF-006:
its attenuate-path leg is FIXED here; full consumer integration (snapshot consumed
on issue/verify/seal/unseal/broker/ledger paths) is Phase R.

Deferred (honest): T5 atomic budget lineage (sibling double-spend) is owned with
`fabric-token::spend`; T6 v1-token migration semantics and T7 live attenuation gate
ride the OpenBao lane. `VerificationContext` is in place for R to consume.

Evidence: `test-evidence/security-remediation/M1/phase-T/`.

Commit: (this change set).
