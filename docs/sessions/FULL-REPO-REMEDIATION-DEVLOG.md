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