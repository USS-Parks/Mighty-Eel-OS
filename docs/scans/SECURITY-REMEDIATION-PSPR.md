# MAI / WSF / AOG Security Remediation — Plan / Sequential Prompt Roster (P-SPR)

**Initiative:** Repository-wide security remediation and production-truth restoration  
**Products:** MAI appliance, Woven Sovereignty Fabric (WSF), Agentic Orchestration Governance (AOG)  
**Repository:** `im-mighty-eel-mai` at `C:\Users\17076\Documents\Claude\Mighty Eel OS\mai`  
**Source audit:** 2026-07-05 parent-agent repository audit at commit `6ffaaeeea0a83c7fa071e114183cfa60c5898703`  
**Author:** Basho Parks + Codex · **Created:** 2026-07-05  
**Status:** **M1 ROOT-FIX MILESTONE LANDED (2026-07-07)** — all seven audit
findings (AF-001…AF-007) have FIXED root controls with offline proof, committed on
`claude/repository-security-audit-2trwtq`. No finding is CLOSED: the live-service
gates, Phase F frontier audits, the remainder of Q, and all of Phase X (external
re-scan, burn-in, owner go/no-go) remain open. See
[SECURITY-REMEDIATION-CLOSURE.md](SECURITY-REMEDIATION-CLOSURE.md) for the honest
accounting and go/no-go.

---

## §0 — Mission, authority, and stop conditions

### 0.1 Mission

Restore the product's claimed trust boundaries in code and evidence. The lane is complete only when:

1. no unauthenticated caller can mint or derive trusted authority;
2. attenuation is cryptographically and semantically monotonic;
3. sealed data and cloud credentials are tenant-bound;
4. revocation applies to every privileged token consumer;
5. production readiness reports observed vault facts, not configuration intent;
6. the full repository verification surface is green and reproducible; and
7. an independent re-scan reports zero Critical or High findings.

This roster treats the audit findings as one connected control-plane failure, not seven unrelated bugs.

### 0.2 STS meaning

STS means **stem to stern**: execute the roster in order, keep moving while a safe next prompt remains, verify every prompt, record evidence, and stop only for a genuine authority boundary, destructive migration decision, unavailable external credential, or failed gate that cannot be repaired inside the prompt.

Drafting this P-SPR does not itself authorize code changes. Execution begins only after the owner says **run it STS** or explicitly approves a milestone.

### 0.3 Git and workspace discipline

- Work only in the standalone `mai/` repository. Do not re-nest it in the Island Mountain website repository.
- Preserve the existing untracked `.opencode/` tree and all unrelated user changes.
- Never commit or push without the explicit approval required by the workspace integrity protocol.
- Before each proposed commit: list staged files, summarize changes and evidence, then ask **Shall I commit?**
- Before push: identify every outgoing commit and ask **Shall I push?**
- One prompt should produce one reviewable change set. Security-contract migrations may use two commits: contract first, consumers second.
- Record each prompt in `docs/sessions/SECURITY-REMEDIATION-DEVLOG.md`.

### 0.4 Universal verification gate

Every implementation prompt must pass the smallest relevant focused tests plus:

1. `cargo fmt --check`
2. `cargo check --workspace`
3. `cargo clippy --workspace -- -D warnings -A clippy::pedantic`
4. focused crate tests
5. `cargo test --workspace` before each milestone closes
6. `cargo audit`
7. `cargo deny check`
8. Python gates from §Q after the Python baseline is repaired
9. `mai/.integrity/scripts/verify-tree.sh <changed-files>` through Git Bash before staging

Trust-touching prompts additionally require a live-service gate against Dockerized OpenBao. Credential-broker prompts require Moto for AWS and the existing GCP/Azure live-emulator gates. A mock-only test cannot close a trust-boundary prompt.

### 0.5 Security regression rules

- No new `#[ignore]` on trust-adjacent tests.
- No test-only authentication branch compiled into production.
- No bearer secret, private key, plaintext envelope, model weight, or cloud credential in logs, receipts, panic text, snapshots, or test evidence.
- Every denial path is receipted without sensitive payloads.
- Every new production mode is fail-closed on missing identity, stale revocation, missing tenant binding, missing key material, or unavailable audit persistence.
- Compatibility behavior must be explicit and versioned; never silently accept a weaker legacy token or envelope.

### 0.6 Stop-ship conditions

The following block release immediately:

- any Critical or High finding in this roster remains open;
- WSF privileged routes are reachable without authenticated principal context;
- a fabricated or invalid parent can produce a signed child token;
- one tenant can decrypt or query another tenant's data;
- a token can select an unapproved cloud role, service account, or Azure application;
- a revoked token remains usable on a privileged path after the configured propagation bound;
- production validation passes an uninitialized, plaintext, or development vault;
- required live-service gates are skipped;
- the final independent scan has incomplete high-impact coverage without an owner-signed deferral.

---

## §1 — Audit baseline and disposition

| ID | Severity | Finding | Primary controls |
|---|---:|---|---|
| AF-001 | Critical | Attenuation signs attacker-constructed children without authenticating or fully constraining the parent | `fabric-token::attenuate`, WSF attenuation route |
| AF-002 | High | Public WSF route issues signed tokens for caller-selected subjects and roles | WSF router, principal derivation, bridge issuance |
| AF-003 | High | Envelope unseal lacks tenant/subject binding | envelope contract, AAD/thread, seal service |
| AF-004 | High | Credential broker accepts caller-selected AWS role | broker policy, role/action/resource binding |
| AF-005 | High | Production readiness certifies uninitialized/plaintext-capable vaults | vault builder, ZFS initialization, readiness |
| AF-006 | Medium | WSF privileged consumers ignore signed revocation snapshots | token verification context, snapshot store |
| AF-007 | Medium | Receipt ledger is unauthenticated and not tenant-filtered | ledger query authz, tenant index |
| AQ-001 | Quality | Clippy gate fails in `mai-core/src/cache.rs` | Rust CI |
| AQ-002 | Quality | Whole-tree Ruff, mypy, and pytest gates fail or do not collect reliably | Python packaging and CI |
| AS-001 | Supply chain | Deployment uses floating image tags and unpinned base-image digests | Docker/Compose/release provenance |

The initial audit enumerated 615 source-like files and completed a full-file parent review of the highest-risk WSF and vault boundary set. The remaining high-impact areas are mandatory frontier work in Phase F; this plan does not mislabel them as cleared.

---

## §2 — Target security architecture

### 2.1 Authenticated principal

Every privileged WSF request carries a server-created `WsfPrincipal` containing:

- tenant ID;
- subject or workload identity;
- service identity;
- authorized roles;
- audience;
- authentication method;
- credential/key identifier; and
- correlation ID.

Production principals come from verified mTLS/workload identity or an equally strong pluggable authenticator. Request headers and JSON bodies are never identity authorities by themselves. Local development uses an explicit dev authenticator that production guards reject.

### 2.2 Restriction-only attenuation

The attenuation API accepts:

- a signed parent token; and
- a `TokenRestrictions` object.

It does not accept a complete child token. The service copies immutable identity and issuer fields from the verified parent, generates child identifiers and timestamps, and applies only narrowing operations. Every axis is monotonic: tenant, subject, service identity, roles, compliance scopes, routes, models, classification, country/person restrictions, offline mode, budget, expiry, audience, resource/action/tool caveats, and lineage depth.

### 2.3 Tenant-bound envelopes

Envelope v2 binds these fields into canonical AAD and the signed provenance thread:

- tenant ID;
- owner subject/service identity;
- audience;
- policy/bundle version;
- classification and compliance scopes;
- permitted operations and destinations;
- transit key ID;
- envelope version; and
- authorizing token ID.

Unseal verifies the envelope signature/AAD, current token, revocation, tenant/owner/audience, policy, operation, destination, and classification before Transit decrypt. Production uses per-tenant or cryptographically tenant-separated wrapping keys.

### 2.4 Bounded credential brokerage

Cloud identity selection is server-side:

- AWS: tenant policy maps token scopes to an allowlisted role ARN and explicit actions/resources.
- GCP: tenant policy maps to an allowlisted service account and scopes.
- Azure: tenant policy maps to an allowlisted application/resource.
- Request bodies select a named grant, never a raw privileged identity.
- Broker root identity can assume only the minimum approved roles.
- Session duration never exceeds token TTL and never gains a floor beyond remaining validity.

### 2.5 Unified verification context

Privileged consumers receive a `VerificationContext` containing trusted time, anchors, current revocation snapshot, expected tenant/audience, required operation, and freshness policy. Signature-only verification remains a low-level primitive; services must use context-aware authorization.

### 2.6 Truthful production vault

Production accepts only initialized real backends. Readiness proves:

- the selected backend type;
- mounted dataset identity;
- encryption enabled and key available;
- PQC/signature engine wired;
- sealed master key recovered under expected PCR policy;
- model write/read encryption round-trip;
- audit persistence;
- snapshot capability when claimed;
- restart recovery; and
- failure before socket bind when any proof fails.

---

## §3 — Milestones and critical path

| Milestone | Outcome | Prompts |
|---|---|---|
| M0 — Contained | Dangerous public surfaces blocked; evidence baseline frozen | 0.1–0.6 |
| M1 — Trust plane repaired | Principal, attenuation, envelope, broker, revocation, ledger controls live | A1–A5, T1–T7, E1–E7, B1–B6, R1–R6, L1–L4 |
| M2 — Vault truth restored | Production vault is initialized, encrypted, testable, and honestly gated | V1–V9 |
| M3 — Repository closure | Deferred high-impact shards audited; all build/security gates green | F1–F9, Q1–Q8 |
| M4 — Re-ship | Migration, burn-in, external scan, release evidence and go/no-go complete | X1–X7 |

Critical path: **0 → A → T → E → B → R → L → V → F/Q → X**.

---

## Phase 0 — Containment and evidence freeze

- [ ] **0.1 — Create remediation lane artifacts.** Add this P-SPR to the doc index; create the DEVLOG, finding register, and evidence directory contract. Record HEAD, worktree status, toolchain versions, and current gate results.  
  *Gate:* exact snapshot and all known failures are reproducible.

- [ ] **0.2 — Emergency WSF exposure containment.** Change production defaults to loopback or disabled bind, remove direct host-port publication from production/HA compose, and require an authenticated ingress contract before privileged routes become reachable. Preserve an explicit opt-in demo profile.  
  *Gate:* unauthenticated host request cannot reach token issue, attenuation, seal/unseal, credential exchange, or receipts.

- [ ] **0.3 — Route inventory and privilege matrix.** Produce a machine-readable inventory of every WSF/AOG/MAI HTTP, gRPC, SSE, WebSocket, CLI, and administrative action with authentication, permission, tenant, rate-limit, and audit requirements.  
  *Gate:* CI fails when a new privileged route lacks a declared policy row.

- [ ] **0.4 — Freeze adversarial regression fixtures.** Add request/token/envelope fixtures for unsigned parent, wrong-key parent, role widening, caveat widening, cross-tenant unseal, arbitrary role selection, stale/revoked tokens, and unfiltered receipt access. Initially assert current vulnerable behavior only in a quarantined evidence harness; product tests must assert the repaired behavior.  
  *Gate:* every AF finding has a deterministic regression identifier.

- [ ] **0.5 — Threat model and claims reconciliation.** Update the repository threat model and map README, architecture, acquisition, security-production, and release claims to concrete controls. Mark unsupported claims as blocked rather than silently editing history.  
  *Gate:* each production claim has code + test + runtime-evidence owner.

- [ ] **0.6 — M0 containment review.** Run focused tests, inspect effective compose ports and generated OpenAPI, and issue a containment report.  
  *Gate:* M0 outcome is independently repeatable on a clean local stack.

---

## Phase A — WSF authentication and issuance authorization

- [ ] **A1 — Principal contract.** Add `WsfPrincipal`, authentication strength, audience, and correlation types to `fabric-contracts`; define serialization boundaries and prohibit accepting a principal from ordinary request JSON.  
  *Gate:* serde tests plus compile-time extractor boundary tests.

- [ ] **A2 — Authenticator seam.** Add a pluggable WSF authenticator with production mTLS/workload implementation and explicit local-dev implementation. Wire it as router middleware over every privileged route.  
  *Gate:* missing, malformed, expired, wrong-audience, and wrong-tenant credentials return 401/403 before handler execution.

- [ ] **A3 — Derived issuance request.** Replace public `tenant_id`, `subject_id`, and `roles` authority with fields derived from `WsfPrincipal` and server-side tenant policy. Permit only bounded request intent such as requested model subset and budget below policy ceiling.  
  *Gate:* caller cannot self-assign tenant, subject, role, service identity, classification, route, or unlimited budget.

- [ ] **A4 — Issuance permission model.** Separate self-service, service-to-service, and administrative issuance permissions; enforce audience and delegation depth. Receipt every allow and deny.  
  *Gate:* principal/action matrix tests cover every role and denial.

- [ ] **A5 — Live issuance gate.** Exercise authenticated issuance against live OpenBao using two tenants and two workload identities.  
  *Gate:* correct identity succeeds; cross-tenant and role-escalation attempts fail and are receipted.

---

## Phase T — Token primitive and attenuation repair

- [ ] **T1 — VerificationContext API.** Introduce context-aware token verification without weakening the low-level signature primitive. Require issuer, audience, time, tenant, revocation, bundle version, and operation checks.  
  *Gate:* omission of a required context check is impossible at privileged service call sites.

- [ ] **T2 — TokenRestrictions contract.** Replace full-child attenuation input with a restriction-only schema. Generate immutable child fields server-side. Version the endpoint and SDK.  
  *Gate:* OpenAPI and Rust SDK expose no attacker-supplied child identity/signature fields.

- [ ] **T3 — Parent authentication.** Verify parent signature, issuer, key ID, expiry, not-before, revocation, tenant, audience, and lineage before any child construction.  
  *Gate:* unsigned, malformed, wrong-key, expired, revoked, stale-bundle, and wrong-audience parents all fail.

- [ ] **T4 — Complete monotonicity.** Enforce subset/equality rules over every authority axis, including roles, scopes, caveats, tenant, subject, service identity, offline mode, budgets and spend counters. Set maximum attenuation depth and prevent cycles/duplicate token IDs.  
  *Gate:* property-based tests generate widenings on each field and prove rejection.

- [ ] **T5 — Atomic budget lineage.** Move spend authority to server-side atomic state or cryptographically safe single-use lineage so sibling children cannot each spend the same remaining parent budget.  
  *Gate:* concurrent sibling spending cannot exceed the parent ceiling.

- [ ] **T6 — Compatibility and migration.** Define token v1 handling: production deny-by-default, bounded verification-only migration if required, no v1 attenuation. Update SDK error semantics.  
  *Gate:* downgrade attempts fail; migration behavior is explicit and logged.

- [ ] **T7 — Live attenuation gate.** Run issue→attenuate→verify and all adversarial cases against live OpenBao and the real WSF API.  
  *Gate:* AF-001 closed with a black-box test, not only unit tests.

---

## Phase E — Tenant-bound envelope security

- [ ] **E1 — Envelope v2 contract.** Add tenant/owner/audience/policy/key/version fields and canonical AAD rules. Freeze test vectors.  
  *Gate:* every security-relevant field changes the authenticated digest.

- [ ] **E2 — Per-tenant key namespace.** Map tenants to separate Transit keys or enforce cryptographic tenant context in derivation and OpenBao policy.  
  *Gate:* tenant A identity cannot ask Transit to unwrap tenant B material.

- [ ] **E3 — Seal authorization.** Derive envelope owner/tenant/label ceilings from verified principal and policy. Reject caller labels that under-classify or broaden destinations/operations.  
  *Gate:* sensitive payload cannot be sealed with a caller-selected weaker label.

- [ ] **E4 — Unseal authorization.** Verify token context and envelope bindings before Transit decrypt. Require operation, destination, audience, policy freshness, and tenant/owner relationship.  
  *Gate:* cross-tenant, cross-subject, wrong-audience, revoked, and stale-policy unseal attempts fail before unwrap.

- [ ] **E5 — Legacy envelope migration.** Build an offline authenticated migration command for v1 envelopes. Production online unseal of unbound v1 envelopes is disabled unless an owner-approved bounded migration flag is present.  
  *Gate:* no silent v1 acceptance; migration is idempotent and audited.

- [ ] **E6 — Storage and receipt binding.** Ensure object-store keys and receipt queries include immutable tenant envelope identifiers without leaking plaintext.  
  *Gate:* tenant-scoped storage/list/query tests pass.

- [ ] **E7 — Live envelope gate.** Two-tenant seal/unseal suite against live OpenBao Transit, including tamper and restart cases.  
  *Gate:* AF-003 closed end-to-end.

---

## Phase B — Cloud credential broker confinement

- [ ] **B1 — Named grant contract.** Replace raw AWS role ARN/GCP service account/Azure application input with a tenant-scoped `grant_id`.  
  *Gate:* public API cannot submit raw privileged cloud identities.

- [ ] **B2 — Server-side grant policy.** Load signed or OpenBao-custodied mappings from tenant + operation + token scope to approved cloud identity, actions, resources, region, and maximum TTL.  
  *Gate:* missing/ambiguous mapping denies; mappings are audited and versioned.

- [ ] **B3 — AWS least privilege.** Bind role ARN, external ID/session tags, actions, resource prefixes, region, and duration. Remove wildcard actions unless the named grant explicitly and validly requires them.  
  *Gate:* Moto test proves approved access works and adjacent role/resource/action access fails.

- [ ] **B4 — GCP and Azure parity.** Apply the same named-grant and scope rules to GCP and Azure brokers.  
  *Gate:* provider-specific negative tests match AWS semantics.

- [ ] **B5 — Credential lifecycle hygiene.** Zeroize secrets, never serialize credentials into receipts, cap response exposure, and prove revocation/TTL handling.  
  *Gate:* log/receipt snapshots contain no credential material.

- [ ] **B6 — Live broker gate.** Run live OpenBao + Moto and the gated GCP/Azure emulator suites with adversarial grant selection.  
  *Gate:* AF-004 closed across all providers.

---

## Phase R — Revocation and trust freshness

- [ ] **R1 — Revocation store.** Add a verified, monotonic snapshot store with anti-rollback, expiry, atomic replacement, and last-known-good behavior.  
  *Gate:* tampered, older, future, expired, and wrong-anchor snapshots fail closed.

- [ ] **R2 — Complete revocation predicate.** Check token ID, subject hash, service identity, signing key, issuer, bundle version, tenant, and parent lineage as applicable.  
  *Gate:* table-driven tests cover each dimension.

- [ ] **R3 — Consumer integration.** Require current revocation state in issue, attenuate, verify, seal, unseal, broker, gateway, tool proxy, approval, and receipt authorization paths.  
  *Gate:* call-site inventory shows no privileged signature-only verification.

- [ ] **R4 — Emergency propagation.** Wire network refresh and removable-media import; define maximum propagation SLO and fail-closed stale behavior.  
  *Gate:* emergency revoke halts the next privileged operation within the SLO.

- [ ] **R5 — Cache and HA behavior.** Prove revocation behavior across replicas, restarts, partitions, air-gap, and key rotation.  
  *Gate:* no replica continues with rolled-back or expired state.

- [ ] **R6 — Live revocation gate.** Issue a token, use it, revoke by every supported dimension, and prove all consumers deny it.  
  *Gate:* AF-006 closed end-to-end.

---

## Phase L — Receipt ledger authorization and integrity

- [ ] **L1 — Authenticated query API.** Require `WsfPrincipal` and an audit-read capability for receipt access.  
  *Gate:* anonymous and unauthorized principals receive no metadata.

- [ ] **L2 — Tenant-safe query model.** Remove arbitrary field queries; expose typed, indexed filters with mandatory tenant predicate unless a separately audited global-auditor role is present. Add pagination and limits.  
  *Gate:* cross-tenant identifier guessing returns no rows and no existence oracle.

- [ ] **L3 — Persistent HA ledger.** Replace process-local receipt state on production paths with the intended persistent ledger and verify chain continuity across restart/replica boundaries.  
  *Gate:* restart and concurrent-ingest evidence verifies off-host.

- [ ] **L4 — Live ledger gate.** Two-tenant ingest/query/export suite plus unauthorized and global-auditor cases.  
  *Gate:* AF-007 closed.

---

## Phase V — Production vault truth restoration

- [ ] **V1 — Backend policy.** Reject `Stub` and `FileDev` in production. Make ZFS or another explicitly reviewed encrypted backend mandatory.  
  *Gate:* production profile parsing and builder tests reject every development backend.

- [ ] **V2 — Real construction.** Build the production vault with initialized PQC, TPM, audit, and storage engines; remove `ZfsVault::new` from production startup.  
  *Gate:* type/path tests prove production uses the fully wired constructor.

- [ ] **V3 — Initialization before publication.** Call and await vault initialization, scan/verify model manifests, recover sealed material, and make initialization failure block socket binding.  
  *Gate:* missing mount, wrong dataset, bad key, tampered manifest, or PCR drift fails startup.

- [ ] **V4 — Encrypted model storage.** Wire `encrypt_model_weights`/`decrypt_model_weights` into storage with authenticated metadata and tenant/model context. Define format version and migration.  
  *Gate:* disk bytes never contain plaintext fixture; wrong model/tenant key fails authentication.

- [ ] **V5 — ZFS property proof.** Query the actual dataset and require expected encryption, key status, mountpoint, readonly, compression, quota, and snapshot capabilities.  
  *Gate:* readiness fails against an ordinary directory masquerading as ZFS.

- [ ] **V6 — Snapshot and rollback implementation.** Replace placeholder metadata with actual bounded ZFS operations using direct argv, validated dataset/snapshot identifiers, and audit receipts.  
  *Gate:* create/list/rollback/destroy integration tests run on a disposable ZFS test environment.

- [ ] **V7 — Deletion semantics.** Remove unsupported “secure wipe” claims on copy-on-write storage; implement cryptographic erasure/key retirement and documented snapshot-retention effects.  
  *Gate:* docs, API responses, and tests describe the same achievable guarantee.

- [ ] **V8 — Runtime readiness evidence.** Replace unconditional `vault_opened=Pass` with measured outcomes for initialization, encryption, keys, storage round-trip, audit, restart, and capacity.  
  *Gate:* every reported pass has a failing negative-control test.

- [ ] **V9 — Vault restart/migration gate.** Install encrypted fixture, restart, verify/decrypt, snapshot, migrate legacy plaintext fixture, and test failure recovery.  
  *Gate:* AF-005 closed and production claims updated.

---

## Phase F — Deferred high-impact frontier closure

- [ ] **F1 — MAI REST/gRPC/stream audit.** Verify middleware parity, health exemptions, internal-profile bypass, reflection, SSE/WebSocket identity lifetime, quotas, and object authorization.  
  *Gate:* every route/protocol row has reportable/suppressed disposition with evidence.

- [ ] **F2 — AOG gateway audit.** Review virtual-key lookup, tenant binding, shadow/report/enforce deployment defaults, cloud routing, tokenization, budget atomicity, revocation, provider errors, streaming, and status/usage exposure.  
  *Gate:* no classified export path relies only on detection heuristics or shadow mode.

- [ ] **F3 — Tool-proxy and approval audit.** Review provenance propagation, caller-controlled `untrusted`, tool manifest authenticity, approval identity/replay, minter binding, egress scanning, session integrity, and guardrail defaults.  
  *Gate:* untrusted content cannot self-clear or trigger mutation.

- [ ] **F4 — Adapter isolation audit.** Verify executable resolution, module loading, NDJSON framing/size/timeouts, cgroup/systemd failure behavior, backend URL/redirect/credential handling, crash loops, and resource caps.  
  *Gate:* isolation is enforced by code and deployment, not architecture prose.

- [ ] **F5 — Package/filesystem audit.** Review USB mount and package-name containment, symlinks/reparse points, signed manifest names, model ID paths, staging, archive/import/export, backup/restore, and update URLs.  
  *Gate:* attacker-controlled path material cannot escape approved roots before verification.

- [ ] **F6 — Compliance/audit proof audit.** Review canonical serialization, chain concurrency, crash consistency, key rotation, report signatures, policy fail-closed behavior, classifier bypass, and tenant correlation.  
  *Gate:* audit/policy claims have negative controls and restart evidence.

- [ ] **F7 — Host/HIL/scheduler audit.** Review command invocation, device paths, air-gap enforcement, power transitions, lock poisoning, unbounded queues/caches, and shared-tenant resource exhaustion.  
  *Gate:* remotely reachable exhaustion and privileged host effects are bounded.

- [ ] **F8 — Deployment/IaC/supply-chain audit.** Review ports, TLS, secrets, container privilege, mounts, network policy, OpenBao HA, image provenance, update flow, and default credentials.  
  *Gate:* production compose has no dev-mode trust core or floating critical image.

- [ ] **F9 — Coverage reconciliation.** Re-run deterministic inventory, close every high-impact ledger row, validate candidates, and publish updated threat model and report.  
  *Gate:* no unexplained deferred high-impact row remains.

---

## Phase Q — Build, CI, dependency, and documentation truth

- [ ] **Q1 — Rust gate repair.** Fix the current clippy failure and ensure the documented command exactly matches CI.  
  *Gate:* fmt/check/clippy green from clean checkout.

- [ ] **Q2 — Python package topology.** Repair hyphenated application/package discovery, stale old-workspace paths, SDK installation, and import mode. Define supported Python version and one canonical environment bootstrap.  
  *Gate:* mypy and pytest collect from the standalone workspace without path leakage.

- [ ] **Q3 — Ruff and typing gate.** Fix deployment mock annotations/bind policy and establish scoped strict typing baselines without blanket suppressions.  
  *Gate:* `ruff check .` and declared mypy commands green.

- [ ] **Q4 — Full Python tests.** Run adapters, SDK, apps, dashboard, tools, integrity, and e2e lanes with explicit live-test markers.  
  *Gate:* all non-live tests green; live skips are counted and justified.

- [ ] **Q5 — Dependency policy.** Resolve or time-box the unmaintained `proc-macro-error2` chain; reduce duplicate critical framework versions where feasible; keep audit/deny evidence.  
  *Gate:* no vulnerable dependency and every warning has owner/expiry.

- [ ] **Q6 — Secret scanning.** Update precise fixture allowlists, scan git history and working tree with gitleaks and detect-secrets, and prove no live credential.  
  *Gate:* zero unexplained secret findings.

- [ ] **Q7 — Reproducible containers.** Pin Docker bases and service images by digest, generate SBOM/provenance, sign images, and verify offline.  
  *Gate:* AS-001 closed; no `latest` in production manifests.

- [ ] **Q8 — Documentation reconciliation.** Update README, architecture, API/OpenAPI, security-production, deployment, known issues, acquisition readiness, and release gates to match observed behavior.  
  *Gate:* automated claim-to-evidence table has no unsupported “production ready” assertion.

---

## Phase X — Migration, independent validation, and re-ship

- [ ] **X1 — Compatibility matrix and migration rehearsal.** Exercise token v1→v2, envelope v1→v2, plaintext→encrypted model storage, configuration changes, rolling upgrade, rollback, and mixed-version denial behavior.  
  *Gate:* migration is repeatable from a frozen pre-remediation fixture.

- [ ] **X2 — Full live integration suite.** Run every trust-adjacent gate against Dockerized OpenBao and Moto plus the existing GCP/Azure lanes. Archive exact logs and versions.  
  *Gate:* zero mock-only trust closure.

- [ ] **X3 — Failure-injection and burn-in.** Test OpenBao partition/seal, revocation delay, broker denial, disk full, audit failure, key rotation, replica restart, ZFS snapshot failure, and 72-hour stability on target hardware.  
  *Gate:* no fail-open behavior; recovery evidence archived.

- [ ] **X4 — Independent security re-scan.** Run a fresh repository-wide scan with delegated file reviewers or an external reviewer. Include runtime API testing of WSF/AOG.  
  *Gate:* zero Critical/High; all Medium findings triaged with owner and release decision.

- [ ] **X5 — Buyer/operator red-team walkthrough.** Follow public/operator docs from a clean machine; attempt the seven original attacks and frontier regressions.  
  *Gate:* docs alone produce a safe install and every attack is visibly denied/audited.

- [ ] **X6 — Final production gate.** Build signed artifacts, verify SBOMs, run `mai-ship-validate`, inspect burn-in/migration/security evidence, and issue a written go/no-go.  
  *Gate:* all stop-ship conditions cleared.

- [ ] **X7 — Re-ship and site-claim alignment.** Prepare release notes, checksums, migration guide, advisory language, and update consuming Island Mountain materials only after the product artifacts exist.  
  *Gate:* external claims map one-to-one to shipped, verified controls.

---

## Appendix A — Finding-to-prompt closure matrix

| Finding | Contain | Root fix | Live proof | Final closure |
|---|---|---|---|---|
| AF-001 attenuation signer oracle | 0.2 | T1–T6 | T7 | X4 |
| AF-002 unauthenticated issuance | 0.2 | A1–A4 | A5 | X4 |
| AF-003 cross-tenant unseal | 0.2 | E1–E6 | E7 | X4 |
| AF-004 arbitrary cloud identity | 0.2 | B1–B5 | B6 | X4 |
| AF-005 false/plaintext vault readiness | 0.5 | V1–V8 | V9 | X3/X4 |
| AF-006 revocation not consumed | 0.2 | R1–R5 | R6 | X4 |
| AF-007 public receipt ledger | 0.2 | L1–L3 | L4 | X4 |
| AQ-001/AQ-002 broken gates | 0.1 | Q1–Q4 | X2 | X6 |
| AS-001 floating images | 0.2 | Q7 | X2 | X6 |

## Appendix B — Required adversarial tests

1. Unsigned, malformed, wrong-key, revoked, expired, future, wrong-issuer, wrong-audience, wrong-tenant parent.
2. Child widening each scalar, set, budget, caveat, identity, and lineage axis.
3. Concurrent sibling attenuation and budget spending.
4. Tenant A unsealing tenant B envelope; subject/service/audience mismatches.
5. Label under-classification and destination broadening.
6. Arbitrary AWS role/account, GCP service account/scope, Azure application/resource.
7. Snapshot rollback, expiry, wrong signer, partition, and emergency revocation.
8. Anonymous, cross-tenant, and enumeration receipt queries.
9. Plain-directory masquerading as ZFS; missing encryption key; PCR drift; plaintext-on-disk search.
10. Package path traversal, absolute paths, symlink/reparse points, manifest path injection.
11. Protocol parity across REST, gRPC, SSE, WebSocket, SDK, and CLI.
12. Log/receipt/panic scans for secrets and regulated payloads.

## Appendix C — Evidence bundle contract

Each prompt records:

- prompt ID and objective;
- pre-change failing test or static proof;
- changed files;
- exact commands and exit codes;
- focused and workspace test counts;
- live-service versions and endpoints;
- negative-control evidence;
- migration/compatibility effect;
- remaining risks;
- proposed commit scope and, after approval, commit SHA.

Milestone evidence lives under `test-evidence/security-remediation/<milestone>/`. Evidence must contain metadata and logs, never regulated payloads or credentials.

## Appendix D — Definition of done

This P-SPR is complete only when all checkboxes are closed, the DEVLOG is complete, all universal gates pass from a clean checkout, the live suite is green, migrations and rollback are proven, the independent scan reports zero Critical/High findings, production claims are reconciled, and the owner signs the final go/no-go.

No finding is closed by documentation alone, a unit test alone where a live boundary exists, or a passing readiness flag that does not measure the claimed runtime property.

