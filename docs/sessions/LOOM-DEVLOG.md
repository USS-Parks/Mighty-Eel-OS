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

### N8 — Attestation-liveness — the differentiator — DONE
`aog-node::attest`: liveness as "is it still the code we trust." `check`
re-measures a running workload (a pluggable `Measurer` — a TPM / Nitro PCR or an
image hash) against the measurement sealed at placement; a match is `Intact`, a
drift **evicts** the workload (driver stop) and returns an `Evicted` verdict
carrying its runtime token id. `revocation_for` builds a signed, emergency
`RevocationSnapshot` denying every drifted token — the artifact R9 fans out
estate-wide and the edge (N6) applies, so the token is denied everywhere. A
tampered replica is removed and cut off, not merely restarted.
- **Files:** `crates/aog-node/src/{lib.rs, attest.rs (new)}`.
- **Verify:** fmt + clippy `-D warnings` clean; **27 tests** pass (2 new).
- **Gate:** a tampered / drifted workload is evicted and its token denied
  estate-wide ✓ (`a_drifted_workload_is_evicted_and_revoked`: a drifted
  measurement stops the workload and puts its token in a signed emergency
  revocation snapshot that verifies against the anchor — the same snapshot R9
  distributes and N6 denies on); `an_intact_workload_is_left_running`.
  **Commit:** `LOOM-N8`.

### N9 — Eviction + drain — DONE
`aog-node::drain`: `plan_drain(reason, inflight, budget)` decides how to drain an
instance. A **planned** drain defers when the disruption budget (a
PodDisruptionBudget-analog) has no room, waits for in-flight authorized calls
when any remain (they are not dropped), and stops when there are none. A **Tier-0
revocation** drain is unconditional and immediate — `ForceStopNow`, ignoring both
in-flight work and the budget (I-9). `execute_drain` applies the action through
the driver.
- **Files:** `crates/aog-node/src/{lib.rs, drain.rs (new)}`.
- **Verify:** fmt + clippy `-D warnings` clean; **33 tests** pass (6 new).
- **Gate:** a graceful drain completes without dropping in-flight authorized
  calls ✓ (`a_graceful_drain_with_in_flight_does_not_stop` — the instance stays
  running while calls are in flight; `a_graceful_drain_with_no_in_flight_stops`);
  a revocation drain is immediate ✓
  (`a_revocation_drain_stops_the_instance_immediately`,
  `..._regardless_of_in_flight_or_budget`); the disruption budget is honoured
  (`..._defers_when_the_budget_is_exhausted`). **Commit:** `LOOM-N9`.

---

**M3b COMPLETE (Phases S + N) — the attested edge.** Phase S: the `aog-scheduler`
filter→score→bind engine — S1 framework + the fake-metrics defect purge
(absence-as-optimism inverted to fail-closed), S2 capacity + utilisation, S3 ring,
S4 the attestation predicate `classification_ceiling ≤ attestation_floor` (the
differentiator), S5 consolidation, S6 spread/HA, S8 preemption — plus the S7
`SchedulerController` that binds replicas, mints Capability-scoped runtime tokens,
and receipts each `Placement` through admission. Phase N: the new `aog-node`
runtime — verified registration (N1), heartbeat + reschedule-on-death (N2), the
CRI-shaped driver trait (N3) with process (N4) and containerd (N5) drivers, W5
offline-safe edge admission (N6), health probes (N7), attestation-liveness
evict-and-revoke (N8), and graceful / forced drain (N9). Trust posture throughout:
fail-closed / fail-static, per-action re-auth, never bearer coasting.
- **Verify:** `fmt --check` clean; each M3b crate clippy `--all-targets --no-deps
  -D warnings` clean; `cargo check --workspace` clean; **`cargo test --workspace`
  2000 passed / 2 ignored (180 suites)**. Four live gates green — **S7** (attested
  binding + scoped token) and **N2** (reschedule-on-death) vs **live OpenBao**;
  **N5** (containerized lifecycle) vs **docker**; plus the deterministic S/N gates.
- All on `session/LOOM-3` (off `session/LOOM-2` `23b25ce`), commits
  `ad32b26`→`50593f1` (17 commits), **NOT pushed/merged** — awaits Basho.
- Next milestone: **M3c — orchestration objects + HA** (Phases O + H), then the
  gated **Summit-Conformance** (Phase V).

---

## Phase O — Orchestration objects (`aog-controller` higher-order)

Built on `session/LOOM-4`, branched from `main` at `406fbf6` (the merged M3a+M3b
tip). One prompt = one focused commit + entry. Live gates (A3.2) for the
placement-touching prompts are written per prompt and run as a batch at the
Phase-O boundary against a live OpenBao (recorded there).

### O1 — `Workload` (Deployment analog): replica-set convergence — DONE
The S7 binding controller becomes the full Deployment analog. Placements are now
**replica-indexed** — replica `i` of workload `w` is `w-r<i>` — which is what lets
a node host more than one replica of one workload (packing) and makes scale-down a
precise "drop the ordinals at or beyond `replicas`".
- **`deploy.rs` (new) — the pure planner.** `plan_replicas(desired, existing,
  scheduler, request, snapshots) -> ReplicaPlan { create, delete, short }` is the
  whole convergence decision, factored out of the OpenBao side so it is
  deterministic and unit-testable. It runs the attested scheduler (Phase S) once
  per unfilled ordinal, threading two per-pass signals the scheduler cannot see:
  **spread** (each node already carrying a replica this pass is fed back as
  `already_placed_on`, so S6 fills fresh nodes first, then packs) and **capacity**
  (a node's free slots are decremented locally per placement, so the S2
  `CapacityFilter` bounds packing to the headroom actually reported; an undeclared
  slot budget is unbounded). Ring / attestation / readiness stay the scheduler's
  hard filters — an ordinal with no satisfying node is left `short`, never
  force-placed, and the caller requeues.
- **`scheduler.rs` — rewired.** `reconcile_workload` parses live placements to
  ordinals, plans, then scales **down** first (delete each excess `Placement` and
  `delete_kv` its runtime token from OpenBao, so the node cannot re-fetch a token
  for a replica that no longer exists — the running replica is drained by N9;
  estate-wide token revocation stays R9's `RevocationIntent`) and **up** (mint a
  scoped token per new ordinal, persist it, create the attested `Placement`
  through admission, which receipts the binding, K9). Converged passes touch
  nothing; a lingering shortfall requeues so a later node-join heals it.
- **Files:** `crates/aog-controller/src/{deploy.rs (new), scheduler.rs, lib.rs}`;
  `crates/aog-controller/tests/live_deploy.rs (new)`.
- **Verify:** `fmt` clean; `clippy -p aog-controller --all-targets --no-deps -D
  warnings` clean; `cargo test -p aog-controller` **35 passed** (8 new planner
  tests: spread, pack-beyond-nodes, capacity-bounds-and-short, unbounded-packs,
  scale-down-drops-highest, converged-no-op, no-ready-node-all-short, name round
  trip). **Live gate (A3.2 — written; runs in the Phase-O live batch):**
  `live_deploy.rs` — `packs_replicas_beyond_node_count` (3 replicas over 2 nodes →
  3 verifiable tokens, one node hosting two) and
  `scale_down_removes_the_dropped_replicas_token` (drop to 1 replica → `gw-r1`
  gone from the estate and its token no longer fetchable from OpenBao).
- **Gate:** declaring N replicas converges to N, correctly placed (packing +
  spread) ✓; scale-down removes the excess and clears its token ✓.
  **Commit:** `LOOM-O1`.

### O2 — Rollout controller (progressive / canary / blue-green) — DONE
A `RolloutPlan` is advanced through availability-safe steps by a **pure,
deterministic stepper** and a thin reconcile loop that receipts each step.
- **`rollout.rs` (new) — the stepper.** `rollout_progress(strategy, total,
  max_surge, max_unavailable, step) -> { updated, unavailable, complete }` is the
  whole decision — no clock, no estate read (A1.12 bar-5 determinism). The
  **availability floor** `available >= total - max_unavailable` holds at *every*
  step because `unavailable <= max_unavailable` by construction. Progressive
  cycles a window = surge + unavailable per step; canary validates a small first
  cohort then the rest; blue-green stands the new set up beside the old and
  switches atomically (zero downtime). `total_steps` gives the terminal step.
- **`RolloutController`.** Resolves the target `Workload`'s replica count, asks
  the stepper where the rollout is, and writes the next `status.step` (an admitted
  update → a receipt, K9) until `complete` → `Ready`. A missing target holds
  `Degraded` (fail-closed), re-checked on the resync heartbeat, never a phantom
  rollout. Physical replica replacement is O1 placement + N9 drain; O2 owns the
  order, the pace, and the receipt trail.
- **Files:** `crates/aog-controller/src/{rollout.rs (new), lib.rs}`;
  `crates/aog-controller/tests/rollout.rs (new)`.
- **Verify:** `fmt` + `clippy -p aog-controller --all-targets --no-deps -D
  warnings` clean; `cargo test -p aog-controller` **46 passed** (7 stepper unit
  tests incl. an exhaustive availability-floor property over every strategy ×
  total × surge × unavailable × step; 2 controller tests). **Gate:** rollout
  maintains availability ✓ (the floor property); each step receipted ✓
  (`receipts_len` grows `>= steps` as the plan reaches `Ready`).
  **Commit:** `LOOM-O2`.

### O3 — Automatic rollback on error-budget breach — DONE
The rollout controller gains an error budget: when a target's observed errors
exceed it mid-rollout, the rollout **reverses to its prior state** and ends
`Failed` — deterministically, every reverse step receipted.
- **Schema (aog-estate).** `RolloutPlanSpec.error_budget: u32` (`0` disables
  auto-rollback) and `RolloutPlanStatus.rolled_back: bool` (a rolled-back rollout
  ends `Failed`, not `Ready`) — both additive `#[serde(default)]`, round-trip clean.
- **`RolloutController` (rollout.rs).** New `ErrorBudgetProbe` (a sync telemetry
  read from receipts/meter) and a `with_error_budget(probe)` builder. Each
  reconcile, before stepping forward, checks `error_budget > 0 && observed >
  budget`: on a breach — or once a rollback is already under way — it reverses one
  step toward 0 (each an admitted receipt) and ends `Failed` at step 0. A rollback
  in flight never un-reverses even if the error signal later clears, so the
  reversal is deterministic and ledger-provable. Forward-only (O2) with no probe.
- **Files:** `crates/aog-estate/src/kinds.rs`,
  `crates/aog-controller/src/{rollout.rs, lib.rs}`;
  `crates/aog-controller/tests/rollout.rs`, `crates/aog-estate/tests/roundtrip.rs`.
- **Verify:** `fmt` + `clippy --all-targets --no-deps -D warnings` clean
  (aog-estate + aog-controller); `cargo test -p aog-estate` **21 passed**,
  `-p aog-controller` **47 passed** (O3 test: a rollout advances two steps clean,
  then a budget breach reverses it to step 0 / `Failed` / `rolled_back`).
  **Gate:** an injected error-budget breach auto-rolls-back deterministically to
  the prior state ✓. **Commit:** `LOOM-O3`.

### O4 — Budget-/ROI-aware autoscaler — DONE
The autoscaler scales a `Workload` on **load and economics together**, not load
alone. New `autoscale.rs`.
- **The pure decision.** `autoscale(current, AutoscaleSignals { utilization,
  budget_headroom, roi }, AutoscalePolicy) -> ScaleDecision` — no clock, no RNG.
  Saturated + affordable → `ScaleUp`; saturated but out of budget or at the ceiling
  → `RecommendHardware` (never overspend — the load needs hardware, not more
  replicas on the same tier); budget-inefficient (ROI ≤ floor) → `ScaleDown`; idle
  → consolidate `ScaleDown`; else `Hold`. Never below `min_replicas`. Saturation
  outranks low ROI (an overloaded workload's immediate need is capacity).
- **`AutoscaleController`.** Reads a real signal snapshot via the `AutoscaleProbe`
  seam (fed by node utilization + the gateway meter / SpendLedger) and applies
  scale up/down by writing `Workload.spec.replicas` (the HPA pattern — O1 then
  converges placements to match). No telemetry → hold (fail-closed, I-4).
  `RecommendHardware` is an operator/console decision, not automatic capacity
  fabrication (the gateway ROI recommender already surfaces it for humans).
- **Files:** `crates/aog-controller/src/{autoscale.rs (new), lib.rs}`;
  `crates/aog-controller/tests/autoscale.rs (new)`.
- **Verify:** `fmt` + `clippy -p aog-controller --all-targets --no-deps -D
  warnings` clean; `cargo test -p aog-controller` **58 passed** (9 pure-decision
  unit tests: up, out-of-budget → hardware, ceiling → hardware, idle,
  budget-inefficient, steady hold, min floor, saturation-outranks-ROI,
  determinism; 2 controller tests: one pass scales up on saturation, one
  consolidates on idle). **Gate:** scale decisions from a fixed fixture are
  deterministic and budget-respecting ✓. **Commit:** `LOOM-O4`.

### O5 — MissionContract operator — DONE
The mission scope envelope becomes concrete authority, and a run cannot exceed it.
New `mission.rs`.
- **Enforcement (`mission_allows`).** Pure, fail-closed: an agent bound to a
  `MissionContract` may take an action only if the tool is in `allowed_tools`, the
  system (when the contract restricts systems — empty = unrestricted) is in
  `allowed_systems`, and `calls_used < call_ceiling`. Out-of-scope or over-budget
  → `Deny`. The monetary `spend` budget rides the derived grant's credential (the
  existing SpendLedger), enforced where spend is.
- **Materialization (`MissionContractController`).** Reconciles a contract into
  one owned `ToolGrant` per allowed tool (scoped to `allowed_systems`, owned by
  the contract so the GC cascades), pruning grants for withdrawn tools. A tool
  outside the contract has no grant, so O6's toolproxy can never mint a credential
  the mission did not sanction. A spent contract (`calls_used >= call_ceiling`)
  is `Failed`; otherwise `Ready`.
- **Files:** `crates/aog-controller/src/{mission.rs (new), lib.rs}`;
  `crates/aog-controller/tests/mission.rs (new)`.
- **Verify:** `fmt` + `clippy -p aog-controller --all-targets --no-deps -D
  warnings` clean; `cargo test -p aog-controller` **67 passed** (7 enforcement
  unit tests: in-scope allow, tool/system out-of-scope deny, restricted-but-no-
  system deny, unrestricted-allows-any, call-ceiling deny, valid derived label
  names; 2 controller tests: materialize exactly the allowed-tool grants, prune on
  shrink). **Gate:** an agent cannot exceed its MissionContract scope/budget ✓.
  **Commit:** `LOOM-O5`.

### O6 — ToolGrant orchestration — DONE
The declarative `ToolGrant`s become a signed, versioned active-grant set on the
channel every proxy polls; revoking one halts the tool on every proxy. New
`toolgrants.rs`.
- **The signed set + edge cache.** `SignedGrantSet` (version + sorted
  `GrantEntry { tool, systems }`) is ML-DSA-signed over its canonical payload
  (signature cleared), exactly like an R6 policy bundle, so a proxy verifies it
  offline with the control-plane public key alone. `EdgeGrantCache::accept`
  refuses a bad signature and an anti-rollback stale set (version ≤ applied — a
  replay cannot resurrect a revoked grant); `allows(tool)` / `allows_system` is
  the per-call enforcement point.
- **`ToolGrantController`.** Compiles every live (non-terminating) `ToolGrant`
  into the set and publishes it to the `GrantStore` channel (`MemGrantStore` + the
  OpenBao poll path in production), advancing the version only on real change. A
  deleted or terminating grant is excluded → the next set omits its tool → the
  proxy denies its next call. Complements O5, which owns the grants' lifecycle.
- **Files:** `crates/aog-controller/src/{toolgrants.rs (new), lib.rs}`;
  `crates/aog-controller/tests/toolgrants.rs (new)`.
- **Verify:** `fmt` + `clippy -p aog-controller --all-targets --no-deps -D
  warnings` clean; `cargo test -p aog-controller` **72 passed** (4 unit tests:
  verify + allow, wrong-key refused, stale replay refused, revocation drops the
  tool; 1 controller test: two proxies both allow `calc`, then deleting its grant
  republishes a newer set and both proxies deny `calc` while `search` keeps
  working). **Gate:** revoking a ToolGrant halts the tool on every proxy ✓.
  **Commit:** `LOOM-O6`.

### O7 — Disruption budgets + node maintenance — DONE
Cordon a node out of scheduling, then drain it within a disruption budget. New
`maintenance.rs`; a one-line cordon exclusion in `scheduler.rs`.
- **Cordon.** A `Node` labelled `loom.io/unschedulable=true` (`CORDON_LABEL` /
  `is_cordoned`) is excluded by the scheduler's `node_snapshots` — no schema
  change — so it takes no new placements and a drained replica is never re-placed
  back onto it (S7 and the O1 planner share `node_snapshots`).
- **Disruption-budget drain (`plan_drain`).** Pure and deterministic: per pass it
  evicts at most `disruption_budget` replicas of any one workload and defers the
  rest, so a workload never drops more than its budget of replicas at once. A
  budget of 0 is treated as 1 (a drain must progress).
- **`MaintenanceController`.** Drains a cordoned node's placements in bounded
  passes; the scheduler re-places each on another **same-ring** node (the S3 ring
  filter is unchanged), so ring isolation holds throughout. Each eviction revokes
  the replica's runtime token via an optional OpenBao seam (mirroring O1
  scale-down); without it the controller drains estate-only.
- **Files:** `crates/aog-controller/src/{maintenance.rs (new), scheduler.rs,
  lib.rs}`; `crates/aog-controller/tests/{maintenance.rs (new),
  live_maintenance.rs (new)}`.
- **Verify:** `fmt` + `clippy -p aog-controller --all-targets --no-deps -D
  warnings` clean; `cargo test -p aog-controller` **81 passed** (6 planner unit
  tests: budget-per-workload, per-workload-not-per-node, zero-budget-progresses,
  determinism, empty-node, cordon-label; 2 controller tests: one pass evicts the
  budget then the node fully drains, uncordoned untouched). **Live gate (A3.2 —
  written; runs in the Phase-O live batch):** `live_maintenance.rs` — draining a
  cordoned node revokes the drained replica's token in OpenBao. **Gate:**
  maintenance drains within budget ✓; ring guarantees preserved (cordon exclusion
  + unchanged S3 filter) ✓. **Commit:** `LOOM-O7`.

---

**Phase O COMPLETE (O1–O7) — orchestration objects.** Deployment-analog replica
convergence (O1), progressive/canary/blue-green rollout (O2) with error-budget
auto-rollback (O3), budget-/ROI-aware autoscaling (O4), MissionContract scope
enforcement (O5), signed ToolGrant distribution with revoke-halts-every-proxy
(O6), and disruption-budget node maintenance (O7). All on `session/LOOM-4` (off
`main` `406fbf6`), commits `5329a7c`→`80ff793` (7 commits).
- **Verify:** each prompt `fmt` + `clippy -p aog-controller --all-targets
  --no-deps -D warnings` clean; `cargo test -p aog-controller` **81 passed**
  (deterministic unit + mock-controller gates for every prompt).
- **Phase-O live batch (A3.2) — green vs a live OpenBao** (Docker
  `openbao/openbao server -dev`): O1 `live_deploy` (packing wrote three
  replica-indexed tokens `gw-r0/r1/r2`; scale-down cleared the dropped token) + O7
  `live_maintenance` (drain revoked the token) + the S7 `live_scheduler`
  regression (replica-indexed binding intact) — **4 passed**, confirmed against
  OpenBao's real KV/approle state, not vacuous skips.
- NOT pushed/merged — awaits Basho.
- Next: **Phase H** (H1–H6) — HA, consensus hardening, DR, federation.

---

## Phase H — HA, consensus hardening, DR, federation

### H1 — Multi-node Raft (≥3-node control plane) — DONE
K3's single-node openraft node is promoted to a real multi-node control plane:
election, replication, leader failover, and a leadership-fenced `SharedGate`.
- **`raft/cluster.rs` (new) — the multi-node transport.** A `Cluster` registry of
  peer `Raft` handles; `ClusterNetwork` routes openraft's `append_entries` /
  `vote` / `install_snapshot` RPCs by direct call to the target — **real** openraft
  consensus across ≥3 in-process nodes. An `isolated` set injects a partition (a
  node in it neither sends nor receives RPCs), the seam H2 uses.
- **`RaftNode` (mod.rs).** Generalized `build<N>` over any network; `join` onto a
  `Cluster` (registers the handle so peers reach it); `initialize` + `add_learner`
  + `change_membership` to form the cluster; `is_leader` / `current_leader` /
  `wait_for_leader`; and a `leadership()` watch.
- **`SharedGate::follow` (aog-controller runtime).** Drives a controller's gate
  from a node's `leadership()` watch, so only the leader reconciles — leadership is
  fenced to the gate, not assumed.
- **Files:** `crates/aog-store/src/raft/{cluster.rs (new), mod.rs}`;
  `crates/aog-controller/src/runtime.rs`; `crates/aog-controller/tests/ha.rs (new)`.
- **Verify:** `fmt` + `clippy -p aog-store` / `-p aog-controller --all-targets
  --no-deps -D warnings` clean; `cargo test -p aog-store` **7 passed** (K3 intact),
  `-p aog-controller` **82 passed**. H1 gate test (`ha.rs`): a 3-node cluster forms
  and replicates a committed write to both followers; partitioning the leader
  triggers a **re-election within SLO** among the survivors, the committed write
  **survives with zero loss**, the new leader commits a fresh write, and the
  `SharedGate` follows to the new leader. **Gate:** leader loss → new leader within
  SLO, zero committed-state loss ✓.
- **Scope note (honest):** an in-process 3-node cluster running real openraft
  consensus with a real leader partition — genuine election/replication/commit, not
  simulated. The over-the-wire mTLS transport is deployment packaging; fencing the
  *old* partitioned leader (classic Raft still has it believe it leads until it
  sees a higher term) is split-brain safety — proven under a real partition in H2.
  **Commit:** `LOOM-H1`.

### H2 — Split-brain fencing (load-bearing) — DONE
The split-brain hazard H1 exposed — a partitioned leader still *believing* it
leads — is closed with a quorum-confirmed leadership check.
- **`RaftNode::confirm_leadership(timeout)` (mod.rs).** A ReadIndex (openraft
  `ensure_linearizable`) that returns `Ok` only when a quorum still acknowledges
  this node as leader. A partitioned minority cannot confirm and returns `false` —
  the split-brain-safe check the trust path uses, **not** the stale metrics view.
- **The fencing.** A `SharedGate` set from `confirm_leadership` closes on a
  minority, so its controllers serve no authoritative decision under partition
  (fail-closed, doctrine I-4). The majority elects a leader that *can* confirm and
  serves the authoritative estate.
- **Files:** `crates/aog-store/src/raft/mod.rs`;
  `crates/aog-controller/tests/split_brain.rs (new)`.
- **Verify:** `fmt` + `clippy -p aog-store` / `-p aog-controller --all-targets
  --no-deps -D warnings` clean; the H2 gate test (`split_brain.rs`) **1 passed**:
  with a **real** injected partition (the `Cluster` severs the leader's RPCs) the
  minority leader fences (`confirm_leadership` → false) while its stale metrics
  still call it leader; the majority elects a quorum-confirmed leader; a capability
  revocation commits across the majority; and the fenced minority — though it still
  holds the stale `granted` value — confirms no quorum, so it authorizes nothing.
  **Gate:** injected partition → minority serves no allow ✓; kill switch honored
  under partition ✓.
- **Note:** a real transport fault + openraft's real quorum reaction (not a
  simulated verdict); in-process 3-node cluster (A3.2 — the wire transport is
  deployment packaging; the correctness it carries is what's pinned).
  **Commit:** `LOOM-H2`.

### H3 — Snapshot / compaction / restore — DONE
A Raft snapshot compacts the state machine; a restore reproduces the exact estate;
the separate receipt chain stays chained across it.
- **`RaftNode::snapshot(timeout)` + `last_snapshot()` (mod.rs).** Trigger a Raft
  snapshot and wait until it is built to the applied index, so the log before it
  can be purged. A node recovers the same estate from the snapshot + log tail.
- **Estate restore (aog-store).** `snapshot.rs`: write 20 keys, snapshot, restart
  from the same dir → the estate is reproduced **exactly** (every key, value, and
  revision; the global revision preserved).
- **Receipt-chain continuity (aog-apiserver).** `restore.rs`: because intent
  (aog-store) and proof (wsf-ledger) are physically separate stores (A1.4), an
  estate snapshot/restore neither reads nor writes the receipt chain — proven by
  building a hash-chained receipt ledger, snapshotting/restoring an estate, then
  showing the receipt chain head is unchanged, its signed pack still verifies
  off-host with the public key alone, and a post-restore receipt links unbroken
  onto the pre-restore head.
- **Files:** `crates/aog-store/src/raft/mod.rs`,
  `crates/aog-store/tests/snapshot.rs (new)`;
  `crates/aog-apiserver/tests/restore.rs (new)`.
- **Verify:** `fmt` + `clippy -p aog-store` / `-p aog-apiserver --all-targets
  --no-deps -D warnings` clean; `cargo test -p aog-store --test snapshot` **1
  passed**, `-p aog-apiserver --test restore` **1 passed**. **Gate:** restore from
  snapshot reproduces the exact estate ✓; receipts remain chained across restore ✓.
  **Commit:** `LOOM-H3`.

### H4 — Backup + DR runbook — DONE
An encrypted estate backup + a documented cold-restore runbook, proven by a DR
drill that restores from a cold backup by the runbook alone.
- **`aog_apiserver::backup` (new module).** `backup_estate(entries, data_key)`
  serializes the estate's committed `key→value` content and **envelope-seals** it
  (AES-256-GCM under a 32-byte DR data key, `fabric-envelope`) — ciphertext at
  rest, safe for removable media. `restore_estate(blob, data_key)` unseals it back
  to entries; a wrong key, tampered blob, or a seal made for another purpose (AAD)
  all fail closed. The data key is escrowed (OpenBao Transit-wrapped in prod,
  operator escrow air-gapped), never stored beside the backup.
- **Runbook.** `docs/LOOM-DR-RUNBOOK.md` — take-a-backup + cold-restore procedures
  (recover key from escrow → read blob from media → unseal → bootstrap fresh node →
  re-apply → verify → re-form HA), plus the intent/proof store separation (the
  receipt ledger recovers from its own segments, H3).
- **Files:** `crates/aog-apiserver/src/{backup.rs (new), lib.rs}`,
  `crates/aog-apiserver/Cargo.toml` (+serde);
  `crates/aog-apiserver/tests/dr_drill.rs (new)`; `docs/LOOM-DR-RUNBOOK.md (new)`.
- **Verify:** `fmt` + `clippy -p aog-apiserver --all-targets --no-deps -D warnings`
  clean; `cargo test -p aog-apiserver` **35 passed** (3 backup unit tests:
  round-trip, ciphertext-at-rest, wrong-key-fails-closed; the DR drill). **DR drill
  (`dr_drill.rs`):** back up a 12-key estate sealed, delete the primary stores
  entirely, then cold-restore from the sealed blob on "media" + the escrowed key
  into a fresh node — the content is reproduced by the runbook steps alone and the
  blob is confirmed ciphertext at rest. **Gate:** full DR drill from cold backup
  succeeds by the runbook alone ✓. **Commit:** `LOOM-H4`.

### H5 — Air-gap federation (signed removable-media snapshots) — DONE
New crate `aog-federation`: a source estate federates policy + revocation to an
air-gapped peer by a **signed snapshot on media**, verifiable offline, no network.
- **`FederationSnapshot`.** Carries a monotonic `version`, the `source` id, the
  `FederatedPolicy`s a peer adopts (PolicyBundle content) and the
  `FederatedRevocation`s it honors (RevocationIntent targets), ML-DSA-signed over
  its canonical payload (signature cleared). `to_media` / `from_media` are the byte
  serialization written to / read from removable media; a snapshot never carries a
  secret (names, not credentials — the peer resolves those from its own OpenBao).
- **`Peer`.** The receiving air-gapped estate: verifies a snapshot with the
  source's public key **alone** (offline), refuses a bad signature and an
  anti-rollback stale replay (version ≤ applied — a replay cannot re-apply a
  superseded policy or un-revoke a token), and on accept returns the policies +
  revocations to apply and advances the applied version.
- **Files:** `crates/aog-federation/{Cargo.toml, src/lib.rs}` (new crate);
  workspace member + `Cargo.lock`.
- **Verify:** `fmt` + `clippy -p aog-federation --all-targets --no-deps -D
  warnings` clean; `cargo test -p aog-federation` **4 passed**: a policy + a
  revocation are signed at site-a, serialized to media bytes, and — with only the
  bytes crossing — verified offline by the peer with the public key alone and
  applied; a wrong source key, a tampered snapshot, and a stale replay are each
  refused. **Gate:** a policy + a revocation cross an air gap on media and apply
  verifiably, no network ✓. **Commit:** `LOOM-H5`.

### H6 — Production guard — DONE
The base WSF dev-fixture guard is reused and extended with Loom's HA + signed-
bundle requirements, so an insecure deployment fails closed in production.
- **`loom_production_guard` (wsf-hardening).** Runs the base `production_guard`
  (dev OpenBao root token, plaintext HTTP transport, weak/uniform HMAC key) and
  adds, in production: `single_node_quorum` (a control plane with `< 3` Raft voters
  is not HA) and `unsigned_bundle` (an unsigned policy bundle). Empty =
  production-ready; a no-op in Dev. `assert_loom_production_ready` errors with the
  violations.
- **Files:** `crates/wsf-hardening/src/lib.rs`;
  `crates/wsf-hardening/tests/loom_guard.rs (new)`.
- **Verify:** `fmt` + `clippy -p wsf-hardening --all-targets --no-deps -D warnings`
  clean; `cargo test -p wsf-hardening` **10 passed** (H6: dev no-op, prod-ready
  passes, single-node quorum blocked, unsigned bundle blocked, dev-OpenBao fixture
  blocked via the reused base guard). **Gate:** prod guard blocks any dev fixture /
  single-node quorum ✓. **Commit:** `LOOM-H6`.

---

**Phase H COMPLETE (H1–H6) — HA, consensus hardening, DR, federation.** Multi-node
Raft with leader failover + a leadership-fenced SharedGate (H1), quorum-confirmed
split-brain fencing under a real partition (H2), snapshot/compaction/restore with
receipt-chain continuity (H3), an envelope-sealed backup + DR runbook + cold-drill
(H4), signed removable-media air-gap federation (H5, new `aog-federation`), and the
Loom production guard (H6). All on `session/LOOM-4` (off `main` `406fbf6`), commits
`b736dc5`→`LOOM-H6`.
- **Verify:** each prompt `fmt` + `clippy --all-targets --no-deps -D warnings`
  clean; per-crate tests green. Consensus proofs (H1/H2) run **real openraft** over
  an in-process ≥3-node cluster with **real partitions** injected at the transport;
  the over-the-wire mTLS transport is deployment packaging (Phase V will exercise
  the containerized multi-node harness end to end).

**M3c COMPLETE (Phases O + H).** Orchestration objects (O1–O7) + HA/consensus/DR/
federation (H1–H6), 13 prompts, all gated green, on `session/LOOM-4`. Phase-O live
gates ran green vs a live OpenBao (Docker). NOT pushed/merged — awaits Basho.
Next milestone: **Summit-Conformance (Phase V)** — the gated "Kubernetes-grade"
proof (conformance suite + chaos/soak/scale + the containerized split-brain &
kill-switch-under-scale harness).

---

## Phase V — Summit-Conformance (the gated "as good as Kubernetes" proof)

**Milestone:** Summit-Conformance — the final M3 milestone; green Phase V is the
precondition to claim "Kubernetes-grade, woven" externally (addendum A1.12 bar 8 /
A5). STS approved 2026-07-05 (full V1–V11, aggressive profile). Worktree
`session/LOOM-5` off `main` `259737a` (M3c).

**V-start targets (fixed now so V8/V9/V10 are falsifiable, per A4) — aggressive:**
- Control plane: **5** Raft voters. Edge nodes: **5** `aog-node`. Workloads: **100**.
- V9 weave-overhead: **p99 ≤ 1 ms** (all per-action checks live; met by local
  crypto, never by skipping a check — Doctrine I-3).
- V10 revocation-to-denial ("the kill number"): **p99 ≤ 3 s** across all replicas;
  a replica past freshness-TTL fails closed (Doctrine I-9 / RC-KILL).
- Governing contract: `AOG-WSF-ROBUSTNESS-AND-ZERO-TRUST-DOCTRINE.md` (I-1..I-9);
  the D9 RC suite lands at V11.
- Extended no-mock-only (A3.2): the consensus / scheduling / admission /
  node-lifecycle / kill-under-scale bars each ship ≥1 test against a live
  ≥3-CP + ≥2-node Docker harness + live OpenBao with **real** partitions — the
  5+5 estate is the aggressive-profile harness.

### V1 — `aog-conformance` suite — DONE
The executable conformance framework + runnable suite: the analog of the K8s
conformance tests, scoped to AOG kinds. `run()` executes the A1.12 bars against a
reference estate and returns a serializable `ConformanceReport`; the
`aog-conformance` bin emits it as JSON and exits non-zero on any failure, so a
customer or CI lane can gate on it directly.
- **`crates/aog-conformance` (new).** `BarId` (A1.12 bars 1–8), `BarStatus`
  {Pass, Fail, Pending}, `BarReport`, `ConformanceReport` (`is_green` /
  `is_summit_ready`). Bar 2 (linearizable writes) is asserted green in-process
  against the real `aog-store` Raft state machine: two compare-and-sets pinned to
  one base revision — the first commits (`RaftResponse::Applied`), the second is
  rejected stale (`RaftResponse::Rejected`), the committed value is the winner's,
  and the global revision advances by exactly one. No lost update.
- **Honest coverage (CANON §11):** the remaining bars are *registered* against the
  Phase-V prompt that implements them on the live harness (bar 1 → V2, bar 3 → V4,
  bar 7 → V5, bars 4/5 → V7, bar 6 → V8) and reported `pending` — never a pass the
  suite did not run. `is_summit_ready()` requires zero pending (A5).
- **Files:** `crates/aog-conformance/{Cargo.toml, src/lib.rs, src/bars.rs,
  src/main.rs}`; workspace member added.
- **Verify:** `cargo fmt` clean; `cargo clippy -p aog-conformance --all-targets
  --no-deps -D warnings` clean; `cargo test -p aog-conformance` **1 passed** (suite
  green, bar 2 asserted). **Gate:** suite green on the reference estate; each bar
  carries an assertion or its owning prompt ✓. **Commit:** `LOOM-V1`.

### V2 — Reconcile idempotency fuzz (bar 1) — DONE
Extends the R1 replay gate (three fixed delivery histories) to a reproducible
fuzz over the real `aog-controller` reconcile runtime: a fixed base estate is
reconciled to convergence, then a fixed late history (update + delete + create)
is delivered under randomized modes — events reordered, duplicated, and
occasionally genuinely dropped by overflowing the watch buffer (a foreign prefix,
so the Loom end state is untouched) forcing the controller to re-list. Every
history must converge to the store's one authoritative end state.
- **`crates/aog-conformance/src/bars.rs`.** `idempotent_reconcile(histories, seed)`
  over a real `RaftNode` + `Controller` + a level-triggered `Recorder` reconciler
  (the R1 pattern), driven by a deterministic SplitMix64 PRNG so a divergence is
  reported with its exact seed + history index. Bar 1 wired into the suite
  (`run()` asserts it at 48 histories); `aog-controller` added as a dependency.
- **Standard lane:** `v2_reconcile_idempotency_fuzz_converges` runs **500**
  randomized histories (reorder + duplicate + overflow-drop), green in **46 s**;
  zero divergence.
- **Full gate:** `v2_reconcile_idempotency_fuzz_full_10k` runs the plan's **10⁴**
  histories; `#[ignore]` (durable Raft writes, ~minutes) so routine `cargo test`
  stays fast — run in the opt-in nightly/CI lane with `-- --ignored`. Not a silent
  cap (Doctrine D8): the full count is in the tree and runnable.
- **Verify:** `cargo fmt` clean; `cargo clippy -p aog-conformance --all-targets
  --no-deps -D warnings` clean; standard-lane tests green (suite + 500-history
  fuzz, 46 s). **Gate:** randomized drop/dup/reorder histories converge; zero
  divergent end-states ✓ (500 in-lane; 10⁴ nightly). **Commit:** `LOOM-V2`.

### V2 — 10⁴ confirmation
The full 10⁴-history idempotency fuzz (`v2_reconcile_idempotency_fuzz_full_10k`,
`#[ignore]`) ran green: **1 passed in 1655.90 s (~27.6 min), zero divergence** across
10,000 randomized reorder/duplicate/overflow-drop delivery histories. The plan's V2
gate is confirmed at full scale; routine runs use the 500-history standard lane.

### V3 — Linearizability under faults, Jepsen-style (bar 2 deepened) — DONE
Deepens bar 2 (V1's deterministic CAS) to linearizability under concurrent clients +
real fault injection. `linearizable_under_faults(clients, attempts, seed)`: N client
tasks race compare-and-set *increments* of one counter through a real 3-node Raft
cluster while a fault task repeatedly isolates then heals a single node (never a
majority), forcing real leader failovers. Clients write only to a quorum-confirmed
leader (`confirm_leadership`), so a fenced minority never serves an allow. The
register invariant — **acknowledged increments ≤ final counter** — catches a lost
update or a stale allow (either would push acks above the counter); only benign
in-flight ambiguity raises the counter above acks.
- **Files:** `crates/aog-conformance/src/bars.rs` (`linearizable_under_faults`,
  `confirmed_leader`, `parse_counter`); wired into suite bar 2 in `lib.rs` at a
  modest in-suite scale (3 clients × 15). Dedicated gate
  `v3_linearizability_under_faults` at 4 clients × 60.
- **Test-isolation fix:** `scratch()` now yields a unique dir per call (pid +
  monotonic counter) so concurrently-run tests never share a redb file — a latent
  bug (fixed dir names) that surfaced once all bars ran in parallel; also hardens
  V1/V2.
- **Verify:** `cargo fmt` clean; `cargo clippy -p aog-conformance --all-targets
  --no-deps -D warnings` clean; **all standard tests green (3 passed, 1 ignored) in
  46.8 s** (suite incl. in-suite fault check + 500-fuzz + V3 gate). **Gate:**
  concurrent clients under fault injection; no linearizability violation ✓.
  **Commit:** `LOOM-V3`.

---

## Phase V — Harness sub-milestone (VH) — the containerized multi-node estate

V1–V3 prove the consistency bars in-process (real openraft + real transport faults).
A3.2 requires the partition/kill/scale gates (V4/V5/V7/V8/V10) on a **live
containerized** multi-node harness with **real network partitions**. That harness
must be built: the Loom control plane had no wire transport (a single-node no-op
network only) and no node daemons — the "deployment packaging" the plan defers to
Phase V. VH1–VH6 build it, then the gates run on it.

### VH1 — Raft wire transport (`aog-wire`) — DONE
The over-the-wire counterpart of the in-process `ClusterNetwork`: consensus now
carries over a real socket.
- **`aog-store` seam:** `RaftNode::start_with_network<N: RaftNetworkFactory>` (build a
  node on a caller-supplied transport) + `RaftNode::raft()` (the openraft handle the
  wire server serves and the daemon drives membership through).
- **`crates/aog-wire` (new):** `WireNetwork` `RaftNetworkFactory` reaches each peer
  over HTTP at the URL in its `BasicNode` addr, JSON-serializing openraft's
  `append_entries`/`vote`/`install_snapshot` and lifting a peer's `RaftError` into
  `RPCError::RemoteError` (transport failure → `Unreachable`). `router(Arc<RaftNode>)`
  serves `/raft/{append-entries,vote,install-snapshot}`; `serve(node, addr)` binds.
- **Gate:** a 3-node cluster over loopback sockets (not the in-process `Cluster`)
  initializes, promotes learners to voters over the wire, elects a leader, and a
  committed leader write replicates to both followers across sockets ✓
  (`cargo test -p aog-wire` 1 passed, 3.34 s).
- **mTLS deferred to VH5:** sender-constraint per doctrine I-3 layers on where
  per-node certs are generated; VH1 proves the consensus carries correctly first.
- **Verify:** fmt + clippy (`aog-wire` + `aog-store`) `--all-targets --no-deps -D
  warnings` clean; wire-consensus test green. **Commit:** `LOOM-VH1`.

### VH2 — `aogd` control-plane node daemon — DONE
The runnable node daemon that turns VH1's wire transport into a *driveable* control
plane: a `RaftNode` on `aog-wire` plus a thin admin API the containerized
conformance harness (VH4+) forms and steers a cluster through. Membership is the
mechanism VH1 exercised only from a test; VH2 is the daemon that exposes it.
- **`crates/aogd` (new, lib + bin).** `Daemon::start` opens a `RaftNode` on the
  `WireNetwork` transport (recover-only — it forms no cluster); `Daemon::app` merges
  `aog-wire`'s Raft peer routes (`/raft/*`) with the admin router into one axum app.
  `Config::from_env` (`AOGD_NODE_ID` / `AOGD_DATA_DIR` / `AOGD_LISTEN` + optional
  `AOGD_ADVERTISE`, default `http://<listen>`) drives the `aogd` binary; it logs via
  `tracing` (the workspace denies `print_*`).
- **Admin API (`src/admin.rs`).** `POST /admin/{initialize,add-learner,change-membership,write,get}`
  + `GET /admin/leader` + `GET /healthz`. Membership operations carry each peer's
  real URL and are issued against the raw openraft handle (`RaftNode::raft()`): the
  `RaftNode::{initialize,add_learner,change_membership}` wrappers address peers by an
  empty id-only `BasicNode` — correct for the in-process `Cluster`, but the wire
  transport reaches a peer by the URL in its `BasicNode`, so the daemon supplies it.
  A node/raft failure is a fail-closed `500` carrying its reason (never a silent
  success; doctrine D7).
- **`Client` (`src/client.rs`).** The async harness client (form cluster / write /
  get / leader / healthz); a non-2xx is surfaced as `ClientError::Status`, never
  swallowed. The lib re-exports the store vocabulary a driver needs
  (`Op`/`Precondition`/`Versioned`/`RaftResponse`).
- **Deferred to VH5 (honest scope).** mTLS / sender-constraint (I-3), the
  authenticated `aog-apiserver` CRUD, `Sealer`/`Authenticator` keys, and the OpenBao
  mount. VH2 proves the cluster forms and replicates over real sockets end to end
  through the daemon; the trust surface constrains that mechanism next.
- **Files:** `crates/aogd/{Cargo.toml, src/{lib,api,admin,client,main}.rs,
  tests/daemon.rs}`; workspace member added.
- **Verify:** `cargo fmt -p aogd --check` clean; `cargo clippy -p aogd --all-targets
  --no-deps -D warnings` clean; `cargo test -p aogd` **1 passed** (the gate, 1.52 s);
  `cargo check --workspace` clean (additive, 0 regressions).
- **Gate:** three `aogd` daemons on loopback, driven **only through the admin API**,
  initialize, promote learners to voters over the wire, elect a leader (reported via
  `/admin/leader`), and a committed leader write replicates to both followers — each
  read back through its own admin `get` ✓. **Commit:** `LOOM-VH2`.

---

**Harness progress: VH1–VH2 done.** The wire transport (VH1) and the node daemon
that runs on it (VH2) exist; a multi-node control plane now forms and replicates
over sockets, driven through the daemon's admin API. Next: **VH3** — the `aog-node`
edge daemon (the Ring-1/2/3 worker side) → VH4 Dockerfile → VH5 5+5 Compose +
OpenBao + per-node mTLS certs → VH6 real network-partition tooling; then the live
gates (V4 split-brain / V5 kill-under-scale / V7 chaos+soak / V8 scale / V10
revocation SLO) run on that estate.
