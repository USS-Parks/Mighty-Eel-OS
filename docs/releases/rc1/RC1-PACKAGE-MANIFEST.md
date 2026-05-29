# RC1 Package Manifest

**Project:** Lamprey MAI
**Release:** RC1 (Tester Bundle)
**Date:** 2026-05-23
**Audience:** release engineers preparing the RC1 tarball, RC1 testers verifying what they received
**Plan reference:** `docs/COGENT-DEPLOYMENT-ROADMAP.md` Session RC-02
**Freeze point:** `dceaabc` (see `docs/RC1-FREEZE-NOTES.md`)

This manifest says exactly which folders and files go into the RC1
tester bundle, which ones do not, and what the unpacked layout looks
like on the tester's machine. The working folder is multi-gigabyte;
the bundle is not.

---

## 1. Build vs. Ship Decision

**Decision: source-build first; prebuilt binary deferred to Session RC-03.**

RC1's default tester path is to clone the source tree and run
`cargo build --release -p mai-api` themselves. Reasoning:

- RC1 testers are technical (acquirer reviewers, security/compliance
  reviewers, release engineers). Running a `cargo build` is in scope.
- Session RC-03 ("Build Release Binary") is the dedicated session for
  producing, sizing, and recording the release binary. RC-02 does not
  pre-empt that decision.
- Omitting a prebuilt binary side-steps the "which platform?" question
  (Windows MSVC vs. Linux glibc vs. Linux musl) and lets RC-03 decide
  it deliberately.

The package layout in §2 reserves an optional `bin/` folder. If RC-03
lands and produces a verified release binary plus build-environment
notes, an RC1 v2 reissue ships with `bin/mai-api[.exe]` and a hash
file alongside. RC1 v1 ships without `bin/`.

## 2. Folder Layout

The unpacked tester bundle:

```
Lamprey-MAI-RC1/
├── README-FIRST.md             # quickstart — populated by RC-04 (placeholder until then)
├── source/                     # the mai/ workspace, filtered per §3 + §4
│   ├── Cargo.toml, Cargo.lock, pyproject.toml, conftest.py, README.md
│   ├── mai-{adapters,agent,api,compliance,core,hil,router,scheduler,sdk-python,sdk-rs,vault}/
│   ├── compliance-dashboard/, adapters/, apps/
│   ├── deployment/, packaging/, configs/, config/, proto/
│   ├── scripts/, tools/, tests/
│   ├── docs/                   # incl. RC1-FREEZE-NOTES.md, RC1-PACKAGE-MANIFEST.md, RC1-TEST-EVIDENCE.md (after RC-05)
│   ├── .integrity/, .githooks/, .github/, .gitignore
│   └── .git/                   # full repo history; lets reviewers verify the freeze commit `dceaabc`
├── bin/                        # optional — RC-03 may populate with mai-api release binary + hash
└── test-evidence/              # populated by RC-05
```

The package root (`Lamprey-MAI-RC1/`) is what the tester sees after
unpacking. `source/` mirrors the existing `mai/` checkout almost
verbatim — the only differences are exclusions listed in §4.

## 3. Include — what goes into `source/`

### 3.1 Cargo workspace files
- `Cargo.toml`, `Cargo.lock`

### 3.2 Python workspace files
- `pyproject.toml`, `conftest.py`

### 3.3 Top-level docs
- `README.md`

### 3.4 Rust crates (10)
- `mai-adapters/`, `mai-agent/`, `mai-api/`, `mai-compliance/`,
  `mai-core/`, `mai-hil/`, `mai-router/`, `mai-scheduler/`,
  `mai-sdk-rs/`, `mai-vault/`

### 3.5 Python packages and scaffolds
- `mai-sdk-python/` — Python SDK (v0.2.0)
- `compliance-dashboard/` — FastAPI dashboard (S44)
- `adapters/` — Python adapter implementations
- `apps/` — six L4–L5 application scaffolds (S30)

### 3.6 Deployment, config, packaging
- `deployment/` — deployment profiles incl. `local-dev`,
  `cloud-trust-core`, `local-mai-node`, `airgap-demo`, and the `ship/`
  profile
- `packaging/` — SHIP-08 packaging (systemd units, install layout)
- `configs/`, `config/` — config samples (intentional duplication —
  both checked in; both referenced by code)
- `proto/` — protobuf definitions

### 3.7 Build, test, and tooling
- `scripts/` — CI scripts (incl. `ci_forbidden_terms.py`,
  `burn-in-72h.sh|ps1`)
- `tools/` — `mai-admin`, `ship12_tests`, `ship12_validate`,
  `mai-ship-validate`
- `tests/` — top-level integration tests

### 3.8 Documentation
- `docs/` — full docs tree, including:
  - `RC1-FREEZE-NOTES.md` (RC-01 output)
  - `RC1-PACKAGE-MANIFEST.md` (this document)
  - `RC1-BUILD-NOTES.md` (RC-03 output)
  - `README-FIRST.md` (RC-04 output — also mirrored at the bundle
    top level by the RC-08 assembler)
  - `RC1-TEST-EVIDENCE.md` (RC-05 output)
  - `RC1-FRESH-MACHINE-NOTES.md` (RC-06 output)
  - `TESTER-INSTRUCTIONS.md` (RC-07 output)
  - `RC1-BUNDLE-NOTES.md` (RC-08 output)
  - `RC1-TESTER-FEEDBACK.md` (RC-09 audit trail; populated as
    feedback arrives)
  - `RC1-SELF-REVIEW-TRACK-C.md` (RC-09 self-review memo, 626 lines)
  - `RC1-CHANGES.md` (RC-10 changelog; tracks RC1.1-docs revisions)
  - `runbooks/` (14 numbered runbooks from SHIP-15, plus the
    `README.md` audience note added in RC1.1-docs)
  - `acquisition/` (Gate D evidence package, S45; demos rewritten
    in RC1.1-docs to use curl against the live HTTP surface)
  - Top-level briefs (SCHEDULER, LAMPREY, AIR-GAP, API-REFERENCE,
    SDK-REFERENCE, plus all operator and ship docs)

### 3.9 Repo metadata and hidden tooling
- `.git/` — full history; reviewers can verify the freeze commit
  `dceaabc` with `git log` / `git show`. Approximately 50–150 MB
  depending on object-pack state. A "lite" reissue may replace this
  with `git archive` output if bundle size becomes an issue.
- `.gitignore`
- `.github/` — CI workflows (incl. the SHIP-13 GPU release workflow)
- `.githooks/` — legacy hooks, still referenced by some docs
- `.integrity/` — anti-truncation tooling: hooks, scripts, subagent
  config, optional MCP server

## 4. Exclude — what does NOT go into `source/`

### 4.1 Build artifacts (multi-GB)
- `target/` — entire Cargo build output (`debug/` and any `release/`).
  Per roadmap §2 (`What Not To Package`), `target/debug/` is
  explicitly excluded; `target/release/` is excluded from RC1 v1 too —
  if a release binary ships, it lives in the top-level `bin/` folder
  via RC-03, not inside `source/target/`.

### 4.2 Caches and stale outputs
- `__pycache__/` (top-level and any nested)
- `.pytest_cache/`, `.mypy_cache/`, `.ruff_cache/`
- `pytest-cache-files-txhvvf0c/` (stray pytest cache; not currently
  in `.gitignore` — add post-RC1)
- `results/` (local-generated perf and test output; not promoted into
  `test-evidence/`)
- `.tmp/`, `.tmp-ship08/` (staging leftovers from prior
  anti-truncation writes)

### 4.3 Tool / session state
- `.claude/` — Claude Code project state (settings, local memory,
  hooks)
- Cowork session state under `/sessions/*/` (not under `mai/`, listed
  for completeness)
- Local IDE state (`.idea/`, `.vscode/`, editor scratch files)

### 4.4 Out of scope
- Model weights — packaged separately if a model pack is built.

## 5. Tree Anomaly

`mai/et HEAD` (4 647 bytes, tracked) is a captured `git diff --stat`
output, almost certainly the result of a shell typo such as
`git diff --stat HEAD > et HEAD` (with the shell parsing `et HEAD` as
a two-token filename). The file is not referenced by any code path or
doc and is not in `.gitignore`.

**Disposition for RC1: remove before packaging.** Recommended
follow-up commit: `chore: remove stray "et HEAD" diff capture` with
the file deleted via `git rm -- "et HEAD"`. If the file is left in
place when RC1 is cut, this manifest must be updated to list it
explicitly under §3 with a "tree debris — ignore" note. Either way,
no mystery files in the bundle.

## 6. Approximate Size

Source tree (filtered per §3 + §4), without `.git/`:

- Rust sources: ~6–8 MB across the 10 crates
- Python sources: ~3–5 MB across SDK, dashboard, adapters, apps
- Docs: ~3–4 MB (~110 markdown files incl. acquisition + runbooks)
- Configs, scripts, packaging, tests: ~1–2 MB
- **Subtotal without `.git/`: ~15–20 MB**

With `.git/`: add ~50–150 MB. Compared with the >60 GB working folder
(dominated by `target/`), the bundle is approximately 0.025–0.3% of
working-folder size. The acceptance criterion "much smaller than the
60 GB+ working folder" is satisfied with three orders of magnitude
of headroom.

## 7. Acceptance Checklist (RC-02)

| Criterion | Status |
|---|---|
| Manifest lists included folders | §3 |
| Manifest lists excluded folders | §4 |
| `mai/target/debug/` explicitly excluded | §4.1 (entire `target/` excluded) |
| Build-or-ship decision recorded | §1 (source-build first; binary deferred to RC-03) |
| Folder layout for `Lamprey-MAI-RC1/` | §2 |
| Package plan smaller than 60 GB+ working folder | §6 (~15–20 MB sans `.git/`) |
| Stray tracked files identified and dispositioned | §5 (`et HEAD` — remove before packaging) |

## 8. Next Session

Session RC-03 (Build Release Binary) runs
`cargo build --release -p mai-api` against the freeze commit
`dceaabc`, captures the binary size and build-environment notes,
validates the health endpoint, and either populates
`Lamprey-MAI-RC1/bin/` or formally records "no prebuilt binary in
RC1 v1" if the build is deferred. Output: `mai/docs/RC1-BUILD-NOTES.md`.
