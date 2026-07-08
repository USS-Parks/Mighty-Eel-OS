# Repository Security Audit — Plan / Sequential Prompt Roster (PSPR)

**Repository:** `C:\Users\17076\Documents\Claude\Mighty Eel OS\mai`  
**Revision audited:** `6ffaaeeea0a83c7fa071e114183cfa60c5898703`  
**Created:** 2026-07-06  
**Execution mode:** Stem to Stern (STS), strictly in roster order  
**Status:** READY FOR EXPLICIT STS AUTHORIZATION — drafting this roster authorizes no code changes  
**Source audit:** 615/615 source-like and configuration files reviewed; 4,840,925 bytes; 136,568 lines  
**Release posture:** STOP SHIP until the P0/P1 closure gate in PSPR-16 passes

---

## 0. How to run this PSPR

When the user (Basho) says **“run this PSPR STS”**, execute `PSPR-00` through `PSPR-16` in order. Do not skip a prompt because a neighboring fix appears to cover it. A prompt closes only when its own acceptance criteria and evidence are complete.

Every prompt is written so an agent can enter cold. The runner must first read:

1. workspace `AGENTS.md` and its `@RTK.md` include if present;
2. this PSPR;
3. `docs/SHIP-PROFILE.md`;
4. `docs/SHIP-HARDENING-PLAN.md`;
5. `docs/dougherty/JOHN-REMEDIATION-PLAN.md`;
6. the latest relevant session/dev log.

### 0.1 Git and workspace safety

- Work only in the standalone `Mighty Eel OS\mai` repository. Never move it into the Island Mountain website repository.
- Preserve pre-existing modifications and untracked files. At audit time these included `docs/INDEX.md`, `.opencode/`, and `docs/scans/SECURITY-REMEDIATION-PSPR.md`.
- Never run `git commit` or `git push` without the user’s separate explicit approval. **STS authorization is not commit or push authorization.**
- Before any requested commit, show the exact staged files and summary, then ask: **“Shall I commit?”**
- Before any requested push, show the exact commits and destination, then ask: **“Shall I push?”**
- If the user (Basho) says to "STS" then you do so, committing and pushing the entire way until done. `
- Use `apply_patch`/atomic edits for existing files. After every write, read the last five lines and record the line count.
- Do not weaken a control, delete a test, add an ignore, or change an expected denial into an allowance merely to make a gate green.

### 0.2 Universal verification gate

Run the narrow tests named by each prompt, then run this gate before marking that prompt complete:

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

If a command depends on unavailable hardware or external infrastructure, do not claim it passed. Save the exact command, blocker, and required operator action. Trust-adjacent changes must also pass a live OpenBao or appropriate cloud-emulator test; mock-only closure is insufficient.

### 0.3 Evidence and handoff contract

For every prompt:

- add or update a session log under `docs/sessions/` with prompt ID, files, tests, results, unresolved risks, and current `git status --short`;
- preserve exact failing output in a bounded evidence file when a gate fails;
- update this roster’s session status only after acceptance criteria pass;
- report files changed and test results;
- do not commit or push;
- when STS execution is active, continue to the next prompt unless an external credential, infrastructure decision, destructive operation, or materially ambiguous design choice requires the user.

---

## 1. Audit finding map

| Finding | Severity | Summary | Closing prompt |
|:--|:--|:--|:--|
| AF-01 | Critical | WSF permits unauthenticated signed-token issuance | PSPR-02 |
| AF-02 | Critical | Attenuation signs attacker-constructed child tokens | PSPR-03 |
| AF-03 | Critical | gRPC trusts caller-authored administrator metadata | PSPR-04 |
| AF-04 | High | Unseal lacks tenant, subject, and audience binding | PSPR-06 |
| AF-05 | High | AWS exchange accepts caller-selected role ARN | PSPR-07 |
| AF-06 | High | Production readiness certifies uninitialized/dev vaults | PSPR-10 |
| AF-07 | High | ZFS vault writes plaintext; snapshots are metadata-only | PSPR-11 |
| AF-08 | High | OpenAI streaming bypasses tokenization/metering | PSPR-08 |
| AF-09 | High | Anthropic streaming bypasses tokenization/metering | PSPR-08 |
| AF-10 | High | Legacy completions bypass compliance controls | PSPR-08 |
| AF-11 | High | Restore manifest paths can escape the target root | PSPR-12 |
| AF-12 | High | Appliance publishes dev OpenBao with known root token | PSPR-01, PSPR-14 |
| AF-13 | High | AOG defaults to non-blocking shadow mode | PSPR-09 |
| AF-14 | Medium | Receipt queries are unauthenticated/cross-tenant | PSPR-02 |
| AF-15 | Medium | Revocation lacks freshness and anti-rollback | PSPR-05 |
| AF-16 | Medium | Cloud credentials can outlive WSF authority | PSPR-07 |
| AF-17 | Medium | Usage/ROI aggregate across tenants | PSPR-09 |
| AF-18 | Medium | AppRole secret is present in repository history | PSPR-01 |
| AF-19 | Medium | Restore authenticity is optional by default | PSPR-12 |
| AF-20 | Medium | Security-critical images use floating tags | PSPR-14 |
| DF-01 | Deferred | Unsigned model manifest fields influence filesystem identity | PSPR-13 |
| QG-01 | Quality gate | Clippy, Ruff, and Python collection are not green | PSPR-15 |

---

## 2. Sequential execution index

| Order | ID | Title | Depends on | Primary gate |
|:--:|:--|:--|:--|:--|
| 1 | PSPR-00 | Freeze baseline and create remediation ledger | — | Reproducible baseline |
| 2 | PSPR-01 | Contain exposed services and AppRole credential | 00 | Exposure removed; rotation evidenced |
| 3 | PSPR-02 | Authenticate and authorize every WSF route | 01 | Anonymous/cross-tenant denials |
| 4 | PSPR-03 | Rebuild token attenuation as monotonic narrowing | 02 | Property tests prove no authority gain |
| 5 | PSPR-04 | Replace gRPC metadata trust with real authentication | 01 | Forged Admin rejected |
| 6 | PSPR-05 | Make revocation fresh, monotonic, and fail-closed | 03, 04 | Stale/rollback snapshots rejected |
| 7 | PSPR-06 | Bind envelopes and unseal to tenant/subject/audience | 05 | Cross-tenant unseal denied |
| 8 | PSPR-07 | Constrain cloud role exchange and credential lifetime | 05 | Role/TTL boundary tests |
| 9 | PSPR-08 | Create one mandatory AOG provider-dispatch pipeline | 05 | All route modes policy-dominated |
| 10 | PSPR-09 | Enforce production policy and tenant-scoped analytics | 08 | Denials block; tenant isolation passes |
| 11 | PSPR-10 | Make vault initialization a real bind-time gate | 05 | Negative readiness matrix |
| 12 | PSPR-11 | Implement encrypted ZFS storage and real snapshots | 10 | At-rest and rollback evidence |
| 13 | PSPR-12 | Make restore signed and root-contained | 10 | Traversal corpus rejected |
| 14 | PSPR-13 | Sign and contain model-package identity | 11 | Manifest tamper/traversal denied |
| 15 | PSPR-14 | Replace dev deployment and pin the supply chain | 01, 07, 11 | Production compose/security smoke |
| 16 | PSPR-15 | Restore every repository quality gate | 02–14 | Universal gate green |
| 17 | PSPR-16 | Live adversarial closure, re-audit, and release decision | 15 | Zero open P0/P1 findings |

---

## PSPR-00 — Freeze baseline and create the remediation ledger

**Closes:** execution ambiguity only  
**Files in play:** new evidence/ledger documents under `docs/scans/` and `docs/sessions/`; no product code

### Sequential prompt

```text
SESSION PSPR-00 — Freeze the audited baseline

GOAL
Create a reproducible starting point for the repository-security remediation lane without modifying product behavior.

READ FIRST
Workspace AGENTS.md, this PSPR, docs/SHIP-PROFILE.md, docs/SHIP-HARDENING-PLAN.md, and the current git status.

IMPLEMENT
1. Confirm the repository root and record HEAD, branch, remotes, tool versions, and git status.
2. Record pre-existing modified/untracked paths so no later session claims or overwrites them.
3. Create docs/scans/REPOSITORY-SECURITY-REMEDIATION-LEDGER.md with one row for AF-01..AF-20, DF-01, and QG-01. Columns: finding, owner/session, status, root-control path, tests, evidence, residual risk.
4. Capture baseline results for the universal verification gate. Do not fix failures in this session.
5. Record whether Docker/OpenBao/Moto and any ZFS test environment are available.
6. Do not stage, commit, or push.

VERIFY
- Ledger contains every finding exactly once.
- HEAD equals the revision being remediated or the ledger clearly records the newer starting revision.
- Baseline evidence distinguishes pass, fail, unavailable, and not-run.
- Re-read every written file's last five lines and line count.

OUTPUT
Report the baseline, dirty-tree ownership, infrastructure availability, and exact blockers. Continue to PSPR-01 when STS is authorized.
```

### Acceptance criteria

- Every audit row has a stable ledger identity.
- Existing user changes are preserved.
- No product source changed.

---

## PSPR-01 — Contain exposed services and respond to the AppRole credential

**Closes:** AF-12 containment, AF-18  
**Files likely:** `deployment/appliance/docker-compose.yml`, `deployment/shadow/docker-compose.yml`, `docs/sessions/THREE-LAYER-MANIFOLD-PLAN.md`, deployment/security docs

### Sequential prompt

```text
SESSION PSPR-01 — Immediate security containment

GOAL
Remove repository-configured public exposure of WSF/gRPC/dev OpenBao and complete the repository side of the exposed AppRole response.

IMPLEMENT
1. Remove the literal AppRole secret from current documentation and replace it with an unmistakable placeholder. Never copy the secret into logs, tests, commit messages, or this roster.
2. Add a credential-incident evidence template recording revocation/rotation time, actor, OpenBao audit-log review, and blast radius. The operator must supply real revocation evidence; do not fabricate it.
3. Stop publishing OpenBao dev mode, WSF, and gRPC on unrestricted host interfaces in appliance/shadow defaults. Prefer private service networks and loopback-only diagnostics.
4. Add a ship-profile check that rejects known root tokens, dev-mode OpenBao, plaintext listeners, and unrestricted security-service binds.
5. Add configuration tests for every rejected unsafe default.
6. Do not rewrite git history without explicit destructive-operation approval. Document the exact history-cleaning command separately if still required.

VERIFY
- Secret scanning does not report the AppRole value in the current tree.
- Unsafe compose fixtures fail the ship-profile validator.
- Safe private-network fixtures pass.
- Record whether credential revocation evidence was supplied; without it AF-18 remains OPEN.
- Run the universal gate.

OUTPUT
List containment changes and the operator-only incident action. Do not commit or push.
```

### Acceptance criteria

- Repository head no longer contains the credential value.
- Default compositions do not publish a known-token dev OpenBao.
- AF-18 closes only with real rotation/revocation evidence.

---

## PSPR-02 — Authenticate and authorize every WSF route

**Closes:** AF-01, AF-14  
**Files likely:** `crates/wsf-api/src/{lib.rs,main.rs,openapi.json,client.rs}`, WSF tests and deployment configuration

### Sequential prompt

```text
SESSION PSPR-02 — WSF boundary authentication and tenant authorization

GOAL
Make every sensitive WSF operation derive identity and authority from authenticated server-side state.

IMPLEMENT
1. Inventory every WSF REST/gRPC route: issue, verify, attenuate, seal, unseal, exchange, receipts, tenant administration, and future wildcard/nested routers.
2. Add one fail-closed authentication layer using mTLS workload identity or a verified service credential. No handler may trust caller-authored tenant, subject, role, budget, model, or audit scope.
3. For issuance, resolve all privilege-bearing claims from server-side tenant/workload policy and allow request fields only as narrowing hints.
4. For receipt queries, require audit permission, derive tenant scope from the principal, prohibit arbitrary-field querying, paginate/rate-limit, and redact correlation identifiers by default.
5. Bind WSF to loopback/private service networking by default; public exposure must be explicit and documented.
6. Update OpenAPI and SDK clients.
7. Add negative tests for anonymous access, forged identity, cross-tenant issue, role elevation, budget widening, and cross-tenant receipt enumeration.
8. Add a live OpenBao integration test; mock-only closure is forbidden.

VERIFY
- Every route appears in an auth-conformance table and test.
- Anonymous and cross-tenant requests fail before signing, decrypting, brokering, or querying.
- Existing valid service flows pass against live OpenBao.
- Run the universal gate.

OUTPUT
Update AF-01 and AF-14 ledger rows with exact tests/evidence. Do not commit or push.
```

### Acceptance criteria

- No WSF privileged handler is reachable without authenticated identity.
- No request directly establishes authoritative privilege claims.
- Receipt output is tenant-scoped.

---

## PSPR-03 — Rebuild attenuation as monotonic narrowing

**Closes:** AF-02  
**Files likely:** `crates/fabric-token/src/lib.rs`, `crates/fabric-token/tests/`, WSF attenuation handler

### Sequential prompt

```text
SESSION PSPR-03 — Token attenuation invariant

GOAL
Guarantee that a child token cannot acquire any identity, privilege, lifetime, budget, routing, compliance, or caveat authority absent from its verified parent.

IMPLEMENT
1. Replace complete-child input with a narrow AttenuationRequest.
2. Verify parent signature, issuer, audience, tenant, time window, key/bundle state, and current revocation before any signing operation.
3. Construct the child server-side. Inherit immutable identity fields exactly.
4. Intersect every privilege-bearing set and clamp every scalar ceiling: roles, routes, models, classification, compliance scopes, countries, person type, service identity, offline mode, budget, expiry, caveats, and future extension fields.
5. Generate child token ID server-side and set parent linkage.
6. Reject unknown privilege-bearing extension fields until explicitly modeled.
7. Add table/property tests that mutate every field independently and prove authority can only stay equal or narrow.
8. Add a live signer/WSF test proving fabricated parents and widened children are rejected before signing.

VERIFY
- Parent verification is a mandatory dominating control.
- Field-inventory test fails when a new token field lacks an attenuation rule.
- Concurrency and budget tests remain green.
- Run the universal gate.

OUTPUT
Record the invariant and field matrix in the ledger. Do not commit or push.
```

### Acceptance criteria

- No caller supplies a complete signed-child payload.
- Every token field has an explicit inherit/intersect/clamp/regenerate rule.
- Fabricated-parent signer-oracle behavior is impossible.

---

## PSPR-04 — Replace gRPC metadata trust with real authentication

**Closes:** AF-03  
**Files likely:** `mai-api/src/grpc/{mod.rs,server.rs,*.rs}`, auth/key-store modules, gRPC integration tests

### Sequential prompt

```text
SESSION PSPR-04 — gRPC authentication and method authorization

GOAL
Make caller-authored gRPC metadata incapable of granting identity or administrator authority.

IMPLEMENT
1. Add a fail-closed tonic interceptor using mTLS identity or the same verified API-key/key-store semantics as HTTP.
2. Treat x-im-profile only as an optional non-authoritative selector, or remove it. Never accept role from the client.
3. Resolve principal, tenant, profile, and role from authenticated server-side state.
4. Define a method-to-permission matrix for inference, model load/unload, power transitions, registry scans, audit reads, and health methods.
5. Apply authorization at each service method and keep public health/reflection exposure deliberately scoped.
6. Add tests for missing credentials, malformed credentials, forged Admin metadata, revoked/expired key, wrong tenant, insufficient role, and valid administrator.
7. Add a real transport-level integration test, not only direct service-method tests.

VERIFY
- x-im-profile: attacker:Admin cannot authorize any privileged RPC.
- Server construction cannot omit the auth interceptor in production.
- Reflection does not expose or invoke privileged data paths without policy.
- Run the universal gate.

OUTPUT
Update AF-03 evidence with the method matrix and negative tests. Do not commit or push.
```

### Acceptance criteria

- Authentication precedes authorization on all non-public RPCs.
- Roles originate only from trusted server-side state.
- Forged metadata is harmless.

---

## PSPR-05 — Make revocation fresh, monotonic, and fail-closed

**Closes:** AF-15  
**Files likely:** `crates/fabric-revocation/`, `crates/fabric-token/`, WSF/AOG/MAI token consumers

### Sequential prompt

```text
SESSION PSPR-05 — Revocation freshness and anti-rollback

GOAL
Ensure a valid signature on stale revocation state cannot preserve revoked authority.

IMPLEMENT
1. Version signed snapshots with issuer, tenant/scope, key ID, bundle version, issued-at, expires-at, and monotonic epoch.
2. Persist the highest accepted epoch per issuer/scope and reject rollback.
3. Define maximum snapshot age and clock-skew policy.
4. Remove security decisions based only on the token's embedded revocation enum.
5. Require current revocation state before issuance, attenuation, unseal, credential exchange, and privileged AOG/MAI use.
6. Fail closed for privileged/cloud operations when revocation state is unavailable. Define narrowly documented offline behavior for air-gapped local-only operations.
7. Test stale, expired, future-dated, wrong-issuer, wrong-tenant, wrong-key, rollback, not-found, and unavailable-source cases.
8. Exercise snapshot refresh and offline application against live OpenBao/signing infrastructure.

VERIFY
- Replaying an older validly signed snapshot fails.
- Revoking a token halts its next operation on every consumer.
- Availability failure cannot silently become allow for privileged/cloud routes.
- Run the universal gate.

OUTPUT
Record the state machine and failure policy. Do not commit or push.
```

### Acceptance criteria

- Freshness and monotonicity are enforced, not documented only.
- Every token consumer uses the same revocation decision contract.

---

## PSPR-06 — Bind envelope unseal to tenant, subject, audience, and policy

**Closes:** AF-04  
**Files likely:** `crates/fabric-envelope/`, `crates/wsf-seal/`, contracts and live tests

### Sequential prompt

```text
SESSION PSPR-06 — Envelope ownership and audience binding

GOAL
Prevent one valid token from decrypting another tenant's or service's envelope.

IMPLEMENT
1. Add tenant ID, owner/subject binding, intended service audience, policy/bundle version, envelope ID, and key ID to authenticated envelope metadata.
2. Include the canonical metadata in AEAD AAD and provenance signatures.
3. Before Transit decrypt, compare authenticated token claims to envelope metadata and required operation.
4. Use tenant-scoped keys or a documented tenant-bound key-derivation scheme; a single undifferentiated transit key is insufficient.
5. Define controlled delegation rules explicitly rather than allowing wildcard subjects/audiences.
6. Add migration/version behavior for existing envelopes; fail closed on ambiguous legacy metadata.
7. Add live OpenBao tests for same-tenant success and cross-tenant, wrong-subject, wrong-audience, stale-policy, swapped-label, and tampered-thread denial.

VERIFY
- Ciphertext copied between tenants cannot be unsealed.
- Changing any binding field fails authentication or authorization.
- Receipts record denial without leaking plaintext or secret metadata.
- Run the universal gate.

OUTPUT
Update AF-04 with contract version and test evidence. Do not commit or push.
```

### Acceptance criteria

- Cryptographic AAD and authorization checks carry the same tenant/audience identity.
- Legacy ambiguity never defaults to allow.

---

## PSPR-07 — Constrain cloud identity exchange and credential lifetime

**Closes:** AF-05, AF-16  
**Files likely:** `crates/wsf-broker/src/{lib.rs,sts.rs,gcp.rs,azure.rs}`, API contracts and live tests

### Sequential prompt

```text
SESSION PSPR-07 — Cloud broker least privilege and lifetime

GOAL
Ensure callers cannot select cloud identities and no cloud credential outlives its WSF authority.

IMPLEMENT
1. Remove AWS role ARN, GCP service account, and Azure target identity from public caller authority.
2. Map authenticated tenant/workload/purpose to server-side allowlisted cloud identities, accounts, partitions, external IDs, actions, resources, and scopes.
3. Intersect signed token caveats with that allowlist; never use Action:*.
4. If remaining WSF lifetime is below a provider's minimum lease, reject the exchange. Never clamp lifetime upward.
5. For Azure or any provider whose bearer lifetime cannot be shortened, use an enforcing proxy or another credential form; do not publish false shorter metadata.
6. Bind session name/tags and receipts to tenant, workload, token ID, role mapping, and actual provider expiry without exposing secrets.
7. Add separate AWS, GCP, and Azure tests for unauthorized identity, account/partition escape, resource/action widening, empty scope, near-expiry token, revocation, and actual expiry.
8. Run AWS against Moto/LocalStack and provider-gated live tests where available.

VERIFY
- Caller input cannot choose a role/service account/workload identity.
- Actual provider credential lifetime is never greater than remaining WSF authority.
- Out-of-scope cloud access is denied by both broker and provider policy.
- Run the universal gate.

OUTPUT
Update AF-05 and AF-16 separately. Do not commit or push.
```

### Acceptance criteria

- Server-side mapping is the source of cloud identity.
- Provider-enforced scope and lifetime match the WSF decision.

---

## PSPR-08 — Create one mandatory AOG provider-dispatch pipeline

**Closes:** AF-08, AF-09, AF-10  
**Files likely:** `crates/aog-gateway/src/{surface_openai.rs,surface_anthropic.rs,provider.rs,meter.rs,tokenize.rs,policy.rs}`

### Sequential prompt

```text
SESSION PSPR-08 — AOG mandatory dispatch pipeline

GOAL
Make every provider call, including every streaming and legacy mode, pass through the same compliance and accounting controls.

IMPLEMENT
1. Inventory every call to provider.complete, provider.stream, embeddings, and future provider sinks.
2. Create one shared dispatch pipeline with mandatory stages: authenticate/resolve tenant, current revocation, classify, route, enforce policy, tokenize/redact egress, reserve budget, dispatch, detokenize permitted response fields, settle actual usage, append durable receipt.
3. Make provider methods inaccessible to HTTP surface modules except through this pipeline.
4. Migrate OpenAI chat non-stream, OpenAI chat stream, legacy completions, embeddings, Anthropic non-stream, and Anthropic stream.
5. For streams, reserve before dispatch and finalize/void on completion, disconnect, provider error, and timeout.
6. Ensure the provider sees placeholders only when tokenization is required.
7. Add one test per independently reachable route/mode plus a route-inventory test that fails when a new provider sink lacks the pipeline.
8. Add live/provider-emulator tests for streaming and non-streaming parity.

VERIFY
- Direct provider calls from surface modules are impossible or rejected by a static architecture test.
- OpenAI stream, Anthropic stream, and legacy completions each create policy decisions, budget effects, and receipts.
- Cancellation/error paths settle consistently.
- Run the universal gate.

OUTPUT
Close AF-08, AF-09, and AF-10 independently with separate test evidence. Do not commit or push.
```

### Acceptance criteria

- Every provider sink is dominated by the shared pipeline.
- Route and stream mode cannot change compliance behavior.

---

## PSPR-09 — Enforce production policy and tenant-scope analytics

**Closes:** AF-13, AF-17  
**Files likely:** AOG main/app/policy/surface/meter modules, ship profile, deployment config

### Sequential prompt

```text
SESSION PSPR-09 — Production enforcement and tenant analytics isolation

GOAL
Make ship deployments fail closed on compliance denials and prevent tenant analytics leakage.

IMPLEMENT
1. Default production/ship profiles to enforce mode. Shadow/report-only must require an explicit development profile and must fail production readiness.
2. Load required HIPAA, ITAR/EAR, and OCAP policy packs before binding; missing or invalid packs are fatal in production.
3. Add deny-path integration tests for each policy family and for deny-wins conflict resolution.
4. Carry authenticated tenant scope into receipt-ledger queries.
5. Make /v1/usage and /v1/roi tenant-scoped. Create a distinct estate-admin endpoint/permission for global aggregation.
6. Add two-tenant tests independently for usage and ROI, including empty tenant, forged tenant selector, and estate-admin cases.
7. Update appliance configuration and operational documentation.

VERIFY
- A denied request never reaches a provider in ship mode.
- Shadow mode cannot pass readiness in ship profile.
- Tenant A cannot infer Tenant B provider, model, workflow, token, or spend data.
- Run the universal gate.

OUTPUT
Update AF-13 and AF-17 separately. Do not commit or push.
```

### Acceptance criteria

- Enforcement is a production invariant.
- Global analytics require an explicit estate-admin authority.

---

## PSPR-10 — Make vault initialization a real bind-time gate

**Closes:** AF-06  
**Files likely:** `mai-api/src/{vault_builder.rs,server.rs,ship_profile.rs,production_guard.rs}`, `mai-vault/`

### Sequential prompt

```text
SESSION PSPR-10 — Production vault readiness

GOAL
Prevent socket bind unless the production vault is initialized, cryptographically equipped, and functionally verified.

IMPLEMENT
1. Reject Stub and FileDev backends in every production/ship path.
2. Construct ZfsVault with required encryption/PQC/audit engines and call initialization before readiness evaluation.
3. Verify real storage properties, key availability/custody, permissions, encryption state, audit linkage, and expected mount/dataset identity.
4. Run a bounded write/read/delete self-test using non-secret probe data and a snapshot/restore capability probe.
5. Replace construction/path-exists RuntimeOutcome::pass results with evidence from completed checks.
6. Make all critical failures prevent listener bind.
7. Add a negative readiness matrix: missing root, ordinary directory masquerading as ZFS, missing key, wrong key, absent engine, read-only storage, failed audit, failed snapshot, FileDev, Stub.
8. Preserve safe local-development behavior outside ship profile.

VERIFY
- Every negative matrix case refuses to bind.
- A fully initialized production fixture passes.
- Readiness messages describe verified facts, not intended configuration.
- Run the universal gate.

OUTPUT
Update AF-06 with the negative matrix evidence. Do not commit or push.
```

### Acceptance criteria

- Production readiness cannot pass from constructor success alone.
- Development storage is impossible in ship profile.

---

## PSPR-11 — Implement encrypted ZFS storage and real snapshots

**Closes:** AF-07  
**Files likely:** `mai-vault/src/zfs.rs`, crypto/key interfaces, vault integration tests

### Sequential prompt

```text
SESSION PSPR-11 — ZFS vault confidentiality and rollback

GOAL
Make model storage genuinely encrypted and make snapshot/restore operations produce verified durable state changes.

IMPLEMENT
1. Encrypt model/package bytes with authenticated encryption before filesystem write; bind AAD to model ID, version, tenant/scope, and format version.
2. Decrypt only after authentication and return a specific integrity error on tamper.
3. Use atomic temporary-file, fsync, rename, and parent-directory sync semantics; zeroize plaintext/key buffers where practical.
4. Decide and document the relationship between application encryption and native ZFS encryption; production readiness must verify the chosen layers.
5. Replace metadata-only snapshot methods with constrained ZFS dataset snapshot/clone/rollback operations. Do not invoke a shell with concatenated input.
6. Verify snapshot existence and rollback result before success.
7. Add tests for plaintext absence, wrong key, ciphertext/AAD tamper, interrupted write, snapshot creation, modified state, rollback, and failed command.
8. Use a real disposable ZFS environment for closure; unit mocks alone do not close AF-07.

VERIFY
- Raw storage inspection cannot find model plaintext.
- Tampering never yields plaintext.
- Snapshot and rollback visibly change and restore actual dataset state.
- Run the universal gate.

OUTPUT
If real ZFS is unavailable, leave AF-07 OPEN with exact operator command and evidence requirement. Do not commit or push.
```

### Acceptance criteria

- At-rest confidentiality is demonstrated.
- Snapshot success represents a real ZFS operation.

---

## PSPR-12 — Make restore signed and root-contained

**Closes:** AF-11, AF-19  
**Files likely:** `tools/mai-admin/src/{main.rs,restore.rs,backup.rs}`, restore tests/docs

### Sequential prompt

```text
SESSION PSPR-12 — Authenticated, contained restore

GOAL
Prevent malicious backup manifests or filesystem links from reading/writing outside approved roots.

IMPLEMENT
1. Require signed restore manifests by default. Make any unsigned override explicit, noisy, development-only, and unavailable in ship profile.
2. Verify the canonical manifest before using any component path or metadata.
3. Reject absolute paths, prefixes/drive/UNC paths, empty/dot/parent components, alternate separators, invalid Unicode/canonical forms, and device names.
4. Resolve source and destination beneath approved roots using no-follow, handle-relative operations where supported.
5. Reject symlinks, junctions/reparse points, hard links, and link/race substitutions throughout recursive copies.
6. Bind signed metadata to component path, type, size, digest, version, and rollback policy.
7. Build a traversal corpus for Unix and Windows: ../, ..\, mixed separators, rooted paths, drive-relative paths, UNC, ADS, symlink/junction, hard link, case/canonicalization, and TOCTOU attempts.
8. Test plan and apply independently; neither may materialize an unsafe path.

VERIFY
- Every traversal corpus item is rejected before file effect.
- Unsigned/tampered manifests fail by default.
- A valid signed backup restores and verifies successfully.
- Run the universal gate.

OUTPUT
Update AF-11 and AF-19 separately. Do not commit or push.
```

### Acceptance criteria

- Signature verification dominates path use.
- Every materialized path is proven contained at operation time.

---

## PSPR-13 — Sign and contain model-package identity

**Closes:** DF-01  
**Files likely:** `mai-core/src/models/{verify.rs,install.rs}`, `mai-vault/`, package contract/tests

### Sequential prompt

```text
SESSION PSPR-13 — Model package manifest integrity

GOAL
Prevent signed weights from being paired with an attacker-controlled manifest or filesystem identity.

IMPLEMENT
1. Define a canonical signed package statement covering the complete manifest, weight digest, package format/version, model identity, quantization, compatibility, and all installed paths.
2. Verify the statement before deriving model_id or touching the vault.
3. Replace free-form path-derived IDs with validated structured identifiers. Reject separators, parent components, prefixes, control characters, normalization ambiguity, and reserved names.
4. Enforce vault-root containment again at the storage boundary; callers cannot waive it.
5. Define migration behavior for legacy packages without silently trusting unsigned metadata.
6. Add tests that pair valid signed weights with a modified manifest, traversal names, absolute names, Unicode/case collisions, and mismatched digest/version.
7. Add a successful production-engine install test and prove no file appears outside the vault root.

VERIFY
- Any manifest mutation invalidates the package signature.
- No package-controlled identifier escapes or aliases another model directory.
- Production install succeeds only with the real verifier/engine.
- Run the universal gate.

OUTPUT
Close DF-01 only with a working production-path test. Do not commit or push.
```

### Acceptance criteria

- Manifest and weights form one authenticated object.
- Storage containment is independently enforced by the vault.

---

## PSPR-14 — Replace dev deployment and pin the supply chain

**Closes:** remaining AF-12, AF-20  
**Files likely:** `deployment/{appliance,shadow,wsf-ha,live-integration}/`, CI/SBOM/provenance configuration

### Sequential prompt

```text
SESSION PSPR-14 — Production OpenBao and immutable deployment

GOAL
Produce a ship deployment with no dev secret store, known credentials, floating images, or unverifiable runtime artifacts.

IMPLEMENT
1. Separate development/test compositions from appliance production composition by file and unmistakable naming.
2. Configure production OpenBao with TLS, persistent storage/HA as designed, least-privilege policies, approved auto-unseal/bootstrap ceremony, private networking, health checks, and no known root token.
3. Pin every container image by immutable digest, including CI/live-test images. Keep human-readable version comments.
4. Generate SBOM and provenance for resolved images and binaries; verify them in CI and appliance validation.
5. Add policy checks rejecting latest/floating tags, dev-mode commands, plaintext secret-store listeners, host-published control ports, default passwords, and missing resource/security settings.
6. Exercise a clean bootstrap, restart, seal/unseal, key rotation, backup/restore, and service authentication against the production-like composition.
7. Keep dev fixtures only where tests explicitly select them; they must never satisfy ship readiness.

VERIFY
- A static deployment-policy scan finds no floating production image or dev OpenBao.
- Production composition survives restart and retains state.
- Services authenticate with least privilege and no root token.
- SBOM/provenance verification passes.
- Run the universal gate.

OUTPUT
Update AF-12 and AF-20 with exact compose and runtime evidence. Do not commit or push.
```

### Acceptance criteria

- Production and development trust stores cannot be confused.
- All shipped image identities are immutable and attestable.

---

## PSPR-15 — Restore every repository quality gate

**Closes:** QG-01 and non-security release debt  
**Files likely:** `mai-core/src/cache.rs`, `deployment/appliance/mock-llm/app.py`, Python packaging/test config, dependency policy

### Sequential prompt

```text
SESSION PSPR-15 — Green-tree quality and reproducibility

GOAL
Make the complete standalone repository verify cleanly from its current un-nested path.

IMPLEMENT
1. Fix the clippy doc_lazy_continuation failure without suppressing the lint.
2. Fix Ruff annotation issues and resolve the wildcard-bind warning by making test-server exposure explicit and safely scoped.
3. Repair Python package/test configuration so pytest can import mai from a clean environment.
4. Remove stale references/caches/configuration pointing at the old nested Island Mountain Mighty Eel OS path.
5. Add a clean-environment Python install/test command to docs and CI.
6. Resolve duplicate-dependency and unmatched-license allowance warnings where feasible; document narrow, owner/date/expiry-bound exceptions.
7. Decide the proc-macro-error2 maintenance warning: upgrade/remove the transitive path or document a time-bounded accepted risk with dependency ownership.
8. Re-run secret scanning with an allowlist limited to reviewed fixtures; scan relevant history without printing secrets.
9. Run the universal gate from a clean shell.

VERIFY
- Universal gate is entirely green.
- pytest collects and runs; no traceback references the old workspace.
- No blanket lint/test/security suppression was added.
- Dependency exceptions are explicit and expiring.

OUTPUT
Update QG-01 and record final counts. Do not commit or push.
```

### Acceptance criteria

- All documented repository gates pass.
- Standalone workspace relocation is reflected in tooling and tests.

---

## PSPR-16 — Live adversarial closure, re-audit, and release decision

**Closes:** the remediation lane  
**Depends on:** every prior prompt

### Sequential prompt

```text
SESSION PSPR-16 — Stem-to-stern security closure

GOAL
Prove the fixed system, not merely the changed source, and issue a defensible release decision.

IMPLEMENT
1. Review the remediation ledger. Every AF/DF/QG row must be closed or carry an explicit owner-approved deferral; no silent blanks.
2. Run the universal gate from a clean checkout-equivalent environment.
3. Run the live trust suite against OpenBao and cloud emulators/providers.
4. Run adversarial end-to-end cases:
   - anonymous WSF issuance;
   - fabricated-parent attenuation;
   - forged gRPC Admin metadata;
   - stale/rollback revocation;
   - cross-tenant unseal;
   - caller-selected cloud identity and near-expiry lease;
   - OpenAI/Anthropic stream and legacy-route egress;
   - policy denial in ship mode;
   - cross-tenant usage/ROI/receipts;
   - uninitialized vault bind;
   - restore and model-package traversal/tamper;
   - dev OpenBao/floating image ship validation.
5. Re-run repository-wide secret, dependency, static security, and route/sink inventory scans.
6. Reconcile every finding to source, test, runtime evidence, and residual risk.
7. Update docs/INDEX.md, deployment roadmap, ship profile, readiness summary, and the remediation ledger with the final status.
8. Produce a final security closure report with severity counts, exact commands, test totals, environment limitations, deferred risks, and release recommendation.
9. Do not mark READY if any Critical/High finding lacks runtime evidence or an explicitly accepted deferral from the user.
10. Do not commit or push. Present the complete diff and ask separately if the user wants a commit.

VERIFY
- All universal and live gates pass.
- No P0/P1 finding remains open.
- Independent route/sink inventory finds no bypass sibling.
- The report can be reproduced from recorded commands.

OUTPUT
State one decision only: STOP SHIP or READY FOR RC-11 RE-BUNDLE. Include the evidence path and current git status. Do not commit or push.
```

### Acceptance criteria

- Zero open Critical/High findings.
- All trust-adjacent claims have live evidence.
- Final documentation and ledger agree.
- Release decision is explicit.

---

## 3. STS stop conditions

The runner must pause and ask the user only when:

1. a credential must be revoked/rotated in an external system;
2. a destructive history rewrite, data migration, rollback, or real-cloud mutation is required;
3. production identity, key custody, ZFS topology, or provider-role mapping cannot be determined from repository evidence;
4. continuing would overwrite pre-existing user work;
5. the user must accept a residual Critical/High risk;
6. commit or push is requested.

Ordinary implementation choices, failing tests, and repairable build errors are not stop conditions. Diagnose, fix, verify, and continue.

---

## 4. Final definition of done

This PSPR is complete only when:

- PSPR-00 through PSPR-16 are executed in order;
- every AF/DF/QG ledger row has evidence-backed closure;
- the universal gate is green;
- live OpenBao/cloud-emulator and real ZFS gates required by affected prompts are green;
- secret rotation evidence exists without reproducing the secret;
- no Critical/High finding remains open;
- the final closure report says either STOP SHIP or READY FOR RC-11 RE-BUNDLE;
- no commit or push has occurred without the explicit approval required by workspace policy.

