# Security Remediation — Closure Report (M1 root-fix milestone)

Initiative: MAI / WSF / AOG security remediation (P-SPR
[SECURITY-REMEDIATION-PSPR.md](SECURITY-REMEDIATION-PSPR.md)).
Branch: `claude/repository-security-audit-2trwtq`. This report is the honest
accounting of what this execution pass landed and what remains.

## Headline

**All seven audit findings (AF-001…AF-007) now have their root controls FIXED,
each with offline proof (unit / property / integration tests) and committed +
pushed.** The two quality findings and the supply-chain finding are partially
closed. **No finding is CLOSED**: closure per the plan's Definition of Done
requires the live-service gates, an independent external re-scan, burn-in, and an
owner go/no-go — none of which this pass can perform (they need live
OpenBao/Moto/ZFS/TPM infrastructure and external parties). This is a **root-fix +
offline-proof milestone (M1)**, not a ship decision.

## Finding status

| ID | Sev | Status | Root fix (phase) | Offline proof | Deferred live gate |
|----|-----|--------|------------------|---------------|--------------------|
| AF-001 | Critical | FIXED | T1–T4: attenuate authenticates the parent (sig/expiry/revocation) + full monotonicity + lineage bound | `fabric-token` regression fixtures flipped to reject; property tests | T7 live OpenBao attenuation |
| AF-002 | High | FIXED | A1–A3: `WsfPrincipal` + authenticator seam; issuance derives identity from the verified principal | `wsf-api` auth unit + `auth_gate` (401 before bridge) | A5 two-tenant live issuance |
| AF-003 | High | FIXED | E1/E3/E4: envelope tenant/owner binding in AAD + signed thread; unseal enforces it | `fabric-envelope` + `wsf-seal` `tenant_binding` (cross-tenant/owner/unbound refused) | E7 live Transit; E2 per-tenant key |
| AF-004 | High | FIXED | B1–B3: tenant-scoped named grant; server-side grant policy; no caller role ARN | `wsf-broker` (unknown/cross-tenant grant denied) | B6 Moto; B4 GCP/Azure parity |
| AF-005 | High | FIXED | V1 reject stub/file-dev in prod; V8 measured `vault_opened` | `mai-api` `vault_bootstrap` (prod file-dev rejected) | V2–V9 real construction / ZFS / TPM |
| AF-006 | Medium | FIXED | T1 context + R1 store + R3 seal/broker consumers consult revocation | `fabric-revocation` store + seal/broker revoked-token refused | R6 revoke-by-dimension; remaining consumers |
| AF-007 | Medium | FIXED | L1/L2: authenticated, tenant-scoped receipt query; no field oracle | `wsf-ledger` isolation + `wsf-api` receipts 401/tenant-scoped | L4 two-tenant live; L3 persistent HA ledger |
| AQ-001 | Quality | RESOLVED | clippy gate green at HEAD (already fixed upstream) | `cargo clippy --workspace -D warnings -A pedantic` exit 0 | — |
| AQ-002 | Quality | PARTIAL | Q3 ruff green (deployment mock annotations/bind fixed) | `ruff check .` exit 0 | Q2/Q4 mypy + full pytest topology repair |
| AS-001 | Supply chain | PARTIAL | CI ghcr repo lowercased (prior commit) | — | Q7 compose digest pinning + SBOM + signing (release pipeline) |

## Verification (this Linux CI container)

Green throughout, run per phase: `cargo fmt --check`; `cargo check --workspace`
(and `--all-targets` per phase); `cargo clippy --workspace -- -D warnings -A
clippy::pedantic`; `bash .integrity/scripts/route-policy-check.sh` (79/79);
`ruff check .`; and focused `cargo test -p <crate>` for every changed crate
(fabric-token, fabric-contracts, fabric-envelope, fabric-revocation, wsf-api,
wsf-seal, wsf-broker, wsf-ledger, aog-apiserver, aog-controller, aog-node,
mai-api). Per-phase evidence: `test-evidence/security-remediation/M1/phase-*/`.

**Not run here:** `cargo test --workspace` compiles all ~40 crates' test binaries
and exhausts this container's disk (rustc-LLVM ENOSPC — infrastructure, not a test
failure); the tests that ran before it filled had zero failures. The full suite is
CI-gated (more disk). All focused suites for changed crates pass.

## Deferred — the honest ledger

These are genuinely out of reach of an offline code pass and remain OPEN:

- **Live-service gates** (A5, T7, E7, B6, R6, L4, V9, X2): every "PROVEN" step
  needs Dockerized OpenBao / Moto / a disposable ZFS+TPM host. The mechanisms are
  wired and offline-proven; the live black-box proofs are not run.
- **Per-provider / per-consumer breadth**: E2 per-tenant Transit keys; B4 GCP/Azure
  named-grant parity; R2/R3 broadened predicate + remaining consumers
  (gateway/tool-proxy/approval); R4 emergency propagation.
- **Vault depth (V2–V7, V9)**: real PQC/TPM construction, init-before-publication,
  encrypted-model round-trip, ZFS property proof, snapshot/rollback, cryptographic
  erasure, restart/migration — all need a live ZFS/TPM environment.
- **Phase F frontier audits (F1–F9)**: the MAI REST/gRPC, AOG gateway, tool-proxy,
  adapter-isolation, package/filesystem, compliance, host/HIL, and deployment/IaC
  deep audits were **not performed** this pass; this pass closed the seven
  enumerated findings, not the frontier review.
- **Python full repair (Q2/Q4)**: ruff is green; mypy topology + whole-tree pytest
  collection (AQ-002) are not repaired.
- **Supply chain (Q7 / AS-001)**: production composes still carry `:latest` for
  first-party + `minio` images; digest pinning + SBOM + signing need the release
  pipeline's built-image digests.
- **Phase X ship gates**: X3 72-hour burn-in, X4 independent external re-scan, X5
  buyer red-team, X6 owner-signed go/no-go — external parties / time / hardware.

## Go / No-Go

- **GO to merge this remediation branch.** It strictly improves the security
  posture (seven trust-boundary root fixes), all repository gates are green, the
  regression fixtures now assert the repaired behavior, and there are no
  regressions in the focused suites.
- **NO-GO for production ship.** The stop-ship conditions in P-SPR §0.6 that require
  *live* validation remain open: the live-service trust gates, the independent
  external re-scan with zero Critical/High, burn-in, and the owner go/no-go. Ship
  only after Phases F/Q(remainder)/X complete on the live lane.
