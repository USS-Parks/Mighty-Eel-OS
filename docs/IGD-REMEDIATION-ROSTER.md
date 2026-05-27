# Internal GitDoctor (IGD) Remediation Prompt Roster

**Lane:** IGD (sessions `IGD-01` through `IGD-12`)
**Companion plan:** `IGD-REMEDIATION-PLAN.md` (read it first)
**Source scan:** `docs/INTERNAL-GITDOCTOR-SCAN-2026-05-26.md` (HEAD = `ee6eb13`, overall 86/100)
**Authoring convention:** every prompt is self-contained for an agent walking in cold with no conversation history. Each prompt carries file paths, expected behavior, verification commands, and the explicit reminder of the canonical co-author footer + the session-worktree protocol.
**Commit footer (every commit, no exceptions):**
```
Copyright 2026 - Co-Authored by Basho Parks and Claude Opus 4.7 xHigh <basho@islandmountain.io> <claude@anthropic.com>
```
**Anti-truncation gate:** every session that writes ≥3 files must spawn a verification subagent before commit per workspace `CLAUDE.md`. Every new file >40 lines uses the two-stage staged-write protocol (on Windows native PowerShell sessions the sync layer is not the failure mode, but follow the protocol anyway).
**Session worktree protocol:** every session begins with `tools/Session-Worktree.ps1 -Action new -Session IGD-XX` from the main `mai/` checkout, then `Set-Location` into the printed path. All work — `git add`, `git commit`, `cargo`, `ruff`, `mypy` — happens inside that worktree on branch `session/IGD-XX`. Finalize with `-Action finalize` then `-Action remove`.

---

## Session index

| ID     | Title                                              | WS  | Files | Depends on | Effort |
|:-------|:---------------------------------------------------|:---:|:------|:-----------|:-------|
| IGD-01 | Gitignore openbao-staging + rotate Vault token     | W1  | 2     | —          | XS     |
| IGD-02 | Upgrade pyo3 to ≥0.24.1 (RUSTSEC-2025-0020)         | W2  | 2-3   | —          | S      |
| IGD-03 | Upgrade url to ≥2.5.4, lift idna ≥1.0.3 (RUSTSEC-2024-0421) | W2 | 1-2 | IGD-02 | XS |
| IGD-04 | Fix 3 clippy collapsible_if in mai-vault            | W3  | 1     | —          | XS     |
| IGD-05 | Install commit-msg hook enforcing co-author footer | W4  | 2-3   | —          | S      |
| IGD-06 | cargo fmt the in-flight gen-trust-staging crate     | W3  | 1     | —          | XS     |
| IGD-07 | Upgrade validator to 0.18 (drops proc-macro-error)  | W5  | 1-2   | IGD-02     | S      |
| IGD-08 | Add trailing newlines to 3 apps/ files + editorconfig | W5 | 4     | —          | XS     |
| IGD-09 | Dev-machine pip upgrade note + CI baseline check    | W6  | 1     | —          | XS     |
| IGD-10 | Triage 4 in-source TODOs (close or accept-and-track)| W7  | 1-5   | —          | S      |
| IGD-11 | Re-run full scan suite, capture green evidence      | W8  | 1     | IGD-01..IGD-10 | S |
| IGD-12 | Update MEMORY.md, close lane, plan next            | W8  | 2-3   | IGD-11     | XS     |

---

## IGD-01 — Gitignore openbao-staging + rotate Vault token (W1, XS)

**Why:** the untracked file `mai/deployment/openbao-staging/openbao-connection.toml` contains a real-looking Vault service token at line 10 (gitleaks rule `vault-service-token`, entropy 4.32). Its parent directory `deployment/` is *not* gitignored (other `deployment/*/profile.toml` files are legitimately tracked), so any future `git add deployment/` or naive `git add -A` commits the secret. This is the only finding in the scan that is one keystroke away from data exfiltration.

**Files:**
- `mai/.gitignore` (edit — add 2 lines)
- `mai/deployment/openbao-staging/openbao-connection.toml` (out-of-band: rotate the token at the source system, then update the file contents)

**Steps:**
1. From the main `mai/` checkout: `.\tools\Session-Worktree.ps1 -Action new -Session IGD-01`, then `Set-Location` into the printed path.
2. Confirm the file is still untracked: `git status --short deployment/`.
3. Edit `.gitignore` and add (under a clear comment header):
   ```
   # Local staging artifacts (IGD-01): never commit live keys / tokens.
   deployment/openbao-staging/
   deployment/*-staging/
   ```
4. Verify the rule applies: `git check-ignore -v deployment/openbao-staging/openbao-connection.toml` should now print the matching rule.
5. Rotate the token in the upstream Vault (`OpenBao`) instance. Replace the contents of `openbao-connection.toml:10` with the rotated value. Do **not** commit the file.
6. Run `gitleaks detect --no-git --redact` from `mai/` and confirm the only leak — if any — is the *new* token (still in the untracked file, still ignored). It should report 0 leaks because gitleaks respects `.gitignore` by default for `--no-git`. If a leak is still reported, the file path needs a more specific ignore rule.
7. Commit only the `.gitignore` change.

**Verification (must all hold):**
- `git check-ignore -v deployment/openbao-staging/openbao-connection.toml` prints a matching rule.
- `gitleaks detect --no-git` returns no leaks against the working tree.
- `gitleaks detect` (full history) returns no leaks — unchanged from current baseline.
- `git status --short` no longer shows `deployment/openbao-staging/` as `??`.

**Commit message template:**
```
IGD-01: gitignore deployment/*-staging/ and rotate openbao staging token

Closes H-1 from docs/INTERNAL-GITDOCTOR-SCAN-2026-05-26.md.
The untracked openbao-connection.toml contained a real-looking Vault
service token (gitleaks rule vault-service-token, entropy 4.32) sitting
in an un-gitignored path. .gitignore now blocks both the specific dir
and the *-staging/ pattern so future siblings inherit the rule.

Copyright 2026 - Co-Authored by Basho Parks and Claude Opus 4.7 xHigh <basho@islandmountain.io> <claude@anthropic.com>
```

---

## IGD-02 — Upgrade pyo3 to ≥0.24.1 (RUSTSEC-2025-0020) (W2, S)

**Why:** `cargo audit` reports `RUSTSEC-2025-0020`: `PyString::from_object` in pyo3 0.22.6 forwards `&str` to the Python C API without checking for terminating NUL bytes, leaking out-of-bounds memory via Python exception text. Current dependency path: `pyo3 v0.22.6 → mai-adapters v0.1.0 → mai-api v0.1.0`. The fix is upstream from 0.24.1.

**Files (likely):**
- `mai/Cargo.toml` (workspace or `mai-adapters` package — wherever pyo3 is pinned)
- `mai/Cargo.lock`
- possibly `mai/mai-adapters/src/*.rs` if the PyO3 API surface used by the bridge changed between 0.22 and 0.24

**Steps:**
1. From the main `mai/` checkout: `.\tools\Session-Worktree.ps1 -Action new -Session IGD-02`, then `Set-Location` into the printed path.
2. Find the pyo3 pin: `Select-String -Path Cargo.toml,mai-adapters/Cargo.toml -Pattern '^pyo3'`.
3. Update the constraint to `pyo3 = { version = "0.24", features = [...] }` (preserve the existing feature set — at minimum `extension-module` if present, plus whatever the adapter bridge uses).
4. `cargo update -p pyo3` (lockfile-only refresh of the transitive graph if a workspace pin already exists). If the `Cargo.toml` constraint update is required, follow with `cargo update`.
5. `cargo check --workspace` — must pass. If it does not, the breakage is most likely in `mai-adapters/src/bridge.rs` or `mai-adapters/src/process.rs`. The PyO3 0.23/0.24 migration notes are mostly about `Bound<'_, T>` smart pointers replacing raw `Py<T>` borrows; follow the upstream guide.
6. `cargo clippy --workspace -- -D warnings -A clippy::pedantic` — must pass (note: IGD-04 fixes 3 unrelated lints in mai-vault; expect those to remain until IGD-04 lands. Track that scope here so the gate is fair).
7. `cargo audit` — RUSTSEC-2025-0020 must be gone. RUSTSEC-2024-0421 (idna) is still expected to appear until IGD-03 lands.
8. `cargo deny check` — advisories pass for pyo3; idna and proc-macro-error remain until IGD-03 / IGD-07.
9. `cargo test --workspace --no-run` if the `mai-api` dev process is not running; otherwise `--exclude mai-api`. Compile must succeed; behavioral regression is bounded by 0.22→0.24 PyO3 changes.

**Verification:**
- `cargo audit` no longer lists RUSTSEC-2025-0020.
- `cargo deny check` reports advisories OK for pyo3 path.
- `cargo check --workspace` exit 0.

**Commit message:**
```
IGD-02: upgrade pyo3 to >=0.24.1 (RUSTSEC-2025-0020)

Closes H-2 from docs/INTERNAL-GITDOCTOR-SCAN-2026-05-26.md.
PyString::from_object in pyo3 0.22.x forwards &str to the Python C API
without ensuring a terminating NUL byte, exposing OOB read via Python
exception text. Upstream fix is in 0.24.1.

Copyright 2026 - Co-Authored by Basho Parks and Claude Opus 4.7 xHigh <basho@islandmountain.io> <claude@anthropic.com>
```

---

## IGD-03 — Upgrade url to ≥2.5.4, lifts idna ≥1.0.3 (RUSTSEC-2024-0421) (W2, XS — depends IGD-02)

**Why:** `cargo audit` reports `RUSTSEC-2024-0421`: idna 0.4.0 accepts Punycode labels that decode to ASCII or empty strings, enabling host-name confusion attacks (e.g. `example.org` vs `xn--example-.org` comparing equal under naive parsing). Wherever host-name comparison is part of an authorization check (auth allow-lists, trust-anchor host matching), a masked `xn--` name can mismatch upstream resolvers and pass downstream comparison. Lifting idna ≥ 1.0.3 (which means url ≥ 2.5.4 transitively) closes the gap.

**Files (likely):**
- `mai/Cargo.toml` and/or `mai/Cargo.lock`
- no source code change expected

**Steps:**
1. Sequence after IGD-02 to avoid lockfile thrash.
2. From `mai/`: `.\tools\Session-Worktree.ps1 -Action new -Session IGD-03`.
3. Identify which crates pull idna 0.4.0: `cargo tree -i idna:0.4.0`. The transitive driver is typically `url`.
4. Bump url: `cargo update -p url` (Cargo will pull the latest compatible). If the pin is too tight in any `Cargo.toml`, loosen to `url = "2.5"`.
5. Confirm: `cargo tree -i idna` shows only idna ≥ 1.0.3 (or no idna entries at all).
6. `cargo check --workspace` — must pass.
7. `cargo audit` — RUSTSEC-2024-0421 must be gone.
8. `cargo deny check` — advisories pass for idna path.

**Verification:**
- `cargo audit` no longer lists RUSTSEC-2024-0421.
- `cargo tree -i idna:0.4.0` returns nothing.

**Commit message:**
```
IGD-03: upgrade url so idna lifts to >=1.0.3 (RUSTSEC-2024-0421)

Closes H-3 from docs/INTERNAL-GITDOCTOR-SCAN-2026-05-26.md.
idna 0.4.0 accepts Punycode labels that decode to ASCII or empty
strings, enabling host-name confusion. Lifted via url 2.5.4+ which
brings idna 1.0.3+.

Copyright 2026 - Co-Authored by Basho Parks and Claude Opus 4.7 xHigh <basho@islandmountain.io> <claude@anthropic.com>
```

---

## IGD-04 — Fix 3 clippy collapsible_if in mai-vault (W3, XS)

**Why:** `cargo clippy --workspace -- -D warnings -A clippy::pedantic` fails on three `clippy::collapsible_if` errors, all in `mai-vault/src/file_dev.rs`. `cargo check` passes — only the `-D warnings` lint gate fails. The fix is mechanical: collapse with `&& let` chains as clippy already suggests.

**Files:**
- `mai/mai-vault/src/file_dev.rs` (lines 107 and 153 — see clippy output)

**Steps:**
1. From `mai/`: `.\tools\Session-Worktree.ps1 -Action new -Session IGD-04`.
2. Apply the clippy auto-suggestion at `file_dev.rs:107` (nested ifs over models_dir): `if models_dir.is_dir() && let Ok(entries) = std::fs::read_dir(models_dir) { ... }`.
3. Apply the clippy auto-suggestion at `file_dev.rs:153` (json snapshot scan, twice nested): `if path.extension().and_then(|s| s.to_str()) == Some("json") && let Ok(content) = std::fs::read_to_string(&path) { if let Ok(snap) = serde_json::from_str::<SnapshotInfo>(&content) { snaps.push(snap); } }`.
4. Run `cargo clippy --workspace -- -D warnings -A clippy::pedantic`. Goal: exit 0.
5. Run `cargo check --workspace` and `cargo test --workspace --no-run --exclude mai-api` to confirm no regression.

**Verification:**
- `cargo clippy --workspace -- -D warnings -A clippy::pedantic` exit 0.
- `cargo test -p mai-vault` exit 0.

**Commit message:**
```
IGD-04: collapse 3 nested ifs in mai-vault::file_dev (clippy)

Closes M-1 from docs/INTERNAL-GITDOCTOR-SCAN-2026-05-26.md.

Copyright 2026 - Co-Authored by Basho Parks and Claude Opus 4.7 xHigh <basho@islandmountain.io> <claude@anthropic.com>
```

---

## IGD-05 — Install commit-msg hook enforcing co-author footer (W4, S)

**Why:** the canonical commit footer rule (declared in `JOHN-REMEDIATION-ROSTER.md` line 9 and in memory `feedback_commit_coauthor.md`) is violated in 165 of 303 commits (54%) and in 50 of the last 50 commits. The rule is a governance signal — it loses force the more it is broken. A `commit-msg` hook that rejects non-compliant messages converts the rule from "should" to "cannot".

**Files:**
- `mai/.githooks/commit-msg` (new file, executable shell script)
- `mai/.integrity/scripts/commit-msg-check.ps1` (new, PowerShell port for Windows-native sessions)
- `mai/docs/CONCURRENT-SESSIONS.md` or a new `mai/docs/COMMIT-MSG-HOOK.md` documenting setup
- optionally: `mai/.github/workflows/<lint>.yml` to mirror the check in CI

**Required pattern (regex, case-insensitive):**
```
Co-Authored by Basho Parks and Claude Opus 4\.7 xHigh <basho@islandmountain\.io> <claude@anthropic\.com>
```

(Optional: also accept the looser `Co-Authored-By:` GitHub convention so squash-merged contributions from other systems still pass. Decide and document the policy in the per-session doc.)

**Steps:**
1. From `mai/`: `.\tools\Session-Worktree.ps1 -Action new -Session IGD-05`.
2. Write `.githooks/commit-msg` (POSIX shell). Read the `$1` argument, grep for the regex, exit 1 with a clear error message if missing.
3. Write `.integrity/scripts/commit-msg-check.ps1` mirroring the same logic for Windows-native git installs that prefer PowerShell hooks.
4. Document the activation: `git config core.hooksPath .githooks` (or per-machine `git config --global core.hooksPath`). Note that the existing `pre-commit` hook already lives under `.githooks/`, so this is additive.
5. Add a `.github/workflows/commit-msg-check.yml` (or extend an existing CI job) that re-runs the regex against the most recent commit on push/PR.
6. Test locally: try to commit a message without the footer — must be rejected. Add the footer — must succeed.
7. Document the rule in a new short `docs/COMMIT-MSG-HOOK.md` (≤30 lines) and link it from `mai/.claude/CLAUDE.md` and from `JOHN-REMEDIATION-ROSTER.md`.

**Verification:**
- A commit without the footer fails locally with a clear error from the hook.
- A commit with the canonical footer succeeds.
- CI rejects a non-compliant commit on push.

**Commit message:**
```
IGD-05: commit-msg hook + CI check enforcing co-author footer

Closes M-2 from docs/INTERNAL-GITDOCTOR-SCAN-2026-05-26.md.
The canonical footer is required by JOHN-REMEDIATION-ROSTER.md and
the user's stated rule, but adoption is 138/303 (46%). This converts
the rule from convention to gate.

Copyright 2026 - Co-Authored by Basho Parks and Claude Opus 4.7 xHigh <basho@islandmountain.io> <claude@anthropic.com>
```

---

## IGD-06 — cargo fmt the in-flight gen-trust-staging crate (W3, XS)

**Why:** `cargo fmt --all -- --check` fails only on `tools/gen-trust-staging/src/main.rs` (currently untracked). The file is part of an in-flight workstream that also touches `Cargo.toml` and `Cargo.lock` (workspace member registration) and the untracked `deployment/openbao-staging/` config bundle. Formatting it now is risk-free and clears a recurring noise source from subsequent `cargo fmt --check` gate runs.

**Files:**
- `mai/tools/gen-trust-staging/src/main.rs` (untracked, ~105 lines — does not need to be tracked to be formatted)

**Steps:**
1. From `mai/`: `.\tools\Session-Worktree.ps1 -Action new -Session IGD-06`.
2. Run `cargo fmt -p gen-trust-staging`. (This requires the `Cargo.toml` workspace-member change to be applied locally so cargo can resolve the package; the change is already present in the dirty working tree per the scan.)
3. Confirm: `cargo fmt --all -- --check` no longer reports a diff in `gen-trust-staging`.
4. Do **not** commit `Cargo.toml`, `Cargo.lock`, the new crate, or `deployment/openbao-staging/`. Those are owned by their respective workstreams (the gen-trust-staging functional session and IGD-01 respectively).
5. If `gen-trust-staging` already has a sibling session in flight, coordinate with that session before formatting — they may have local edits.

**Verification:**
- `cargo fmt -p gen-trust-staging -- --check` exit 0.
- No tracked-file changes staged.

**Commit:** none if untracked; if the crate is being committed in this session, fold the format pass into that commit instead of a standalone IGD-06 commit. Document the outcome (`format applied, no commit needed`) in the IGD-12 close-out doc.

---

## IGD-07 — Upgrade validator to 0.18 (drops proc-macro-error) (W5, S — depends IGD-02)

**Why:** `cargo audit` reports `RUSTSEC-2024-0370`: `proc-macro-error 1.0.4` is unmaintained (2+ years, no upstream response). Dependency path: `proc-macro-error → validator_derive 0.16.0 → validator 0.16.1 → mai-api`. Upgrading `validator` to 0.18 (or whichever release first dropped `proc-macro-error`) removes the warning. Informational only — no security exposure.

**Files:**
- `mai/Cargo.toml` and/or `mai/mai-api/Cargo.toml`
- `mai/Cargo.lock`
- possibly `mai/mai-api/src/**` if the validator derive macro API surface changed between 0.16 and 0.18 (it did — derive attribute syntax was modernized)

**Steps:**
1. Sequence after IGD-02 to keep lockfile diffs small.
2. From `mai/`: `.\tools\Session-Worktree.ps1 -Action new -Session IGD-07`.
3. Update the validator pin to `validator = { version = "0.18", features = ["derive"] }`.
4. `cargo update -p validator`.
5. `cargo check --workspace` — most likely failure is in `mai-api/src/handlers/*.rs` or wherever `#[validate(...)]` attributes are used. The migration guide for 0.18 covers attribute renames; apply minimal edits.
6. `cargo test --workspace --no-run --exclude mai-api` (or include if dev server is down) — must pass.
7. `cargo audit` — RUSTSEC-2024-0370 must be gone.

**Verification:**
- `cargo audit` no longer warns about proc-macro-error.
- `cargo tree -i proc-macro-error` returns nothing (or only some other harmless path).
- `cargo check --workspace` exit 0.

**Commit message:**
```
IGD-07: upgrade validator to 0.18 (drops proc-macro-error)

Closes L-3 from docs/INTERNAL-GITDOCTOR-SCAN-2026-05-26.md.
proc-macro-error 1.0.4 has been unmaintained for 2+ years
(RUSTSEC-2024-0370). validator 0.18 cuts the transitive dep.

Copyright 2026 - Co-Authored by Basho Parks and Claude Opus 4.7 xHigh <basho@islandmountain.io> <claude@anthropic.com>
```

---

## IGD-08 — Add trailing newlines to 3 apps/ files + editorconfig (W5, XS)

**Why:** three Python files end without a trailing newline: `apps/compliance-routed/main.py`, `apps/operator/main.py`, `apps/operator/tests/test_smoke.py`. Not enforced by any current hook. Add the newlines and an `.editorconfig` so future files inherit the rule.

**Files:**
- `mai/apps/compliance-routed/main.py`
- `mai/apps/operator/main.py`
- `mai/apps/operator/tests/test_smoke.py`
- `mai/.editorconfig` (new, ≤15 lines)

**Steps:**
1. From `mai/`: `.\tools\Session-Worktree.ps1 -Action new -Session IGD-08`.
2. For each of the 3 files: read the last byte; if not `0x0A`, append one. (PowerShell: `Add-Content -Path <file> -Value '' -NoNewline` is the wrong direction; use `[System.IO.File]::AppendAllText(<file>, "`n")`. Or simpler: open in any editor and save with newline-at-EOF preference.)
3. Verify: re-run the trailing-newline scan from the internal GitDoctor scan (script in §3 of the scan report) — must show 0 misses.
4. Add `.editorconfig` declaring `insert_final_newline = true` for `[*]`, with appropriate `charset = utf-8` and `end_of_line = lf` defaults. Ruff and most editors honor `.editorconfig` automatically.
5. Optional: add a pre-commit hook line that rejects files lacking trailing newlines.

**Verification:**
- All 3 files now end with `0x0A`.
- `.editorconfig` exists at repo root with `insert_final_newline = true`.
- `ruff check apps/` still passes.

**Commit message:**
```
IGD-08: trailing newlines on 3 apps/ files + .editorconfig

Closes L-1 from docs/INTERNAL-GITDOCTOR-SCAN-2026-05-26.md.

Copyright 2026 - Co-Authored by Basho Parks and Claude Opus 4.7 xHigh <basho@islandmountain.io> <claude@anthropic.com>
```

---

## IGD-09 — Dev-machine pip upgrade note + CI baseline check (W6, XS)

**Why:** `pip-audit` reports CVE-2026-3219 and CVE-2026-6357 against the *system* pip 26.0.1 — not a project dependency, just the version pip-audit ran under. Fix is `pip install -U pip` on each developer machine and bumping the build-image baseline.

**Files:**
- `mai/docs/DEV-ENVIRONMENT.md` or extend an existing one (new short section)
- if the project has a Dockerfile or devcontainer that pins pip: bump it

**Steps:**
1. From `mai/`: `.\tools\Session-Worktree.ps1 -Action new -Session IGD-09`.
2. Search for any pip pin in build configs: `Select-String -Path **/Dockerfile,**/*.devcontainer/*.json,**/*.yml -Pattern 'pip[=<>]'`.
3. If found, bump to the latest patched line (≥ 26.1).
4. Add a short note under "Developer setup" in the dev environment doc instructing `python -m pip install -U pip` after cloning.
5. No code change required; this is hygiene + documentation.

**Verification:**
- `python -m pip_audit` on a freshly-updated dev machine no longer reports the pip CVEs.
- Documentation updated.

**Commit message:**
```
IGD-09: doc the pip upgrade requirement + bump any pinned baseline

Closes M-4 from docs/INTERNAL-GITDOCTOR-SCAN-2026-05-26.md.
pip 26.0.1 carries CVE-2026-3219 and CVE-2026-6357 (fixed in 26.1).
Not a project dep; this is purely dev-environment hygiene.

Copyright 2026 - Co-Authored by Basho Parks and Claude Opus 4.7 xHigh <basho@islandmountain.io> <claude@anthropic.com>
```

---

## IGD-10 — Triage 4 in-source TODOs (close or accept-and-track) (W7, S)

**Why:** four `TODO` markers remain in committed Rust source. All are session-pinned and already disclosed in `docs/KNOWN-ISSUES.md`, but a clean repo should either resolve them or convert them into explicit issues with deadlines. This session triages each one.

**Files (read at minimum):**
- `mai/mai-adapters/src/manager.rs:586` — "Track in-flight request count per adapter"
- `mai/mai-core/src/models/usb.rs:161`
- `mai/mai-scheduler/src/default.rs:394`
- `mai/mai-scheduler/src/default.rs:399`
- `mai/mai-scheduler/src/default.rs:402`

**Steps:**
1. From `mai/`: `.\tools\Session-Worktree.ps1 -Action new -Session IGD-10`.
2. For each TODO, decide one of three outcomes:
   - **Close inline** — implement the missing behavior in this session if it is ≤30 lines and low-risk.
   - **Convert to tracked work** — replace the bare `// TODO` with `// TODO(IGD-10-followup-N):` referencing a new entry in `docs/KNOWN-ISSUES.md` that includes owner, target session, and deadline.
   - **Accept** — leave the TODO but ensure `KNOWN-ISSUES.md` carries it explicitly and clearly explains why it is intentionally deferred (e.g. blocked on hardware, blocked on upstream).
3. Update `docs/KNOWN-ISSUES.md` with the resulting status table.
4. The `mai-adapters/src/manager.rs:586` item (in-flight request count) is likely the most actionable — assess whether it requires a new metrics field on the adapter manager and a counter increment/decrement around dispatch. If yes, close inline.

**Verification:**
- `Grep` for bare `TODO\b` in `**/src/**/*.rs` returns either zero hits or only hits that match the `TODO(IGD-10-followup-N):` pattern.
- `docs/KNOWN-ISSUES.md` is current.

**Commit message template:**
```
IGD-10: triage in-source TODOs — N closed, M tracked, K accepted

Closes L-2 from docs/INTERNAL-GITDOCTOR-SCAN-2026-05-26.md.
- mai-adapters/manager.rs:586  [closed | tracked | accepted, see KNOWN-ISSUES.md]
- mai-core/models/usb.rs:161    [...]
- mai-scheduler/default.rs:394  [...]
- mai-scheduler/default.rs:399  [...]
- mai-scheduler/default.rs:402  [...]

Copyright 2026 - Co-Authored by Basho Parks and Claude Opus 4.7 xHigh <basho@islandmountain.io> <claude@anthropic.com>
```

---

## IGD-11 — Re-run full scan suite, capture green evidence (W8, S — depends on IGD-01..IGD-10)

**Why:** the IGD lane only counts if a fresh scan confirms the deltas. This session re-runs the same set of tools used in the original scan and writes the new evidence pack.

**Files:**
- `mai/docs/INTERNAL-GITDOCTOR-SCAN-<YYYY-MM-DD>.md` (new — same structure as `INTERNAL-GITDOCTOR-SCAN-2026-05-26.md`)

**Steps:**
1. From `mai/`: `.\tools\Session-Worktree.ps1 -Action new -Session IGD-11`.
2. Re-run, capturing output:
   - `cargo fmt --all -- --check`
   - `cargo check --workspace --offline`
   - `cargo clippy --workspace -- -D warnings -A clippy::pedantic`
   - `cargo test --workspace --no-run --exclude mai-api` (or include if dev server is down)
   - `cargo audit`
   - `cargo deny check`
   - `ruff check adapters/ mai-sdk-python/ apps/`
   - `python -m mypy --strict mai-sdk-python/src/`
   - `python -m mypy adapters/`
   - `gitleaks detect --no-git --redact`
   - `gitleaks detect --redact` (full history)
   - integrity scan (null bytes + brace imbalance + trailing newlines, native PowerShell port)
3. Score the result against the same per-category rubric as the 2026-05-26 scan.
4. Write the new dated scan report, mirroring §1-§6 of the previous one.
5. The lane goal is ≥95 overall. If it falls short, identify which findings remain open and either land a same-session fix or open a follow-up session.

**Verification:**
- New `docs/INTERNAL-GITDOCTOR-SCAN-<date>.md` exists.
- Overall score ≥ 95.
- All HIGH findings from the 2026-05-26 scan show CLOSED.

**Commit message:**
```
IGD-11: re-scan evidence — overall <score>/100, all HIGH closed

Captures the post-IGD-lane state of the repo.

Copyright 2026 - Co-Authored by Basho Parks and Claude Opus 4.7 xHigh <basho@islandmountain.io> <claude@anthropic.com>
```

---

## IGD-12 — Update MEMORY.md, close lane, plan next (W8, XS — depends IGD-11)

**Why:** the lane is only truly closed when the persistent memory layer reflects it and the next workstream is identified.

**Files:**
- `C:\Users\17076\.claude\projects\C--Users-17076-Documents-Claude-Island-Mountain-Mighty-Eel-OS\memory\MEMORY.md` (add one index line)
- `C:\Users\17076\.claude\projects\...\memory\project_igd_lane.md` (new per-lane memory file, ≤40 lines)
- `mai/docs/INTERNAL-GITDOCTOR-SCAN-2026-05-26.md` (append "CLOSED by IGD lane <date>" footer)

**Steps:**
1. From `mai/`: `.\tools\Session-Worktree.ps1 -Action new -Session IGD-12`.
2. Write `memory/project_igd_lane.md` summarizing: start date, end date, sessions IGD-01..IGD-11 with one-line outcomes, ending overall score, residual findings (if any), pointer to the post-scan evidence file.
3. Add one line to `memory/MEMORY.md` under a sensible position: `- [IGD remediation lane](project_igd_lane.md) — <one-line hook>`.
4. Append a closure footer to `docs/INTERNAL-GITDOCTOR-SCAN-2026-05-26.md`.
5. Identify the next lane. Likely candidates: RC-12 bundle re-roll, GD75-01..GD75-16 (external GitDoctor 75→95 push, already scaffolded), the `gen-trust-staging` functional close-out, or kicking off the next deferred SHIP runtime check.

**Verification:**
- `MEMORY.md` has a single new index line pointing to the lane memory file.
- The lane memory file exists and follows the project's memory file structure (frontmatter, body, links).
- The IGD lane status is publicly tractable from a cold session.

**Commit message:**
```
IGD-12: close IGD lane — memory + lane closure footer

Final commit of the IGD remediation lane. See INTERNAL-GITDOCTOR-SCAN-<date>
for the post-lane evidence, and memory/project_igd_lane.md for the snapshot.

Copyright 2026 - Co-Authored by Basho Parks and Claude Opus 4.7 xHigh <basho@islandmountain.io> <claude@anthropic.com>
```

---

## Session checklist (every session, every time)

| Check | Mandatory |
|-------|-----------|
| Started in a fresh `session/IGD-XX` worktree via `Session-Worktree.ps1` | YES |
| Commit body ends with the canonical co-author footer | YES |
| Pre-commit integrity check returns 0 errors and 0 warnings on staged files | YES |
| Batches of 3+ files spawn a verification subagent before commit | YES |
| Run-of-record tool output captured (cargo audit / cargo deny / clippy / fmt / gitleaks / mypy / ruff) | YES, scope-appropriate |
| `git diff --cached --stat` inspected before commit (no unexpected deletions) | YES |
| Worktree finalized + removed after the commit lands on `origin/main` | YES |

---

*IGD Remediation Roster — 2026-05-26 — Co-Authored by Basho Parks and Claude Opus 4.7 xHigh \<basho@islandmountain.io\> \<claude@anthropic.com\>*
