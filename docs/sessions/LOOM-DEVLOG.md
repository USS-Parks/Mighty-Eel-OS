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
