# M4 (Phase X) - migration, live validation, re-ship gate: evidence

Phase X validated the Phase F fixes + the prior trust-plane remediation. Narrative +
verify results: `docs/sessions/SECURITY-REMEDIATION-DEVLOG.md` (Phase X section).
Owner-facing go/no-go: the top-level `SECURITY-REMEDIATION-FX-REPORT.md`.

## Ran green
- X1 migration/compat: token v1->v2 versioning, envelope, model-storage migration.
- X2 live trust plane (Docker OpenBao + Moto): wsf-api (A5/T7/B6/W6/L4/R6), wsf-seal
  (E7/E2), wsf-broker, aog-gateway (kill-switch/live-gateway), wsf-ledger (W4) - all
  green, no SKIPs. (aog-controller live_deploy passes in isolation; its parallel-run
  failure was a shared-OpenBao concurrency artifact, and no commit touches that crate.)
- X4 independent re-scan: no new reachable Critical/High; two self-gaps found + fixed
  (DF-01A production enforcement wiring, platform-dependent unit test).
- X6 software gates: cargo fmt, clippy -D warnings, cargo test, cargo audit (0 vulns
  over 518 deps), cargo deny (advisories/bans/licenses/sources ok), gitleaks (no leaks
  after the vetted-fixture allowlist), detect-secrets (baselined) - all green.

## Deferred (owner / hardware lane)
- V9 vault live gate on real ZFS+TPM hardware.
- 72-hour burn-in on target hardware.
- Signed-artifact build + SBOM + cosign + CDN ship pipeline (needs signing infra + an
  explicit push).
- Owner formal sign-off + external re-scan (the gate that moves findings to CLOSED).