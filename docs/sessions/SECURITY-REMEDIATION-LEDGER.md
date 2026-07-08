# Security Remediation Ledger

**Source audit:** `AUDIT_REPORT.md` (scan `60c5b33e-4963-4f68-bd7e-bcaa6816f918`)
**Audited revision:** `6ffaaeeea0a83c7fa071e114183cfa60c5898703`
**Snapshot digest:** `codex-security-snapshot/v1:sha256:b920522f2f117347053cfb8f0e35237868c1da3b9743ecd7549edb755bf7ddb4`
**PSPR:** Repository Security Remediation (PSPR-00 … PSPR-31)
**Ledger created:** 2026-07-07 (PSPR-00)
**Release posture:** STOP SHIP until PSPR-31 issues an owner-approved GO.

## Status legend

- `OPEN` — not yet remediated.
- `CODE-FIXED` — root-control change landed + unit/regression proof green, live/final proof still owed.
- `CLOSED` — root fix **and** every listed live/final proof complete with receipts.
- Live-proof columns reference the prompt that supplies black-box / on-disk evidence.

## Reportable findings (24: 11 High, 13 Medium)

| ID | Audit title | Sev | Owner prompt | Root-control location | Live/final proof | Regression test | Evidence path | Migration impact | Residual risk | Status |
|:--|:--|:--:|:--:|:--|:--:|:--|:--|:--|:--|:--:|
| AF-01 | Unauthenticated WSF token issuance grants caller-selected authority | High | PSPR-03 | `crates/wsf-api/src/lib.rs:230` | 28, 31 | `wsf-api` `auth.rs` adversarial matrix (10) | `PSPR-03/` | `issue` now bearer-authed + server-derived; SDK `with_credential`; `IssueReq.issuance_kind` | live 2-tenant OpenBao proof owed (28); other privileged routes authed by PSPR-04/08/09/16 | CODE-FIXED |
| AF-02 | Attenuation signs attacker-constructed child tokens without authenticating parent | High | PSPR-04 | `crates/fabric-token/src/lib.rs:121` | 28, 31 | pending | pending | pending | pending | OPEN |
| AF-03 | gRPC trusts caller-authored administrator metadata | High | PSPR-05 | `mai-api/src/grpc/mod.rs:98` | 28, 31 | pending | pending | pending | pending | OPEN |
| AF-04 | Envelope unseal not bound to tenant/subject/audience/policy | High | PSPR-08 | `crates/wsf-seal/src/lib.rs:305` | 28, 31 | pending | pending | pending | pending | OPEN |
| AF-05 | AWS credential exchange accepts caller-selected role ARN | High | PSPR-09 | `crates/wsf-api/src/lib.rs:333` | 28, 31 | pending | pending | pending | pending | OPEN |
| AF-06 | Production readiness certifies a merely constructed vault | Medium | PSPR-21 | `mai-api/src/server.rs:689` | 29, 31 | pending | pending | pending | pending | OPEN |
| AF-07A | ZFS vault stores/loads model weights as plaintext | Medium | PSPR-19 | `mai-vault/src/zfs.rs:275` | 29, 31 | pending | pending | pending | live ZFS UNAVAILABLE (see infra) | OPEN |
| AF-07B | Vault snapshot/rollback APIs report success without ZFS operations | Medium | PSPR-20 | `mai-vault/src/zfs.rs:453` | 29, 31 | pending | pending | pending | live ZFS UNAVAILABLE (see infra) | OPEN |
| AF-08 | OpenAI streaming bypasses tokenization/metering/receipts | High | PSPR-12 | `crates/aog-gateway/src/surface_openai.rs:176` | 28, 31 | pending | pending | pending | pending | OPEN |
| AF-09 | Anthropic streaming bypasses tokenization/metering/receipts | High | PSPR-13 | `crates/aog-gateway/src/surface_anthropic.rs:133` | 28, 31 | pending | pending | pending | pending | OPEN |
| AF-10 | Legacy OpenAI completions bypass compliance routing/accounting | High | PSPR-14 | `crates/aog-gateway/src/surface_openai.rs:398` | 28, 31 | pending | pending | pending | pending | OPEN |
| AF-11 | Restore manifest component paths can escape backup/target roots | Medium | PSPR-23 | `tools/mai-admin/src/restore.rs:292` | 29, 31 | pending | pending | pending | pending | OPEN |
| AF-12 | Appliance composition publishes dev OpenBao with known root token | High | PSPR-01, 24 | `deployment/appliance/docker-compose.yml:11` | 30, 31 | `deployment/appliance/tests/test_validate_profile.py` (10 cases) | `PSPR-01/` (containment); PSPR-24/30 (prod+live) | demo now `--profile demo` + `.env` injection | **containment landed PSPR-01** (profile-gated, loopback, injected, validator); final closure PSPR-24 | OPEN |
| AF-13 | AOG defaults to non-blocking shadow policy mode | High | PSPR-02 | `crates/aog-gateway/src/main.rs:62` | 28, 31 | `policy.rs` `resolve_mode` matrix (7) + `policy_modes.rs` | `PSPR-02/` | prod unset→enforce; shadow/report dev-only+explicit; demo sets `AOG_PROFILE=development` | live startup-fail proof owed (PSPR-28) | CODE-FIXED |
| AF-14 | WSF receipt queries are unauthenticated and cross-tenant | Medium | PSPR-16 | `crates/wsf-api/src/lib.rs:361` | 28, 31 | pending | pending | pending | pending | OPEN |
| AF-15 | Revocation snapshots lack freshness/scope/anti-rollback | Medium | PSPR-06 | `crates/fabric-revocation/src/lib.rs:151` | 28, 31 | pending | pending | pending | pending | OPEN |
| AF-15B | AOG revocation check fails open when snapshot is absent | Medium | PSPR-07 | `crates/aog-gateway/src/lib.rs:188` | 28, 31 | pending | pending | pending | pending | OPEN |
| AF-16 | AWS credentials can outlive remaining WSF token authority | Medium | PSPR-10 | `crates/wsf-broker/src/lib.rs:245` | 28, 31 | pending | pending | pending | pending | OPEN |
| AF-17A | Usage endpoint returns aggregates for every tenant | Medium | PSPR-15 | `crates/aog-gateway/src/surface_openai.rs:233` | 28, 31 | pending | pending | pending | pending | OPEN |
| AF-17B | ROI endpoint computes recommendations from every tenant | Medium | PSPR-15 | `crates/aog-gateway/src/surface_openai.rs:259` | 28, 31 | pending | pending | pending | pending | OPEN |
| AF-19 | Restore accepts unsigned/unverified manifests by default | Medium | PSPR-22 | `tools/mai-admin/src/main.rs:123` | 29, 31 | pending | pending | pending | pending | OPEN |
| AF-20 | Production-like deployment images use mutable tags | Medium | PSPR-25 | `deployment/wsf-ha/docker-compose.yml:57` | 30, 31 | pending | pending | pending | pending | OPEN |
| DF-01A | Model package signature authenticates weights but not manifest identity | High | PSPR-17 | `mai-core/src/models/verify.rs:120` | 29, 31 | pending | pending | pending | pending | OPEN |
| DF-01B | Manifest-derived model ID can escape the vault root | Medium | PSPR-18 | `mai-vault/src/zfs.rs:275` | 29, 31 | pending | pending | pending | pending | OPEN |

## Deferred runtime surfaces (2)

| ID | Surface | Owner prompt | Root-control location | Live/final proof | Status |
|:--|:--|:--:|:--|:--:|:--:|
| DEF-1 | Adapter resource isolation runtime (CPU/mem/fs/proc/net) | PSPR-26 | `mai-adapters/src/process.rs` | 30, 31 | OPEN |
| DEF-2 | Signed, bounded update transport (SSRF/rollback resistant) | PSPR-27 | `mai-core/src/models/update.rs`; `mai-api/src/handlers/updates.rs` | 30, 31 | OPEN |

## Quality follow-up (1)

| ID | Item | Owner prompt | Root-control location | Status | Evidence |
|:--|:--|:--:|:--|:--:|:--|
| Q-1 | Clippy release gate (`clippy::doc_lazy_continuation`) | PSPR-00 | `mai-core/src/cache.rs:109` (+9 more sites, see note) | **CLOSED** | `test-evidence/security-remediation/PSPR-00/` |

**Q-1 note (audit correction):** The audit reported the Clippy gate failing at a single site
(`mai-core/src/cache.rs:109`). In fact the gate (`cargo clippy --workspace -- -D warnings
-A clippy::pedantic`) aborts at the first failing crate, masking further violations of the same
lint. A forced full re-lint found **10** `doc_lazy_continuation` warnings across three crates
(`mai-core/src/cache.rs`, `mai-scheduler/src/scoring/mod.rs`, `mai-api/src/routes.rs`,
`mai-api/src/server.rs`). All 10 were repaired as documentation-only edits (restore list bullets;
indent numbered sub-steps) with no `#[allow]`, no lint suppression, and no product-behavior change.
Gate now green (exit 0).

## Infrastructure availability (PSPR-00 inventory)

| Capability | Needed by | Status |
|:--|:--|:--|
| Docker daemon | 24, 25, 28 | AVAILABLE (Docker Desktop 29.6.1) |
| OpenBao container image | 03–16, 24, 28 | AVAILABLE (`openbao/openbao:latest` present locally) |
| Moto (AWS mock) | 09, 10, 28 | INSTALLABLE (`boto3` present; `moto` not yet installed) |
| Disposable real ZFS dataset | 19, 20, 29 | **UNAVAILABLE** (Windows host; WSL2 kernel lacks ZFS module) |
| TPM / PCR measurement | 21 | **UNAVAILABLE** in dev environment |
| GCP / Azure emulators | 09 | UNAVAILABLE (not installed) |
| NVIDIA GPU | perf-adjacent | PRESENT (not appliance-class) |

Per PSPR §0.6, live-ZFS-dependent proofs (AF-07A/07B live gates in PSPR-20/29) and TPM-dependent
readiness probes (PSPR-21) are blocked on infrastructure. Their **code-level** remediation still
proceeds against a mocked command seam with unit/property tests; the live/final proof is recorded
UNAVAILABLE until a ZFS/TPM-capable host is provided.
