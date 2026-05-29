# RC1 Track C Self-Review

> **STATUS — CLOSED (2026-05-24)**
> Self-review delivered 12 findings (5H/4M/3L); 9 resolved in the RC-10 RC1.1-docs pass (commit `b0fcdee`), 3 deferred. Self-review work fed into the DOUGHERTY remediation lane after John Dougherty's external review superseded it. Kept as literal record of the internal pre-tester check.

**Project:** Lamprey MAI
**Release:** RC1 v2 (Tester Bundle — source + binaries)
**Freeze commit:** `dceaabc` (SHIP-17 hotfix on `main`)
**Reviewer:** Claude (co-author on the build effort)
**Date of review:** 2026-05-24
**Bundle reviewed:** `C:/Users/17076/Documents/Claude/Island-Mountain-RC1-self-review/Lamprey-MAI-RC1/` (extracted from `Lamprey-MAI-RC1.3.zip` sha256 `9a2f95ee…`)
**Plan reference:** `docs/COGENT-DEPLOYMENT-ROADMAP.md` Session RC-09
**Track:** C (security / compliance review) per `docs/TESTER-INSTRUCTIONS.md` §4.C

---

## 0. Status — This Is Not Outside-Tester Evidence

This document is a **self-review** performed by Claude against the
extracted RC1 v2 bundle on the build host. It is parallel to the
RC-06 fresh-machine rehearsal (which exercised Track A + Track B as
a self-test). It exists to:

- Catch anything an outside Track C reviewer would hit before they
  hit it.
- Exercise the full Track C reading list against the bundle exactly
  as a recipient would see it.
- Produce a findings memo with the same shape an outside reviewer's
  output would take, so the §7 triage matrix in
  `RC1-TESTER-FEEDBACK.md` can be exercised end-to-end before the
  real tester replies.

**This review does not close RC-09.** RC-09's acceptance criterion
is "at least one person besides the original builder has tried
RC1." Claude was a co-author on every session in the build lane;
this self-review is the builder reviewing their own work. The
findings here are still real — and several are High-severity that
would block an outside reviewer — but the outside-tester slot in
`RC1-TESTER-FEEDBACK.md` §6.1 remains open.

Per the project's test-evidence-literalism rule
(`feedback_test_evidence_literalism`), §10 names everything that
was not exercised in this pass.

---

## 1. Scope Reviewed

### 1.1 Execution

| Step | Command | Result |
|---|---|---|
| Bundle integrity | `sha256sum -c CHECKSUMS.txt` | 667/667 OK, 0 FAILED |
| Track A binary path | `bin/lamprey-mai-api.exe` cold boot, `curl /v1/health` | Boot 76 ms, HTTP 200, `"status":"healthy"`, `"air_gap_status":"compliant"` |
| Track B subset | `cargo test -p mai-compliance --test compliance_demos` | 6 passed / 0 failed (build 1m28s cold, test 0.32s) |
| Track B perf | `cargo test -p mai-compliance --test compliance_perf --release -- --nocapture` | 3 passed / 0 failed — composer P99 **300 ns**, audit **119 494/s**, report **1.687 ms** |

### 1.2 Documents read end-to-end

- `source/docs/acquisition/ARCHITECTURE.md` (350 lines)
- `source/docs/acquisition/demos/healthcare.md` (197 lines)
- `source/docs/acquisition/demos/defense.md` (243 lines)
- `source/docs/acquisition/demos/tribal.md` (223 lines)
- `source/docs/acquisition/demos/multi-domain.md` (286 lines)
- `source/mai-compliance/tests/compliance_demos.rs` (509 lines — the
  six test bodies that back the four demo narratives plus audit
  tamper + Trust Manifold)
- `source/mai-compliance/tests/compliance_perf.rs` (229 lines)
- `source/docs/runbooks/05-verify-audit-chain.md`
- `source/docs/runbooks/06-generate-compliance-report.md`
- `source/docs/runbooks/11-trust-bundle-expired.md`
- `source/docs/runbooks/12-audit-wal-tamper.md`
- `source/docs/runbooks/13-air-gap-violation.md`
- `source/docs/RC1-FREEZE-NOTES.md` §4 "What RC1 Excludes"

### 1.3 Cross-reference checks

- All twelve filename cross-refs from
  `ARCHITECTURE.md` §"What this document is not" → resolved.
- All inter-runbook cross-refs (04→05, 05↔12, 11→04, 12→08,
  13→INCIDENT-RESPONSE) → resolved with current numbering.

---

## 2. Findings — Quick Index

| ID | Severity | Bucket | Subject |
|---|---|---|---|
| **H-1** | High | docs | `mai-admin` runbook commands not implemented |
| **H-2** | High | docs | Acquisition demos reference a `mai` CLI that does not exist |
| **H-3** | High | docs | Acquisition demos cite wrong API port (8080 vs actual 8420) |
| **H-4** | High | docs | Acquisition demos hardcode the builder's workspace path |
| **H-5** | High | docs | `TESTER-INSTRUCTIONS.md` §4.C cites all five runbook numbers wrong |
| **M-1** | Medium | docs | `TESTER-INSTRUCTIONS.md` §4.C step 2 references three layer docs that do not exist |
| **M-2** | Medium | docs or code | `README-FIRST.md` §5.C documents logs on stderr; runtime puts them on stdout |
| **M-3** | Medium | docs | Demos prescribe `cargo run --release` instead of the bundled binary |
| **M-4** | Medium | docs | Runbooks describe Linux production; RC1 bundle is Windows tester-only — gap is implicit |
| **L-1** | Low | docs | Em-dash vs hyphen mismatch in README-FIRST.md §5.C "MAI server ready" string |
| **L-2** | Low | docs | `ARCHITECTURE.md` §"Source-of-truth navigation" prefixes `mai/` on paths that are bare inside the bundle |
| **L-3** | Low | code | Health endpoint reports `"gpus":[]` while topology log reports `gpus=1` |

---

## 3. High Severity Findings

### H-1. `mai-admin` runbook commands are not implemented at the freeze

**Files:**
- `source/tools/mai-admin/src/main.rs:36-50` (the `Command` enum)
- `source/tools/mai-admin/src/main.rs:1-7` (header comment)
- Referenced from: `docs/runbooks/05-verify-audit-chain.md:23-47`,
  `06-generate-compliance-report.md:25-65`,
  `11-trust-bundle-expired.md:53-56`,
  `12-audit-wal-tamper.md:41-46`,
  `13-air-gap-violation.md:24-29`

**What I saw:** `mai-admin/src/main.rs:36-50` declares the top-level
`Command` enum with `Backup` and `Restore` as subcommand groups
(with full structs) and `Audit`, `Trust`, `Vault` as bare variants.
The header comment at lines 1-7 explicitly says: "SHIP-09 wires
`backup create` and `backup verify`. SHIP-10 adds `restore plan`
and `restore apply`. **The remaining `audit`, `trust`, and `vault`
subcommands ship in later sessions and stub here with a clear
exit-with-message** so the operator UX of `mai-admin --help`
reflects the whole roadmap."

The runbooks instruct an operator to run:
- `mai-admin audit verify --wal-dir … --anchors …` (runbook 05, 12)
- `mai-admin compliance report --start … --end … --sign` (runbook 06)
- `mai-admin compliance verify <report> --signature …` (runbook 06)
- `mai-admin policy inspect <bundle>.cbor` (runbook 11)
- `mai-admin audit tail --grep airgap.violation` (runbook 13)

None of these subcommands are implemented at `dceaabc`. The `audit`
top-level variant is a stub (per source comment); the `compliance`
and `policy` top-level variants are not even declared as stubs.

**What I expected:** Either the runbooks describe RC2+ posture and
say so explicitly, or the freeze includes a working implementation
of the commands the operator is told to run.

**Why it matters:** A Track C reviewer following the runbooks against
the freeze sees five distinct dead ends in the operator surface.
Even if every command's intent is correct, the runbooks read as
specification rather than as runnable procedure.

**Suggested fix in RC-10:** Add a header band to each affected
runbook ("This command requires `mai-admin` from the SHIP-XX
session. The RC1 freeze ships the `mai-admin backup` and
`mai-admin restore` surfaces; the `audit`, `trust`, `vault`,
`compliance`, and `policy` surfaces are stubbed and ship in
post-RC1 sessions") and a top-level note in `runbooks/README.md`.
Cheaper than re-implementing the surfaces in the RC1 cut.

### H-2. Acquisition demos reference a `mai` CLI that does not exist

**Files:**
- `docs/acquisition/demos/healthcare.md:49,76,125,144,156`
- `docs/acquisition/demos/defense.md:50,81,158,172,180,195,199`
- `docs/acquisition/demos/tribal.md:49,77,93,147,156,166,175`
- `docs/acquisition/demos/multi-domain.md:63,75,141,206,231`

**What I saw:** All four acquisition demo narratives use a
hypothetical `mai` CLI:
- `mai chat "<prompt>" --model lamprey/medical-local --metadata …`
- `mai compliance status`
- `mai compliance audit --tenant <t> --module <m> --limit 5`
- `mai compliance report generate HIPAA --scope tenant=local-dev`
- `mai compliance report download <id> --format json --out …`
- `mai scheduler instance-metrics ranger-eu-001`

The bundle ships exactly three binaries:
- `bin/lamprey-mai-api.exe` (the daemon)
- `bin/lamprey-mai-ship-validate.exe` (the ship-profile validator)
- `tools/mai-admin/` (build-from-source CLI, with the stub
  limitations documented in H-1)

There is no `mai` binary anywhere in the bundle. The chat surface
itself exists (REST `POST /v1/chat/completions`, gRPC
`ChatCompletion`), but no CLI wraps it.

**What I expected:** Either the demos use direct HTTP calls
(curl + JSON payloads) or the bundle ships a `mai` CLI.

**Why it matters:** An acquirer reviewer running the demos will
block at the first interactive command in every demo. The
narrative becomes "the script doesn't work" rather than "here is
what the policy stack decides." This is the most acquirer-facing
documentation in the entire bundle.

**Suggested fix in RC-10:** Rewrite each demo's "Setup script" and
"Verification steps" to use `curl` against the REST API (port
8420) with the first-boot admin key. Or, if a real `mai` CLI is
intended for RC2, add a clear header to each demo: "These
commands assume a CLI that ships in RC2. The RC1 equivalent path
is to invoke the REST endpoints directly — see
`docs/API-REFERENCE.md`."

### H-3. Acquisition demos cite wrong API port

**Files:**
- `docs/acquisition/demos/healthcare.md:45,137`
- `docs/acquisition/demos/defense.md:47,196`
- `docs/acquisition/demos/tribal.md:46`
- `docs/acquisition/demos/multi-domain.md:60,239`

**What I saw:** Demos set `MAI_API_BASE = "http://127.0.0.1:8080"`
and curl `http://127.0.0.1:8080/v1/...`. The multi-domain demo
points at dashboard `http://127.0.0.1:8081/alerts`.

The actual daemon, verified during Track A self-test, binds:
- REST: `127.0.0.1:8420`
- gRPC: `127.0.0.1:8421`

`README-FIRST.md` §5.D correctly uses `:8420`. The demos diverge.

**What I expected:** Consistent port across README-FIRST and demos.

**Why it matters:** Anyone copy-pasting a demo `curl` will see
"connection refused" on first try. Compounds with H-2: the
visible failure mode is "CLI not found"; the failure mode after a
reviewer reverses-engineers to direct HTTP is "wrong port".

**Suggested fix in RC-10:** Global s/8080/8420/ across the four
demo files. Confirm dashboard port separately.

### H-4. Acquisition demos hardcode the builder's workspace path

**Files:**
- `docs/acquisition/demos/healthcare.md:39`
- `docs/acquisition/demos/defense.md:41`
- `docs/acquisition/demos/tribal.md:40`
- `docs/acquisition/demos/multi-domain.md:54`

**What I saw:** First command in every demo:
```
cd "$env:USERPROFILE\Documents\Claude\Island Mountain Mighty Eel OS\mai"
```

This is the build host's path. A tester unpacking the RC1 bundle
to `~/Lamprey-MAI-RC1/` or any other location fails immediately —
the `cd` either silently puts them in their own user's empty
directory or errors.

**What I expected:** A relative path (e.g., `cd source/`) or a
documented env var (`MAI_BUNDLE_ROOT`).

**Why it matters:** Composes with H-2 and H-3 to make all four
demos non-runnable as written. A reviewer with the discipline to
adapt would get past it; a reviewer being shown the bundle to
form a buy-vs-build opinion would conclude the demos were never
tested.

**Suggested fix in RC-10:** Replace each `cd` with `cd source` (the
bundle convention) and add a pre-flight note: "Assumes the
bundle was unpacked at `Lamprey-MAI-RC1/`; all paths are relative
to its `source/`."

### H-5. TESTER-INSTRUCTIONS.md §4.C cites all five runbook numbers wrong

**File:** `docs/TESTER-INSTRUCTIONS.md:182-187` (lines from the
workspace-side file; bundle-side mirror at
`source/docs/TESTER-INSTRUCTIONS.md`)

**What I saw:** TESTER-INSTRUCTIONS.md §4.C step 4 asks Track C
reviewers to read these runbooks by number:
```
`04-verify-audit-chain.md`, `05-generate-compliance-report.md`,
`09-trust-bundle-expired.md`, `10-audit-wal-tamper.md`,
`11-air-gap-violation.md`.
```

The actual files in `source/docs/runbooks/`:
- `04-install-policy-bundle.md`
- `05-verify-audit-chain.md`
- `06-generate-compliance-report.md`
- `09-recover-from-failed-upgrade.md`
- `10-adapter-crash-loop.md`
- `11-trust-bundle-expired.md`
- `12-audit-wal-tamper.md`
- `13-air-gap-violation.md`

Every single number TESTER-INSTRUCTIONS cites resolves to the
wrong file. A reviewer opening `04-` expecting audit-chain
verification reads policy-bundle install; `09-` expecting
trust-bundle expiry reads upgrade recovery; and so on.

**What I expected:** Numbers consistent with the actual files in
`source/docs/runbooks/`.

**Why it matters:** TESTER-INSTRUCTIONS is the doc Track C testers
follow to budget their time. They open the wrong files, get the
wrong content, and waste the front part of their review window
realigning. Self-inflicted; this is the most embarrassing of the
H-class findings because the author wrote both files.

**Suggested fix in RC-10:** Update the line in TESTER-INSTRUCTIONS
§4.C to the correct numbering:
```
`05-verify-audit-chain.md`, `06-generate-compliance-report.md`,
`11-trust-bundle-expired.md`, `12-audit-wal-tamper.md`,
`13-air-gap-violation.md`.
```

---

## 4. Medium Severity Findings

### M-1. TESTER-INSTRUCTIONS.md §4.C step 2 references nonexistent layer docs

**File:** `docs/TESTER-INSTRUCTIONS.md:180`

**What I saw:** "Read the architecture overview:
`source/docs/acquisition/ARCHITECTURE.md` and the three layer docs
under `source/docs/acquisition/` (router, policy, audit)."

`source/docs/acquisition/` contains only:
`ARCHITECTURE.md`, `COMPETITIVE.md`, `INTEGRATION.md`, `IP.md`,
`READY.md`, plus `demos/`. No separate router/policy/audit files.
The three layers are described inside `ARCHITECTURE.md` itself
(§"System diagram" lines 35-52, then the
§"Source-of-truth navigation" table lines 300-323).

**Why it matters:** Reviewer searches for files that do not exist,
then has to figure out the layers are inline in ARCHITECTURE.md.
Compounds with H-5 to make the reviewer suspicious that the
TESTER-INSTRUCTIONS author hadn't opened the bundle they were
describing.

**Suggested fix in RC-10:** Rewrite the bullet to: "Read the
architecture overview: `source/docs/acquisition/ARCHITECTURE.md`
— the three layer descriptions (router, policy, audit) are inline
in its §'System diagram' and §'Source-of-truth navigation'."

### M-2. Logs documented on stderr, observed on stdout

**Files:**
- `docs/README-FIRST.md:157-158`
- vs runtime behavior of `bin/lamprey-mai-api.exe`

**What I saw:** Track A smoke test ran
`bin/lamprey-mai-api.exe > stdout.log 2> stderr.log`. After daemon was
ready, `stderr.log` was empty; `stdout.log` contained every JSON
log line plus the boxed first-boot banner.

README-FIRST.md §5.C says: "the daemon emits a JSON-formatted
info log stream on stderr and a single banner block on stdout."

**Why it matters:** A tester configuring journald, Windows Event
Forwarding, or any log-routing per the documented contract loses
all logs. A tester filtering stdout for just the banner gets
banner + all log lines mixed.

**Suggested fix in RC-10:** Decide whether the contract is "logs
on stderr, banner on stdout" (then patch the tracing-subscriber
config in `mai-api/src/server.rs` or wherever it lives — the
runtime is the bug) or "everything on stdout" (then patch
README-FIRST.md §5.C to match). The "logs on stderr" contract is
the conventional one and the one referenced by the SHIP-15
runbooks; defaulting toward fixing the runtime is probably right.

### M-3. Demos prescribe `cargo run --release` instead of the bundled binary

**Files:**
- `docs/acquisition/demos/healthcare.md:42`
- `docs/acquisition/demos/defense.md:44`
- `docs/acquisition/demos/tribal.md:43`
- `docs/acquisition/demos/multi-domain.md:57`

**What I saw:** Every demo's setup script ends with
`cargo run --release --bin mai-api`. RC1 v2 ships pre-built
`bin/lamprey-mai-api.exe`; the demos do not mention it.

**Why it matters:** Adds ~3 minutes of cold release build to every
demo on first run (verified against this self-review's 1m56s
release `compliance_perf` build time, which is a smaller
target). Wastes the demo's "ten minutes end to end" budget.

**Suggested fix in RC-10:** Replace `cargo run --release …` with:
"Use the bundled binary if you took the RC1 v2 path:
`..\bin\lamprey-mai-api.exe` (Windows) or `../bin/mai-api` (Unix); or
build from source: `cargo run --release --bin mai-api`."

### M-4. Runbooks describe Linux production; bundle is Windows tester

**Files:** all five Track-C runbooks (05, 06, 11, 12, 13)

**What I saw:** Every runbook uses `sudo systemctl`,
`/var/lib/mai/audit`, `/etc/mai/trust-anchors`, `/var/backups/mai`,
`smartctl`, `dmesg`, `journalctl`. RC1 v2 binaries are Windows
MSVC only. A Track C reviewer running on Windows cannot execute
any runbook step on their tester machine.

This is **consistent** if the runbooks are understood as "production
deployment posture" — which is what they are. But the gap is
implicit: nothing in the runbooks or in README-FIRST says
explicitly "these runbooks describe the Linux production
deployment; the RC1 bundle ships Windows binaries because it is a
tester bundle, not a production install."

**Why it matters:** A Track C reviewer flips between the
Windows-on-laptop tester experience and the Linux-on-appliance
runbook content with no signposting. They will either spend time
trying to make the runbooks work on their tester host (waste) or
form a partial impression that the product is Linux-only without
realizing the Windows binaries are tester convenience only.

**Suggested fix in RC-10:** Add one paragraph to
`source/docs/runbooks/README.md` (or create one if missing):
"These runbooks describe the Linux systemd production deployment.
The RC1 v2 tester bundle ships Windows MSVC binaries because the
RC1 audience is laptop testers, not appliance operators. Linux
appliance binaries arrive in RC2; until then, treat these runbooks
as design documentation rather than as procedures executable on
your tester machine."

---

## 5. Low Severity / Informational

### L-1. Em-dash vs hyphen mismatch in "MAI server ready" string

`README-FIRST.md:175` shows
`MAI server ready - REST on 127.0.0.1:8420, gRPC on 127.0.0.1:8421`
with a hyphen. Actual runtime emits an em-dash: `MAI server ready
— REST on …`. Cosmetic for humans; would break exact-string-match
tooling.

### L-2. ARCHITECTURE.md prefixes `mai/` on paths that are bare in the bundle

`docs/acquisition/ARCHITECTURE.md:318` references
`mai/compliance-dashboard/`, `mai/deployment/{...}`. Inside the
bundle, `source/` IS the mai workspace; the path is just
`compliance-dashboard/` or `deployment/<profile>/`. A reviewer
adjusts mentally; minor.

### L-3. Topology says gpus=1; health says gpus=[]

During Track A boot, the log stream showed `nvidia-smi unavailable,
using flat topology` followed by `GPU topology loaded gpus=1
nvlink_cliques=0`. The `/v1/health` response on the same boot
returned `"hardware":{"gpus":[],"air_gap_status":"compliant"}`.

The discrepancy is probably intentional (scheduler's flat-topology
fallback synthesises a placeholder GPU for placement math; the
hardware-health endpoint reports actual queryable devices, of
which there are none). But it presents as inconsistent in the
smoke output. Worth either documenting or unifying.

---

## 6. Trust Manifold Anchor Review (Track C narrative anchor #1)

Per TESTER-INSTRUCTIONS.md §4.C, the
`test_trust_manifold_disconnected_and_expired` test is one of the
two "most-load-bearing" demos. Reviewed:

- **6a Disconnected path.** Sets `connectivity = AirGapped`,
  asserts `offline_mode() == true`, runs ITAR jurisdiction
  evaluation, records audit. Key assertion at lines 475-478:
  `entry_offline.correlation.trust_bundle_version` matches
  `bundle_offline.trust.trust_bundle_version`. **The audit
  correlation field is correctly carried even in offline mode.**
- **6b Expired / Unknown-revocation path.** Sets
  `revocation_status = Unknown` plus an ancient
  `trust_bundle_version`. Asserts ITAR evaluator returns
  `Outcome::DenyExport` at line 488-493 with the explicit
  rationale: "ITAR with Unknown revocation must fail closed".
  **Fail-closed semantics verified directly.**
- **Report generation in degraded mode** at lines 502-507: even
  with stale trust, a certified ITAR report still generates and
  carries `content_hash_hex`. **The TrustSection captures the
  expired bundle version for the regulator.**

This is a strong test. The Trust Manifold dry-run isn't a smoke
check — it directly asserts the two regulatory properties
(fail-closed on unknown revocation; trust state in the audit
chain even when offline) that a security reviewer cares about.

---

## 7. Audit Tamper Anchor Review (Track C narrative anchor #2)

`test_audit_tamper` (compliance_demos.rs:394-445):

- Records three benign decisions to build a non-trivial chain.
- Verifies the unmodified chain via
  `verify_chain::<MlDsaBundleVerifier>` (line 419-420).
- **Tampers entry #2** by rewriting `routing_reason = "TAMPERED"`
  (line 426).
- Re-verifies and asserts the failure shape is exactly
  `ChainError::LinkBroken` (line 429-432). Because the chain link
  from #2 to #3 is `previous_hash = content_hash(#2)`, mutating
  #2 invalidates #3's link — exactly the property a regulator
  needs to be able to demonstrate.
- Recording the chain break via the public API at line 436-444
  surfaces a `Severity::Critical` escalation — the
  dashboard/SIEM-bridge surface picks it up.

The test directly demonstrates the property runbook 12 promises:
"Any mismatch means corruption, tampering, or a missing range.
None of those are silent." Strong test.

---

## 8. Positive Observations

These are the things the bundle does right that a reviewer should
not have to dig for:

- **Bundle integrity perfect after archive round-trip.** 667/667
  files matched CHECKSUMS.txt after a zip extract — no extraction
  layer corruption.
- **Boot speed 76 ms** vs README-FIRST.md's "about 60 ms" — within
  the published tolerance on a 4-core laptop.
- **Compliance demos pass cold on the extracted bundle** in
  1m28s + 0.32s.
- **Performance numbers comfortably under budget:** composer P99
  300 ns (budget 5 ms — 16 666× headroom), audit 119 494/s (budget
  1000/s — 119× headroom), report 1.687 ms (budget 10 s — 5926×
  headroom). The headroom is what an acquirer's perf team will
  appreciate.
- **All 12 ARCHITECTURE.md cross-references resolve.** Internal doc
  consistency is high once you get past the demos and
  TESTER-INSTRUCTIONS.
- **Runbooks 05/06/11/12/13 are operationally consistent** with the
  test suite: runbook 12's "any mismatch" contract is exactly what
  `test_audit_tamper` proves; runbook 11's bundle-expiry
  fail-closed is exactly what
  `test_trust_manifold_disconnected_and_expired` proves;
  runbook 13's air-gap-violation surface lines up with the
  `airgap_status` field in `/v1/health` and the
  `ConnectivityState::AirGapped` path the Trust Manifold test
  exercises.
- **The compliance demo tests are well-named, self-contained, and
  deterministic** (per-test atomic clocks, isolated audit logs,
  no shared state). A reader can run any single test in isolation
  and reproduce it.

---

## 9. Severity-Adjusted Summary For The Triage Matrix

If only RC-10 doc-patch capacity is available, fix in priority
order:

1. **H-5** (TESTER-INSTRUCTIONS runbook numbers) — five
   one-character edits in one file. Cheapest blocker to clear.
2. **H-4** (demo workspace paths) — four files, one `cd` line each.
3. **H-3** (demo port 8080 → 8420) — global find/replace across the
   same four files.
4. **H-2** (demos reference nonexistent `mai` CLI) — bigger rewrite;
   alternative is the deferring header band.
5. **H-1** (runbook commands not implemented) — header band per
   runbook is cheapest; rewriting commands to use what's actually
   built is bigger.
6. **M-1** (TESTER-INSTRUCTIONS layer-doc reference) — one bullet
   rewrite.
7. **M-2** (logs stream channel) — needs a decision on contract vs
   runtime fix.
8. **M-3** (demos build from source) — append a "use bundled
   binary" note in each demo.
9. **M-4** (runbook OS gap) — one paragraph in runbooks/README.md.
10. L-1, L-2, L-3 — discretionary.

H-1 through H-4 are all in the most acquirer-facing material
(`docs/acquisition/demos/`). H-5 and M-1 are in the doc Track C
testers use to budget their time. If RC1 is going to an acquirer
reviewer before all H-class are fixed, the cover note should warn
them explicitly that the demo narratives are out of sync with the
shipped CLI surface.

---

## 10. Did Not Exercise (Per Literalism Rule)

This list is load-bearing. Anything below was not exercised in
this self-review; do not infer that it works because the
self-review did not flag it.

- **Full `cargo test --workspace`.** Relied on RC-05 evidence
  (1539 tests pass at freeze).
- **Python SDK tests.** RC-05 covered (94 pass).
- **Dashboard tests.** RC-05 covered (20 pass).
- **App scaffold tests** (`apps/<scaffold>/tests/`). RC-05 covered
  (61 across six scaffolds).
- **`lamprey-mai-ship-validate.exe` invocation.** Binary is present, hash
  verified; I did not run it. Per H-1 the runbook 11 reference to
  `PROD-TRUST-100` is therefore unverified.
- **gRPC surface (port 8421).** Daemon binds it; I only hit REST.
- **Live policy bundle install** (runbook 04). No test bundle in
  the package.
- **72-hour burn-in driver.** Explicitly out of RC1 scope per
  RC-05.
- **Any GPU / CUDA path.** Build host has no NVIDIA GPU; the
  flat-topology fallback was exercised once during Track A.
- **Linux glibc target.** Bundle is Windows MSVC only.
- **`mai-admin backup` / `mai-admin restore` end-to-end.** Not in
  Track C reading list.
- **Source-build path for `mai-api`.** Used pre-built binary.
- **`tar.gz` archive variant.** Extracted from the `.zip` only;
  `.tar.gz` integrity was hashed at RC-08 close but not
  round-tripped here.
- **Cross-host transfer.** Sibling path on the same Windows user
  profile.
- **Each demo's interactive narrative** (HIPAA chat, ITAR chat,
  OCAP chat, multi-domain chat). H-2 blocks these without
  manual REST translation, which I did not perform.
- **Compliance dashboard live SSE** (port 8081 per multi-domain.md
  §"Verification steps" 4). Compounds with H-3.
- **Outside reviewer's perspective.** The reviewer is Claude, a
  co-author. The whole point of RC-09 is that someone else looks.
  This self-review does not substitute.

---

## 11. Acceptance Against §4.C Track C Steps

| Step | Status |
|---|---|
| 1. Run Track A and Track B first | A done (smoke + health, §1.1); B partial (compliance_demos + compliance_perf only — full workspace test relied on RC-05) |
| 2. Read architecture overview + layer docs | ARCHITECTURE.md read (§1.2); "three layer docs" finding M-1 raised — they don't exist as separate files |
| 3. Read six demo narratives + 2 demo code files | done (§1.2) |
| 4. Read 5+ runbooks | done (§1.2); finding H-5 raised — TESTER-INSTRUCTIONS' numbering was wrong, read the correct runbooks anyway |
| 5. Read RC1-FREEZE-NOTES §"Intentionally Excluded" | done (§4 of that doc, "What RC1 Excludes") |
| 6. Produce findings memo | this document |

**Track C reading list executed. RC-09 acceptance still NOT met:**
an outside reviewer remains the blocker.
