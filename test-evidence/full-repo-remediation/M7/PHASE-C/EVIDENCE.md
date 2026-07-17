# Phase C — CI / Layer-3 enforcement (X4/X5 close-out, M7-C)

Executed C1→C6 STS on 2026-07-10. Objective: every Layer-3 check runs in CI
against the real artifacts and fails the build on a violation (invariant A10).

## C1 — Profile validator wired into CI + scope gaps closed
- New `compose-trust-validation` job in `.github/workflows/ship-validation.yml`
  (push/PR + nightly `needs`): `validate_profile.py --profile production
  deployment/wsf-ha/…`, `--profile demo deployment/appliance/…`, `--profile
  demo deployment/shadow/…`, plus the validator's own pytest regression suite.
  `deployment/loom-harness` is the multi-node TEST estate (never shipped);
  its exclusion is documented in the job.
- Scope gaps (validate_profile.py): trust-core detection now also matches
  `container_name`, server-only env markers (`BAO_LOCAL_CONFIG`,
  `VAULT_LOCAL_CONFIG`, `*_DEV_ROOT_TOKEN_ID`), and `entrypoint` launch
  tokens (`…/bao`, `…/vault`) — a retagged image no longer evades it. The
  port rule covers EVERY host-published trust-core port (API, cluster 8201,
  remapped), not just 8200: production → none at all; demo → loopback only.
- The shadow lead artifact was brought up to the demo bar it now gates:
  demo credentials injected from `.env` (`:?`-required, never baked — the
  literal `root` token is gone), the stack gated behind the `shadow` compose
  profile, README/env.example updated (3-line bring-up).
- Gate evidence: all three compositions PASS their intended profile locally
  (exit 0 each); validator pytest 75 passed (incl. new cases: entrypoint
  detection, env-marker detection, non-8200 production publish, non-loopback
  demo publish, wsf-ha-as-production, shadow-as-demo + shadow-rejected-as-
  production); ship12 workflow tests pin the job + its three invocations.

## C2 — sbom-sign gated behind phone-home
- `supply-chain.yml`: `sbom-sign` now carries `needs: [phone-home]` — a
  phone-home violation blocks build + publish + sign + attest.

## C3 — gpu-release stops blessing failed builds
- `gpu-bundle`: `if: always() && gpu-build…` replaced with `!cancelled() &&`
  success required from gpu-build, gpu-integration, AND gpu-benchmarks;
  gpu-package must be success or explicitly `skipped` (the skip_package
  dispatch input). A failed gate can no longer produce a signed bundle.
- The readiness/validate step dropped `continue-on-error` — a readiness FAIL
  fails the build (the report still uploads via the `if: always()` step).
- Regression pins added to `tools/gpu_release_tests/test_workflow_yaml.py`:
  no `always()` on gpu-bundle; the ONLY soft step in the lane is the
  advisory benchmark comparison. 75 passed / 11 skipped.

## C4 — Scanner scope
- `.gitleaks.toml`: the blanket `deployment/*-staging/` allowlist (which
  silenced TRACKED staging files) narrowed to the untracked local runtime
  artifacts only (`openbao-tls/`, `openbao-audit/`, `state/`, local
  `bundle.json`). Stale entries fixed for files that moved
  (`docs/scans/LOCAL-GITDOCTOR-*`, `docs/sessions/THREE-LAYER-MANIFOLD-PLAN`),
  and `.secrets.baseline` itself allowlisted (its entries are SHA-1 hashes of
  findings, not findings). Full-tree `gitleaks detect --no-git` → 0 leaks.
- Negative control PROVEN: a planted realistic-shaped secret under
  `deployment/openbao-staging/` is CAUGHT (2 findings; the first attempt
  used AWS's documentation example keys, which the default ruleset
  deliberately ignores — replanted with realistic shapes), and the tree is
  clean again after removal. (`.secrets.baseline` line numbers refreshed by
  the scan run — no entry added or removed.)
- `no-phone-home.sh`: scan scope extended to `mai-*/src` alongside
  `crates/*/src`; vendor regex covers `islandmountain.(io|ai)`; the one
  sanctioned vendor host — the OTA update default
  `updates.islandmountain.ai` (`UpdateClientConfig::base_url`, overridable,
  air-gap-denied) — is excluded by exact host with rationale, so any OTHER
  vendor reference fails; RFC 2606 doc/test hosts skipped;
  `huggingface.co` added to the provider allowlist; comment-only lines
  excluded from the beacon check (matching check 2). Result: PASS over the
  full shipped tree.

## C5 — sign.sh provenance bound
- Keyless verify no longer accepts any identity/issuer. Inside GitHub
  Actions the certificate identity is pinned to the exact signing workflow
  ref (`https://github.com/${GITHUB_WORKFLOW_REF}`); outside, a pinned
  regexp for the canonical `USS-Parks/Mighty-Eel-OS` supply-chain
  workflow on `refs/tags/v*` / `refs/heads/main`. Issuer pinned to
  `https://token.actions.githubusercontent.com` (env-overridable for a
  fork). `bash -n` clean; SUPPLY-CHAIN.md status updated.

## C6 — no-slop bare-.md dangling-ref gap closed
- `no-slop-scan.sh` DOC check: docs/-prefixed citations count anywhere (as
  before); BARE `<name>.md` citations now count on comment lines outside
  test code — in code a bare name is data (fixture paths), in a test comment
  it names a fixture, in a source comment it is a citation and must resolve.
- Self-test extended (7 passed): dangling bare citation flagged; the same
  name as a code string literal passes; a resolving citation passes.
- Both real danglers the new check surfaced were fixed:
  `apps/openbao-trust-demo/config.toml` (dead `BUILD-EXECUTION-PLAN-V2-
  UPDATED.md §777` → cites `docs/compliance/TRUST-MANIFOLD.md` +
  `docs/operations/OPENBAO-INTEGRATION.md`, both tracked) and
  `mai-hil/src/lib.rs` (dead `CONVENTIONS.md` cite reworded). Full-tree
  scan: clean.

## Verify
- Workflow YAML all parses (`yaml.safe_load` × 3); pytest suites green
  (appliance 75, ship12 within the 170 packaging run, gpu_release 75/11
  skipped); ruff clean; `bash -n` clean on both supply-chain scripts;
  no-slop self-test 7/7 + full scan clean; gitleaks full-tree 0.

Commits: gated — recorded in the DEVLOG once Basho approves the commit plan.
