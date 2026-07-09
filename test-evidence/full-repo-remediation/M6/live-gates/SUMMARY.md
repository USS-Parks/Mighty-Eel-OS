# M6 / X2 — WSF Live-OpenBao Gate (no-mock closure): live run at the converged tip — RED

**Date (UTC):** 2026-07-09 04:49
**Repo state at run:** `50399c56e9d84524061ceecf8c5783b0de6d0725` (`session/AUDIT-FIX-2` == `origin/main` tip; clean tree)
**Plan context:** `docs/audits/2026-07-08-full-repo/REMEDIATION-PSPR.md` Phase X prompt **X2**
("full live suite; archive logs + versions; gate: zero mock-only trust closure"), milestone M6.
This record covers the **OpenBao + Moto leg** (the CI `wsf-live` set). The X2 `>=3-node harness`
leg (CI `loom-live`) is a separate record and was **also red in CI** at this tip
(`loom-harness-cp3-1` unhealthy) — not covered here.

## What ran (exact commands + exit codes)

1. `bash deployment/live-integration/run-live-suite.sh` (fresh Dockerized OpenBao + Moto) -> **exit 1** (13/15 PASS)
2. `cargo test -p aogd --test live_openbao_anchor -- --nocapture` -> **exit 0** (the 16th test; CI `wsf-live` parity)
3. Isolation reruns of both failures against the same live pair -> **both still FAIL** (not suite interference)

## Live services (black-box over HTTP)

See `service-versions.txt`. OpenBao v2.5.4 (dev, container digest pinned in that file), Moto latest,
rustc/cargo 1.96.1, Windows 11 + Docker 29.6.1.

## Results — 14/16 PASS, 2 FAIL

```
PASS wsf-bridge/live_openbao        PASS wsf-api/live_api
PASS wsf-broker/live_localstack     PASS aog-gateway/live_gateway
PASS wsf-broker/live_gcp            FAIL aog-gateway/kill_switch
PASS wsf-broker/live_azure          FAIL aog-gateway/openai_surface
PASS wsf-seal/live_seal             PASS aog-gateway/anthropic_surface
PASS wsf-ledger/live_ledger         PASS aog-gateway/policy_modes
PASS wsf-cache/live_cache           PASS aog-gateway/metering
PASS wsf-tenants/live_tenants       PASS aogd/live_openbao_anchor (separate run, exit 0)
LIVE SUITE HAD FAILURES (above).  SUITE_EXIT=1  AOGD_EXIT=0
```

## The two failures — hardened product contracts vs unreconciled live tests

**`aog-gateway::kill_switch` — `budget_exhaustion_and_revocation_halt_the_next_call`,
panic at `crates/aog-gateway/tests/kill_switch.rs:242`:**
`in-budget resolves: Unauthorized("revocation snapshot unavailable (fail-closed)")`.
The test wipes the revocation snapshot during provision (the M1-era rerun-safety fix) and then
asserts "no snapshot yet -> nothing revoked -> resolves" — a **fail-open-on-absent** contract.
Since `e284942` (AF-15B: complete, fail-closed, fresh revocation) the gateway **denies** when the
snapshot is absent (`crates/aog-gateway/src/lib.rs`, `OpenBaoError::NotFound` arm). The product is
doing what AF-15B intends; the test asserts the pre-hardening contract.

**`aog-gateway::openai_surface` — `openai_client_completes_chat_and_stream`,
panic at `crates/aog-gateway/tests/openai_surface.rs:348`:** expected **200**, got **403**.
The test posts a PHI payload and asserts shadow-mode semantics: 200 + `x-aog-route: local_only`
("shadow decides + logs, never blocks"). The G-lane PHI-egress hardening in this wave
(`e4ac0d6` G1 composer fail-closed on an unvetted request; `5fa22db` G3 router forces Local on a
medical entity) now yields a deny on this path.

**Why this is a real tip-state finding, not an environment artifact:**
- GitHub CI at the same tip fails identically: run `28991840496`, job `WSF Live-OpenBao Gate
  (no-mock closure)`, same panic at `kill_switch.rs:242`. CI aborts its test step there, so CI
  never reaches `openai_surface` — this local run surfaced the second failure CI cannot see yet.
- Both tests fail in isolation against a fresh live pair.
- Neither test file was touched during this wave (`kill_switch.rs` last: `5849198`;
  `openai_surface.rs` last: `fc0090d`) while the product contracts under them hardened.

## Disposition (owner call — no product or test code changed in this run)

The gate did its job: the wave's fail-closed hardening was never reconciled into the two gateway
live tests that still assert pre-hardening semantics. Per plan §0.6, a required live gate red
blocks re-ship; X2 cannot close until these green honestly (§0.5 forbids `#[ignore]`).

- `kill_switch`: reconcile to AF-15B — publish a baseline (nothing-revoked) signed snapshot before
  the "resolves" assertion, keep the revoke->deny flow, and add the new negative control
  (absent snapshot -> deny, the fail-closed contract).
- `openai_surface`: decide the intended semantic first. If G3 "force Local" is meant to *route*
  PHI locally (200 + `local_only`), the 403 is a mode-resolution defect in the G1/G3 path worth a
  product look; if PHI-on-an-unvetted-request is meant to deny even in shadow, reconcile the test
  to assert the deny + receipt.

## Redaction

No cloud-credential material in the run logs (AKIA/ASIA grep: 0 hits). Nothing redacted.

## Files

- `service-versions.txt` — service/toolchain/endpoint fingerprint (image digests pinned)
- `live-suite-run.log`, `aogd-anchor-run.log` — full verbatim runner output; **local artifacts
  only** (`*.log` is gitignored repo-wide); key lines embedded verbatim above. Rerun:
  `bash deployment/live-integration/run-live-suite.sh` (fresh pair) or `SKIP_DOCKER=1` to reuse.

---

## Follow-up (2026-07-09): tests reconciled — gate GREEN 16/16

The two unreconciled tests were fixed (test code only; no product change) and the full gate
re-run against a fresh OpenBao + Moto pair (same image digests as `service-versions.txt`):

- `crates/aog-gateway/tests/kill_switch.rs` — now asserts the AF-15B fail-closed contract live
  (absent snapshot -> Unauthorized deny) as a new negative control, then publishes a baseline
  nothing-revoked snapshot (sequence 1; the revoking snapshot advances to 2) before the
  budget/revocation flow. Strictly more coverage than the pre-hardening version.
- `crates/aog-gateway/tests/openai_surface.rs` — the PHI case now asserts the enforce-default
  contract: **403** with the `policy_denied` / `aog_enforce` error body and
  `x-aog-policy: deny` + `x-aog-policy-blocked: true` headers. Shadow/report tag-don't-block
  semantics remain the `policy_modes` gate's coverage.

Verify: `cargo fmt -p aog-gateway` no-op; `cargo clippy -p aog-gateway --tests -- -D warnings
-A clippy::pedantic` clean; both tests green in isolation, then the full suite + anchor:
**15/15 PASS + aogd anchor PASS = 16/16, "LIVE SUITE GREEN"** (`SUITE_EXIT=0`, `AOGD_EXIT=0`).
Runs: `live-suite-run-2-green.log`, `aogd-anchor-run-2.log` (local-only, gitignored).

Repo state: `50399c5` + the two test files uncommitted (commit pending owner approval; SHA to be
recorded on commit). **X2 leg 1 (OpenBao + Moto): GREEN.** The `loom-live` >=3-node leg remains
the outstanding X2 item.
