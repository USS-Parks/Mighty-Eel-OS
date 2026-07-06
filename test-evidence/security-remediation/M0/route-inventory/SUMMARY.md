# 0.3 — Route inventory + privilege matrix

Gate: `.integrity/scripts/route-policy-check.sh` vs `.integrity/route-policy.tsv`
(79 production HTTP routes; the policy file is derived from the live route
extraction, so it is provably complete against source).

- Positive (current tree): `route-policy: OK — 79 routes declared`. exit 0.
- Negative control: dropped the `/v1/tokens/attenuate` row -> gate exit 1 naming
  the undeclared route (`+ /v1/tokens/attenuate`). Restored -> exit 0.
- Enforcement: wired at pre-push (`.integrity/hooks/pre-push`) beside the no-slop
  full-tree scan. No GitHub Actions workflow exists in this repo (`.github/` empty);
  the script is CI-portable if one is added.

Extraction: perl slurp-mode (multi-line aware), scanning `crates/wsf-api`,
`crates/aog-gateway`, `crates/aog-approvals`, `crates/aog-toolproxy`, `mai-api`;
tests / benches / mocks excluded.

Full per-service inventory (HTTP / gRPC / SSE / WS / CLI) + privilege matrix:
`docs/scans/SECURITY-ROUTE-INVENTORY.md`. Raw gate logs: `gate-pass.log`,
`gate-fail-negative-control.log` (local; `*.log` is gitignored).
