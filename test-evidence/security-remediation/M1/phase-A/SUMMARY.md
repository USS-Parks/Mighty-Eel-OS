# M1 / Phase A — AF-002 issuance authorization (evidence)

Prompt: A1–A3 (WsfPrincipal + authenticator seam + principal-derived issuance).
Finding: AF-002 High.

## Root cause
`wsf-api` `issue` handler copied `tenant_id` / `subject_id` / `roles` from request
JSON into the bridge issuance call — any reachable caller minted a token for any
tenant/subject with any roles, unauthenticated.

## Changed files
- `crates/fabric-contracts/src/identity.rs` (+ `lib.rs`) — `WsfPrincipal`.
- `crates/wsf-api/src/auth.rs` (new) — `WsfAuthenticator` trait,
  `SignedIdentityAuthenticator` (verifies a signed `Identity` under the anchor,
  roles from server-side policy), `DevAuthenticator`, `DenyAllAuthenticator`.
- `crates/wsf-api/src/lib.rs` — `AppState.authenticator`; `require_principal`
  middleware over `/v1/tokens/issue`; `IssueReq` reduced to narrowing intent;
  `issue` derives identity from the principal.
- `crates/wsf-api/src/main.rs` — fail-closed authenticator wiring
  (`WSF_IDENTITY_ANCHOR_PK` required unless `WSF_DEV_AUTH`).
- `crates/wsf-api/src/client.rs` — `WsfClient::with_identity()` (x-wsf-identity).
- `crates/wsf-api/Cargo.toml` — dep `fabric-identity`.
- Tests: `crates/wsf-api/tests/auth_gate.rs` (new, offline gate proof);
  `crates/wsf-api/tests/live_api.rs` migrated to a signed-identity principal.

## Commands + results
- `cargo fmt --check` .................................. exit 0
- `cargo check --workspace` ........................... exit 0
- `cargo clippy -p wsf-api -p fabric-contracts --all-targets -- -D warnings -A clippy::pedantic` exit 0
- `cargo test -p fabric-contracts` ................... ok (5)
- `cargo test -p wsf-api` ............................ ok (auth unit 6 + auth_gate 2
  + live_api skip)
- `bash .integrity/scripts/route-policy-check.sh` .... OK (79/79)

## Negative controls
- `issue_without_identity_is_401` — POST /v1/tokens/issue with no identity → 401,
  refused before the bridge is consulted.
- `missing_identity` / `wrong_anchor_key` / `expired_identity` → `Unauthenticated`.
- `unknown_identity_gets_no_roles` — a verified identity with no role grant gets
  an empty role set (fail-closed), never caller-supplied roles.
- Positive: `issue_with_verified_principal_passes_the_gate` (past the gate → 502 at
  the dummy bridge) and `signed_identity_yields_principal_with_policy_roles`.

## Deferred (honest)
- A4 fine-grained issuance-permission matrix (self/service/admin + delegation
  depth); A5 live two-tenant OpenBao issuance gate (→ PROVEN); production mTLS
  peer-identity binding (the signed-assertion authenticator is the equally-strong
  pluggable seam §2.1 permits). `openapi.json` issue schema reconciled in Q8.
