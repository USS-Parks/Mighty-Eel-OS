# M1 / Phase E — AF-003 tenant-bound envelopes (evidence)

Prompt: E1/E3/E4 (envelope binding in AAD + signed thread; unseal authorization).
Finding: AF-003 High.

## Root cause
The AEAD AAD bound only the handling `Label`; `unseal` verified signature, expiry,
classification clearance, and permitted_ops but never the tenant/owner. Any valid,
sufficiently-cleared token could open any tenant's envelope.

## Changed files
- `crates/fabric-contracts/src/envelope.rs` — `Thread` gains
  `tenant_id` / `owner_subject_hash` / `audience` (skip-if-empty).
- `crates/fabric-envelope/src/lib.rs` — `EnvelopeBinding`, `ThreadSpec.binding`;
  `envelope_aad` binds label + binding into the AEAD AAD; binding signed into the
  thread; `envelope_binding()` reader.
- `crates/wsf-seal/src/lib.rs` — seal sets binding from the token; unseal refuses
  unbound / cross-tenant / cross-owner before Transit (receipted).
- `crates/aog-gateway/src/route.rs` — routing test Thread literal updated.
- Tests: `crates/fabric-envelope/tests/envelope.rs` (binding-tamper), new
  `crates/wsf-seal/tests/tenant_binding.rs`.

## Commands + results
- `cargo fmt --check` .................................. exit 0
- `cargo check --workspace` ........................... exit 0
- `cargo clippy -p fabric-contracts -p fabric-envelope -p wsf-seal -p aog-gateway --all-targets -- -D warnings -A clippy::pedantic` exit 0
- `cargo test -p fabric-envelope` ................... ok (6)
- `cargo test -p wsf-seal` .......................... ok (inline 3 + tenant_binding 4
  + live_seal skip)

## Negative controls
- `cross_tenant_unseal_is_refused_before_transit` — tenant-a token vs tenant-b
  envelope → `Unauthorized`, before OpenBao.
- `cross_owner_same_tenant_unseal_is_refused` — same tenant, other owner →
  `Unauthorized`.
- `unbound_v1_envelope_is_refused` — no binding → `Unauthorized` (no silent v1).
- `tampering_the_tenant_binding_breaks_the_thread` — rebinding after sealing →
  `InvalidSignature` (binding is signed).
- Positive: `owner_token_passes_binding_and_reaches_transit` — same tenant+owner →
  past the binding, fails only at the dummy Transit (`OpenBao` error, not
  `Unauthorized`).

## Deferred (honest)
- E2 per-tenant Transit key namespace (needs OpenBao policy); E5 offline v1
  migration command (unbound v1 refused now); E6 tenant-scoped storage/receipt
  keys; E7 live two-tenant OpenBao Transit gate (→ PROVEN). Audience binding is
  carried but not yet enforced (no token audience field yet).
