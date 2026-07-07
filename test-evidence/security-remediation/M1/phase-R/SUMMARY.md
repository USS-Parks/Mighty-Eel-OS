# M1 / Phase R — AF-006 revocation consumption (evidence)

Prompt: R1 (revocation store) + R3 (seal/broker consumer integration), on the
Phase-T `VerificationContext`. Finding: AF-006 Medium.

## Root cause
`wsf-seal` and `wsf-broker` verified only signature + on-token revocation_status,
never a signed revocation snapshot — a revoked-by-snapshot token still sealed data
and brokered credentials.

## Changed files
- `crates/fabric-revocation/src/lib.rs` (+ Cargo chrono) — `RevocationStore`
  (anti-rollback, monotonic, last-known-good) + `BadTimestamp`/`Expired`/`Rollback`.
- `crates/wsf-seal/src/lib.rs` (+ Cargo fabric-revocation) —
  `SealService.with_revocation`; `verify_token` uses `verify_in_context`.
- `crates/wsf-broker/src/lib.rs` (+ Cargo fabric-revocation) —
  `AwsStsBroker.with_revocation`; shared `verify_token(.., revocation, ..)` uses
  `verify_in_context`; gcp/azure pass `None` (B4 parity follow-on).
- Tests: fabric-revocation store; wsf-seal snapshot-revoked-at-seal; wsf-broker
  snapshot-revoked.

## Commands + results
- `cargo fmt --check` .................................. exit 0
- `cargo check --workspace` ........................... exit 0
- `cargo clippy -p fabric-revocation -p wsf-seal -p wsf-broker --all-targets -- -D warnings -A clippy::pedantic` exit 0
- `cargo test -p fabric-revocation` ................. ok (store)
- `cargo test -p wsf-seal` .......................... ok (tenant_binding 5 incl. revoked)
- `cargo test -p wsf-broker` ........................ ok (19 incl. snapshot revoked)

## Negative controls
- `install_verifies_and_rejects_rollback_expiry_and_forgery` — older snapshot →
  Rollback; newer-but-expired → Expired; attacker-key → InvalidSignature;
  last-known-good retained each time.
- `snapshot_revoked_token_is_refused_at_seal` — seal denies a revoked token before
  Transit.
- `snapshot_revoked_token_is_refused` (broker) — refused before AWS.

## Deferred (honest)
- R2 broaden predicate (issuer/tenant/lineage); R3 continued (gateway, tool-proxy,
  approval, GCP/Azure); R4 emergency propagation + SLO + appliance snapshot poll;
  R5 HA/partition/air-gap; live R6 gate (→ PROVEN).
