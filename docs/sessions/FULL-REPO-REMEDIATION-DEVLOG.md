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
### 0.2 - Emergency containment (C1/C2)

Objective: stop off-host reach to the unauthenticated `/admin/*` and plaintext `/raft/*`
surfaces before the real auth/mTLS fixes (phase A) - contain by network exposure.

Confirmed: `AOGD_LISTEN` is a required operator-set SocketAddr with no default
(`aogd/src/lib.rs:108`), so a `0.0.0.0` bind exposes both surfaces; `main.rs:20` bound it
with no guard.

Changed (`crates/aogd/src/main.rs`): `check_bind_containment` refuses a non-loopback bind
before `TcpListener::bind` - loopback proceeds; non-loopback (incl. all-interfaces
`0.0.0.0`) fails closed with a message pointing at phase A, unless the operator sets
`AOGD_ALLOW_INSECURE_BIND=1` to accept the risk on an isolated network.

Verify: fmt clean; `cargo clippy -p aogd --all-targets -- -D warnings -A clippy::pedantic`
PASS; `cargo test -p aogd` PASS (new: loopback-ok / non-loopback-refused / opt-in matrix).
C1/C2 CONTAINED (root fixes owned by A1/A2). Commit: (this change set).
## Phase A - AOG control-plane auth & transport

### A1 - authenticate the aogd admin API (C1 root fix)

The `/admin/*` surface was mounted with no auth layer and merged onto the daemon socket
alongside the authenticated `/apis/**` CRUD (`aogd/src/lib.rs`), so any peer could commit
arbitrary Raft `Op`s.

Changed: the admin router now takes the front-door `Authenticator` (threaded from the
daemon's `AppState` when an anchor is provisioned) and gates the mutating routes
(initialize / add-learner / change-membership / write / get) behind a `require_admin`
middleware - a valid WSF token carrying the `aog-admin` role; `/healthz` and read-only
`/admin/leader` stay open. The write leader-forward hop propagates the caller's
`x-wsf-token` so the leader re-authenticates the original caller (the hop is not trusted
until mTLS lands in A2). Pre-anchor bootstrap (no authenticator) relies on the 0.2 loopback
containment.

Verify: fmt; clippy -D warnings; `cargo test -p aogd` PASS (new admin-role gate; existing
daemon/edge/auth_crud suites green - they run anchorless so bootstrap stays open). The full
authenticated-refusal black-box proof is the A6 multi-node live gate (deferred - needs a
>=3-node host). C1 root-fixed at the code boundary. Commit: (this change set).