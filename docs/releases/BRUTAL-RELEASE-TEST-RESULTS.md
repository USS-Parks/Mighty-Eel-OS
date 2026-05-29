# Brutal Release Test Results

**Project:** Island Mountain MAI + Lamprey  
**Release under test:** RC1 track after RC-05  
**Date:** 2026-05-23  
**Tester role:** adversarial release tester / credibility auditor  
**Repo state at start:** `8182249` (`RC-05: test evidence refresh`) on `main`, synced with `origin/main`  
**Working tree at start:** clean  
**Important parallel work observed:** untracked `test-evidence/rc-06/` appeared during this review; it was not touched.

This is not a friendly pass/fail summary. It records what I tried to
break, what held, what did not get exercised, and what would block an
external RC1 handoff if the bundle were cut right now.

---

## Executive Verdict

The code/test surface is in good shape for an RC1 candidate, but the
release package is not yet safe to hand to an outside tester as-is.

The failures I found are mostly release-process failures, not runtime
crashes:

1. A tracked stray file, `et HEAD`, is still in the tree even though
   the manifest says to remove it before packaging.
2. The package-root `README-FIRST.md` does not exist in the current
   working tree; it exists only at `docs/README-FIRST.md`. That is
   fine before RC-08, but it is a hard blocker if someone zips the tree
   and calls it RC1.
3. The project still has no actual 72-hour burn-in evidence. The
   tooling exists; the endurance report does not.
4. The release validator can print `PASS` while also listing 7
   `DEFERRED` config-only checks. That is expected for config-only
   validation, but the wording is dangerous unless release notes state
   exactly what was and was not runtime-validated.
5. I could not run my own daemon smoke test in this session because
   the local tool policy blocked background process launches. RC-03
   already has a recorded smoke test, but this brutal pass did not
   independently reproduce it.

No new code-level test failures were found in the command surfaces I
could run.

---

## Rerun After RC-06

After RC-06 landed, I reran every brutal-test surface that had been
run in the first pass. The second pass ran at:

- **HEAD:** `b940989` (`RC-06: fresh-machine rehearsal`)
- **Branch:** `main`, synced with `origin/main`
- **Working tree:** only this report was untracked/modified during the
  rerun
- **RC-06 evidence present:** `test-evidence/rc-06/`

### Side-by-Side Results

| Surface | First brutal pass (`8182249`, post-RC-05) | Second brutal pass (`b940989`, post-RC-06) | Change |
|---|---|---|---|
| Release state | `main...origin/main`; report untracked | `main...origin/main`; report untracked | HEAD advanced to RC-06 |
| Root quickstart placement | root `README-FIRST.md` missing; `docs/README-FIRST.md` present | root `README-FIRST.md` missing; `docs/README-FIRST.md` present | unchanged; still must be handled by RC-08 bundle assembly |
| Stray tracked file audit | `et HEAD` tracked | `et HEAD` tracked | unchanged; still a packaging blocker |
| Tracked generated-output audit | no tracked `target/`, cache, top-level temp, or local `results/`; `tests/benchmarks/results/.gitkeep` present | same | unchanged |
| `mai-api.exe` SHA256 | `4E201A8498D3E46361C83FC4EFF6E04C1021FCA3187B04A4D9F55F398B1462B6` | same | unchanged; README hash still matches |
| `mai-ship-validate.exe` SHA256 | `A32DDC2891A7690CB015A9D1ED06CB84D4160F92976E61AC50CB14069E9AE8F8` | same | unchanged |
| `mai-ship-validate --profile deployment\ship\profile.toml` | exit `0`; `34 pass / 0 fail / 7 deferred / 0 skipped` | exit `0`; `34 pass / 0 fail / 7 deferred / 0 skipped` | unchanged; config-only PASS remains easy to misquote |
| `mai-ship-validate --profile config\production.example.toml` | exit `0`; `34 pass / 0 fail / 7 deferred / 0 skipped` | exit `0`; `34 pass / 0 fail / 7 deferred / 0 skipped` | unchanged |
| Missing profile negative test | exit `2` | exit `2` | unchanged; correct |
| Missing state-dir negative test | exit `3` | exit `3` | unchanged; correct |
| Forbidden-term scan | PASS: `204 files, 6 terms, 0 disallowed hits` | PASS: `204 files, 6 terms, 0 disallowed hits` | unchanged |
| `scripts\build-package.ps1 -ValidateOnly -SkipDashboard` | exit `0`; staged layout at commit `8182249c0de2` | exit `0`; staged layout at commit `b940989cd259` | still passes |
| `python -m pytest tools\packaging_tests tools\ship12_tests -v` | `168 passed` | `168 passed` | unchanged |
| `python -m pytest tools\gpu_release_tests tools\burnin_tests -v` | `169 passed, 23 skipped` | `169 passed, 23 skipped` | unchanged; Windows skips remain |
| Independent daemon smoke from this session | not run; background process launch blocked by local policy | not run directly; RC-06 artifacts now show bundle first-boot and ready log | improved evidence exists, but not from my own process launch |

### New RC-06 Evidence Observed

The second pass did not launch a daemon directly from this session, but
RC-06 left package-level evidence that addresses the biggest limitation
from the first brutal pass:

| RC-06 artifact | Observed evidence |
|---|---|
| `test-evidence/rc-06/bundle-first-boot-stdout.log` | first-boot admin-key banner present; ready log says REST on `127.0.0.1:8420`, gRPC on `127.0.0.1:8421` |
| `test-evidence/rc-06/bundle-compliance-demos.log` | `test result: ok. 6 passed; 0 failed` |
| `test-evidence/rc-06/openbao-trust-demo-tests.log` | `17 passed` |
| `test-evidence/rc-06/bundle-bin-SHA256SUMS.txt` | bundle binary hashes captured |

Interpretation: RC-06 materially improves the release story. The
fresh-package path now has evidence for first boot, API readiness,
compliance demos, and the OpenBao trust demo. It does not change the
remaining packaging blockers: `et HEAD`, package-root quickstart
placement, and no 72-hour burn-in report.

---

## Commands Run

### Release State

```text
git status --short --branch
git log --oneline --decorate -10
```

Result:

- `main...origin/main`
- HEAD: `8182249 RC-05: test evidence refresh`
- No tracked modifications at the start of review.

### Package / Tree Audits

```text
git ls-files -- "et HEAD"
git ls-files | rg -n "(^|/)(target|__pycache__|\\.pytest_cache|\\.mypy_cache|\\.ruff_cache|pytest-cache-files|results|\\.tmp|\\.tmp-ship08)(/|$)"
```

Results:

- `et HEAD` is still tracked.
- No tracked `target/`, Python cache, pytest cache, mypy cache, ruff
  cache, top-level temp, or local `results/` files were found.
- One tracked nested placeholder exists:
  `tests/benchmarks/results/.gitkeep`. This is not local generated
  output and is probably benign, but the manifest's broad `results/`
  exclusion should be worded carefully if package automation uses
  pattern-based excludes.

### Binary Hash Check

```text
Get-FileHash target\release\mai-api.exe -Algorithm SHA256
Get-FileHash target\release\mai-ship-validate.exe -Algorithm SHA256
```

Results:

- `mai-api.exe` SHA256:
  `4E201A8498D3E46361C83FC4EFF6E04C1021FCA3187B04A4D9F55F398B1462B6`
- `mai-ship-validate.exe` SHA256:
  `A32DDC2891A7690CB015A9D1ED06CB84D4160F92976E61AC50CB14069E9AE8F8`

The `README-FIRST.md` expected hash for `mai-api.exe` matches the
actual binary.

### Ship Validator Exit Semantics

```text
target\release\mai-ship-validate.exe --profile deployment\ship\profile.toml
target\release\mai-ship-validate.exe --profile config\production.example.toml
target\release\mai-ship-validate.exe --profile C:\nope\missing-profile.toml
target\release\mai-ship-validate.exe --profile deployment\ship\profile.toml --state-dir C:\nope\missing-state
```

Results:

- Ship profile config-only validation: exit `0`
- Production example config-only validation: exit `0`
- Missing profile: exit `2`
- Missing state dir: exit `3`

Negative-path exit semantics held.

Risk observed: both config-only success cases print
`MAI Production Readiness: PASS` while showing `34 pass / 0 fail / 7
deferred / 0 skipped`. That may be technically correct for config-only
validation, but it is too easy for a reader to misquote as "production
runtime passed." The RC1 notes should say "config-only ship validator
passed; runtime state checks remain deferred unless run with a real
state dir and ship profile."

### Forbidden-Term Scan

```text
python scripts\ci_forbidden_terms.py
```

Result:

- PASS: `204 files, 6 terms, 0 disallowed hits`

### Package Layout Validation

```text
scripts\build-package.ps1 -ValidateOnly -SkipDashboard
```

Result:

- Exit `0`
- Staged layout under `build\package-staging`
- No release binary was built because `-ValidateOnly` was used.

### Packaging / SHIP-12 Static Tests

```text
python -m pytest tools\packaging_tests tools\ship12_tests -v
```

Result:

- `168 passed`
- Exit `0`

### GPU Release / Burn-In Contract Tests

```text
python -m pytest tools\gpu_release_tests tools\burnin_tests -v
```

Result:

- `169 passed`
- `23 skipped`
- Exit `0`

Important interpretation:

- The skipped tests are not harmless decoration. On this Windows host,
  Bash-script execution and the burn-in smoke E2E path were skipped.
- This proves the contracts, signer logic, workflow definitions, and
  PowerShell-side script surfaces more than it proves actual burn-in.
- It does not prove a 72-hour run occurred.

---

## Findings

### P1 - `et HEAD` Is Still Tracked

`docs/RC1-PACKAGE-MANIFEST.md` already identifies `et HEAD` as a stray
tracked diff-stat capture and says: "remove before packaging."

Current result:

```text
git ls-files -- "et HEAD"
et HEAD
```

Impact:

- If RC1 is cut now, the bundle contains known tree debris.
- This violates the spirit of RC-01/RC-02: no mystery local or stray
  files silently packaged.

Recommended fix:

```text
git rm -- "et HEAD"
```

Then commit the removal before RC-08 creates the bundle.

### P1 - Package-Root `README-FIRST.md` Is Not Present Yet

Current tree:

- Present: `docs/README-FIRST.md`
- Missing: root-level `README-FIRST.md`

The manifest and quickstart describe this unpacked layout:

```text
MAI-Lamprey-RC1/
|-- README-FIRST.md
|-- source/
```

Impact:

- This is acceptable before RC-08 because the bundle has not been
  assembled yet.
- It becomes a hard release blocker if someone simply zips the repo or
  forgets to copy `docs/README-FIRST.md` to the package root.

Recommended fix:

- RC-08 package assembly must copy `source/docs/README-FIRST.md` to
  `MAI-Lamprey-RC1/README-FIRST.md`.
- Add an RC-08 assertion that the unpacked bundle root contains
  `README-FIRST.md`.

### P1 - No Actual 72-Hour Burn-In Evidence Exists Here

The burn-in machinery is present and the contract tests passed, but
this review found no full 72-hour signed report from this host.

Impact:

- RC1 can still proceed if it is framed as a tester/acquirer bundle.
- Production-appliance claims must not say "72-hour burn-in passed"
  unless a real signed report exists.

Current honest wording:

```text
SHIP-14 burn-in tooling is implemented; full 72-hour hardware burn-in
evidence remains pending.
```

### P2 - Ship Validator `PASS` Output Can Be Misread

The validator correctly returned exit `0` for config-only profile
checks and exit `2`/`3` for missing inputs. However, the successful
config-only report includes deferred runtime checks:

```text
Checks: 34 pass / 0 fail / 7 deferred / 0 skipped
```

The deferred labels also still say things like "lands in SHIP-03",
"lands in SHIP-04", and so on, even though the roadmap says later
runtime startup can flip those checks to pass/fail.

Impact:

- A release reader may interpret config-only `PASS` as runtime
  production readiness.
- The stale "lands in SHIP-X" wording makes the validator output look
  behind the current project state.

Recommended fix:

- Change the config-only heading or docs to something like:
  "Config profile validation: PASS; runtime checks deferred."
- Update deferred messages so they describe the missing runtime
  precondition rather than historical SHIP session numbers.

### P2 - Burn-In / GPU Tests Have Significant Skips on Windows

`tools\gpu_release_tests tools\burnin_tests` passed, but with
`23 skipped`.

Impact:

- Good: contract tests are healthy.
- Not proven: Bash driver execution, GPU runtime, burn-in smoke E2E,
  and full 72-hour endurance.

Recommended fix:

- Keep the Windows result as contract evidence.
- Add a Linux/self-hosted run artifact before claiming GPU or burn-in
  operational readiness.

### P3 - Release Gate Wording Is Easy to Overquote

`docs/RELEASE-GATES.md` says:

```text
A build is shippable when every command above exits 0 and the
72-hour burn-in produced a signed report.
```

This is true as a future gate definition, but dangerous if quoted
beside RC-05 as if it already happened.

Recommended fix:

- No code change required.
- Any RC1 release note should explicitly say RC-05 did not run the
  72-hour gate.

### Test Limitation - I Did Not Reproduce the API Smoke

I attempted to start `target\release\mai-api.exe` in a clean working
directory and poll `/v1/health`, but this session's local approval
policy rejected background process launches. I did not bypass that
policy.

Impact:

- This is a limitation of this brutal test pass, not a product failure.
- RC-03 already records an API smoke test, including first-boot and
  health endpoint success.
- RC-06 should independently repeat this in a clean package directory.

---

## What Held Up

- RC-05 evidence exists and is committed.
- RC-05 raw logs are present under `test-evidence/rc-05/`.
- The release binary hash in the quickstart matches the actual
  `mai-api.exe`.
- `mai-ship-validate` negative exit-code behavior held.
- Forbidden-term scan passed.
- Package validate-only staging completed.
- Packaging and SHIP-12 static tests passed.
- GPU release and burn-in contract tests passed, with transparent
  skips.

---

## What Must Be True Before Outside RC1

Before sending RC1 outside the project, I would require:

1. Remove `et HEAD`.
2. Assemble the actual RC1 folder, not just the repo tree.
3. Confirm package root contains `README-FIRST.md`.
4. Confirm `source/` excludes `target/`, caches, local temp folders,
   and stale `results/`.
5. Include `test-evidence/rc-05/` intentionally, not as accidental
   local state.
6. Run RC-06 from the assembled package, not the dev checkout.
7. In RC-06, actually start `mai-api`, capture health response, run at
   least one Trust Manifold dry-run, and run at least one compliance
   demo test.
8. State plainly that 72-hour burn-in and GPU runtime validation are
   not part of RC1 evidence unless new signed reports are attached.

---

## Bottom Line

The test suite is strong enough to continue. The release story is not
yet strong enough to hand off without RC-06 and RC-08 discipline.

The current risk is not "the code obviously fails." The risk is that a
human packages the wrong shape of the project or overstates what the
evidence proves. That is exactly the kind of failure RC1 must prevent.
