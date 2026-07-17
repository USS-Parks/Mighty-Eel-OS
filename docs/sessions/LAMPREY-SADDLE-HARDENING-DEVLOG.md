# Lamprey Saddle WSF + AOG Hardening DEVLOG

Initiative: close the 2026-07-15 WSF/AOG workflow findings and complete the interrupted high-risk review.  
Repository: `USS-Parks/Mighty-Eel-OS` (renamed from `USS-Parks/im-mighty-eel-mai` on 2026-07-16).
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

Status: **PASS** (M2 implementation commit `1700f87`).

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

Status: **PASS** (M2 implementation commit `1700f87`).

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

Status: **PASS — implementation commit `1700f87`; DEVLOG closeout `17fc3ec`; pushed to `main`**.

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

Commit state: C4/C5 implementation and milestone evidence were committed as `1700f87`; the exact-SHA DEVLOG closeout was committed as `17fc3ec`. Both commits carry the canonical `Authored and reviewed by Basho Parks, copyright 2026` footer. The pre-push full no-slop and 79-route policy gates passed, and `origin/main` advanced from `20edccb` through `17fc3ec` on 2026-07-16. This final ledger update records the confirmed remote checkpoint.

---

## M3 — Gateway and tool boundary (in progress)

### LSH-G1 — One final gateway authorization decision

Status: **PASS** (G1/G2 implementation commit `1e921be`).

The vulnerable path remained reachable at the M2 checkpoint: `authorize` returned a valid signed token without consulting its model, route, or classification caveats; each protocol surface independently resolved a mutable target and called a provider; and `RoutingDecision::Denied` became `LocalOnly` plus an audit marker, allowing a local provider to execute. An authenticated holder of a restricted virtual key could therefore invoke an excluded configured model, and content at the router's terminal deny floor could still reach local inference.

`AuthorizedDispatch` is now the sole provider-execution capability used by OpenAI chat, OpenAI streaming, legacy completion, Anthropic messages, and Anthropic streaming. It is created only after virtual-key/token verification, revocation/budget preflight, final alias-to-provider resolution, locality, classification, router decision, signed caveats, and deny-wins policy evaluation are known. Its fields are private; its `complete` and `stream` methods overwrite any later request-model mutation with the frozen authorized upstream target before invoking the private provider handle.

Terminal controls:

- an explicit router deny returns 403 for local and cloud targets in every policy mode;
- a non-empty signed `allowed_models` caveat must contain the inbound public model alias;
- a non-empty signed `allowed_routes` caveat must authorize the resolved provider locality;
- unknown classification fails closed, and a recognized classification above the signed maximum is denied; and
- registry/provider identity mismatch is denied before dispatch.

Changed files:

- `crates/aog-gateway/src/app.rs`;
- `crates/aog-gateway/src/policy.rs`;
- `crates/aog-gateway/src/surface_openai.rs`; and
- `crates/aog-gateway/src/surface_anthropic.rs`.

Focused and compatibility gates:

- `cargo test -p aog-gateway app::tests --lib` — PASS, 6/6; the exhaustive locality/route table proves an explicit deny cannot become allow, excluded aliases stay denied across target transforms, route/classification caveats are terminal, and the provider sink receives the frozen upstream model rather than a post-authorization mutation.
- `cargo test -p aog-gateway` — PASS: 66 library tests plus all gateway integration and doc-test targets; existing valid OpenAI, legacy, Anthropic, provider, policy-mode, metering, tenant-isolation, tokenization, revocation, and budget behavior remains green.
- `cargo clippy -p aog-gateway --all-targets -- -D warnings -A clippy::pedantic` — PASS.
- `cargo fmt --check` — PASS.
- `git diff --check` — PASS.

Change-aware bypass review: repository search finds no direct `provider.complete` or `provider.stream` call in either protocol surface; all five provider sinks consume `AuthorizedDispatch`. Removing the terminal `route.denied` check, the alias caveat check, or the frozen-target overwrite makes its corresponding focused regression fail.

Residual scope: LSH-G2 remains sequentially required for the generated five-surface model/route matrix and the explicit omitted/empty/subset compatibility contract. G1 deliberately preserves current empty-caveat token behavior until that prompt resolves and tests it consistently; no claim is made that `LSF-015`, `LSF-016`, or all protocol instances are closed before G2. G3/G4 and later gateway prompts still own atomic spend and mandatory current revocation.

### LSH-G2 — Protocol parity for model/route caveats

Status: **PASS** (G1/G2 implementation commit `1e921be`).

G2 completes the compatibility/security contract left open by G1. Because `TrustToken.allowed_models` and `allowed_routes` deserialize omitted fields to empty vectors, both omitted and explicit-empty values now mean no gateway inference authority and fail closed. A signed non-empty model caveat authorizes the public inbound alias; a signed non-empty route caveat must authorize the final provider locality. Operator model mapping remains free to translate an authorized public alias to its configured upstream name, but it cannot make an excluded alias authorized and the private dispatch object freezes that upstream name at the sink.

The generated matrix names and exercises all five reachable instances:

- OpenAI chat non-stream;
- OpenAI chat stream;
- OpenAI legacy completion;
- Anthropic message non-stream; and
- Anthropic message stream.

For every surface, the matrix covers model omission/empty, excluded model subset, route omission/empty, excluded route subset, and a valid subset control. This yields the 10 instance regressions required by `REG-LSF-015-gateway-model-matrix` and `REG-LSF-016-gateway-route-matrix`; the shared G1 terminal-deny table simultaneously covers all `LSF-018` locality/route transformations. Existing live-gateway fixtures now issue explicit model and route authority instead of relying on an empty-as-unbounded legacy interpretation.

Additional changed test files:

- `crates/aog-gateway/tests/openai_surface.rs`;
- `crates/aog-gateway/tests/anthropic_surface.rs`;
- `crates/aog-gateway/tests/completions_legacy.rs`;
- `crates/aog-gateway/tests/metering.rs`;
- `crates/aog-gateway/tests/policy_modes.rs`; and
- `crates/aog-gateway/tests/tenant_isolation.rs`.

Gates:

- `cargo test -p aog-gateway app::tests --lib` — PASS, 8/8, including the generated five-surface matrix and omitted-field deserialization regression.
- `cargo test -p aog-gateway` — PASS: 68 library tests plus all gateway integration and doc-test targets.
- `cargo clippy -p aog-gateway --all-targets -- -D warnings -A clippy::pedantic` — PASS.
- `cargo fmt --check` — PASS.
- `git diff --check` — PASS.

Closure statement: the original excluded-model, excluded-route, and explicit-router-deny paths no longer reproduce at the shared authorization-to-provider boundary. Repository search confirms the five concrete sinks can invoke providers only through `AuthorizedDispatch`; the caveat matrix fails if empty authority is treated as allow or if either signed subset check is removed. Existing valid OpenAI, legacy, and Anthropic compatibility tests remain green with explicit signed authority. `LSF-015`, `LSF-016`, and `LSF-018` are closed for their 15 recorded protocol instances by the combined G1/G2 evidence.

Residual scope: atomic reservations (`LSH-G3`), mandatory current revocation (`LSH-G4`), preflight amplification, endpoint policy, response bounds, authoritative metering, and the adversarial live compatibility gate remain open. No M3 milestone claim is made.

Commit state: G1/G2 implementation, explicit-authority fixtures, and prompt evidence were committed as `1e921befb3e6f47cee89a6ed8d3abd2af5fad6bf`; the exact-SHA DEVLOG closeout was committed as `32a4a557781fbb8159f4b379d5e0a8d64215ad59`. Both commits are SSH-signed and carry the canonical `Authored and reviewed by Basho Parks, copyright 2026` footer. The manually executed Git Bash pre-push gate passed the full-tree no-slop scan and the 79-route policy inventory. `origin/main` advanced from `ffc6262` through `32a4a55` on 2026-07-16. This final ledger update records the confirmed remote checkpoint.

### LSH-G3 — Atomic gateway spend

Status: **PASS** (implementation commit `7e8c05c`).

The former gateway budget path performed a non-reserving preflight and charged
only after provider completion. Concurrent stream and non-stream calls could all
observe remaining authority, execute, and then push the lineage beyond its token,
USD, or call ceiling. Streaming cancellation was metered on drop, but it did not
hold authority while the stream was in flight.

All five OpenAI/Anthropic execution surfaces now share the LSH-A4
`ReservationLedger` through `AppState`. After the immutable G1/G2 decision and
before provider execution, the gateway atomically reserves a conservative input +
maximum-output estimate, its priced USD ceiling, and one provider call against
the tenant/root-lineage/gateway namespace. A denied reservation returns 402 before
the provider is touched.

Settlement behavior:

- non-stream success atomically replaces the estimate with final usage and
  releases unused authority;
- provider failure or any pre-execution abort drops and releases the pending
  reservation;
- stream creation moves the reservation into `StreamMeter`, whose drop settles
  exactly once on clean completion, provider error, or client cancellation;
- the legacy runtime spend ledger remains updated for compatibility with existing
  preflight/telemetry paths, and provider executions now count against the call
  axis rather than recording zero calls; and
- if final usage exceeds its reservation and cannot fit, reconciliation commits
  the bounded pre-authorized amount and returns a fail-closed overrun instead of
  releasing the side effect as free usage. G7/G8 still own response bounds and
  locally authoritative usage; G3 does not treat provider usage as trusted beyond
  the existing metering contract.

Changed files:

- `crates/fabric-token/src/spend.rs`;
- `crates/aog-gateway/src/app.rs`;
- `crates/aog-gateway/src/meter.rs`;
- `crates/aog-gateway/src/surface_openai.rs`; and
- `crates/aog-gateway/src/surface_anthropic.rs`.

Gates:

- `cargo test -p fabric-token` — PASS: 49 unit/integration tests plus doc tests;
- `cargo test -p aog-gateway` — PASS: 70 library tests plus every gateway
  integration and doc-test target;
- the G3 100-way mixed stream/non-stream barrier admits exactly five calls and
  commits exactly 10,000 tokens, 375 cents, and five calls without crossing any
  axis;
- provider-failure drop releases capacity, while simulated stream cancellation
  commits one call exactly once; estimate-to-actual release and bounded-overrun
  regressions pass in `fabric-token`;
- `cargo clippy -p fabric-token -p aog-gateway --all-targets -- -D warnings -A
  clippy::pedantic` — PASS;
- `cargo fmt --check` — PASS; and
- `git diff --check` — PASS.

Closure statement: `LSF-017` is closed for the five gateway execution surfaces at
the shared authorization-to-provider boundary. Atomic spend no longer depends on
a post-execution check/charge race, and cancellation/failure cannot leave a live
reservation or charge it twice. LSH-G4 (mandatory current revocation) is the next
sequential prompt; no broader M3 milestone claim is made.

Commit state: implementation and prompt evidence were committed as
`7e8c05c1a4c4176fd596fa15de495c0054434fdf`; the exact-SHA DEVLOG closeout was
committed as `fffa6ea1945e2162698a6639c30ad82da415b905`. Both commits are
SSH-signed and carry the canonical `Authored and reviewed by Basho Parks,
copyright 2026` footer. The pre-push full no-slop and 79-route policy gates
passed, and `origin/main` advanced from `01dfba2` through `fffa6ea` on
2026-07-16. This final ledger update records the confirmed remote checkpoint.

### LSH-G4 — Mandatory gateway revocation

Status: **PASS** (implementation pending commit).

Production gateway construction now requires a revocation source and loads a
current, verified snapshot before a provider registry or listener can be made
reachable. The gateway holds revocation state in a monotonic store: absent,
expired, malformed, and rollback snapshots fail closed, while an identical
current snapshot may be re-read without manufacturing a sequence advance. The
legacy constructor remains explicitly development/test-only and cannot be used
by the production startup branch.

The declared enforcement bound is the next privileged gateway step after a
revocation becomes visible through OpenBao: authorization refreshes immediately
before atomic spend reservation, immediately before provider dispatch, and
before every provider stream continuation frame. A newly revoked stream emits a
protocol-native authorization error and terminates without a synthetic normal
completion marker. The shared check covers token, subject, key, issuer, policy
bundle, tenant, and tenant-ring revocation dimensions for all five OpenAI and
Anthropic execution surfaces.

Changed files:

- `crates/fabric-revocation/src/lib.rs`;
- `crates/fabric-revocation/tests/revocation.rs`;
- `crates/aog-gateway/src/lib.rs`;
- `crates/aog-gateway/src/main.rs`;
- `crates/aog-gateway/src/app.rs`;
- `crates/aog-gateway/src/meter.rs`;
- `crates/aog-gateway/src/surface_openai.rs`;
- `crates/aog-gateway/src/surface_anthropic.rs`; and
- `crates/aog-gateway/tests/kill_switch.rs`.

Gates:

- `cargo test -p fabric-revocation` — PASS: 8 tests plus doc tests, including
  explicit absent-state failure and current monotonic sequence acceptance;
- `cargo test -p aog-gateway app::tests --lib` — PASS: 12 focused tests,
  including a 35-case matrix (five surfaces times seven revocation dimensions)
  that records zero provider calls after denial;
- the stream-continuation regression publishes a newer revocation sequence
  after dispatch and proves the next frame is denied before provider polling;
- live `cargo test -p aog-gateway --test kill_switch -- --nocapture` against an
  isolated loopback OpenBao dev instance — PASS, with the test enabled: an
  absent production snapshot prevents startup, a newer token revocation stops
  the next call, and replaying the lower baseline sequence is rejected;
- `cargo test -p aog-gateway` — PASS: 72 library tests plus every gateway
  integration and doc-test target;
- `cargo clippy -p fabric-revocation -p aog-gateway --all-targets -- -D
  warnings -A clippy::pedantic` — PASS;
- `cargo fmt --check` — PASS; and
- `git diff --check` — PASS.

Closure statement: `LSF-019` is closed for the five gateway execution surfaces.
Production cannot start without current revocation state, a stale or rollback
snapshot cannot replace held authority, and every provider dispatch or stream
continuation crosses the same current-state check. LSH-G5 remains sequentially
responsible for bounding public preflight amplification and adding safe negative
caching; G4 intentionally prioritizes immediate revocation visibility and makes
no broader M3 milestone claim.

Commit state: implementation and prompt evidence were committed as
`bcd332f2c5f3f3c33eb0daf70adabe77518260fb`. The commit is SSH-signed and
carries the canonical `Authored and reviewed by Basho Parks, copyright 2026`
footer. The exact-SHA DEVLOG closeout was committed as
`eccc6a425239a23e8a832acddd624a23a007cfca`. Both commits passed the full
pre-push no-slop and 79-route policy gates, and `origin/main` advanced from
`2590b50` through `eccc6a4` on 2026-07-16. This final ledger update records the
confirmed remote checkpoint.

### LSH-G5 — Preflight authentication amplification controls

Status: **PASS** (implementation pending commit).

Virtual-key admission is now bounded before unauthenticated input can fan out
into OpenBao. Both Bearer and Anthropic `x-api-key` paths converge on the same
resolver validation: keys remain opaque for compatibility, but must be 1–128
bytes of ASCII URL-safe material. Whitespace, separators, control/non-ASCII
material, empty values, and oversized values fail as 401 before hashing, AppRole
login, or KV work.

Syntactically valid candidates enter a shared admission controller with default
production bounds of 32 concurrent resolutions and 128 newly admitted
resolutions per one-second fixed window. Only SHA-256 key hashes enter admission
memory. A duplicate hash already in flight receives a stable 429 instead of
starting another login/read. Confirmed OpenBao 404 results are cached for one
second in a 1,024-entry FIFO/expiry-bounded negative cache; transport, auth,
protocol, token, and revocation results are never negative-cached. The public
`GatewayAdmissionConfig` override permits tighter deployment/test bounds without
changing the gateway request API.

This placement preserves G4: successfully resolved tokens and every subsequent
privileged/stream step still refresh current revocation state from OpenBao.
Negative caching can delay recognition of a newly provisioned previously absent
key by at most its one-second TTL, but it cannot make a revoked or stale token
authorized.

Changed files:

- `crates/aog-gateway/src/lib.rs`;
- `crates/aog-gateway/src/http.rs`; and
- `docs/sessions/LAMPREY-SADDLE-HARDENING-DEVLOG.md`.

Gates:

- `cargo test -p aog-gateway
  adversarial_bearers_bound_backend_calls_and_memory --lib` — PASS against an
  instrumented local OpenBao wire mock;
- 500 malformed candidates caused zero AppRole logins and zero KV reads;
- 500 concurrent requests for one validly shaped unknown key collapsed to one
  login/read, then remained a stable cached 401; concurrent followers received
  bounded 429 responses;
- 500 distinct validly shaped unknown keys under a test limit of 8 concurrent,
  16 per 60-second window, and 8 negative entries produced no more than 16
  login/read pairs, retained no more than 8 hashes, and left zero in-flight
  entries;
- `cargo test -p aog-gateway` — PASS: 73 library tests plus every gateway
  integration and doc-test target;
- `cargo clippy -p aog-gateway --all-targets -- -D warnings -A
  clippy::pedantic` — PASS;
- `cargo fmt --check` — PASS; and
- `git diff --check` — PASS.

Closure statement: `LSF-020` is closed at the shared public virtual-key
resolution boundary. Unauthenticated input can no longer create unbounded
OpenBao work or unbounded cache growth, and overload is an explicit 429 rather
than backend queue accumulation. LSH-G6 is the next sequential prompt; no M3
milestone claim is made.

Commit state: implementation and prompt evidence were committed as
`27bc5819af01df14f71ededecaa0eae81de00183`. The commit is SSH-signed and
carries the canonical `Authored and reviewed by Basho Parks, copyright 2026`
footer. The initial exact-SHA DEVLOG closeout is
`fa78113280aa4744efed7adcfc4c33dc09830d08`. The first pre-push route-policy
gate then identified the instrumented test server's two literal mock paths as
unregistered production routes; the bounded fixture-only repair was committed
as `aadcf36b2afccbfe3195bd583aa7333797b2e7eb`. The focused adversarial test,
strict clippy, file integrity, full no-slop scan, and 79-route policy inventory
all pass after that repair. Its ledger record is
`f45e8a291d4a6f407d224fa81dc3d5019b907db0`. The complete four-commit G5
range passed signature/footer verification and the full pre-push no-slop and
79-route policy gates; `origin/main` advanced from `f01aa49` through `f45e8a2`
on 2026-07-16. This final ledger update records the confirmed remote checkpoint.

### LSH-G6 — Provider endpoint and credential policy

Status: **PASS** (implementation commit `3b48b52`).

The production seam previously parsed provider URLs and rejected credentialed
HTTP plus non-loopback plaintext local backends, while provider adapters still
accepted and retained raw base URLs. A configured `AOG_LOCAL_BASE` was trusted
as sovereign solely because the registry named it `local`; DNS answers were not
inspected or pinned, private/metadata destinations were not comprehensively
blocked, and the adapter constructor itself did not require evidence that the
destination had passed startup policy. Redirect following was already disabled
in the shared client and was retained rather than reimplemented.

`ApprovedProviderEndpoint` is now the capability required by both provider
adapters. Startup canonicalizes each exact origin, rejects embedded credentials
and query/fragment components, resolves every hostname before any OpenBao work
or listener bind, validates every returned address, and carries the accepted
socket set into reqwest's per-host resolver override. This pins the approved DNS
answer set for the client lifetime, so a later rebind cannot redirect the
credentialed request. Unspecified, multicast, broadcast, link-local/metadata,
private, shared-address, loopback, unique-local IPv6, and IPv4-mapped forms fail
closed unless the exact origin has the applicable explicit authority; metadata
and link-local destinations are never provider destinations.

The configuration contract is explicit:

- credentialed providers require HTTPS; only an explicitly enabled development
  fixture may use a credentialed loopback endpoint;
- `AOG_LOCAL_ALLOWED_ORIGINS` binds the security-significant `local` provider
  name to exact canonical origins (the production default is the loopback local
  model at `http://127.0.0.1:8000`);
- `AOG_PRIVATE_PROVIDER_ALLOWED_ORIGINS` separately authorizes an intentional
  private HTTPS origin without suffix or wildcard matching; and
- a non-loopback HTTP development fixture requires both its exact local origin
  and `AOG_ALLOW_INSECURE_PROVIDER_FIXTURES=1`. The appliance demo declares that
  narrow override for `mock-llm`; production cannot activate it.

The only raw-URL adapter path left is the public `loopback_fixture` constructor
used by integration tests. It accepts only an IP-literal loopback URL, so tests
cannot silently create an arbitrary/private/public credential sink. The existing
OpenAI, Anthropic, gateway-surface, metering, tenant, and managed-controller
fixtures were migrated to that constructor. No dependency or lockfile changed.

Changed files:

- `crates/aog-gateway/src/{main.rs,posture.rs,provider.rs}`;
- `crates/aog-gateway/src/provider/{openai.rs,anthropic.rs}`;
- gateway provider/surface/metering/policy/tenant integration fixtures;
- `crates/aog-controller/tests/{managed_gateway.rs,managed_toolproxy.rs}`; and
- `deployment/appliance/{docker-compose.yml,.env.example,README.md}`.

Gates:

- `cargo test -p aog-gateway posture::tests --lib` — PASS, 5/5. HTTP,
  metadata IP, arbitrary local origin, and mixed public/private DNS answers are
  denied before provider construction; approved public/private answers retain
  their exact pinned socket sets; and the development HTTP override is both
  allowlisted and profile-bound;
- `cargo test -p aog-gateway --test providers` — PASS, 2/2. OpenAI Bearer and
  Anthropic `x-api-key` requests each receive a cross-origin 307; both surface
  the redirect as an upstream error and the credential sink records zero hits;
- `cargo test -p aog-gateway` — PASS: 73 library tests plus every gateway
  integration and doc-test target;
- `cargo test -p aog-controller --test managed_gateway --test
  managed_toolproxy` — PASS, 2/2;
- `cargo clippy -p aog-gateway --all-targets -- -D warnings -A
  clippy::pedantic` — PASS;
- `python deployment/appliance/validate_profile.py --profile demo
  deployment/appliance/docker-compose.yml` — PASS after providing the CI-listed
  PyYAML prerequisite in ignored `target/`; validator regression suite PASS,
  16/16 (one environment-only warning because the transient lane did not include
  the repository's optional pytest-asyncio plugin);
- `cargo fmt --check` — PASS; and
- `git diff --check` — PASS.

Closure statement: `LSF-021`, `LSF-022`, and `LSF-023` are closed at the
provider-construction and connection boundary. Provider credentials and governed
prompts cannot be dispatched over unapproved plaintext, to an arbitrary semantic
`local` destination, through a mixed/private DNS answer, to metadata/link-local
space, or across a redirect. DNS changes intentionally require a gateway restart
and revalidation. G7 remains responsible for response/body/SSE bounds and truthful
termination; G8 for authoritative usage; G9 for the complete adversarial live
compatibility matrix. No broader M3 milestone claim is made.

Commit state: implementation and prompt evidence were committed as
`3b48b521c8b43e15cb4792d329a70bd1de8a7c26`. The commit is SSH-signed and
carries the canonical `Authored and reviewed by Basho Parks, copyright 2026`
footer. The exact-SHA DEVLOG closeout was committed as
`4f6d6e9be2f9021ebc7da98a9d7f70eb01a92643`; it is also SSH-signed and carries
the canonical footer. Both outgoing commits passed exact-footer verification;
the pre-push full-tree no-slop and 79-route policy gates passed; and
`origin/main` advanced from `a35dffd` through `4f6d6e9` on 2026-07-16. This
final ledger update records the confirmed G6 remote checkpoint.

### LSH-G7 — Bounded provider responses and truthful stream termination

Status: **PASS** (implementation commit `b3bccb8`).

The shared provider boundary previously used unbounded `Response::json` and
`Response::text`, buffered SSE into a newline-delimited `String` without a line,
frame, or total-byte cap, and configured only connect plus per-read idle
timeouts. A provider that continuously trickled bytes could therefore retain a
task and grow memory indefinitely. On the client side, both gateway surfaces
discarded provider errors and premature EOF: OpenAI synthesized
`finish_reason:"stop"` plus `[DONE]`, while Anthropic synthesized `end_turn`
plus `message_stop` over partial output.

`ProviderLimits` now defines one shared production contract: 10-second connect,
120-second idle, and 15-minute total deadlines; 128 headers / 64 KiB aggregate
header bytes; 8 MiB non-stream body; 64 KiB bounded error body; and 16 MiB SSE
total, 1 MiB line, and 2 MiB frame ceilings. The reqwest client enforces both
idle and total deadlines. Every response validates header count/bytes and any
declared content length before body consumption. Success JSON is collected
incrementally to the body cap; error text is retained only to its smaller cap
and marked truncated rather than consuming an attacker-sized error.

The shared SSE parser now buffers raw bytes across transport chunks, validates
UTF-8 only at complete line boundaries, counts total/line/frame bytes with
checked arithmetic, and emits explicit `Limit`, `Decode`, `Transport`, or
`Truncated` errors. EOF without a protocol terminal event is always truncation.
Provider adapters preserve the authenticated terminal reason: OpenAI requires a
real `finish_reason` before accepting `[DONE]`; Anthropic requires a
`stop_reason` before accepting `message_stop`. Usage frames after the semantic
finish remain observable before the final sentinel.

Both outward surfaces now emit a protocol-native error event on upstream error,
limit, timeout, malformed data, or premature EOF. Only a verified provider
terminal frame can produce OpenAI `[DONE]` or Anthropic `message_stop`; the real
finish/stop reason is preserved (`length` maps to `max_tokens` for the Anthropic
surface). Revocation continues to use its distinct authorization error and also
never emits a success sentinel. Stream-meter settlement remains drop-based, so
faulted and cancelled streams are still receipted/charged exactly once.

Changed files:

- `crates/aog-gateway/src/provider.rs`;
- `crates/aog-gateway/src/provider/{openai.rs,anthropic.rs}`;
- `crates/aog-gateway/src/surface_{openai,anthropic}.rs`;
- `crates/aog-gateway/src/meter.rs`; and
- `crates/aog-gateway/tests/{providers.rs,openai_surface.rs,metering.rs}`.

No dependency, lockfile, public route, or deployment-manifest change was needed.
Existing compatible provider fixtures were corrected to emit the required real
OpenAI finish frame before their usage frame and `[DONE]`; accepting their old
sentinel-only shape would have preserved the vulnerability.

Gates:

- `cargo test -p aog-gateway --test providers -- --nocapture` — PASS, 3/3.
  The adversarial provider fixture proves oversized OpenAI and Anthropic JSON,
  excess headers, newline-free SSE, oversized multi-line frames, malformed JSON,
  `[DONE]` without `finish_reason`, premature EOF, Anthropic `message_stop`
  without `stop_reason`, and a 20 ms trickle stream under a 75 ms total deadline
  all fail boundedly;
- `cargo test -p aog-gateway provider_failure_never --lib` — PASS, 2/2.
  Partial output followed by provider truncation yields an explicit error and
  contains no OpenAI `stop`/`[DONE]` or Anthropic `end_turn`/`message_stop`;
- `cargo test -p aog-gateway` — PASS: 75 library tests plus every gateway
  integration and doc-test target; valid OpenAI/Anthropic streaming, metering,
  policy, tokenization, revocation, and tenant behavior remain green;
- `cargo clippy -p aog-gateway --all-targets -- -D warnings -A
  clippy::pedantic` — PASS;
- `cargo fmt --check` — PASS; and
- `git diff --check` — PASS.

Closure statement: `LSF-024` and reachable `LSD-005/006` response-fault and
false-success instances are closed at the shared response parser plus both
client-protocol surfaces. G8 still owns authoritative local usage and safe
reconciliation; G9 owns the complete two-tenant adversarial live compatibility
matrix. No broader M3 milestone claim is made.

Commit state: implementation and prompt evidence were committed as
`b3bccb89c1a811031393cb4082399c388018b6f1`. The commit is SSH-signed and
carries the canonical `Authored and reviewed by Basho Parks, copyright 2026`
footer. The exact-SHA DEVLOG closeout was committed as
`a0b130ba48219e3e5a7832f58c9591906c4600d9`; it is also SSH-signed and carries
the canonical footer. Both outgoing commits passed exact-footer verification;
the pre-push full-tree no-slop and 79-route policy gates passed; and
`origin/main` advanced from `34bb6d8` through `a0b130b` on 2026-07-16. This
final ledger update records the confirmed G7 remote checkpoint.

### LSH-G8 — Authoritative metering

Status: **PASS** (implementation commit `be2e900`).

Provider usage was previously trusted at the settlement boundary. Non-stream
OpenAI, legacy-completion, and Anthropic responses passed provider-controlled
counts directly into the reservation, receipt, price, and compatibility spend
ledgers. Streams fell back to local accounting only when a field was exactly
zero, so any positive low count suppressed the request estimate or observed
output. The output fallback also rounded byte counts down.

The gateway now treats provider usage as evidence. One shared reconciler computes
a saturated, round-up local lower bound at four UTF-8 bytes per token and selects
the per-field maximum of that bound and the provider report. Non-stream paths
measure the message text actually dispatched after tokenization plus the raw
provider response before detokenization. Stream paths retain the conservative
input estimate, accumulate output bytes with saturating arithmetic, and merge
late or split provider frames by per-field maximum before the same reconciliation.
A missing, zero, low positive, contradictory, or late report therefore cannot
reduce settlement below locally observed usage; high provider evidence remains
visible and is conservatively reconciled through the bounded G3 reservation.

Every real `GatewayReceipt` now carries `usage_reconciliation` with the
`local_estimate`, `provider_reported`, and authoritative `final_usage` explicitly.
The receipt's existing token and spend fields, atomic reservation settlement, and
legacy runtime spend ledger all use `final_usage`. The outward compatibility
response preserves the provider-reported usage fields, while the append-only
receipt retains both that evidence and the gateway's authoritative accounting.
The reconciliation field is omitted only for historical/hand-built pre-G8
fixtures, preserving their byte and hash shape.

Changed files:

- `crates/aog-gateway/src/meter.rs`;
- `crates/aog-gateway/src/route.rs`;
- `crates/aog-gateway/src/app.rs`;
- `crates/aog-gateway/src/surface_{openai,anthropic}.rs`;
- `crates/aog-gateway/tests/tokenization.rs`; and
- this DEVLOG.

No dependency, lockfile, public route, provider protocol, or deployment-manifest
change was needed.

Gates:

- focused G8 unit fixtures — PASS. Missing-normalized, explicit zero, positive
  low, high, contradictory-field, stream-late/split, and positive-low streaming
  reports all settle at or above the local policy; partial byte groups round up;
- non-stream receipt fixture — PASS. The chain-verifying receipt contains the
  local estimate, provider evidence, and final authoritative usage explicitly;
- `cargo test -p aog-gateway` — PASS: 78 library tests plus every gateway
  integration and doc-test target. Valid OpenAI chat/stream, legacy completion,
  Anthropic message/stream, metering, budgets, policy, tokenization, revocation,
  and tenant behavior remain green;
- `cargo clippy -p aog-gateway --all-targets -- -D warnings -A
  clippy::pedantic` — PASS;
- `cargo fmt --check` — PASS; and
- `git diff --check` — PASS.

Closure statement: `LSF-025` is closed for its four OpenAI/Anthropic stream and
non-stream instances at the shared reconciliation and receipt boundary. Provider
counts can no longer suppress local accounting, and the evidence-to-final
decision is auditable in the receipt chain. LSH-G9 owns the complete two-tenant
adversarial live compatibility matrix and is the next sequential prompt. No
broader M3 milestone claim is made.

Commit state: implementation and prompt evidence were committed as
`be2e900217b3b3aa77a2b8d6c1713a82a6ea612e`. The commit is SSH-signed and
carries the canonical `Authored and reviewed by Basho Parks, copyright 2026`
footer. The exact-SHA DEVLOG closeout was committed as
`e6bc6c26ab4d1f331289d3b48d24db05e50fb92d`; it is also SSH-signed and carries
the canonical footer. Both outgoing commits passed exact-footer verification;
the pre-push full-tree no-slop and 79-route policy gates passed; and
`origin/main` advanced from `6a19392` through `e6bc6c2` on 2026-07-16. This
final ledger update records the confirmed G8 remote checkpoint.

### LSH-G9 — Gateway live compatibility matrix

Status: **PASS** (implementation commit `1c4d290`).

The existing OpenAI and Anthropic surface tests used raw `reqwest` requests that
matched SDK wire shapes but explicitly deferred real SDK execution. They also
split valid protocol checks, policy, budget, revocation, tenant isolation,
redirects, response faults, and false usage across independent fixtures. That
left no single live proof that vendor-maintained clients remained compatible
after the G1–G8 security boundaries were composed.

`official_client_compat.rs` now provisions a production `Gateway` against live
OpenBao with a signed, current revocation snapshot; seeds separate tenant A,
tenant B, budget, and revocation virtual keys; launches both governed provider
adapters behind one adversarial upstream; and invokes
`official_client_probe.py` as a child process. The probe uses pinned official
OpenAI Python `2.45.0` and Anthropic Python `0.116.0` clients with only base URL
and virtual-key changes.

The live matrix proves all five execution surfaces: OpenAI chat non-stream,
OpenAI chat stream, legacy OpenAI completion, Anthropic message non-stream, and
Anthropic message stream. It also proves two-tenant receipt isolation, enforced
PHI route denial, a first authorized call followed by atomic budget 402, a valid
call followed by bridge-signed live revocation 403, redirect refusal with zero
credential-sink hits, malformed and oversized JSON errors, truthful truncated
OpenAI/Anthropic streams, and false provider usage reconciled from reported
output `1` to the locally observed authoritative `8` in a chain-verifying
receipt. Compatibility responses continue to expose provider evidence in the
official SDK's native types.

The CI live-trust job now installs those exact SDK versions and runs this matrix
against its disposable OpenBao service. The local full live suite also exposed
one stale pre-G3 metering fixture that expected a 1,400-token cap to admit the
default 4,096-token reservation. The fixture now reserves exactly 1,500 tokens
and settles the provider's 1,500-token usage, preserving its intended first-call
success and next-call 402 assertion under the atomic reservation contract.

Changed files:

- `.github/workflows/ci.yml`;
- `crates/aog-gateway/tests/official_client_compat.rs`;
- `crates/aog-gateway/tests/official_client_probe.py`;
- `crates/aog-gateway/tests/metering.rs`; and
- this DEVLOG.

No product runtime, Cargo dependency/lockfile, public route, or deployment
manifest change was needed. Official SDK packages are test-only and pinned in
the live CI lane.

Gates:

- `cargo test -p aog-gateway --test official_client_compat -- --nocapture` with
  live OpenBao — PASS, 1/1. Both official SDK phases report PASS and the complete
  adversarial matrix closes;
- `cargo test -p aog-gateway --test metering
  streamed_call_accrues_spend_and_cap_refuses_next_call -- --nocapture` with live
  OpenBao — PASS, 1/1 after correcting the stale reservation fixture;
- `cargo test -p aog-gateway` with live OpenBao and the pinned official SDKs —
  PASS: 78 library tests, all 16 integration tests, and doc tests. The previous
  OpenAI/Anthropic wire, policy, budget, revocation, tenant, metering,
  tokenization, response-bound, and provider-origin gates remain green;
- `cargo clippy -p aog-gateway --all-targets -- -D warnings -A
  clippy::pedantic` — PASS;
- Ruff `0.15.21` over `official_client_probe.py` — PASS;
- `cargo fmt --check` — PASS; and
- `git diff --check` — PASS.

Closure statement: the gateway/provider half of M3 is complete. The live matrix
confirms the composed closures for `LSF-015`–`LSF-025` and reachable
`LSD-005/006` without breaking valid official-client behavior. LSH-T1 is the
next sequential prompt; the broader M3 milestone remains open through LSH-T6.

Commit state: implementation and prompt evidence were committed as
`1c4d29078a0f79396670efec7ac0c3f6f95f6b1d`. The commit is SSH-signed and
carries the canonical `Authored and reviewed by Basho Parks, copyright 2026`
footer. The exact-SHA DEVLOG closeout was committed as
`a373cf10c7e0fcd4271917ff8e589b200a58c51d`; it is also SSH-signed and carries
the canonical footer. Both outgoing commits passed exact-footer verification;
the pre-push full-tree no-slop and 79-route policy gates passed; and
`origin/main` advanced from `0c2ddc4` through `a373cf1` on 2026-07-16. This
final ledger update records the confirmed G9 remote checkpoint.

### LSH-T1 — Server-derived provenance

Status: **PASS** (implementation commit
`5f92865ec807846934f89c14defaa5daadb5fbbe`).

`InvokeContext.untrusted` let an integrating caller assert that model context
was trusted. With no approval inbox, `false` admitted a side-effecting tool call
directly to `ToolExecutor::execute`. That was the confirmed `LSF-026` confused-
deputy path.

The caller-controlled flag is removed from the public invocation context and
all consumers. The proxy now treats tool-result, missing, unknown, and
mismatched lineage as untrusted. Read-only calls remain available, while every
mutation requires an approval decision; without an inbox it is denied before
credential minting or executor entry.

Each completed result now creates a server-owned provenance binding keyed by
session and tied to the executing tool, authorizing signed-grant/trust-token id,
mission, call id, and exact receipt-chain head. A matching binding is copied
into the next receipt as `provenance_source`; callers cannot construct or clear
that lineage through `InvokeContext`. The adversarial regression performs a
read whose result contains an injected `delete_all` instruction, then submits a
plain caller context. The proxy derives the prior result receipt itself and
denies the mutation. Separate regressions prove missing lineage routes to a
configured inbox or denies when no inbox exists, while read-only execution is
unchanged.

The affected-crate all-target lint also found the G4 live revocation fixture
still calling the removed optional `.with_revocation_path` builder. The fixture
now uses `Gateway::new_production(..., REV_PATH)` and loads the already-seeded
mandatory baseline snapshot. This is a compile-only compatibility repair to the
existing live gate, not a new runtime path.

Changed files:

- `crates/aog-toolproxy/src/lib.rs`;
- `crates/aog-toolproxy/src/receipt.rs`;
- `crates/aog-toolproxy/src/guard.rs`;
- `crates/aog-toolproxy/src/mission.rs`;
- `crates/aog-approvals/src/lib.rs`;
- `crates/aog-conformance/tests/robustness_conformance.rs`;
- `crates/aog-controller/tests/managed_toolproxy.rs`;
- `crates/aog-controller/tests/live_revocation.rs`; and
- this DEVLOG.

Gates:

- `cargo test -p aog-toolproxy
  tests::reg_lsf_026_server_provenance_blocks_injected_mutation -- --exact` —
  PASS, 1/1 named adversarial regression;
- `cargo test -p aog-toolproxy -p aog-approvals` — PASS: 53 toolproxy tests,
  five approval tests, and doc tests. The server-derived injection, missing-
  lineage approval/deny, receipt binding, mutation, credential, mission,
  guardrail, session, and egress regressions are green;
- `cargo test -p aog-conformance --test robustness_conformance` — PASS, 11/11;
- `cargo test -p aog-controller --test managed_toolproxy --test
  live_revocation` — PASS, 2/2 in the current prerequisite-free lane;
- `cargo clippy -p aog-toolproxy -p aog-approvals -p aog-conformance -p
  aog-controller --all-targets -- -D warnings -A clippy::pedantic` — PASS;
- source search for `InvokeContext` `untrusted` fields/defaults and the removed
  revocation builder — clean;
- `cargo fmt --check` — PASS; and
- `git diff --check` — PASS.

Closure statement: `LSF-026` is closed at the toolproxy request-context and
executor boundary. Caller-supplied provenance flags/defaults no longer exist,
and injected tool-result instructions cannot reach mutation without approval.
LSH-T2 is the next sequential prompt. The broader M3 milestone remains open
through LSH-T6.

Commit state: implementation and prompt evidence were committed as
`5f92865ec807846934f89c14defaa5daadb5fbbe`. The commit is SSH-signed and
carries the canonical `Authored and reviewed by Basho Parks, copyright 2026`
footer. The exact-SHA DEVLOG closeout was committed as
`be08d5dab72147dee188cce9a2f3467562008e59`; it is also SSH-signed and carries
the canonical footer. Both outgoing commits passed exact-footer and signature
verification; the pre-push full-tree no-slop and 79-route policy gates passed;
and `origin/main` advanced from `d97a8a7` through `be08d5d` on 2026-07-16. This
final ledger update records the confirmed T1 remote checkpoint.

### LSH-T2 — Atomic mission and guard enforcement

Status: **PASS** (implementation commit
`e168ca6d3bb6a56178c6c3d3d7e852fcef0f7fae`).

The mission and operator blast-radius controls previously checked counters,
released their locks, awaited approval, and only then recorded usage. Parallel
calls could therefore observe the same pre-charge state and all execute beyond
the configured call, spend, or distinct-system ceiling. The same boundary also
treated absent caller-supplied `system` metadata as unconstrained.

Toolproxy now reuses the LSH-A4 `ReservationLedger` for mission call/spend,
operator call, and operator distinct-system authority. Reservations occur
before asynchronous approval; denial and early return release pending authority
automatically; admitted calls commit it before credential minting/execution.
Mission and guard ledgers are independent so one call does not double-charge a
shared axis, while both use immutable tenant/root-lineage/mission keys rather
than session, profile, or call IDs.

`InvokeContext` can no longer be built with a public struct literal containing
caller-selected lineage or system metadata. `from_verified_request` derives the
tenant, immutable token root, grant/principal identity, and final canonical
system from `VerifiedRequestContext`; `unverified` carries neither. A constrained
mission treats missing canonical system identity as a deviation requiring
approval or denial. An active operator call/system cap hard-denies missing
authenticated lineage, and an active system cap hard-denies a missing canonical
target.

The deterministic `REG-LSF-027` regression holds the first call inside approval
while a second call rotates session, profile, and call IDs. Mission call,
mission spend, and operator call ceilings each admit exactly one executor entry.
The `LSD-009` regression denies omitted system metadata, permits the same
canonical system after session rotation, and blocks fan-out to a second system.

Changed files:

- `Cargo.lock`;
- `crates/aog-toolproxy/Cargo.toml`;
- `crates/aog-toolproxy/src/lib.rs`;
- `crates/aog-toolproxy/src/mission.rs`;
- `crates/aog-toolproxy/src/guard.rs`;
- `crates/aog-approvals/src/lib.rs`;
- `crates/aog-conformance/tests/robustness_conformance.rs`;
- `crates/aog-controller/tests/managed_toolproxy.rs`; and
- this DEVLOG.

Gates:

- `cargo test -p aog-toolproxy
  tests::reg_lsf_027_atomic_mission_and_guard_reservations -- --exact` — PASS,
  1/1 deterministic concurrency regression;
- `cargo test -p aog-toolproxy
  tests::reg_lsd_009_missing_and_multi_system_fanout_fail_closed -- --exact` —
  PASS, 1/1 omitted-target and fan-out regression;
- `cargo test -p aog-toolproxy -p aog-approvals` — PASS: 55 toolproxy tests,
  five approval tests, and doc tests;
- `cargo test -p aog-conformance --test robustness_conformance` — PASS, 11/11;
- `cargo test -p aog-controller --test managed_toolproxy --test
  live_revocation` — PASS, 2/2 in the current prerequisite-free lane;
- `cargo clippy -p aog-toolproxy -p aog-approvals -p aog-conformance -p
  aog-controller --all-targets -- -D warnings -A clippy::pedantic` — PASS;
- source search confirms the non-atomic `Mission::check`/`record` and
  `TaskUsage` paths are removed and canonical system state is private;
- `cargo fmt --check` — PASS; and
- `git diff --check` — PASS.

Closure statement: both `LSF-027` instances are closed by A4 reservations, and
the two reachable `LSD-009` omission paths are closed at the toolproxy boundary.
LSH-T3 is the next sequential prompt. The broader M3 milestone remains open
through LSH-T6.

Commit state: implementation and prompt evidence were committed as
`e168ca6d3bb6a56178c6c3d3d7e852fcef0f7fae`. The commit is SSH-signed and
carries the canonical `Authored and reviewed by Basho Parks, copyright 2026`
footer. The exact-SHA DEVLOG closeout was committed as
`1fc190a271766ce834eee33a9423957228985ce4`; it is also SSH-signed and carries
the canonical footer. Both outgoing commits passed exact-footer and signature
verification; the pre-push full-tree no-slop and 79-route policy gates passed;
and `origin/main` advanced from `1dbcab4` through `1fc190a` on 2026-07-16. This
final ledger update records the confirmed T2 remote checkpoint.

### LSH-T3 — Complete bounded egress scanning

Status: **PASS** (remote checkpoint confirmed).

The prior scanner traversed only JSON string values. Tool error text flowed
unchanged to the caller and receipt, object keys were cloned verbatim, and
post-execution recursion had no byte, node, depth, string, or decode-work bound.
Those were the confirmed `LSF-028`/`LSF-029` paths and the reachable
`LSD-010` unbounded-scan case.

The unified egress path now scans successful output, tool error text, and every
free-text receipt/session field before exposure or persistence. Provenance
bindings are sanitized recursively before entering the receipt chain. Sensitive
JSON object keys quarantine the complete result rather than risk key collision
or structural ambiguity. Tool errors are redacted with the same PHI, ITAR, and
secret detectors as successful values, and their redaction kinds contribute to
the receipt evidence.

Scanning now enforces deterministic limits: 1 MiB aggregate key/string bytes,
100,000 JSON nodes, depth 64, and 256 KiB per string. A limit violation returns
JSON null plus a stable non-sensitive quarantine error and receipt label.
Whole-string standard-base64 and hex representations up to 128 KiB are decoded
within the same bounded path; decoded PHI, ITAR, or secret findings redact the
encoded source as one span. Non-UTF-8 decoded data remains opaque and cannot
expand traversal work.

Changed files:

- `Cargo.lock`;
- `crates/aog-toolproxy/Cargo.toml`;
- `crates/aog-toolproxy/src/lib.rs`;
- `crates/aog-toolproxy/src/scan.rs`; and
- this DEVLOG.

Gates:

- `cargo test -p aog-toolproxy
  tests::reg_lsf_028_tool_error_and_receipt_metadata_are_scanned -- --exact` —
  PASS, 1/1;
- `cargo test -p aog-toolproxy
  scan::tests::reg_lsf_029_sensitive_object_key_quarantines_the_result --
  --exact` — PASS, 1/1;
- `cargo test -p aog-toolproxy
  scan::tests::reg_lsd_010_depth_node_and_byte_limits_quarantine_deterministically
  -- --exact` — PASS, 1/1;
- base64/hex, nested value, PHI, ITAR, AWS, GitHub, OpenAI, Slack, PEM, benign,
  scalar, and multi-finding scanner fixtures — PASS;
- `cargo test -p aog-toolproxy -p aog-approvals` — PASS: 59 toolproxy tests,
  five approval tests, and doc tests;
- `cargo test -p aog-conformance --test robustness_conformance` — PASS, 11/11;
- `cargo test -p aog-controller --test managed_toolproxy` — PASS in the current
  prerequisite-free lane;
- `cargo clippy -p aog-toolproxy -p aog-approvals -p aog-conformance -p
  aog-controller --all-targets -- -D warnings -A clippy::pedantic` — PASS;
- `cargo fmt --check` — PASS; and
- `git diff --check` — PASS.

Closure statement: `LSF-028` and `LSF-029` are closed across model-facing and
receipt surfaces. The post-execution scanner branch of `LSD-010` is closed with
deterministic quarantine limits; credential-cancellation behavior remains owned
by LSH-T4/D4. LSH-T4 is the next sequential prompt. The broader M3 milestone
remains open through LSH-T6.

Commit state: implementation and prompt evidence were committed as
`39c34384bf3b94bce4c7e64634aa55d78b3a61c4`. The commit is SSH-signed and
carries the canonical `Authored and reviewed by Basho Parks, copyright 2026`
footer. The exact-SHA DEVLOG closeout was committed as
`e251a9094169c17b2c1b51f02eea69bc5a99c0e7`; it is also SSH-signed and carries
the canonical footer. Both outgoing commits passed exact-footer and signature
verification; the pre-push full-tree no-slop and 79-route policy gates passed;
and `origin/main` advanced from `ade0178` through `e251a90` on 2026-07-16. This
final ledger update records the confirmed T3 remote checkpoint.

### LSH-T4 — Cancellation-safe credentials

Status: **PASS** (remote checkpoint confirmed).

The prior `CredentialMinter` returned a caller-defined duration and exposed only
an asynchronous best-effort revoke operation. `ToolProxy::invoke` minted before
the executor await and revoked only on the normal continuation, so cancellation,
panic, task loss, or shutdown could skip cleanup. No production
`CredentialMinter` implementation or caller exists in the repository; the only
implementation was the toolproxy unit-test fixture. That confirms the deferred
`ATPROXY-CRED-CANCEL-REVOKE` path while keeping live production reachability for
LSH-T6/D4 rather than inventing a deployment claim.

The minter contract now receives an authority TTL that must be enforced by the
external minting authority and must return that authority's absolute expiry.
The proxy requests the smaller of the tool timeout and a public 60-second hard
ceiling, rejects expired or overlong authority responses before execution, and
uses the accepted remaining lifetime as the executor timeout and receipt TTL.

Every accepted credential is owned by a `CredentialLease` scope guard. Its
synchronous revocation seam is suitable for a local durable queue and runs both
on normal completion and when Rust drops the invocation future during
cancellation or panic. The guard initiates revocation exactly once. If the
process disappears before receiving the lease id, or a revocation worker/network
is partitioned, the authority-side expiry remains the non-optional hard bound.

Changed files:

- `crates/aog-toolproxy/src/lib.rs`; and
- this DEVLOG.

Gates:

- `cargo test -p aog-toolproxy
  tests::reg_lsd_010_cancellation_safe_authority_bounded_credentials -- --exact`
  — PASS, 1/1 deterministic lifecycle regression covering cancellation,
  executor-task loss, panic, timeout, revocation partition, and loss after
  authority mint but before the response returns;
- `cargo test -p aog-toolproxy
  tests::authority_expiry_outside_the_requested_bound_is_rejected -- --exact`
  — PASS, 1/1 fail-closed authority-bound regression;
- `cargo test -p aog-toolproxy -p aog-approvals` — PASS: 61 toolproxy tests,
  five approval tests, and doc tests;
- `cargo test -p aog-conformance --test robustness_conformance` — PASS, 11/11;
- `cargo test -p aog-controller --test managed_toolproxy` — PASS in the current
  prerequisite-free lane;
- `cargo clippy -p aog-toolproxy -p aog-approvals -p aog-conformance -p
  aog-controller --all-targets -- -D warnings -A clippy::pedantic` — PASS;
- source search confirms no production `CredentialMinter` implementation or
  `with_minter` caller exists outside toolproxy tests;
- `cargo fmt --check` — PASS; and
- `git diff --check` — PASS.

Closure statement: the reachable local cancellation-cleanup branch of `LSD-010`
is closed by the authority-bound mint contract and drop-safe revocation handoff.
LSH-T6/D4 still own wiring and live-system validation of the eventual production
minter; this prompt does not claim that absent integration already exists.
LSH-T5 is the next sequential prompt. The broader M3 milestone remains open
through LSH-T6.

Commit state: implementation and prompt evidence were committed as
`14b88d0c2ee34e06ca9cd4fb7d456ea5de2ff43c`. The commit is SSH-signed and
carries the canonical `Authored and reviewed by Basho Parks, copyright 2026`
footer. The exact-SHA DEVLOG closeout was committed as
`16e0c7eb7a29cc881b9e03ce3376fa16b7f44e2a`; it is also SSH-signed and carries
the canonical footer. Both outgoing commits passed exact-footer and signature
verification; the pre-push full-tree no-slop and 79-route policy gates passed;
and `origin/main` advanced from `f7ee236` through `16e0c7e` on 2026-07-16. This
final ledger update records the confirmed T4 remote checkpoint.

### CI recovery checkpoint — GitHub Actions failure window

Status: **PASS** (remote checkpoint confirmed).

This bounded recovery interrupted, but did not advance, the sequential roster
after the GitHub workflows remained red across the preceding commit window. The
failure set crossed four integration boundaries rather than one prompt:

- the WSF live bridge minted an exactly 900-second token, then reached AWS STS
  after enough elapsed time to fall below STS's 900-second minimum;
- the development Loom estate attempted authenticated Raft forwarding that only
  exists in the production mTLS topology, and V5 discarded rejected scale-write
  responses;
- in-process conformance setup assumed the bootstrap handle remained leader,
  then accumulated concurrent Raft estates without stopping their cores on the
  Windows aggregate runner; and
- the live revocation test held a write guard across an await, which the exact CI
  clippy command rejected.

The recovery gives WSF exchanges explicit authority headroom, directs
development-estate writes to the reported leader while preserving strict
production forwarding, makes membership setup leader-transparent, checks every
V5 scale write, releases the revocation lock before I/O, serializes real-Raft
conformance gates within their test binary, and stops each estate after its bar.
The first shared-owner stop seam briefly changed the consuming `shutdown`
contract and kept a redb store lock alive; both Linux MAI CI and Windows Lamprey
exposed that regression. The final correction restores consuming shutdown for
store release and keeps a separate borrowed stop seam for Arc-owned test estates.

Changed files across the recovery commits:

- `crates/wsf-api/tests/live_api.rs`;
- `crates/wsf-api/tests/live_revocation.rs`;
- `crates/aog-conformance/src/bars.rs`;
- `crates/aog-store/src/raft/mod.rs`;
- `deployment/loom-harness/gates/v5-kill-switch-under-scale.sh`;
- `deployment/loom-harness/gates/v7-chaos-soak.sh`;
- `deployment/loom-harness/gates/v8-scale.sh`; and
- `deployment/loom-harness/gates/v10-revocation-slo.sh`.

Local gates:

- `cargo test -p aog-conformance --lib` — PASS in normal parallel mode: three
  passed and five aggressive/SLO/nightly gates intentionally ignored; the final
  run completed in 51.45 seconds;
- `cargo test -p aog-apiserver --test receipt
  pending_intent_survives_crash_and_recovers_as_verifiable_evidence -- --exact`
  — PASS, 1/1;
- `cargo test -p aog-store` — PASS, eight integration tests plus crate/doc tests;
- affected-crate clippy with `-D warnings -A clippy::pedantic`, `cargo fmt
  --check`, shell syntax checks, and `git diff --check` — PASS;
- live OpenBao + Moto integration suite — PASS, all 15 trust-adjacent tests,
  including the WSF AWS STS exchange; and
- containerized 5-control-plane + 5-edge Loom estate — PASS: V5, V8, V10, V4,
  and V7.

The local full-workspace check remained environment-blocked because this Windows
host has no `protoc` on `PATH`; the GitHub Rust gate installed the protobuf
compiler and passed compilation, clippy, formatting, and `cargo test --workspace`
on the final SHA.

Commit and remote evidence:

- integration-boundary repair:
  `4bfe8c9adcd3024749d547b198fb10ce5466c082`;
- conformance lifecycle repair:
  `a4e7337df1816cd2cd6bc03889e20c6d289c204c`;
- consuming-shutdown correction and final code checkpoint:
  `3a0684b3f50f1c2f77f0a85ca0951af34b94beb4`;
- MAI CI run
  `29552235789` — PASS:
  <https://github.com/USS-Parks/Mighty-Eel-OS/actions/runs/29552235789>;
- Lamprey MAI Validation run
  `29552235838` — PASS:
  <https://github.com/USS-Parks/Mighty-Eel-OS/actions/runs/29552235838>;
- SHIP Validation `29552235791`, commit-message validation `29552235790`, and
  Pages `29552235462` — PASS.

All three recovery commits are SSH-signed and carry the canonical `Authored and
reviewed by Basho Parks, copyright 2026` footer. The pre-commit no-slop checks and
pre-push full-tree no-slop plus 79-route policy gates passed for every published
range. This recovery checkpoint closes the observed GitHub failure window;
LSH-T5 remains the next sequential PSPR prompt.

### Repository identity migration — `USS-Parks/Mighty-Eel-OS`

Status: **PASS** (canonical GitHub identity verified; publication carried by
this bounded governance checkpoint).

On 2026-07-16, the GitHub repository was renamed from
`USS-Parks/im-mighty-eel-mai` to `USS-Parks/Mighty-Eel-OS`. The active repository
identity was updated across the PSPR and DEVLOG front doors, operator and tester
instructions, package metadata, protobuf Go package metadata, supply-chain
signing policy, workflow links, and the local Git `origin` URL.

Historical scan filenames, release reports, tester evidence, baseline metadata,
and retired local clone paths retain the former name because changing them would
rewrite immutable evidence or break references to archived artifacts. Closed
governance plans and rosters that still cite those artifacts now carry an
explicit repository-identity annotation.

Verification:

- authenticated GitHub metadata resolved `USS-Parks/Mighty-Eel-OS`, with `main`
  as the default branch;
- `origin` fetch and push URLs resolve to
  `https://github.com/USS-Parks/Mighty-Eel-OS.git`;
- live identity surfaces contain the new repository name; and
- the retained former-name references are confined to annotated history and
  immutable evidence.

### LSH-T5 — Authenticated approval decisions

Status: **PASS** (reconciled implementation and remote checkpoint `acd755a88c95960c53374f271e2c7616b03f2376`).

The prior inbox accepted a free-form actor string, keyed pending work directly
by caller-supplied call id, silently replaced an existing pending request on id
collision, had no expiry or nonce, and carried neither tenant nor immutable
argument binding. The toolproxy trusted the returned actor and wrote only that
string to its receipt. Actor spoofing, replay, argument substitution,
cross-tenant approval, and duplicate-id replacement were therefore not
structurally excluded.

Approval submission now accepts untrusted request material but derives the
canonical arguments digest, monotonic server sequence, nonce, approval id,
request time, and absolute expiry inside the inbox. Duplicate call ids receive
independent nonce-derived approval ids and cannot overwrite one another.
Tickets time out against the absolute expiry, atomically remove the exact
nonce-bound pending item, and append a fail-closed expiry decision.

Approve and deny operations now require a `VerifiedRequestContext` established
for the dedicated `AogToolApprove` operation on the exact `approval/<id>` resource. The authenticated
principal must carry `aog:approve` and match the pending tenant. A successful
decision yields one `ApprovalGrant` binding approval id, authenticated actor,
role, tenant, call id, canonical arguments digest, nonce, and expiry. Consuming
the pending entry makes replay return unknown rather than re-authorize.

The toolproxy independently revalidates every returned grant immediately before
execution. Wrong role, tenant, call id, arguments digest, missing binding, bad
expiry, or expired decision fails closed without entering the executor. The
complete sanitized grant is included in the tool receipt chain, while the
approval inbox decision chain carries the same binding metadata.

Changed files:

- `Cargo.lock`;
- `crates/aog-approvals/Cargo.toml`;
- `crates/aog-approvals/src/lib.rs`;
- `crates/aog-conformance/tests/robustness_conformance.rs`;
- `crates/aog-toolproxy/src/lib.rs`;
- `crates/aog-toolproxy/src/receipt.rs`; and
- this DEVLOG.

Gates:

- `cargo test -p aog-approvals
  tests::reg_lsh_t5_authenticated_single_use_approval_decisions -- --exact` —
  PASS, 1/1 authenticated actor, cross-tenant, duplicate-id, replay, nonce, and
  expiry regression;
- `cargo test -p aog-toolproxy
  tests::reg_lsh_t5_invalid_approval_bindings_fail_closed -- --exact` — PASS,
  1/1 role/tenant/call/arguments/expiry matrix with zero executor entries;
- `cargo test -p aog-toolproxy -p aog-approvals` — PASS: 62 toolproxy tests,
  six approval tests, and doc tests;
- `cargo test -p aog-conformance --test robustness_conformance` — PASS, 11/11;
- `cargo test -p aog-controller --test managed_toolproxy` — PASS in the current
  prerequisite-free lane;
- `cargo clippy -p aog-toolproxy -p aog-approvals -p aog-conformance -p
  aog-controller --all-targets -- -D warnings -A clippy::pedantic` — PASS;
- `cargo fmt --check` — PASS; and
- `git diff --check` — PASS.

Closure statement: T5's approval boundary authenticates the decider and makes
every positive decision exact-call, exact-arguments, tenant-bound, expiring,
nonce-bearing, and single-use in both receipt chains. This section preserves
the earlier local T5 evidence while rebasing its implementation onto the
published CI-recovery and repository-identity checkpoint. LSH-T6 remains
responsible for the real production caller/executor/credential/approval live
gate. LSH-T6 is the next sequential prompt and the final prompt in M3.

Commit state: the reconciled implementation and prompt evidence were committed
as `acd755a88c95960c53374f271e2c7616b03f2376`. The commit is SSH-signed with
fingerprint `SHA256:PE4Wpbp27IeZC6y4dd97YDNLiFrDvky2KOWSqvdkTEc`, carries the
canonical footer, passed the full-tree no-slop and 79-route pre-push gates, and
advanced `origin/main` from `df119fb6321e60e8cfffc1b36281ba95f9f5004a` to
`acd755a88c95960c53374f271e2c7616b03f2376` on 2026-07-17. LSH-T6 is active.

### LSH-T6 — Tool governance live gate

Status: **PASS** (published source checkpoint; M3 complete).

The repository previously exposed `ToolProxy`, `CredentialMinter`,
`ApprovalInbox`, and the focused WSF OpenBao client only as independent library
seams. No production caller composed them, no real executor consumed the
call-scoped credential, and the only credential-minter implementations lived in
tests. Consequently T1–T5's local proofs did not establish that the complete
runtime path preserved their invariants.

The new `aog-tool-runtime` crate is the production composition boundary. It
accepts only a `VerifiedRequestContext` established for the dedicated
`AogToolInvoke` operation and exact canonical `tool/<id>` resource, derives tool
role from authenticated principal claims, requires a verified tenant plus root
token lineage, and rejects caller-selected routes or credential roles.
Side-effecting calls traverse the real `ApprovalInbox`; decisions now use the
separate `AogToolApprove` operation instead of overloading generic AOG update.

Credential authority is configured by exact tenant and tool. Each binding names
an OpenBao token role and explicit child policy set. The WSF OpenBao client
creates a non-renewable token with no default policy, a requested TTL and equal
explicit maximum TTL no longer than 60 seconds, metadata binding tenant/tool/
session, and a redacted debug representation. The runtime passes the token only
as an HTTP Authorization header to an operator-allowlisted executor endpoint;
HTTPS is mandatory except loopback tests, redirects are disabled, and response
bodies are streamed under a byte ceiling.

Revocation is cancellation safe. `CredentialLease::Drop` synchronously writes
an accessor-only record through a temporary file plus `sync_all` and atomic
rename. A serialized worker drains records by logging into OpenBao and revoking
the accessor. Pending records survive process loss and are retried at startup;
authority-enforced expiry remains the upper bound if OpenBao is unavailable.
Both `MintedCredential` and `OpenBaoTokenLease` now redact secrets from `Debug`
and zeroize their token strings on drop. Session ids are restricted to a
bounded metadata-safe alphabet before authority or receipt use.

Live evidence used disposable OpenBao image
`openbao/openbao@sha256:436eaf9778cad75507ff70ea26ace30dcbe15606e619ac3823495663d7f7c115`
on loopback. The test provisioned separate parent and child policies, an
AppRole, and a token role, then exercised benign, injected, approved mutation,
four concurrent, oversized, secret-bearing, and cancelled calls through the
real runtime and a live HTTP tool. Ten distinct tokens reached the tool while
active; all ten accessors were subsequently rejected by OpenBao. Nine completed
calls produced an intact receipt chain; the cancelled call produced no
fabricated completion receipt, while its drop guard durably revoked the lease.
The secret fixture appeared neither in returned model context nor serialized
receipts.

Changed files:

- `Cargo.toml` and `Cargo.lock`;
- `crates/aog-tool-runtime/Cargo.toml`;
- `crates/aog-tool-runtime/src/lib.rs`;
- `crates/aog-tool-runtime/tests/live_tool_governance.rs`;
- `crates/aog-approvals/src/lib.rs`;
- `crates/aog-toolproxy/src/lib.rs`;
- `crates/fabric-contracts/src/principal.rs`;
- `crates/wsf-bridge/src/lib.rs`;
- `crates/wsf-bridge/src/openbao.rs`;
- `docs/verification/LSH-T6-LIVE-TOOL-GATE.md`; and
- this DEVLOG.

Gates:

- `cargo test -p aog-tool-runtime --test live_tool_governance -- --nocapture`
  with live OpenBao — PASS, full benign/injected/mutating/concurrent/cancelled/
  oversized/secret-bearing matrix;
- `cargo test -p aog-tool-runtime -p aog-toolproxy -p aog-approvals -p
  wsf-bridge` with live OpenBao — PASS;
- `cargo test -p aog-conformance --test robustness_conformance` — PASS, 11/11;
- `cargo test -p aog-controller --test managed_toolproxy` — PASS, 1/1;
- focused strict clippy for the runtime, proxy, approvals, bridge, conformance,
  and controller — PASS;
- `cargo check --workspace` using the installed host `protoc` — PASS;
- `cargo clippy --workspace --all-targets -- -D warnings -A
  clippy::pedantic` — PASS;
- `cargo test --workspace` with the live T6 environment — PASS;
- `cargo fmt --all -- --check` and `git diff --check` — PASS;
- `cargo audit` — PASS, zero vulnerabilities across 495 dependencies; and
- `cargo deny check` — PASS (`advisories ok, bans ok, licenses ok, sources ok`;
  configured duplicate/unmatched-license warnings remain non-fatal).

Closure statement: the production caller, approval, credential authority,
executor, egress, revocation, and receipt chain now close `LSF-026` through
`LSF-029` and the currently reachable `LSD-009/010` paths end to end. M3's
gateway/tool acceptance and full workspace gates are green. The implementation
was SSH-signed and committed as
`5e541e5324269a051d3304e94ae868080d876a25`, carries the canonical footer, and
advanced `origin/main` from `4b5335e8d989b2b792f9d1dcb7e2ea53000844d9` to
`5e541e5324269a051d3304e94ae868080d876a25` on 2026-07-17 after the full-tree
no-slop and 79-route policy gates passed. This DEVLOG closeout records the
exact source snapshot eligible for the independent Saddle `SAD-01` import. The
source hardening roster's next deferred prompt remains `LSH-D1`; no additional
source-lane closure is claimed here.
