# Full-Repo Remediation DEVLOG

Initiative: remediation of the 2026-07-08 full-repo audit.
Plan of record: [../audits/2026-07-08-full-repo/REMEDIATION-PSPR.md](../audits/2026-07-08-full-repo/REMEDIATION-PSPR.md).
Audit: [../audits/2026-07-08-full-repo/FULL-REPO-SECURITY-AUDIT.md](../audits/2026-07-08-full-repo/FULL-REPO-SECURITY-AUDIT.md).
Branch: session/AUDIT-FIX-1 (off main @ 700cf2b). Commit per prompt, gates green; no push
without explicit approval. Each entry: objective, evidence, verify result, commit.

---

## Phase 0 - Containment & lane

### 0.1 - Lane artifacts + baseline freeze

Baseline HEAD 700cf2b (== main, post Phase F/X). Toolchain: cargo 1.96.1, clippy 0.1.96,
cargo-audit 0.22.1, cargo-deny 0.19.7, gitleaks 8.30.1, detect-secrets 1.5.0, ruff 0.15.14,
docker 29.6.1 (+ openbao/openbao image), moto 5.2.2, protoc 34.1. Absent on this host:
`zfs`, `/dev/tpm*`, a >=3-node cluster host, artifact-signing infra (owner-lane per PSPR X3/X6, V6).

Baseline gates (captured at 700cf2b during the audit, frozen as the pre-remediation
reference): `cargo clippy --workspace -- -D warnings -A clippy::pedantic` PASS; `ruff
check .` PASS; `cargo audit` PASS (0 vulns / 518 deps); `cargo deny check` PASS; `gitleaks
detect` PASS (no leaks); `detect-secrets` baselined; no-slop full scan PASS (mechanical).

Artifacts: this DEVLOG; the audit report + PSPR (docs/audits/2026-07-08-full-repo/);
evidence tree test-evidence/full-repo-remediation/{M0..M6}. Finding register is the audit
report's tables + the PSPR Appendix A closure matrix.

Verify: branch created clean off 700cf2b; docs land under docs/ (no-slop exempt). Commit: (this change set).
### 0.2 - Emergency containment (C1/C2)

Objective: stop off-host reach to the unauthenticated `/admin/*` and plaintext `/raft/*`
surfaces before the real auth/mTLS fixes (phase A) - contain by network exposure.

Confirmed: `AOGD_LISTEN` is a required operator-set SocketAddr with no default
(`aogd/src/lib.rs:108`), so a `0.0.0.0` bind exposes both surfaces; `main.rs:20` bound it
with no guard.

Changed (`crates/aogd/src/main.rs`): `check_bind_containment` refuses a non-loopback bind
before `TcpListener::bind` - loopback proceeds; non-loopback (incl. all-interfaces
`0.0.0.0`) fails closed with a message pointing at phase A, unless the operator sets
`AOGD_ALLOW_INSECURE_BIND=1` to accept the risk on an isolated network.

Verify: fmt clean; `cargo clippy -p aogd --all-targets -- -D warnings -A clippy::pedantic`
PASS; `cargo test -p aogd` PASS (new: loopback-ok / non-loopback-refused / opt-in matrix).
C1/C2 CONTAINED (root fixes owned by A1/A2). Commit: (this change set).
## Phase A - AOG control-plane auth & transport

### A1 - authenticate the aogd admin API (C1 root fix)

The `/admin/*` surface was mounted with no auth layer and merged onto the daemon socket
alongside the authenticated `/apis/**` CRUD (`aogd/src/lib.rs`), so any peer could commit
arbitrary Raft `Op`s.

Changed: the admin router now takes the front-door `Authenticator` (threaded from the
daemon's `AppState` when an anchor is provisioned) and gates the mutating routes
(initialize / add-learner / change-membership / write / get) behind a `require_admin`
middleware - a valid WSF token carrying the `aog-admin` role; `/healthz` and read-only
`/admin/leader` stay open. The write leader-forward hop propagates the caller's
`x-wsf-token` so the leader re-authenticates the original caller (the hop is not trusted
until mTLS lands in A2). Pre-anchor bootstrap (no authenticator) relies on the 0.2 loopback
containment.

Verify: fmt; clippy -D warnings; `cargo test -p aogd` PASS (new admin-role gate; existing
daemon/edge/auth_crud suites green - they run anchorless so bootstrap stays open). The full
authenticated-refusal black-box proof is the A6 multi-node live gate (deferred - needs a
>=3-node host). C1 root-fixed at the code boundary. Commit: (this change set).
### A3 - authorize deletes against the target (H3)

`validate` ran K7 policy only `if let Some(object)` (`admission.rs:168`) and deletes carry
`object: None` (`handlers.rs:164`), so any authenticated principal could delete any object
incl. a `RevocationIntent` (reversing a live kill).

Changed (`aog-apiserver/src/admission.rs`, Verb::Delete): after loading the target, run the
same `self.policy.evaluate(&target, principal)` the create/update path uses, and refuse a
cross-tenant delete (tenant-scoped principal may only delete objects in its own tenant).
Deletes now traverse the K7 authorization gate.

Verify: fmt; clippy -D warnings; `cargo test -p aog-apiserver` PASS (35 tests; existing
create/update/delete flows green under the added check). Cross-tenant / kill-reversal
black-box proof is the A6 live gate. Residual: a kind-level "RevocationIntent delete
requires the aog-admin authority" rule beyond classification/compliance is a tracked
follow-on. Commit: (this change set).

### A2 / A4 / A5 / A6 - DEFERRED (multi-node + cert infra)

These need resources absent on this host and are dispositioned to the owner/hardware lane
per PSPR 0.2: A2 (wire `aog-wire::NodeTls` mTLS into the serve path + https peer URLs)
needs a CA + a >=3-node cluster to provision and prove; A4 (quorum-fenced reads via
`confirm_leadership`) and A5 (durable/replicated receipt ledger) need a multi-node estate
to implement and gate; A6 is the >=3-node live gate itself. C1 is contained (0.2) + auth-
gated (A1) and H3 is fixed in the meantime; the transport-security + consensus-fence legs
land when a cluster host is available. Critical path continues at Phase K (safe next prompt).

## Phase K - trust primitives (fabric-token / fabric-crypto)

### K1 - attenuation empty-model-list widening (H1)

An empty `allowed_models` list means "every model" (the unrestricted sentinel). The child
narrowing check (`fabric-token/src/lib.rs`, `narrow_child`) only rejected a child model that
was *absent from* a restricted parent's set - so a child restriction of `Some(vec![])`
against a parent restricted to e.g. `["gpt-4"]` passed the `iter().all(..)` vacuously and
*widened* the child back to all models. A monotonicity break: attenuation must only narrow.

Changed (`fabric-token/src/lib.rs`): the `allowed_models` guard now also refuses an empty
child list when the parent is restricted -
`if !parent.allowed_models.is_empty() && (models.is_empty() || !models.iter().all(..))` ->
`AttenuationWidens { axis: "allowed_models" }`. An empty child against an *unrestricted*
parent stays legal (both mean "all"); a genuine subset still narrows.

Verify: fmt; `cargo clippy -p fabric-token --all-targets -- -D warnings -A clippy::pedantic`
PASS; `cargo test -p fabric-token` PASS - new `attenuation_monotonicity_tests`: empty-child-
vs-restricted-parent refused, subset narrows, empty-child-vs-unrestricted-parent ok. H1
closed at the primitive. Commit: (this change set).

### K2 - bind key_id + alg into the token signature (H2)

`signing_hash` (`fabric-token/src/lib.rs`) removed the *entire* `signature` object before
hashing, so `signature.alg` and `signature.key_id` were signed over by nothing - they sat
outside the signed payload. `issue` writes alg+key_id, then hashes, then fills the value.

Severity correction (adversarial re-trace at fix time, honest per CANON 10). The audit filed
H2 as a "revocation bypass". Re-checked against the actual token verification path, it is NOT
currently exploitable: `Authenticator` (`aog-apiserver/src/auth.rs`) verifies every token
against a single fixed trust-anchor public key with a hardcoded `MlDsa87Verifier`, and
revocation keys off `token_id` / `subject_hash` - never the signing `key_id`, and the
algorithm is never selected from `token.signature.alg`. So mutating key_id/alg on a token in
flight buys an attacker nothing today (the value must still verify under the anchor). This is
a **latent** key-identity / algorithm-substitution gap, not a live bypass - downgraded from
High-exploitable to defense-in-depth hardening.

It becomes live the moment token verification either resolves the key *by* key_id (multi-key
/ rotation - the `fabric-proof` bundle keyring already does exactly this for proof artifacts)
or picks the verify algorithm from the token. Binding both fields now, JWS-style, closes the
class before that seam opens.

Changed: `signing_hash` strips only `signature.value` (the bytes, absent at signing time) and
keeps `alg` + `key_id` in the signed payload. `issue` and `verify` share `signing_hash`, so
they stay mutually consistent; no persisted golden signatures exist to break (all fixtures
mint fresh - verified by grep + the downstream suites).

Verify: fmt; clippy -D warnings PASS; `cargo test -p fabric-token` PASS - new
`tampered_key_id_or_alg_fails_verification` (swap key_id -> InvalidSignature; swap alg ->
InvalidSignature; fails on the old code, passes now). Blast radius clean: `cargo test -p
aog-apiserver -p wsf-bridge -p wsf-cache -p aogd -p fabric-revocation` all green (mint+verify
round-trips unaffected). Commit: (this change set).

### K3 - zeroize the ML-DSA secret key + KDF seed (H-crypto-hygiene)

`RustCryptoMlDsa87` held `secret_key: Vec<u8>` (the offline/air-gap ML-DSA-87 signing key)
with no `Drop`, so on drop the key stayed in freed heap memory; likewise the 32-byte KDF
`seed_bytes` in `keypair()` - which alone reconstructs the whole secret key - was left on the
stack. A memory-hygiene gap (post-use secret residue), not a logic flaw.

Changed (`fabric-crypto`): added the audited `zeroize` 1.8 crate (already in-tree via
wsf-broker) as a direct dep; `keypair()` now `seed_bytes.zeroize()`s once the keypair is
derived; and a manual `impl Drop for RustCryptoMlDsa87` wipes `secret_key` on drop. The wipe
is deliberately scoped to the secret only - `key_id` and `public_key` are not sensitive.

Honest scope: this is best-effort. It clears the buffers this crate owns; it does not reach
copies `ml-dsa`'s `key_gen_internal` / `sign` make internally (third-party, outside our
control) or the plain `(Vec,Vec)` `keypair()` hands to external key-storage callers by design.
Manual `Drop` (not `#[derive(ZeroizeOnDrop)]`) so only the one secret field is targeted.

Verify: fmt; clippy -D warnings PASS; `cargo test -p fabric-crypto` PASS - existing sign/verify
+ from_keypair round-trips green (the zeroize wiring preserves function), plus new
`signer_drop_is_sound_after_zeroize_wiring` (drop an ephemeral signer, an independent one
still verifies: no double-free / cross-talk). The wipe itself is a Drop-time guarantee of the
`zeroize` crate, verified by code review + preserved-function tests (not observable post-free
without UB). Commit: (this change set).

### K4 - attenuation-monotonicity property suite (H1 breadth)

K1 fixed the empty-model-list widening; K4 proves the invariant holds across *every* axis,
not just the one the audit named. New `fabric-token/tests/attenuation_property.rs`: a
deterministic seeded (`StdRng::seed_from_u64`, no flakes) generator that, over 1000
iterations round-robining all seven narrowable axes, constructs a genuine *widening* on the
chosen axis - later expiry; a route / model / role / compliance-scope the parent lacks; the
H1 empty-model sentinel; a classification above the parent ceiling; a budget cap over the
parent's remaining - and asserts each returns `AttenuationWidens { axis }` for exactly that
axis. A companion control (`valid_narrowing_on_every_axis_succeeds`) proves a legitimate
narrowing on each axis is *accepted*, so the rejection suite is not passing vacuously; it
also documents the routes/scopes-vs-models asymmetry (empty routes/scopes = narrowing; empty
models = widening). `rand` 0.8 added as a dev-dep (repo convention; no proptest in-tree).

Verify: fmt; clippy -D warnings PASS (factored the control's fn-pointer vec behind a
`Narrowing` type alias for `clippy::type_complexity`); `cargo test -p fabric-token` PASS -
`randomized_widening_on_every_axis_is_rejected` (1000 iters, every axis hit >100x, zero
widenings admitted) + the narrowing control green. Offline mode is monotone by construction
(`set_offline_mode` only forces offline on; no widening input exists) - noted, not generated.

### K5 - DEFERRED (live attenuation/revocation gate)

The black-box live proof of H1/H2 through real OpenBao custody (Appendix A: H1,H2 live =
K5) is a live-gate prompt in the same class as A6 / U5 / V6 / X2. Deferred to the
owner/hardware lane per PSPR 0.2 alongside the other live proofs; the in-process proofs
(K1/K2 regression tests + the K4 property suite) close H1/H2 at the code boundary in the
meantime. Critical path continues at Phase U (audit-chain verification, reachable).

## Phase U - audit-chain verification (mai-compliance)

### U1 - enforce interval-boundary signatures (H7)

`verify_chain` (`mai-compliance/src/audit/chain.rs`) verified only signatures that were
*present*: the loop did `let Some(sig) = entry.signature else { continue }`. An attacker who
stripped the signature off a signing-boundary entry (one the signer stamps because
`(id+1) % signature_interval == 0`, per `finalize`) sailed through - the tamper-evidence a
periodic signature is supposed to provide was defeated by simply deleting it.

Changed: when a verifier is configured and `signature_interval > 0`, `verify_chain` now
computes `is_boundary = (id+1).is_multiple_of(interval)` (the exact predicate `finalize`
signs on) and requires a signature on every boundary entry - a missing one is the new
fail-closed `ChainError::SignatureMissing { id }`, not a skip. Any present signature is still
verified as before; non-boundary entries may legitimately be unsigned.

Verify: fmt; `cargo clippy -p mai-compliance --all-targets -- -D warnings -A clippy::pedantic`
PASS; `cargo test -p mai-compliance` PASS (332 tests) - new
`verify_chain_rejects_stripped_boundary_signature` (clean signed chain verifies; stripping a
boundary signature -> `SignatureMissing { id: 1 }`). Existing signed/unsigned/tamper/monotonic
suites green (no legitimate unsigned-boundary-with-verifier path regressed). H7 closed at the
verifier; the persisted-WAL leg is U2. Commit: (this change set).

### U3 - fix the >8192-entry HeadHashNonZero false positive (H8, part 1)

`verify_full` verifies `store.entries()` - the in-memory tail, capped at
`max_in_memory` (default 8192). Once the log outgrows that cap the genesis entry (id 0) is
evicted, so the tail's first entry has a non-zero `previous_hash`; `verify_chain`'s
`first.is_chain_head()` assertion then fired `HeadHashNonZero` on a perfectly clean chain -
a false positive that made a healthy long-running log report as tampered.

Changed: split `verify_chain` (asserts the genesis head, unchanged public signature) from a
new `pub(crate) verify_segment` (linkage + monotonicity + boundary-signature checks, no head
assertion); `verify_chain` now delegates to it. `verify_full` verifies from genesis while the
tail still holds id 0 and switches to `verify_segment` once the head is evicted
(`entries.first().is_none_or(|e| e.id == 0)`). The genesis guarantee is retained for the
from-head path; only the evicted-tail case drops the assertion that no longer applies. The
evicted prefix's own tamper-evidence comes from the WAL in U2.

Verify: fmt; clippy -D warnings PASS; `cargo test -p mai-compliance` PASS (333 tests) - new
`verify_full_passes_after_head_eviction` (max_in_memory=4, 10 entries -> head evicted -> clean
log verifies). U1's stripped-signature regression and the head-nonzero/tamper/monotonic
suites stay green (the genesis check still fires when the slice starts at id 0). Commit: (this
change set).

### U2 - verify the persisted WAL from the true head (H8, part 2)

U3 stopped the false positive but `verify_full` still only saw the in-memory tail, so tampering
with an entry already *evicted* to the WAL went undetected. U2 makes `verify_full` verify the
durable WAL from its true head.

Two coupled gaps had to be closed first:
- **Read path.** `StoreSealer` was write-only (`seal`); there was no way to read a sealed WAL
  back. Added `unseal` to the trait - `NullSealer` returns the bytes; `AeadSealer` delegates to
  its existing decrypt, mapping the AEAD failure to a fail-closed `StoreError::WalUnseal`.
- **Framing.** The WAL wrote `sealed || '\n'`. Sealed (AES-GCM) records are binary and can
  contain `0x0A`, so newline framing could not round-trip them - the sealed WAL was effectively
  unreadable. Records are now **hex-framed** (`hex(sealed) || '\n'`), one record per line, safe
  for arbitrary bytes. No external reader consumes this WAL (grep-confirmed) and no correctly-
  readable sealed WAL existed before, so this establishes the format rather than migrating data.

Added `AuditStore::read_wal` (read -> hex-decode -> unseal -> JSON per line, every failure an
error) and `has_wal`; `chain::verify_wal`, which splits the WAL at each chain head (all-zero
`previous_hash`) so a daily-rotation-reset WAL of several from-genesis segments verifies
segment-by-segment; and `ChainError::WalRead` so a read/decode failure surfaces fail-closed
through `verify_full`'s existing signature. `verify_full` now prefers the WAL when configured
and falls back to the U3 in-memory path otherwise.

Verify: fmt; clippy -D warnings PASS; `cargo test -p mai-compliance` PASS (335 tests) - new
`verify_full_detects_tampering_with_evicted_entry` (evict id 0 to WAL, tamper it on disk ->
`LinkBroken`, status Tampered) and `verify_full_verifies_sealed_wal_from_head` (AeadSealer:
clean sealed WAL round-trips through unseal + hex and verifies); `wal_roundtrip_in_temp_file`
updated for hex framing. H8 closed (U1 boundary sigs + U3 false positive + U2 from-head WAL);
the restart-tamper live proof is U5. Commit: (this change set).

### U4 - production crypto guard (2/3 verified done; signer leg deferred)

"Refuse NullSigner / NullSealer / AcceptAll* in production." Two legs are already
implemented and wired fail-closed into boot (verified, not re-done):
- **NullSealer**: `mai-api/src/sealer_builder.rs::build_sealer` returns
  `NullSealerAllowedInProduction` when a production profile sets
  `audit.allow_null_sealer=true`, and `ship_profile.rs` rejects the same at parse time;
  `server.rs::apply_ship_profile` maps the builder error to `ServerError::Init` (boot fails).
  Runtime introspection is `production_guard.rs` AUDIT-005 + `compliance_sealer_real`.
- **AcceptAll bundle verifier**: `build_trust_components` returns
  `TrustBuildError::AcceptAllInProduction`, wired to `ServerError::Init` the same way
  (`mai-api/tests/trust_production.rs` covers both).

**Deferred - NullSigner (audit-chain signer):** `server.rs` builds the compliance
`AuditLog` with the default `NullSigner` (no `.signer(..)`), so the chain is hash-linked but
unsigned in production. Closing this *correctly* is not just a boot guard: a chain signer is
only useful if its signatures are verifiable, which needs a **stable** provisioned ML-DSA
chain-signing key (its public half registered with verifiers) plus `ChainConfig.signing_key_id`
/ anchor coordination. Unlike the AEAD data key, an ephemeral per-boot key is wrong (verifiers
could never match it), and wiring a signer with an empty `key_id` would emit signatures nothing
can verify - disguised incompleteness (CANON 11). This is signing-infra work (PSPR 0.2), lane-
deferred with A2 / K5 / V1 / V6. Until it lands, the chain's tamper-evidence is the hash link
(now verified from the true head, U1-U3) rather than periodic signatures. Recorded, not faked.

Verify: no code change this prompt (the two live legs already exist and are gated); disposition
recorded here. Commit: (this change set, docs-only).

### U5 - restart + tamper gate (H7/H8 restart evidence)

The remaining U evidence: verification must survive a process restart. After a restart the
in-memory tail is empty, so verification has to lean on the durable WAL - exactly the U2 path.
New unit test `verify_full_survives_restart_via_wal`: record entries into a WAL-backed log,
drop it (simulate restart), build a *fresh* `AuditLog` on the same WAL path (asserting its
in-memory tail is empty), and confirm `verify_full` re-reads the WAL and verifies; then tamper
a persisted entry on disk and confirm a third fresh instance detects it (`LinkBroken`). This is
a single-process restart simulation (the multi-node live gate is A6, deferred); it closes the
in-process restart-evidence leg for H7/H8.

Verify: fmt; clippy -D warnings PASS; `cargo test -p mai-compliance` PASS (336 tests) - the new
restart test green. With U1 (boundary sigs), U2 (from-head WAL), U3 (long-log false positive)
and now U5 (restart evidence), H7/H8 are closed at the code boundary; the only U item open is
K5-class live infra (U-live, owner lane). Commit: (this change set).

## Phase V - vault integrity truth (mai-vault)

### V2 + V3 - full-field audit hash + honest primitive label (H6)

`AuditWriter::compute_entry_hash` (`mai-vault/src/audit.rs`) hashed only 5 of the
`VaultAuditEntry`'s 13 fields - `previous_hash`, `timestamp`, `profile_id`, `action`,
`status`. The other eight (`entry_id`, `model_id`, `tokens_in/out`, `latency_ms`,
`adapter_id`, `error_code`, `ip_source`) were outside the hash, so an attacker could rewrite
which model ran, which adapter, the source IP, the error code, or the token counts of a
persisted entry without breaking the chain (H6). Separately, the module + field docs claimed
`SHA3-256` while the code computed BLAKE3 - a false control claim (CANON 0.5).

V2: `compute_entry_hash` now takes the whole `&VaultAuditEntry` and hashes the canonical
serialization with the two *output* fields (`entry_hash`, `pqc_signature`) blanked - binding
every security-relevant field. The three callers (append verify, `verify_chain` loop,
`build_audit_entry`) go through the one function, so producer and verifier stay consistent;
`build_audit_entry` now populates the struct then hashes it. This changes `entry_hash` values
(a format change), acceptable because the vault is pre-production (H5 TPM sealing not done, so
no certified vault chain exists to migrate). V3: the module doc and the `mai-core`
`previous_hash`/`entry_hash` field docs now say BLAKE3-256, matching the code (the `sha3` crate
stays in use for HKDF in `tpm.rs`/`pqc.rs` - unrelated to the audit hash).

Verify: fmt; `cargo clippy -p mai-vault -p mai-core --all-targets -- -D warnings -A
clippy::pedantic` PASS; `cargo test -p mai-vault` PASS (84) + `mai-core` (83) - new
`entry_hash_covers_previously_unhashed_fields` (flipping `entry_id`/`model_id`/`adapter_id`/
`error_code`/`ip_source`/`tokens_out`/`latency_ms` each breaks the digest); existing
chain-integrity / broken-chain / checkpoint-signature suites green. H6 closed.

### V1 / V4 / V5 / V6 - DEFERRED (TPM hardware + convergence follow-on)

- **V1 (H5) master-key TPM seal** and **V4 KEK TPM seal**: `mai-vault/src/tpm.rs` is a
  software simulation (BLAKE3 over a fixed PCR string), and this host has no `/dev/tpm*`
  (PSPR 0.2 baseline). Real sealing, or the "fail-closed + production guard that refuses to
  certify readiness without a real seal + honest non-production doc on the simulated TPM"
  alternative, is owner/hardware-lane work (with the TPM/attestation legs S1-S4 and V6). It
  wants its own prompt - a vault-readiness production guard - not a drive-by.
- **V5 audit-chain convergence**: dedupe the two divergent chain implementations
  (mai-vault vs the `mai-compliance` full-canonical model) onto one. Reachable but a pure
  refactor with no open vuln (both are individually correct after V2); scheduled as a
  maintainability follow-on rather than ahead of the open Phase D/G/P robustness Highs.
- **V6 live ZFS+TPM gate**: needs the real ZFS+TPM host (0.2), owner lane with X2/X3.

Critical path continues at Phase D (DoS / panic-safety, reachable).

## Phase S - attested scheduling (aog-scheduler)

### S4-doc - reconcile the attestation filter's overstated claim (H4, §0.5)

H4: attested placement trusts self-declared attestation. `AttestationFilter`
(`aog-scheduler/src/filters.rs`) requires, for a Restricted+ ceiling, that the node declare a
hardware platform (TPM/Nitro/SEV-SNP) + a recorded PCR - a bare claim is refused. But the
`platform`/`pcr` are the node's **self-declared** values (`NodeSnapshot` projects the node's
own `spec.attestation`); the filter checks their *presence*, never a control-plane-verified
hardware quote. Its doc, however, claimed placement was "provably held" via "a real hardware
root … backed by" - a control the code does not implement (a §0.5 / §11 docs-vs-reality
overstatement layered on the H4 trust gap).

Changed (doc/comment only): rewrote the filter doc to state honestly that platform+PCR are
self-declared and their *presence* is checked, not a verified quote, with the CP quote-
verification (signed quote + AK cert chain + pinned PCRs + fresh nonce) marked deferred to the
hardware lane; added a `TODO(basho)` at the check site recording the same. No behavior/test
change - the doc now matches the code.

**Root fix DEFERRED (S1-S3, hardware).** Actually closing H4 needs CP verification of a real
hardware quote (vendor roots + pinned reference PCRs + nonce), which needs TPM/attestation
hardware (PSPR 0.2, with V1/V4/V6). The audit's S4 fail-closed alternative - *deny* Restricted+
placement outright until verification lands - is reachable but removes a placement capability
(it flips `ring3_secret_placed_on_attested_node` to Pending), a product decision that is
owner-gated (0.2), not a unilateral remediation edit. H4 therefore remains **open** (honest:
counts against §0.6 stop-ship at re-ship) with the gap now accurately documented and tracked
in-code, and two closure options spelled out for the owner. Verify: fmt; clippy -D warnings
PASS; `cargo test -p aog-scheduler` PASS (unchanged behavior). Commit: (this change set).

## Phase D - DoS / panic-safety (aog-gateway, mai-api, ...)

### D3 - provider HTTP client hang guards (H11)

The OpenAI and Anthropic provider clients (`aog-gateway/src/provider/{openai,anthropic}.rs`)
were built with bare `reqwest::Client::new()` - no timeouts. A hung or unroutable backend
could stall a request (and the gateway worker behind it) indefinitely.

Fix: a shared `provider::build_http_client()` both providers now use, with a `connect_timeout`
(10s, bounds TCP+TLS establishment) and an idle `read_timeout` (120s, bounds the gap between
response bytes). Crucially it sets **no total `timeout`**: these clients stream chat
completions over SSE, which legitimately run long, so a total request timeout (as in the
non-streaming `spend.rs` KV client) would truncate healthy streams. An idle read timeout
catches a genuine hang - a backend that connects then sends nothing - without that side
effect. The static builder only fails on a TLS-backend init fault (unrecoverable), mirroring
`reqwest::Client::new`'s internal `expect`.

Verify: fmt; `cargo clippy -p aog-gateway --all-targets -- -D warnings -A clippy::pedantic`
PASS; `cargo test -p aog-gateway` PASS (existing OpenAI/Anthropic surface + streaming tests
unaffected - normal operation intact). A timing-based hung-backend proof is a flaky unit test
by nature; it belongs to the D9 fuzz/soak gate, not here. H11 closed at the client config.
Commit: (this change set).

### D2 - bound the STT audio buffer (H10)

`AudioBuffer::append_frame` (`mai-agent/src/stt.rs`) guarded only on accumulated *duration*,
computed as `samples * 1000 / sample_rate`. Two ways that floors to zero while bytes still
accumulate: a **sub-sample** frame (`frame_bytes < bytes_per_sample*channels` -> `samples == 0`)
and a **sub-millisecond** frame (a single 16-bit sample at 16 kHz is `1000/16000 == 0` ms).
Either lets an attacker stream tiny frames that each append bytes but add 0 ms, so the duration
cap never trips and `data` grows unbounded (OOM, H10). A degenerate format
(`bit_depth/channels == 0`) additionally divided by zero in `frame_duration_ms` (panic).

Fix: `append_frame` now, before any duration arithmetic, (1) rejects an empty frame and any
frame that is not a whole number of samples (`frame.len() % frame_align`), which also guards
`frame_align != 0` so the division can't panic; and (2) enforces an absolute byte cap
(`max_bytes = bytes_per_second * max_duration_secs`, the memory a full utterance would occupy),
refusing a frame that would exceed it. The byte cap is what actually bounds the sub-millisecond
case the duration guard cannot. New `AgentError::{MalformedAudioFrame, AudioBytesExceeded}`.

Verify: fmt; `cargo clippy -p mai-agent --all-targets -- -D warnings -A clippy::pedantic` PASS;
`cargo test -p mai-agent` PASS (67) - new `zero_duration_frame_flood_is_byte_bounded` (100k
single-sample 0-ms frames -> `AudioBytesExceeded`, `byte_count <= 32000`) and
`malformed_audio_frames_are_rejected` (odd-length + empty rejected, whole-sample accepted);
existing feed/silence/take suites green. H10 closed. Commit: (this change set).

### D1 - /v1/status verify DoS (H9) — VERIFIED already mitigated

Re-traced H9 against current source (adversarial verification, as with H2). The
unauthenticated-flood-triggers-O(n)-verify-under-lock vector is not present in the shipped
code:
- The only auth-exempt endpoint that verifies is `production_probe` (`/v1/health/production`,
  `handlers/health.rs`). It verifies just the **last 64 entries** (`read_recent(64)`) inside a
  500 ms `tokio::time::timeout`, then verifies lock-free (the 64-clone is the only work under
  the store lock) - bounded, time-boxed, not an O(n) whole-chain verify.
- An unauthenticated flood is additionally capped by the SEC-95 token-bucket rate limiter,
  layered **outside** auth (`routes.rs`) precisely so a flood cannot exhaust the auth/verify
  path.
- The full O(n) chain verify (`verify_audit`, `/v1/compliance/audit/verify`) is auth +
  `view_audit` permission-gated - not unauthenticated. `compliance_status` returns the cached
  `integrity_status()` (O(1)), never a live verify.
- The only unbounded in-memory ledger is `MemoryAuditWriter`, the explicit dev/test writer;
  production uses the WAL-backed writer. mai-compliance's store is bounded (`max_in_memory`
  eviction, U3), and its `verify_full` runs the verify outside the status lock (U2).

So H9's four sub-concerns (auth-or-strip, cache, bound the ledger, no-lock-across-verify) are
each already satisfied by existing hardening (SHIP-11 bounded probe + SEC-95 rate limit +
auth-gated full verify) plus this remediation's U2/U3. The audit over-stated H9 by not crediting
the 64-entry bound + timeout + rate limit. No code change - adding auth to a load-balancer
health probe or re-bounding an already-bounded verify would be wrong/gold-plating (§13). H9
dispositioned closed at the code boundary; docs-only.

**Phase D Highs status:** H9 (verified mitigated), H10 (D2, fixed), H11 (D3, fixed) - all
closed. The Phase D **Mediums** (D4 lock-poison, D5 breaker `from_secs_f64`, D6 toolproxy TTL,
D7 SSE slot release, D8 streaming budget) and the D9 fuzz/soak gate remain as reachable
robustness follow-ons below the Critical/High bar. Commit: (this change set, docs-only).

### D5 - circuit-breaker cooldown panic on a long outage (M)

`CircuitBreaker::calculate_cooldown` (`mai-core/src/circuit_breaker.rs`) computed
`Duration::from_secs_f64(base * cooldown_multiplier.powf(cooldown_cycles))`. `cooldown_cycles`
grows on every trip, so a long (multi-day) outage drives the exponential `multiplier` past
f64's range to `+inf`; `Duration::from_secs_f64` **panics** on a non-finite (or out-of-range)
value. The result was already `.min(cooldown_max)`, so the panic was pure downside.

Fix: compute the target seconds, and if it is non-finite or at/above `cooldown_max`, return
`cooldown_max` directly - `from_secs_f64` is only called on a finite, in-range value. The
cooldown clamps to max exactly as before, minus the panic.

Verify: fmt; clippy -D warnings PASS; `cargo test -p mai-core circuit_breaker` PASS (10) - new
`test_cooldown_saturates_without_panic_after_many_trips` (cooldown_cycles = 100000 and 2000,
both -> cooldown_max, no panic). D5 gate ("a multi-day outage does not panic") holds. Commit:
(this change set).

### D8 - streaming budget bypass (M) — owner-gated

Confirmed: `surface_openai.rs`'s streaming branch calls `provider.stream()` with **no**
`meter::record`/`record_spend` (only classified-cloud egress is refused, AF-08), while the
non-streaming path meters + decrements. A budget-bearing token can therefore stream past its
cap. The codebase already notes per-chunk stream metering is an unimplemented follow-on
(`meter.rs:12`).

The two closures are both owner-gated, so no unilateral code change here:
- **Terminal-frame metering** (the audit's primary option) is a real feature - wrap the SSE
  stream, capture the terminal usage frame, then `record_spend`. That is the correct fix and
  belongs with the streaming-metering follow-on, not a drive-by.
- **Refuse budgeted streaming** (the audit's fallback) closes the bypass in a few lines, but in
  this gateway effectively *all* virtual keys are budgeted, so it disables streaming for every
  real key - a product-capability regression. Imposing that unilaterally is out of scope
  (§0.2, like the S4 deny decision); it also breaks the live streaming demo test
  (`openai_surface.rs`, budgeted key) which cannot be verified without the OpenBao live gate.

Prototyped the refuse-fallback, verified it closes the bypass, then reverted it (surface
unchanged vs HEAD) pending the owner's choice. D8 stays open (Medium; cost-overrun via
streaming, not a data/authz breach) with both fixes and their trade-offs documented. Commit:
(this change set, docs-only).

## Phase G - compliance / routing fail-closed (mai-compliance, aog-gateway)

### G1 - composer fail-open on an unvetted request (M, PHI egress)

Two sites treated an **empty** compliance decision set (no module vetted the request) as
permissive, so disabling the HIPAA module could egress PHI to cloud:
- **Enforce path** (`aog-gateway/src/policy.rs::evaluate`): `allowed_cloud = aggregate.allowed`.
  `allowed` is vacuously `true` over an empty set, so a disabled HIPAA module (composer drops
  its decision -> empty set) yielded `allowed_cloud = true -> CloudAllowed`. Fixed:
  `allowed_cloud = aggregate.allowed && !aggregate.modules_applied.is_empty()` - an unvetted
  request is not cloud-eligible and routes local.
- **Audit vocabulary** (`mai-compliance/src/audit/entry.rs::from_aggregate`): `(true,
  Some(Cloud) | None) => Allow` collapsed a `None` (unvetted) route onto `Allow`, mislabelling
  the egress as permitted. Split so `None` fails closed to `LocalOnly`.

aog-apiserver's `policy.rs` empty->allowed was checked and left as-is: there an empty set means
the resource declares no compliance scopes (nothing to enforce), a different, correct semantic -
not a PHI-cloud-routing fail-open.

Verify: fmt; `cargo clippy -p mai-compliance -p aog-gateway --all-targets -- -D warnings -A
clippy::pedantic` PASS; `cargo test -p mai-compliance -p aog-gateway` PASS (337 + gateway) -
new `unvetted_none_route_fails_closed_to_local` (None -> LocalOnly, not Allow) and
`disabled_module_is_unvetted_not_cloud_allowed` (HIPAA disabled -> empty vetting set -> not
cloud-allowed). The G1 egress gate ("disabling HIPAA does not egress PHI in Enforce") holds.
Commit: (this change set).

### G2 - classifier fail-closed on an empty required tier (M)

`RuleBasedClassifier::new` (`mai-router/src/classifier.rs`) compiled whatever pattern set it
was handed and never checked a tier was populated. A config that omits or mis-keys the
`regulated` tier (its patterns catch PHI/SSN) compiled to zero regulated regexes, so regulated
data matched nothing and classified as `Public` -> cloud-eligible. A test even enshrined it
(`test_empty_config_classifies_everything_public`).

Fix: after compiling, `new` fails with the new `ClassifierError::EmptyRequiredTier` when
`regulated` has no patterns - a mis-keyed/stripped config is refused at construction, never
silently accepted. Compilation still runs first, so a bad regex is still `InvalidPattern`. The
lower tiers (internal/sensitive) and `critical` stay operator-optional per deployment, matching
the audit gate's focus ("never classifies regulated as Public"); production uses `baseline()`,
which populates regulated, so it is unaffected.

Verify: fmt; clippy -D warnings PASS; `cargo test -p mai-router` PASS (63) - the fail-open test
flipped to `test_empty_regulated_tier_is_rejected` (empty -> `EmptyRequiredTier`), plus
`test_regulated_only_config_constructs` (regulated-only config builds and classifies). Commit:
(this change set).

### G3 - router forces Local on a medical entity (M, PHI egress)

`DefaultRouter::route` (`mai-router/src/router.rs`) forced Local for an export-controlled
(`has_export_controlled`) or tribal (`has_tribal`) entity regardless of classification, but
computed no `has_medical` - a medical/PHI entity had no entity-floor. It stayed local only if
the *classifier* independently rated the text at/above the cloud ceiling; a medical entity the
classifier scored low (the baseline dictionary flags "hospital", which is not a regulated
classifier pattern) fell through to the default cloud route - PHI egress.

Fix: compute `has_medical` and force Local on it in step 2, mirroring export-controlled/tribal
(reason "medical/PHI entity detected (HIPAA baseline)"). The entity floor is now independent of
the classifier for all three regulated entity kinds. (The config-driven rules engine already
had the HIPAA medical->local rule in `rules-config/hipaa.toml`; this closes the *hardcoded*
`DefaultRouter` path that omitted it.)

Verify: fmt; clippy -D warnings PASS; `cargo test -p mai-router` PASS (64) - new
`test_medical_entity_forces_local_below_ceiling` ("is there a hospital nearby" -> Local via the
medical entity, which previously routed cloud); existing `test_phi_query_routes_local` /
export / tribal cases green. G3 gate ("a medical-context query routes Local") holds. Commit:
(this change set).

## Phase P - posture & auth hardening (wsf-cache, ...)

### P2 - wsf-cache clock fail-closed (M, clock rollback re-opens cloud)

`Ring3Cache::connectivity` computed `age_secs = now_secs.saturating_sub(last_refresh_secs)`.
A clock **rollback** (now < last refresh) saturates the age to 0 - read as freshest = Online -
so a rewound clock re-opened cloud egress on an appliance that should have decayed to
local-only. A **pre-epoch** `now` hit the same floor: `decide` maps a non-representable
(negative) timestamp to `now_secs = 0`, which then subtracts to 0.

Fix (`connectivity`): `now_secs.checked_sub(last_refresh_secs).unwrap_or(u64::MAX)` - time at
or before the last refresh did not advance, so the age is treated as maximal and freshness
decays to `Expired` (route ceiling local-only). It still routes through `evaluate`, so a
sticky operator air-gap keeps precedence. The pre-epoch case flows through the same guard
(0 < last_refresh -> max age) once a refresh has happened; the never-refreshed initial state
is already non-fresh (`reachable == false`, `last_refresh_secs == 0`).

Verify: fmt; `cargo clippy -p wsf-cache --all-targets -- -D warnings -A clippy::pedantic` PASS;
`cargo test -p wsf-cache` PASS (8) - new `clock_rollback_does_not_reopen_cloud` (refresh fresh,
present a day-earlier clock -> Expired, no CloudAllowed) and `pre_epoch_clock_is_expired`
(negative timestamp -> Expired, no CloudAllowed); the fresh / hard-TTL / air-gap suites stay
green. P2 gate ("clock rollback cannot re-open cloud egress") holds. Commit: (this change set).

### P3 - WebSocket identity from middleware, not the in-band handshake (M, priv-esc)

`handle_auth_handshake` (`mai-api/src/streaming/ws.rs`) derived the connection's role and
profile id from the client-supplied `auth.handshake` payload (`handshake.role`) and did **no**
token validation at all - the doc even claimed "Validates the profile token" (false). Any
client could send `{profile_id, role: "admin"}` and be granted admin permissions: a complete
WebSocket auth bypass / privilege escalation. (`/v1/ws` is not auth-exempt, so the upgrade
request was already API-key-authenticated by the middleware - that verified identity was then
ignored.)

Fix: `ws_upgrade` now extracts the middleware-authenticated `ProfileInfo` and threads it into
the connection (`ConnectionState::authenticated(profile)`), which is authenticated from the
start. `handle_auth_handshake` no longer reads the payload - it confirms the middleware
identity and ignores any declared `profile_id`/`role`; the forgeable `AuthHandshake` struct is
deleted. The module doc is corrected to state auth comes from the API key, the handshake only
confirms (§0.5).

Verify: fmt; `cargo clippy -p mai-api --all-targets -- -D warnings -A clippy::pedantic` PASS;
`cargo test -p mai-api --lib streaming::ws` PASS (15) - new
`test_handshake_ignores_declared_role_uses_middleware_identity` (a Guest connection sending a
handshake declaring `admin` stays Guest, profile id unchanged) and
`test_handshake_without_authenticated_profile_fails_closed`; removed the tests that asserted
the in-band-role behavior. P3 gate ("a Guest key cannot self-declare admin over WS") holds.
Commit: (this change set).