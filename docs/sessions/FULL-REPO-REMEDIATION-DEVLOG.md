# Full-Repo Remediation DEVLOG

Initiative: remediation of the 2026-07-08 full-repo audit.
Plan of record: [../audits/2026-07-08-full-repo/REMEDIATION-PSPR.md](../audits/2026-07-08-full-repo/REMEDIATION-PSPR.md).
Audit: [../audits/2026-07-08-full-repo/FULL-REPO-SECURITY-AUDIT.md](../audits/2026-07-08-full-repo/FULL-REPO-SECURITY-AUDIT.md).
Branch: session/AUDIT-FIX-1 (off main @ 700cf2b). Commit per prompt, gates green; no push
without explicit approval. Each entry: objective, evidence, verify result, commit.

---

## Phase 0 - Containment & lane

### 0.1 - Lane artifacts + baseline freeze

Baseline HEAD 700cf2b (== main, post Phase F/X). Toolchain: cargo 1.96.1, clippy 0.1.96,
cargo-audit 0.22.1, cargo-deny 0.19.7, gitleaks 8.30.1, detect-secrets 1.5.0, ruff 0.15.14,
docker 29.6.1 (+ openbao/openbao image), moto 5.2.2, protoc 34.1. Absent on this host:
`zfs`, `/dev/tpm*`, a >=3-node cluster host, artifact-signing infra (owner-lane per PSPR X3/X6, V6).

Baseline gates (captured at 700cf2b during the audit, frozen as the pre-remediation
reference): `cargo clippy --workspace -- -D warnings -A clippy::pedantic` PASS; `ruff
check .` PASS; `cargo audit` PASS (0 vulns / 518 deps); `cargo deny check` PASS; `gitleaks
detect` PASS (no leaks); `detect-secrets` baselined; no-slop full scan PASS (mechanical).

Artifacts: this DEVLOG; the audit report + PSPR (docs/audits/2026-07-08-full-repo/);
evidence tree test-evidence/full-repo-remediation/{M0..M6}. Finding register is the audit
report's tables + the PSPR Appendix A closure matrix.

Verify: branch created clean off 700cf2b; docs land under docs/ (no-slop exempt). Commit: (this change set).