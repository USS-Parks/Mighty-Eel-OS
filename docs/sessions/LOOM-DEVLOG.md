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
