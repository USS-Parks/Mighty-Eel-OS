# RC1 Fresh-Machine Rehearsal Notes

> **STATUS — CLOSED (2026-05-23)**
> Historical record of the RC-06 fresh-machine rehearsal against freeze `dceaabc`. 8 frictions logged (4 packaging gaps fixed via `git archive` cleanup + 2 quickstart gaps fixed via README-FIRST patches + 2 cosmetic). Lessons folded into the live README-FIRST and TESTER-INSTRUCTIONS docs. Kept as literal record; do not edit retroactively.

**Project:** Lamprey MAI
**Release:** RC1 (Tester Bundle)
**Date of run:** 2026-05-23
**Freeze commit:** `dceaabc` (SHIP-17 hotfix)
**Bundle build location:** `C:/Users/17076/Documents/Claude/Island-Mountain-RC1-rehearsal/Lamprey-MAI-RC1/`
**HEAD on build host at rehearsal time:** `8182249` (RC-05 commit)
**Plan reference:** `docs/COGENT-DEPLOYMENT-ROADMAP.md` Session RC-06
**Artifacts directory:** `test-evidence/rc-06/`

This document is a literal record of what was actually executed
during the RC-06 rehearsal on this host. Per the project's
test-evidence-literalism rule, it does not claim more than was done,
and it names every friction the rehearsal exposed — including the
ones a release engineer must apply by hand before the bundle is
viable.

---

## 1. Scope of This Rehearsal

The roadmap RC-06 acceptance says "Prove the package works outside
the original development folder." This rehearsal proves that
partially.

**What this rehearsal did simulate:**

- A bundle assembled outside the source tree at a sibling path
  (`C:/Users/17076/Documents/Claude/Island-Mountain-RC1-rehearsal/`),
  with no environment variables carried over from the workspace
  shell. The bundle was constructed at the freeze commit via
  `git archive`, exactly as a release engineer cutting RC1 would.
- README-FIRST.md was followed verbatim, with each step run in a
  shell whose `PYTHONPATH` had been explicitly `unset` to mimic a
  tester who has not pre-configured their environment.
- The binary path (§5.A) and the source path (§5.B / §6) of the
  README were both exercised.

**What this rehearsal did NOT simulate:**

- A different physical machine. The rust and python toolchains were
  already installed on this host; toolchain-installation friction
  was not exercised.
- A different operating system. Windows MSVC only. The Linux glibc
  path remains untested.
- A fresh user account. The rehearsal CWD was sibling to the source
  tree under the same Windows user profile.

Workspace-coupling bugs (relative paths, missing files, env
dependencies, wrong commands in the doc) are the ones RC-06 catches.
Toolchain-installation bugs and cross-OS gaps are not.

## 2. Bundle Construction Recipe (What a Release Engineer Has to Do)

| Step | Command (POSIX) | Result |
|---|---|---|
| 1. Make bundle root | `mkdir -p Lamprey-MAI-RC1/{source,bin,test-evidence}` | three empty subdirs |
| 2. Export source from freeze | `git archive --format=tar dceaabc \| tar -x -C Lamprey-MAI-RC1/source/` | 6.9 MB tracked content at the freeze |
| 3. Remove stray `et HEAD` (manifest §5) | `rm Lamprey-MAI-RC1/source/"et HEAD"` | gone |
| 4. Remove `.claude/` (manifest §4.3) | `rm -rf Lamprey-MAI-RC1/source/.claude` | gone (was tracked: `.claude/CLAUDE.md`, `.claude/skills/safe-edit/SKILL.md`) |
| 5. Remove `pytest-cache-files-*` (manifest §4.2) | `rm -rf Lamprey-MAI-RC1/source/pytest-cache-files-*` | gone (was tracked: `pytest-cache-files-txhvvf0c/v/cache/nodeids`) |
| 6. Add RC1 docs (post-freeze) | `cp docs/README-FIRST.md docs/RC1-{FREEZE-NOTES,PACKAGE-MANIFEST,BUILD-NOTES,TEST-EVIDENCE}.md Lamprey-MAI-RC1/source/docs/` | the RC1-* docs land in the bundle |
| 7. Add prebuilt binaries (RC1 v2 only) | `cp target/release/lamprey-mai-api.exe target/release/lamprey-mai-ship-validate.exe Lamprey-MAI-RC1/bin/` then `cd bin && sha256sum *.exe > SHA256SUMS` | bin/ populated, sums file lowercase per Unix convention |
| 8. Place README at bundle root | `cp docs/README-FIRST.md Lamprey-MAI-RC1/README-FIRST.md` | duplicate copy, but the root one is the entry point |

Final bundle layout: `Lamprey-MAI-RC1/{README-FIRST.md, source/, bin/, test-evidence/}`.

Final bundle size: **19 MB** (6.9 MB source + 12 MB bin + small
files). Compared with the >60 GB working folder, that's the
manifest's stated three-orders-of-magnitude reduction.

## 3. Verbatim Quickstart Walk

### 3.1 §5.A Hash verification (PowerShell)

```
PS> $h = (Get-FileHash bin\lamprey-mai-api.exe -Algorithm SHA256).Hash
PS> $h
4E201A8498D3E46361C83FC4EFF6E04C1021FCA3187B04A4D9F55F398B1462B6
```

README's literal "Expect:" line matches. PASS. (Note the case
mismatch with `bin/SHA256SUMS` — see §4 friction 5.)

### 3.2 §5.A Run from a clean working directory

```
mkdir mai-test-run; cd mai-test-run
../bin/lamprey-mai-api.exe > stdout.log 2> stderr.log &
```

| Field | Value |
|---|---|
| Process start | 2026-05-23 21:41:31 PDT (`04:41:31.313859Z` UTC) |
| Server ready | 2026-05-23 21:41:31.391086Z UTC |
| Boot wall clock | **~77 ms** (consistent with RC-03's ~57 ms; same machine, normal variance) |
| Banner emitted | yes — full block on stdout, plaintext key + hash + ready-to-paste TOML, matched docs/FIRST-BOOT.md contract exactly |
| Banner key (this run) | redacted; process killed, key dead |
| Banner hash (this run) | `ee63756a9771e30b88d885814a166a6fcd8699a8c3876adece878fa6f9a2d905` (format evidence only) |
| REST listening | `127.0.0.1:8420` |
| gRPC listening | `127.0.0.1:8421` |
| Artifact | `test-evidence/rc-06/bundle-first-boot-stdout.log` (raw, contains the unredacted banner; not to be propagated) |

### 3.3 §5.D Health check

```
curl http://127.0.0.1:8420/v1/health
```

Result: HTTP 200,
`{"status":"healthy","alert_level":"Normal","adapters":[],"hardware":{"gpus":[],"air_gap_status":"compliant"},...}`.
PASS.

### 3.4 §6 Compliance demos from the bundle

```
cd Lamprey-MAI-RC1/source
cargo test -p mai-compliance --test compliance_demos
```

| Field | Value |
|---|---|
| Started | 2026-05-23 21:43:25 PDT |
| Finished | 2026-05-23 21:44:59 PDT |
| Wall clock | 1 m 34 s (1 m 32 s fresh-tree compile + 0.33 s tests) |
| Compile target | bundle's own `target/debug/` — no shared artifacts with the build-host workspace |
| Exit code | 0 |
| Tests | 6 pass / 0 fail / 0 ignored |
| Trust Manifold dry-run coverage | `test_trust_manifold_disconnected_and_expired` (one of the 6) — counts as the RC-06 acceptance "at least one Trust Manifold dry-run" |
| Artifact | `test-evidence/rc-06/bundle-compliance-demos.log` |

### 3.5 Trust Manifold dry-run via scaffold (extra)

The compliance_demos run already covered the Trust Manifold dry-run
via `test_trust_manifold_disconnected_and_expired`. As an
additional Python-side dry-run, the OpenBao trust-demo scaffold was
exercised from the bundle:

```
cd Lamprey-MAI-RC1/source
export PYTHONPATH="$PWD/mai-sdk-python/src"   # NOT in README — see friction 7
python -m pytest apps/openbao-trust-demo/tests/
```

Result: 17 pass in 3.36 s.
Artifact: `test-evidence/rc-06/openbao-trust-demo-tests.log`.

## 4. Friction Inventory

Eight items, ranked by impact on a real tester following README-FIRST
cold.

### Friction 1 — `et HEAD` survives `git archive`
**Severity:** medium. **Surface:** packaging.

The `et HEAD` stray (RC1-PACKAGE-MANIFEST §5) is a tracked file, so
`git archive` includes it. The manifest's §5 already names this and
recommends `git rm -- "et HEAD"` before RC1 cut. The rehearsal
confirms the manifest's prediction is correct and the cleanup is
required.

### Friction 2 — Freeze commit predates RC1 docs
**Severity:** high. **Surface:** packaging.

`git archive dceaabc` does NOT contain README-FIRST.md,
RC1-FREEZE-NOTES.md, RC1-PACKAGE-MANIFEST.md, RC1-BUILD-NOTES.md, or
RC1-TEST-EVIDENCE.md. Those documents landed in commits `414ed97`,
`44f7bc4`, `278f661`, `fa0df32`, `8182249` — all AFTER the freeze.
A release engineer who naively archives at the freeze gets a bundle
that cannot describe itself.

**Resolution applied during rehearsal:** copy the post-freeze RC1
docs into the bundle's `source/docs/` after the `git archive`
extraction, as recipe step 6 above. This treats the RC1 doc
collection as packaging metadata layered on top of the code freeze,
not part of it.

**Resolution recommended for the manifest:** add a sub-section to
RC1-PACKAGE-MANIFEST.md §3 making this explicit: "RC1 docs are
authored after the code freeze and must be copied into the bundle's
`source/docs/` from the build-host workspace; they are not in
`git archive <freeze>` output."

### Friction 3 — `.claude/` survives `git archive`
**Severity:** medium. **Surface:** packaging.

`source/.claude/CLAUDE.md` and `source/.claude/skills/safe-edit/SKILL.md`
are tracked in-repo (they hold Claude Code project state). Manifest
§4.3 says exclude. Release engineer must `rm -rf source/.claude/`
after extraction.

### Friction 4 — `pytest-cache-files-txhvvf0c/` survives `git archive`
**Severity:** medium. **Surface:** packaging.

A single tracked file at `pytest-cache-files-txhvvf0c/v/cache/nodeids`
got accidentally committed at some earlier point. Manifest §4.2
already calls out the directory; release engineer must `rm` after
extraction. Should also land in `.gitignore` post-RC1 so it stops
recurring.

### Friction 5 — `Get-FileHash` returns UPPERCASE; `SHA256SUMS` is lowercase
**Severity:** low. **Surface:** quickstart.

PowerShell's `Get-FileHash <file>.Hash` returns the digest in
upper-case hex. The Unix-standard `bin/SHA256SUMS` is lower-case.
README §5.A's literal "Expect:" line is upper-case so the
PowerShell comparison succeeds, but a tester opening both files
side-by-side would see different case and worry. Hex compares are
case-insensitive; the bytes are identical.

**Resolution applied:** added a one-line note to README §5.A.

### Friction 6 — README never tells testers to set `PYTHONPATH`
**Severity:** high (if testers run Python tests). **Surface:** quickstart.

Any Python invocation — SDK tests, dashboard tests, scaffold tests,
including the openbao-trust-demo Trust Manifold dry-run — fails
without `PYTHONPATH=source/mai-sdk-python/src`. Error:
`ModuleNotFoundError: No module named 'mai'` during pytest
collection.

README §6 only tells testers to run `cargo test ...`, which does not
need PYTHONPATH, so the strictly-documented smoke path is unaffected.
But any tester exploring beyond §6 (e.g. running the dashboard or
the OpenBao scaffold) hits this immediately.

**Resolution applied:** added a "Python paths" subsection to
README §3 covering when and why to export PYTHONPATH.

### Friction 7 — README §6 doesn't name what each compliance_demos test covers
**Severity:** low. **Surface:** quickstart.

A tester running `cargo test -p mai-compliance --test compliance_demos`
sees six test names (`test_hipaa_workflow`, `test_itar_workflow`,
`test_ocap_workflow`, `test_multi_domain`, `test_audit_tamper`,
`test_trust_manifold_disconnected_and_expired`) but no description
of what each one proves. The RC-06 acceptance asks for "at least one
Trust Manifold dry-run"; the tester has no way to tell from the
README which of the six is that dry-run.

**Resolution applied:** added a six-row table to README §6 mapping
each test name to its scenario.

### Friction 8 — bash + Windows-exe stdout redirection has a path quirk
**Severity:** low. **Surface:** rehearsal-environment-specific.

When MSYS/MINGW bash launches a Windows .exe with
`> stdout.log 2> stderr.log &` and then immediately tries to `head`
the file, the file is sometimes not yet visible in the bash CWD
because of a path-translation race between MSYS and Windows native
I/O. By the time the next bash invocation runs, the files are
present at the expected path. Testers using PowerShell
`Start-Process` (the form §5.A actually uses) or Linux bash will not
hit this.

**Resolution applied:** none. Not a README defect; rehearsal-host
artefact. Documented here so future RC-06 reruns know to expect it.

## 5. README-FIRST Patches Applied This Commit

Three surgical edits, all in `docs/README-FIRST.md`:

1. **§3 — Python paths subsection.** Adds a paragraph and an
   `export PYTHONPATH=source/mai-sdk-python/src` example covering
   when testers need it (any pytest invocation).
2. **§5.A — hash case note.** One-line clarification that
   `Get-FileHash` returns uppercase and `bin/SHA256SUMS` is
   lowercase; the comparison is case-insensitive.
3. **§6 — test name enumeration.** Six-row table mapping each
   `compliance_demos` test to its scenario, with `test_trust_manifold_disconnected_and_expired`
   explicitly flagged as the Trust Manifold dry-run.

The line-count delta is small; existing prose is unchanged.

## 6. What Was NOT Done in RC-06

Per the test-evidence-literalism rule, this section is explicit.

- **Different physical machine:** not exercised. Same build host.
- **Linux glibc target:** not exercised.
- **macOS:** not exercised.
- **Tester without rustc/cargo/python pre-installed:** not
  exercised. Toolchain install friction is unknown from this run.
- **Slower hardware:** the boot timings, compile timings, and test
  timings all came from a multi-core developer laptop. A genuine
  minimum-spec laptop has not been tested.
- **Real network adapter:** server bound `127.0.0.1` only.
- **No-Python tester:** a tester who never runs Python tools would
  not have hit friction 6. But would also not exercise the SDK,
  dashboard, or scaffolds, which means much of the surface
  documented in `docs/RC1-PACKAGE-MANIFEST.md` §3.5 is unverified
  on the cold path.
- **Re-verifying README-FIRST after the patches in §5:** the
  rehearsal was a single walk-through; the patched README has not
  itself been rehearsed cold.

## 7. Acceptance Checklist (RC-06)

| Criterion | Status |
|---|---|
| Bundle copied to a clean directory | §2 — `Island-Mountain-RC1-rehearsal/Lamprey-MAI-RC1/` |
| Quickstart run from scratch | §3 — README-FIRST followed verbatim, with PYTHONPATH unset |
| API started | §3.2 — `bin/lamprey-mai-api.exe`, ~77 ms to ready, port 8420 listening |
| At least one Trust Manifold dry-run | §3.4 (`test_trust_manifold_disconnected_and_expired`) + §3.5 (openbao-trust-demo scaffold) |
| At least one compliance demo test | §3.4 — 6/6 pass |
| Every missing dependency or confusing step documented | §4 — 8 friction items |
| Quickstart updated based on what happened | §5 — three surgical patches to README-FIRST.md |

## 8. Artifacts (`test-evidence/rc-06/`)

| File | Purpose |
|---|---|
| `rehearsal-start.txt`, `rehearsal-end.txt` | Wall-clock bracket of the rehearsal |
| `bundle-bin-SHA256SUMS.txt` | Copy of the bundle's `bin/SHA256SUMS` so the doc points at a verifiable hash record |
| `bundle-first-boot-stdout.log` | Unredacted boot transcript from the bundle's `lamprey-mai-api.exe`. Contains a one-time admin key for a process that was immediately killed; do not propagate. |
| `bundle-compliance-demos.log` | Output of `cargo test -p mai-compliance --test compliance_demos` from inside the bundle |
| `openbao-trust-demo-tests.log` | Output of `python -m pytest apps/openbao-trust-demo/tests/` from inside the bundle, with PYTHONPATH set |

## 9. Next Session

Session RC-07 (Tester Instructions And Issue Form) defines three
tester tracks — local smoke tester, technical build/test reviewer,
security/compliance reviewer — each with expected hardware, what to
run, and an issue-report template. RC-06 deliberately did NOT
re-rehearse the README after patching it; a clean re-walk of the
patched quickstart belongs in RC-07's track definitions or in a
later RC-06b if the user prefers.
