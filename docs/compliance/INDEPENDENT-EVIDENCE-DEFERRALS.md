# Independent Evidence Deferrals

**Session:** J-10b (DOUGHERTY lane, W5)
**Date:** 2026-05-24
**Companion:** [docs/LOCAL-GITDOCTOR-EVIDENCE.md](../scans/LOCAL-GITDOCTOR-EVIDENCE.md), [.cargo/audit.toml](../.cargo/audit.toml), [deny.toml](../deny.toml), [.gitleaks.toml](../.gitleaks.toml), [pyproject.toml](../pyproject.toml) `[tool.bandit]` and `[tool.ruff.lint.per-file-ignores]`

This document records every advisory or finding that the local
independent-evidence layer (Layer 2 of `tools/local_gitdoctor_evidence.py`)
either suppresses via tool config OR continues to report as FAIL with a
deliberate reason. It is the auditable counterpart to the policy files
above: a reviewer can see, in one place, every check that does not
report PASS on its own and the rationale for that state.

The rule is the J-10b acceptance criterion: **every probe must either be
PASS or have an entry in this document.** New deferrals require a
specific advisory ID, the affected scope, the reason, an owner, and a
named lane that will revisit it.

---

## 1. Cargo audit / cargo deny

Layer 2 probes: `IND-RS-004` (`cargo audit`), `IND-RS-005` (`cargo deny check`).
Current status: **PASS** for both, because the policies below allow-list
each advisory by ID. The advisories themselves remain present in the
dependency tree; they are deferred, not fixed.

### 1.1 RUSTSEC-2025-0144 — Timing side-channel in `ml-dsa::decompose`

| Field | Value |
|:--|:--|
| Crate | `ml-dsa 0.0.4` |
| Severity | 6.4 (medium) |
| Path | `mai-vault`, `mai-compliance`, `mai-admin`, `mai-api` → `ml-dsa` |
| Vulnerable range | `<0.1.0-rc.3` |
| Suggested fix | Upgrade to `0.1.0-rc.3` |
| Suppression | [.cargo/audit.toml](../.cargo/audit.toml), [deny.toml](../deny.toml) (both reference this entry) |
| Owner | Basho Parks |
| Revisit lane | post-RC1 dependency-refresh |

**Why deferred.** The advisory describes a `UDIV`-instruction timing leak
in `decompose()` during ML-DSA signing. Exploitation requires precise
timing measurements against the signer process. The MAI signing path
runs inside the air-gap boundary on dedicated hardware; an attacker
capable of those measurements has already breached invariants the threat
model considers stronger than this advisory.

The fix is a semver-incompatible major-version bump (`0.0.4` →
`0.1.0-rc.3`) that breaks call sites in `mai-vault::sign`,
`mai-compliance::report_signer`, and `mai-admin::audit`. Bumping a
crypto primitive across an RC1 freeze without re-running the SHIP-14
ML-DSA report-signer 72h burn-in is the larger risk; that burn-in is the
gate that ships with the RC.

**Revisit trigger.** Either the `0.1.0` stable release lands and we open
a dependency-refresh lane, or someone demonstrates a practical exploit
against an air-gapped MAI deployment.

### 1.2 RUSTSEC-2024-0384 — `instant` is unmaintained

| Field | Value |
|:--|:--|
| Crate | `instant 0.1.13` |
| Severity | unmaintained (no CVSS) |
| Path | `mai-api` → `notify 7.0.0` → `notify-types 1.0.1` → `instant` |
| Vulnerable range | all (no maintained fork) |
| Suggested fix | Migrate to `web-time` upstream of `notify` |
| Suppression | [.cargo/audit.toml](../.cargo/audit.toml), [deny.toml](../deny.toml) |
| Owner | Basho Parks |
| Revisit lane | post-RC1 dependency-refresh |

**Why deferred.** `instant` is reachable only as a transitive of `notify`
7.0.0, which MAI uses for filesystem watch on the config-reload path.
The maintainer recommends `web-time`. The fix is either upgrading
`notify` past the version that still depends on `instant`, or replacing
`notify` with a hand-rolled inotify wrapper. Both are scoped to the
post-RC1 dependency-refresh lane.

**Revisit trigger.** `notify` ships a version with the `instant`
dependency dropped, OR we decide to vendor a thin watcher.

### 1.3 RUSTSEC-2024-0436 — `paste` is no longer maintained

| Field | Value |
|:--|:--|
| Crate | `paste 1.0.15` |
| Severity | unmaintained (no CVSS) |
| Path | `mai-api` → `mai-vault` → `pqcrypto-mldsa 0.1.2` → `paste` |
| Vulnerable range | all (no maintained fork) |
| Suggested fix | Wait on `pqcrypto-mldsa` to drop `paste`, OR fork |
| Suppression | [.cargo/audit.toml](../.cargo/audit.toml) (cargo-audit only — cargo-deny does not currently match this advisory against this lockfile) |
| Owner | Basho Parks |
| Revisit lane | post-RC1 dependency-refresh |

**Why deferred.** `paste` is a compile-time macro crate used by
`pqcrypto-mldsa` for trait expansion. It has no runtime presence in the
built binaries (it expands to direct code at compile time, then drops
out). Upstream `pqcrypto-mldsa` has not yet shipped a version without
it. Same shape as the `instant` advisory: transitive, no semver upgrade
available without changing the parent dependency.

**Revisit trigger.** `pqcrypto-mldsa` releases a `paste`-free version.

---

## 2. pip-audit (host-pip CVEs)

Layer 2 probe: `IND-PY-004` (`python -m pip_audit`).
Current status: **FAIL**, deliberately. The CVEs are against `pip`
itself in the developer's venv, not against any project dependency.

### 2.1 CVE-2026-3219 and CVE-2026-6357 — pip 26.0.1

| Field | Value |
|:--|:--|
| Package | `pip 26.0.1` (the package manager binary in the venv) |
| Severity | per-CVE; both apply to the index/build flow |
| Path | `<venv>/Lib/site-packages/pip` (not a project dep) |
| Fix version | `pip 26.1` |
| Suppression | none — probe still FAILs |
| Owner | the developer running the scan |
| Revisit lane | developer-environment refresh (per-machine) |

**Why deferred and not suppressed.** The MAI project does NOT pin pip.
`requirements-lock.txt` and `pyproject.toml` declare runtime / dev
dependencies; pip itself is the package manager that resolves them. The
fix (`pip install --upgrade pip`) is a developer-environment action on
each machine that runs the audit, not a code change in this repository.

Adding `pip` to the project's locks would be wrong — projects do not
constrain their bootstrap tool's version. Suppressing the CVEs in a
`pip-audit` config would hide a real concern (the dev environment IS
running a vulnerable pip) without fixing it.

The honest posture: leave the probe FAILing, point the developer at the
remediation. CI builds (when they land in a hardening lane after J-10b)
will use the freshly-released pip via `pip install --upgrade pip` as a
first step, at which point this probe goes PASS on those hosts.

**Revisit trigger.** Either the developer upgrades local pip (no code
change required), or a CI baseline image is published that ships pip ≥
26.1.

### 2.2 `mai-sdk` "could not be audited"

`pip-audit` also reports:

```
mai-sdk  Dependency not found on PyPI and could not be audited:
        mai-sdk (0.2.0)
```

This is the workspace's own local editable install. It is not a real
finding; pip-audit cannot probe a non-PyPI distribution. It carries no
deferral entry of its own — pip-audit conflates "skip" and "fail" for
unreachable packages.

---

## 3. Other findings (silenced via tool policy, NOT deferred)

The items below have been resolved by policy in the corresponding tool
config and are not deferrals. They are listed here so a reviewer
auditing the local evidence package can see the full picture in one
place.

### 3.1 Bandit — `pyproject.toml [tool.bandit]`

| Rule | Resolution |
|:--|:--|
| `B101` assert_used (1372 hits) | Skipped globally. Asserts are used pervasively for invariants and tests; bandit cannot distinguish prod from test. Real secret detection is gitleaks + detect-secrets. |
| `B105` hardcoded_password_string (16 hits) | Skipped globally. Pattern fires on header-name constants (`X-IM-Auth-Token`) and test fixtures. Real secrets are gitleaks + detect-secrets. |
| `B310` urllib_urlopen (19 hits) | Skipped globally. Stdlib-only HTTP is the air-gap policy per ARCHITECTURE.md; pulling `requests` is a regression. |
| `B311` random_not_for_security (10 hits) | Skipped globally. Used for non-crypto IDs in tests and tools; crypto code uses `secrets` / OS rng explicitly. |
| `B404` import_subprocess (11 hits) | Skipped globally. Tooling pre-flag for B603/B607; not a finding on its own. |
| `B603` subprocess_without_shell (21 hits) | Skipped globally. `tools/` shells out to known binaries (cargo, python, pytest) with full argv arrays. |
| `B607` start_process_with_partial_path (14 hits) | Skipped globally. Same as B603; PATH-based resolution is intentional. |
| `B608` hardcoded_sql_expressions (1 hit) | `# nosec B608` at `compliance-dashboard/app.py:272` — HTML template, every variable passes through `html.escape`; not SQL. |
| `B104` hardcoded_bind_all_interfaces (1 hit) | `# nosec B104` at `tools/packaging_tests/test_systemd_units.py:167` — test asserts the ABSENCE of `0.0.0.0` in unit files, does not bind. |

### 3.2 Ruff — `pyproject.toml [tool.ruff.lint.per-file-ignores]`

See the inline comments in `pyproject.toml`. Per-file ignores apply to
test trees (assert + subprocess + fixture patterns), tool trees (long
literal strings + subprocess), hyphenated app directories (`N999`), and
the compliance dashboard's header-constant module (`S105`).

One `# noqa: N818` at `apps/tribal-sovereignty/main.py:35`
(`SovereigntyViolation` exception name) is deferred to a dedicated
rename session — the class is referenced by the OCAP wire surface and
by every test in `apps/tribal-sovereignty/tests/`. A lint sweep is the
wrong vehicle for a public-API rename.

### 3.3 Gitleaks — `.gitleaks.toml`

20 baseline findings, all false positives, all allowlisted by path or by
regex:

| Category | Count | Allowlist |
|:--|--:|:--|
| `target/**/*.rmeta` build artifacts containing pkcs8 crate format markers | 5 | path |
| `docs/LOCAL-GITDOCTOR-EVIDENCE.md` containing detect-secrets scan output (recursive scan of scan output) | 12 | path |
| `test-evidence/rc-06/bundle-first-boot-stdout.log` (captured stdout from a test bundle's first-boot key print) | 1 | path |
| `mai-api/src/{metrics,auth}.rs` test fixtures (`sk-live-abcdef0123456789`, `hvs.CAESIQABCDEF`, `im-test-key-12345`) — used inside `#[test]` blocks to verify the project's redaction and hashing code | 2 | regex |

Real secret detection still runs (default ruleset is inherited via
`useDefault = true`); only the named false positives are allowed.

---

## 4. Re-evaluation policy

This document is the single source of truth for non-PASS probes. Each
re-run of the evidence package (e.g. ahead of J-14) MUST:

1. Confirm every advisory ID still listed in `.cargo/audit.toml` and
   `deny.toml` still appears in this doc.
2. Confirm every entry in this doc still has a non-PASS probe finding
   that justifies it. Stale deferrals (the underlying advisory was
   patched upstream) should be removed from both the config and this
   doc in the same commit.
3. Add any new findings the rescan surfaces. New entries require all
   five fields (advisory ID, scope, reason, owner, revisit lane); no
   silent suppressions.

The lane that owns each "Revisit lane" cell is responsible for closing
its row. When all rows here are closed, J-10b's policy artifacts can be
removed and the local evidence package will once again be PASS-only on
its own.
