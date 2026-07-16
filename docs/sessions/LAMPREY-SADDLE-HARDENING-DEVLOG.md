# Lamprey Saddle WSF + AOG Hardening DEVLOG

Initiative: close the 2026-07-15 WSF/AOG workflow findings and complete the interrupted high-risk review.  
Repository: `im-mighty-eel-mai`.  
Worktree: `mai-worktrees/mai-LSH-1`; branch `session/LSH-1`.  
Plan of record: [`../../../PLANNING/LAMPREY-SADDLE-WSF-AOG-SECURITY-HARDENING-PSPR.md`](../../../PLANNING/LAMPREY-SADDLE-WSF-AOG-SECURITY-HARDENING-PSPR.md).  
Finding register: [`../scans/LAMPREY-SADDLE-HARDENING-FINDINGS.json`](../scans/LAMPREY-SADDLE-HARDENING-FINDINGS.json).  
Evidence root: `test-evidence/lamprey-saddle-hardening/`.

Every entry records the vulnerable path/invariant, legitimate behavior, changed files, focused and milestone gates, residual risk, and commit state. A prompt is not complete until its named gate passes. A narrowed pass is never reported as a full pass.

---

## M0 — Contained and reproducible

### LSH-00 — Execution lane bootstrap and drift check

Status: **PASS** (implementation commit `c6282ab9c5933f4b5a014a49449b02f77dd8e9f4`).

Objective: create an isolated execution lane at the exact assessed revision and preserve the interim scan evidence before implementation.

Source identity:

- HEAD: `361f70b272c0fbee6375f462c912bd8d5b5891bb`.
- Scan: `361f70b_20260715T130144Z`.
- Snapshot: `codex-security-snapshot/v1:sha256:2f504c2504ea119582f8981b2fa67c948906810eeab7a61cec73e74596695e80`.
- Fresh worktree status before lane writes: clean.
- Untracked `.opencode/` exists only in the original working tree and was preserved; it is absent from this isolated worktree.

Toolchain:

- `rustc 1.96.1 (31fca3adb 2026-06-26)`;
- `cargo 1.96.1 (356927216 2026-06-26)`;
- `git 2.54.0.windows.1`;
- Docker client `29.6.1`;
- `cargo-audit 0.22.1`; and
- `cargo-deny 0.19.7`.

Environment limitations recorded honestly:

- `Session-Worktree.ps1` attempted `git fetch origin --quiet`, but Windows Schannel returned `SEC_E_NO_CREDENTIALS`. This did not affect source identity because the exact assessed commit was already available locally and the worktree was created from that immutable revision.
- Docker client is installed, but reading the user Docker config returned access denied; live-engine availability is not claimed until the relevant live gate runs.
- Git cannot read the user's global ignore file in this sandbox; repository status still resolves and no global configuration was changed.

Evidence frozen under `test-evidence/lamprey-saddle-hardening/M0/source-scan/`:

- 81 candidate ledgers;
- nine completed file-review bundles;
- 141-row selected-file worklist;
- coverage ledger; and
- repository threat model.

Gate result: tracked revision matches the assessment; no relevant source drift exists. LSH-00 is complete.

### LSH-01 — Canonical finding and regression registry

Status: **PASS** (implementation commit `c6282ab9c5933f4b5a014a49449b02f77dd8e9f4`).

The machine-readable register imports all 81 raw instances through the frozen evidence root and maps them exactly once to 29 confirmed families or 10 deferred families. Each confirmed family has a stable regression ID and prompt owner; each deferred family has a reachability question and prompt owner.

`SECURITY-REMEDIATION-PSPR.md` and `docs/INDEX.md` now state the historical truth: the older lane's DEVLOG records execution, while its unchecked roster is preserved; the current Lamprey Saddle lane owns closure.

Gate: `python .integrity/scripts/lamprey-finding-register-check.py` PASS — 81 raw instances, 29 confirmed families, 10 deferred families, and 81 candidate ledgers reconcile exactly.

### LSH-02 — Immediate production containment

Status: **PASS** (implementation commit `c6282ab9c5933f4b5a014a49449b02f77dd8e9f4`).

Containment now fails before listener bind at the production startup seams:

- `aogd` defaults `AOGD_PROFILE` to production, requires trust, and refuses production while admin authorization and Raft peer mTLS are not yet actually wired. `AOGD_ALLOW_INSECURE_BIND=1` has no effect in production. The Loom harness must declare `AOGD_PROFILE=development` and then separately opt into its isolated non-loopback bind.
- `wsf-api` defaults `WSF_PROFILE` to production and requires hardened OpenBao material, workload authentication, and a mandatory revocation store even on loopback. Because the binary does not yet wire the shared revocation store, production remains intentionally unstartable until `LSH-W1`; appliance/shadow demos explicitly declare development plus the isolated-network bind opt-in.
- `aog-gateway` validates mandatory production revocation and provider endpoints before constructing OpenBao or binding. Credentialed production providers require HTTPS; plaintext local providers are confined to loopback; the default listener and local backend are loopback; provider redirects are disabled so credentials cannot follow a redirect.
- `deployment/wsf-ha` now states `WSF_PROFILE=production` explicitly. It remains contained until W1 supplies the mandatory revocation dependency; this is intentional and is not reported as production-ready.

Changed product/deployment files:

- `crates/aogd/src/main.rs`;
- `crates/wsf-api/src/main.rs`, `crates/wsf-api/src/posture.rs`;
- `crates/aog-gateway/src/lib.rs`, `main.rs`, `posture.rs`, `provider.rs`; and
- appliance, shadow, WSF-HA, Loom Compose, and Loom k3s manifests.

Focused gates:

- `cargo test -p aogd --bin aogd` — PASS, 4/4;
- `cargo test -p wsf-api posture` — PASS, 8/8;
- `cargo test -p aog-gateway posture` — PASS, 5/5; and
- `cargo clippy -p aogd -p wsf-api -p aog-gateway --all-targets -- -D warnings -A clippy::pedantic` — PASS.

Legitimate behavior retained: development harnesses still run, but only after an explicit development profile; non-loopback insecure harness binds require a second explicit opt-in. Production omission always resolves to the fail-closed profile.

Residual risk: the underlying admin authorization, Raft mTLS, and mandatory shared WSF revocation implementations are not falsely marked fixed. Production is deliberately unavailable at those seams until C1/C2/C3 and W1 replace containment with real controls.

### LSH-03 — Baseline and adversarial fixture freeze

Status: **PASS** (implementation commit `c6282ab9c5933f4b5a014a49449b02f77dd8e9f4`).

`test-evidence/lamprey-saddle-hardening/M0/regression-plan.json` maps all 29 confirmed families to a unique canonical regression ID, narrow boundary, fixture, execution mode, vulnerable red condition, and repaired green condition. Destructive mutation and external-state PoCs are request-fixture-only until an owning prompt creates disposable isolated state. All 10 deferred families have argv-form read-only `rg` reachability questions.

Gates:

- `python .integrity/scripts/lamprey-regression-plan-check.py --run-reachability` — PASS: 29 confirmed plans, 10 deferred executable questions; nine queries returned matches and `LSD-009` returned no matches, which is preserved as a reachability result rather than coerced into a finding.
- Initial `cargo test --workspace` — ENVIRONMENT BLOCKED after all preceding tests passed: two `aog-wire/tests/mtls.rs` setup failures reported `openssl on PATH: program not found`.
- Located existing prerequisite at `C:\Program Files\Git\usr\bin\openssl.exe`; no installation or persistent system change was made.
- `cargo test -p aog-wire --test mtls` with that directory prepended to the command-local `PATH` — PASS, 2/2.
- Full `cargo test --workspace` rerun with the same command-local prerequisite — PASS, exit 0, including all workspace and doctest lanes (repository-declared ignored/nightly/SLO tests remained ignored by the standard command).

M0 acceptance: **PASS**. Containment is active, all frozen evidence and plans are machine-reproducible, focused lint/tests pass, and the standard full workspace gate passes when its existing OpenSSL prerequisite is supplied. Implementation and evidence are committed as `c6282ab9c5933f4b5a014a49449b02f77dd8e9f4`; this DEVLOG SHA update is the follow-up metadata commit.

---

## M1 — Trust and tenant boundary

### LSH-A1 — Mandatory verified request context

Status: **PASS** (M1 implementation commit `3f54495`).

The shared `fabric-contracts` boundary now carries server-established roles, immutable token lineage, the authenticated audience and correlation identity, an exact privileged operation, and a final canonical resource. `WsfPrincipal`, `CanonicalResource`, and `VerifiedRequestContext` remain non-deserializable/private-field types, so ordinary JSON cannot manufacture them. An operation-specific sink check rejects replay of a valid context at the wrong operation.

WSF privileged handlers establish the context before bridge, seal, broker, or ledger work; tenant-binding sinks now require the verified context rather than a raw principal. AOG admission establishes the same context before validation/mutation and internal controllers can obtain estate authority only through the server-owned `admit_system` seam.

Changed files:

- `crates/fabric-contracts/src/{lib.rs,principal.rs}`;
- `crates/wsf-api/src/{auth.rs,audit.rs,lib.rs}`;
- `crates/aog-apiserver/src/{admission.rs,policy.rs}`; and
- `crates/aog-controller/src/objects.rs`.

Focused gate:

- `cargo fmt` — PASS;
- `cargo test -p fabric-contracts -p wsf-api -p aog-apiserver -p aog-controller` — PASS, including two compile-fail principal construction tests, wrong-audience and forged-resource-tenant refusal, wrong-operation sink refusal, all crate unit/integration tests, and doc tests.

Residual risk: A1 supplies the authenticated context contract; mandatory current revocation, explicit tenant/estate capabilities, reservations, and durable audit remain owned by A2–A5 and their downstream migration prompts.

### LSH-A2 — Mandatory current revocation provider

Status: **PASS** (M1 implementation commit `3f54495`).

`MonotonicRevocationStore::authorize` is now the single trusted-time consumer contract. Only anchor-verified, strictly advancing snapshots enter the store; absence, sequence zero, malformed/future issue time, malformed/expired freshness, rollback, and every complete-predicate revocation dimension fail closed. WSF seal and broker consumers reuse this contract instead of duplicating partial freshness logic.

Gate: `cargo test -p fabric-revocation -p wsf-seal -p wsf-broker` PASS. The table-driven contract covers absent, wrong-anchor, future, expired, lower-sequence rollback, and revoked state.

### LSH-A3 — Tenant and estate capability types

Status: **PASS** (M1 implementation commit `3f54495`).

`TenantScope` and `EstateScope` are private-field, non-deserializable proof types derived only from `VerifiedRequestContext`. Exact roles are defined for tenant revocation, estate revocation, global mutation, ring/key destruction, and policy publication. A tenant-bound principal cannot construct estate authority even if its verified token contains an estate-looking role; the server-owned estate principal remains explicit.

Gate: `cargo test -p fabric-contracts` PASS, including the complete dangerous-capability matrix and compile-fail principal construction proofs.

### LSH-A4 — Atomic reservation and immutable lineage

Status: **PASS** (M1 implementation commit `3f54495`).

Attenuation now stamps an immutable `root_id` on the first child and copies it through nested descendants. `ReservationLedger` provides atomic reserve/commit/release across tenant, root lineage, mission, and system keys; unsettled reservations release on drop/cancellation. One hundred barrier-synchronized contenders against a ten-call cap admit exactly ten, and dropped reservations restore capacity.

Gate: `cargo test -p fabric-token` PASS, including nested-root lineage and deterministic concurrent reservation regressions.

### LSH-A5 — Durable mutation/audit contract

Status: **PASS** (M1 implementation commit `3f54495`).

AOG admission now Raft-commits a serialized `aog.audit-intent/v1` outbox record before each desired-state mutation. The intent binds correlation, tenant/subject, exact operation/resource, before/after digests, and planned store operation; the resulting receipt references the durable intent. A failure before the intent produces no mutation, while every later ordering retains a Raft-durable recovery record.

Gate: `cargo test -p aog-apiserver` PASS; three successful mutations produce three receipts and three durable pre-commit intents, while a pre-admission rejection produces neither. Focused `cargo check` and clippy `-D warnings` across the shared contracts, WSF consumers, AOG API/controller, and node crates PASS.

Residual migration: W1–W5, O1–O7, G3/G4, and T2 consume these mandatory contracts at their production seams; O4 owns outbox delivery/restart idempotency rather than treating the durable intent alone as final receipt delivery.

### LSH-W1 — Revocation on every WSF privileged endpoint

Status: **PASS** (M1 implementation commit `3f54495`).

`AppState` now carries non-optional `RevocationEnforcement`; production startup loads and verifies the current OpenBao snapshot and refuses startup without it. Issue, verify, attenuate, seal, unseal, exchange, and audit/export paths consult current principal/token revocation before privileged work, and denials are tenant-safe and receipted.

Gate: live OpenBao + Moto test PASS. One valid token succeeded, a sequence-2 tenant revocation was adopted without restart, and issue/verify/attenuate/seal/unseal/exchange all returned 403; rollback replay remained denied.

### LSH-W2 — Owner-bound envelope authorization

Status: **PASS** (M1 implementation commit `3f54495`).

Unseal now requires the presenting token's subject hash to match the signed envelope owner. Cross-subject service access requires both a verified service identity and the exact `envelope:delegate-unseal` capability; generic admin/delegation roles do not qualify.

Gate: offline named-capability matrix and live OpenBao HTTP seal/unseal PASS; two same-tenant subjects cannot unseal one another's envelope.

### LSH-W3 — Complete tenant issuance policy

Status: **PASS** (M1 implementation commit `3f54495`).

Omitted/empty requested models now resolve to the restrictive tenant allowlist. Signed token models, routes, compliance scopes, classification ceiling, and service identity are selected from server policy plus authenticated context; untrusted issue bodies remain bounded intent only.

Gate: omission/empty/subset/over-broad model matrix, service-identity matrix, policy-authority bridge composition tests, `cargo test -p wsf-api -p wsf-bridge --lib`, and all-target compilation PASS.

### LSH-W4 — Attenuation depth, revocation, and budget lineage

Status: **PASS** (M1 implementation commit `3f54495`).

Attenuation verifies the original signed/current parent, enforces the tenant's absolute maximum depth, carries signed immutable root/depth/ancestor lineage, rejects duplicate/cyclic child IDs, and folds authoritative root-lineage reservations into remaining budget before signing a descendant.

Gate: revoked parent, maximum depth, 100-way sibling reservation concurrency, spend-then-attenuate state, duplicate ID, and recursive root-reset regressions PASS in `fabric-token`/`wsf-api`; live attenuation-before-revocation and denial-after-revocation PASS.

### LSH-W5 — Credential TTL intersection

Status: **PASS** (M1 implementation commit `3f54495`).

AWS/GCP requested duration is the strict intersection of provider/grant maximum, token remaining lifetime, and revocation-snapshot freshness. When remaining authority is below the provider floor the broker denies before custody/cloud calls; it never rounds authority upward.

Gate: every AWS remaining-token case from 1 through 899 seconds denies; revocation freshness below the floor denies; live Moto STS PASS and returned expiration is no later than token authority.

### LSH-W6 — WSF live two-tenant gate

Status: **PASS** (M1 implementation commit `3f54495`).

The live gate now drives issue → verify → attenuate → seal → unseal → exchange through real HTTP against OpenBao and Moto for two authenticated tenants and two same-tenant subjects. It proves subject/tenant isolation, clean tenant-B continuity while tenant A is revoked, snapshot sequence rollover, rollback refusal, and revocation-state rehydration after server restart.

Gate: `cargo test -p wsf-api --test live_revocation -- --nocapture`, `cargo test -p wsf-seal --test live_seal -- --nocapture`, and `cargo test -p wsf-broker --test live_localstack -- --nocapture` PASS with no credential/plaintext logging in evidence output.

### LSH-O1 — Tenant-safe GET and LIST

Status: **PASS** (M1 implementation commit `3f54495`).

Authenticated GET/LIST now require principal context and apply tenant scope before lookup/pagination. Foreign-tenant GET is indistinguishable from absence; estate reads require the explicit `estate:read` role and use bounded continuation pagination (`limit` clamped to 1–1000).

Gate: the cross-tenant GET/LIST/existence/pagination matrix in `aog-apiserver/tests/crud.rs` PASS.

### LSH-O2 — Global object mutation rules

Status: **PASS** (M1 implementation commit `3f54495`).

Update/delete of `tenant=None` objects now requires the exact global-object estate capability. Tenant principals cannot mutate another tenant's object, and tenant deletion of revocation/kill records is always refused.

Gate: tenant-owned, other-tenant, global-object, and protected-revocation create/update/delete cases PASS in the AOG CRUD suite.

### LSH-O3 — Revocation intent authorization

Status: **PASS** (M1 implementation commit `3f54495`).

Tenant revocation requires `tenant:revocation` and an exact tenant target. Estate targets require `estate:revocation`; ring destruction additionally requires `estate:ring-key-destruction`. Empty-tenant authenticated principals no longer acquire tenant scope accidentally.

Gate: the exact tenant/estate/ring authorization matrix PASS; normal tenant tokens cannot enqueue estate-wide or ring-key actions.

### LSH-O4 — Audit-before-success mutation path

Status: **PASS** (M1 implementation commit `3f54495`).

Every privileged mutation persists a Raft-replicated audit intent before desired-state commit, finalizes a durable outbox record after commit, and idempotently delivers the receipt into the separately signed ledger. Startup recovery replays finalized receipts. A crash in the post-commit/pre-finalization window now converts the retained intent into signed, explicitly `indeterminate` evidence, preserving the audit barrier without falsely claiming commit success.

Gate: `cargo test -p aog-apiserver --test receipt -- --nocapture` PASS (3/3), including cold restart from the crash window, idempotent replay, off-host pack verification, and rejection-without-receipt.

### LSH-O5 — Ring/key destruction confinement

Status: **PASS** (M1 implementation commit `3f54495`).

Ring actions require both estate revocation and the dedicated ring-key-destruction authority; tenant-created intents are rejected before controller execution and cannot target shared estate Transit keys.

Gate: the authorization matrix plus live OpenBao `live_ring` test PASS and observe only the authorized key target.

### LSH-O6 — Policy-bundle publication trust boundary

Status: **PASS** (M1 implementation commit `3f54495`).

Global PolicyBundle admission requires estate publication authority. The controller refuses tenant-scoped bundles as estate truth, retracts any previously derived publication, and records degraded status rather than re-signing arbitrary tenant desired state.

Gate: tenant/global admission cases, bundle signature/anti-rollback unit coverage, and live OpenBao `live_bundle` PASS.

### LSH-O7 — Derived controller state ownership and persistence

Status: **PASS** (M1 implementation commit `3f54495`).

Mission-derived grants are keyed by tenant, immutable mission UID, mission name, and tool; namesake tenants cannot collide. Scope shrink updates allowed systems, removed tools are pruned, edge grant caches are tenant-partitioned, and client updates cannot lower controller-owned versions, counters, or typed status persisted in desired state.

Gate: mission rename/namesake, tenant collision, scope reduction, tool removal, replay, and authoritative-status preservation tests PASS across `aog-controller`, `aog-node`, and AOG admission.

### LSH-O8 — AOG API/controller integration gate

Status: **PASS** (M1 implementation commit `3f54495`).

Combined black-box evidence exercises tenant-isolated API admission and Raft state, controller reconciliation, OpenBao-backed ring revocation and policy publication, durable audit recovery, replay, and restart behavior. `LSF-009`–`LSF-014` and the reachable M1 instances of `LSD-007/008` are closed.

Gate: targeted `receipt`, `crud`, `mission`, `toolgrants`, `replay`, `live_ring`, `live_bundle`, and `live_revocation` tests PASS; `cargo test -p aog-apiserver -p aog-controller` PASS.

### M1 — Trust and tenant boundary milestone gate

Status: **PASS — implementation commit `3f54495`; M1 checkpoint pushed to `main`**.

Final verification on 2026-07-15:

- `cargo fmt --check` — PASS.
- `cargo check --workspace` — PASS.
- `cargo clippy --workspace -- -D warnings -A clippy::pedantic` — PASS.
- `cargo test --workspace` — PASS, including `aog-wire` mTLS when the existing `C:\Program Files\Git\usr\bin\openssl.exe` was added to this command's `PATH`; no machine configuration was changed.
- `cargo audit` — PASS; 476 locked dependencies scanned against 1,160 RustSec advisories, no vulnerability reported.
- `cargo deny check` — PASS (`advisories ok, bans ok, licenses ok, sources ok`); informational unmatched-license and duplicate-version warnings remain existing dependency-policy output.
- Live disposable services — PASS with OpenBao on port 8200 and Moto on port 5566; no secret/plaintext evidence was logged.

Gate notes: the first unmodified-`PATH` workspace test run failed only because `openssl` was not discoverable; the command-local Git OpenSSL path closed that environment prerequisite. Full gates then exposed and closed one `aogd` principal-accessor migration and eleven strict WSF needless-borrow lints before the final green run.

---

## M2 — Authenticated AOG control plane (partial checkpoint)

Checkpoint status: **PASS and PUSHED for LSH-C1 through LSH-C3; M2 remains IN PROGRESS**. LSH-C4 and LSH-C5 were not started in this checkpoint and remain required before M2 acceptance.

### LSH-C1 — Node identity and TLS provisioning contract

Status: **PASS** (implementation commit `9b31ad9`).

`aog-wire` now validates a credential-free HTTPS advertised origin, the exact `spiffe://loom/node/<node-id>` URI SAN, advertised-host SAN, estate CA chain, server/client EKUs, validity and rotation window, and certificate/private-key pairing. `aogd` provisions DER identity from either mounted files or an OpenBao record and validates it before listener startup. Private-key material is represented only by a redacted diagnostic marker. The operator contract and rolling-rotation procedure are recorded in `docs/operations/AOG-CONTROL-PLANE-TLS.md`.

Gate: missing, malformed, wrong-node, wrong-host, wrong-CA, rotation-unsafe, and mismatched-key fixtures fail closed; valid material passes. The focused `aog-wire` and `aogd` tests pass with the existing Git OpenSSL directory prepended only to the command's `PATH`.

### LSH-C2 — Integrate mTLS into Raft client and server

Status: **PASS** (implementation commit `9b31ad9`).

The daemon now constructs the Raft network and admin forwarding client from the validated node identity, disables redirects, terminates Axum through a client-certificate-requiring TLS listener, extracts the authenticated node identity before HTTP/Raft decoding, and rejects claimed Raft sender identities that do not match the certificate. Secure membership requires HTTPS.

Gate: `crates/aogd/tests/daemon_mtls.rs` forms and writes through a real three-node mTLS Raft cluster. No-certificate, rogue-CA, HTTP membership, and valid-certificate forged vote/append/snapshot attempts are rejected before Raft state handling.

### LSH-C3 — Fail-closed admin trust and bounded bootstrap

Status: **PASS** (implementation commit `9b31ad9`).

Without configured admin trust, normal admin mutations remain unavailable. The sole bootstrap exception is one loopback `/admin/initialize` request, atomically consumed and kept closed after persisted membership survives restart. Remote bootstrap, wrong-path mutation, and replay are denied. `AOGD_ALLOW_INSECURE_ADMIN=1` is an explicit development-harness escape hatch and is rejected by the production posture check.

Gate: the unit authorization matrix and `crates/aogd/tests/bootstrap.rs` pass, including normal-write denial, one loopback initialization, replay denial, and replay denial after shutdown/restart with the same data directory.

### Partial-M2 verification

Final focused verification on 2026-07-15:

- `cargo fmt --check` — PASS.
- `cargo check -p aog-wire -p aogd -p aog-noded --all-targets` — PASS.
- `cargo clippy -p aog-wire -p aogd -p aog-noded --all-targets -- -D warnings -A clippy::pedantic` — PASS.
- `cargo test -p aog-wire` — PASS (5 mTLS/identity cases plus the three-node wire test).
- `cargo test -p aogd` — PASS, including admin authorization, persistent bootstrap, real three-node mTLS, and OpenBao provisioning seams.
- `cargo test -p aog-noded` — PASS, including downstream edge registration/heartbeat behavior.
- `git diff --check` — PASS.

During final verification, one production-posture test fixture still enabled the newly forbidden insecure-admin flag. Correcting that boolean fixture restored the intended production-with-mTLS case; the complete focused gate set was rerun and passed afterward.

Commit state: implementation and evidence committed in `9b31ad9`; DEVLOG SHA closeout committed in `6eb5293`; `origin/main` advanced from `5ea75a9` through `6eb5293` on 2026-07-15. This final ledger update records the confirmed remote checkpoint.

### LSH-C4 — Membership and forwarding destination policy

Status: **PASS — uncommitted; M2 commit approval pending**.

The membership-selected outbound seams now consume one strict canonical origin contract. Secure membership accepts only bounded credential-free HTTPS origins with a host and valid port; paths, queries, fragments, port zero, surrounding whitespace, and userinfo are rejected. Initialization rejects duplicate IDs/origins, and learner admission rejects node-ID rebinding or reuse of another member's origin. The wire transport independently revalidates and canonicalizes every `BasicNode` address before dispatch, so a bypass around the admin API still fails closed.

Both Raft and admin HTTP clients disable redirects. Follower writes validate the current leader's pinned ID/origin immediately before dispatch. The follower authenticates the caller at the ingress node, then the leader authenticates the forwarding node's current membership and mTLS SPIFFE identity; the caller's WSF trust token and `Authorization` header are never attached to the membership-selected request. Forward markers on a non-write path, without secure transport, without a node identity, from a non-member, or alongside a bearer token are denied.

Changed files:

- `crates/aog-wire/src/lib.rs`;
- `crates/aogd/src/admin.rs`;
- `crates/aogd/src/lib.rs`; and
- `crates/aogd/tests/daemon_mtls.rs`.

Focused gate:

- strict canonical-origin matrix — PASS;
- redirect fixture — PASS; the sink received zero requests;
- forwarded-request credential regression — PASS; no WSF token or Authorization header;
- malicious membership path/query/userinfo, duplicate-origin, and node-ID-rebind cases — PASS; and
- real three-node mTLS cluster after the destination-policy migration — PASS.

Residual risk: membership changes remain privileged admin operations and certificate issuance remains anchored in the estate CA. C4 prevents URL reinterpretation, redirect following, identity/address collision, and bearer forwarding; it does not treat compromise of the estate CA or an authorized quorum as an address-policy bypass.

### LSH-C5 — Control-plane adversarial gate

Status: **PASS — uncommitted; M2 commit approval pending**.

The Raft router now enforces a 1 MiB request-body ceiling before JSON decoding. The live three-node mTLS fixture was extended beyond convergence to cover:

- no certificate and rogue CA rejection during TLS handshake;
- valid-certificate/wrong-node forged vote, append, and snapshot rejection before Raft;
- replay of the forged vote with the same fail-closed result;
- malformed JSON rejection and 1 MiB + 1 byte body rejection;
- fresh same-CA certificate rotation for an unchanged node SPIFFE ID and advertised origin, followed by cold Raft restart and committed-state recovery;
- current-leader loss, election of a different leader by the remaining quorum, and a successful post-failover write within the ten-second bound; and
- the separate C3 bootstrap fixture's one-shot and post-restart replay denial.

`LSF-007` and `LSF-008` are closed at their production seams by the combined C1-C5 evidence: omission cannot expose a steady-state admin surface, and unauthenticated/plaintext/misdirected Raft traffic cannot reach consensus decoding or privileged forwarding.

### M2 — Authenticated AOG control plane milestone gate

Status: **PASS — code and DEVLOG are not committed**.

Final verification on 2026-07-16:

- `cargo fmt --check` — PASS.
- `cargo check --workspace` — PASS.
- `cargo clippy --workspace -- -D warnings -A clippy::pedantic` — PASS.
- `cargo test --workspace --quiet` with the existing Git OpenSSL directory added only to that command's `PATH` — PASS, exit 0. The long repository fuzz/convergence lanes and standard ignored-test declarations behaved as expected.
- `cargo audit` — PASS; 494 locked dependencies scanned against 1,160 RustSec advisories, no vulnerability reported.
- `cargo deny check` — PASS (`advisories ok, bans ok, licenses ok, sources ok`); existing informational unmatched-license and duplicate-version warnings remain.
- Focused `aog-wire`, `aogd`, and `aog-noded` test suites — PASS, including the live multi-node C4/C5 fixture.
- `git diff --check` and integrity verification are run after this ledger write and before staging.

Recovery note: an initial workspace run overlapped earlier still-running Cargo invocations and two autoscale fixtures reported `Database already open. Cannot acquire lock.` The exact `aog-controller --test autoscale` target passed immediately in isolation, and one clean tracked workspace run then passed completely. `cargo audit` and `cargo deny` initially could not create advisory-database lock files under the sandbox's read-only Cargo home; the permitted escalation path ran both required gates successfully without changing repository dependencies.

Commit state: C4/C5 implementation plus this DEVLOG closeout are intentionally unstaged and uncommitted. The workspace Git rule requires the staged-file disclosure and the owner's explicit answer to **“Shall I commit?”** before any commit. Push remains a separate approval after a commit exists.
