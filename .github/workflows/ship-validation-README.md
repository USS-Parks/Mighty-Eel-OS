# SHIP-12 — SHIP Validation Workflow

Workflow file: [`ship-validation.yml`](./ship-validation.yml)

SHIP-12 wires CI enforcement for everything the SHIP hardening lane needs to
stay green between sessions. Each gate below is enforced on every push to
`main` and every PR; the long-running matrix runs nightly.

## Gates

| Job | Purpose | Failure means |
|-----|---------|---------------|
| `forbidden-term-scan` | Runs `scripts/ci_forbidden_terms.py` against the production crate roots declared in `config/forbidden-terms.toml`. | Someone introduced `StubVault`, `MemoryAuditWriter`, `AcceptAllBundleVerifier`, `NullSealer`, `dashboard-dev`, or `allow_demo_defaults` outside the per-term allowlist. Either remove the use or add the file to `allowed_paths` with rationale. SHIP-16 closes the allowlists permanently. |
| `ship-validator` | Builds `mai-ship-validate` (SHIP-07B) and asserts the documented exit-code matrix against `deployment/ship/profile.toml` and `config/production.example.toml`. | The production profile no longer parses as ship-ready in config-only mode, OR the validator no longer honors the exit-code contract (0/2/3 paths probed explicitly). |
| `mai-admin-backup` | Runs `cargo test -p mai-admin --locked`, which exercises the SHIP-09 create + verify round-trip end-to-end. | Backup manifest, signing, or replay logic regressed. |
| `package-build-validate` | Runs `scripts/build-package.sh --validate-only --skip-dashboard`, then the static suites in `tools/packaging_tests/` and `tools/ship12_tests/`. | systemd, debian, or install-layout drift broke SHIP-08's contract, OR a SHIP-12 gate definition drifted. |
| `compose-trust-validation` | Runs `deployment/appliance/validate_profile.py` against the shipped compositions — `wsf-ha` as `production`, `appliance` and `shadow` as `demo` — plus the validator's own regression tests. (`loom-harness` is the multi-node test estate, exercised by its live gate, not this job.) | A dev-token, ungated demo trust core, or host-published trust-core port was reintroduced into a shipped composition, or the validator itself regressed. |
| `mypy-strict-sdk` | Enforces `mypy --strict mai-sdk-python/src/` AND `mypy adapters/` with no `continue-on-error`. | A new untyped public surface entered the SDK, OR the adapters tree introduced an error severe enough to fail under the per-package overrides recorded in `pyproject.toml`. |
| `nightly-integration` | Schedule-only (03:30 UTC) and `workflow_dispatch`. Runs the full `cargo test --workspace`, every pytest tools/ suite, and the SDK + adapter pytest tree. Depends on every other job above. | A long-running integration regression. The 72-hour burn-in is SHIP-14 and lives in a separate workflow. |

## Triggers

- `push` to `main`
- `pull_request` targeting `main`
- `schedule`: `30 3 * * *` (nightly)
- `workflow_dispatch` for manual reruns

## Exit-code contract (validator probes)

The `ship-validator` job pins the SHIP-07B exit-code matrix:

| Code | Meaning | How CI checks it |
|------|---------|------------------|
| 0 | Ship-ready (config-only or full runtime). | Two clean runs against the two ship-shaped profiles in the tree. |
| 2 | Profile unreadable. | Probed with `--profile /nonexistent/profile.toml`. |
| 3 | State dir missing or not a directory. | Probed with `--state-dir /nonexistent/state`. |

Exit codes 1 (critical fail) and 4 (internal validator error) are covered by
the unit + integration tests inside `mai-api`, not by the CI gates here.

## Forbidden-term scanner

Single source of truth: [`config/forbidden-terms.toml`](../../config/forbidden-terms.toml).

The scanner walks every `*.rs`, `*.toml`, and `*.py` under the declared
roots, applies a literal substring match per term, and ignores files whose
repo-relative path is in that term's `allowed_paths`. The current allowlist
captures every legitimate use on `main` (builder error message strings,
type re-exports, the legacy no-profile bring-up path, the production-guard
check IDs). SHIP-16 will physically delete the underlying types and shrink
each `allowed_paths` to empty.

Local invocation:

```bash
python3 scripts/ci_forbidden_terms.py
python3 scripts/ci_forbidden_terms.py --json   # machine-readable
```

## Adding a new gate

1. Add the job to `ship-validation.yml`. Keep it under 5 minutes wall time
   so PR feedback stays fast.
2. Add a row to the gate table above.
3. Add a static test in `tools/ship12_tests/` that asserts the job exists
   in the workflow and runs the expected command.
4. If the gate requires a script, put the script under `mai/scripts/` and
   make it idempotent (exit 0 on success, non-zero with a clear message
   on failure).

## Anchors

- Workflow: `.github/workflows/ship-validation.yml`
- Scanner: `scripts/ci_forbidden_terms.py`
- Scanner config: `config/forbidden-terms.toml`
- Static gate tests: `tools/ship12_tests/`
- Profile fixtures: `deployment/ship/profile.toml`, `config/production.example.toml`
- Validator binary: `mai-api/src/bin/mai_ship_validate.rs` (SHIP-07B)
- mypy config: `pyproject.toml` (`[tool.mypy]` plus per-package overrides)
- SHIP-08 packaging gate (re-used in nightly): `tools/packaging_tests/`
- SHIP-13 release gate (re-used in nightly): `tools/gpu_release_tests/`
