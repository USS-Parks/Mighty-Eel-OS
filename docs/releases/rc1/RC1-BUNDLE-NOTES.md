# RC1 Bundle Assembly Notes

**Project:** Lamprey MAI
**Release:** RC1 v2 (Tester Bundle — source + binaries)
**Date of assembly:** 2026-05-23
**Freeze commit:** `dceaabc` (SHIP-17 hotfix on `main`)
**Bundle location:** `C:/Users/17076/Documents/Claude/Island-Mountain-RC1-release/Lamprey-MAI-RC1/`
**Build host HEAD at assembly time:** `d42591d` (RC-07 commit)
**Plan reference:** `docs/COGENT-DEPLOYMENT-ROADMAP.md` Session RC-08

This document is a literal record of what was actually assembled
on this host during the RC-08 packaging pass. Per the project's
test-evidence-literalism rule (`feedback_test_evidence_literalism`),
it distinguishes "bundle exists" from "bundle has been tested" and
names anything that was not exercised.

---

## 1. Scope

RC-08 assembles the real RC1 v2 tester bundle from the freeze
commit. It is **not** a rehearsal — RC-06 was the rehearsal. This
pass produces the artefact a release engineer can hand to a tester
in Session RC-09.

The bundle was assembled outside the working tree, at a sibling
path on the same Windows build host. Cross-host transfer is not
part of RC-08; that is RC-09's job.

## 2. Bundle Location And Size

| Item | Value |
|---|---|
| Path | `C:/Users/17076/Documents/Claude/Island-Mountain-RC1-release/Lamprey-MAI-RC1/` |
| Total size, uncompressed | **19 MB** |
| Total file count | **666** (per top-level `CHECKSUMS.txt`) |
| Top-level layout | `README-FIRST.md`, `bin/`, `source/`, `test-evidence/`, `CHECKSUMS.txt` |

Size breakdown by top-level entry:

| Entry | Size | Contents |
|---|---|---|
| `source/` | 6.9 MB | filtered `mai/` workspace at `dceaabc` (no `.git/`) plus RC1 docs copied forward |
| `bin/` | 12 MB | `lamprey-mai-api.exe`, `lamprey-mai-ship-validate.exe`, `SHA256SUMS` |
| `test-evidence/` | 169 KB | `rc-05/` (full-workspace test logs) + `rc-06/` (fresh-machine rehearsal logs) |
| `README-FIRST.md` | 16 KB | canonical first-run guide, also mirrored at `source/docs/README-FIRST.md` |
| `CHECKSUMS.txt` | 64 KB | SHA-256 of every file in the bundle, except itself |

Compared with the >60 GB working folder (dominated by
`target/`), the bundle is approximately 0.03% of working-folder
size — within the order of magnitude predicted by manifest §6
(15–20 MB sans `.git/`).

## 3. Assembly Recipe Executed

| Step | Command | Result |
|---|---|---|
| 1. Make bundle root | `mkdir -p Lamprey-MAI-RC1/{source,bin,test-evidence}` | three empty subdirs |
| 2. Extract source from freeze | `git archive --format=tar dceaabc \| tar -x -C source/` | 6.9 MB, includes the four tracked-but-excluded items |
| 3. Remove `et HEAD` | `rm "source/et HEAD"` | gone (manifest §5) |
| 4. Remove `.claude/` | `rm -rf source/.claude` | gone (manifest §4.3) |
| 5. Remove pytest cache | `rm -rf source/pytest-cache-files-*` | gone (manifest §4.2) |
| 6. Copy README-FIRST to top level | `cp mai/docs/README-FIRST.md Lamprey-MAI-RC1/README-FIRST.md` | 16 KB |
| 7. Copy RC1 docs into `source/docs/` | `cp mai/docs/{RC1-*.md,README-FIRST.md,TESTER-INSTRUCTIONS.md} source/docs/` | 7 files |
| 8. Copy binaries | `cp mai/target/release/{lamprey-mai-api.exe,lamprey-mai-ship-validate.exe} bin/` | 12 MB |
| 9. Write per-binary hashes | `(cd bin && sha256sum *.exe > SHA256SUMS)` | 2 lines |
| 10. Copy test-evidence | `cp -r mai/test-evidence/{rc-05,rc-06} test-evidence/` | 169 KB |
| 11. Write top-level checksums | `find . -type f ! -name CHECKSUMS.txt -print0 \| sort -z \| xargs -0 sha256sum > CHECKSUMS.txt` | 666 entries |

Step 7 also includes `RC1-BUNDLE-NOTES.md` (this document) once it
lands in the workspace.

## 4. Manifest Conformance

### 4.1 Include sweep (manifest §3)

| §3 item | Result |
|---|---|
| §3.1 Cargo workspace files | `source/Cargo.toml`, `source/Cargo.lock` present |
| §3.2 Python workspace files | `source/pyproject.toml`, `source/conftest.py` present |
| §3.3 Top-level docs | `source/README.md` present |
| §3.4 Ten Rust crates | all ten present: `mai-{adapters,agent,api,compliance,core,hil,router,scheduler,sdk-rs,vault}` |
| §3.5 Python packages + scaffolds | `mai-sdk-python/`, `compliance-dashboard/`, `adapters/`, `apps/` all present |
| §3.6 Deployment / config / packaging | `deployment/`, `packaging/`, `configs/`, `config/`, `proto/` all present |
| §3.7 Build / test / tooling | `scripts/`, `tools/`, `tests/` present |
| §3.8 Documentation | `source/docs/` includes the seven RC1 docs copied forward |
| §3.9 Repo metadata | `.gitignore`, `.github/`, `.githooks/`, `.integrity/` present; `.git/` intentionally **not** included (see §6) |

### 4.2 Exclude sweep (manifest §4)

Anti-exclusion scan over the assembled bundle (`find ... -name <pat>`),
ten patterns, all clean:

```
OK: no 'target'
OK: no '__pycache__'
OK: no '.pytest_cache'
OK: no '.mypy_cache'
OK: no '.ruff_cache'
OK: no 'et HEAD'
OK: no '.claude'
OK: no 'pytest-cache-files'
OK: no '.tmp'
OK: no '.tmp-ship08'
```

No accidental `target/debug/`, no caches, no session state, no
stray diff capture. The four tracked-but-excluded items from RC-06
friction analysis were extracted by `git archive` (as expected) and
then deleted in steps 3-5.

## 5. Binary Hash Verification

Both release binaries copied into `bin/` carry the SHA-256 values
recorded in `RC1-BUILD-NOTES.md` for the RC-03 build:

| Binary | Expected (RC-03) | Bundle (RC-08) | Match |
|---|---|---|---|
| `lamprey-mai-api.exe` | `4e201a8498d3e46361c83fc4eff6e04c1021fca3187b04a4d9f55f398b1462b6` | `4e201a8498d3e46361c83fc4eff6e04c1021fca3187b04a4d9f55f398b1462b6` | yes |
| `lamprey-mai-ship-validate.exe` | `a32ddc2891a7690cb015a9d1ed06cb84d4160f92976e61ac50cb14069e9ae8f8` | `a32ddc2891a7690cb015a9d1ed06cb84d4160f92976e61ac50cb14069e9ae8f8` | yes |

Verified by `sha256sum lamprey-mai-api.exe lamprey-mai-ship-validate.exe` on the
build-host copy before staging into `bin/`, and again on the bundle
copy via the top-level `CHECKSUMS.txt` rollup.

## 6. RC-06 Frictions: What Was Applied

The RC-06 rehearsal logged eight frictions; this assembly pass
applied the four packaging-recipe ones and inherits the README
patches.

| RC-06 friction | Disposition in RC-08 |
|---|---|
| (1) `et HEAD` survives `git archive` | Removed in step 3. |
| (2) Freeze commit predates RC1 docs | Step 7 copies all seven RC1-era docs forward into `source/docs/`. README-FIRST.md is also mirrored at the bundle root (step 6) so testers find it without navigating into `source/`. |
| (3) `.claude/{CLAUDE.md,skills/}` tracked but excluded | Removed in step 4. |
| (4) `pytest-cache-files-*/` tracked but excluded | Removed in step 5. |
| (5) `Get-FileHash` upper-case vs `SHA256SUMS` lower-case | Already noted in README-FIRST.md §5.A "Hash case note" patch from RC-06. |
| (6) Missing `PYTHONPATH` instruction in README | Already fixed in README-FIRST.md §3 patch from RC-06. |
| (7) §6 missing per-test enumeration | Already fixed in README-FIRST.md §6 patch from RC-06. |
| (8) MSYS bash + Windows .exe stdout redirection quirk | Rehearsal-host artefact; no bundle action. |

The `.git/` directory called out in manifest §3.9 is **not**
included in this assembly. Reasoning: `git archive` was used (not
`git clone`), the bundle is RC1 v2, and the freeze commit can be
verified via the per-file hashes in `CHECKSUMS.txt` plus the
release-binary SHA-256 line in `RC1-BUILD-NOTES.md`. A "with
`.git/`" reissue is a 50–150 MB delta — straightforward if a
specific reviewer asks for it; not added preemptively.

## 7. Acceptance vs RC-08 Criteria

| Criterion | Result |
|---|---|
| RC1 bundle exists | Yes — `Lamprey-MAI-RC1/` at the path in §2. |
| Bundle size is explainable | Yes — §2 size breakdown table. |
| Bundle contents match the manifest | Yes — §4.1 include sweep + §4.2 exclude sweep. |
| No accidental `target/debug/` inside it | Yes — §4.2 anti-exclusion sweep, line 1. |

## 8. What Was NOT Done In RC-08

Per the test-evidence-literalism rule, this section is load-bearing:

- **The bundle was not unpacked and re-tested.** RC-06 did the
  verbatim README-FIRST walk on a sibling-path bundle built from
  the same freeze commit. RC-08 produces the canonical bundle but
  does not repeat that walk. If the freeze commit, the README-FIRST
  patches, or the exclusion list changed since RC-06, a fresh walk
  is owed.
- **No cross-host transfer was exercised.** The bundle sits on the
  same disk as the build host. Tarball or zip transmission, archive
  integrity across a network path, and unpacking on a different
  user account are RC-09's concern.
- **The bundle was not compressed.** RC-08's acceptance criteria
  speak to "the RC1 folder"; a compressed archive (`.tar.gz` or
  `.zip`) is downstream of the folder and would normally be the
  release engineer's last step in RC-09. Compression is **deferred**
  rather than skipped — see §9.
- **No `.git/` history was included.** See §6 last paragraph.
- **No Linux glibc binaries.** RC1 v2 is Windows MSVC only. Linux
  reissue is RC2 work.
- **No model weights.** Manifest §4.4 — out of scope.
- **No GPG / signature.** Binary integrity is by SHA-256 only;
  signing is RC2 / Production Appliance territory.
- **No 72-hour burn-in evidence carried into the bundle.** SHIP-14
  tooling is present under `source/scripts/burn-in-72h.{sh,ps1}` and
  `source/mai-api/src/ship/burn_in.rs`; no signed endurance report
  is in `test-evidence/`. This matches RC-05 §7.

## 9. Next Steps

- **Compression.** When RC-09 picks a transfer mechanism, the
  release engineer compresses the folder. Suggested commands:
  - POSIX: `tar -czf Lamprey-MAI-RC1.3.tar.gz Lamprey-MAI-RC1/`
  - PowerShell: `Compress-Archive -Path Lamprey-MAI-RC1 -DestinationPath Lamprey-MAI-RC1.3.zip`
  Re-hash the archive after compression and publish that hash
  alongside the bundle.
- **RC-09** (First Outside Tester). Send the bundle to one
  trusted technical person; collect environment + failures into
  `RC1-TESTER-FEEDBACK.md`.
- **RC-10** (RC1 Fix Pass). Patch docs / manifest / startup
  problems found in RC-09. Reissue as RC1.1 if code changed.
