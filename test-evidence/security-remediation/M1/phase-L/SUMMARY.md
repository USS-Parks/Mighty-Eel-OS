# M1 / Phase L — AF-007 receipt ledger authorization (evidence)

Prompt: L1/L2 (authenticated, tenant-scoped receipt query). Finding: AF-007 Medium.

## Root cause
`/v1/receipts` was unauthenticated, accepted an arbitrary `field=/value=` query
(enumeration oracle), and returned all entries with no tenant filter.

## Changed files
- `crates/wsf-ledger/src/lib.rs` — `query_tenant` (mandatory tenant predicate, no
  existence oracle, paged) + `query_global` (auditor).
- `crates/wsf-seal/src/lib.rs` — `SealReceipt.tenant_id` (from the token).
- `crates/wsf-api/src/lib.rs` — `/v1/receipts` gated by `require_principal`; typed
  `ReceiptsQuery { token_id, limit }`; tenant predicate from the principal;
  `global-auditor` role for cross-tenant; SDK `receipts(token_id, limit)` + identity.
- Tests: wsf-ledger `tenant_scoped_query_isolates_tenants`; wsf-api `auth_gate`
  receipts 401 + tenant-scoped.

## Commands + results
- `cargo fmt --check` .................................. exit 0
- `cargo check --workspace` ........................... exit 0
- `cargo clippy -p wsf-ledger -p wsf-seal -p wsf-api --all-targets -- -D warnings -A clippy::pedantic` exit 0
- `bash .integrity/scripts/route-policy-check.sh` .... OK (79/79)
- `cargo test -p wsf-ledger` ......................... ok (5)
- `cargo test -p wsf-seal` ........................... ok (inline 3 + tenant_binding 4)
- `cargo test -p wsf-api` ............................ ok (auth 6 + auth_gate 4 + live skip)

## Negative controls
- `receipts_without_identity_is_401` — no identity → 401 (AF-007).
- `tenant_scoped_query_isolates_tenants` — tenant-a sees only tenant-a; tenant-b
  and untenanted receipts hidden; cross-tenant token id → 0 rows (no oracle).
- `receipts_are_tenant_scoped_to_the_principal` — HTTP surface returns only the
  principal's tenant rows.
- Positive: `query_global` sees all; `limit` caps results.

## Deferred (honest)
- L3 persistent HA ledger (production still uses the in-process ledger); live L4
  two-tenant ingest/query/export gate (→ PROVEN).
