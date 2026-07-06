# Security Remediation — Evidence Bundle

Evidence for the MAI / WSF / AOG security remediation lane. Layout:

    test-evidence/security-remediation/
      M0/  containment + baseline          M3/  repository closure (F, Q)
      M1/  trust plane (A, T, E, B, R, L)  M4/  re-ship (X)
      M2/  vault truth (V)

Per-prompt evidence records (plan Appendix C): prompt id + objective; pre-change failing
test or static proof; changed files; exact commands + exit codes; focused + workspace test
counts; live-service versions + endpoints; negative-control evidence; migration /
compatibility effect; remaining risks; commit scope then SHA.

Hard rule (plan §0.5): evidence carries metadata and logs only — never a bearer secret,
private key, plaintext envelope, model weight, cloud credential, or regulated payload.
Denial paths are receipted without sensitive payloads.

## M0/baseline

Verify-ladder state captured at `6ffaaee` before any change — see
[M0/baseline/SUMMARY.md](M0/baseline/SUMMARY.md) and the per-gate logs. The red gate here
is the reproduction of AQ-001 (clippy `doc_lazy_continuation`); fmt / check / test are green.
