# TESTER INSTRUCTIONS

**Project:** Lamprey MAI
**Release:** RC1 (Tester Bundle)
**Date:** 2026-05-23
**Freeze commit:** `dceaabc` (SHIP-17 hotfix on `main`)
**Plan reference:** `source/docs/COGENT-DEPLOYMENT-ROADMAP.md` Session RC-07
**Companion doc:** `README-FIRST.md` (the canonical first-run guide)

This document tells you **which job you are doing** when you sit down
with the RC1 bundle. It does not replace `README-FIRST.md`; it points
you at the parts of `README-FIRST.md` (and the rest of the bundle)
that matter for your specific tester role.

If you do not know which role you are, you are almost certainly the
**Local Smoke Tester** (Track A). Start there.

---

## 1. The Three Tracks

| Track | Role | Time | Hardware floor | Output we need back |
|---|---|---|---|---|
| **A** | Local Smoke Tester | ~30 min | Any modern laptop, no GPU | Pass/fail on the smoke test + health check |
| **B** | Technical Build / Test Reviewer | ~90 min | 4-core x86_64, 8 GB RAM, 60 GB free disk | Pass/fail on the full workspace test run + your `compliance_perf` numbers |
| **C** | Security / Compliance Reviewer | ~3-4 hr | Same as Track B + ability to read Rust | Findings memo against the freeze (see §4.C) |

You do not need to run more than one track. If you do run more than
one, run them in order (A then B then C); each track assumes the
previous track's setup is in place.

---

## 2. Before You Start (All Tracks)

1. **Read** `README-FIRST.md` sections 1-4. That is the bundle
   overview, the "what this is not" disclaimers, the minimum
   hardware list, and the folder layout. Five minutes.
2. **Confirm the freeze commit.** From the unpacked bundle:
   ```
   cd Lamprey-MAI-RC1/source
   git rev-parse HEAD     # if the bundle is a git checkout
   ```
   Expected: `dceaabc...`. If your bundle was built from `git
   archive` (no `.git/` directory), confirm via the
   `RC1-FREEZE-NOTES.md` header instead.
3. **Unset `PYTHONPATH`** in the shell you will use, or set it to
   the bundled SDK source root per `README-FIRST.md` §3. Track B
   and Track C will hit `ModuleNotFoundError: No module named
   'mai'` without this.
4. **Pick a shell and stay in it.** Mixing PowerShell and POSIX
   bash within a single run is the most common source of "my
   commands look right but nothing works" reports.

---

## 3. Hardware By Track (Detail)

The numbers in §1 are the floor. Below is what we actually verified
against on the build host, so you can calibrate.

### Track A - Local Smoke Tester

- **CPU:** any x86_64 with 2+ cores.
- **RAM:** 4 GB free is enough; the daemon boots in ~60 ms and
  idles under 200 MB.
- **Disk:** 200 MB for the bundle if you take the binary path
  (RC1 v2). 5 GB if you take the source path and only build
  `mai-api`. **No GPU.**
- **OS:** Windows 10 / 11 x86_64 (binary path) or any OS with the
  Rust + Python toolchains from `README-FIRST.md` §3 (source
  path).
- **Network:** none. The daemon binds to `127.0.0.1` only.

### Track B - Technical Build / Test Reviewer

- **CPU:** x86_64, **4 cores minimum**. The workspace test run
  parallelises across cores; on 2 cores it takes about twice as
  long.
- **RAM:** **8 GB minimum.** Peak during `cargo test --workspace`
  is 2-3 GB.
- **Disk:** **60 GB free.** A full release build of the
  workspace leaves a `target/` directory in the 50+ GB range.
  Plan for it. If you have less, run `cargo clean` between
  the release build (§5.B of `README-FIRST.md`) and the test
  run (§4.B below).
- **OS:** same options as Track A.
- **Toolchain:** rustc 1.85+ (RC1 was built on 1.95.0), Python
  3.12+.
- **Network:** none required for the run; you may want it if
  you do not already have the Rust crate cache populated -
  cargo will fetch dependencies on first build.

### Track C - Security / Compliance Reviewer

- **Everything Track B needs**, plus:
- The ability to read Rust without a guide.
- A note-taking surface (the findings memo from §4.C is the
  artefact we need; format is your choice).
- Optional but useful: a copy of the relevant statute / standard
  for whatever domain you are reviewing (HIPAA Privacy Rule,
  ITAR Part 120-130, OCAP principles document). The bundle does
  not ship these.

---

## 4. What To Run By Track

The commands below assume your CWD is `Lamprey-MAI-RC1/` or
`Lamprey-MAI-RC1/source/` as appropriate. POSIX shell syntax;
PowerShell equivalents are in `README-FIRST.md`.

### 4.A Track A - Local Smoke Tester

**Goal:** prove the daemon boots, prints a first-boot key, and
responds to `/v1/health`. About 30 minutes.

1. Follow `README-FIRST.md` §5.A (binary path) **or** §5.B (source
   path). Pick one. Do not run both.
2. Confirm the success banner from §5.C (the boxed first-boot
   admin key).
3. Run the health check from §5.D. Confirm HTTP 200 and
   `"status":"healthy"`.
4. Stop the daemon per §5.D.
5. **Send back:** §5 of this document (issue form). If everything
   passed, send a one-line "Track A pass on `<your OS> / <your
   CPU>`, freeze `dceaabc`, no findings" report.

You are done. You do **not** need to run the demos (`README-FIRST.md`
§6) unless you want to.

### 4.B Track B - Technical Build / Test Reviewer

**Goal:** confirm the workspace test run is green at the freeze on
your platform, and capture your local `compliance_perf` numbers.

1. Run Track A first. Confirm green.
2. From `Lamprey-MAI-RC1/source/`, run the full workspace test
   suite:
   ```
   cargo test --workspace
   ```
   Expected: **1539 tests, 0 failed, 2 ignored**, in roughly 5-6
   minutes on a 4-core laptop. The `2 ignored` are the burn-in
   driver and the ML-DSA report signer fixture; both are
   intentional and called out in `RC1-TEST-EVIDENCE.md` §3.
3. Run the six compliance demos from `README-FIRST.md` §6:
   ```
   cargo test -p mai-compliance --test compliance_demos
   ```
   Expected: **6 passed, 0 failed**.
4. Run the release-mode perf assertions:
   ```
   cargo test -p mai-compliance --test compliance_perf --release
   ```
   Expected: three asserts pass. **Capture the three measured
   values** (composer P99, audit throughput, report generation).
   Send them back even on pass - the numbers vary by host and we
   want a wider sample.
5. (Optional) Run the SDK and dashboard suites if you have the
   Python environment per `README-FIRST.md` §3:
   ```
   pytest mai-sdk-python/
   pytest dashboard/
   ```
   Expected: 94 pass / 0 fail in the SDK; 20 pass / 0 fail in the
   dashboard.
6. **Send back:** the `cargo test --workspace` final summary line,
   the three `compliance_perf` measured values, your hardware
   spec, and the issue form (§5) for any non-pass.

### 4.C Track C - Security / Compliance Reviewer

**Goal:** independent read of the compliance stack against the
freeze. Output is a findings memo, not a pass/fail.

1. Run Track A and Track B first. The findings memo assumes the
   tests are green on your host.
2. Read the architecture overview:
   `source/docs/acquisition/ARCHITECTURE.md`. The router, policy,
   and audit layers are described inline in its §"System diagram"
   (the three boxes inside the Lamprey block) and in its
   §"Source-of-truth navigation" table — there are no separate
   layer files. For more depth on each layer, follow the
   cross-references at the bottom of that doc into
   `LAMPREY-BRIEF.md`, `TRUST-MANIFOLD.md`, and
   `AUDIT-CORRELATION.md`.
3. Read the four demo narratives:
   `source/docs/acquisition/demos/{healthcare,defense,tribal,multi-domain}.md`,
   and the two demo files embedded in code:
   `source/mai-compliance/tests/compliance_demos.rs` and
   `compliance_perf.rs`.
4. Read at least the following runbooks under
   `source/docs/runbooks/`:
   `05-verify-audit-chain.md`, `06-generate-compliance-report.md`,
   `11-trust-bundle-expired.md`, `12-audit-wal-tamper.md`,
   `13-air-gap-violation.md`.
5. Read `source/docs/RC1-FREEZE-NOTES.md` §"Intentionally
   Excluded" so you know what we already know is missing.
6. **Produce a findings memo.** Suggested structure:
   - Scope you actually reviewed (file paths + commit `dceaabc`).
   - Findings, each tagged **Critical / High / Medium / Low /
     Informational**.
   - For each finding: file:line reference, what you saw, what you
     expected, why it matters, suggested fix (optional).
   - A "did not review" section. We treat this as load-bearing,
     per the project's test-evidence-literalism rule.
7. **Send back:** the memo, plus the issue form (§5) for any
   single finding you want tracked as a discrete bug rather than
   prose.

Track C reviewers: **the Trust Manifold dry-run test
(`test_trust_manifold_disconnected_and_expired`) and the audit
tamper test (`test_audit_tamper`) are the two most-load-bearing
demos.** If your time is short, anchor your read there.

---

## 5. Issue Report Template

Copy this block into the body of a GitHub issue at
[github.com/USS-Parks/Mighty-Eel-OS/issues](https://github.com/USS-Parks/Mighty-Eel-OS/issues),
or into an email to the release engineer who sent you the bundle.
One issue per problem; do not batch unrelated problems.

```
### RC1 Tester Issue

**Track:** A (smoke) / B (build) / C (security)  -- pick one
**Severity:** Blocker / High / Medium / Low / Info
**Freeze commit:** dceaabc                          -- confirm with `git rev-parse HEAD`
**Bundle variant:** RC1 v1 (source only) / RC1 v2 (source + binaries)
**Path taken:** README-FIRST §5.A (binary) / §5.B (source)

#### Platform
- OS + version:
- CPU model + core count:
- RAM:
- Free disk before run:
- rustc --version:
- python --version:

#### What I ran
(Exact command, copy-pasted from the shell. Include the CWD.)

#### What I expected
(One sentence. Cite the section of README-FIRST.md or this doc
that set the expectation, if applicable.)

#### What I saw
(Full stderr / log output. Do NOT redact paths. DO redact any
plaintext admin keys you happened to capture.)

#### Reproduction
[ ] Reproduces every time
[ ] Reproduces sometimes
[ ] Saw it once, could not reproduce

#### Notes
(Anything else - environment quirks, things you tried, hunches.)
```

**Rules of the road:**

- One issue per problem. A failing test that produces ten
  warnings is one issue, not eleven.
- Do not edit the committed config or source to "fix" something
  during testing. File the issue against the freeze; the fix
  belongs in the next RC1 reissue, not on your machine. This is
  also rule §7 of `README-FIRST.md`.
- If you reach for "I'll just patch this and keep going" - stop
  and file the issue first. We would rather have a tester report
  than a tester workaround that obscures the bug.
- If a single command produces a wall of output, attach the full
  log as a file rather than pasting; the bug is usually in the
  first error, not the last.

---

## 6. Definition Of Done (Per Track)

- **Track A done:** §4.A steps 1-4 executed; one-line report sent;
  any failure filed via §5.
- **Track B done:** Track A done + §4.B steps 2-4 executed; the
  three `compliance_perf` numbers sent back; any failure filed
  via §5.
- **Track C done:** Track A + Track B done + findings memo
  delivered; per-bug issues filed via §5 as needed.

If you got this far, thank you. RC1 exists so we find the
problems before a customer does.
