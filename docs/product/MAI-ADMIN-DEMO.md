# mai-admin demo · Narrated Compliance Walk-Through

**Lane:** WELCOME-01
**Audience:** technical testers, acquirers, and reviewers running the
RC1.2 (or later) bundle for the first time
**Time required:** ~12 seconds for all 6 demos with default pacing;
~1 second with pacing disabled

`mai-admin demo` is the play-by-play companion to the headless
inference daemon. It walks through six end-to-end compliance
scenarios with phase-by-phase narration, so a viewer sees the
Lamprey policy engine's reasoning unfold in real time rather than
just the final pass/fail of `cargo test`.

The same scenarios run as integration tests in
`mai-compliance/tests/compliance_demos.rs` under CI. This subcommand
is a *literate* version of those tests: every API call is paired
with a printed phase explaining what is about to happen and what
came back. There is no shared event bus — the runner is the
narration source.

---

## Usage

```
mai-admin demo all                              # all 6, default pacing, boot banner
mai-admin demo all --no-banner                  # skip the lamprey art (CI / piping)
mai-admin demo run hipaa                        # single scenario
mai-admin demo run itar
mai-admin demo run ocap
mai-admin demo run multi                        # multi-domain (HIPAA + ITAR + OCAP at once)
mai-admin demo run tamper                       # audit-chain tamper detection
mai-admin demo run trust                        # trust-manifold offline + expired
```

Environment overrides:

- `MAI_DEMO_PACING_MS=0` — disable the 150 ms between-phase pause for
  instant playback. Useful in CI.
- `NO_COLOR=1` — force monochrome output. ANSI escapes are still
  parseable by terminals that strip them.
- `CLICOLOR_FORCE=1` — force color even when stdout is not a TTY
  (useful for piping into `less -R`).
- `COLUMNS=<n>` — override detected terminal width. Below 150 cols
  the runner falls back to a text-only mini-banner instead of the
  full lamprey art.

---

## What each scenario exercises

| Scenario | Demonstrates | Wall time (no pacing) |
|---|---|---|
| `hipaa` | PHI detection · BAA enforcement · composer · audit chain · certified HipaaAuditTrail · chain verify | ~300 ms |
| `itar` | ITAR + EAR detection · jurisdiction with non-US actor · fail-closed DenyExport · certified ItarComplianceSummary | ~200 ms |
| `ocap` | TribalData + Treaty + Cultural detectors · OCAP evaluator under Council role · certified OcapGovernance report | ~250 ms |
| `multi` | All three modules on one prompt · any-deny-wins propagation · explainability via rule codes · MonthlyDigest | ~350 ms |
| `tamper` | hash-chain LinkBroken detection · Critical-severity escalation pipeline · `verify_chain` over an in-memory mutation | ~1 ms |
| `trust` | AirGapped connectivity · offline-mode handling · Unknown revocation + ancient bundle → fail-closed | ~155 ms |

Times are measured wall-clock during the actual API calls, excluding
the inter-phase pacing. With default 150 ms pacing the human-visible
runtime is ~5-12 seconds per scenario.

---

## Sample output (single phase, monochrome)

```
  [t=  0.084ms] ▸ BAA Enforcer evaluating cloud destination
              ├─ mode               BaaMode::Standard
              ├─ phi_present        14
              └─ decision           DENY cloud — PHI present, no per-vendor BAA
```

With color (any modern terminal): phase headers in cyan, decisions
in green/red, redaction counts in yellow, audit cryptography in
magenta, timing in dim. Color detection follows the standard
conventions (TTY check + `NO_COLOR` + `TERM=dumb`).

---

## When to use this

- **First-run experience** after unpacking the RC1.2 bundle — pair
  with `bin/lamprey-mai-api.exe` + `curl http://127.0.0.1:8420/v1/health`
  for the daemon side. `mai-admin demo` shows the compliance side.
- **Acquirer / partner demo** when explaining what Lamprey actually
  does. Slow pacing + color is the default for this case.
- **CI smoke** at `MAI_DEMO_PACING_MS=0 --no-banner` — verifies the
  full compliance engine surface end-to-end in under a second.

## When not to use this

- **Performance benchmarks** — see `cargo test -p mai-compliance
  --test compliance_perf --release` for headroom measurements.
  This subcommand is narrative, not statistical.
- **Production observability** — the runner has no event bus into
  the daemon. Live request narration against `mai-api` is a future
  WELCOME-02 deliverable.

---

## Implementation note

The runner lives at `tools/mai-admin/src/demo.rs` and intentionally
mirrors the test functions in `mai-compliance/tests/compliance_demos.rs`
1:1. When those tests gain a scenario, the demo runner gains one too.
No new public API surface in `mai-compliance` — the runner only
calls the same exports the tests already use.

The lamprey boot banner is baked from `docs/assets/lamprey-banner.txt`
via `include_str!` at compile time; no asset files are read at runtime.
