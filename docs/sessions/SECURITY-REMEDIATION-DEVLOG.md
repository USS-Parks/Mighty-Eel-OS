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

Commit: (pending — this change set).

## Phase A — WSF authentication and issuance authorization

Closes M1's Phase A (A1–A5) and the structural half of **AF-002** (public WSF
route issued signed tokens for caller-selected subjects/roles). Live gates run
against a source-built OpenBao dev server (registry blobs are egress-blocked in
this environment; see `test-evidence/security-remediation/M1/live-gates/`).

### A1 — Principal contract

`crates/fabric-contracts/src/principal.rs`: `WsfPrincipal`, `AuthStrength`,
`Audience`, `AuthenticatedFacts`. The principal derives `Serialize` (receipts
need it) but **not** `Deserialize`, and a private zero-size witness blocks
external struct literals — the wire layer cannot manufacture a principal.
`establish()` is the sole constructor (called by the A2 authenticator).

Gate: serde tests + two `compile_fail` doctests proving `serde_json::from_str::
<WsfPrincipal>` and a `DeserializeOwned` bound both fail to compile. Commit `97ad579`.

### A2 — Authenticator seam

`crates/wsf-api/src/auth.rs`: `WsfAuthenticator` trait; `WorkloadAuthenticator`
verifies a signed `WorkloadCredential` (`Authorization: Workload <b64-json>`) —
ML-DSA signature over a length-prefixed domain-separated preimage FIRST, then
expiry, audience, optional bound-tenant; `LocalDevAuthenticator` is the explicit
dev principal. `require_principal` middleware (route_layer over `/v1/*`) returns
401/403 before the handler; `/healthz` + `/openapi.json` stay open.

Gate: `tests/auth_gate.rs` (8) — missing→401, malformed→401, forged-sig→401,
tampered-after-signing→401, expired→401, wrong-audience→403, wrong-tenant→403,
valid→principal. Commit `a71bf03`.

### A3 — Derived issuance request

`crates/wsf-api/src/policy.rs` + `issue()` rewrite: `IssueReq` is bounded intent
only (`deny_unknown_fields` — smuggling tenant_id/subject_id/roles → 422).
Tenant + subject come from the principal; roles ⊆ tenant-grantable; models ⊆
allowlist; `authorize_budget` refuses any over-ceiling counter (omitted ⇒
ceiling, never unlimited).

Gate: `tests/issue_authz.rs` (live) — issued token bound to the principal's
tenant; smuggled tenant/roles → 422; ungranted role / over-ceiling budget → 403.
Plus lib + policy unit tests. Commit `1a87685`.

### A4 — Issuance permission model

`IssuanceMode {SelfService, ServiceToService, Administrative}` classified from
principal kind + requested roles; `permitted_modes` + `max_delegation_depth`
gate the request; **every allow and deny** is receipted to the ledger (source
`wsf-issuance`, metadata only).

Gate: `tests/issuance_perms.rs` (5, offline — denials never dial the bridge):
each refusal returns 403 AND lands a mode-labeled deny receipt; live test
asserts the allow receipt + ≥2 deny receipts. Commit `4dd56d1`.

### A5 — Live issuance gate

`tests/issue_authz.rs::two_tenants_two_workloads_against_live_openbao`:
authenticated issuance via the real `WorkloadAuthenticator` with **two workload
identities in two tenants**. Each token binds to its own tenant; cross-tenant is
structurally impossible (the authority-signed credential is the only tenant
source); role-escalation → 403 + deny receipt; no credential → 401.

Gate: correct identity succeeds; cross-tenant + escalation fail and are
receipted. PASS against live OpenBao. Commit `c509219`.

## Phase T — token primitive and attenuation repair

Closes **AF-001** (attenuation signed attacker-constructed children without
authenticating or fully constraining the parent — a signer oracle).

### T1 — VerificationContext

`fabric-token`: `VerificationContext` + `verify_in_context` check revocation,
signature under the issuer key, expiry, not-before, tenant, and bundle version
in one required-fields call, so a privileged call site cannot omit a check.
`Operation` records intent for receipts.

### T2 — TokenRestrictions

Attenuation input is restriction-only (`deny_unknown_fields`): subset/lower/
earlier axes + a child id. The child's identity/authority is copied server-side
from the authenticated parent, so no attacker-suppliable child field exists. The
WSF `/v1/tokens/attenuate` request and Rust SDK now carry `{parent, restrictions}`.

### T3 — Parent authentication (the AF-001 fix)

`attenuate()` runs `verify_in_context(parent)` before constructing any child.
Unsigned, wrong-key, expired, not-yet-valid, revoked, wrong-tenant, and
stale-bundle parents fail closed. `attenuate_preverified()` shares the narrowing
for callers that authenticate the parent at their own boundary (aog-apiserver
admission), with a doc warning against misuse.

### T4 — Complete monotonicity

Subset/equality on routes, models, roles, compliance scopes, classification,
budget (fits remaining), expiry; offline can only turn on; child id non-empty
and ≠ parent (no trivial cycle/dup); per-hop depth budget refuses at zero.

Gates:
- `fabric-token` unit/property suite — 26 tests: issue/verify, the full
  `verify_in_context` matrix, a widening per axis, id/depth/offline, and the
  preverified path. The AF-001/AF-006 regression fixtures flipped from asserting
  the vulnerable behavior to asserting rejection and moved into the product suite
  (feature gate retired). Commit `aee70e1`.
- **T7 live gate** — `wsf-api/tests/attenuate_live.rs` against live OpenBao:
  issue→attenuate→verify succeeds and the child inherits the parent's tenant;
  a tampered parent and an attacker-signed parent are refused 403; a widening
  restriction is refused 422. AF-001 closed black-box, not only in unit tests.
  Commit `bfdfa6a`.

### T5 — Atomic budget lineage

`fabric_token::lineage_key(token)` — the immediate parent for an attenuated
token, its own id for a root — keys the gateway's budget fold + `record_spend`,
so all siblings of a parent draw from one shared atomic counter (the
`LocalSpendLedger`/`LeasedSpendLedger` from X1). Sibling children can no longer
each spend the parent's full remaining. Root tokens are unchanged, so the live
`kill_switch` gate stays green. Gate: `tests/budget_lineage.rs` — two siblings
share one counter; 8 concurrent siblings land every unit on one counter with the
ceiling holding. Honest bound: one level of siblings shares with their parent;
full deep-subtree accounting against the lineage root needs the Phase-L chain.
Commit `f68f95b`.

### T6 — Compatibility and migration

`VerificationContext::require_current_bundle(current)` classifies any other
bundle as legacy (v1); `permit_legacy_verify()` opens a bounded verify-only
window. Production denies legacy by default (`UnsupportedTokenVersion`); a legacy
token is never an attenuation parent (`LegacyAttenuationDenied`), even under the
migration flag; with no policy set, behavior is unchanged (back-compat). Wired
into `/v1/tokens/attenuate` via the bridge's `bundle_version()`. Gates:
`tests/token_versioning.rs` (4) + a live legacy-parent-refused case in
`attenuate_live.rs`. Commit `1543b93`.

**Phase T complete (T1–T7).** AF-001 PROVEN.
