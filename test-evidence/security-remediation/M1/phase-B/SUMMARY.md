# M1 / Phase B — AF-004 credential broker confinement (evidence)

Prompt: B1–B3 (named grant contract + server-side grant policy + AWS least
privilege). Finding: AF-004 High.

## Root cause
`broker_credentials(token, .., role_arn, ..)` assumed whatever role ARN the caller
named; only the session-policy resources were scoped, never the role. The exchange
DTO carried a raw `role_arn`.

## Changed files
- `crates/wsf-broker/src/error.rs` — `GrantDenied`.
- `crates/wsf-broker/src/lib.rs` — `GrantMapping` + `GrantPolicy`;
  `AwsStsBroker.with_grants`; `broker_credentials(.., grant_id, ..)` resolves the
  grant (tenant-checked) before AWS; `build_session_policy(grant, token)`.
- `crates/wsf-api/src/lib.rs` — `ExchangeReq.role_arn` → `grant_id`; GrantDenied →
  403.
- Tests: `wsf-broker` unit (grant denial + policy scoping); `live_localstack` +
  `live_api` migrated to the grant model.

## Commands + results
- `cargo fmt --check` .................................. exit 0
- `cargo check --workspace` ........................... exit 0
- `cargo clippy -p wsf-broker -p wsf-api --all-targets -- -D warnings -A clippy::pedantic` exit 0
- `cargo test -p wsf-broker` ......................... ok (18 + live skips)

## Negative controls
- `unknown_grant_is_denied` — empty policy, any grant → `GrantDenied` before AWS.
- `cross_tenant_grant_is_denied` — tenant-a token vs tenant-b grant → `GrantDenied`.
- `session_policy_denies_all_when_grant_has_no_resources` — fail-closed deny-all.
- Positive: `session_policy_scopes_to_the_grant` (grant actions+resources) and
  `token_caveat_narrows_the_grant` (a caveat only shrinks the grant).

## Deferred (honest)
- B2 signed/OpenBao-custodied grant loading (broker starts empty ⇒ exchanges
  fail closed until grants wired); B4 GCP/Azure named-grant parity; B5 credential
  zeroization/receipt-scrub audit; live B6 Moto/GCP/Azure gate (→ PROVEN).
