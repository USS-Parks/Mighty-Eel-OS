# Security Remediation — Finding Register

Living register for the MAI / WSF / AOG security remediation lane. Source audit:
2026-07-05 repository audit at `6ffaaee` (`cleanup/artifact-audit`). Plan of record:
[SECURITY-REMEDIATION-PSPR.md](SECURITY-REMEDIATION-PSPR.md). Work log:
[../sessions/SECURITY-REMEDIATION-DEVLOG.md](../sessions/SECURITY-REMEDIATION-DEVLOG.md).

Status values: OPEN (unstarted), CONTAINED (M0 mitigation in place, root fix pending),
FIXED (root controls landed + focused tests green), PROVEN (live-service gate green),
CLOSED (independent re-scan confirms, per Appendix D). No finding reaches CLOSED on
documentation or mock-only tests where a live boundary exists (plan §0.4).

## Findings (plan §1)

| ID | Sev | Finding | Primary controls | Status |
|----|-----|---------|------------------|--------|
| AF-001 | Critical | Attenuation signs attacker-constructed children without authenticating or fully constraining the parent | `fabric-token::attenuate`, WSF attenuation route | PROVEN (T1–T4, T7) |
| AF-002 | High | Public WSF route issues signed tokens for caller-selected subjects and roles | WSF router, principal derivation, bridge issuance | PROVEN (A1–A5) |
| AF-003 | High | Envelope unseal lacks tenant/subject binding | envelope contract, AAD/thread, seal service | PROVEN (E1–E7 complete: per-tenant Transit keys, migration, receipts) |
| AF-004 | High | Credential broker accepts caller-selected AWS role | broker policy, role/action/resource binding | PROVEN (B1–B6 complete: grant-bound actions/region/external-id/TTL, credential hygiene) |
| AF-005 | High | Production readiness certifies uninitialized / plaintext-capable vaults | vault builder, ZFS initialization, readiness | FIXED (V1 backend policy, V2/V3 initialized construction blocks bind, V4 sealed-at-rest storage, V5/V6 ZFS ops, V7 cryptographic erasure, V8 measured probe; V9 restart/migration live gate open) |
| AF-006 | Medium | WSF privileged consumers ignore signed revocation snapshots | token verification context, snapshot store | PROVEN (R1 anti-rollback store + seal/broker fail-closed consumers + R6 live gate) |
| AF-007 | Medium | Receipt ledger is unauthenticated and not tenant-filtered | ledger query authz, tenant index | PROVEN (L1/L2 + E6 binding + auditor-only signed export, L4 live gate; L3 durable backend = ops plumbing) |
| AQ-001 | Quality | Clippy gate fails: `clippy::doc_lazy_continuation` at `mai-core/src/cache.rs:109` | Rust CI | FIXED (workspace clippy clean under CI flags `-D warnings -A clippy::pedantic`; the cache.rs doc list is properly continued) |
| AQ-002 | Quality | Whole-tree Ruff / mypy / pytest gates fail or do not collect reliably | Python packaging and CI | FIXED (SDK pytest self-contained via `pythonpath=["src"]`; whole-tree ruff clean; 1310 tests collect with 0 errors; SDK 179 pass + mypy clean) |
| AS-001 | Supply chain | Deployment uses floating image tags and unpinned base-image digests | Docker/Compose/release provenance | OPEN |

## Closure matrix (plan Appendix A)

| Finding | Contain | Root fix | Live proof | Final closure |
|---------|---------|----------|------------|---------------|
| AF-001 | 0.2 | T1–T6 | T7 | X4 |
| AF-002 | 0.2 | A1–A4 | A5 | X4 |
| AF-003 | 0.2 | E1–E6 | E7 | X4 |
| AF-004 | 0.2 | B1–B5 | B6 | X4 |
| AF-005 | 0.5 | V1–V8 | V9 | X3 / X4 |
| AF-006 | 0.2 | R1–R5 | R6 | X4 |
| AF-007 | 0.2 | L1–L3 | L4 | X4 |
| AQ-001 / AQ-002 | 0.1 | Q1–Q4 | X2 | X6 |
| AS-001 | 0.2 | Q7 | X2 | X6 |

## Stop-ship conditions (plan §0.6 — any one blocks release)

Any Critical/High open; WSF privileged routes reachable without an authenticated principal;
a fabricated/invalid parent yields a signed child; one tenant can decrypt or query another's
data; a token selects an unapproved cloud identity; a revoked token stays usable past the
propagation bound; production validation passes an uninitialized / plaintext / dev vault;
required live gates are skipped; the final scan lacks high-impact coverage without an
owner-signed deferral.
