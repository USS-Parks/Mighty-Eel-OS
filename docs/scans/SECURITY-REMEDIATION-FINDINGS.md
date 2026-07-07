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
| AF-001 | Critical | Attenuation signs attacker-constructed children without authenticating or fully constraining the parent | `fabric-token::attenuate`, WSF attenuation route | FIXED (T1–T4; live T7 → PROVEN deferred) |
| AF-002 | High | Public WSF route issues signed tokens for caller-selected subjects and roles | WSF router, principal derivation, bridge issuance | FIXED (A1–A3; live A5 → PROVEN deferred) |
| AF-003 | High | Envelope unseal lacks tenant/subject binding | envelope contract, AAD/thread, seal service | FIXED (E1/E3/E4; E2 per-tenant Transit + live E7 → deferred) |
| AF-004 | High | Credential broker accepts caller-selected AWS role | broker policy, role/action/resource binding | FIXED (B1–B3; B4 GCP/Azure + live B6 → deferred) |
| AF-005 | High | Production readiness certifies uninitialized / plaintext-capable vaults | vault builder, ZFS initialization, readiness | FIXED (V1 reject stub/file-dev + V8 measured vault_opened; deep proofs V2–V9 → live gate deferred) |
| AF-006 | Medium | WSF privileged consumers ignore signed revocation snapshots | token verification context, snapshot store | FIXED (T1 context + R1 store + R3 seal/broker consumers; GCP/Azure + live R6 → deferred) |
| AF-007 | Medium | Receipt ledger is unauthenticated and not tenant-filtered | ledger query authz, tenant index | FIXED (L1/L2; L3 persistent HA + live L4 → deferred) |
| AQ-001 | Quality | Clippy gate fails: `clippy::doc_lazy_continuation` at `mai-core/src/cache.rs:109` | Rust CI | RESOLVED (clippy `--workspace -D warnings -A pedantic` green at HEAD) |
| AQ-002 | Quality | Whole-tree Ruff / mypy / pytest gates fail or do not collect reliably | Python packaging and CI | OPEN |
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
