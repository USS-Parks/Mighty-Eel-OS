# review_wsf_crypto summary

Scan `361f70b_20260715T130144Z`; repository mode; target `mai`. Resolved security guidance was read first and was empty (0 bytes); the repository threat model was then read in full. All five assigned files were read in full and receipted in `work_ledger.jsonl`.

## Candidate disposition

- `RWC-001` — deferred runtime reachability: `Ring3Cache::refresh` accepts authentic expired/lower-sequence snapshots, stamps them fresh/reachable, and authorization then uses them. Logic is confirmed; no non-test cache poll integrator exists in this snapshot.
- `RWC-002` — validated candidate: WSF API constructs `SealService` without the optional revocation store, so revoked-but-unexpired tokens can still unseal and recover plaintext.
- `RWC-003` — validated candidate: the independently reachable seal operation has the same absent revocation control and can consume Transit/signing authority for a revoked token.
- `RWC-004` — validated candidate: owner subject is cryptographically bound during seal but never authorized during unseal, enabling same-tenant cross-subject disclosure.
- `RWC-005` — validated candidate: recursive attenuation changes the immediate-parent lineage key while resetting embedded spend, allowing repeated full-budget use at arbitrary depth.

## Exact deferred proof gaps

1. `RWC-001`: identify the deployed `Ring3Cache` poll/refresh integrator and confirm whether an attacker or lagging distribution channel can replay an older signed snapshot. Repository search found only tests and downstream test composition.
2. `fabric-revocation` restart monotonicity: `MonotonicRevocationStore` is in-memory/default-only, but no non-test production constructor or persistence contract was found. Confirm whether a deployed consumer persists a high-water sequence outside this crate before promoting a separate restart rollback instance.
3. Dynamic negative controls for `RWC-002`/`003`/`004`: an OpenBao-backed exploit was not run. Static route-to-sink composition is complete; dynamic proof requires a live Transit service and two same-tenant subjects for `RWC-004`.
4. Dynamic budget proof for `RWC-005`: an end-to-end WSF attenuation plus AOG metering run was not executed. Static key evolution is exact (`R -> C -> G` yields spend keys `R`, then `C`).

## Exact suppressions

- Cross-tenant unseal is blocked before custody by `wsf-seal/src/lib.rs:385-406`, and unwrap selects the envelope tenant key at `424-432`.
- Forged or tampered envelope plaintext recovery is blocked by thread verification followed by AEAD authentication in `fabric-envelope/src/lib.rs:316-357`.
- Fabric-token fabricated-parent attenuation is blocked by `verify_in_context` in `fabric-token/src/lib.rs:452-470`; the observed preverified AOG wrapper is behind per-request token authentication.
- Snapshot signature forgery is blocked by `fabric_revocation::verify`; `RWC-001` requires replay of an authentic prior snapshot, not forging one.
