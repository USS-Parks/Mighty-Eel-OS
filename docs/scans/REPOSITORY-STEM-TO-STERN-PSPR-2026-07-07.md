# Mighty Eel MAI — Canonical Stem-to-Stern Plan / Sequential Prompt Roster

**Created:** 2026-07-07  
**Baseline revision:** `6ffaaeeea0a83c7fa071e114183cfa60c5898703`  
**Audit authority:** `REPOSITORY-STEM-TO-STERN-AUDIT-2026-07-07.md`  
**Security trace authority:** repository-root `AUDIT_REPORT.md`  
**Status:** READY FOR USER APPROVAL; execution has not started  
**Release posture:** STOP-SHIP until STS-45 closes

## 0. Canonical authority

This is the sole top-level execution roster for the 2026-07-07 repository audit. It subsumes the scope of the untracked `SECURITY-REMEDIATION-PSPR.md` and `REPOSITORY-SECURITY-REMEDIATION-PSPR-2026-07-07.md` while adding functional truth, CI, scanner, documentation, governance, and live-proof work. Those files remain evidence until explicitly marked superseded; they must not be executed as parallel plans.

“Stem to stern” means the sequence runs from baseline and containment through architecture, implementation, migration, clean-checkout gates, live appliance proof, independent re-scan, and release disposition. It does not mean every session is pre-authorized to commit, push, deploy, rotate a real credential, or mutate an external service.

## 1. Universal execution contract

Every session executor must obey all of the following:

1. Work from `C:\Users\17076\Documents\Claude\Mighty Eel OS\mai`; never re-nest the repository.
2. Read root `AGENTS.md`, this roster, the audit, and the previous session receipt before editing.
3. Treat all pre-existing modifications and untracked files as user-owned. Never discard or overwrite them.
4. Execute exactly one numbered prompt at a time. Do not opportunistically start a later prompt.
5. Resolve the current HEAD and record it in the session receipt. If the baseline changed, revalidate affected evidence.
6. Add a failing regression test before or with each behavioral/security fix. A test that cannot fail on the old behavior is not closure evidence.
7. Reuse one canonical application service for equivalent REST, gRPC, WebSocket, OpenAI, and Anthropic operations; do not clone policy logic into handlers.
8. Fail closed at trust boundaries. Caller-provided tenant, role, audience, policy, path, model ID, or cloud identity is a request, never authority.
9. Never weaken, skip, allowlist, or mark a gate optional merely to obtain green output. Narrow suppressions require a documented false-positive proof.
10. Never print or copy secrets into receipts, logs, fixtures, prompts, or documentation. Record only paths, rule IDs, and redacted fingerprints.
11. Run targeted tests first, then the session gate. Record exact commands, exit codes, pass/skip counts, and any unavailable external dependency.
12. Run `.integrity/scripts/verify-tree.sh <changed-files>` through Git Bash before proposing staging.
13. Do not run `git commit` or `git push`. Before either action, summarize staged files/changes and ask the user separately, exactly as root policy requires.
14. Stop on an architectural contradiction, missing migration path, unexplained test regression, real secret exposure, destructive data operation, or need for production credentials.
15. Do not call a finding closed until implementation, negative tests, positive tests, operational docs, and required live evidence all agree.

## 2. Required session receipt

At the end of every prompt, append a receipt to `docs/scans/sts/STS-EXECUTION-LEDGER.md` containing:

- session ID, title, UTC and local timestamps, starting and ending HEAD;
- audit finding IDs addressed and their disposition (`open`, `implemented`, `live-proof-pending`, `closed`, or `risk-accepted`);
- files changed, migrations introduced, and public contract changes;
- tests added and exact verification commands/results;
- security invariants checked and adversarial cases exercised;
- deferred work, blockers, and rollback procedure;
- `git status --short` output summary;
- explicit statement: `No commit or push performed`, unless the user separately approved one.

## 3. Global invariants

The following must remain true after every session:

- No unauthenticated path can issue, attenuate, exchange, unseal, query, restore, install, update, or administer authority.
- Authenticated principal and tenant are derived server-side and propagated immutably.
- Attenuation is restriction-only, parent-authenticated, expiry-bounded, and auditable.
- Revocation is issuer/tenant scoped, fresh, monotonic, anti-rollback, and fail-closed in production.
- Every provider path applies the same ingress policy, egress policy, metering, receipt, cancellation, and error semantics.
- Every filesystem operation is contained beneath a canonical approved root after decoding and normalization.
- A signature covers the complete semantic identity consumed after verification.
- Production readiness proves the actual configured runtime dependency; construction is not readiness.
- A success state means the promised side effect completed and was verified.
- Development modes, mock servers, and known credentials cannot be selected by a production profile.
- Release artifacts are immutable, reproducible, signed, SBOM-attested, and provenance-bound.

## 4. Audit-to-session closure matrix

| Finding | Primary session | Required closure evidence |
|---|---:|---|
| SEC-01 caller-selected AWS role | STS-11 | Named grants + denied arbitrary ARN live test |
| SEC-02 unauthenticated WSF issuance | STS-08 | Authenticated issuer + tenant derivation tests |
| SEC-03 AOG shadow default | STS-15 | Production fail-closed configuration tests |
| SEC-04 legacy completions bypass | STS-20 | Cross-surface policy/meter/receipt parity |
| SEC-05 known-token dev OpenBao | STS-02, STS-29 | Containment + production composition proof |
| SEC-06 partial package signature | STS-23 | Canonical signed manifest identity |
| SEC-07 unauthenticated attenuation | STS-09 | Parent verification + monotonic caveats |
| SEC-08 unbound unseal | STS-13 | Context-bound AEAD/signature verification |
| SEC-09 caller-authored gRPC admin | STS-10 | Transport-authenticated principal |
| SEC-10 OpenAI stream bypass | STS-17, STS-18 | Incremental governed stream receipts |
| SEC-11 Anthropic stream bypass | STS-17, STS-19 | Incremental governed stream receipts |
| SEC-12 unsigned restore | STS-27 | Mandatory trusted manifest verification |
| SEC-13 model ID path escape | STS-24 | Opaque contained storage identity |
| SEC-14 absent revocation fail-open | STS-14 | Production denial on absent/stale state |
| SEC-15 cross-tenant ROI | STS-21 | Authenticated tenant-scoped query tests |
| SEC-16 restore path escape | STS-28 | Canonical component containment tests |
| SEC-17 constructed-vault readiness | STS-26 | Live storage/crypto probe before bind |
| SEC-18 cross-tenant usage | STS-21 | Tenant-scoped aggregation tests |
| SEC-19 overlong cloud credentials | STS-12 | TTL bounded to remaining authority |
| SEC-20 open receipt queries | STS-22 | Authn/authz/tenant scope + pagination caps |
| SEC-21 fake ZFS snapshot/rollback | STS-26 | Real commands + postcondition proof |
| SEC-22 plaintext ZFS weights | STS-25 | Authenticated encryption + migration |
| SEC-23 mutable deployment images | STS-30 | Digest pins + provenance/SBOM checks |
| SEC-24 weak revocation snapshots | STS-14 | Scope/freshness/sequence/signature model |
| FUN-01 false model Loaded state | STS-31 | Transactional verified placement |
| FUN-02 zero-output WebSocket | STS-32 | Real governed stream parity |
| FUN-03 placeholder gRPC stream/embed | STS-33 | Real backend output and parity tests |
| FUN-04 fabricated metadata/profile/scan | STS-34 | Source-of-truth repositories |
| FUN-05 IPC receiver ownership | STS-35 | Bounded request demultiplexer |
| FUN-06 dual schedulers | STS-36 | One lifecycle authority |
| BLD-01/02 red static gates | STS-03 | Green Clippy, Ruff, Bandit |
| BLD-03 Python environment drift | STS-05 | One clean-checkout command |
| CI-01 console absent from CI | STS-39 | Locked typecheck/build/test job |
| SUP-01 scanner/allowlist drift | STS-04 | Narrow policy + zero unreviewed hits |
| SUP-02 dependency debt | STS-41 | Reviewed policy and upgrade disposition |
| DOC-01 165 broken links | STS-06, STS-40 | Automated zero-broken-link gate |
| GOV-01/02/03 governance ambiguity | STS-01, STS-40 | One dated authority chain |

## 5. Sequential execution index

| Phase | Sessions | Exit gate |
|---|---|---|
| A — Baseline, governance, containment | STS-00..06 | Reproducible ledger; immediate hazards contained; basic gates green |
| B — Identity and trust primitives | STS-07..15 | One authenticated principal, issuance, attenuation, revocation, and enforcement model |
| C — Protocol and data-plane convergence | STS-16..22 | Every protocol/provider path has policy, meter, receipt, and tenant parity |
| D — Package, vault, restore, deployment | STS-23..30 | Complete signed identity, contained paths, encrypted storage, real readiness, immutable artifacts |
| E — Functional truth and concurrency | STS-31..38 | No advertised placeholder-success surfaces; isolation and update transport closed |
| F — CI, docs, dependency reconciliation | STS-39..41 | Clean checkout and documentation graph are green; dependency debt adjudicated |
| G — Live proof and release | STS-42..45 | Live closure, full evidence, independent re-scan, explicit release decision |

## 6. Sequential prompts

### STS-00 — Freeze the baseline and create the execution ledger

**Prompt:** Resolve HEAD, branch, remotes, tracked/untracked state, toolchain versions, workspace members, and the exact audit artifact hashes. Create `docs/scans/sts/STS-EXECUTION-LEDGER.md`, `FINDING-STATUS.md`, and `EVIDENCE-MANIFEST.json` without modifying product code. Record pre-existing user changes separately from planned changes. Define stable IDs for all findings and evidence bundles.

**Acceptance:** The baseline can be reconstructed; every finding maps to one primary session; no user-owned file is overwritten; the receipt records that no commit/push occurred.

**Verification:** `git status --short --branch`; `git rev-parse HEAD`; `cargo metadata --no-deps`; hash the audit and roster; integrity-check the three new files.

### STS-01 — Repair governance authority and canonical status

**Prompt:** Determine whether missing root `RTK.md` should be restored or the stale include removed. Reconcile root `AGENTS.md`, `CLAUDE.md`, `docs/INDEX.md`, deployment roadmap, and current lane status into one dated authority chain. Mark competing security rosters as superseded by this roster without deleting evidence.

**Acceptance:** A new agent can load every referenced governing file; DOUGHERTY/RC/GITDOCTOR/IGD/security states do not conflict; all canonical paths exist.

**Verification:** Automated referenced-file check over governance documents; manual status cross-check against Git history; zero broken authority links.

### STS-02 — Contain development OpenBao and known credentials

**Prompt:** Prevent any production-like profile or composition from starting development OpenBao, using a known root token, or publishing its listener. Separate demo composition by filename, profile, network, and explicit warning. Inventory and rotate/revoke any credential that may have escaped its intended local scope; external rotation requires user authorization.

**Acceptance:** Production startup rejects dev mode and known/default tokens; demo startup is opt-in and loopback/private-network constrained; no secret is written to receipts.

**Verification:** Negative production configuration tests; Compose config inspection; Gitleaks/detect-secrets; authorized live token revocation evidence if applicable.

### STS-03 — Restore the mandatory static gates

**Prompt:** Fix the Clippy documentation continuation and make `deployment/appliance/mock-llm/app.py` typed and explicit about bind policy. If the mock must bind all container interfaces, configure the address, restrict exposure in Compose, document test-only scope, and use the narrowest justified scanner suppression.

**Acceptance:** Workspace Clippy, full Ruff, and Bandit pass with no broad exclusions; mock behavior remains covered by tests.

**Verification:** `cargo clippy --workspace -- -D warnings -A clippy::pedantic`; `python -m ruff check .`; Bandit with repository config; mock unit/smoke test.

### STS-04 — Rebuild secret-scanner policy around tracked truth

**Prompt:** Classify every Gitleaks and detect-secrets hit without revealing values. Remove the broad `deployment/*-staging/` allowlist or split tracked templates from ignored generated state. Rebase moved GitDoctor evidence paths. Add narrow path/regex allowlists only for proven fixtures, and add a test that fails if an ignored path contains tracked files.

**Acceptance:** Zero unreviewed findings; tracked staging files are scanned; ignored key material stays untracked with documented ACL/cleanup requirements; CI and local scanners use the same policy.

**Verification:** `git ls-files deployment/*-staging`; Gitleaks no-git scan; detect-secrets serial scan with reviewed baseline; negative seeded-secret policy test.

### STS-05 — Define one clean-checkout Python and repository test contract

**Prompt:** Make a clean checkout able to run the full Python suite without a hand-set `PYTHONPATH`. Choose a documented editable install, tox/nox/uv environment, or wrapper that installs the SDK and test extras deterministically. Preserve scoped fast CI jobs but add a full collection gate.

**Acceptance:** One documented command collects and runs every intended Python test; optional live tests skip with explicit reasons; no import depends on stale paths or global packages.

**Verification:** Fresh virtual environment; full pytest; SDK/adapters mypy; Ruff; record pass/skip counts.

### STS-06 — Install a tracked Markdown link gate and baseline repairs

**Prompt:** Add a deterministic local-link checker that understands URL escaping, directory links, anchors, and repository-root references. Exclude generated/untracked build trees. Use it to classify the 165 broken links and repair the canonical README, INDEX, HANDOFF, operations, incident, and runbook front doors first.

**Acceptance:** The checker is tested and CI-ready; all release-critical documentation links pass; remaining historical-link work is enumerated for STS-40.

**Verification:** Link checker unit tests; checker run over tracked Markdown; manual sample of repaired operational links.

### STS-07 — Introduce the canonical authenticated principal and request context

**Prompt:** Define one immutable server-derived principal/context type carrying subject, tenant, issuer, authenticated roles/grants, token ID, audience, policy version, expiry, and trace ID. Specify trusted constructors for HTTP, gRPC, service identity, and offline verification; forbid handler construction from raw metadata/body fields.

**Acceptance:** All privileged services accept the canonical context; caller-controlled authority fields are absent or treated only as requested values; serialization does not expose secrets.

**Verification:** Constructor negative tests, tenant-confusion tests, and compile-time migration inventory for every privileged handler.

### STS-08 — Authenticate WSF issuance and derive authority server-side

**Prompt:** Require the canonical principal for token issuance. Load tenant/workload grants from trusted server-side state, intersect requested claims with grants, enforce audience and maximum TTL, and write an issuance receipt. Remove public caller-selected tenant/role authority.

**Acceptance:** Anonymous issuance, cross-tenant issuance, role escalation, excessive TTL, and unknown audience all fail; valid issuance remains interoperable.

**Verification:** Unit, API, and live OpenBao issuance tests; receipt-chain assertion; SEC-02 closure trace.

### STS-09 — Make attenuation parent-authenticated and restriction-only

**Prompt:** Verify parent signature, issuer, audience, expiry, revocation, tenant, and chain before attenuation. Construct the child by monotonic intersection: no new actions/resources/roles/tenants, no later expiry, and an incremented bounded depth. Bind and receipt the parent-child relationship.

**Acceptance:** Forged parent, widened scope, tenant swap, expiry extension, removed caveat, excessive depth, and revoked parent all fail.

**Verification:** Property tests for subset/expiry monotonicity; adversarial token corpus; SEC-07 closure trace.

### STS-10 — Replace gRPC metadata roles with transport-authenticated identity

**Prompt:** Remove authorization decisions based on caller-authored administrator/profile metadata. Build canonical context from mTLS/service identity or a verified bearer token interceptor and apply the same authorization service as HTTP.

**Acceptance:** Spoofed metadata never grants access; HTTP and gRPC produce equivalent allow/deny results and tenant bindings.

**Verification:** gRPC interceptor tests, spoofing tests, cross-protocol authorization table, SEC-09 closure trace.

### STS-11 — Replace arbitrary cloud role ARNs with named grants

**Prompt:** Change credential exchange to accept a server-defined grant ID. Resolve provider account/project/subscription, role/service account, external ID, region, actions, and maximum TTL from tenant/workload policy. Log the resolved grant without credentials.

**Acceptance:** Arbitrary ARN/account/project/subscription input is impossible or rejected; grant ownership is tenant-bound; disabled grants fail closed.

**Verification:** Broker unit tests plus live Moto/LocalStack and provider-emulator tests; SEC-01 closure trace.

### STS-12 — Bound brokered credentials to remaining token authority

**Prompt:** Compute credential TTL as the minimum of requested TTL, grant cap, provider cap, remaining verified token lifetime, and policy cap, with clock-skew safety. Reject authority too near expiry and ensure refresh reauthorizes from scratch.

**Acceptance:** No credential outlives its authorizing token; zero/negative/overflow lifetimes fail; refresh cannot extend revoked authority.

**Verification:** Boundary/property tests with controlled clocks; live provider TTL assertion; SEC-19 closure trace.

### STS-13 — Bind sealed envelopes to the full verification context

**Prompt:** Define a versioned canonical envelope header containing tenant, subject/owner, audience, purpose/operation, policy version, key ID, expiry, and nonce. Authenticate the header as AEAD associated data and verify it against canonical context before plaintext release. Design versioned migration/reseal.

**Acceptance:** Cross-tenant, cross-subject, cross-audience, cross-operation, stale-policy, replay, tampered-header, and wrong-key unseal attempts fail.

**Verification:** Round-trip and mutation matrix; live OpenBao seal/unseal; SEC-08 closure trace.

### STS-14 — Make revocation scoped, fresh, monotonic, and fail-closed

**Prompt:** Redesign signed snapshots with issuer, tenant, audience, generated/expiry times, monotonic sequence, previous digest, and key ID. Persist last accepted sequence/digest, reject rollback and scope mismatch, define bounded offline grace, and deny production requests when required state is absent/stale.

**Acceptance:** Missing, stale, replayed, rolled-back, wrong-tenant, wrong-issuer, and invalid-signature snapshots deny; cache restart preserves anti-rollback state.

**Verification:** State-machine/property tests, restart tests, offline-grace tests, live revocation propagation; SEC-14 and SEC-24 closure traces.

### STS-15 — Make AOG production enforcement explicit and fail-closed

**Prompt:** Replace shadow-by-default behavior with profile-aware configuration: production requires enforce mode and validated policy/revocation dependencies; shadow is development-only, prominently observable, and cannot satisfy readiness. Define kill-switch precedence.

**Acceptance:** Missing/invalid mode cannot start production; shadow responses and receipts are unmistakable; dependency loss denies according to policy.

**Verification:** Configuration matrix, readiness tests, policy-mode integration tests, SEC-03 closure trace.

### STS-16 — Create one mandatory provider-dispatch application service

**Prompt:** Build a shared request pipeline for authentication, tenant binding, ingress classification/policy, provider selection, budget admission, dispatch, egress inspection, metering, receipts, and cleanup. All external surfaces must call it; direct provider calls become private or test-only.

**Acceptance:** Static route inventory shows no bypass; each stage has typed input/output and fail-closed error semantics; cancellation cleans budgets and resources.

**Verification:** Pipeline stage tests, bypass-search test, protocol conformance harness.

### STS-17 — Implement governed incremental streaming primitives

**Prompt:** Add bounded stream framing that tokenizes/counts incremental output, applies egress policy before release, records final/aborted receipts, enforces budgets mid-stream, propagates cancellation, and handles provider framing errors without leaking unreviewed text.

**Acceptance:** Streaming and non-streaming totals reconcile; denied chunks are not emitted; disconnect, timeout, policy denial, and provider error all receipt correctly.

**Verification:** Fragmentation/fuzz tests, backpressure tests, cancellation tests, metering reconciliation.

### STS-18 — Route OpenAI chat streaming through the mandatory pipeline

**Prompt:** Adapt OpenAI-compatible SSE to STS-16/17 without parsing or policy logic in the handler. Preserve compatible framing while deriving usage and finish reason from governed state.

**Acceptance:** No direct OpenAI provider stream bypass remains; ingress/egress policy, meter, budget, and receipt parity are proven.

**Verification:** OpenAI surface integration and adversarial stream tests; SEC-10 closure trace.

### STS-19 — Route Anthropic streaming through the mandatory pipeline

**Prompt:** Adapt Anthropic event streaming to STS-16/17, including event ordering, partial JSON/tool blocks, usage, cancellation, and error mapping. Normalize internally without weakening Anthropic contract behavior.

**Acceptance:** No direct Anthropic stream bypass remains; malformed/partial events fail safely; parity with OpenAI governance is proven.

**Verification:** Anthropic surface integration and adversarial stream tests; SEC-11 closure trace.

### STS-20 — Route legacy OpenAI completions through the mandatory pipeline

**Prompt:** Implement legacy completions as a compatibility adapter over the same canonical request service or remove it from production. Do not retain a separate provider/accounting path.

**Acceptance:** Policy, tenant, budget, meter, and receipt behavior matches chat completion for equivalent input; unsupported fields fail explicitly.

**Verification:** Compatibility matrix and bypass regression test; SEC-04 closure trace.

### STS-21 — Scope usage and ROI analytics to authenticated tenancy

**Prompt:** Require canonical context in usage and ROI services. Apply tenant predicates at repository/query boundaries, authorize any fleet-wide role server-side, and suppress small-cell or sensitive cross-tenant inference as policy requires.

**Acceptance:** Ordinary users cannot observe or influence other tenants; fleet admins are explicit and audited; pagination/export preserve scope.

**Verification:** Multi-tenant fixtures, query-shape assertions, cross-tenant denial tests; SEC-15/18 closure traces.

### STS-22 — Authenticate and constrain receipt queries

**Prompt:** Require canonical context for receipt lookups, derive tenant scope, authorize privileged audit roles, cap ranges/page sizes, bind cursors to filters, and audit access to audit data.

**Acceptance:** Anonymous, cross-tenant, cursor-tampering, oversized, and enumeration requests fail; chain-verification endpoints do not disclose unrelated receipts.

**Verification:** API and repository tests, pagination fuzzing, live ledger query; SEC-20 closure trace.

### STS-23 — Sign the complete model-package semantic identity

**Prompt:** Define deterministic canonical bytes covering manifest version, model identity, all file paths/hashes/sizes, compatibility, license/tier, policy/safety metadata, encryption metadata, and rollback/version constraints. Verify before trusting any field and reject ambiguous/duplicate encodings.

**Acceptance:** Changing any consumed manifest field invalidates the signature; duplicate keys, alternate normalization, unknown critical fields, and file substitution fail.

**Verification:** Golden vectors, mutation/property tests, cross-tool builder/verifier test; SEC-06 closure trace.

### STS-24 — Replace free-form model IDs with contained storage identity

**Prompt:** Separate display/model IDs from filesystem keys. Validate canonical component syntax, reject separators/dot segments/absolute/UNC/device/alternate-data-stream forms, resolve beneath an approved root, and recheck containment before every operation.

**Acceptance:** Traversal corpus fails on Windows and Unix semantics; legitimate IDs remain stable; symlink/reparse-point escapes are addressed.

**Verification:** Cross-platform path corpus and package-install integration tests; SEC-13 closure trace.

### STS-25 — Encrypt and authenticate model weights at rest

**Prompt:** Replace raw ZFS weight reads/writes with chunked authenticated encryption tied to package/model/tenant identity and managed keys. Define atomic write, crash recovery, key rotation, integrity verification, zeroization boundaries, and migration from existing plaintext.

**Acceptance:** New production writes are never plaintext; tampering and wrong context fail before model use; migration is resumable and rollback-safe.

**Verification:** Ciphertext inspection, tamper tests, interrupted migration, rotation tests, live dataset proof; SEC-22 closure trace.

### STS-26 — Implement real ZFS operations and truthful vault readiness

**Prompt:** Implement bounded dataset/snapshot list/create/delete/rollback through a narrow command runner or API with exact argv, timeout, allowlisted dataset, and postcondition checks. Make production readiness verify mounted dataset, writable atomic I/O, crypto round-trip, snapshot capability, capacity, and audit append/read.

**Acceptance:** No production snapshot/rollback path returns success without a real verified operation; constructed objects cannot satisfy readiness; failures prevent socket bind.

**Verification:** Fake-runner unit tests, live disposable-ZFS tests, readiness fault injection; SEC-17/21 closure traces.

### STS-27 — Require a trusted signed restore manifest

**Prompt:** Make restore verification mandatory. Bind signature to backup identity, appliance/tenant scope, creation time, schema, components, hashes/sizes, encryption/key metadata, and compatibility. Verify trust and rollback policy before creating target state.

**Acceptance:** Unsigned, unknown-key, expired, wrong-appliance, downgraded, or mutated backups fail without partial restore.

**Verification:** Restore mutation matrix, key-rotation trust tests, SEC-12 closure trace.

### STS-28 — Contain restore components and make restore transactional

**Prompt:** Canonicalize every source and destination component beneath approved roots, reject links/reparse points and special files as policy dictates, enforce size/count quotas, extract into staging, verify, then atomically activate with rollback.

**Acceptance:** Traversal, absolute path, encoded separator, link escape, overwrite, decompression bomb, and partial-failure attacks fail safely.

**Verification:** Malicious archive corpus on Windows/Linux semantics, interrupted restore tests; SEC-16 closure trace.

### STS-29 — Build the production OpenBao composition

**Prompt:** Create a non-dev OpenBao deployment with TLS/mTLS as designed, persistent storage, initialized/unsealed operational procedure, least-privilege AppRoles/policies, secret injection, audit devices, health checks, rotation, backup, and no published dev root token. Keep demo composition separate.

**Acceptance:** Production profile cannot start dev mode; bootstrap secrets are never committed/logged; MAI authenticates with least privilege; restart/rotation/revocation are proven.

**Verification:** Compose/config validation and authorized live rehearsal; SEC-05 final closure trace.

### STS-30 — Pin, sign, attest, and reproduce deployment artifacts

**Prompt:** Pin every production base/runtime image and external artifact by immutable digest/version; update through reviewed automation; generate SBOM and provenance; verify signatures/attestations before use; prove clean rebuild reproducibility within documented limits.

**Acceptance:** No mutable production tag remains; digest updates are visible; release manifests bind all artifacts and SBOMs.

**Verification:** Pin scanner, image-signature verification, SBOM diff, two clean builds; SEC-23 closure trace.

### STS-31 — Make model loading transactional and truthful

**Prompt:** Redesign model load so bytes are verified/decrypted, resources reserved, adapter/backend placement completed, health proven, and only then state changes to `Loaded`. Define intermediate states, idempotency, cancellation, crash recovery, and rollback.

**Acceptance:** `Loaded` implies a usable backend instance; every failure releases resources and publishes a truthful terminal state.

**Verification:** State-machine/property tests, fault injection at every step, real adapter smoke test; FUN-01 closure.

### STS-32 — Implement real WebSocket inference parity

**Prompt:** Replace immediate zero-token completion with the canonical pipeline and streaming primitive. Support auth, scheduling, token events, backpressure, cancellation, timeout, policy denial, usage, finish reasons, and receipts.

**Acceptance:** Equivalent REST/SSE/WebSocket requests produce equivalent governed outcomes; no zero-output placeholder success remains.

**Verification:** Protocol parity, disconnect/backpressure, and adversarial egress tests; FUN-02 closure.

### STS-33 — Implement real gRPC streaming and embeddings

**Prompt:** Route gRPC chat streaming and embeddings through canonical services and real adapter backends. Enforce model capability, input limits, dimensions, ordering, cancellation, auth, policy, budget, meter, and receipt semantics.

**Acceptance:** No synthetic single-chunk or empty-vector success remains; REST/gRPC equivalence holds.

**Verification:** Real/mock adapter integration, vector contract tests, protocol parity; FUN-03 closure.

### STS-34 — Replace fabricated profile, registry, and model metadata

**Prompt:** Implement authoritative repositories for profiles and model manifests. Make scan actually discover/validate models; source safety/default metadata from signed manifests/policy; return unavailable/unsupported instead of fabricated values. Migrate interim JSON profile data to the chosen durable store.

**Acceptance:** Every returned field has a traceable source; admin and tenant scopes are correct; scan count reflects verified discoveries; migrations are idempotent.

**Verification:** Repository/migration tests, multi-tenant API tests, signed-manifest metadata tests; FUN-04 closure.

### STS-35 — Build a concurrency-safe adapter IPC demultiplexer

**Prompt:** Give one task ownership of the adapter event receiver and demultiplex by request ID into bounded channels. Handle unknown/late/duplicate events, adapter death, timeouts, cancellation, cleanup, and per-request backpressure without head-of-line authority leaks.

**Acceptance:** Concurrent callers cannot consume each other's events; abandoned requests do not leak channels/resources; adapter restart fails in-flight work deterministically.

**Verification:** High-concurrency stress, reorder/duplicate/fault injection, loom-style reasoning where practical; FUN-05 closure.

### STS-36 — Converge hot-swap and lifecycle on one scheduler

**Prompt:** Migrate `HotSwapManager` from the legacy scheduler to the canonical scheduler trait/state. Define atomic pause, drain, unload, load, health, resume, rollback, and admission behavior with one source of lifecycle truth.

**Acceptance:** No second scheduler is constructed for production lifecycle; failures restore a safe routable state or stay explicitly unavailable.

**Verification:** Lifecycle integration, concurrent admission/hot-swap, rollback and restart tests; FUN-06 closure.

### STS-37 — Close adapter runtime isolation on supported platforms

**Prompt:** Define and enforce the adapter threat boundary: least-privilege user, filesystem roots, environment allowlist, network policy, resource caps, process tree cleanup, IPC authentication, executable provenance, and platform-specific sandbox controls. Production must reject unsupported isolation claims.

**Acceptance:** A malicious adapter fixture cannot read protected files, inherit secrets, escape allowed network/process/resource boundaries, or impersonate another adapter.

**Verification:** Linux and Windows isolation harnesses, negative capability tests, crash/kill cleanup evidence.

### STS-38 — Implement signed, bounded production update transport

**Prompt:** Complete update transport with signed canonical metadata, TLS/pinned trust as policy requires, bounded redirects/timeouts/size, resumable verified shards, rollback/freeze protection, atomic activation, and air-gap removable-media equivalence. No manifest field may be trusted before signature verification.

**Acceptance:** Tampered, stale, rollback, wrong-tier/license, redirect, oversized, partial, and mirror-substitution updates fail safely; recovery is documented.

**Verification:** Update adversarial corpus, interrupted activation rollback, online/offline parity, live staging rehearsal.

### STS-39 — Make clean-checkout CI cover every shipped language and surface

**Prompt:** Add locked console `npm ci`, typecheck, build, tests, and audit to CI; add the canonical full Python collection command; retain Rust fmt/check/clippy/test, packaging, integrity, secret, dependency, and link gates. Pin CI actions and remove stale “future phase” comments.

**Acceptance:** A clean checkout on supported runners runs all required gates; no job silently continues on error; artifacts/logs are retained without secrets.

**Verification:** Local CI-command rehearsal and a user-authorized remote CI run; CI-01/BLD-03 closure.

### STS-40 — Reconcile the complete documentation and claims graph

**Prompt:** Repair all remaining tracked Markdown links, rebase links after directory moves, mark historical plans clearly, update API/SDK/operations/runbooks to match implemented behavior, and remove or qualify claims lacking live evidence. Ensure the index points to this roster and current status ledger.

**Acceptance:** Zero broken local links; no production claim contradicts code/readiness; historical docs cannot be mistaken for current instructions.

**Verification:** Link/anchor checker over tracked Markdown, API route/doc diff, runbook command smoke checks; DOC-01/GOV closure.

### STS-41 — Adjudicate dependency, license, and duplicate-graph debt

**Prompt:** Resolve or time-bound the unmaintained `proc-macro-error2` chain, review duplicate major/minor dependency families for risk/size, remove unused deny allowances, and align Cargo/Python/npm audit policies with release requirements. Do not upgrade blindly across protocol/crypto boundaries.

**Acceptance:** Every warning has a fix or named risk owner/expiry; vulnerability policy is explicit; lockfiles are deterministic and reviewed.

**Verification:** Cargo audit/deny, Python audit, npm audit, SBOM/license scan, targeted regression tests; SUP-02 closure.

### STS-42 — Run the live trust-plane adversarial closure suite

**Prompt:** In an isolated authorized environment, run real OpenBao and cloud emulators/providers through issuance, attenuation, revocation, seal/unseal, credential exchange, provider dispatch, streaming, receipts, analytics, and identity rotation. Exercise the required negative corpus from every SEC finding.

**Acceptance:** All trust-plane security findings have live positive and negative evidence; no test uses production credentials; logs are redacted and hash-manifested.

**Verification:** Live suite receipts, service audit logs, evidence hashes, restart/rotation/failure-injection results.

### STS-43 — Run storage, restore, package, appliance, and hardware closure

**Prompt:** On disposable representative infrastructure, prove ZFS encryption/snapshots/rollback, signed package install, malicious package rejection, backup/restore containment, migration, TPM/key behavior, GPU/model placement, adapter isolation, update rollback, air-gap behavior, and production OpenBao composition.

**Acceptance:** Every promised hardware/storage control is observed, not mocked; destructive tests target disposable datasets only; recovery succeeds from injected failures.

**Verification:** Signed evidence bundle with commands, versions, redacted logs, hashes, before/after state, and operator attestation.

### STS-44 — Execute the complete clean-room release gate

**Prompt:** From a fresh clone at the candidate revision, run formatting, compilation, Clippy, all Rust/Python/console tests, mypy, Ruff, Bandit, secret scans, dependency audits, deny/license checks, integrity/no-slop, link checks, packaging, SBOM/provenance, appliance smoke, and deterministic artifact comparison.

**Acceptance:** Every mandatory gate passes; skips are limited to explicitly separate live/hardware evidence already satisfied in STS-42/43; artifact hashes and tool versions are recorded.

**Verification:** Machine-readable gate manifest and immutable evidence index; no working-tree contamination.

### STS-45 — Independent re-scan and explicit release decision

**Prompt:** Have an independent reviewer or clean-context agent re-scan the candidate without relying on remediation claims. Rebuild source/control/sink traces, compare against all finding IDs, audit evidence completeness, and issue one decision: GO, CONDITIONAL GO with named time-bounded risk acceptance, or NO-GO.

**Acceptance:** No unresolved critical/high finding; every medium has closure or explicit authorized acceptance; documentation and production profile match evidence; the user makes the release decision.

**Verification:** Independent report, finding-diff matrix, final `git status`, candidate revision/hash, and signed release-decision record. Do not commit, push, tag, publish, or deploy without separate user approval.

## 7. Required adversarial corpus

The following tests are mandatory across the applicable sessions:

- anonymous, malformed, expired, wrong-audience, wrong-issuer, wrong-tenant, revoked, and replayed tokens;
- forged parent attenuation, widened actions/resources/roles, tenant swap, expiry extension, and depth exhaustion;
- arbitrary cloud identities, cross-tenant grants, excessive TTL, refresh after revocation, and clock-boundary cases;
- envelope header mutation, context swapping, nonce replay, stale policy, wrong key, and ciphertext tampering;
- absent/stale/rolled-back/wrong-scope revocation snapshots and restart persistence;
- provider fragmentation, partial JSON/tool calls, malformed frames, disconnect, slow consumer, budget exhaustion, and denied egress;
- analytics/receipt enumeration, cursor tampering, oversized ranges, and fleet-role spoofing;
- package duplicate keys, normalization ambiguity, manifest mutation, file substitution, traversal, absolute/UNC/device/ADS paths, links/reparse points, and oversized content;
- restore archive traversal, link escape, overwrite, decompression bomb, interruption, wrong appliance, rollback, and unknown signer;
- adapter cross-request event theft, late/duplicate events, process escape, secret inheritance, network escape, crash, and restart;
- update freeze/rollback, mirror substitution, redirect abuse, truncation, resume corruption, activation interruption, and offline-media tampering.

## 8. Milestone gates

| Gate | Sessions | Required outcome |
|---|---|---|
| G0 Baseline | 00..06 | Authority, evidence, scanners, and basic gates are coherent |
| G1 Trust | 07..15 | Identity, issuance, attenuation, revocation, cloud, envelope, and enforcement converge |
| G2 Data plane | 16..22 | No protocol/provider/tenant/audit bypass remains |
| G3 Storage/release substrate | 23..30 | Packages, paths, crypto storage, restore, readiness, OpenBao, and artifacts are truthful |
| G4 Product truth | 31..38 | Advertised operations execute real bounded work |
| G5 Repository truth | 39..41 | Clean checkout, CI, docs, and dependencies are reconciled |
| G6 Release | 42..45 | Live proof, clean-room gates, independent review, and explicit decision complete |

No later gate may be declared closed while an earlier gate is open. A session may be implemented before external proof is available, but its finding remains `live-proof-pending`, not `closed`.

## 9. Definition of done

This roster is complete only when all sessions have receipts, all findings have a disposition, all mandatory gates pass from a clean clone, all live proofs are captured on disposable representative systems, documentation matches runtime truth, an independent re-scan reports no unresolved high/critical issue, and the user explicitly authorizes the resulting release action.

Completing the roster does not itself authorize a commit, push, tag, publication, deployment, credential rotation, or destructive migration.
