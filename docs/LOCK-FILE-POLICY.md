# Lock-File Policy (MAI)

**Owner:** DOUGHERTY lane J-03 (`docs/dougherty/JOHN-REMEDIATION-PLAN.md` §3 W2)
**Status:** Active 2026-05-24
**Tooling decision:** `pip-tools` for Python, `npm` (lockfileVersion 3) for the MCP server.

## Why we lock

Closes GitDoctor PRJ-004 HIGH (missing dependency lock files). Reproducible installs across developer machines, CI runners, and the J-04 Dockerfile image are required for release and for the J-14 rescan to be apples-to-apples against John's 2026-05-24 baseline.

## Files in scope

| File | Tool | Purpose |
|:--|:--|:--|
| `Cargo.lock` | cargo | Rust workspace lock. Already present and pinned by the workspace `Cargo.toml`. No human action required. |
| `requirements.txt` | hand-maintained | Human-readable runtime dependency set, version ranges, mirrors `pyproject.toml [project.dependencies]`. |
| `requirements-lock.txt` | `pip-compile` | Fully pinned runtime lock with `--generate-hashes`. The artifact CI and Docker install from. |
| `.integrity/mcp-server/package-lock.json` | `npm install --package-lock-only` | Pinned Node deps for the integrity MCP server. lockfileVersion 3. |

Dev-only deps (`pytest`, `ruff`, `mypy`) under `[project.optional-dependencies].dev` are intentionally NOT locked in this lane. If a future session needs a dev lock, generate `requirements-dev-lock.txt` from the dev extras and reference it here.

## Regeneration commands

Run from `mai/`:

```bash
# Python runtime lock (re-derive after editing pyproject.toml [project.dependencies])
python -m piptools compile --generate-hashes --quiet \
    --output-file=requirements-lock.txt pyproject.toml

# MCP server Node lock (re-derive after editing .integrity/mcp-server/package.json)
cd .integrity/mcp-server && npm install --package-lock-only && cd ../..
```

`requirements.txt` is hand-maintained and must be kept in sync with the source of truth (`pyproject.toml`). When `pyproject.toml [project.dependencies]` changes, edit `requirements.txt` to match, then regenerate `requirements-lock.txt`.

## CI / consumer expectations

- CI installs Python deps with `python -m pip install --require-hashes -r requirements-lock.txt`.
- CI installs Node deps with `cd .integrity/mcp-server && npm ci` (uses `package-lock.json`, fails if it drifts from `package.json`).
- The J-04 Dockerfile inherits both lock files via `COPY` before the install step so the build is reproducible.

## When to bump

- Security advisories on a pinned dep: re-run the relevant regeneration command, commit the new lock with a `chore(deps):` prefix.
- Adding a new dep: edit `pyproject.toml` (Python) or `.integrity/mcp-server/package.json` (Node), edit `requirements.txt` if Python, then regenerate the lock.
- Major version bumps: open a separate session with a focused commit; never bundle dep bumps with feature work.

## Footnote

This policy was written under the DOUGHERTY lane in response to John Dougherty's 2026-05-24 tester verdict. The decision to use `pip-tools` over `uv` was driven by portability: `pip-compile` produces a `requirements-lock.txt` that any environment with `pip` can install from, with no requirement to install `uv` on the consumer side. If the team later standardises on `uv`, the conversion is a single regeneration step and a one-paragraph amendment to this document.
