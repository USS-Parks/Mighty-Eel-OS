# RC1 Tester Feedback

**Project:** Island Mountain MAI + Lamprey
**Release:** RC1 v2 (Tester Bundle — source + binaries)
**Freeze commit:** `dceaabc` (SHIP-17 hotfix on `main`)
**Plan reference:** `docs/COGENT-DEPLOYMENT-ROADMAP.md` Session RC-09
**Companion docs:** `README-FIRST.md`, `TESTER-INSTRUCTIONS.md`, `RC1-BUNDLE-NOTES.md`

This document is the audit trail for the RC-09 outside-tester pass.
It records who was sent the bundle, what they ran, what they found,
and how each finding was triaged. It is updated as feedback arrives;
the current §3 status field is the source of truth for whether
RC-09's acceptance criteria have been met.

Per the project's test-evidence-literalism rule
(`feedback_test_evidence_literalism`), nothing in this document is
forward-looking promise — every entry is a record of something that
actually happened on a specific date with a specific tester.

---

## 1. Scope

RC-09's acceptance is "at least one person besides the original
builder has tried RC1." This document captures:

- which testers were invited and which actually ran the bundle
- their environment (per the issue form in `TESTER-INSTRUCTIONS.md` §5)
- each finding, classified by track (A/B/C) and severity
- triage into the four buckets from the roadmap (docs / packaging
  / code / environment)
- the blocker list that gates RC-10 (RC1 Fix Pass)

What this document does **not** capture:

- The author's own runs. RC-05 and RC-06 are the author's pre-flight
  test evidence; RC-09 is specifically the "someone else tried it"
  evidence.
- Speculative issues. Every entry must trace to a real run by a
  real tester.

## 2. Bundle Artefacts Available For Distribution

Re-assembled 2026-05-24 after the RC-10 RC1.1-docs patch pass
(commit `b0fcdee`). Binary freeze unchanged at `dceaabc`; binary
hashes unchanged from RC-03; only the source/docs/ tree differs
from the RC-08 assembly. **Use the hashes below when sending to a
tester — the original RC-08 hashes are stale.**

**Authoritative source for archive hashes:** the `SHA256SUMS` file
at the release directory (`Island-Mountain-RC1-release/SHA256SUMS`,
177 bytes). The hashes inline in the table below are a snapshot at
the time of this commit; if the bundle is re-rolled,
`SHA256SUMS` wins. (Embedding hashes inside the bundle hits a
classic self-reference: each rebuild changes a file inside, which
changes the archive hash, which would need a new commit, which
would change the file again. The external SHA256SUMS is the
fixed point.)

| Artefact | Size | SHA-256 (snapshot) |
|---|---|---|
| `MAI-Lamprey-RC1/` (uncompressed folder, 670 file entries) | 19 MB | per `MAI-Lamprey-RC1/CHECKSUMS.txt` (internal) |
| `MAI-Lamprey-RC1.tar.gz` | 5.7 MB | `35ada78f66f57901c1c3a438709712cbf0e8f43f60e5b8383eb2343c4a66c76a` |
| `MAI-Lamprey-RC1.zip` | 6.1 MB | `6200c1ccfcd25132e417c03f465eef474ccf35cbd9a8e063256f0089d3ccee84` |
| `SHA256SUMS` | 177 B | (covers the two archives above) |

Bundle and archives live at `C:/Users/17076/Documents/Claude/Island-Mountain-RC1-release/`
on the build host. Both archives carry the same 671 file entries
(670 in CHECKSUMS.txt + CHECKSUMS.txt itself); the zip also
includes a small number of explicit empty-directory markers, which
is the normal POSIX-tar vs PKZip metadata difference, not a content
difference.

Pick **tar.gz** for Unix recipients, **zip** for Windows recipients
who do not have a tar implementation.

**Self-reference note for testers reading this inside the bundle:**
If you opened this doc *inside* the archive you just downloaded,
the snapshot hashes in the table above were the latest as of the
commit that built your archive. The archive you actually downloaded
may carry slightly different file hashes inside (this doc was the
*last* thing updated before the archive was rolled). To verify
your download integrity, compare your `sha256sum` against the
external `SHA256SUMS` file your sender provided alongside the
archive — that file is the contract.

**Delta from the RC-08 assembly:** 3 new docs added to
`source/docs/` (RC1-CHANGES.md, RC1-SELF-REVIEW-TRACK-C.md,
RC1-TESTER-FEEDBACK.md) and 13 docs updated (README-FIRST.md
mirrored at top level and inside source/docs/,
TESTER-INSTRUCTIONS.md, RC1-PACKAGE-MANIFEST.md, the four
acquisition demos, runbooks/README.md, and five individual
runbooks). See `source/docs/RC1-CHANGES.md` for the per-file
finding-by-finding patch matrix.

## 3. Current Status

| Field | Value |
|---|---|
| Track planned for first tester | **C** (security/compliance review) — selected 2026-05-23 |
| Transfer mechanism | Handled by user out-of-band; both archive variants ready |
| Testers invited | **0** |
| Testers responded | **0** |
| Self-reviews completed | **1** (Claude self-review 2026-05-24, see §6.1 — does NOT count toward acceptance) |
| Findings filed (self-review) | **12** (5 High / 4 Medium / 3 Low — see §7) |
| Blockers open from self-review | **5** (H-1 through H-5, all docs bucket, all fix-in-RC10) |
| RC-09 acceptance met | **NO** — waiting on first outside tester; self-review does not substitute |

This field block is the source of truth. Update it whenever a
tester is invited, responds, or files a finding.

## 4. Tester Roster

| # | Tester | Role / why invited | Track | Bundle variant | Invited (date) | Responded (date) | Status |
|---|---|---|---|---|---|---|---|
| _none yet_ | | | | | | | |

Add one row per invitation. Status values: `invited` → `running` →
`reported` → `triaged`. If a tester declines or never responds,
record that — non-responses are data too.

## 5. Invitation Template

Send one of the two messages below per invitation. Customise the
**bracketed** fields, leave everything else verbatim. The hash line
is what protects the recipient from a tampered archive.

### 5.A Short version (Slack / DM / text)

```
Hi [Name] — would you be up for spending [~30 min / ~90 min /
~3 hr] testing the Island Mountain MAI + Lamprey RC1 tester bundle
next week?

It's a self-contained release-candidate for our local-AI-with-
compliance-governance stack, frozen at commit dceaabc. The
[smoke / build+test / security] track is what I'd ask of you.

I'll send you [MAI-Lamprey-RC1.zip / .tar.gz] (~6 MB). After
download, verify SHA-256:

  [35ada78f66f57901c1c3a438709712cbf0e8f43f60e5b8383eb2343c4a66c76a for .tar.gz]
  [6200c1ccfcd25132e417c03f465eef474ccf35cbd9a8e063256f0089d3ccee84 for .zip]

Then unpack and open README-FIRST.md. Total reading is ~10 min;
TESTER-INSTRUCTIONS.md tells you which sections of README-FIRST
to actually execute given your track.

The bundle is not safe for real regulated data — please use a
test machine. Reply via the issue form in TESTER-INSTRUCTIONS.md
§5 (one issue per problem, even if the answer is "everything
passed").

Thanks — RC-09 of our release plan literally requires "at least
one person besides the original builder has tried it," so your
30 minutes unblocks the whole release.
```

### 5.B Long version (email)

```
Subject: RC1 tester ask — Island Mountain MAI + Lamprey, ~[30 min / 90 min / 3 hr]

Hi [Name],

I'm at Session RC-09 of the release plan for our local AI +
compliance stack (Island Mountain MAI + Lamprey), and the
acceptance criterion for this session is literally "at least one
person besides the original builder has tried RC1." I'd like that
person to be you, if you have the time.

WHAT IT IS

A 19 MB self-contained tester bundle frozen at commit dceaabc.
"MAI" runs local AI inference; "Lamprey" decides what that
inference is allowed to do under HIPAA, ITAR/EAR, and OCAP
(tribal data sovereignty) rules and signs an audit chain. The
bundle ships source plus prebuilt Windows binaries.

WHAT I'M ASKING

Track [A / B / C] of TESTER-INSTRUCTIONS.md. That's about
[30 minutes / 90 minutes / 3-4 hours] of your time.

- Track A is just "does the daemon boot and respond to /v1/health"
  on any laptop with no GPU.
- Track B is "does cargo test --workspace come back green on
  your machine" — needs 4-core x86_64, 8 GB RAM, 60 GB free disk.
- Track C is a security/compliance read of the policy and audit
  layers; needs the same hardware as B plus Rust literacy.

If you only have time for one, Track A is the most valuable —
the whole release lane is gated on "did it work for someone other
than me."

HOW TO RECEIVE THE BUNDLE

I'll send you [MAI-Lamprey-RC1.zip / MAI-Lamprey-RC1.tar.gz] via
[mechanism]. After download, please verify the SHA-256:

  .tar.gz: 35ada78f66f57901c1c3a438709712cbf0e8f43f60e5b8383eb2343c4a66c76a
  .zip:    6200c1ccfcd25132e417c03f465eef474ccf35cbd9a8e063256f0089d3ccee84

If the hash does not match, do not unpack — message me and I'll
re-send.

WHAT TO READ FIRST

After unpacking, README-FIRST.md is the canonical first-run guide
(307 lines, ~10 minutes to read). TESTER-INSTRUCTIONS.md tells you
which sections to execute given your track.

CONSTRAINTS

- Do not point this at real PHI, ITAR-controlled data, or tribal
  records. The bundle is tester-only — use a test machine and
  synthetic data.
- Do not edit committed config to "fix" something during testing.
  File the issue instead (TESTER-INSTRUCTIONS.md §5). Patches to
  the freeze go in RC1.1, not on your machine.

HOW TO REPLY

Use the issue form in TESTER-INSTRUCTIONS.md §5 (track, severity,
freeze, platform, what-ran, expected, saw). One issue per problem.
If everything passed, a one-line "Track [A/B/C] pass on [your OS /
your CPU], freeze dceaabc, no findings" report is exactly what I
need.

Reply by [date]. If anything is unclear, ask before running — the
worst outcome is wasted tester-hours from a documentation gap
that's already known.

Thanks — this unblocks the whole release.

[Your name]
```

## 6. Feedback Intake

One subsection per tester. Add as feedback arrives.

### 6.1 Self-Review — Claude (NOT outside-tester evidence)

**Type:** Self-review (parallel to RC-06's Track A+B rehearsal).
**Track:** C — security/compliance review.
**Date:** 2026-05-24.
**Bundle:** extracted from `MAI-Lamprey-RC1.zip` (sha256 `9a2f95ee…`)
to `C:/Users/17076/Documents/Claude/Island-Mountain-RC1-self-review/`.
**Full memo:** [`RC1-SELF-REVIEW-TRACK-C.md`](RC1-SELF-REVIEW-TRACK-C.md)
(626 lines).

**Why this does not satisfy RC-09 acceptance:** Claude was a
co-author on every session in the build lane. RC-09 specifically
requires "someone besides the original builder" — this is the
builder reviewing their own work. The findings here are still
real and several are High; the outside-reviewer slot remains
open. The self-review exists to catch what an outside Track C
reviewer would hit before they hit it, and to exercise the
triage matrix structurally.

**Environment:**
- OS: Windows 11 Home (build host)
- CPU: x86_64, 4-core laptop class
- RAM: ample (per laptop spec)
- Free disk before run: 647 GB
- rustc: 1.95.0
- Bundle integrity: 667/667 files OK against `CHECKSUMS.txt`

**Execution summary (§1.1 of full memo):**
- Track A binary path: boot 76 ms, `/v1/health` HTTP 200, status
  `healthy`, air-gap `compliant`.
- `cargo test -p mai-compliance --test compliance_demos`:
  **6 passed / 0 failed** (1m28s cold build, 0.32s test).
- `cargo test -p mai-compliance --test compliance_perf --release`:
  **3 passed / 0 failed** — composer P99 **300 ns**, audit
  **119 494/s**, report **1.687 ms**.

**Findings:** see §7 below, all 12 rows with IDs `H-1` through
`L-3`. Full file:line references in
`RC1-SELF-REVIEW-TRACK-C.md` §3-§5.

### 6.2 _Tester 1_ (placeholder)

_To be populated when the first outside tester replies. Each
subsection should include the tester's environment block from the
issue form, each finding numbered, and the raw reply (or a link to
it) for audit._

## 7. Triage Matrix

Each finding from §6 gets one row. Categorise into one of the
roadmap's four buckets and assign disposition.

| ID | Tester | Track | Severity | Bucket | Summary | Disposition |
|---|---|---|---|---|---|---|
| H-1 | self-review §6.1 | C | High | docs | `mai-admin` runbook commands (`audit verify`, `compliance report/verify`, `policy inspect`, `audit tail`) are stubs or undeclared at the freeze | fix-in-RC10 |
| H-2 | self-review §6.1 | C | High | docs | All four acquisition demos reference a `mai` CLI that does not ship | fix-in-RC10 |
| H-3 | self-review §6.1 | C | High | docs | All four acquisition demos cite REST port 8080 (and dashboard 8081); actual daemon binds 8420 / 8421 | fix-in-RC10 |
| H-4 | self-review §6.1 | C | High | docs | All four acquisition demos hardcode `cd "$env:USERPROFILE\Documents\Claude\Island Mountain Mighty Eel OS\mai"` — the builder's workspace path | fix-in-RC10 |
| H-5 | self-review §6.1 | C | High | docs | TESTER-INSTRUCTIONS.md §4.C step 4 cites all five runbook numbers wrong (04/05/09/10/11 vs actual 05/06/11/12/13) | fix-in-RC10 |
| M-1 | self-review §6.1 | C | Medium | docs | TESTER-INSTRUCTIONS.md §4.C step 2 references "three layer docs (router, policy, audit)" that do not exist as separate files (they're inline in ARCHITECTURE.md) | fix-in-RC10 |
| M-2 | self-review §6.1 | C | Medium | docs OR code | README-FIRST.md §5.C documents logs on stderr; observed runtime puts all logs + banner on stdout | needs-investigation (decide doc vs runtime fix) |
| M-3 | self-review §6.1 | C | Medium | docs | Demos prescribe `cargo run --release --bin mai-api` instead of leveraging the bundled `bin/mai-api.exe` from RC1 v2 | fix-in-RC10 |
| M-4 | self-review §6.1 | C | Medium | docs | Runbooks 05/06/11/12/13 use Linux systemd / `/var/lib/mai/...` paths exclusively; bundle is Windows MSVC tester-only. Gap is implicit — no header note tells Track C reviewer these runbooks describe production posture, not tester procedure | fix-in-RC10 |
| L-1 | self-review §6.1 | C | Low | docs | README-FIRST.md:175 "MAI server ready - REST …" uses hyphen; runtime emits em-dash. Cosmetic | dismiss-or-low-fix |
| L-2 | self-review §6.1 | C | Low | docs | ARCHITECTURE.md:318 references `mai/compliance-dashboard/` and `mai/deployment/...` — inside the bundle the path is bare (no `mai/` prefix) | dismiss |
| L-3 | self-review §6.1 | C | Low | code | Health endpoint reports `"gpus":[]` while topology log reports `gpus=1` (probably intentional layer divergence; presents as inconsistent) | needs-investigation |

**Bucket definitions** (per roadmap RC-09):

- **docs** — README-FIRST, TESTER-INSTRUCTIONS, runbooks, or any
  other documentation file is wrong, missing, or misleading. Fix
  in RC-10 with a doc patch.
- **packaging** — manifest exclusion missed something, the bundle
  contains a stray file, an RC1-era doc was not forwarded, or
  the archive itself is broken. Fix in RC-10 by patching
  `RC1-PACKAGE-MANIFEST.md` and rebuilding.
- **code** — the freeze itself misbehaves on a supported platform.
  Fix requires touching `mai-*/src/*` and bumps the freeze
  commit. May force an RC1.1 reissue.
- **environment** — the problem is on the tester's machine
  (wrong toolchain version, missing dependency outside our
  declared minimums, antivirus interference, etc.). Record the
  workaround in `README-FIRST.md` §3 if it's likely to recur;
  otherwise note and dismiss.

**Disposition values:** `fix-in-RC10` (mandatory before wider
sharing), `defer-to-RC2` (known limitation, explicitly out of
RC1 scope), `dismiss` (not actionable / not our bug), or
`needs-investigation`.

## 8. Blockers For Wider Sharing

A blocker is any finding whose disposition is `fix-in-RC10` and
whose severity is `Blocker` or `High`.

| Blocker | Origin (§7 ID) | Owner | Target resolution |
|---|---|---|---|
| Acquisition demos non-runnable as written (H-2 + H-3 + H-4) | H-2, H-3, H-4 | RC-10 | Rewrite each demo's setup script to use `curl` against `:8420` from `cd source` |
| Operator runbooks reference unimplemented CLI surfaces (H-1) | H-1 | RC-10 | Header band on runbooks 05/06/11/12/13 stating which `mai-admin` subcommands are stubbed at the freeze |
| Track C reading list points at wrong runbooks (H-5) | H-5 | RC-10 | Five-character edits in TESTER-INSTRUCTIONS.md §4.C step 4 |

**Note:** All five blockers came from the Claude self-review (§6.1)
and so are not RC-09 acceptance-grade evidence. They are predictive
of what an outside reviewer would file. An outside reviewer may file
additional, distinct blockers — until they have, the list above is
the working set.

The roadmap's RC-09 acceptance includes "blockers are known
before wider sharing." This table is the answer to that.

## 9. Acceptance vs RC-09 Criteria

| Criterion | Status |
|---|---|
| At least one person besides the original builder has tried RC1 | **NO** — §3 (self-review at §6.1 does not satisfy this) |
| Feedback is captured in `RC1-TESTER-FEEDBACK.md` | **PARTIAL** — self-review intake at §6.1 + 12 findings triaged in §7; outside-tester intake at §6.2 still empty |
| Blockers are known before wider sharing | **PARTIAL** — §8 lists 5 self-review blockers; outside reviewer may add more |

RC-09 is open. The self-review pre-flighted the triage matrix and
filed five doc-bucket blockers that RC-10 must address regardless
of what an outside reviewer reports. RC-09 closes when at least one
**outside** reviewer has tried RC1 and their findings have a final
disposition in §7.
