# R8 — Streaming budget metering (X4/X5 close-out, milestone M7)

**Objective.** Close the streaming budget bypass (revalidation finding R8 / D8, invariant A8):
a streamed call must accrue budget spend and be receipted, and a budgeted key must be refused
once its cap is crossed on the stream path.

**Source.** `X4-X5-REVALIDATION-REPORT.md` §R8 (audit run at `803e85e`); roster
`PLANNING/X4-X5-CLOSEOUT-PSPR.md` Phase R, prompt R8. Base for this change: `1cf766b`.

## Pre-change proof (static)

At the base commit both SSE branches returned without metering — the audit pinned it and it
was re-verified from source before the change:

- `crates/aog-gateway/src/surface_openai.rs` stream arm: `Ok(chunks) => chat_sse(inbound_model, chunks)`
  — no `meter::record`, no `record_spend` (non-stream arm has both).
- `crates/aog-gateway/src/surface_anthropic.rs` stream arm: `Ok(chunks) => messages_sse(inbound_model, chunks)`
  — same gap.
- `crates/aog-gateway/src/meter.rs` module doc conceded it: "Metering the streamed path from
  its terminal usage frame is a follow-on; every non-stream call is metered."

Effect: a budgeted virtual key could stream past its cap indefinitely — `record_spend` never
ran, so the pre-flight fold in `Gateway::resolve_and_check` never saw streamed usage.

## Change

`StreamMeter` (new, `crates/aog-gateway/src/meter.rs`): an accounting guard owned by the SSE
generator. Every frame passes through `observe()` (provider-reported usage merged per-field —
Anthropic splits input onto `message_start` and output onto `message_delta`; OpenAI reports
both on one terminal frame — and delta text accrues toward a fallback estimate). Settlement
(receipt append + `record_spend` keyed by `fabric_token::lineage_key`, per T5) runs exactly
once in `Drop` — so a clean `[DONE]`, a mid-stream provider error, and a client that
disconnects early all meter alike; an early hang-up cannot dodge the budget. A usage-silent
provider is metered from the fallbacks (request-text estimate for input, ~4 chars/token of
streamed deltas for output) rather than metering zero.

Both surfaces construct the guard in their stream arm and pass it to `chat_sse` /
`messages_sse`. Stale "does not meter" comments corrected.

**Files changed:** `crates/aog-gateway/src/{meter.rs, surface_openai.rs, surface_anthropic.rs}`,
`crates/aog-gateway/tests/metering.rs`.

## New tests

- `meter::tests::stream_meter_settles_on_drop_without_a_terminal_frame` — disconnect dodge
  closed; fallback estimates receipted; 20-token cap exhausted by the accrual.
- `meter::tests::stream_meter_prefers_reported_usage_and_merges_split_frames` — reported usage
  wins; split-frame merge; 45-cent spend at baseline gpt-4o-mini pricing; 1500-token accrual.
- `surface_openai::tests::streamed_chat_is_metered_and_accrues_spend` — end-to-end through the
  OpenAI SSE generator; receipt + accrual after body completion.
- `surface_anthropic::tests::streamed_messages_are_metered_across_split_usage_frames` —
  end-to-end through the Anthropic SSE event sequence with the split usage frames.
- `tests/metering.rs::streamed_call_accrues_spend_and_cap_refuses_next_call` — the live gate
  (below).

## Commands and exit codes

| Command | Result | Exit |
|---|---|---|
| `cargo fmt --check -p aog-gateway -p mai-vault -p mai-api` | clean | 0 |
| `cargo clippy -p aog-gateway --all-targets -- -D warnings -A clippy::pedantic` | no issues | 0 |
| `cargo test -p aog-gateway` (no OpenBao) | 68 passed, 11 suites | 0 |
| `cargo test -p aog-gateway --test metering -- --nocapture` (live) | 2 passed — R8 + G7 legs | 0 |
| `cargo test -p aog-gateway` (with live OpenBao env) | 68 passed, 11 suites | 0 |
| `cargo test --workspace` | 2273 passed, 0 failed, 8 ignored (229 suites) | 0 |
| `cargo audit` | no vulnerabilities | 0 |
| `cargo deny check advisories bans licenses` | ok / ok / ok | 0 |
| `git diff HEAD \| gitleaks stdin` (change set) | no leaks found | 0 |
| `detect-secrets scan <changed .rs>` | `results: {}` | 0 |
| `.integrity/scripts/no-slop-scan.sh all` | clean | 0 |
| `.integrity/scripts/verify-tree.sh <13 changed files>` | 13/13 passed | 0 |

## Live gate

Dockerized OpenBao (`openbao/openbao:latest`, dev mode, `http://127.0.0.1:8200`, root token),
per the CI recipe. `streamed_call_accrues_spend_and_cap_refuses_next_call` seeds a virtual key
with `token_cap = 1400`, streams one SSE completion end-to-end through
`/v1/chat/completions` (mock upstream emits deltas + a terminal usage frame of 1000 + 500,
the `stream_options.include_usage` shape), then asserts the next call on the same key is
refused. Output: `R8 live gate PASSED against http://127.0.0.1:8200 (streamed call accrued
spend; cap crossed → next call 402)`. The pre-existing G7 live gate still passes at the same
tip (no metering regression).

## Negative control observed

The second call after the cap-crossing stream returned **402 Payment Required** from the
pre-flight budget fold — the exact denial the roster gate demands ("a budgeted key is refused
once its cap is crossed on the stream path"). Additionally, the drop-settlement unit test
proves the disconnect dodge (streaming content then hanging up before `[DONE]`) still lands
in the receipt ledger and the spend ledger.

## Commit

`72d89e7` — remediation(R8): meter the streamed path and settle spend on SSE drop
(branch `session/AUDIT-FIX-2`, base `1cf766b`; approved by Basho, pushed to `origin/main`).

Note: source comments and test names carry no roster step-codes (CANON §11, enforced by the
pre-commit no-slop PROV gate); the R8 mapping lives here, in the DEVLOG, and in git history —
the in-code vocabulary is "stream-budget gate".
