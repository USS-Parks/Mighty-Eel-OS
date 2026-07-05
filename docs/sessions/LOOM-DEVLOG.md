# LOOM DEVLOG — AOG Orchestration Engine (M3)

Execution log for the **Loom** orchestration engine (M3 Summit addendum,
`PLANNING/AOG-ORCHESTRATION-ENGINE-M3-SUMMIT-ADDENDUM-PSPR.md`), governed by the
`AOG-WSF-ROBUSTNESS-AND-ZERO-TRUST-DOCTRINE.md` (invariants I-1..I-9). One prompt
= one focused commit + one entry (id · files · verify result · SHA). Built in the
`session/LOOM-1` worktree, branched from `origin/main` at `afe1c4c` (the pushed
M1+M2 tip).

Milestones: **M3a** kernel (Phases K + R + X1–X2) → **M3b** attested edge (S + N)
→ **M3c** objects + HA (O + H) → **Summit-Conformance** (V). "Kubernetes-grade,
woven" is a *gated* claim, earned only when the Phase V conformance suite is green.

---

## Phase K — Control-plane kernel (`aog-store`, `aog-apiserver`, `aog-estate`)

### K1 — `aog-estate` resource model — DONE
The typed resource model — Loom's "CRDs". New crate `crates/aog-estate`, a
pure-types layer that **extends `fabric-contracts`** with zero ad-hoc structs:
every trust-bearing field reuses a frozen contract type (`Classification`,
`Budget`, `Caveat`, `Route`, `ComplianceScope`, `RoutingDecision`).

- **Core envelope** (`src/lib.rs`): `Kind` (13 kinds), `TypeMeta` (api-version +
  kind, flattened on the wire), `ObjectMeta` (name/uid/tenant/generation/
  resource_version/labels/annotations + `token_ref` + `receipt_ref` +
  finalizers/deletion-timestamp for R2 GC), and the generic
  `Resource<Spec, Status>`. `EstateKind` binds a spec to its one `Kind` (the
  "no ad-hoc structs" enforcement); `Validate` is fail-closed (doctrine D7).
  A `resource_objects!` macro generates the type-erased `ResourceObject` enum +
  its JSON dispatch (`from_value`/`to_value`/`kind`/`name`/`validate`) — the unit
  `aog-store` will persist and `aog-apiserver` will admit.
- **The 13 kinds** (`src/kinds.rs`, A1.5): `Tenant`, `TrustRing`, `VirtualKey`,
  `Capability`, `PolicyBundle`, `ProviderPool` (+ `ModelEndpoint`), `Workload`
  (kind = gateway|agent|toolproxy|inference), `Placement`, `Node`,
  `MissionContract`, `ToolGrant`, `RolloutPlan`, `RevocationIntent`. Trust
  invariants show up as validation already: a `Capability` with `ttl_seconds == 0`
  is rejected (zero standing privilege, I-1); `Node`/`Workload` carry the
  attestation floor/ceiling that S4 will match; `AttestationProfile.air_gapped`
  is the I-8 gate.
- **Schema versioning:** `API_VERSION = aog.islandmountain.io/v1`; `validate()`
  rejects an unknown api-version and any `type_meta.kind` that disagrees with the
  spec's `EstateKind` — the seam K10 conversion will hook.
- **Files:** `crates/aog-estate/{Cargo.toml, src/lib.rs, src/kinds.rs,
  tests/roundtrip.rs}`; workspace member added.
- **Verify:** `cargo clippy -p aog-estate --all-targets -- -D warnings` clean
  (pedantic); `cargo test -p aog-estate` = **21 passed** (13 kinds round-trip
  through JSON + the erased `ResourceObject` path; 8 schema-reject: bad/empty
  name, bad ring, kind mismatch, unknown api-version, ttl-zero, unknown kind,
  body/kind mismatch); `cargo fmt --check` clean; `cargo check --workspace` clean
  (50 crates, 0 regressions — additive). **Gate:** round-trip + schema-reject
  test for every kind ✓; `fabric-contracts` dep, no ad-hoc structs ✓.
  **Commit:** `LOOM-K1`.

### K2 — `aog-store` state machine — DONE
The deterministic desired-state KV. New crate `crates/aog-store`: keys map to
`Versioned` values (bytes + create/mod revision + version); a monotonic global
revision bumps once per successful mutation. Writes carry a `Precondition`
(compare-and-set): `Any`, `Absent`, or `Revision(n)`. `Op::{Put,Delete}` are the
units the Raft log will carry (K3); `Store::apply`/`apply_all` are a **pure
function of the op sequence** — the same log replays to identical state on any
backend.
- **Engine decision (A4): redb** (2.6.3) — stable, maintained, pure-Rust,
  single-file ACID; sled's 1.0 is perpetually beta. A `Backend` trait keeps the
  revision/CAS state machine engine-independent: `MemBackend` (BTreeMap — tests +
  Raft's in-core state) and `RedbBackend` (durable; `Versioned` as JSON in one
  table). Global revision recovers on open as `max(mod_revision)`.
- **Files:** `crates/aog-store/{Cargo.toml, src/lib.rs, src/redb_backend.rs,
  tests/store.rs}`; workspace member added; `redb` added to the lock.
- **Verify:** clippy `--all-targets -D warnings` clean; `cargo test -p aog-store`
  = **3 passed** (deterministic apply from a fixed op log → identical results,
  revision, and state across two stores; CAS rejects a stale revision, an
  `Absent` clash, and a missing-key delete; redb persists the value and recovers
  the revision across reopen); fmt clean; `check --workspace` clean. **Gate:**
  CAS rejects stale writes ✓; deterministic apply from a fixed op log ✓.
  **Commit:** `LOOM-K2`.

### K3 — `aog-store` Raft (openraft) — DONE
Wrapped the K2 store in **openraft 0.9.24**. A4 consensus decision: **openraft**
over raft-rs — async-native fits the tokio/axum estate, and the Raft protocol
itself is not hand-rolled (A4: "getting this wrong is expensive"). Single-node
bootstrap now; multi-node election/replication is H1.
- **`features = ["serde", "storage-v2"]`** — the v2 storage traits
  (`RaftLogStorage`/`RaftStateMachine`) are sealed without `storage-v2`.
- **`raft/types.rs`** — `TypeConfig` (D=`RaftRequest{op}`, R=`RaftResponse`,
  NodeId=u64, Node=BasicNode). An application-level CAS rejection is a
  `RaftResponse::Rejected` **value**, never a `StorageError` (which would fault
  the node) — fail-closed at the store, consensus still commits (D7).
- **`raft/log_store.rs`** — `RedbLogStore`: `RaftLogStorage` + `RaftLogReader`
  over redb; durable entries (index→JSON) + persisted `vote`/`committed`/
  `last_purged`; `append` flushes, then signals `LogFlushed`.
- **`raft/state_machine.rs`** — `RedbStateMachine`: `RaftStateMachine` +
  `RaftSnapshotBuilder`. The applied KV **is** the K2 `Store<RedbBackend>`;
  `last_applied`/membership persisted alongside so `applied_state` recovers on
  restart. State behind `Arc<RwLock>` so the committed KV is readable outside
  openraft (which owns the machine). `Store::restore` added for snapshot install.
- **`raft/network.rs`** — single-node no-peer stub; H1 replaces it with a real
  sender-constrained transport (I-3).
- **`raft/mod.rs`** — `RaftNode`: `bootstrap` (init single voter + wait for
  leader), `start` (recover only), `write` (linearizable `client_write`), `get`,
  `revision`, `shutdown`.
- **Files:** `crates/aog-store/src/raft/{mod,types,network,log_store,
  state_machine}.rs`; `tests/raft.rs`; `Store::restore` in `lib.rs`; `openraft` +
  `tokio` deps + lock.
- **Verify:** clippy `--all-targets -D warnings` clean (one scoped
  `result_large_err` allow — openraft's 224-byte `StorageError` is forced by its
  API); `cargo test -p aog-store` = **5 passed** (K2 ×3 + K3: a linearizable
  `client_write` commits+applies, a failed precondition returns `Rejected`;
  committed state + revision survive a full node restart from the durable
  stores); fmt clean; `check --workspace` clean (75 crates). **Gate:**
  linearizable-write test ✓; leader restart preserves committed state ✓.
  **Commit:** `LOOM-K3`.

### K4 — Watch / informer — DONE
The controller read path. The state machine now fans out a change-event stream as
it applies mutations; an `Informer` keeps a prefix-scoped cache current from it
and **re-lists authoritative state on lag or reconnect** — so it can never miss a
final state (the K4 gate).
- **`raft/watch.rs`** — `WatchEvent{revision,key,kind}` + `EventKind{Put,Delete}`;
  `Informer` (local cache + a `broadcast::Receiver`): `resync` (re-subscribe then
  re-list from the store — authoritative), `poll` (drain events; on `Lagged` →
  `resync`), `snapshot`/`revision`. Correctness is resync, not buffering.
- **`raft/state_machine.rs`** — a `broadcast::Sender<WatchEvent>` (buffer 64,
  small on purpose); `apply` publishes a Put/Delete event per successful mutation
  (a rejected CAS emits none); added `subscribe()` + `range()`.
- **`raft/mod.rs`** — `RaftNode::informer(prefix)` + `range(prefix)`.
- **Files:** `crates/aog-store/src/raft/{watch.rs, state_machine.rs, mod.rs}`;
  `tests/watch.rs`. No new dependencies.
- **Verify:** clippy `--all-targets -D warnings` clean; `cargo test -p aog-store`
  = **7 passed** (K2×3 + K3×2 + K4: informer tracks writes/updates/deletes and
  ignores out-of-prefix keys; after flooding 100 writes past the 64-event buffer,
  `poll` detects `Lagged`, re-lists, and reconstructs all 105 keys ==
  authoritative); fmt clean; `check --workspace` clean; deny ok. **Gate:**
  informer reconstructs full state after a dropped connection ✓; no missed final
  state ✓. **Commit:** `LOOM-K4`.

### K5 — `aog-apiserver` CRUD surface — DONE
The typed control-plane API and the **admission choke point**. New crate
`crates/aog-apiserver`: an axum 0.8 router exposing CRUD per estate kind where
**every mutation is forced through one admission method**, and no handler can
reach a store write any other way — the K5 gate, enforced by type.
- **The type invariant.** `Admission` privately owns the sole writable `RaftNode`
  handle in the crate; `Admission::admit` is the only method that calls
  `RaftNode::write`. A handler receives `AppState { admission, reader }` — an
  `Arc<Admission>` (write path = the chain) and a read-only `StoreReader`
  (`get`/`list` only, no write method). The raw node is reachable from neither,
  so a handler physically cannot construct a bypassing write.
- **The chain (A1.7), staged to the roster.** `admit` runs authenticate → validate
  → mutate → commit → receipt. Live now: structural `validate()` (fail-closed,
  D7), metadata stamping (uid / generation / created_at), and the one
  CAS-guarded `aog-store` commit (Create = `Absent`; Update/Delete =
  `Revision(current)` read-modify-write, so a concurrent write loses the CAS →
  `409`). Named seams, each a marked method: authenticate (K6 front-door WSF
  token), policy deny-wins (K7), envelope-seal + child-token attenuation (K8),
  `fabric-proof` receipt to `wsf-ledger` (K9). `resource_version` is the store's
  `mod_revision`, overlaid on read (etcd/K8s convention) — never authoritative in
  the stored body.
- **Surface.** `POST/GET/PUT/DELETE /apis/aog.islandmountain.io/v1/{kind}[/{name}]`
  + `GET /healthz|/readyz`; `ApiError` → HTTP (400/404/409/422/500, plus the
  401/403 K6/K7 seams). URL `{kind}` → `Kind` via the estate deserializer (no
  drift; `aogctl` K11 reuses it). `aog-estate` gained
  `ResourceObject::metadata`/`metadata_mut` (additive; K1's 21 tests still green)
  so admission can stamp any kind.
- **Files:** `crates/aog-apiserver/{Cargo.toml, src/{lib,error,codec,reader,
  admission,handlers}.rs, tests/{crud,admission_bypass}.rs}`; `ResourceObject`
  accessors in `crates/aog-estate/src/lib.rs`; workspace member added.
- **Verify:** clippy `--all-targets -D warnings` clean (pedantic); `cargo test -p
  aog-apiserver` = **7 passed** — CRUD round-trip
  (create→get→list→update→delete: 201/200/200/200/204 then 404), duplicate→409,
  update-missing→404, unknown-kind→400, kind-mismatch→400; and the **gate suite**:
  an admission-rejected request (bad spec / bad name) persists **nothing** (the
  list stays empty), and every admitted object bears the mutate/commit stamps
  (uid, generation=1, resource_version) a bypassing write could not produce.
  `aog-estate` = 21 passed; fmt clean; `check --workspace` clean (additive, 0
  regressions). Driven in-process via `tower::ServiceExt::oneshot` — no socket;
  the router + admission + Raft store are the real ones.
- **Note (A3.2 live-harness scope):** K5 is the CRUD/choke-point plumbing. The
  admission *trust* stages A3.2's live-OpenBao + multi-node clause governs (token
  authN, policy, receipts; kill-switch / split-brain under scale) land at
  K6/K7/K9 and are proven under real partitions at H2/V4/V5. Single-node Raft is
  all K3 built, so a ≥3-node harness is not yet constructible here — this is
  called out, not skipped silently (doctrine D8.9).
- **Gate:** no write reaches `aog-store` bypassing admission — enforced by type
  (private node handle + read-only reader) ✓ and by test
  (reject-persists-nothing + admission-stamps) ✓. **Commit:** `LOOM-K5`.

### K6 — WSF authN at the front door — DONE
Every `/apis/**` request must present a valid, in-budget, unrevoked WSF trust
token, verified **before** admission (the K6 gate: unauth / over-budget / revoked
rejected pre-admission). New `crate::auth`.
- **Local verify, no coasting (doctrine I-3/I-4).** `Authenticator` holds the WSF
  trust-anchor public key + an optional revocation snapshot. A
  `from_fn_with_state` middleware (`require_token`) runs on the API routes only
  (health stays open): it reads `x-wsf-token: base64(json(TrustToken))`, then —
  all local ML-DSA, no OpenBao round-trip — `fabric_token::verify` (signature +
  on-token revocation), `is_expired`, revocation-snapshot membership
  (`fabric_revocation`), and a budget pre-flight. Any failure fails closed
  (401; over-budget → 402). The verified `Principal` (subject, tenant,
  `token_ref`, and the token itself for K8) is stashed in request extensions.
- **Admission takes the verified principal.** `Admission::admit` no longer
  self-authenticates (the K5 stage-1 seam is deleted); it receives the front-door
  `Principal`, and `stamp_create` now stamps the real `token_ref` as provenance.
  `Principal` gained `tenant` + the verified `TrustToken`.
- **Tests refactored + gate.** New `tests/common/mod.rs` mints ML-DSA tokens and
  builds authenticated apps; `crud` / `admission_bypass` now carry a token. New
  `tests/auth.rs` proves the gate: missing / wrong-anchor / expired / revoked →
  401, over-budget → 402, valid → 201, `/healthz` open.
- **Files:** `crates/aog-apiserver/{Cargo.toml, src/{auth.rs (new), lib.rs,
  admission.rs, handlers.rs, error.rs}, tests/{common/mod.rs (new), auth.rs (new),
  crud.rs, admission_bypass.rs}}`; deps `fabric-{contracts,crypto,token,revocation}`
  + `base64`.
- **Verify:** clippy `--all-targets --no-deps -D warnings` clean (own crate;
  `--no-deps` because the new fabric-* dep edge surfaces a **pre-existing**
  `manual_let_else` in `fabric-crypto` under clippy 1.95 — not K6's to fix, and
  green on `main`); `cargo test -p aog-apiserver` = **14 passed** (5 CRUD + 2
  bypass + 7 auth); fmt clean; `check --workspace` clean.
- **Note (A3.2):** verification is local asymmetric crypto by design (I-3), so the
  K6 gate needs no live OpenBao. The live-OpenBao + multi-node kill-switch /
  propagation proof stays owned by R9 / H2 / V5 / V10 (RC-KILL).
- **Gate:** unauth / over-budget / revoked request rejected pre-admission ✓.
  **Commit:** `LOOM-K6`.

### K7 — Admission: validate (deny-wins) — DONE
The admission `validate` stage now runs real policy after structural validation:
a mutation that asserts authority the caller's token lacks is refused with a
specific reason. New `crate::policy::AdmissionPolicy`.
- **Two fail-closed checks (D7).** (1) **Per-kind resource authority** — a
  resource whose declared classification ceiling
  (`Tenant`/`Workload`/`Node`/`Capability`) exceeds the token's
  `max_data_classification` is denied; you cannot govern data above your own
  authority. (2) **Compliance, deny-wins** — for each regime a resource declares
  in `compliance_scopes`, the token must hold that scope; the per-regime verdicts
  are folded by the **mai-compliance `PolicyComposer`** (the same deny-wins engine
  the data-path gateway uses), so control plane and data plane share one
  composition contract. Any deny → `ApiError::Forbidden(reason)` (403).
- **Local, no OpenBao.** Evaluated from the token the front door (K6) already
  verified. `Admission` gained a baseline `AdmissionPolicy` (OCAP > ITAR > HIPAA,
  all enabled); `validate` is now `&self` + principal and calls it.
- **Files:** `crates/aog-apiserver/{Cargo.toml (+mai-compliance), src/{policy.rs
  (new), admission.rs, lib.rs}, tests/policy.rs (new)}`.
- **Verify:** clippy `--all-targets --no-deps -D warnings` clean; `cargo test -p
  aog-apiserver` = **19 passed** (+5 K7: unheld-scope → 403 with reason; deny-wins
  across HIPAA + ITAR when the token holds only one; classification
  over-authority → 403; compliant tenant → 201; a no-facts kind is a policy
  no-op); fmt + `check --workspace` clean.
- **Gate:** a policy-violating mutation denied with a specific reason ✓; deny-wins
  holds across composed rules (an ITAR deny wins over a HIPAA allow) ✓.
  **Commit:** `LOOM-K7`.

### K8 — Admission: mutate + seal + attenuate — DONE
The mutate stage now does two more things after stamping metadata, both needing
control-plane keys. New `crate::seal::Sealer` (a fixed kernel data key + a signer;
production custodies both in OpenBao, Phase W).
- **Envelope-seal flagged spec fields (I-2).** A designated sensitive field
  (`TrustRing.transit_key`, `ToolGrant.credential_ref`) is AES-256-GCM sealed via
  `fabric-envelope`; the plaintext is replaced by a `sealed:wsf-envelope`
  placeholder and the sealed blob stashed in a `wsf.io/sealed.<field>` metadata
  annotation. The control-plane truth store never holds the plaintext (A1.3.8).
- **Attenuate a child token (I-1/I-3).** `finish_mutation` mints a child that
  narrows the caller's token to the action's scope (its classification ceiling)
  via `fabric-token::attenuate` — a strict subset that fails closed on any widen —
  and sets the object's `token_ref` to that child, so the object is authorized by
  a capability scoped to its own creation, not the broad parent.
- **Files:** `crates/aog-apiserver/{Cargo.toml (+fabric-envelope), src/{seal.rs
  (new), admission.rs, lib.rs, policy.rs}, tests/{seal.rs (new), common/mod.rs,
  crud.rs}}`. `AppState::bootstrap/start` take a `Sealer`.
- **Verify:** clippy `--all-targets --no-deps -D warnings` clean; `cargo test -p
  aog-apiserver` = **21 passed** (+2 K8: a TrustRing's `transit_key` is sealed at
  rest — placeholder in the field, ciphertext in the annotation, plaintext never
  surfaced by create or GET; a scoped child is bound to the parent, narrowed on
  classification, budget ≤ parent remaining, and verifies); fmt + `check
  --workspace` clean.
- **Gate:** a sealed field is unreadable in the store (only the placeholder +
  ciphertext appear) ✓; the child token is a strict subset of the parent
  scope/budget ✓. **Commit:** `LOOM-K8`.

### K9 — Admission: receipt binding — DONE
The receipt stage is now real: every admitted mutation emits one hash-chained
receipt to a `wsf-ledger::Ledger` — provable off-host with the public key alone,
physically separate from the intent store (A1.4 / doctrine I-5).
- **One receipt per mutation.** After commit, `receipt` ingests a metadata-only
  receipt — `token_id` (the K8 scoped child), tenant, kind/name/verb,
  `before_digest` / `after_digest` (canonical-JSON `fabric-proof` digests of the
  prior/stored object: create = none→digest, update = digest→digest, delete =
  digest→none), decision `admit`, revision, timestamp — into the ledger's BLAKE3
  chain. A rejected mutation (structural / policy / CAS) never reaches this stage,
  so it writes nothing.
- **Off-host proof.** The ledger signs an `EvidencePack` (ML-DSA-87);
  `wsf_ledger::verify_pack` checks it with the public key alone — no ledger, no
  running system. `Admission`/`AppState` expose `receipts_len`,
  `receipts_public_key`, `export_receipts`. The ledger signer is generated at
  construction (kernel; production custodies it in OpenBao).
- **Files:** `crates/aog-apiserver/{Cargo.toml (+wsf-ledger, +fabric-proof),
  src/{admission.rs, lib.rs}, tests/{receipt.rs (new), common/mod.rs}}`.
- **Verify:** clippy `--all-targets --no-deps -D warnings` clean; `cargo test -p
  aog-apiserver` = **23 passed** (+2 K9: three mutations → three receipts, the
  signed pack verifies off-host and a tampered receipt / wrong key fail; a
  rejected mutation → zero receipts); fmt + `check --workspace` clean.
- **Gate:** mutation ↔ receipt 1:1 ✓; the chain verifies off-host with the public
  key only ✓. **Commit:** `LOOM-K9`.

### K10 — Resource versioning + conversion — DONE
The estate is served at a single **hub** api-version; a stored object at an older
schema version is upgraded to the hub transparently **on read**, so a kind bump
serves old objects with no migration or downtime. New `crate::convert`.
- **Read-path conversion.** `ConversionRegistry` maps `(Kind, from_version)` → a
  single-step converter and holds the hub version. `StoreReader` reads at the
  Value level (`codec::decode_value`, `resource_version` overlaid) and walks the
  converter chain to the hub before serving (bounded against a non-advancing
  cycle); an unknown-but-valid older version is served as stored, never dropped.
  The default is the **identity** registry (hub = the estate `API_VERSION`) —
  every object served exactly as stored, so K5–K9 behavior is unchanged. Writes
  are untouched: admission still validates the estate's current schema.
- **Files:** `crates/aog-apiserver/{src/{convert.rs (new), reader.rs, codec.rs,
  handlers.rs, lib.rs}, tests/{convert.rs (new), common/mod.rs}}`.
  `AppState::with_conversions` sets the registry.
- **Verify:** clippy `--all-targets --no-deps -D warnings` clean; `cargo test -p
  aog-apiserver` = **25 passed** (+2 K10: a stored v1 `PolicyBundle` is served as
  v2 on GET and in LIST — api-version bumped, a new field defaulted, the original
  field preserved; the identity registry serves the stored v1 unchanged); fmt +
  `check --workspace` clean.
- **Gate:** a v1→v2 kind converts transparently on read ✓; old (v1) objects still
  served ✓. **Commit:** `LOOM-K10`.

### K11 — `aogctl` (kernel subset) — DONE
The control-plane CLI. New crate `crates/aogctl`: a `Client` library over the
apiserver's typed CRUD surface + a thin binary, both presenting a WSF token in
`x-wsf-token` (the K6 front door) so the CLI earns each action like any caller.
- **Client (lib).** `apply` (create, or replace on a create-time 409), `get`,
  `list`, `delete` over `reqwest`. A non-2xx response becomes a
  `ClientError::Status { status, message }` — a refusal (401/402/403/409) is
  surfaced to the operator, never swallowed.
- **Binary.** `aogctl apply -f FILE | get KIND [NAME] | describe KIND NAME |
  delete KIND NAME`; server + token from `AOGCTL_SERVER`/`AOGCTL_TOKEN`;
  `--output json` (default: a compact `KIND NAME REV` table). `print_*` is allowed
  only in the binary (a CLI writes stdout).
- **Files:** `crates/aogctl/{Cargo.toml, src/{lib.rs, main.rs}, tests/roundtrip.rs}`;
  workspace member added.
- **Verify:** clippy `--all-targets --no-deps -D warnings` clean; `cargo test -p
  aogctl` = **2 passed** against a **live in-process apiserver on an ephemeral
  port** (real HTTP): apply→get round-trips (create, then create→409→replace
  update, list, delete→404); an over-budget apply surfaces a client-visible 402;
  fmt + `check --workspace` clean.
- **Gate:** apply-then-get round-trips a resource ✓; an over-budget apply is
  rejected client-visibly (402 + message) ✓. **Commit:** `LOOM-K11`.

---

**Phase K complete (K1–K11).** M3a's control-plane kernel: a typed estate
(`aog-estate`) over a consensus store (`aog-store`: CAS KV → openraft →
watch/informer), served by `aog-apiserver` — a typed CRUD surface where every
mutation is forced through the admission chain (authenticate → validate → mutate
→ commit → receipt), and driven by `aogctl`. The chain is real end to end:
front-door WSF token verify (K6), deny-wins compliance policy (K7), envelope-seal
+ child-token attenuation (K8), off-host-verifiable hash-chained receipts (K9);
reads convert stored objects to the hub version (K10). Next: Phase R
(reconciliation runtime + controllers), then the M3a wrap (X1–X2).

---

## Phase R — Reconciliation runtime + controllers

### R1 — Controller runtime — DONE
New crate `crates/aog-controller`: the level-triggered reconcile framework the
Phase-R controllers run on. Read path = the K4 informer; write path = never the
store — controllers mutate desired state only through the apiserver admission
chain, like any other caller (A1.7, I-3/I-5).
- **`queue.rs` — the workqueue.** Dedup (an already-queued key coalesces), dirty
  re-add (a key changed *mid-reconcile* is re-queued on `done` — no lost update),
  per-key exponential backoff `base·2^(n-1)` capped at `max` (`retry`/`forget`),
  and delayed requeue (`requeue_after`/`drain_ready`). Time is always passed in
  (`now: Instant`) — the queue never reads a clock, so tests are deterministic.
- **`runtime.rs` — the loop.** `Reconciler` (async, key-only: observe current
  state and converge; must be idempotent), `Action::{Done, Requeue,
  RequeueAfter}`, `Controller::sync` = one pass: poll the informer (first pass
  re-lists; a lagged watch re-lists inside K4), diff revisions against
  last-observed, enqueue changed/deleted keys, then — leaders only — drain due
  retries and reconcile up to a per-pass budget. `LeaderGate` ("singleton
  controllers"): `AlwaysLeader` for the single-node kernel, `SharedGate` for
  H1's Raft wiring; a non-leader observes (cache warm, queue accumulating) but
  never acts — on takeover the queue is exactly the reconcile-everything a new
  leader owes. `run` loops sync on an interval until a shutdown watch flips.
- **Files:** `crates/aog-controller/{Cargo.toml, src/{lib.rs, queue.rs,
  runtime.rs}, tests/replay.rs}`; workspace member added.
- **Verify:** clippy `--all-targets --no-deps -D warnings` clean; `cargo test -p
  aog-controller` = **10 passed** (6 queue unit tests + 4 replay integration:
  the R1 gate proper — three controllers fed the same history clean, duplicated
  (every key force-enqueued 3×), and dropped (83 writes overflow the 64-slot
  watch buffer, so recovery runs the production lag re-list, not a simulation) —
  record byte-identical end states equal to the store's authoritative state,
  deletes included; failed reconciles retry with backoff and converge; a
  non-leader observes but never acts, then converges on takeover; Requeue /
  RequeueAfter run the key again); fmt + `check --workspace` clean.
- **Gate:** duplicate/dropped events converge identically (replay test) ✓.
  **Commit:** `LOOM-R1`.

### R2 — Finalizers + GC — DONE (two commits: R2a apiserver, R2b controllers)
Deprovisioning as governed teardown: nothing is dropped on the floor, and a
deleted tenant's tokens die at the front door.
- **R2a — two-phase delete (apiserver).** DELETE on an object with finalizers
  stamps `deletion_timestamp` and keeps it (soft delete, 200 + terminating
  object); removing the last finalizer via update commits the promised hard
  delete. While terminating: **spec frozen**, **finalizers only shrink**, the
  deletion timestamp is **carried forward** (no resurrection). A repeat delete
  is an admitted no-op — no mutation, no receipt (K9 stays 1:1). Updates
  asserting a stale `resource_version` are refused (optimistic concurrency for
  controllers). New `aog-estate` `OwnerRef` + `metadata.owner_refs` (frozen
  after create — ownership cannot be hijacked); `Kind::ALL` for estate sweeps.
  The authenticator gains the **estate-driven `RevocationView` kill leg**,
  consulted on every request, fail-closed on lock poisoning. `AppState` exposes
  `admission()/reader()/authenticator()/informer()` so in-process controllers
  wire up with no writable store handle.
- **R2b — the controllers** (on the R1 runtime, all writes through admission as
  the system principal — validated, sealed, CAS-guarded, receipted):
  `EstateClient` (typed reads + admitted writes; already-exists / not-found =
  convergence); `GarbageCollector` (whole-estate informer: cascade — a
  terminating/gone owner's dependents swept by owner-ref and, for tenants, by
  `metadata.tenant` scope; orphans — missing/terminating/uid-mismatched owner —
  collected); `TenantTeardown` (guards live tenants with
  `loom.aog/tenant-teardown`; on terminate: declares a `RevocationIntent`
  targeting the tenant — the kill record deliberately unscoped so it survives
  its tenant — waits for the sweep, releases the finalizer);
  `RevocationIndexer` (rebuilds the front door's `RevocationView` from the
  full intent list — a pure function of desired state — then acks each
  enforced intent `Ready`/propagated through admission; Ring targets stay
  honestly `Pending` until R4 wires ring darkness).
- **Files:** `crates/aog-estate/src/lib.rs`; `crates/aog-apiserver/{src/{admission,
  auth, handlers, reader, lib}.rs, tests/finalizers.rs}`; `crates/aog-controller/
  {Cargo.toml, src/{lib, objects, gc, teardown, intents}.rs, tests/gc.rs}`.
- **Verify:** clippy `--all-targets --no-deps -D warnings` clean; **63 passed**
  across aog-{estate, apiserver, controller} (+5 finalizer semantics, +2 R2
  gate: full live in-process teardown — tenant finalizer-guarded → delete →
  intent declared → children swept → finalizer released → tenant gone, its
  token refused at the front door while an unrelated tenant's passes, ≥8
  receipts; orphan/cascade by owner-ref incl. uid-mismatch incarnation check);
  fmt + `check --workspace` clean.
- **Gate:** deleting a Tenant revokes its tokens everywhere (kernel leg; R9
  extends estate-wide) + GCs children ✓; no dangling capability ✓.
  **Commits:** `LOOM-R2a`, `LOOM-R2b`.

### R3 — Tenant controller (live OpenBao) — DONE
A declared `Tenant` becomes a live OpenBao tenant record and stays converged
with it. New `provision.rs`: `TenantProvisioner` reconciles `Tenant` against
`kv/data/tenants/<id>` through the **M1 `wsf-tenants::TenantAdmin`** (reused,
not rebuilt), guarding every tenant with its own `loom.aog/tenant-openbao`
finalizer (composing with R2's teardown finalizer — the estate object cannot
vanish before OpenBao is deprovisioned).
- **Provision:** record missing (a *genuine* 404 only — any other OpenBao
  failure retries with backoff, never "assume missing and overwrite", which
  would silently rotate the key; I-4) → write scopes + classification ceiling +
  a fresh per-tenant subject-HMAC key; status → `Ready` + `openbao_path`
  (written only on change).
- **Rotate:** past the spec's rotation window (0 → 90-day default) the
  subject-HMAC key is re-minted. Driver: new R1 `Controller::with_resync` —
  a periodic full re-enqueue heartbeat for time-based reconciliation (+1 unit
  test: heartbeat re-reconciles with zero store changes).
- **Deprovision:** terminating tenant → record deleted **and** an
  anchor-signed revocation snapshot (revoking every control-plane token id
  enumerable from the tenant's estate objects) persisted to
  `kv/data/revocations/<id>` — the path the Ring-3 caches poll — before the
  finalizer is released. (Tenant-wide front-door kill is the R2 intent leg;
  R9 fans intents estate-wide.)
- **Files:** `crates/aog-controller/{Cargo.toml (+wsf-tenants, +wsf-bridge,
  +chrono, +serde; dev +reqwest, +fabric-revocation), src/{provision.rs (new),
  runtime.rs (with_resync), lib.rs}, tests/{live_tenant.rs (new), replay.rs}}`.
- **Verify:** clippy `--all-targets --no-deps -D warnings` clean; **14 passed**
  with `WSF_OPENBAO_ADDR` set — the R3 live gate ran against a **live OpenBao
  2.5.4** (`openbao/openbao` dev container): provision (record live, scopes
  `["hipaa"]`, ceiling `restricted`, 32-byte hex HMAC key; both finalizers on
  the estate object) → **issue** (real ML-DSA token minted through
  `wsf-bridge::TrustBridge` from the live record) → **rotate** (rotation stamp
  doctored to 2020 via root; heartbeat wake re-mints the key) → **deprovision**
  (record gone; post-deprovision issuance fails; snapshot on the poll path
  verifies off-host against the anchor and revokes the enumerated token ids;
  front door refuses the tenant's token, bystander tenant unaffected); fmt +
  `check --workspace` clean.
- **Gate:** provision→issue→deprovision→revoked-everywhere (live OpenBao) ✓.
  **Commit:** `LOOM-R3`.

### R4 — TrustRing controller — DONE
Rings become per-ring OpenBao Transit keys, and a ring can be **darkened** —
its key disabled so everything sealed under it stops unsealing, and its
workloads halt. New `transit.rs` (`TransitAdmin`: ensure/read/disable a
Transit key over the AppRole login — the key-lifecycle calls the M1 client
never needed; encrypt/decrypt stay on `wsf_bridge::OpenBaoAuth`) + `rings.rs`
(`TrustRingController`, run on a `"TrustRing/"` **and** a
`"RevocationIntent/"` informer):
- **Live ring:** Transit key `loom-ring-<n>` ensured; status
  `Ready`/`key_version`/`dark:false` (written only on change).
- **Dark ring:** a `RevocationIntent` targeting `Ring(n)` → the key is
  disabled (deletion-allowed + delete; idempotent via an existence check —
  Transit answers 400, not 404, to a config write on a deleted key, the one
  live-only defect this gate caught), every ring-`n` `Workload` is marked
  `Failed`/0-ready (estate halt; M3b node runtime enforces eviction), status
  goes `dark`/`Degraded`, and the intent is acknowledged `Ready`/propagated —
  closing the loop R2's indexer honestly left `Pending`.
- **Ring deletion retains the key** (reclaim-policy Retain): sealed data never
  becomes unreadable as a side effect; darkness is a declared, receipted
  intent only.
- **Files:** `crates/aog-controller/{Cargo.toml (reqwest→main, dev +wsf-seal),
  src/{transit.rs (new), rings.rs (new), lib.rs}, tests/live_ring.rs (new)}`.
- **Verify:** clippy `--all-targets --no-deps -D warnings` clean; **15 passed**
  with `WSF_OPENBAO_ADDR` set — the R4 live gate against live OpenBao 2.5.4 +
  live Transit: ring declared → key exists (version 1, status Ready) → an
  envelope sealed through **`wsf-seal`** under the ring key unseals fine →
  one declarative darken-intent → the key is gone from Transit, **the same
  envelope now fails to unseal** (its wrapped data key is undecryptable), the
  ring reports dark/Degraded, the ring's workload is halted (Failed, 0
  ready), and the intent is propagated; apiserver suite 30 passed (no
  regression); fmt + `check --workspace` clean.
- **Gate:** disabling a ring key makes its envelopes unreadable ✓ (live
  crypto, not simulated) + halts ring workloads ✓ (estate leg).
  **Commit:** `LOOM-R4`.

### X1 + R5 — Shared SpendLedger + Capability controller — DONE
The Tier-2 A2 hazard closed: F3's "atomic budget" was per-process — N gateway
replicas could bill a $100 cap ~$N×100. Now the ledger is shared.
- **X1 — `fabric_token::spend` (new module).** `SpendLedger` trait +
  `LocalSpendLedger` (G9a promoted verbatim — the gateway swaps onto it,
  single-process behavior unchanged) + **`LeasedSpendLedger`**: each replica
  leases a bounded slice of the shared budget from a `LeaseStore` (atomic,
  CAS-backed) and decrements **locally** — no per-call shared-store
  round-trip; the store never leases past the cap, so estate-wide spend
  cannot exceed it; worst-case stranding = one slice per replica (**ε =
  replicas × slice**, published). A dry axis is remembered — a fixed cap
  never refills — so post-exhaustion denials are local too (live-caught: the
  first live run made one probe per denied call). Fail-closed: store errors
  deny. Production store: `aog_gateway::spend::OpenBaoLeaseStore` (KV-v2
  `options.cas`; version-conflict → re-read + retry, bounded).
- **R5 — `CapabilityController`.** Capability lifecycle → status `Ready`;
  its budget is metered under the shared key `cap-<name>` by every replica's
  leased ledger. (Token resolution against capabilities is R8.)
- **Drive-by:** 8 pre-existing clippy-1.95 drift lints in aog-gateway
  (untouched since M2) fixed — trivially-copy by-ref, needless pass-by-value,
  float strict-eq, one scoped `#[expect]` on the SSE Result-wrap; plus
  `signed`→`minted` renames in fabric-token's tests (similar_names).
- **Files:** `crates/fabric-token/{Cargo.toml, src/{lib.rs, spend.rs (new)},
  tests/token.rs}`; `crates/aog-gateway/src/{lib.rs, spend.rs (new), http.rs,
  app.rs, policy.rs, recommend.rs, surface_openai.rs, surface_anthropic.rs,
  provider.rs, provider/{openai,anthropic}.rs}`;
  `crates/aog-controller/{Cargo.toml, src/{capability.rs (new), lib.rs},
  tests/live_spend.rs (new)}`.
- **Verify:** clippy `--all-targets --no-deps -D warnings` clean on all three
  crates; **106 passed** across fabric-token / aog-gateway / aog-controller /
  aog-apiserver with `WSF_OPENBAO_ADDR` set — including 4 new spend unit
  tests (3-replica concurrency over a fake pool: ≤ cap, ≥ cap−ε, amortized;
  dry-stays-dry with zero further store contact; unmetered axes free) and the
  **X1/R5 live gate**: 3 `LeasedSpendLedger` replicas race one $100.00
  capability cap through live OpenBao KV CAS ($5.00 slices, 180 × $1.00
  attempts) — total approved ≤ cap, ≥ cap−ε ($15.00), the shared record never
  leased past the cap, and store round-trips ≪ calls; fmt + `check
  --workspace` clean.
- **Gates:** budgets hold across ≥3 replicas under load (live) ✓; over-spend
  ≤ ε ✓ (0 observed — leases are CAS-bounded); no per-call shared-store
  round-trip ✓; single-process behavior unchanged ✓ (gateway suite green);
  concurrent decrement across 3 clients never over-spends a cap (live) ✓.
  **Commit:** `LOOM-X1-R5`.

### R6 — PolicyBundle controller — DONE
A declared `PolicyBundle` becomes a signed artifact on the channel every
gateway/node edge polls, and an edge verifies it with the control-plane public
key **alone** — offline, air-gap-fit. New `bundle_store.rs` (the artifact +
channel) + `bundles.rs` (the controller).
- **Sign + distribute.** `sign_bundle` mints a `SignedBundle` — the bundle's
  `(version, mode, rules)` ML-DSA-signed over its canonical BLAKE3 payload
  (signature field cleared), the exact shape of a `fabric-revocation` snapshot,
  reusing `fabric-crypto`'s `Signer`. `BundleStore` is the channel:
  `OpenBaoBundleStore` publishes to KV-v2 `kv/data/policy-bundles/<name>` (the
  poll path R3 established), `MemBundleStore` is its in-memory double. The
  controller signs and publishes, then records `status.distributed_to` = every
  `Node` + gateway `Workload` the estate declares.
- **Edge verify + anti-rollback (I-3/I-4/I-8).** `verify_bundle` checks the
  signature under the public key alone — wrong key, tampered content, or a
  malformed signature all fail closed. `EdgeBundleCache::accept` is the node's
  decision: a bad signature is refused, and a validly-signed but **stale**
  bundle (version `<=` the applied one) is refused too, so a replayed older
  bundle can never silently downgrade enforcement.
- **Never regress the channel.** The controller is level-triggered and
  idempotent — it re-signs/publishes only on real drift (absent, content
  changed, or a tampered artifact healed) — and refuses to ship a spec whose
  version is behind the live artifact (`Degraded`, not published; I-4). Rollback
  is roll-forward to a new signed version; every prior signed artifact stays
  independently verifiable.
- **Files:** `crates/aog-controller/{Cargo.toml (fabric-crypto/-proof/-contracts
  + hex to deps), src/{bundle_store.rs (new), bundles.rs (new), lib.rs},
  tests/live_bundle.rs (new)}`.
- **Verify:** clippy `--all-targets --no-deps -D warnings` clean +
  `--workspace -D warnings -A clippy::pedantic` (CI) clean; **20 passed** across
  aog-controller with `WSF_OPENBAO_ADDR` set — +3 bundle unit tests
  (sign→verify round-trip incl. wrong-key/tamper; edge accepts forward, refuses
  stale replay + forged signature; mem-store publish/fetch/retract) and the
  **R6 live gate**: a `PolicyBundle` is signed and published to **live** OpenBao
  KV, an edge fetches it and verifies with the public key alone, a v2 update
  reaches the edge, a replayed v1 is refused (`Stale`), a stale spec is
  `Degraded` (channel not downgraded), and a tampered artifact is refused
  (`BadSignature`); hermetic across repeated runs (channel purged on setup).
  fmt + `check --workspace` clean.
- **Gate:** a bundle update reaches all nodes ✓ (`distributed_to` = the estate's
  nodes + gateways; published to the live poll path); signature verifies at the
  edge ✓ (public key alone, off-host); stale bundle rejected ✓ (edge
  anti-rollback + controller no-regress). **Commit:** `LOOM-R6`.

### R7 — ProviderPool / ModelEndpoint controller — DONE
Provider/model health folded into each pool's schedulable set, so the scheduler
(Phase S) only ever places on a reachable endpoint. New `health.rs` (the probe)
+ `providers.rs` (the controller).
- **The probe.** `HealthProbe` answers "is this endpoint reachable now?",
  fail-closed (I-4). `HttpHealthProbe` is the live impl: a provider with a
  configured base URL is liveness-GET'd (any non-2xx or transport error =
  unhealthy); a provider with no URL — a local, air-gapped model with no HTTP
  surface — falls back to the endpoint's declared `healthy` flag. Base URLs are
  deployment config, not signed desired-state, so they live in the probe, not
  the estate.
- **The fold.** `ProviderPoolController` probes every endpoint and writes
  `status.healthy` = the schedulable model set; a pool with endpoints but none
  healthy is `Degraded`, not silently `Ready`. Level-triggered + a resync
  heartbeat (`with_resync`) re-checks health on a cadence, so a provider that
  goes down drops out within that SLO and one that recovers rejoins — without
  any desired-state edit.
- **Files:** `crates/aog-controller/{src/{health.rs (new), providers.rs (new),
  lib.rs}, tests/provider_health.rs (new)}`.
- **Verify:** clippy `--all-targets --no-deps -D warnings` + CI workspace clippy
  clean; **22 passed** across aog-controller — +2 R7 tests, both **real HTTP** (a
  live local server stands in for the provider, no mock): flipping it 503 drops
  both models from the schedulable set on the next resync and marks the pool
  Degraded, recovery re-adds them (declared `healthy: false` throughout, so the
  live probe is proven to govern); a provider with no probe URL uses its declared
  health. fmt + `check --workspace` clean. (No OpenBao: provider health is HTTP
  liveness, outside the A3.2 live-harness clause; the test runs in the normal CI
  lane.)
- **Gate:** an unhealthy provider is removed from the schedulable set within SLO
  ✓ (real 503 → Degraded/empty on the resync heartbeat; recovery rejoins).
  **Commit:** `LOOM-R7`.

### R8 — VirtualKey controller — DONE
A declared `VirtualKey` becomes a resolvable entry at the gateway's
key-resolution path, so the gateway (G1) turns the presented key into a
verified, scoped, in-budget trust token — and a change to the key's capability
is reflected on the gateway's next request, no restart. New `vkeys.rs`.
- **Resolution write.** The gateway resolves a key by reading
  `<prefix>/<sha256(key)>` from OpenBao KV and verifying the `token` there
  against the trust anchor (reused verbatim, not rebuilt). `VirtualKeyController`
  writes that entry: it reads the `Capability` the key names, mints a trust
  token carrying its scope + budget + ttl (`fabric_token::issue`, signed by the
  anchor), and `put`s `{"token": …}` at the key's path. Level-triggered and
  idempotent — it re-mints only on drift (absent, scope changed, or a tampered
  entry that no longer verifies).
- **Fail-closed teardown (I-4).** The controller owns a
  `loom.aog/virtualkey-kv` finalizer, so a deleted key's entry is **retracted
  before** the estate object is collected — a removed key stops resolving, never
  lingers. A key whose capability is missing/terminating is retracted too and
  marked `Degraded`; it never resolves to a stale token. The kernel models the
  presented key by the object's name; a secret-key indirection is Phase-W's.
- **Files:** `crates/aog-controller/{Cargo.toml (fabric-token→deps, +sha2),
  src/{vkeys.rs (new), lib.rs}, tests/live_vkey.rs (new)}`.
- **Verify:** clippy `--all-targets --no-deps -D warnings` + CI workspace clippy
  clean; **23 passed** across aog-controller with `WSF_OPENBAO_ADDR` set —
  including the **R8 live gate** against **live OpenBao and the real
  `aog-gateway`**: a key resolves to cap-basic's scope/budget through
  `Gateway::resolve_and_check`; repointing it at cap-premium (bigger budget, more
  models, higher classification) is reflected by the **same** gateway instance
  with no rebuild/restart; deleting the key retracts its entry and the gateway
  then returns `UnknownKey`; hermetic across runs. fmt + `check --workspace`
  clean.
- **Gate:** a key change is reflected at the gateway without restart ✓ (the real
  gateway resolves the new capability's scope/budget after the edit), plus a
  deleted key stops resolving (fail-closed retraction). **Commit:** `LOOM-R8`.

### R9 — RevocationIntent controller — DONE
The kill leg: a declarative `RevocationIntent` fans out to a signed
`fabric-revocation` snapshot on the channel every gateway replica polls **and**
on removable media for an air-gapped node — bounded, provable revocation
(doctrine I-9), effective on every replica and offline. New `revocation.rs`.
- **Fan-out.** `RevocationController` builds the snapshot as a pure function of
  the current intents (level-triggered: a dropped or duplicated event cannot
  skew it): `Token` targets → `revoked_tokens`, `Subject` → `revoked_subjects`
  (tenant-wide stays the R3 / front-door leg, `Ring` the R4 leg). It signs with
  the anchor (`fabric-revocation`, reused), publishes `{"snapshot": …}` to the
  online KV path the gateway's G9 kill switch reads, and writes the same signed
  artifact to a removable-media file. Idempotent — it republishes only when the
  revoked set drifts or the live snapshot stops verifying — then acks each
  covered intent `propagated`.
- **Complements R2.** R2's indexer is the in-process apiserver front-door kill
  view; R9 is the data-path + air-gap leg — together they close the loop R2's
  indexer honestly left for the snapshot channel.
- **Files:** `crates/aog-controller/{Cargo.toml (fabric-revocation→deps),
  src/{revocation.rs (new), lib.rs}, tests/live_revocation.rs (new)}`.
- **Verify:** clippy `--all-targets --no-deps -D warnings` + CI workspace clippy
  clean; **24 passed** across aog-controller with `WSF_OPENBAO_ADDR` set —
  including the **R9 live gate** against **live OpenBao and the real
  `aog-gateway`**: a virtual key resolves (R8), a `RevocationIntent` for its
  token makes R9 publish the snapshot, and the **same** gateway then denies the
  key (`Revoked`); the media file — verified **offline with the public key
  alone** — reports the token revoked; the intent is acknowledged `propagated`;
  hermetic across runs. fmt + `check --workspace` clean.
- **Gate:** intent → token denied on every replica ✓ (real gateway kill switch)
  + on an air-gapped node via media ✓ (offline-verified snapshot). **Commit:**
  `LOOM-R9`.

---

**Phase R complete (R1–R9).** M3a's reconciliation runtime and its controllers:
the level-triggered runtime (R1) with finalizers / GC / tenant-teardown (R2);
then the live-OpenBao Tenant (R3), TrustRing with declarative ring-darkness (R4),
Capability over the shared lease-based SpendLedger (X1 + R5), PolicyBundle
distribution (R6), ProviderPool health (R7), VirtualKey resolution (R8), and
RevocationIntent kill (R9) controllers. Every trust-adjacent leg is proven
against live OpenBao and, where the gateway is the edge, the real `aog-gateway`.
Next: the M3a wrap — X2 (`aog-gateway` as a managed `Workload`).

---

## Phase X — Migration / cutover

### X2 — Gateway as a managed `Workload` — DONE
`aog-gateway` is now a first-class managed `Workload` in the estate, with **no
change to its data-path API** — an existing OpenAI client is byte-identical
across the cutover to management. New `workloads.rs` + a gateway ledger seam.
- **Managed Workload.** `WorkloadController` reconciles a gateway `Workload`: it
  reflects the `Placement`s bound to it (attested placement stays the Phase-S
  scheduler's — this controller never mints them) and probes liveness through a
  `WorkloadProbe` (`HttpWorkloadProbe` GETs the gateway's `/healthz`;
  `StaticWorkloadProbe` is the M3a default), writing `phase`/`ready_replicas`:
  unplaced → `Pending`, placed + healthy → `Ready` with its replicas, placed +
  unhealthy → `Degraded`. Level-triggered, resync-heartbeat-driven.
- **Ledger seam (no API change).** The gateway's runtime spend ledger is
  promoted to `Arc<dyn SpendLedger>` (default `LocalSpendLedger`, byte-for-byte
  the old behavior) with a `with_spend_ledger` swap — the X1 seam realized on the
  gateway. Honest deferral: the lease-based shared ledger's reserve flow uses a
  distinct `try_spend` API, not `fold`/`add`; adopting it in the request path is
  scale-out work that lands with the node runtime running replicas (M3b), not the
  M3a single-node kernel.
- **Files:** `crates/aog-gateway/src/lib.rs` (spend seam);
  `crates/aog-controller/{src/{workloads.rs (new), lib.rs}, tests/managed_gateway.rs (new)}`.
- **Verify:** clippy `--all-targets --no-deps -D warnings` (both crates) + CI
  workspace clippy clean; **25 passed** across aog-controller + the full
  aog-gateway suite green (on clean OpenBao) — including the **X2 live gate**
  against **live OpenBao and the real gateway OpenAI surface**: a client completes
  a chat; the gateway is declared a `Workload` + bound by a `Placement` +
  reconciled to `Ready` / `ready_replicas=1` by the controller probing its live
  `/healthz`; the **same** client request is byte-identical after — no API change.
  The `Arc<dyn SpendLedger>` change was proven regression-free by a stash test:
  the pre-existing `kill_switch` stale-revocation-snapshot flake (a test-hygiene
  gap, not this change) fails identically with and without it, and the suite is
  green once the stale record is cleared. fmt + `check --workspace` clean.
- **Gate:** an existing OpenAI client is unaffected across the cutover ✓
  (byte-identical response before/after management). **Commit:** `LOOM-X2`.

---

**M3a COMPLETE (Phases K + R + X1–X2).** The Loom kernel: a typed estate over a
consensus store served by the admission-choke-point apiserver (K); the
level-triggered reconciliation runtime and its nine controllers (R1–R9); the
shared lease-based SpendLedger (X1); and `aog-gateway` brought under management
as a `Workload` (X2). Every trust-adjacent path is proven against live OpenBao,
and where the gateway is the edge, the real `aog-gateway` — the model proven end
to end with zero orchestration-scale risk. Next milestone: **M3b — the attested
edge** (Phase S scheduler + Phase N node runtime).

---

## Phase S — Scheduler (`aog-scheduler`, revived from `mai-scheduler`)

_M3b begins. Branch `session/LOOM-3` off `session/LOOM-2` (`23b25ce`)._

### S1 — Framework + defect purge — DONE
New crate `crates/aog-scheduler`: the K8s-style **filter → score → bind**
placement engine (A1.8), revived from `mai-scheduler`'s `Scheduler` /
`PlacementEngine` shape and rebuilt for the AOG workload domain — with the
fake-metrics path deleted rather than inherited (A4).
- **Framework.** Two extension seams. `Filter` is a hard, deny-wins predicate
  (one `Unfit` removes the node); `Scorer` is a soft preference returning
  `Option<f64>` where `None` **excludes** the node — the engine never fabricates
  a missing score (doctrine I-4). `Scheduler` runs every node through the filter
  chain, scores the survivors, and binds the workload to the highest scorer;
  a workload with no surviving, scorable node stays `Pending`, never force-placed
  (A1.8 / the S4 gate). Deterministic: no clock, no RNG; score ties break by node
  name, so an estate always replays to the same decision. Every decision carries
  per-node `SignalProvenance` (resource version, reconciled readiness, heartbeat
  presence, reported allocatable) — the audit trail that ties a placement to real
  inputs. Binds the estate `Placement`/`Node`/`Workload` types directly; no
  parallel structs.
- **Defect purge.** `mai-scheduler`'s metrics are real-feedback-driven, but it
  carries one anti-pattern — **absence-as-optimism**: an instance with zero
  telemetry scores as maximally healthy (`metrics/health.rs`: an empty tracker
  returns `1.0`; `test_empty_tracker_is_healthy`). A defensible cold-start guess
  for inference routing; a custody breach for attested placement, where an
  unmeasured — therefore untrusted — node would look fit. The revival inverts it:
  `NodeSnapshot::from_node` projects a status-less node **fail-closed**
  (`ready == false`, zero `allocatable`, no heartbeat — a generous *spec*
  capacity never leaks in as reported *allocatable*), and `ReadinessFilter`
  rejects any node without reconciled liveness. Absence of a signal can never
  become a favourable one.
- **Scope call (honest).** `mai-scheduler` is a separate, parked crate still
  driving the MAI inference runtime; gutting it is out of Phase-S scope and would
  risk that path. Per A1.11 (`aog-scheduler` is a **new** crate) and A4 (revived
  "by deletion-and-rebuild of the fake paths, not by extending them"), the fake
  path is deleted by not carrying it into the new crate; the anti-pattern is
  documented in the crate docs and **guarded by a source-audit test** so it
  cannot creep back.
- **Live-harness (A3.2) — honest deferral.** S1 is a pure, deterministic decision
  engine over in-memory estate projections — no OpenBao, no consensus, no node
  I/O — so its obligations are fully proven by unit + integration tests. The
  live-multi-node / live-OpenBao gate binds the prompts that actually bind and
  mint against a real estate (S7 runtime-token mint) and the attested-scheduling
  breach proof on a real multi-node estate (V6); those land with the node runtime
  (Phase N).
- **Files:** `crates/aog-scheduler/{Cargo.toml, src/{lib.rs, types.rs,
  framework.rs, filters.rs}, tests/no_fabricated_metrics.rs}` (new); `Cargo.toml`
  (workspace member).
- **Verify:** `cargo fmt --check` (workspace) clean; `clippy -p aog-scheduler
  --all-targets --no-deps -D warnings` clean (workspace pedantic); **14 passed**
  (`cargo test -p aog-scheduler` — 10 unit + 4 gate); `cargo check --workspace`
  clean (358 crates).
- **Gate:** no fabricated metric in any code path (audit + test) ✓ — the
  `source_has_no_fabrication_apis` audit walks `src/` and asserts no RNG /
  synthetic-generator API appears; the fail-closed test proves a status-less node
  is never placed and its spec capacity never leaks in; the `None`-score test
  proves an unscorable node is excluded, not defaulted. Decisions trace to real
  inputs ✓ — `decision_traces_to_real_signals` asserts the winning decision's
  `SignalProvenance` mirrors the exact estate `resource_version` / `ready` /
  heartbeat / `allocatable`. **Commit:** `LOOM-S1`.

### S2 — Capacity + real metrics — DONE
The scheduler now weighs real node capacity. `NodeSnapshot` carries the node's
declared total `capacity` (spec) alongside reported `allocatable` (status), so a
utilisation *fraction* is computable from real signals.
- **CapacityFilter (hard).** A node that declares a workload-slot budget but
  reports none free (`capacity.max_workloads > 0 && allocatable.max_workloads ==
  0`) is saturated and drops out; a node that declares no slot budget is not
  slot-constrained here (the utilisation scorer still weighs its cpu/mem/gpu).
- **UtilizationScorer (soft, normalised).** Score = mean free fraction
  (`allocatable / capacity`) over the dimensions the node declares (cpu, memory,
  gpu, slots), in `[0,1]` so it composes with S5/S6. Fail-closed on absent
  signal: a node declaring no total capacity abstains (`None`) rather than
  inventing a fraction.
- **Framework correction — scorer abstention.** A `Scorer` returning `None` now
  **abstains** (contributes nothing) rather than excluding the node. Excluding a
  safe, placeable node for want of a soft-preference signal is an availability
  bug, not a fail-closed win; hard exclusion stays the filters' job. The
  anti-fabrication guarantee is unchanged — absence becomes a neutral `0`
  contribution, never a favourable value, and a scorer never fabricates.
- **`attested_scheduler()`.** The wiring the control plane (S7) drives:
  readiness + capacity filters + the utilisation scorer. `baseline_scheduler()`
  stays the readiness-only S1 foundation the framework tests pin to.
- **Files:** `crates/aog-scheduler/src/{types.rs, framework.rs, filters.rs,
  scorers.rs (new), lib.rs}`, `tests/attested_placement.rs (new)`.
- **Verify:** fmt + `clippy -p aog-scheduler --all-targets --no-deps -D warnings`
  clean; **22 tests** pass (16 unit + 4 S1 gate + 2 attested-placement).
- **Gate:** placement reflects real load; a saturated node is not selected ✓
  (`saturated_node_is_not_selected`) and the less-loaded of two candidates wins
  (`less_loaded_node_is_preferred`). **Commit:** `LOOM-S2`.

### S3 — Ring filter (hard) — DONE
`RingFilter`: a workload places only within its own trust ring
(`request.ring == node.ring`); a mismatch is `Unfit`, and being a hard filter no
score can rescue it. Rings are the Trust Manifold isolation boundary — crossing
one is a sovereignty violation. Wired into `attested_scheduler()` after
readiness.
- **Files:** `crates/aog-scheduler/src/{filters.rs, lib.rs}`,
  `tests/attested_placement.rs`.
- **Verify:** fmt + clippy `-D warnings` clean; **26 tests** pass.
- **Gate:** cross-ring placement impossible ✓ — a ring-2 workload against a
  ring-1-only estate stays Pending (`cross_ring_placement_is_impossible`); a
  ring-2 node takes it (`same_ring_node_takes_the_workload`). **Commit:**
  `LOOM-S3`.

### S4 — Attestation predicate (hard) — the differentiator — DONE
`AttestationFilter`: a workload is placed only where its data-classification
ceiling is provably held. Two hard conditions:
- **Ordering.** `classification_ceiling <= node.attestation_floor` — the node is
  attested to hold at least as sensitive as the workload's data.
- **Hardware backing.** A sensitive ceiling (`>= Restricted`) additionally
  requires the floor to be rooted in real hardware — an attestation platform
  (TPM / Nitro / SEV-SNP) with a recorded PCR. A node that merely *claims* a
  high floor with no hardware is under-attested; a bare assertion is not
  attestation (I-4). Public / Internal workloads need no hardware root.

Wired into `attested_scheduler()` after the ring filter. `Classification` is the
frozen `fabric-contracts` ordinal (Public < Internal < Restricted < Controlled <
Secret) — no re-declaration.
- **Air-gap compatibility — honest deferral.** A1.8 also lists air-gap
  compatibility as a hard predicate, but a workload's air-gap *requirement*
  derives from its `TrustRing`, not from `WorkloadSpec`, and the scheduler is not
  handed the ring resource. That match lands when the binding controller (S7)
  enriches the request from the ring, and at the node edge (N6, which denies
  cloud routes on an air-gapped node). The node's own `air_gapped` attestation
  flag is already carried on `NodeSnapshot`. Noted, not skipped.
- **Files:** `crates/aog-scheduler/src/{filters.rs, lib.rs}`,
  `tests/attested_placement.rs`.
- **Verify:** fmt + clippy `-D warnings` clean; **34 tests** pass.
- **Gate:** a Ring-3 Secret workload is refused on an under-attested node and
  stays Pending, never force-placed ✓
  (`ring3_secret_refused_on_underattested_node`,
  `never_force_placed_on_least_bad_node`); it is placed on a TPM-attested node
  with a matching floor (`ring3_secret_placed_on_attested_node`). **Commit:**
  `LOOM-S4`.

### S5 — Budget/ROI (consolidation) scorer — DONE
`ConsolidationScorer`: prefer bin-packing onto already-used nodes to reduce the
number of active nodes and the hardware bill — the placement-time, real-signal
half of the budget/ROI objective. Score = mean *used* fraction
(`1 - free_fraction`) over the dimensions a node declares, normalised `[0,1]`.
Shares the `mean_free_fraction` real-telemetry basis with the S2 utilisation
scorer (they are exact complements), so neither invents a value; a node with no
declared capacity abstains.
- **Posture, not default.** Consolidation (pack) is the deliberate opposite of
  utilisation (spread); wiring both at full weight cancels. So
  `attested_scheduler()` keeps the spread posture as the safe default for a
  sovereignty appliance, and `ConsolidationScorer` ships opt-in for
  cost-optimising operators. Posture selection is the controller's (S7).
- **Meter coupling — honest deferral.** Spend-weighted ROI (actual dollars /
  value) is a runtime meter signal the scheduler does not hold at placement time;
  it folds in when the meter feeds per-node efficiency into the estate. The
  scheduler does not depend on `aog-gateway`'s meter.
- **Files:** `crates/aog-scheduler/src/{scorers.rs, lib.rs}`.
- **Verify:** fmt + clippy `-D warnings` clean; **38 tests** pass.
- **Gate:** deterministic score from a fixed telemetry fixture ✓
  (`consolidation_is_deterministic_from_fixture` asserts an exact `0.75` from a
  fixed `Capacity`; `utilization_and_consolidation_are_complementary` proves the
  exact complement). **Commit:** `LOOM-S5`.

### S6 — Spread / HA scorer — DONE
`SpreadScorer`: anti-affinity across nodes for replica resilience. A fresh
replica scores a node `0.0` if it already hosts a sibling replica of this
workload, `1.0` otherwise — so replicas spread across the nodes of their ring.
Replicas share a ring (the ring filter), so anti-affinity is node-wise, not
ring-wise. The scorer reads a new `ScheduleRequest.already_placed_on` (the nodes
already hosting this workload's replicas); `from_workload` starts it empty and
the binding controller enriches it per replica from the estate's `Placement`s
(S7).
- Wired into `attested_scheduler()` alongside utilisation — both are the spread
  posture, so they compose (no cancel).
- **Files:** `crates/aog-scheduler/src/{types.rs, scorers.rs, filters.rs,
  framework.rs, lib.rs}`, `tests/attested_placement.rs`.
- **Verify:** fmt + clippy `-D warnings` clean; **41 tests** pass.
- **Gate:** replicas of one workload spread across ≥2 nodes when available ✓
  (`replicas_spread_across_nodes`: replica 2, told its sibling is on `node-a`,
  lands on `node-b`). **Commit:** `LOOM-S6`.

### S8 — Preemption + priority — DONE (planner; S7 executes it)
_Built before S7: S7's controller was recon-blocked, and the preemption planner
is an independent scheduler-lib unit._ `plan_preemption(incoming_priority,
decision, occupancy)`: when a prior schedule left a workload Pending, a
higher-priority workload may reclaim room by evicting strictly-lower-priority,
disruptible occupants.
- **Only capacity-blocked nodes are targets.** A node is a preemption candidate
  only if it failed *exactly* the capacity filter and passed every other hard
  predicate — so a ring- or attestation-mismatched node is never preempted (no
  hard predicate is violated by preemption).
- **PDB-analog.** A `Victim` carries a `disruptible` flag the controller computes
  from its disruption budget; the planner never evicts a protected victim, nor
  one at equal-or-higher priority. Lowest-priority victim chosen, ties by node
  name — deterministic.
- **Inputs, not estate churn.** Priority and occupancy are explicit planner
  inputs (`incoming_priority`, `NodeOccupancy` / `Victim`), so S8 adds no field to
  `WorkloadSpec`; the controller adapts estate data to them (as with S6's
  `already_placed_on`). Executing the plan (drain victims, then bind) is the
  controller's job (S7); the full disruption budget is Phase O (O7). A workload
  with no lawful preemption stays Pending — pressure never forces a placement.
- **Files:** `crates/aog-scheduler/src/{preemption.rs (new), lib.rs}`.
- **Verify:** fmt + clippy `-D warnings` clean; **46 tests** pass (5 new).
- **Gate:** preemption honors the PodDisruptionBudget-analog ✓
  (`respects_disruption_budget`); no hard-predicate (ring) violation during
  preemption ✓ (`never_targets_a_ring_mismatched_node`); lowest-priority victim
  chosen, equal/higher never evicted. **Commit:** `LOOM-S8`.

### S7 — Binding + runtime-token mint — DONE (live OpenBao)
`SchedulerController` (`aog-controller`, a new dep on `aog-scheduler`): reconciles
each `Workload` on the `"Workload/"` informer and turns its desired replicas into
attested `Placement`s. Per unplaced replica it runs `attested_scheduler()` over
the estate's `Node`s — spreading replicas by passing the nodes already placed as
`already_placed_on` (S6) — mints a runtime `TrustToken` scoped to the workload's
`Capability` (budget/caveats/routes/models/classification/ttl), persists that
token to OpenBao for the node to fetch, and creates the `Placement` through the
admission choke point.
- **Receipt is automatic.** Every admitted mutation emits a `fabric-proof`
  receipt (K9); creating the `Placement` through `EstateClient` (as
  `Principal::system()`) receipts the binding — no separate step. The
  control-plane sibling of the data-path guarantee (I-5).
- **Scope + fail-closed.** The token carries exactly the named capability's
  scope; a missing/terminating capability yields a *minimal* token (less
  privilege, never broader — I-4). A replica with no attestation-satisfying node
  stays Pending and requeues; it is never force-placed. Each `Placement` is owned
  by its workload (owner-ref) so the GC reclaims it on delete (R2/W9).
- **Separation.** This controller *mints* placements; the X2 `WorkloadController`
  reflects them into `Workload` status — the seam X2 left open. One replica per
  node (placement keyed `<workload>-<node>`); multi-replica-per-node is Phase O.
- **Files:** `crates/aog-controller/{Cargo.toml, src/{lib.rs, scheduler.rs
  (new)}, tests/live_scheduler.rs (new)}`.
- **Verify:** fmt + `clippy -p aog-controller --all-targets --no-deps -D warnings`
  clean; `check --workspace` clean; controller suite **26 passed** (non-live) +
  the **S7 live gate green vs live OpenBao** (`loom-r3-openbao`, port 8200): a
  2-replica workload binds across two ready nodes, and the persisted runtime
  token verifies against the anchor and carries the capability's budget (5000),
  model set, and classification.
- **Gate:** a bound workload receives a scoped token; the binding is receipted ✓
  (`scheduler_binds_replicas_with_scoped_tokens`). **Commit:** `LOOM-S7`.

---

## Phase N — Node / edge runtime (`aog-node`)

### N1 — Node agent + registration — DONE
New crate `crates/aog-node`. A node joins with a `fabric-identity` leaf signed by
the trust anchor, plus its declared attestation profile + capacity (the `Node`
spec). `mint_node_identity` issues the leaf (anchor-signed, binding node name +
PKI fingerprint, TTL-bounded); `Registrar::admit` verifies a `NodeRegistration`
against the roster anchor key and refuses — fail-closed — a non-workload
identity, one that names a different node, or one that does not verify. A node
that cannot prove its identity does not join.
- **Files:** `crates/aog-node/{Cargo.toml, src/{lib.rs, registration.rs}}` (new
  crate) + workspace `Cargo.toml` member.
- **Verify:** fmt + clippy `-D warnings` clean; **3 tests** pass.
- **Gate:** a node joins with a verified identity ✓
  (`an_anchor_signed_identity_joins`); a spoofed node is rejected ✓
  (`a_spoofed_identity_is_rejected` — a leaf signed by a non-anchor key fails
  verification; `an_identity_naming_another_node_is_rejected` — subject
  mismatch). **Commit:** `LOOM-N1`.

### N2 — Heartbeat + status — DONE (live OpenBao)
Node liveness, both legs. **Node side** (`aog-node::heartbeat`): `heartbeat`
builds the `NodeStatus` a live node reports (ready, timestamped, advertising free
`allocatable`); `is_stale` flags a beat aged past its freshness window
(fail-closed on a missing/unparseable timestamp). **Control side**
(`aog-controller::NodeController`): a node that reports not-ready, or whose
heartbeat is stale past the TTL, is marked down — so the scheduler stops choosing
it (the readiness filter reads the `ready` bool) — and its `Placement`s are
evicted, so the scheduler re-places those replicas on live nodes.
- **Files:** `crates/aog-node/src/{lib.rs, heartbeat.rs (new)}`;
  `crates/aog-controller/{src/{lib.rs, node.rs (new)}, tests/live_node.rs (new)}`.
- **Verify:** fmt + clippy `-D warnings` clean (both crates); aog-node **7 tests**;
  aog-controller **27 passed** (non-live) + the **N2 live gate green vs live
  OpenBao**.
- **Gate:** a killed node's workload reschedules ✓
  (`a_killed_node_reschedules_its_workload`): a 2-replica workload placed on
  node-a + node-b; node-a's heartbeat goes stale; the node controller marks it
  down and evicts its placement; a fresh scheduler pass re-places the freed
  replica on the idle live node-c → replicas end on {node-b, node-c}. **Commit:**
  `LOOM-N2`.

### N3 — Workload driver trait (CRI-shaped) — DONE
`aog-node::driver`: the pluggable `WorkloadDriver` trait — `start` / `inspect` /
`stop` over `WorkloadRun` → `WorkloadHandle` / `WorkloadState`. Object-safe, so a
node holds a `Box<dyn WorkloadDriver>` and swaps process (N4), containerd (N5),
or wasmtime impls without the rest of the runtime changing. Ships `NoopDriver`
(a bookkeeping driver for shadow mode X4 and tests).
- **Files:** `crates/aog-node/src/{lib.rs, driver.rs (new)}`.
- **Verify:** fmt + clippy `-D warnings` clean; **9 tests** pass.
- **Gate:** the same workload runs via the trait on two driver impls ✓
  (`the_same_workload_runs_via_two_drivers` — `NoopDriver` and a stateless
  `EchoDriver` both start + report Running for the same `WorkloadRun`);
  `noop_driver_reflects_stop` proves lifecycle tracking. **Commit:** `LOOM-N3`.

### N4 — process / systemd driver — DONE
`aog-node::driver::ProcessDriver`: runs a workload replica as a real child
process (`std::process`) — the **air-gap appliance default**, no container
runtime required. On Linux, production wraps it in a systemd unit for boot
supervision + restart-on-failure; the lifecycle it provides (start / inspect /
stop / clean restart) is the same regardless of the service manager on top. A
restart reaps any prior PID for the name (no leak); `stop` kills + reaps.
- **Files:** `crates/aog-node/src/driver.rs`.
- **Verify:** fmt + clippy `-D warnings` clean; **11 tests** pass.
- **Gate:** a gateway replica has a full process lifecycle with a clean restart ✓
  (`a_gateway_replica_has_a_process_lifecycle` spawns a real long-running child,
  reads Running, stops it, then starts + reads Running again under the same
  name). systemd unit management is the Linux packaging of this same lifecycle —
  noted, not required on the appliance path. **Commit:** `LOOM-N4`.

### N5 — containerd driver (optional) — DONE (live via docker)
`aog-node::containerd::ContainerdDriver`: runs a workload replica as a container
through a containerd-compatible CLI (`nerdctl` / `ctr`; also `docker`, which is
containerd-backed), behind the same `WorkloadDriver` trait as the process driver
— so a workload's lifecycle is identical whichever runs it (the N3 parity). On
the appliance the process driver (N4) is the default; this is for hosts already
running containerd. Command construction (`run -d --name`, `inspect -f
{{.State.Running}}`, `rm -f`) is pure and unit-tested; the exec path shells to
the CLI.
- **Files:** `crates/aog-node/{src/{lib.rs, containerd.rs (new)},
  tests/live_containerd.rs (new)}`.
- **Verify:** fmt + clippy `-D warnings` clean; **15 tests** (incl. 3
  command-construction unit tests) + the **N5 live gate green via docker**.
- **Gate:** a containerized workload lifecycle, parity with N4 ✓
  (`a_containerized_workload_has_a_lifecycle`, env-gated on `LOOM_CONTAINER_CLI`,
  run here against docker + `alpine`: start → Running → stop → not-Running →
  clean restart → Running). Skips inert on the air-gap path where no container
  CLI is configured. **Commit:** `LOOM-N5`.

### N6 — Edge admission + W5 offline-safe cache — DONE
`aog-node::edge::EdgeAdmission`: the node verifies a runtime token **locally**
(signature via the anchor key, expiry, revocation snapshot) and narrows the
route the caller may use to what current connectivity safely allows — the W5
state machine (`fabric-cache::evaluate` → `route_ceiling`) combined with the
token's own allowance and the request. Fail-static (I-4): degradation only
*reduces* privilege, never widens.
- **Offline-safe.** With the control plane unreachable but within soft TTL →
  `Degraded` → the node keeps deciding (still cloud); past hard TTL → `Expired`
  → `LocalOnly`. It never fails to decide; it narrows.
- **Air-gap (I-8).** An air-gapped node is `AirGapped` → `LocalOnly`, so a cloud
  request is narrowed to local — cloud denied.
- **Local auth.** A tampered, expired, or revoked token is denied without any
  control-plane round-trip (local asymmetric verify + the last-applied
  `RevocationSnapshot`).
- **Files:** `crates/aog-node/{Cargo.toml, src/{lib.rs, edge.rs (new)}}`.
- **Verify:** fmt + clippy `-D warnings` clean; **22 tests** pass (7 new).
- **Gate:** the node keeps issuing safe, narrowed decisions with the control
  plane unreachable ✓ (`an_unreachable_but_fresh_node_still_decides`,
  `a_stale_node_narrows_to_local`); an air-gapped node denies cloud routes ✓
  (`an_air_gapped_node_denies_cloud`); tampered / expired / revoked tokens denied
  locally. **Commit:** `LOOM-N6`.

### N7 — Health probes — DONE
`aog-node::probes`: node supervision. `keep_live(driver, run, handle)` restarts
an instance the driver reports not-Running (an unhealthy replica is replaced),
returning the fresh handle; a running one is left untouched. `ready_targets`
filters instances through a pluggable `ReadinessProbe` — only ready instances
take traffic. Both seams accept an HTTP `/healthz` / `/ready` behind the trait.
- **Files:** `crates/aog-node/src/{lib.rs, probes.rs (new)}`.
- **Verify:** fmt + clippy `-D warnings` clean; **25 tests** pass (3 new).
- **Gate:** an unhealthy replica is restarted / replaced ✓
  (`an_unhealthy_replica_is_restarted` — a stopped instance is restarted to
  Running; `a_healthy_replica_is_left_running` leaves a live one alone);
  readiness gates traffic ✓ (`readiness_gates_traffic` — only the ready instance
  is a target). **Commit:** `LOOM-N7`.
