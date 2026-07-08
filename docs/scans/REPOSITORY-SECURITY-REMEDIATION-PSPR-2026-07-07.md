# Repository Security Remediation — Plan / Sequential Prompt Roster (PSPR)

**Repository:** `C:\Users\17076\Documents\Claude\Mighty Eel OS\mai`  
**Source audit:** `AUDIT_REPORT.md`  
**Scan ID:** `60c5b33e-4963-4f68-bd7e-bcaa6816f918`  
**Audited revision:** `6ffaaeeea0a83c7fa071e114183cfa60c5898703`  
**Snapshot digest:** `codex-security-snapshot/v1:sha256:b920522f2f117347053cfb8f0e35237868c1da3b9743ecd7549edb755bf7ddb4`  
**Created:** 2026-07-07  
**Execution mode:** Stem to Stern (STS), strictly in roster order  
**Status:** READY FOR EXPLICIT STS AUTHORIZATION — this draft authorizes no implementation, staging, commit, or push  
**Audit baseline:** 615/615 deterministic inventory rows reviewed; 24 validated findings: 11 High, 13 Medium  
**Release posture:** STOP SHIP until PSPR-31 closes

---

## 0. Authority, execution, and evidence contract

### 0.1 STS authorization

Drafting this PSPR does not authorize product changes. Execution begins only when the owner says **“run this PSPR STS”** or explicitly authorizes a named prompt or milestone.

STS means execute `PSPR-00` through `PSPR-31` in order. A prompt may not be skipped because another change appears to cover it. Each prompt closes only when its own tests, evidence, acceptance criteria, and ledger receipt are complete.

Before execution, read in this order:

1. workspace `AGENTS.md` and any resolvable `@RTK.md` include;
2. this PSPR and `AUDIT_REPORT.md`;
3. `docs/scans/threat_model.md`;
4. `docs/SHIP-PROFILE.md` and `docs/SHIP-HARDENING-PLAN.md`;
5. `docs/dougherty/JOHN-REMEDIATION-PLAN.md`;
6. the current remediation ledger and latest session log.

If authority documents conflict, stop and record the exact conflict. Never resolve a security invariant downward merely to preserve compatibility.

### 0.2 Git and workspace safety

- Work only in the standalone `mai/` repository. Do not re-nest it or edit the sibling Island Mountain website.
- Preserve every pre-existing modification and untracked path. At drafting time these include `docs/INDEX.md`, `.opencode/`, `AUDIT_REPORT.md`, and pre-existing files under `docs/scans/`.
- Never run `git commit` or `git push` without separate, explicit user approval. **STS is not commit or push approval.**
- Before a commit, show staged files and evidence, then ask **“Shall I commit?”** Before a push, show outgoing commits and destination, then ask **“Shall I push?”**
- Do not rewrite history, rotate production credentials, destroy snapshots, migrate production data, or contact external systems without the required explicit authority.
- Use atomic edits. After every write, read the last five lines and verify the line count.
- Do not weaken a control, suppress a warning, delete a test, add an ignore, or convert an expected denial into an allowance to make a gate pass.

### 0.3 Universal prompt receipt

Every prompt must append a receipt to `docs/sessions/SECURITY-REMEDIATION-DEVLOG.md` containing:

- prompt ID, objective, starting HEAD, ending HEAD, and `git status --short`;
- pre-change failing test or exact static proof;
- files changed and why;
- exact commands, exit codes, test counts, and bounded failure output;
- live-service versions/endpoints or an explicit `UNAVAILABLE` result;
- negative-control evidence and tenant/identity matrix where applicable;
- migration, rollback, compatibility, and documentation impact;
- residual risk, blocked work, and next prompt;
- proposed commit scope only; no commit or push without separate approval.

Prompt evidence belongs under `test-evidence/security-remediation/PSPR-XX/`. Evidence must never contain secrets, credentials, regulated payloads, plaintext model weights, or private keys.

### 0.4 Verification tiers

Run focused tests during implementation. Before each prompt closes, run every applicable command below and record non-applicable or unavailable gates honestly:

```powershell
cargo fmt --check
cargo check --workspace
cargo clippy --workspace -- -D warnings -A clippy::pedantic
cargo test --workspace
cargo audit
cargo deny check
ruff check .
mypy --strict mai-sdk-python/src/
mypy adapters/
pytest -q
```

Before staging any files, run the integrity tooling through Git Bash against the exact changed-file list:

```bash
mai/.integrity/scripts/verify-tree.sh <changed-files>
```

Trust-plane prompts require black-box proof against Dockerized OpenBao. AWS brokerage requires Moto or an owner-approved isolated AWS test account. ZFS claims require a disposable real ZFS dataset. Mock-only evidence cannot close a live trust boundary.

### 0.5 Global security invariants

All implementation prompts preserve these invariants:

1. Identity, tenant, privilege, cloud identity, and audit scope come from authenticated server-side state.
2. Missing identity, policy, revocation, key material, audit persistence, or runtime proof fails closed.
3. Attenuation only narrows verified parent authority.
4. Encryption and sealing bind tenant, owner, audience, policy, operation, and version.
5. Every protocol mode traverses the same classification, policy, tokenization, routing, budget, metering, and receipt controls.
6. Untrusted paths are normalized, root-contained, no-follow, and authenticated before privileged I/O.
7. Readiness reports observed runtime facts, never constructor success or configuration intent.
8. Legacy compatibility is explicit, versioned, bounded, receipted, and disabled by default in production.

### 0.6 Stop conditions

Stop the active prompt and request direction only for an external credential, unavailable required infrastructure, destructive migration, unresolved authority conflict, or materially ambiguous product decision. A failing test is normally work to fix, not a stop condition.

Release remains blocked while any of these is true:

- any audit High finding remains open;
- any Medium finding lacks a documented disposition and owner-approved release decision;
- any privileged WSF, gRPC, AOG, broker, vault, restore, or package path fails open;
- live trust-boundary evidence is missing;
- either deferred runtime surface remains unresolved;
- the universal gate is not green;
- the independent closure scan is incomplete or reports a High/Critical finding.

---

## 1. Complete audit-to-prompt closure matrix

| Audit finding | Severity | Root-control location | Root-fix prompt | Live/final proof |
|:--|:--:|:--|:--:|:--:|
| AF-01 Unauthenticated WSF issuance | High | `crates/wsf-api/src/lib.rs:230` | 03 | 28, 31 |
| AF-02 Attenuation signer oracle | High | `crates/fabric-token/src/lib.rs:121` | 04 | 28, 31 |
| AF-03 Caller-authored gRPC admin | High | `mai-api/src/grpc/mod.rs:98` | 05 | 28, 31 |
| AF-04 Cross-tenant envelope unseal | High | `crates/wsf-seal/src/lib.rs:305` | 08 | 28, 31 |
| AF-05 Caller-selected AWS role | High | `crates/wsf-api/src/lib.rs:333` | 09 | 28, 31 |
| AF-06 Constructed vault passes readiness | Medium | `mai-api/src/server.rs:689` | 21 | 29, 31 |
| AF-07A Plaintext model weights | Medium | `mai-vault/src/zfs.rs:275` | 19 | 29, 31 |
| AF-07B Metadata-only snapshot/rollback | Medium | `mai-vault/src/zfs.rs:453` | 20 | 29, 31 |
| AF-08 OpenAI stream bypass | High | `crates/aog-gateway/src/surface_openai.rs:176` | 12 | 28, 31 |
| AF-09 Anthropic stream bypass | High | `crates/aog-gateway/src/surface_anthropic.rs:133` | 13 | 28, 31 |
| AF-10 Legacy completion bypass | High | `crates/aog-gateway/src/surface_openai.rs:398` | 14 | 28, 31 |
| AF-11 Restore path traversal | Medium | `tools/mai-admin/src/restore.rs:292` | 23 | 29, 31 |
| AF-12 Known-token dev OpenBao exposure | High | `deployment/appliance/docker-compose.yml:11` | 01, 24 | 30, 31 |
| AF-13 AOG defaults to shadow | High | `crates/aog-gateway/src/main.rs:62` | 02 | 28, 31 |
| AF-14 Unauthenticated cross-tenant receipts | Medium | `crates/wsf-api/src/lib.rs:361` | 16 | 28, 31 |
| AF-15 Revocation replay/rollback | Medium | `crates/fabric-revocation/src/lib.rs:151` | 06 | 28, 31 |
| AF-15B Missing revocation fails open | Medium | `crates/aog-gateway/src/lib.rs:188` | 07 | 28, 31 |
| AF-16 AWS credential lifetime mismatch | Medium | `crates/wsf-broker/src/lib.rs:245` | 10 | 28, 31 |
| AF-17A Global usage aggregates | Medium | `crates/aog-gateway/src/surface_openai.rs:233` | 15 | 28, 31 |
| AF-17B Global ROI recommendations | Medium | `crates/aog-gateway/src/surface_openai.rs:259` | 15 | 28, 31 |
| AF-19 Unsigned/unverified restore default | Medium | `tools/mai-admin/src/main.rs:123` | 22 | 29, 31 |
| AF-20 Mutable deployment images | Medium | `deployment/wsf-ha/docker-compose.yml:57` | 25 | 30, 31 |
| DF-01A Manifest identity not signed | High | `mai-core/src/models/verify.rs:120` | 17 | 29, 31 |
| DF-01B Manifest model ID path traversal | Medium | `mai-vault/src/zfs.rs:275` | 18 | 29, 31 |
| Follow-up: adapter runtime isolation | Deferred | `mai-adapters/src/process.rs` | 26 | 30, 31 |
| Follow-up: signed update transport | Deferred | `mai-core/src/models/update.rs`; `mai-api/src/handlers/updates.rs` | 27 | 30, 31 |
| Follow-up: Clippy release gate | Quality | `mai-core/src/cache.rs:109` | 00 | 30, 31 |

No audit item closes from documentation alone. Each reportable finding needs a root-fix receipt and its listed live/final proof.

---

## 2. Sequential execution index

| Order | Prompt | Outcome | Depends on |
|:--:|:--:|:--|:--|
| 1 | PSPR-00 | Freeze baseline, ledger, and quality gate | — |
| 2 | PSPR-01 | Emergency OpenBao containment | 00 |
| 3 | PSPR-02 | Production AOG fails closed | 00 |
| 4 | PSPR-03 | Authenticated, policy-derived WSF issuance | 01 |
| 5 | PSPR-04 | Restriction-only token attenuation | 03 |
| 6 | PSPR-05 | Real gRPC authentication and authorization | 01 |
| 7 | PSPR-06 | Fresh, scoped, monotonic revocation snapshots | 04, 05 |
| 8 | PSPR-07 | AOG revocation-state loss fails closed | 06 |
| 9 | PSPR-08 | Tenant/subject/audience-bound envelope unseal | 06 |
| 10 | PSPR-09 | Server-selected cloud grants | 03, 06 |
| 11 | PSPR-10 | Credential lifetime never exceeds authority | 09 |
| 12 | PSPR-11 | One mandatory AOG dispatch pipeline | 02, 07 |
| 13 | PSPR-12 | OpenAI streaming parity | 11 |
| 14 | PSPR-13 | Anthropic streaming parity | 11 |
| 15 | PSPR-14 | Legacy completion parity | 11 |
| 16 | PSPR-15 | Tenant-scoped usage and ROI | 03, 11 |
| 17 | PSPR-16 | Authenticated tenant-scoped WSF receipts | 03, 06 |
| 18 | PSPR-17 | Authenticate complete model-package identity | 01 |
| 19 | PSPR-18 | Typed, contained model identity paths | 17 |
| 20 | PSPR-19 | Encrypted model storage | 18 |
| 21 | PSPR-20 | Real ZFS snapshots and rollback | 19 |
| 22 | PSPR-21 | Runtime-measured vault readiness | 19, 20 |
| 23 | PSPR-22 | Signed restore is mandatory | 01 |
| 24 | PSPR-23 | Restore paths remain inside approved roots | 22 |
| 25 | PSPR-24 | Production OpenBao composition | 01, 03, 06 |
| 26 | PSPR-25 | Immutable, verified deployment artifacts | 24 |
| 27 | PSPR-26 | Adapter OS isolation closure | 05 |
| 28 | PSPR-27 | Signed, bounded update transport | 17, 25 |
| 29 | PSPR-28 | Live trust-plane adversarial suite | 03–16 |
| 30 | PSPR-29 | Storage, package, restore, and migration suite | 17–23 |
| 31 | PSPR-30 | Full build/deployment/claims reconciliation | 24–29 |
| 32 | PSPR-31 | Independent re-scan and release decision | 30 |

---

## 3. Sequential prompts

## PSPR-00 — Freeze baseline, create the ledger, and repair the known Clippy gate

**Closes:** quality follow-up only  
**Primary files:** `docs/scans/`, `docs/sessions/`, `mai-core/src/cache.rs`

### Sequential prompt

```text
SESSION PSPR-00 — Baseline and remediation ledger

GOAL
Create a reproducible remediation baseline and restore the known Clippy release gate without changing product behavior.

IMPLEMENT
1. Record repository root, branch, HEAD, remotes, tool versions, snapshot digest, and exact dirty tree.
2. Create/update a remediation ledger containing every row in §1, with status, owner prompt, root-control location, regression test, evidence path, migration impact, and residual risk.
3. Run the complete verification tier and save exact baseline results. Distinguish PASS, FAIL, UNAVAILABLE, and NOT RUN.
4. Repair only the `clippy::doc_lazy_continuation` error at `mai-core/src/cache.rs:109`; do not suppress the lint.
5. Inventory Docker/OpenBao, Moto, GCP/Azure emulators, disposable ZFS, and target-hardware availability.
6. Create the DEVLOG/evidence directory contract. Do not modify product behavior, stage, commit, or push.

VERIFY
- The ledger contains 24 reportable findings, two deferred surfaces, and the quality follow-up exactly once.
- `cargo fmt --check`, focused Clippy, and workspace Clippy pass after the documentation-only repair.
- Pre-existing user changes remain byte-for-byte preserved.

OUTPUT
Write the prompt receipt, current blockers, available live infrastructure, and next prompt readiness.
```

### Acceptance criteria

- Baseline is reproducible and every audit issue has an accountable closure path.
- The known Clippy failure is fixed without an allow or suppression.
- No security finding is marked closed.

---

## PSPR-01 — Emergency containment of the known-token development OpenBao

**Closes:** AF-12 containment only  
**Primary files:** `deployment/appliance/docker-compose.yml`, deployment validation/tests, operator security docs

### Sequential prompt

```text
SESSION PSPR-01 — Remove default remote trust-root takeover

GOAL
Make the shipped/default appliance composition incapable of exposing a development OpenBao with a known root token.

IMPLEMENT
1. Capture the effective current compose configuration and a negative test proving the vulnerable combination.
2. Remove dev mode and literal/default root-token behavior from appliance defaults.
3. Remove unrestricted host publication of port 8200; place OpenBao on a private network with explicit health access.
4. Require secret injection from an approved runtime source and reject missing, placeholder, known, or committed secrets.
5. Add a production-profile validator that rejects dev mode, plaintext/unrestricted listeners, known tokens, and host-published trust services.
6. Keep any demo composition in an unmistakably non-production profile with loopback binding and warnings; production must never inherit it.
7. Add compose/config regression fixtures for every rejected unsafe combination.

VERIFY
- Rendered production compose has no known token, dev command, or unrestricted OpenBao bind.
- Unsafe fixtures fail before services start; a secure fixture reaches health.
- Secret and config scans contain no live credential material.

OUTPUT
Record containment evidence. Keep AF-12 open until PSPR-24 supplies the production live gate.
```

### Acceptance criteria

- Default deployment cannot remotely expose a known-token trust root.
- Demo convenience cannot be selected accidentally by production profile.

---

## PSPR-02 — Make production AOG policy enforcement fail closed

**Closes:** AF-13  
**Primary files:** `crates/aog-gateway/src/main.rs`, `crates/aog-gateway/src/lib.rs`, deployment configuration and tests

### Sequential prompt

```text
SESSION PSPR-02 — Eliminate implicit shadow mode

GOAL
Ensure missing or ambiguous configuration cannot send regulated content through non-blocking policy mode.

IMPLEMENT
1. Freeze tests for absent, empty, malformed, mixed-case, shadow, report, and enforce configuration.
2. Make ship/production profiles require explicit enforce mode or default unconditionally to enforce.
3. Restrict shadow/report modes to an explicit development capability unavailable in production builds/profiles.
4. Ensure `AppState` construction cannot silently reintroduce Shadow when caller configuration is absent.
5. Make startup fail before listener bind when production policy mode or policy backend is unavailable.
6. Expose effective mode in readiness and sanitized audit events.
7. Update compose, examples, docs, and tests to agree on the secure default.

VERIFY
- Missing `AOG_MODE` in production either selects Enforce or aborts before bind.
- A deny decision prevents provider dispatch in every supported production configuration.
- Shadow/report require explicit development opt-in and are visible in readiness.

OUTPUT
Close AF-13 only with startup and denied-dispatch negative evidence.
```

### Acceptance criteria

- No supported production path defaults to non-blocking policy.
- Misconfiguration is observable and fail-closed.

---

## PSPR-03 — Authenticate WSF and derive issuance authority server-side

**Closes:** AF-01  
**Primary files:** `crates/wsf-api/src/lib.rs`, WSF contracts/OpenAPI/clients, policy and integration tests

### Sequential prompt

```text
SESSION PSPR-03 — Authenticated WSF issuance

GOAL
Prevent anonymous or lower-trust callers from minting tenant, subject, role, model, or budget authority.

IMPLEMENT
1. Inventory every WSF route and classify authentication, action, tenant, rate-limit, and receipt requirements.
2. Introduce a trusted `WsfPrincipal` extractor backed by mTLS/workload identity or another reviewed production authenticator.
3. Apply authentication middleware to every privileged route; exemptions must be explicit and non-sensitive.
4. Replace authoritative request fields with server-derived tenant, subject, service identity, roles, audience, and ceilings.
5. Permit caller input only as a narrowing request; validate model subset and budget against policy.
6. Separate self, delegated, service, and administrative issuance permissions.
7. Rate-limit issuance and receipt every allow/deny without token material.
8. Version OpenAPI/SDK behavior and define fail-closed legacy handling.

VERIFY
- Anonymous, forged, wrong-audience, wrong-tenant, role-elevation, model-widening, and budget-widening cases fail before signing.
- Route conformance test fails CI when a privileged route lacks policy metadata.
- Two-tenant black-box issuance against OpenBao succeeds only for authorized identity.

OUTPUT
Close AF-01 with route inventory, policy matrix, negative tests, and live issuance evidence.
```

### Acceptance criteria

- No caller-authored field establishes signed authority.
- Every privileged WSF route has authenticated principal context.

---

## PSPR-04 — Rebuild attenuation as restriction-only monotonic narrowing

**Closes:** AF-02  
**Primary files:** `crates/fabric-token/src/lib.rs`, WSF attenuation contracts/handler, SDK and property tests

### Sequential prompt

```text
SESSION PSPR-04 — Eliminate the attenuation signing oracle

GOAL
Make it impossible for attenuation to sign a child with authority absent from a verified parent.

IMPLEMENT
1. Replace complete-child input with a versioned `TokenRestrictions` request.
2. Verify parent signature, issuer, key ID, audience, tenant, subject/service identity, not-before, expiry, bundle version, revocation, and lineage before child construction.
3. Generate immutable child identity, parent link, token ID, issuer, and signature server-side.
4. Enforce subset/equality across roles, scopes, models, operations, destinations, classifications, caveats, offline flags, delegation depth, lifetime, and budgets.
5. Add maximum depth, cycle/duplicate-ID rejection, and atomic sibling-budget accounting.
6. Reject unknown restriction fields and non-canonical encodings.
7. Define v1 handling: no production attenuation; bounded verification-only migration if explicitly required.
8. Add property-based generators that attempt to widen each authority axis independently and concurrently.

VERIFY
- Unsigned, malformed, wrong-key, expired, future, revoked, stale-bundle, wrong-audience, and wrong-tenant parents fail before signing.
- Every generated widening is rejected; valid narrowing preserves lineage and succeeds.
- Concurrent sibling creation/spend cannot exceed the parent ceiling.
- Black-box issue→attenuate→verify passes against live OpenBao.

OUTPUT
Close AF-02 only with property, concurrency, compatibility, and live black-box evidence.
```

### Acceptance criteria

- Public contracts expose restrictions, not attacker-constructed children.
- Every child is a provable monotonic narrowing of a currently valid parent.

---

## PSPR-05 — Replace caller-authored gRPC roles with authenticated identity

**Closes:** AF-03  
**Primary files:** `mai-api/src/grpc/mod.rs`, gRPC server/bootstrap, interceptors, RPC authorization tests

### Sequential prompt

```text
SESSION PSPR-05 — Trusted gRPC principal and permission matrix

GOAL
Prevent `x-im-profile` or any client metadata from conferring administrator authority.

IMPLEMENT
1. Inventory every gRPC service/method, including reflection and health, with required action and tenant scope.
2. Add transport authentication and an interceptor that creates a trusted principal from verified credentials.
3. Treat display/profile metadata only as an optional non-authoritative hint or remove it.
4. Resolve roles and permissions from server-side identity policy; deny missing or ambiguous mappings.
5. Apply object/tenant authorization to model, power, registry, audit, and streaming RPCs.
6. Bound principal lifetime for streams and re-check revocation at defined intervals.
7. Disable or authenticate production reflection as policy requires.
8. Receipt denied privileged attempts without echoing credentials.

VERIFY
- Forged `x-im-profile: admin` with no valid identity is rejected before method execution.
- Method-by-role/tenant matrix covers every RPC and protocol state.
- Credential expiry/revocation during long streams terminates privileged access within the SLO.

OUTPUT
Close AF-03 with method inventory, interceptor proof, and forged-admin black-box evidence.
```

### Acceptance criteria

- Client metadata cannot establish role or tenant authority.
- Every privileged RPC is dominated by authenticated authorization.

---

## PSPR-06 — Make revocation snapshots fresh, scoped, and anti-rollback

**Closes:** AF-15  
**Primary files:** `crates/fabric-revocation/src/lib.rs`, snapshot storage/import, consumer interfaces and tests

### Sequential prompt

```text
SESSION PSPR-06 — Monotonic revocation state

GOAL
Prevent a validly signed but stale or wrong-scope snapshot from restoring revoked authority.

IMPLEMENT
1. Version the snapshot schema with issuer, tenant/scope, epoch/sequence, issued-at, expires-at, bundle/key ID, previous digest, and canonical payload digest.
2. Verify signature, trust anchor, canonical form, scope, time bounds, predecessor, and strictly increasing epoch before publication.
3. Persist high-water marks and last-known-good snapshots atomically across restart.
4. Reject rollback, fork, replay, future, expired, wrong-issuer, wrong-tenant, and wrong-anchor snapshots.
5. Define bootstrap and emergency-recovery procedures that require explicit operator authority and receipts.
6. Cover token ID, subject, service identity, signer key, issuer, bundle version, tenant, and lineage revocation dimensions.
7. Integrate a context-aware revocation requirement into every privileged verifier.

VERIFY
- Table/property tests cover each invalid dimension and snapshot ordering race.
- Restart, replica, and partition tests never accept an older snapshot than the persisted high-water mark.
- Emergency revoke reaches every privileged consumer within the declared SLO.

OUTPUT
Close AF-15 with schema vectors, anti-rollback tests, persistence evidence, and consumer inventory.
```

### Acceptance criteria

- Signature validity alone is never sufficient to install revocation state.
- Snapshot freshness and scope survive restart and concurrency.

---

## PSPR-07 — Fail closed when AOG revocation state is absent

**Closes:** AF-15B  
**Primary files:** `crates/aog-gateway/src/lib.rs`, OpenBao revocation adapter, readiness and tests

### Sequential prompt

```text
SESSION PSPR-07 — AOG revocation-state loss

GOAL
Ensure missing, deleted, unreadable, stale, or unverifiable revocation state cannot authorize a privileged request.

IMPLEMENT
1. Freeze tests for NotFound, permission denied, timeout, malformed payload, bad signature, stale epoch, expired snapshot, and rollback.
2. Replace “not found means no revocations” with a typed unavailable/invalid security-state result.
3. Fail privileged request authorization closed while preserving only explicitly documented non-sensitive health behavior.
4. Decide and implement bounded last-known-good use consistent with PSPR-06 freshness limits.
5. Reflect revocation health in readiness and prevent production bind when no acceptable initial snapshot exists.
6. Add alert/receipt events that distinguish unavailable from revoked without leaking subjects.

VERIFY
- Every absent/invalid-state fixture prevents provider dispatch and privileged operations.
- Recovery installs only a newer valid snapshot and resumes safely.
- OpenBao deletion/partition black-box tests match unit behavior.

OUTPUT
Close AF-15B only with negative state-loss and recovery evidence.
```

### Acceptance criteria

- Revocation uncertainty cannot be interpreted as authorization.
- Startup and runtime behavior use the same freshness policy.

---

## PSPR-08 — Bind envelope unseal to tenant, owner, audience, and policy

**Closes:** AF-04  
**Primary files:** `crates/wsf-seal/src/lib.rs`, envelope contracts, OpenBao transit policy, migration/tests

### Sequential prompt

```text
SESSION PSPR-08 — Tenant-bound envelope v2

GOAL
Prevent a valid token from unsealing another tenant’s, subject’s, service’s, or audience’s envelope.

IMPLEMENT
1. Define envelope v2 fields and canonical AAD for tenant, owner, service identity, audience, classification, policy/bundle version, operation/destination, key ID, envelope version, and authorizing token lineage.
2. Derive seal authority and labels from verified principal/policy; requests may only narrow them.
3. Use per-tenant keys or cryptographically tenant-separated context with least-privilege OpenBao policy.
4. Before Transit decrypt, verify envelope integrity, current token context, revocation, tenant/owner/audience, policy freshness, operation, destination, and classification.
5. Reject cross-binding attempts before key unwrap and receipt every denial safely.
6. Freeze tamper vectors proving each security field changes authentication.
7. Provide an offline, authenticated v1→v2 migration; disable silent production v1 unseal.

VERIFY
- Cross-tenant, cross-subject, cross-service, wrong-audience, revoked, stale-policy, label-downgrade, and tamper cases fail before Transit decrypt.
- Two-tenant live OpenBao seal/unseal, restart, and key-rotation tests pass.
- Migration is idempotent and rollback behavior is documented/tested.

OUTPUT
Close AF-04 with vectors, pre-decrypt denial proof, live two-tenant evidence, and migration receipt.
```

### Acceptance criteria

- Envelope cryptography and authorization bind the same security context.
- No caller-selected label or valid foreign token enables unseal.

---

## PSPR-09 — Replace caller-selected cloud identities with named grants

**Closes:** AF-05  
**Primary files:** `crates/wsf-api/src/lib.rs`, `crates/wsf-broker/`, policy contracts and provider tests

### Sequential prompt

```text
SESSION PSPR-09 — Server-side cloud grant selection

GOAL
Prevent callers from selecting any AWS role trusted by broker root credentials.

IMPLEMENT
1. Remove raw AWS role ARN from the public exchange contract; accept only a tenant-scoped `grant_id` and narrowing operation intent.
2. Resolve grant→role/action/resource/region/external-ID/session-tag/TTL policy from authenticated tenant/workload state.
3. Sign/version grant policy, cache it with freshness bounds, and deny missing, ambiguous, stale, or broadened mappings.
4. Enforce least privilege in the broker root role and assumed-role session policy.
5. Apply equivalent named-grant semantics to GCP and Azure paths where supported.
6. Never put credentials or full privileged identity material in logs/receipts.
7. Version clients and define a fail-closed migration from raw identity fields.

VERIFY
- Arbitrary role/account, adjacent grant, wrong tenant, action/resource/region widening, and missing policy fail before STS.
- Authorized grant succeeds in Moto/isolated provider test; adjacent access fails.
- Contract/OpenAPI contains no public raw privileged cloud identity field.

OUTPUT
Close AF-05 with grant-policy matrix, negative provider tests, and live/emulated exchange evidence.
```

### Acceptance criteria

- Cloud identity is chosen by trusted server-side policy.
- Provider sessions are narrower than both principal and named grant.

---

## PSPR-10 — Bound cloud credential lifetime to remaining token authority

**Closes:** AF-16  
**Primary files:** `crates/wsf-broker/src/lib.rs`, provider adapters, clock/TTL tests

### Sequential prompt

```text
SESSION PSPR-10 — Credential TTL invariant

GOAL
Ensure brokered credentials never survive the WSF token authority that created them.

IMPLEMENT
1. Define effective TTL as the minimum of remaining token validity, grant ceiling, provider maximum, and policy ceiling minus clock-skew safety margin.
2. Never round or clamp effective authority upward to satisfy a provider minimum.
3. If remaining authority is below provider minimum, deny exchange and instruct the caller to renew authority.
4. Re-check token/revocation/grant freshness immediately before provider call.
5. Use provider session tags/conditions and short-lived credentials; zeroize response material after serialization.
6. Add deterministic boundary tests around minimum, maximum, zero, negative, skew, and race-to-expiry values.

VERIFY
- Near-expiry token cannot produce a 900-second credential.
- Returned expiry is never later than token/grant/policy expiry in property tests.
- Moto/provider integration confirms requested and returned durations.

OUTPUT
Close AF-16 with the formal TTL equation, boundary tests, and provider evidence.
```

### Acceptance criteria

- Provider minimums cause denial, never authority extension.
- Clock skew and exchange latency are explicitly bounded.

---

## PSPR-11 — Create one mandatory AOG provider-dispatch pipeline

**Closes:** architectural prerequisite for AF-08, AF-09, AF-10  
**Primary files:** `crates/aog-gateway/src/lib.rs`, surface handlers, provider/meter/tokenizer/receipt interfaces

### Sequential prompt

```text
SESSION PSPR-11 — Policy-dominated dispatch architecture

GOAL
Make provider invocation structurally impossible without completing the full governance pipeline.

IMPLEMENT
1. Inventory every OpenAI, Anthropic, streaming, legacy, embedding, batch, and future provider entrypoint.
2. Extract one typed dispatch pipeline: authenticate → derive tenant → classify → policy → tokenize/redact → route → reserve budget → provider → settle usage → receipt/audit.
3. Make raw provider handles private to the pipeline module; handlers cannot invoke them directly.
4. Model streaming as a lifecycle with preflight reservation, chunk controls, cancellation, final settlement, and error receipt.
5. Define fail-closed behavior for classifier, policy, tokenizer, meter, receipt store, provider, disconnect, and partial stream failures.
6. Add a compile-time or architecture test that detects direct provider calls outside approved dispatch code.
7. Preserve neutralized payload confidentiality and prohibit sensitive content in telemetry.

VERIFY
- Every route maps to the pipeline inventory and no handler has a bypassing provider capability.
- Injected failures at every stage prevent or terminate egress safely and settle budget exactly once.
- Non-streaming behavior remains compatible where security invariants permit.

OUTPUT
Do not close AF-08/09/10 yet. Record the shared pipeline contract and route migration checklist.
```

### Acceptance criteria

- Provider dispatch is dominated by all required controls by construction.
- Streaming and non-streaming share the same security stages.

---

## PSPR-12 — Route OpenAI streaming through the mandatory pipeline

**Closes:** AF-08  
**Primary files:** `crates/aog-gateway/src/surface_openai.rs`, streaming integration tests

### Sequential prompt

```text
SESSION PSPR-12 — OpenAI streaming parity

GOAL
Apply tokenization, metering, budget, receipt, and policy controls to every OpenAI stream.

IMPLEMENT
1. Replace the direct `provider.stream` path with PSPR-11’s dispatch API.
2. Ensure only tokenized/approved content reaches the provider.
3. Reserve budget before first byte; meter prompt and completion chunks; settle exactly once on success, error, timeout, or disconnect.
4. Emit start/final/error receipts linked by correlation ID without payload leakage.
5. Bound chunk size, stream duration, backpressure, and client-abandon behavior.
6. Add parity tests against non-streaming chat and adversarial classified payloads.

VERIFY
- A denied or tokenization-failed request produces zero provider calls and zero streamed bytes.
- Usage and receipts reconcile for success, provider error, timeout, and disconnect.
- Black-box stream test proves regulated source text is absent from provider capture/logs.

OUTPUT
Close AF-08 with provider-spy, meter, receipt-chain, and black-box evidence.
```

### Acceptance criteria

- OpenAI streaming cannot bypass any mandatory pipeline stage.
- Partial streams cannot evade accounting or leak unapproved payloads.

---

## PSPR-13 — Route Anthropic streaming through the mandatory pipeline

**Closes:** AF-09  
**Primary files:** `crates/aog-gateway/src/surface_anthropic.rs`, streaming integration tests

### Sequential prompt

```text
SESSION PSPR-13 — Anthropic streaming parity

GOAL
Give Anthropic streaming the same governance guarantees as every other provider path.

IMPLEMENT
1. Replace direct streaming provider access with PSPR-11’s dispatch API.
2. Normalize Anthropic request/content blocks into the canonical classified/tokenized representation without losing security metadata.
3. Apply reservation, chunk metering, final settlement, and receipt linkage on every terminal state.
4. Handle tool-use/content-block events without allowing raw sensitive fields to bypass tokenization.
5. Bound event/frame sizes, duration, backpressure, and disconnect cleanup.
6. Add cross-surface parity tests proving equivalent OpenAI/Anthropic policy outcomes.

VERIFY
- Deny/tokenizer/meter/receipt failures prevent provider output.
- Tool-use and mixed content cannot leak pre-tokenized source material.
- Usage and receipt totals reconcile under success and every failure mode.

OUTPUT
Close AF-09 with protocol-specific and cross-surface parity evidence.
```

### Acceptance criteria

- Anthropic protocol differences cannot create a control bypass.
- Settlement is exactly once across all stream outcomes.

---

## PSPR-14 — Route legacy OpenAI completions through the mandatory pipeline

**Closes:** AF-10  
**Primary files:** `crates/aog-gateway/src/surface_openai.rs`, legacy API/compatibility tests

### Sequential prompt

```text
SESSION PSPR-14 — Legacy completion parity or retirement

GOAL
Ensure the legacy completion endpoint cannot bypass classification, policy, tokenization, routing, budgets, metering, or receipts.

IMPLEMENT
1. Decide explicitly between secure pipeline migration and endpoint retirement; do not leave a partial compatibility path.
2. If retained, translate legacy input into the canonical dispatch request with no direct provider handle.
3. Apply the same tenant, classification, policy, tokenization, route, budget, meter, and receipt semantics as modern chat.
4. If retired, return a stable non-success status, remove deployment exposure, update OpenAPI/SDK/docs, and test absence of provider calls.
5. Add malformed/multi-prompt/large-prompt/model-alias and downgrade tests.
6. Record the compatibility and migration decision.

VERIFY
- Every retained request follows the pipeline; every retired request is denied before provider access.
- Legacy model aliases cannot select an unauthorized provider/model.
- Accounting and receipts match modern endpoint semantics.

OUTPUT
Close AF-10 with the explicit retention/retirement decision and negative provider-call evidence.
```

### Acceptance criteria

- No legacy compatibility route bypasses current governance.
- Clients receive deterministic migration behavior.

---

## PSPR-15 — Scope usage and ROI analytics to the authenticated tenant

**Closes:** AF-17A, AF-17B  
**Primary files:** `crates/aog-gateway/src/surface_openai.rs`, ledger/ROI query interfaces and tests

### Sequential prompt

```text
SESSION PSPR-15 — Tenant-isolated analytics

GOAL
Prevent ordinary authenticated tenants from learning another tenant’s provider, model, workflow, spend, or estate metadata.

IMPLEMENT
1. Preserve the authenticated tenant returned by authorization and pass it as a mandatory query key.
2. Change ledger aggregation APIs so tenant scope cannot be omitted by ordinary callers.
3. Compute ROI only from the authorized tenant’s aggregates and tenant-visible pricing/configuration.
4. Create a distinct, audited global-auditor capability with explicit elevation and response redaction if global views are required.
5. Add pagination/limits, response-shape redaction, cache-key tenant binding, and no existence oracle.
6. Test empty, sparse, shared-provider, shared-model, and guessed-identifier cases across two tenants.

VERIFY
- Tenant A’s usage and ROI are invariant when only tenant B’s data changes.
- Tenant keys cannot request or infer global aggregates.
- Global auditor behavior requires separate authorization and is receipted.

OUTPUT
Close AF-17A and AF-17B separately with endpoint-specific two-tenant receipts.
```

### Acceptance criteria

- Tenant scope is mandatory in data access, computation, caching, and output.
- Shared dimensions do not create cross-tenant inference.

---

## PSPR-16 — Authenticate, authorize, and constrain WSF receipt queries

**Closes:** AF-14  
**Primary files:** `crates/wsf-api/src/lib.rs`, receipt ledger/query contracts and tests

### Sequential prompt

```text
SESSION PSPR-16 — Tenant-safe receipt access

GOAL
Prevent anonymous or cross-tenant disclosure of evidence and correlation metadata.

IMPLEMENT
1. Require `WsfPrincipal` and an explicit audit-read action before acquiring/querying the ledger.
2. Replace arbitrary field/value queries with typed allowlisted filters.
3. Make tenant predicate mandatory except for a separately audited global-auditor action.
4. Add pagination, maximum limits, stable ordering, rate limits, and bounded query cost.
5. Redact token IDs, subject correlation, provider identifiers, and sensitive metadata by role/purpose.
6. Ensure error/status behavior reveals no cross-tenant existence oracle.
7. Add two-tenant ingest/query/export tests and arbitrary-field rejection tests.

VERIFY
- Anonymous, unauthorized, cross-tenant, guessed-ID, unbounded, and disallowed-field queries return no metadata.
- Authorized tenant and global-auditor paths return only policy-permitted shapes.
- Live ledger restart/query evidence preserves tenant filtering.

OUTPUT
Close AF-14 with auth-before-lock proof and two-tenant black-box results.
```

### Acceptance criteria

- Receipt access is authenticated, action-authorized, tenant-bound, and bounded.
- Query errors do not disclose foreign record existence.

---

## PSPR-17 — Authenticate the complete model-package identity

**Closes:** DF-01A  
**Primary files:** `mai-core/src/models/verify.rs`, package schema/builder/import tests

### Sequential prompt

```text
SESSION PSPR-17 — Signed model package manifest

GOAL
Prevent valid signed weights from being paired with attacker-selected identity, compatibility, or security metadata.

IMPLEMENT
1. Define a versioned canonical package manifest containing model identity, version, quantization, hashes/sizes, compatibility, capabilities, classification, provenance, and payload inventory.
2. Sign the canonical manifest digest plus every payload digest and package format version.
3. Verify signature, trusted signer policy, canonical encoding, manifest/payload completeness, digests, sizes, and duplicate/unknown entries before parsing privileged fields.
4. Reject signature swapping, manifest substitution, extra files, missing files, duplicate names, ambiguous Unicode, and unsupported versions.
5. Bind approval/import receipts to the verified manifest digest.
6. Provide an explicit offline migration/re-sign path for legacy packages; production import denies unsigned legacy packages.

VERIFY
- Changing any manifest field or payload invalidates verification.
- Signed weights with a substituted model name/version/quantization are rejected before installation.
- Golden vectors verify across producer/importer implementations.

OUTPUT
Close DF-01A with canonical vectors, tamper corpus, and legacy disposition.
```

### Acceptance criteria

- One signature authenticates the complete package identity and contents.
- Privileged install decisions use only verified manifest data.

---

## PSPR-18 — Replace free-form model IDs with contained storage identity

**Closes:** DF-01B  
**Primary files:** `mai-vault/src/zfs.rs`, model identity types, install/storage tests

### Sequential prompt

```text
SESSION PSPR-18 — Path-safe model storage identity

GOAL
Prevent manifest-derived names, versions, or quantization strings from escaping the vault root.

IMPLEMENT
1. Introduce validated typed components with explicit grammar, length, normalization, and reserved-name rules.
2. Prefer a server-derived opaque storage key from the verified manifest digest rather than concatenated display fields.
3. Reject separators, absolute paths, drive/UNC prefixes, dot segments, alternate data streams, control characters, ambiguous Unicode, and Windows device names.
4. Join under an already-open approved root and enforce canonical containment with no-follow semantics.
5. Defend against symlink, junction, reparse-point, mount, and time-of-check/time-of-use substitution.
6. Apply the same invariant to staging, final install, load, delete, snapshot, export, and migration paths.
7. Add cross-platform traversal/property tests.

VERIFY
- Traversal corpus cannot create/read/delete outside the disposable root.
- Existing valid identities map deterministically and collision-free.
- Windows and Unix path semantics are both tested.

OUTPUT
Close DF-01B with containment proof, traversal corpus, and migration mapping.
```

### Acceptance criteria

- Display metadata never becomes an unchecked filesystem path.
- All model lifecycle operations share one contained storage resolver.

---

## PSPR-19 — Encrypt and authenticate model weights at rest

**Closes:** AF-07A  
**Primary files:** `mai-vault/src/zfs.rs`, vault crypto/storage format, migration and tests

### Sequential prompt

```text
SESSION PSPR-19 — Encrypted model storage

GOAL
Ensure raw storage disclosure or tampering cannot reveal or silently alter model weights.

IMPLEMENT
1. Define a versioned encrypted object format with algorithm/key IDs, nonce, chunk metadata, authenticated manifest digest, tenant/model context, and plaintext size bounds.
2. Wire the production PQC/encryption engine into store and load; remove direct plaintext `weights.bin` writes from production paths.
3. Derive/separate keys by tenant and verified model identity; enforce nonce uniqueness and authenticated context.
4. Stream encryption/decryption with bounded memory and atomic temp→final publication after authentication.
5. Reject truncation, reordering, bit flips, wrong tenant/model/key/version, extra chunks, and rollback.
6. Zeroize plaintext/key buffers and prevent payloads from logs, panic text, receipts, and core dumps where controllable.
7. Build an authenticated, resumable plaintext→encrypted migration with backup and rollback plan.

VERIFY
- Disk search never finds the plaintext fixture or known fragments.
- Every tamper/wrong-context case fails before plaintext release.
- Restart, concurrent read/write, interrupted migration, and key-rotation tests pass.

OUTPUT
Close AF-07A only with real storage bytes, tamper, restart, and migration evidence.
```

### Acceptance criteria

- Production store/load is authenticated encryption, not merely ZFS configuration intent.
- No unauthenticated plaintext reaches consumers.

---

## PSPR-20 — Implement real bounded ZFS snapshot and rollback operations

**Closes:** AF-07B  
**Primary files:** `mai-vault/src/zfs.rs`, ZFS command seam, integration tests and operator docs

### Sequential prompt

```text
SESSION PSPR-20 — Truthful snapshot and rollback

GOAL
Replace in-memory success responses with real, verified ZFS operations and honest failure behavior.

IMPLEMENT
1. Define validated dataset/snapshot identifier types; never interpolate shell commands.
2. Invoke ZFS with direct argv through a narrow adapter, cleared environment, timeout, bounded output, and exact exit handling.
3. Implement create/list/verify/rollback/destroy against the configured dataset and reconcile returned state from ZFS.
4. Require authorization, maintenance state, audit persistence, and explicit destructive confirmation for rollback/destroy.
5. Make unsupported platforms/backends return a typed unsupported error, never success.
6. Handle holds, clones, busy datasets, missing snapshots, partial failure, restart, and concurrent requests.
7. Replace unsupported “secure wipe” claims with cryptographic-erasure and retention semantics.

VERIFY
- Disposable ZFS tests prove state changes on disk and across process restart.
- Fake directory, failed command, timeout, stale metadata, invalid name, and partial operation never return success.
- Rollback restores the expected encrypted fixture and maintains audit continuity.

OUTPUT
Close AF-07B with direct ZFS state evidence and destructive-operation safeguards.
```

### Acceptance criteria

- API success corresponds to observed ZFS state.
- No shell injection, path ambiguity, or metadata-only recovery claim remains.

---

## PSPR-21 — Make vault readiness measure runtime truth before bind

**Closes:** AF-06  
**Primary files:** `mai-api/src/server.rs`, vault builder/readiness, ship validation and negative tests

### Sequential prompt

```text
SESSION PSPR-21 — Runtime-measured vault readiness

GOAL
Prevent listener bind when the vault is merely constructed, uninitialized, plaintext-capable, or misconfigured.

IMPLEMENT
1. Reject Stub/FileDev and every unreviewed backend in ship profile.
2. Build the production vault through the fully wired constructor with crypto, TPM/PCR, audit, storage, and recovery dependencies.
3. Await initialization before readiness publication or socket bind.
4. Measure dataset identity, ZFS encryption/key status, mount properties, crypto/key availability, manifest verification, encrypted write/read round trip, audit persistence, capacity, snapshot capability, and restart recovery.
5. Represent every readiness pass with measured evidence and timestamp; distinguish fail/degraded/unavailable honestly.
6. Make every Critical Fail block all listeners, including alternate HTTP/gRPC startup paths.
7. Add negative controls for missing mount, ordinary directory, wrong dataset/key, PCR drift, tampered manifest, plaintext write, audit failure, and snapshot failure.

VERIFY
- Each claimed pass has a test that forces it to fail.
- No listener binds until all required runtime probes pass.
- `mai-ship-validate` and server startup evaluate the same evidence contract.

OUTPUT
Close AF-06 with the negative matrix, bind-order proof, and real ZFS restart result.
```

### Acceptance criteria

- Constructor/open success is not readiness.
- Production cannot expose vault-dependent APIs before real controls are proven.

---

## PSPR-22 — Require verified signed manifests for every restore

**Closes:** AF-19  
**Primary files:** `tools/mai-admin/src/main.rs`, restore manifest verification, CLI/docs/tests

### Sequential prompt

```text
SESSION PSPR-22 — Restore authenticity gate

GOAL
Prevent tampered or unsigned backup metadata from driving privileged restore.

IMPLEMENT
1. Make `require_signed=true` non-optional for production restore; remove insecure defaults.
2. Require a trusted verification key/policy and fail if absent, unknown, stale, revoked, or wrong-purpose.
3. Verify canonical manifest signature before planning, displaying trusted fields, reading component paths, or touching targets.
4. Bind signature to backup ID, source appliance/tenant, creation time, schema version, component paths/types, hashes, sizes, encryption/key metadata, and compatibility constraints.
5. Reject downgrade, partial signature, extra/missing component, duplicate, ambiguous, and replayed manifests.
6. Separate a clearly unsafe forensic-inspection command that cannot perform restore if unsigned artifact inspection is needed.
7. Update CLI help, automation, and operator documentation.

VERIFY
- Unsigned, signed-without-key, wrong-key, revoked-key, stale, tampered, replayed, and unknown-version manifests fail before restore planning/I/O.
- Valid signed fixture proceeds to the containment checks in PSPR-23.

OUTPUT
Close AF-19 with auth-before-plan proof and CLI compatibility/migration evidence.
```

### Acceptance criteria

- No restore-capable command accepts unverified metadata.
- Verification policy is mandatory and purpose-bound.

---

## PSPR-23 — Contain every restore component under approved roots

**Closes:** AF-11  
**Primary files:** `tools/mai-admin/src/restore.rs`, copy/filesystem helper, traversal tests

### Sequential prompt

```text
SESSION PSPR-23 — Root-contained restore I/O

GOAL
Prevent signed or malicious component paths from reading or writing outside backup and target roots.

IMPLEMENT
1. Validate component paths as relative typed paths with no root/prefix/dot/empty/alternate-stream/ambiguous components.
2. Reject duplicate, ancestor-overlap, target-collision, case-fold, Unicode-normalization, and reserved-name conflicts.
3. Resolve beneath already-open roots using no-follow semantics; reject symlinks, junctions, reparse points, mounts, hardlink tricks, and TOCTOU substitutions.
4. Enforce source and destination containment separately before any recursive copy.
5. Bound component/file count, depth, individual/total bytes, sparse expansion, permissions, ownership, and special file types.
6. Stage atomically, verify hashes/sizes after copy, fsync as required, then publish; cleanly roll back failures.
7. Apply identical containment to dry-run and actual execution.

VERIFY
- Cross-platform corpus covers `..`, absolute/UNC/drive paths, ADS, separators, symlink/reparse races, case/Unicode collisions, deep trees, special files, and oversized content.
- No test can read/write outside disposable roots.
- Valid signed restore completes and post-copy hashes match.

OUTPUT
Close AF-11 with source/destination containment, race, and rollback evidence.
```

### Acceptance criteria

- Authentication and path containment are independent mandatory gates.
- Restore publication is atomic and verified.

---

## PSPR-24 — Build and prove the production OpenBao composition

**Closes:** AF-12 final closure  
**Primary files:** `deployment/appliance/`, OpenBao policy/bootstrap, secrets/TLS/readiness docs and tests

### Sequential prompt

```text
SESSION PSPR-24 — Production trust-root deployment

GOAL
Replace containment with a production-grade, authenticated, encrypted, least-privilege OpenBao deployment.

IMPLEMENT
1. Configure non-dev storage/HA as supported, TLS/mTLS, private networking, health endpoints, and sealed startup.
2. Bootstrap via one-time operator ceremony with no static root token in files/environment after initialization.
3. Create least-privilege policies and separate identities for WSF signing, transit, revocation, AOG, readiness, and administration.
4. Store/inject runtime credentials through an approved secret mechanism with renewal/revocation and no log exposure.
5. Pin configuration/plugins/images and verify provenance.
6. Add backup/recovery, unseal/key rotation, audit-device, failure, and incident procedures.
7. Make production startup fail on dev mode, plaintext, unsealed/unhealthy state, broad policy, missing audit, or known credentials.

VERIFY
- Rendered configuration exposes no known/static root credential or public plaintext trust port.
- Live stack proves least privilege, TLS identity, seal/unseal, restart, rotation, audit, and denied adjacent operations.
- PSPR-01 unsafe fixtures remain rejected.

OUTPUT
Close AF-12 with live effective-config, network exposure, policy, and restart evidence.
```

### Acceptance criteria

- Production OpenBao is neither dev mode nor remotely exposed by default.
- Services possess only their required trust operations.

---

## PSPR-25 — Pin and verify every production deployment artifact

**Closes:** AF-20  
**Primary files:** `deployment/wsf-ha/docker-compose.yml`, all production manifests, build/provenance policy and CI

### Sequential prompt

```text
SESSION PSPR-25 — Immutable supply-chain inputs

GOAL
Prevent registry tag mutation from substituting a trust-plane runtime.

IMPLEMENT
1. Inventory every production base image, service image, plugin, package, binary download, and chart/module reference.
2. Replace tags such as `latest` with immutable digests while retaining human-readable version annotations.
3. Generate SBOMs and signed provenance for project-built artifacts; verify signatures/attestations before deploy.
4. Define an approved digest update workflow with review, vulnerability scan, changelog, rollback digest, and offline bundle support.
5. Make CI/deployment validation reject mutable critical references, unapproved registries, missing provenance, and platform mismatch.
6. Pin multi-architecture manifests intentionally and verify the selected platform digest.
7. Document emergency update and rollback without bypassing verification.

VERIFY
- Production manifests contain no mutable security-critical reference.
- Simulated tag retargeting cannot change deployed content.
- Signature/provenance, SBOM, offline verification, update, and rollback tests pass.

OUTPUT
Close AF-20 with immutable inventory and deployment verification evidence.
```

### Acceptance criteria

- Every production artifact is content-addressed and policy-verified.
- Updating a digest is explicit, reviewed, and reversible.

---

## PSPR-26 — Close adapter runtime isolation on every supported platform

**Closes:** deferred `adapter-resource-isolation-runtime`  
**Primary files:** `mai-adapters/src/process.rs`, deployment/service definitions, platform integration tests

### Sequential prompt

```text
SESSION PSPR-26 — Adapter process isolation proof

GOAL
Resolve the audit’s deferred proof gap for CPU, memory, filesystem, process, and network isolation.

IMPLEMENT
1. Define the supported-platform isolation contract and stop claiming unsupported guarantees.
2. On Linux, enforce cgroup/systemd limits, user/group separation, no-new-privileges, capability drop, filesystem sandbox, process count, timeout, and network policy.
3. On Windows, implement Job Object/token/filesystem/network controls or explicitly disable unsupported production execution.
4. Treat isolation setup failure as fatal before adapter code runs; never fall back silently.
5. Bound NDJSON frames, stdout/stderr, environment, executable/module resolution, restart rate, and child processes.
6. Add hostile adapter fixtures for memory/CPU/fork/disk/network/path/output/crash abuse.
7. Surface measured isolation state in readiness without leaking host details.

VERIFY
- Hostile fixtures hit each limit without affecting sibling tenants or host control plane.
- Isolation setup failure produces zero adapter execution.
- Live Linux and each claimed Windows mode have measured evidence; unsupported modes are rejected.

OUTPUT
Close the deferred row with platform matrix, hostile-fixture results, and honest residual limitations.
```

### Acceptance criteria

- Isolation is enforced by runtime evidence, not architecture prose.
- No supported production platform silently runs unconfined adapters.

---

## PSPR-27 — Implement signed, bounded production update transport

**Closes:** deferred `update-transport-runtime`  
**Primary files:** `mai-core/src/models/update.rs`, `mai-api/src/handlers/updates.rs`, transport/policy tests

### Sequential prompt

```text
SESSION PSPR-27 — Secure update transport and installation

GOAL
Replace the progress stub with an authenticated update path resistant to SSRF, substitution, rollback, and partial installation.

IMPLEMENT
1. Define a signed update manifest with product/channel/version, target platform, artifact digests/sizes, minimum versions, expiry, key ID, rollout constraints, and rollback metadata.
2. Verify manifest signature, trust policy, freshness, monotonic version, and target compatibility before downloads.
3. Allowlist HTTPS origins and resolved destinations; reject userinfo, fragments, redirects across policy, non-HTTPS, IP literals as policy requires, private/link-local/metadata ranges, DNS rebinding, and proxy bypass.
4. Bound redirects, time, bytes, decompression, concurrency, and disk use; stream to a contained staging area.
5. Verify content digest/signature before install and use atomic activation with tested rollback.
6. Integrate package identity rules from PSPR-17 and immutable provenance from PSPR-25.
7. Receipt update decisions without URLs containing credentials or sensitive host data.

VERIFY
- SSRF corpus covers loopback, RFC1918, link-local, cloud metadata, IPv6, redirects, DNS rebinding, alternate schemes, and oversized content.
- Tampered, expired, rollback, wrong-target, partial, and digest-mismatch updates never activate.
- Live local HTTPS harness proves successful signed update and rollback.

OUTPUT
Close the deferred row with transport, signature, containment, atomicity, and rollback evidence.
```

### Acceptance criteria

- Network retrieval and artifact authenticity are both mandatory.
- Failed updates leave the previous verified version operational.

---

## PSPR-28 — Run the live trust-plane adversarial closure suite

**Closes:** live proof for AF-01/02/03/04/05/08/09/10/13/14/15/15B/16/17A/17B  
**Primary files:** integration/e2e harnesses and evidence only, except fixes needed for discovered regressions

### Sequential prompt

```text
SESSION PSPR-28 — Trust-plane black-box closure

GOAL
Prove the repaired network trust boundaries against real services, not mocks alone.

IMPLEMENT
1. Start a clean pinned stack with production-like TLS, OpenBao, WSF, AOG, MAI gRPC, Moto, and available GCP/Azure emulators.
2. Provision two tenants, ordinary/admin/auditor/workload identities, distinct keys/grants/budgets, and deterministic non-sensitive fixtures.
3. Execute every positive flow and every adversarial test named in PSPR-03 through PSPR-16.
4. Add concurrency, restart, seal/partition, revocation propagation, disconnect, timeout, key rotation, and stale-policy scenarios.
5. Verify provider captures, receipts, budgets, logs, and storage contain no secret or regulated fixture material.
6. Reconcile every finding to exact black-box test IDs and archive sanitized commands/versions/results.
7. Fix regressions in the owning prompt’s control; do not weaken the suite.

VERIFY
- All listed findings have a passing positive control and failing negative attack.
- No privileged sink is reached after its expected denial point.
- Receipt and budget state reconcile exactly after failure/restart/concurrency.

OUTPUT
Publish the trust-plane evidence index. Any failed or unavailable required case keeps its finding open.
```

### Acceptance criteria

- Every network/trust finding has live black-box closure.
- Mock-only closure is eliminated.

---

## PSPR-29 — Run storage, package, restore, and migration closure

**Closes:** live proof for AF-06/07A/07B/11/19/DF-01A/DF-01B  
**Primary files:** disposable ZFS/package/restore/migration harnesses and evidence

### Sequential prompt

```text
SESSION PSPR-29 — Privileged filesystem and recovery closure

GOAL
Prove cryptographic storage, real recovery, authenticated packages, contained restore, and safe migrations on real filesystem primitives.

IMPLEMENT
1. Build a disposable ZFS dataset and frozen pre-remediation fixtures without production data.
2. Exercise signed package import, path resolution, encrypted storage, restart/load, snapshot, mutation, rollback, and delete/key-retirement behavior.
3. Exercise signed backup restore with the full traversal/reparse/symlink/size/special-file corpus.
4. Rehearse legacy package, plaintext model, envelope/token where relevant, and backup migrations plus rollback/interruption recovery.
5. Inject missing mount/key, wrong dataset, PCR drift, disk full, audit failure, corrupt ciphertext, snapshot failure, and process restart.
6. Search raw storage/evidence/logs for plaintext fixtures and secrets.
7. Reconcile observed guarantees with API, readiness, and operator documentation.

VERIFY
- Every privileged I/O finding has positive, negative, restart, and migration evidence.
- Raw storage contains no plaintext fixture; traversal cannot affect sentinel files outside roots.
- Readiness fails for every injected broken control before bind.

OUTPUT
Publish the storage/recovery evidence index. Unavailable real ZFS or migration proof keeps affected findings open.
```

### Acceptance criteria

- Storage and recovery claims match real on-disk and post-restart behavior.
- Migration failure is recoverable and never silently weakens protection.

---

## PSPR-30 — Reconcile repository gates, deployment, documentation, and claims

**Closes:** integration/quality/claims milestone  
**Primary files:** CI, deployment, docs, OpenAPI/SDK, release/readiness evidence

### Sequential prompt

```text
SESSION PSPR-30 — Full repository and production-truth gate

GOAL
Make the complete repository, shipped configuration, APIs, and product claims agree with the remediated controls.

IMPLEMENT
1. Run the full verification tier from a clean reproducible environment and repair root causes without suppressions.
2. Run all Rust, Python, adapter, SDK, CLI, dashboard, integrity, deployment, and e2e suites; enumerate intentional skips with owners.
3. Render every production compose/profile and run security validators, secret scans, dependency audit/deny, SBOM, provenance, and immutable-reference checks.
4. Update contracts, OpenAPI, SDKs, CLI help, migrations, operator runbooks, threat model, ship profile, known issues, acquisition/readiness claims, and docs index.
5. Build a claim-to-evidence matrix: each security/production claim maps to code, automated test, live evidence, owner, and expiry/revalidation trigger.
6. Verify no credential, regulated fixture, plaintext weight, or private key exists in tree/evidence/logs; do not rewrite history without separate authority.
7. Review the two former deferred surfaces and all 24 ledger rows for complete receipts.

VERIFY
- Universal gate is green with exact counts and zero unexplained skips/warnings.
- Production rendering and `mai-ship-validate` pass only the secure stack and reject every unsafe fixture.
- Claim-to-evidence matrix has no unsupported “production ready” assertion.

OUTPUT
Issue the remediation candidate report and exact remaining risks. Do not declare release-ready until PSPR-31.
```

### Acceptance criteria

- Code, tests, deployment, APIs, docs, and claims describe one consistent secure product.
- All audit rows are ready for independent verification.

---

## PSPR-31 — Independent re-scan, adversarial review, and release decision

**Closes:** the PSPR  
**Primary files:** new independent scan artifacts, final ledger, release decision

### Sequential prompt

```text
SESSION PSPR-31 — Independent closure and go/no-go

GOAL
Independently verify that remediation is complete and issue an evidence-based release decision.

IMPLEMENT
1. Freeze the candidate revision, working-tree digest, tool/config versions, and complete evidence inventory.
2. Run a new repository-wide Codex Security scan against the frozen candidate; do not reuse this audit’s finding judgments as proof.
3. Require complete high-impact coverage and explicit disposition of every new/deferred row.
4. Independently replay all 24 original attacks plus adapter and update follow-ups against the production-like stack.
5. Review migrations, rollback, incident procedures, key rotation, supply-chain verification, and operator installation from a clean machine.
6. Reconcile new findings with the ledger. Reopen the owning prompt for any regression or incomplete proof.
7. Produce a written go/no-go signed by the owner; record any accepted Medium/Low risk with rationale, owner, deadline, and compensating control.
8. Prepare proposed commit/release scopes, but do not commit or push without separate explicit approval.

VERIFY
- Independent scan reports zero Critical/High and no unexplained incomplete high-impact coverage.
- All original attack replays are denied at the intended control and safely receipted.
- Universal, live, migration, deployment, and integrity gates remain green on the frozen candidate.

OUTPUT
Mark the PSPR complete only after an owner-approved GO. Otherwise issue NO-GO with reopened prompt IDs and exact evidence gaps.
```

### Acceptance criteria

- Zero Critical/High findings remain.
- Every Medium has an explicit owner-approved disposition.
- The owner issues a written GO based on reproducible evidence.

---

## 4. Required adversarial corpus

The following cases are mandatory wherever their boundary applies:

1. Missing, malformed, forged, wrong-key, expired, future, revoked, wrong-issuer, wrong-audience, wrong-tenant, and stale-bundle identity.
2. Authority widening of tenant, subject, service, roles, scopes, models, operations, destinations, classification, caveats, lifetime, budget, and lineage.
3. Concurrent sibling attenuation/spend and replay across replicas/restarts.
4. Cross-tenant/subject/service/audience envelope unseal and label under-classification.
5. Arbitrary/adjacent AWS role, account, action, resource, region, GCP service account/scope, and Azure application/resource.
6. Missing, stale, rolled-back, forked, expired, wrong-scope, and unavailable revocation state.
7. Streaming deny, tokenizer failure, meter failure, receipt failure, timeout, provider error, disconnect, oversized frame, and tool-use event.
8. Anonymous/cross-tenant receipt, usage, and ROI enumeration with guessed identifiers and shared dimensions.
9. Unsigned/substituted/tampered package and restore manifests, extra/missing/duplicate files, and rollback versions.
10. Absolute, drive, UNC, dot-segment, ADS, Unicode/case collision, symlink, junction, reparse, mount, hardlink, and TOCTOU paths.
11. Plain-directory masquerading as ZFS, missing key, wrong dataset, PCR drift, corrupt ciphertext, disk full, snapshot failure, and restart.
12. Mutable tag retargeting, wrong platform digest, missing/invalid provenance, and offline verification failure.
13. Update SSRF through loopback/private/link-local/metadata/IPv6/redirect/DNS rebinding/proxy and oversized/decompression content.
14. Adapter CPU, memory, process, disk, filesystem, network, output, crash-loop, and isolation-setup failure.
15. Secret/regulated payload scans across logs, receipts, panic output, evidence, images, storage, and backups.

---

## 5. Milestone gates

| Milestone | Prompts | Exit gate |
|:--|:--|:--|
| M0 — Baseline and containment | 00–02 | Reproducible ledger; known OpenBao exposure contained; production policy fail-closed |
| M1 — Identity and trust authority | 03–10 | Authenticated principals; monotonic attenuation/revocation; tenant-bound envelope; bounded cloud grants |
| M2 — AOG and evidence isolation | 11–16 | Every provider route policy-dominated; analytics/receipts tenant-scoped |
| M3 — Storage and recovery truth | 17–23 | Signed/contained inputs; encrypted model storage; real snapshots; measured readiness |
| M4 — Deployment and deferred closure | 24–27 | Production OpenBao, immutable supply chain, adapter isolation, secure updates |
| M5 — Integrated proof and release | 28–31 | Live suites green; claims reconciled; independent scan zero Critical/High; owner GO |

No milestone closes while one of its prompts lacks a receipt or required live evidence.

---

## 6. Definition of done

This PSPR is complete only when:

- `PSPR-00` through `PSPR-31` have executed in order and each acceptance criterion is evidenced;
- all 24 validated audit findings have root-fix and live/final closure receipts;
- both explicitly deferred runtime areas are closed with measured evidence;
- the complete verification tier passes from the frozen candidate with zero unexplained skips or suppressions;
- trust-plane and storage/recovery black-box suites pass against real required services;
- migrations, restart, rollback, key rotation, failure injection, and operator recovery are reproducible;
- production manifests contain no known/default credential, dev trust core, unrestricted security bind, or mutable critical artifact;
- documentation and external readiness claims map one-to-one to observed controls;
- an independent repository-wide scan reports zero Critical/High findings and complete high-impact coverage;
- every remaining Medium/Low risk has an owner-approved disposition; and
- the owner records a written GO.

No finding is closed by documentation alone, by a unit test alone where a live boundary exists, or by a readiness flag that does not measure the claimed runtime property.
