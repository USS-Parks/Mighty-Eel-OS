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

Commit: `fb92641`.

---

## Phase A — WSF authentication and issuance authorization (AF-002)

### A1–A3 — WsfPrincipal + authenticator seam + principal-derived issuance

Objective: stop `/v1/tokens/issue` from minting signed tokens for caller-selected
tenant / subject / roles. Identity must come from a verified principal, never the
request body.

Root cause (confirmed reading `wsf-api/src/lib.rs`): the `issue` handler took
`IssueReq { tenant_id, subject_id, roles, .. }` straight from request JSON and
passed it to `bridge.issue_token()` with no caller authentication — any reachable
client minted a token for any tenant/subject with any roles.

Contract (A1):
- `fabric-contracts`: new `WsfPrincipal` (tenant, subject, service identity, roles,
  audience, auth method, credential id, correlation id) — the server-created
  authenticated principal. It is the sole issuance identity authority.

Authenticator seam (A2) — new `wsf-api::auth`:
- `WsfAuthenticator` trait → `Result<WsfPrincipal, AuthError>`.
- `SignedIdentityAuthenticator` (production): verifies a presented signed
  `fabric_contracts::Identity` assertion (ML-DSA, via `fabric-identity`) under the
  identity anchor, checks expiry, and maps it to a principal — **roles come from
  server-side policy (`with_role_grant`), never the caller**. Reuses the existing
  signed-identity contract rather than inventing a new assertion.
- `DevAuthenticator` (explicit local-dev opt-in) and `DenyAllAuthenticator` (the
  fail-closed default). Production `main.rs` requires `WSF_IDENTITY_ANCHOR_PK`
  unless `WSF_DEV_AUTH` is explicitly set — no authenticator ⇒ startup fails.
- `require_principal` axum middleware wraps `/v1/tokens/issue`: missing / malformed
  / unverifiable / expired identity ⇒ 401; verified-but-not-permitted ⇒ 403,
  before the handler runs.

Issuance (A3):
- `IssueReq` loses `tenant_id` / `subject_id` / `roles`; it now carries only
  narrowing intent (`allowed_models`, `budget`). The handler builds the
  `IssueTokenRequest` from `principal.{tenant_id, subject_id, roles}`.
- SDK `WsfClient::with_identity(..)` attaches the `x-wsf-identity` header.

Verify (this Linux container; offline — no live OpenBao needed for the gate):
- `cargo fmt --check` PASS; `cargo check --workspace` PASS (exit 0);
  `cargo clippy -p wsf-api -p fabric-contracts --all-targets -D warnings -A pedantic` PASS.
- `cargo test -p fabric-contracts` PASS (5).
- `cargo test -p wsf-api` PASS: auth unit tests (6 — signed-identity → principal
  with policy roles, missing/wrong-key/expired identity rejected, unknown identity
  → no roles, deny-all); `auth_gate` integration (2 — unauthenticated issue → 401
  before the bridge; a verified principal passes the gate → 502 at the dummy
  bridge, i.e. past the boundary); live_api compiles + skips without OpenBao.

Regression: REG-AF-002-caller-subject flips to REPAIRED — proven by
`wsf-api/tests/auth_gate.rs` (401 for the unauthenticated caller; principal-derived
identity).

Findings: AF-002 CONTAINED → **FIXED** (root controls + offline gate proof; the
live A5 two-tenant OpenBao gate is deferred → PROVEN).

Deferred (honest): A4 fine-grained issuance-permission matrix (self/service/admin,
delegation depth) beyond role-grant policy; A5 live two-tenant OpenBao issuance
gate; production mTLS peer-identity binding (the signed-assertion seam is the
equally-strong pluggable authenticator §2.1 allows). The `openapi.json` issue
schema is reconciled in Q8.

Evidence: `test-evidence/security-remediation/M1/phase-A/`.

Commit: `9c63a9e`.

---

## Phase E — Tenant-bound envelope security (AF-003)

### E1/E3/E4 — envelope binding in AAD + signed thread; unseal authorization

Objective: stop any valid token from unsealing any tenant's envelope. Bind the
envelope to its owning tenant/subject and enforce that binding on unseal.

Root cause (confirmed reading `wsf-seal` + `fabric-envelope`): the AAD bound only
the handling `Label` (classification/scopes/ops/destinations); `unseal` checked
signature + expiry + classification clearance + permitted_ops but **never the
tenant/owner** — and a single Transit key wrapped every tenant's data keys. Any
valid, sufficiently-cleared token could open any tenant's envelope.

Binding (E1):
- `fabric-contracts::Thread` gains `tenant_id` / `owner_subject_hash` / `audience`
  (`skip_serializing_if` empty ⇒ a v1 envelope's bytes are unchanged).
- `fabric-envelope`: `EnvelopeBinding`, `ThreadSpec.binding`; the AEAD **AAD now
  binds the label AND the binding** (`envelope_aad`), and the binding is signed
  into the provenance thread (`thread_content`). `envelope_binding()` reads it back.

Seal authorization (E3):
- `wsf-seal::seal` sets the binding from the authorizing token —
  `tenant_id = token.tenant_id`, `owner_subject_hash = token.subject_hash`.

Unseal authorization (E4):
- `wsf-seal::unseal` refuses, **before any Transit decrypt** and with a receipt: an
  unbound (v1) envelope (no silent v1 acceptance, E5), a tenant mismatch, or an
  owner mismatch. Cross-tenant and cross-owner unseal both fail closed.

Verify (this Linux container; offline — the binding check precedes OpenBao):
- `cargo fmt --check` PASS; `cargo check --workspace` PASS (exit 0);
  `cargo clippy -p fabric-contracts -p fabric-envelope -p wsf-seal -p aog-gateway --all-targets -D warnings -A pedantic` PASS.
- `cargo test -p fabric-envelope` PASS (6 — incl. new
  `tampering_the_tenant_binding_breaks_the_thread`: rebinding after sealing →
  InvalidSignature, so the binding is signed).
- `cargo test -p wsf-seal` PASS (inline 3 + tenant_binding 4): cross-tenant,
  cross-owner, and unbound-v1 unseal all → `Unauthorized` before Transit; the
  legitimate owner passes the binding and fails only at the dummy OpenBao.

Regression: REG-AF-003-cross-tenant-unseal flips to REPAIRED (wsf-seal
`tenant_binding.rs`).

Findings: AF-003 CONTAINED → **FIXED** (root controls + offline proof).

Deferred (honest): E2 per-tenant Transit key namespace (defence-in-depth so
Transit itself won't unwrap cross-tenant — needs OpenBao policy); E5 offline v1
migration command (unbound v1 is refused now); E6 tenant-scoped storage/receipt
keys; E7 live two-tenant OpenBao Transit gate (→ PROVEN). Audience binding field
is carried but not yet enforced (no token audience field until Phase A/W matures).

Evidence: `test-evidence/security-remediation/M1/phase-E/`.

Commit: `7dd5f64`.

---

## Phase B — Cloud credential broker confinement (AF-004)

### B1–B3 — named grant contract + server-side grant policy + AWS least privilege

Objective: stop the credential broker from assuming a caller-chosen role. The
caller names a tenant-scoped grant; the broker resolves it to an approved role,
actions, resources, region, and TTL server-side.

Root cause (confirmed reading `wsf-broker`): `broker_credentials(token, .., role_arn, ..)`
verified the token, then assumed **whatever role ARN the caller named** (only the
session-policy *resources* were scoped, never the role itself). The exchange DTO
carried a raw `role_arn`.

Contract (B1): `wsf-api` `ExchangeReq.role_arn` → `grant_id` (a tenant-scoped
named grant). The public API cannot submit a raw cloud identity.

Grant policy (B2/B3):
- `wsf-broker::GrantMapping` (tenant, approved role ARN, allowed actions, resource
  prefixes, region, max TTL) + `GrantPolicy` (named grants; empty ⇒ fail closed).
  `AwsStsBroker::with_grants`.
- `broker_credentials(.., grant_id, ..)`: after the token verifies, `resolve()`s
  the grant — **unknown or cross-tenant ⇒ `GrantDenied` before any AWS/OpenBao
  call**. The session policy is the grant's actions on its resources, narrowed by
  the token's resource caveats; the duration is capped by the grant's max TTL; the
  assumed role is the grant's, not the caller's.

Verify (offline — grant resolution precedes OpenBao/STS):
- `cargo fmt --check` PASS; `cargo check --workspace` PASS (exit 0);
  `cargo clippy -p wsf-broker -p wsf-api --all-targets -D warnings -A pedantic` PASS.
- `cargo test -p wsf-broker` PASS (18): `unknown_grant_is_denied` +
  `cross_tenant_grant_is_denied` (→ `GrantDenied` before AWS);
  `session_policy_scopes_to_the_grant`, `token_caveat_narrows_the_grant`,
  `session_policy_denies_all_when_grant_has_no_resources`; token-reject / expiry
  preserved.

Regression: REG-AF-004-arbitrary-role flips to REPAIRED.

Findings: AF-004 CONTAINED → **FIXED** (root controls + offline proof).

Deferred (honest): B2 signed/OpenBao-custodied grant loading (the broker binary
starts with an empty policy ⇒ every exchange fails closed until grants are wired);
B4 GCP/Azure named-grant parity (their brokers are unchanged this phase); B5
credential zeroization audit; live B6 Moto/GCP/Azure gate (→ PROVEN). The
`live_localstack` + `live_api` gates were migrated to the grant model.

Evidence: `test-evidence/security-remediation/M1/phase-B/`.

Commit: `c0c95a4`.

---

## Phase L — Receipt ledger authorization and integrity (AF-007)

### L1/L2 — authenticated, tenant-scoped receipt query

Objective: stop the receipt ledger from serving cross-tenant metadata to any
caller. Authenticate the query and enforce a mandatory tenant predicate.

Root cause (confirmed reading `wsf-api` + `wsf-ledger`): `/v1/receipts` was
unauthenticated, accepted an arbitrary `field=/value=` query (an enumeration
oracle), and returned **all** entries with no tenant filter.

Query model (L1/L2):
- `wsf-ledger`: `query_tenant(tenant, token_id?, limit)` — only entries whose
  receipt carries `tenant_id == tenant` (a receipt with no `tenant_id` is never
  returned to a tenant query; no existence oracle); `query_global(token_id?,
  limit)` for the audited global-auditor. Both paged.
- `wsf-seal::SealReceipt` gains `tenant_id` (from the presenting token) so seal
  receipts are tenant-attributable; the bridge's `AuditCorrelation` already carries
  it.
- `wsf-api`: `/v1/receipts` is gated by `require_principal`; `ReceiptsQuery` is
  typed (`token_id`, `limit` — capped at 1000), the tenant predicate comes from the
  principal, and the `global-auditor` role is the only cross-tenant path. SDK
  `receipts(token_id, limit)` attaches the identity header.

Verify (offline):
- `cargo fmt --check` PASS; `cargo check --workspace` PASS (exit 0);
  `cargo clippy -p wsf-ledger -p wsf-seal -p wsf-api --all-targets -D warnings -A pedantic` PASS;
  route gate OK.
- `cargo test -p wsf-ledger` PASS (5 — incl. `tenant_scoped_query_isolates_tenants`:
  cross-tenant hidden, untenanted receipt hidden, no oracle, limit caps, global sees all).
- `cargo test -p wsf-seal` PASS (tenant_binding + inline, with SealReceipt.tenant_id).
- `cargo test -p wsf-api` PASS: `auth_gate` now 4 — unauthenticated `/v1/receipts`
  → 401; a tenant-a principal sees only tenant-a's receipts through the HTTP surface.

Regression: REG-AF-007-unfiltered-receipts flips to REPAIRED.

Findings: AF-007 CONTAINED → **FIXED** (root controls + offline proof).

Deferred (honest): L3 persistent HA ledger (production still uses the in-process
ledger; restart/replica continuity is the persistence prompt); live L4 two-tenant
ingest/query/export gate (→ PROVEN).

Evidence: `test-evidence/security-remediation/M1/phase-L/`.

Commit: `5987c53`.

---

## Phase R — Revocation and trust freshness (AF-006)

### R1 store + R3 consumer integration (seal, broker)

Objective: make the WSF privileged consumers actually consult signed revocation.
Phase T added the `VerificationContext` + `verify_in_context` mechanism (which
checks a snapshot by token/subject/signing-key/bundle); R adds the store and wires
the consumers.

Root cause (confirmed): `wsf-seal` and `wsf-broker` verified only the signature +
on-token `revocation_status`, never a signed snapshot — a revoked-by-snapshot
token still sealed data and brokered credentials. (The AOG kernel `auth.rs` already
consumed revocation; the gap was the WSF trust-plane services.)

Store (R1) — `fabric-revocation::RevocationStore`:
- Anti-rollback, monotonic install: a new snapshot must verify under the anchor,
  be unexpired, and be strictly newer by `issued_at` (an emergency snapshot may
  replace at an equal timestamp). Any failure keeps the last-known-good snapshot —
  a stale, expired, or forged update cannot blind the store. New errors
  `BadTimestamp` / `Expired` / `Rollback`.

Consumer integration (R3):
- `wsf-seal::SealService.with_revocation(snapshot)`; `verify_token` now uses
  `verify_in_context` — a snapshot-revoked token is refused (and receipted) on both
  seal and unseal, before any Transit call.
- `wsf-broker`: the shared `verify_token` takes the snapshot and uses
  `verify_in_context`; `AwsStsBroker.with_revocation(snapshot)` supplies it — a
  revoked token is refused before any AWS call. GCP/Azure pass `None` for now (their
  wiring lands with the B4 parity prompt; on-token revocation + expiry apply).

Verify (offline):
- `cargo fmt --check` PASS; `cargo check --workspace` PASS (exit 0);
  `cargo clippy -p fabric-revocation -p wsf-seal -p wsf-broker --all-targets -D warnings -A pedantic` PASS.
- `cargo test -p fabric-revocation` PASS (store: install → newer replaces → older is
  Rollback → newer-but-expired is Expired → forged is InvalidSignature, last-known-good kept).
- `cargo test -p wsf-seal` PASS (snapshot_revoked_token_is_refused_at_seal).
- `cargo test -p wsf-broker` PASS (19 — incl. snapshot_revoked_token_is_refused).

Regression: REG-AF-006-revoked-parent stays REPAIRED (Phase T); the seal/broker
snapshot-consumption tests extend the coverage to the consumer paths.

Findings: AF-006 CONTAINED → **FIXED** (context + store + seal/broker consumers,
with offline proof).

Deferred (honest): R2 broaden the snapshot predicate (issuer/tenant/lineage
dimensions) beyond token/subject/key/bundle; R3 continued — gateway, tool-proxy,
approval, and GCP/Azure broker consumption via the same `VerificationContext` seam;
R4 emergency network/removable-media propagation + SLO; R5 HA/partition/air-gap
behavior; live R6 revoke-by-every-dimension gate (→ PROVEN). The appliance snapshot
poll into `wsf-api` main is R4 (the mechanism is in place; the store + consumers
accept a snapshot today).

Evidence: `test-evidence/security-remediation/M1/phase-R/`.

Commit: `7b1fe51`.

---

## Phase V — Production vault truth restoration (AF-005)

### V1 backend policy + V8 measured readiness

Objective: stop production readiness from certifying an uninitialized or
plaintext-capable vault.

Root cause (confirmed reading `mai-api`): two holes.
1. `vault_builder::build_vault` **accepted the `FileDev` backend in production** — a
   plaintext-capable filesystem vault (its own doc matrix said `production |
   file-dev | FileDevVault`). Only `Stub` was refused.
2. `server.rs` set the runtime `vault_opened` outcome to an **unconditional
   `RuntimeOutcome::pass`** — it never opened or verified the vault, so
   `PROD-VAULT-100` passed regardless of the real backend/root.

Fix:
- V1: `build_vault` now returns `FileDevInProduction` for a production file-dev
  profile (and the doc matrix is corrected). The config guard `PROD-VAULT-001` also
  rejects `stub | file-dev` as "a plaintext-capable dev backend."
- V8: the runtime `vault_opened` outcome is **measured** — a production stub/file-dev
  backend, or a missing vault root, flips it to `Fail`, so startup fails closed on
  the `PROD-VAULT-100` Critical check. (`mai-ship-validate` already measured via
  `build_vault`; the running server now matches.)

Verify (offline):
- `cargo fmt --check` PASS; `cargo check --workspace` PASS (exit 0);
  `cargo clippy -p mai-api -D warnings -A pedantic` PASS.
- `cargo test -p mai-api --test vault_bootstrap` PASS (10 — incl. new
  `production_rejects_file_dev_backend`).
- `cargo test -p mai-api --test ship_convergence --test production_guard` PASS (6 + 4).

Findings: AF-005 OPEN → **FIXED** for the backend-policy + readiness-truth dimensions
(the two audit holes), with offline proof.

Deferred (honest — these need a live ZFS/TPM environment, the V-phase live gate):
V2 real construction (PQC/TPM/audit engines) + removing `ZfsVault::new` from the
production boot; V3 initialization-before-publication (mount/dataset/key/manifest/PCR
proof); V4 encrypted model storage round-trip; V5 ZFS property proof; V6 snapshot/
rollback; V7 deletion/cryptographic-erasure semantics; V9 restart/migration gate
(AF-005 live closure). `vault_opened` now measures backend + root, not the deeper
encryption/init/key proofs — those flip from pass to a real pass only under V9.

Evidence: `test-evidence/security-remediation/M1/phase-V/`.

Commit: (this change set).
