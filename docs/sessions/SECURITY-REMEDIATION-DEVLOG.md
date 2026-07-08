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

## Phase E — tenant-bound envelope security (core)

Closes **AF-003** (envelope unseal lacked tenant/subject binding — any
sufficiently-cleared token could unseal any tenant's envelope).

- **E1** contract: `EnvelopeBinding {tenant_id, owner_subject_hash, audience,
  envelope_version}` on the `Envelope`, readable pre-decrypt and folded into
  both the AEAD AAD and the provenance thread — swapping any binding field
  breaks decryption and the signature (tested).
- **E3** seal authorization: `wsf-seal` derives the binding from the verified
  token (tenant + subject), never caller-chosen; envelopes are stamped v2.
- **E4** unseal authorization: before any Transit decrypt, unseal denies a
  legacy unbound envelope, a cross-tenant token, or a wrong-audience envelope —
  each receipted.
- **E7** live gate: `wsf-seal/tests/live_seal.rs` adds a cross-tenant case — a
  fully-cleared token from another tenant is refused 403 before unwrap. PASS
  against live OpenBao Transit. Commit `8fcb6ae`.

Remaining in-phase hardening: **E2** per-tenant Transit key namespace, **E5**
offline v1-envelope migration command, **E6** storage/receipt tenant-key binding.

## Phase B — cloud credential broker confinement (core)

Closes **AF-004** (the broker accepted a caller-selected AWS role ARN).

- **B1** contract: `ExchangeReq` carries a tenant-scoped `grant_id`, not a
  `role_arn`, with `deny_unknown_fields` — a smuggled `role_arn` is a 422.
- **B2** server-side grants: `wsf-api/src/grants.rs` (`CloudGrant` + `GrantStore`
  seam; `StaticGrants` for dev/tests). The exchange handler requires the WSF
  audience, requires the presented token's tenant to equal the authenticated
  principal, resolves `(tenant, grant_id)` server-side, and only then brokers
  with the approved ARN. Missing/cross-tenant grant → 403.
- **B6** live gate: `wsf-api/tests/broker_grant.rs` against live OpenBao + Moto
  STS — approved grant brokers scoped creds; raw `role_arn` → 422; unknown grant
  → 403. Commit `9b66c3a`.

The low-level broker primitive still takes an ARN (W2 `live_localstack` exercises
it directly and stays green); the AF-004 fix is at the public API. Remaining
in-phase: **B3** AWS least-privilege scope binding, **B4** GCP/Azure parity,
**B5** credential-lifecycle hygiene.

## Phase L — receipt ledger authorization (core) + E6 receipt binding

Closes **AF-007's core** (the ledger was unauthenticated and not tenant-filtered).

- **L1** authenticated: `/v1/receipts` requires the A2 principal + WSF audience.
- **L2** tenant-scoped: an entry is returned only if its receipt's `tenant_id`
  equals the caller's tenant; an optional typed field filter is always
  intersected with that scope, so cross-tenant identifier guessing returns no
  rows and no existence oracle. Results bounded by `RECEIPTS_LIMIT`. Untenanted
  receipts are withheld (fail closed).
- **E6** receipt binding: `SealReceipt` gains `tenant_id` (stamped from the
  token), so seal/unseal receipts are tenant-filterable like issuance receipts.

Gate: `wsf-api/tests/ledger_authz.rs` (offline, two tenants) — each principal
sees only its own tenant's receipts; a cross-tenant token-id query returns
nothing. W3/W6 live gates green with tenant-stamped receipts. Commit `38b7113`.

Remaining in-phase: **L2** global-auditor role, **L3** persistent HA ledger,
**L4** end-to-end live gate incl. export.

## Phase R — revocation and trust freshness (complete; AF-006 → PROVEN)

Core (earlier commits `76f1ca5`, `116b0f1`): **R2** the complete revocation
predicate — `RevocationSnapshot` gained `revoked_tenants` / `revoked_issuers` /
`revoked_service_identities` and a single `revokes(&token)` covering every
dimension (token id, subject, signing key, issuer, bundle, tenant, service
identity); **R3** the shared verify path (`fabric-token::verify_in_context`
with `with_revocation`).

Hardening (this session):

- **R1** anti-rollback: `MonotonicRevocationStore` (`fabric-revocation`) —
  `advance` adopts a candidate snapshot only if it verifies against the trust
  anchor **and** strictly advances the new `sequence` counter (serialized only
  when non-zero, so pre-R1 signatures keep verifying). A replayed older
  "nothing revoked" view is refused (`RevocationError::Rollback`) and the held
  state stands. Emergency snapshots share the counter, so an out-of-band
  revocation cannot be rolled back by a lagging regular publication.
- **Consumer wiring**: `SealService` and all three brokers (AWS/GCP/Azure) take
  `with_revocation_store(...)`. Once wired they fail closed **before any
  custody or cloud call**: no held snapshot, an expired snapshot, or a snapshot
  revoking the token on any dimension all deny (receipted on the seal side,
  `TokenRejected` on the broker side). Unit gates prove the denial fires with
  unreachable OpenBao/STS endpoints.
- **R6 live gate**: `wsf-api/tests/live_revocation.rs` against live OpenBao +
  Moto — a signed snapshot travels the real KV distribution channel; with a
  clean sequence-1 snapshot engaged, issue/seal/unseal/exchange all succeed;
  after publishing sequence-2 revoking the tenant, **unseal and credential
  exchange both deny (403)** for the still-signature-valid token with no
  restart; replaying the stale clean snapshot is refused (R1) and the denials
  stand. PASS.

**R4/R5 posture** (ops-plane): propagation is poll-driven; the fail-closed
freshness check bounds exposure at snapshot TTL — an appliance that cannot
fetch a fresh snapshot stops honoring privileged ops rather than serving from
a stale view. Signed snapshots verify offline (`fabric-revocation` docs), which
is the air-gap leg; HA distribution (multi-channel publication) is deployment
plumbing tracked for the ops runbook, not a code gap.

## Phase E — hardening complete (E2/E5)

- **E2** per-tenant Transit keys (commit `0a7f383`): the seal service wraps
  each tenant's data keys under `<base>-<tenant>`; unseal unwraps under the
  **binding's** tenant key. Live gate `wsf-seal/tests/live_tenant_keys.rs`
  proves OpenBao itself refuses a cross-tenant unwrap (400) independent of the
  app-layer E4 check.
- **E5** authenticated v1→v2 migration (commit `a22cf63`):
  `fabric-envelope::migrate_legacy` verifies the legacy thread against the
  original sealer key, opens the label-only AAD, and re-seals with the tenant
  binding; idempotent on v2; tampered v1 refused. Offline gates in
  `fabric-envelope/tests/envelope.rs`.

With E6 (receipt binding, logged with Phase L) this completes Phase E:
**AF-003 fully PROVEN**.

## Phase B — hardening complete (B3/B4/B5)

- **B3** least privilege: `build_session_policy(token, allowed_actions)` emits
  the **grant's** approved IAM actions on the token's `ResourcePrefix` caveats
  — never `Action:"*"`. A wildcard action in a misconfigured grant is dropped
  (alone → deny-all); token `ToolAllowlist` caveats intersect (narrow) the
  action set. `GrantScope` binds role ARN + actions + signing-region override +
  `ExternalId` (confused-deputy defense, appended to the AssumeRole form) + a
  TTL ceiling that tightens the STS window and refuses grants below the 900s
  floor before any custody call. `CloudGrant::to_scope()` carries all of it
  from the API's grant store into the broker.
- **B5** credential hygiene: `RootCredentials` zeroize on drop
  (`zeroize::ZeroizeOnDrop`) with fully-redacted `Debug`;
  `TemporaryCredentials` redacts secret access key + session token in `Debug`
  (access key id stays visible — CloudTrail correlation data). Regression
  tests prove `{:?}` output carries no secret material.
- **B4** parity: `GcpCredentials` / `AzureCredentials` redact their bearer
  tokens in `Debug`; GCP/Azure brokers share the same fail-closed
  `verify_token` (including the revocation consult) and TTL clamps.

Gates: 26 wsf-broker unit tests + W2 (`live_localstack` via `GrantScope`) +
B6 (`broker_grant`) + W6 (`live_api`) all green against live OpenBao + Moto.
With B1/B2/B6 this completes Phase B: **AF-004 fully PROVEN**.

## Phase Q — Q5 dependency-policy config drift fixed

`cargo audit` and `cargo deny` read the same RustSec DB but had drifted:
`.cargo/audit.toml` ignored 4 advisories (0144, 0384, 0176, 0177) while
`deny.toml` ignored 5 — the extra being **RUSTSEC-2026-0173**
(`proc-macro-error2` unmaintained, compile-time-only via `validator_derive`).
So `cargo audit` would have **failed** on 0173 while `cargo deny` passed —
exactly the "documented command doesn't match CI" gate inconsistency Q1/Q5
target. Synced the two: `.cargo/audit.toml` now ignores the same 5 advisories
(verified equal by parsing both), added the missing §1.5 to
`docs/compliance/INDEPENDENT-EVIDENCE-DEFERRALS.md` (the audit config's own
rule is "no ignore without a doc entry"), and fixed the stale doc path both
configs cited (`docs/…` → `docs/compliance/…`). Every ignore remains a
specific advisory id with a written rationale and a named revisit lane — no
blanket suppression. Running the tools to confirm no *new* advisory falls
through is the remaining step (the binaries aren't installed here and
compiling them from source risks the tight disk).

## Phase L — hardening complete (L2 auditor / L4 export; AF-007 → PROVEN)

- **L2 remainder — global auditor**: `wsf-api/src/audit.rs` (`AuditorStore` +
  `StaticAuditors`). Enrollment is by authenticated `principal_id`, server-side
  only — nothing in the request can confer it, and the default everywhere is
  `StaticAuditors::none()`. An enrolled auditor is the single exception to
  receipt tenant scoping (still bounded by `RECEIPTS_LIMIT`); everyone else's
  queries stay mandatorily tenant-scoped exactly as before.
- **L4 — signed evidence export**: `GET /v1/receipts/export` (A2-gated +
  auditor-only) returns the ledger's ML-DSA-signed `EvidencePack`, which
  verifies **offline** via `wsf_ledger::verify_pack` with the ledger public key
  alone. Non-auditors get 403. SDK: `WsfClient::export_receipts`. OpenAPI
  updated.
- Gates: offline `ledger_authz.rs` — auditor sees both tenants, plain principal
  stays scoped, exported pack verifies offline, a tampered entry breaks the
  signature, non-auditor export 403. Live `live_api.rs` (W6/L4) — the SDK's
  unenrolled principal is refused 403; the enrolled auditor principal exports
  over live HTTP and the pack verifies offline against the ledger key. PASS.
- **L3 posture** (ops-plane): the in-memory chain + signed offline-verifiable
  export is the evidence path; a durable HA backend is deployment plumbing for
  the ops runbook — the authorization and integrity controls above are
  backend-independent.

With L1/L2 core + E6 receipt binding this completes Phase L: **AF-007 fully
PROVEN**.

## Live re-verification sweep — E2 interaction fixes

Full re-run of every live suite in the changed-code dependency set surfaced
two E2 (per-tenant Transit keys) interactions the per-crate gates had missed:

- **W4 (`wsf-ledger/tests/live_ledger.rs`)**: the test provisioned the bare
  base key and an exact-path Transit policy, so the E2 seal path
  (`<base>-<tenant>`) had no key/permission. Test provisioning updated to the
  per-tenant key + wildcard policy (same shape as the other live suites).
- **R4 ring darkening (real regression, `aog-controller`)**: darkening a
  trust ring disabled only the base ring key, but the seal service now wraps
  under per-tenant derivatives — so a darkened ring's tenant-namespaced
  envelopes **still unsealed**. `TransitAdmin` gains `list_keys` +
  `disable_key_family`, and the ring reconciler darkens the whole key family
  (`<base>` and every `<base>-*`). The R4 live gate now also asserts the
  per-tenant derivative is dead after darkening, and the behavioral proof
  (unseal fails on a dark ring) is green again.

Post-fix state: every crate depending on the phase-B/R/L-changed code passes
its full suite against live OpenBao + Moto (fabric-token, fabric-revocation,
wsf-bridge, wsf-broker, wsf-seal, wsf-api, wsf-cache, wsf-tenants,
wsf-ledger, aog-gateway, aog-controller, aog-apiserver, aog-node,
aog-conformance, aogd). `protoc` was also installed in the dev container,
unblocking `mai-api` builds (pre-existing environment gap, unrelated to the
remediation).

## Phase V — V1/V2/V3 + V8 core (AF-005)

- **V1 backend policy**: `build_vault` production mode accepts only the
  reviewed encrypted backend. The plaintext `file-dev` backend — previously
  accepted in production whenever `vault.root` existed — is refused regardless
  of `allow_stub` or root state, and the `PROD-VAULT-001` static check fails
  on it explicitly.
- **V2 initialized construction**: the ZFS arm no longer hands out a bare
  `ZfsVault::new` (no PQC, no audit writer, nothing awaited). It constructs
  and initializes `PqcEngine` and the PQC-signed `AuditWriter`, creates the
  storage tree, builds `ZfsVault::with_engines`, and — when the new
  `[vault].dataset` profile field names the backing dataset — wires real
  `ZfsOps` so initialization **proves the live dataset's properties** (V5)
  instead of trusting a directory.
- **V3 initialization blocks binding**: `vault.initialize()` is awaited in
  the builder and any failure is `VaultBuildError::InitFailed`, which aborts
  `MaiServer::run` before any socket binds.
- **V8 core — measured readiness**: the unconditional
  `vault_opened = Pass("vault opened")` fabrication is gone. `probe_vault`
  runs a storage round-trip (store → load → byte-compare, unique per-boot id)
  through the live `VaultInterface` and its outcome feeds `PROD-VAULT-100`;
  a missing probe fails closed. `mai_ship_validate` runs the same probe.
  Gate: `vault_bootstrap.rs` proves the probe passes on an initialized vault
  and FAILS on the stub (which the old code would have certified).

## Phase V — V4 sealed storage + V7 cryptographic erasure

- **V4 encrypted model storage**: `ZfsVault::store_model_package` seals
  weights at rest through `PqcProvider::encrypt_model_weights` (ML-KEM-1024 +
  AES-256-GCM, key-derived with the model id as context) when a PQC engine is
  wired — which the V2 builder always does for the ZFS backend. The manifest
  records a `weights_format` (`mlkem1024-aesgcm-v1`); `load_model_weights`
  dispatches on it, decrypting v1 and reading legacy `plaintext-v0` for
  migration. Encrypted weights presented to an engine-less vault **fail
  closed** rather than returning ciphertext. Integrity hashes are computed
  over the stored (cipher) bytes, so verification needs no key. Gates in
  `zfs.rs`: sealed-at-rest (plaintext never appears in the file) + round-trip,
  legacy-plaintext load, engine-less fail-closed.
- **V7 cryptographic erasure**: the former `secure_overwrite_passes` /
  `secure_wipe` model was false on copy-on-write ZFS — overwriting a file
  writes new blocks and the originals persist (indefinitely in any snapshot).
  Replaced by `PqcProvider::crypto_erase_model`, which retires the model's KEM
  key (scrubs + drops the secret); once gone, the at-rest envelope — on disk
  and in every retained snapshot — is permanently undecryptable.
  `ZfsVault::remove_model` calls it and its comment now states the real
  guarantee; `RemoveOptions`/`RemovalResult` and the API `ModelRemoveResponse`
  carry `crypto_erased` instead of `secure_wipe`, with snapshot-retention
  effects documented on the fields. Gate proves retained ciphertext is
  undecryptable after erasure.

## Phase V — V9 key persistence + restart recovery

The blocker under V9's "restart, verify/decrypt" leg was that the per-model
KEM keys `encrypt_model_weights` mints lived only in memory — a process
restart lost them, so V4-sealed weights became undecryptable across exactly
the reboot V9 must survive. Fixed in `PqcEngine`:

- **Persisted, wrapped keys**: on first use the engine loads-or-creates a
  32-byte key-encryption key at `<key_store>/kek.bin`; each per-model KEM
  secret is AES-256-GCM-wrapped under it and written to
  `<key_store>/model-keys/<key_id>.json` (public key + metadata clear, secret
  never plaintext on disk). `ensure_model_keypair` recovers a persisted key
  before minting a new one, and `decrypt_model_weights` lazily loads it on a
  cold cache — so re-sealing reuses the key and a fresh process decrypts
  weights sealed before the restart. Persist failure is an error, not a
  warning (a key that can't durably store wouldn't survive a reboot).
- **Erasure survives restart (V7 completion)**: `crypto_erase_model` now
  deletes the persisted key file as well as the in-memory copy, using the
  deterministic key id so it works whether or not the key was loaded this
  boot. A reborn engine over the same store cannot resurrect an erased key.
- Gates (`pqc.rs`): `v9_model_key_survives_restart` (seal → drop engine →
  fresh engine decrypts) and `v9_crypto_erase_survives_restart` (erase → drop
  → fresh engine cannot decrypt retained ciphertext). Test fixtures moved to
  per-call unique key stores so on-disk state can't leak across tests.

In production the key store sits on the encrypted ZFS dataset (V4/V5) and the
KEK should additionally be TPM-sealed; the dev/no-TPM path keeps it as a
plaintext file on that still-encrypted dataset.

**V9 migration** (the last unimplemented V9 code path):
`ZfsVault::migrate_model_to_encrypted` reads a legacy `plaintext-v0` model,
seals it under the model's KEM key, and rewrites the weights file + manifest
as `mlkem1024-aesgcm-v1`. Idempotent on already-encrypted models; refuses
without a wired engine (never leaves plaintext silently). The doc notes the
CoW caveat: migration seals the *live* copy, but a pre-existing snapshot from
the plaintext era still retains the old blocks — snapshot afresh and retire
the pre-migration ones. Gates: `v9_migrate_legacy_plaintext_to_encrypted`
(seal-in-place + round-trip + idempotent) and `v9_migrate_requires_a_pqc_engine`.

The **V9 live gate** (`mai-vault/tests/live_zfs.rs`, env-gated on
`MAI_ZFS_TEST_DATASET`) now also exercises, on a real dataset: (1) restart
recovery — seal a model, drop the whole vault, bring a fresh one up over the
same dataset + key store, decrypt; and (2) legacy-plaintext migration in
place. It skips cleanly where there is no ZFS.

Remaining for AF-005: **running** the V9 live gate on a real ZFS+TPM host
(this container has neither `zfs` nor `/dev/tpm*`) — every V9 code path is now
implemented, unit-proven, and wired into the ready-to-run live gate; only the
host execution + TPM-sealed-KEK hardening are deferred.

## Phase Q/M3 — quality + supply-chain gates (AQ-001, AQ-002, AS-001)

- **AQ-001** (clippy): the workspace is clean under the CI flags
  (`-D warnings -A clippy::pedantic`, 0 errors); the `doc_lazy_continuation`
  the finding cited at `mai-core/src/cache.rs:109` no longer exists (the
  cacheability doc list is properly continued). No code change needed.
- **AQ-002** (Python gates collect reliably): the SDK uses a `src/` layout,
  so tests importing `mai` only resolved with an editable install — the
  "does not collect" root cause. Added a local `[tool.pytest.ini_options]`
  to `mai-sdk-python` (`pythonpath = ["src"]`, `testpaths`, `asyncio_mode`)
  so the gate is self-contained and rootdir is the SDK dir. Whole-tree ruff
  had 3 findings, all in the appliance mock-llm demo server (two missing
  annotations + an intentional bind-all-interfaces now `# noqa`'d with
  rationale) — fixed. Verified: SDK 179 pytest pass + mypy clean; whole-tree
  ruff clean; whole tree collects 1310 tests with 0 errors once requirements
  are installed.
- **AS-001** (image pinning): every **third-party** base/service image is now
  digest-pinned across the three deployment Dockerfiles
  (`rust`, `debian`, `node`, `nginx`) and the four compose stacks
  (`openbao`, `postgres`, `minio`, `python`); digests resolved 2026-07-07 via
  `docker buildx imagetools inspect` (registry manifest reachable through the
  agent proxy; layer pulls are not, so `RepoDigests` was unavailable). The
  release `Dockerfile` was already digest-pinned. First-party
  `islandmountain/*` / `wsf-api` references stay tagged — they are local
  `build:` outputs, not registry pulls, so a digest can't precede the build.

## Session close (2026-07-07 — paused for owner audit)

State at pause: every finding is FIXED or PROVEN; none OPEN, none yet CLOSED
(CLOSED requires the Phase X independent re-scan). AF-001/002/003/004/006/007
PROVEN with live gates; AF-005 FIXED (V1–V8 + V9 persistence/restart/migration,
all unit-proven and wired into the env-gated ZFS live gate); AQ-001/AQ-002/
AS-001 FIXED. See the register's "Session status" block for the deferral list
(V9 live-host run, `cargo audit`/`gitleaks`/`detect-secrets` executions, Phase
F/X audits) — each blocked on hardware or on independent-review scope, not on
further implementation.

Governance/commit convention reaffirmed at close: the canonical commit footer
is `Authored and reviewed by Basho Parks, copyright 2026` with **no** AI
co-author credit — enforced by `.githooks/commit-msg` (+ `footer-filter.awk`),
mirrored in `.integrity/hooks/`, the PowerShell port, and the
`.github/workflows/commit-msg-check.yml` CI gate. This pass unified the two
stray `Copyright` (capital-C) variants in the `.integrity/` mirror and
`HANDOFF.md` to the lowercase `copyright` canon and loosened the mirror's strip
regex to the `^Authored and reviewed by` prefix so re-stamping stays idempotent.
Branch `claude/live-gates-docker-zfs-qoe2bc`; all work pushed to origin.

## Phase F — deferred high-impact frontier closure

Phase F audits the deferred high-impact shards (F1-F9). Each prompt was reviewed
by an independent read-only auditor against its PSPR gate, then every High/Critical
claim was confirmed against source before any fix. Reachable, bounded,
host-testable findings are fixed with regression tests; feature-scale (full adapter
resource isolation) and hardware-blocked (real ZFS/TPM) items are dispositioned as
tracked deferrals, never fake-closed. Per-prompt dispositions:
`test-evidence/security-remediation/M3/`.

### F5(a) - model-package integrity (DF-01A, DF-01B, F5-NEW-1/2)

Objective: close the model-package trust-boundary findings - arbitrary out-of-vault
write via an unsanitized manifest-derived id (DF-01B), and the manifest-not-
authenticated gap (DF-01A) - plus two correctness fixes.

Confirmed against source (`mai-core/src/models/verify.rs:116`,
`mai-vault/src/{zfs,file_dev}.rs`): `verify_signature` signed only the weights, so
the manifest - and the `model_id = name:version:quant` derived from it - was
unauthenticated; that `model_id` was then joined onto the vault root with no
sanitization, giving arbitrary directory creation + file write outside the vault on
a crafted manifest name.

Changed:
- DF-01B: `mai_core::vault::validate_model_id` rejects path separators, `..`,
  absolute/drive-qualified, empty/over-long, control chars, and non-`[A-Za-z0-9._:-]`
  ids, and requires a single Normal path component. Enforced at the storage boundary
  in both `ZfsVault` and `FileDevVault` on store/load/migrate before any join.
- DF-01A: `verify_package` authenticates the manifest. A v2 package carries
  `manifest.mldsa` (ML-DSA over the canonical `manifest.toml` bytes) and declares
  `security.integrity_hash_tree` equal to the weights hash-tree root; a present-but-
  invalid manifest signature, or a manifest not bound to the signed weights, is a
  hard `verified=false`. A legacy unsigned package stays installable outside strict
  mode (back-compat) but reports `manifest_authenticated=false`;
  `ModelRegistry::with_signed_manifest_required(true)` (production) refuses it at the
  install boundary. `pkg-builder --manifest-signature` emits v2 and gates the
  manifest-to-weights binding at build time.
- F5-NEW-1: model install/remove audit entries built with `serde_json`, not string
  interpolation, so an untrusted id/name cannot forge JSON.
- F5-NEW-2: `install_from_usb` validates the caller-supplied `package_name` as a
  single safe path component before joining it onto the USB mount.

Verify (raw cargo, changed crates): fmt clean; `cargo check -p mai-core -p mai-vault`
PASS; `cargo clippy -p mai-core -p mai-vault -p mai-pkg-builder --all-targets -- -D
warnings -A clippy::pedantic` PASS; `cargo test` PASS (285 tests; new regressions:
validate_model_id accept/reject matrix, file_dev store rejects traversal id +
nothing written outside root, verify manifest-authenticated / binding-mismatch-fails
/ strict-mode-rejects-unsigned).

Residual (honest scope): DF-01B fully closed. DF-01A closed at the consumer for v2
packages and enforceable in production via strict mode; the operational completion
is the offline signer emitting `manifest.mldsa` and deployments constructing the
registry in strict mode. DF-01A / DF-01B -> CODE-FIXED.

Commit: (this change set).
### F1 - MAI REST/gRPC/stream authorization (AF-03, F1-NEW-1/3, reflection)

Objective: close the gRPC and REST identity-trust gaps in the MAI API surface.
AF-03 (Critical-class) was the headline: the gRPC plane had no auth interceptor and
trusted a caller-authored `x-im-profile: id:role` metadata string for every
privileged authorization (power transition, model load/unload, registry scan, audit
read, inference) - an attacker sending `x-im-profile: anyone:admin` got admin.

Confirmed against source (`grpc/mod.rs:98`, `grpc/server.rs`): `extract_grpc_profile`
read the caller-supplied role with only a string-whitelist check; `build_grpc_server`
registered no interceptor; the module doc's "all RPCs pass through the auth
interceptor" was false.

Changed:
- AF-03: `authenticate_grpc` resolves the caller from an `x-im-auth-token` API key
  against the shared `ApiKeyStore` (the same store REST uses) and returns the
  authenticated role - never the caller-claimed one. The legacy self-declared
  `x-im-profile` path is honored only when the store explicitly enables
  `allow_internal_profile_header` (dev/internal), the exact mirror of the REST auth
  middleware; production rejects it. Every privileged gRPC call site
  (power/audit/registry/models/inference) authenticates before the authz check.
- gRPC reflection is opt-in: `build_grpc_server` honors `config.enable_reflection`
  (previously added unconditionally, disclosing the full service/method map);
  `MaiServer::run` defaults it off in production.
- F1-NEW-1 (High): `POST /v1/models/install` used the caller-supplied `X-IM-Profile`
  header for its role instead of the middleware-injected authenticated `ProfileInfo`
  (a REST analog of AF-03 letting any valid low-privilege key escalate). It now reads
  the authenticated identity from the request extensions.
- F1-NEW-3: `/v1/admin/rotate-credentials` checked a non-existent "admin" permission
  (which `check_permission` always denies, admins included) - a dead endpoint. It now
  checks `manage_profiles` (admin-only).

Verify: fmt clean; `cargo check -p mai-api` PASS; `cargo clippy -p mai-api
--all-targets -- -D warnings -A clippy::pedantic` PASS; `cargo test -p mai-api` PASS
(new regressions: resolve_grpc_identity valid-token / invalid-token / rejects-caller-
claimed-role-in-production / dev-header-fallback).

Residual (honest scope): AF-03 closed. gRPC transport still has no rate limiting (F1
item6, Low); MaiHealth is intentionally unauthenticated (F1-4b, Low, mirrors the REST
health exemption); the WS self-declared-role path (F1-5b) is latent (WS privileged ops
are stubs) - dispositioned for the streaming-auth follow-on. The "admin sees all
tenants" IDOR (F1 item7) loses its forge-admin amplification with AF-03; per-tenant
object scoping is a separate design item (single-operator model today).
AF-03 -> CODE-FIXED.

Commit: (this change set).
### F2 - AOG gateway (AF-15B fail-open + revocation completeness/freshness)

The gateway audit CONFIRMED-FIXED the prior AF-08/09/10/13/17 gateway work and
surfaced three revocation gaps at the gateway's own snapshot consumer (a simpler
path than the R1/R6 seal/broker consumers):
- N1 (High): the kill switch checked only `is_token_revoked(token_id)` +
  `is_subject_revoked(subject_hash)` (2 dimensions), so revoking a compromised
  signing key, an issuer, a bundle version, or an entire TENANT (the R2 blast-radius
  switch) was silently ineffective at the gateway.
- N2 (Med-High): a configured revocation path with no published snapshot (NotFound)
  was treated as "nothing revoked" - fail-OPEN, contradicting AF-15B.
- N3 (Med): the snapshot signature was verified but never its freshness, so a stale
  but validly-signed snapshot (predating a revocation) was accepted - a replay bypass.

Changed (`crates/aog-gateway/src/lib.rs`):
- `revocation_decision` runs the complete predicate `RevocationSnapshot::revokes`
  (token id, subject, signing key, issuer, bundle, tenant, service identity) instead
  of the two-field check (N1).
- A configured revocation path with an absent snapshot now fails CLOSED (N2).
- The verified snapshot is rejected when stale (`now >= expires_at`; an unparseable
  expiry is treated as stale) before it is trusted (N3).

Verify: fmt clean; `cargo clippy -p aog-gateway --all-targets -- -D warnings -A
clippy::pedantic` PASS; `cargo test -p aog-gateway` PASS (new: stale-detection,
fail-closed-on-stale, denies-revoked-tenant [the N1 dimension], allows-clean). The
full live path (fail-closed on absent + tenant revocation halting the next call) is
provable by the R6-style live gate against Dockerized OpenBao (X2).

Residual (honest scope): N1/N2/N3 closed. Sequence-based anti-rollback (rejecting a
replayed older-sequence snapshot within its TTL) needs the gateway to hold the last
sequence in state; the expiry bound above caps staleness at the snapshot TTL. The
streaming budget/tokenize follow-on and the unwired lease-based SpendLedger (F2 N4/6)
remain documented follow-ons that predate this audit. AF-15B -> CODE-FIXED.

Commit: (this change set).
### F5(b) - backup/restore boundary (AF-11 path escape, AF-19 unsigned-by-default)

Objective: close the two restore-tool findings - a restore manifest whose component
paths escape the backup/target roots (AF-11), and restore accepting unsigned or
unverified manifests by default (AF-19).

Confirmed against source (`tools/mai-admin/src/restore.rs:292/402`, `main.rs`):
`component.path` (manifest-controlled) was joined onto `backup_dir` and `target_dir`
with no containment check - an `apply` with a crafted manifest could copy from,
delete, or write outside the target. And `require_signed` defaulted false, with a
signed-but-no-key manifest treated as `Signed` without actually verifying, so
`restore apply` ran against a fully unverified manifest by default - which is what
made AF-11 reachable.

Changed:
- AF-11: `validate_component_path` requires every component path to be relative and
  composed only of `Normal` components (no `..`, absolute, root, drive prefix);
  `plan_restore` validates each once and uses the validated relative path for both the
  backup-side read and the target-side write, so neither join can escape.
- AF-19: signature verification is on by default for `restore plan`, `restore apply`,
  and `backup verify`; the escape hatch is an explicit `--allow-unsigned`. With
  verification on, an unsigned manifest or a signed manifest presented without a
  verifying key both hard-fail rather than proceeding on a warning.

Verify: fmt clean; `cargo clippy -p mai-admin --all-targets -- -D warnings -A
clippy::pedantic` PASS; `cargo test -p mai-admin` PASS (new: component-path validation
accept/reject matrix; existing plan_require_signed_rejects_unsigned still green).

AF-11 / AF-19 -> CODE-FIXED.

Commit: (this change set).
### F8 - deployment/IaC/supply-chain (AF-20 image pin)

Objective: close AF-20 (production-like deployment images use mutable tags). The F8
audit CONFIRMED the rest of the supply-chain posture is strong - AS-001 third-party
digest pins hold in the production HA stack; no dev-mode OpenBao / known root token /
host-published privileged plane / committed secret / privileged container in any
production manifest; the supply-chain workflow cosign-signs + attests an SBOM.

Confirmed against source (`deployment/wsf-ha/docker-compose.yml:59`): the production
HA reference published `islandmountain/wsf-api:latest` - a mutable tag with no
rollback/reproducibility anchor. (The report's `:57` pointed at the preceding comment.)

Changed: pinned to `islandmountain/wsf-api:${WSF_API_VERSION:-v0.1.0}` - an immutable
release tag (default tracks the workspace version), never `latest`, overridable per
release. Every third-party pull in this stack is already digest-pinned (AS-001).

Verify: `yaml.safe_load` parses; no `:latest` remains in the production HA manifest.

Residual (dispositioned, not fixed): the demo appliance stack pulls mutable third-party
tags (F8-N1, Low - demo/loopback, not production); the compose/profile validator is not
yet a CI gate (F8-N3, Low - a Layer-3 gap; the validator + its 10-case test exist and
run as a documented manual step). Both are Low, demo/ops-plane items outside AF-20's
production scope. AF-20 -> CODE-FIXED.

Commit: (this change set).
### F4 - adapter isolation runtime (bounded DoS fixes + honest DEF-1 deferral)

The adapter framework IS production-wired (`mai-api/src/server.rs:311-347`:
AdapterManager::new -> discover -> start_adapter), so the trusted parent reads a
spawned adapter's stdout/stderr. The audit found two reachable DoS vectors plus the
DEF-1 core (resource isolation) unimplemented.

Fixed (bounded, host-testable):
- Unbounded stdout frame (F4 item3, High): the reader used `AsyncBufReadExt::lines`,
  which buffers a no-newline stream without limit -> OOM in the trusted parent from a
  hostile/buggy adapter. Replaced with `read_bounded_frame` capped at 8 MiB; an
  over-long frame closes the reader instead of growing memory.
- Undrained stderr (F4-N3): stderr was piped but never read, so a chatty adapter
  filling the ~64 KiB pipe buffer blocks on write (hang). stderr is now drained on its
  own task with the same bounded reader.

Verify: fmt clean; `cargo clippy -p mai-adapters --all-targets -- -D warnings -A
clippy::pedantic` PASS; `cargo test -p mai-adapters` PASS (new: bounded-frame line
framing + EOF; oversized-frame rejected).

Deferred honestly (DEF-1 remains a deferred runtime surface, PSPR-26): the CPU/memory
cgroup path is `cfg(target_os="linux")`-only AND its config fields are never populated
by `FrameworkConfig::from_toml`, so in every shipping config adapters run unconfined.
Full CPU/mem/fs/proc/net isolation with fail-closed semantics needs a Linux+cgroups
host and is out of reach here. F4-item4/N1 (cgroup wiring + env-clear on the cgroup
path) and item6 (crash-loop counter reset, in the unwired restart path) are
dispositioned to the DEF-1 lane, not fixed. DEF-1 stays OPEN.

Commit: (this change set).
### F3, F6, F7 - dispositions (pre-integration / behind-defaults / design-note)

These prompts are audits whose gates are reportable/suppressed dispositions with
evidence. Every High/Med claim was confirmed against source; none is a reachable
stop-ship issue requiring a fix in this lane, so each is dispositioned honestly with
its fix direction, and the residual is tracked.

F3 - tool-proxy + approval (`crates/aog-toolproxy`, `aog-approvals`): PRE-INTEGRATION.
`ToolProxy::invoke` has no production caller (constructed only in in-crate tests), and
the signed tool-grant enforcement (`aog-controller::EdgeGrantCache`) is not yet wired
into the proxy. Unauthenticated/overwritable tool manifests (N1), the "egress" scanner
inspecting only inbound results not outbound arguments (N2), session-id-rotation evading
blast-radius caps (N3), approvals lacking principal-auth/nonce/expiry/args-digest (N4),
and the caller-set `untrusted` flag being violable via manifest control are real but
unreachable until the proxy is wired into a serving path. REPORTABLE (pre-integration);
fix direction recorded; tracked to the tool-proxy integration lane, not force-fixed into
an unwired crate.

F6 - compliance/audit-proof (`fabric-proof`, `wsf-ledger`, `mai-compliance`): the signed
trust-bundle/claim path, receipt canonicalization, report certification, and in-memory
chain tamper all have negative controls and are sound. The gaps - sign-before-mutate
invalidating periodic audit signatures (N1), non-atomic finalize->mutate->restore forking
the chain under concurrency (N2), verify_chain not requiring signatures at interval
boundaries (N3), composer fail-open on missing/errored module input (N6), and
NullSigner/NullSealer/AcceptAll defaults with no production guard (N7) - sit on the
audit-chain `record()` path, which ships with Null crypto by default. Fixed here: the
fabric-proof `AcceptAllBundleVerifier` comment no longer claims a non-existent production
guard (N7 dangling reference / CANON §11). Dispositioned to the audit-integrity
hardening lane (a production crypto guard + sign-after-mutate + interval-signature
assertion + fail-closed composer are a feature set, not a boundary bug); REPORTABLE with
fix direction.

F7 - host/HIL/scheduler: mostly ALREADY-FIXED - validated ZFS argv with anti-injection
tests, loopback-only air-gap egress a request cannot re-enable, admin-gated + audited
power transitions (no OS poweroff wired), all channels bounded, scheduler queue + KV pool
capped. Residual is Low design-note: no per-tenant fairness (one authenticated caller can
saturate the global 256-slot queue; edge rate limit is per-route-global and off by
default), the `mai-router` cloud-routing layer does not itself gate on connectivity
(inert - not wired into the appliance request path), and the air-gap switch reader is a
dev stub (fail-safe: defaults air-gapped). SUPPRESSED/design-note for the single-operator
appliance model; per-tenant quota is a multi-tenant hardening item.

### F9 - coverage reconciliation

Phase F closed the reachable trust-boundary findings with tests + gates: AF-03 (F1),
DF-01A/DF-01B + F5-NEW-1/2 (F5a), AF-11/AF-19 (F5b), AF-15B + gateway revocation
completeness/freshness (F2), AF-20 (F8), and the F1 REST analogs (NEW-1/NEW-3). F4 landed
the reachable adapter DoS bounds; DEF-1 (full cgroup isolation) and DEF-2 (signed bounded
update transport) stay OPEN as deferred runtime surfaces (Linux/feature). F3 and the F6
audit-chain hardening are REPORTABLE pre-integration/feature items with recorded fix
direction. F7 is hardened bar Low multi-tenant design notes. No new unexplained
high-impact row remains beyond these tracked deferrals. Per-prompt dispositions + verify
results: `test-evidence/security-remediation/M3/README.md`.