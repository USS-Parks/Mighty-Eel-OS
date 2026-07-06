# Security Remediation DEVLOG

Initiative: MAI / WSF / AOG security remediation (Critical/High trust-boundary closure).
Repository: im-mighty-eel-mai. Worktree: `mai-worktrees/mai-SEC-1`, branch `session/SEC-1`.
Baseline: `6ffaaee` (`cleanup/artifact-audit`) — the 2026-07-05 audit commit.
Plan of record: [../scans/SECURITY-REMEDIATION-PSPR.md](../scans/SECURITY-REMEDIATION-PSPR.md).
Finding register: [../scans/SECURITY-REMEDIATION-FINDINGS.md](../scans/SECURITY-REMEDIATION-FINDINGS.md).

Each entry records objective, evidence, verify result, and commit SHA per the plan's
Appendix C. Red is reported as red; a skipped step is reported as skipped.

---

## Phase 0 — Containment and evidence freeze

### 0.1 — Remediation lane artifacts + baseline freeze

Objective: stand up the lane (plan tracked in-repo, DEVLOG, finding register, evidence
contract) and freeze a reproducible baseline — HEAD, worktree status, toolchain, gate results.

Environment (win32; `mai-SEC-1` worktree):
- rustc / cargo 1.96.1, rustfmt 1.9.0, clippy 0.1.96
- cargo-audit 0.22.1, cargo-deny 0.19.7
- Python 3.14.4, ruff 0.15.14, mypy 2.1.0, pytest 9.0.3
- gitleaks 8.30.1, detect-secrets 1.5.0
- Docker 29.6.1, Docker Compose v5.3.0 (live-gate infrastructure available)

Baseline HEAD `6ffaaee`; worktree clean at capture (fresh from baseline).

Baseline verify ladder — raw cargo, evidence under
`test-evidence/security-remediation/M0/baseline/`:
- `cargo fmt --check` ................................... PASS (exit 0)
- `cargo check --workspace` ............................. PASS (exit 0)
- `cargo clippy --workspace -- -D warnings -A pedantic`   FAIL (exit 101) — AQ-001:
  `clippy::doc_lazy_continuation`, "doc list item without indentation" at
  `mai-core/src/cache.rs:109`. Reproduced, not introduced; owned by Q1.
- `cargo test --workspace` ............................. PASS (exit 0; 0 failed, all suites)

Not exercised this prompt (captured when their phases open): `cargo audit`, `cargo deny`,
Python gates (ruff / mypy / pytest). AQ-002 (Python) is expected-red per the audit; owned
by Phase Q.

Artifacts:
- `docs/scans/SECURITY-REMEDIATION-PSPR.md` — canonical plan, tracked into SEC-1
- `docs/scans/SECURITY-REMEDIATION-FINDINGS.md` — finding register
- `docs/sessions/SECURITY-REMEDIATION-DEVLOG.md` — this log
- `test-evidence/security-remediation/{M0..M4}/` + `README.md` — evidence contract
- `docs/INDEX.md` — remediation-lane pointer added to the build-state block

Crate to finding map recorded for the trust-boundary phases:
- AF-001 `crates/fabric-token` (`fn attenuate`) + `crates/wsf-api` (attenuation route)
- AF-002 `crates/wsf-api` + `crates/fabric-identity` + `crates/wsf-bridge`
- AF-003 `crates/fabric-envelope` + `crates/wsf-seal`
- AF-004 `crates/wsf-broker`
- AF-005 `mai-vault`
- AF-006 `crates/fabric-revocation`
- AF-007 `crates/wsf-ledger`

Verify: fmt clean; check green; clippy red on the pre-existing AQ-001 only; tests green.
Commit: `21efec1`.

### 0.2 — Emergency WSF exposure containment

Objective: stop unauthenticated host access to the privileged WSF trust plane
before the real authentication fix (Phase A) — contain by network exposure, not
by new auth logic.

Found: the code default and all three deployment composes exposed the privileged
plane. `crates/wsf-api/src/main.rs` defaulted `WSF_LISTEN` to `0.0.0.0:8300`;
`deployment/wsf-ha` (production/HA) host-published wsf-api (8300) and openbao
(8200); `deployment/appliance` + `deployment/shadow` (dev-mode root-token demos)
published wsf-api / gateway / console / openbao on 0.0.0.0.

Changed:
- main.rs default bind 0.0.0.0:8300 -> 127.0.0.1:8300 (production fail-safe;
  explicit WSF_LISTEN widens it behind an ingress).
- wsf-ha: removed the wsf-api + openbao host ports; wsf-api sets
  WSF_LISTEN=0.0.0.0:8300 to bind the internal compose network for the LB only.
- appliance + shadow: all host ports loopback-bound (127.0.0.1); headers marked
  insecure opt-in demos.

Verify: fmt PASS; cargo check --workspace PASS; clippy -p wsf-api PASS (no new
lint; AQ-001 in mai-core still owned by Q1); test -p wsf-api PASS (0 crate-local
tests; workspace suite was green at baseline, re-run at the M0 close). YAML valid
for all three composes. Evidence: test-evidence/security-remediation/M0/containment/.

Gate: an unauthenticated host request cannot reach token issue/attenuate, seal/
unseal, credential exchange, or receipts in the production/HA posture — those
routes are no longer host-published (static + config proof). The live black-box
proof rides the Phase-A ingress gate (A5), once the authenticated ingress exists
and images are built.

Findings: AF-001/002/003/004/006/007 -> CONTAINED (network exposure removed;
root fixes land in their phases). AF-005 untouched (its contain step is 0.5).
AS-001 (floating images) not addressed here — owned by Q7.

Commit: `6daa146`.

### 0.3 — Route inventory + privilege matrix

Objective: a machine-readable inventory of every WSF/AOG/MAI entry point with its
auth/tenant/audit posture, plus a gate that fails when a new privileged route has
no policy row.

Surface (read-only enumeration cross-checked against the WSF router read): 79
production HTTP routes — wsf-api (9, the unauthenticated privileged plane),
aog-gateway (10, per-handler virtual-key `authorize()`), mai-api (60, global
`X-IM-Auth-Token` middleware + per-route `check_permission`; health/metrics
exempt) — plus ~20 mai-api gRPC methods, SSE (`/v1/chat/completions` stream),
WebSocket (`/v1/ws`, post-upgrade auth), and local CLI (mai-admin, mai-api
validate, wsf-seed).

Deliverables:
- `.integrity/route-policy.tsv` — 79-row machine-readable HTTP policy file,
  derived from the live route extraction so it is provably complete.
- `.integrity/scripts/route-policy-check.sh` — perl-based, multi-line aware,
  tests/benches/mocks excluded; asserts every source route has a policy row.
- `.integrity/hooks/pre-push` — invokes the gate next to the no-slop full scan.
- `docs/scans/SECURITY-ROUTE-INVENTORY.md` — human inventory + privilege matrix
  (HTTP/gRPC/SSE/WS/CLI), with the WSF no-auth findings and the
  `/v1/auth/exchange_token` stub + `/v1/ws` post-upgrade flags called out.

Verify: gate exit 0 on the current tree (79/79 declared). Negative control:
dropping the `/v1/tokens/attenuate` row makes the gate exit 1 and name the
undeclared route; restoring returns exit 0. Evidence:
`test-evidence/security-remediation/M0/route-inventory/`.

Notes: no GitHub Actions workflow exists (`.github/` is empty), so the gate rides
the pre-push hook (the repo's Layer-3 owner), not a CI YAML; the script is
CI-portable. The automated gate covers axum HTTP route literals; gRPC/SSE/WS/CLI
are inventoried but not yet auto-gated (F-phase hardening).

Commit: `edc9021`.

### 0.4 — Adversarial regression fixtures

Objective: freeze a deterministic regression identifier per finding, with a
quarantined harness asserting current vulnerable behavior (product tests flip to
repaired behavior in-phase). No `#[ignore]` (§0.5).

Reading fabric-token pinned AF-001 exactly: `attenuate()` checks monotonicity on
routes/models/classification/budget/expiry but never verifies the parent
signature and never constrains tenant_id, roles, service_identity, or
revocation_status. So a fabricated/unsigned/wrong-key parent yields a signed
child (signer oracle), a valid parent can be widened on roles/tenant, and a
revoked parent still attenuates (AF-006).

Delivered:
- `crates/fabric-token/Cargo.toml`: `security-regression` feature (off by default).
- `crates/fabric-token/tests/security_regression.rs`: 5 feature-gated fixtures
  (REG-AF-001 unsigned / wrong-key / role-widening / tenant-swap, REG-AF-006
  revoked-parent) asserting the current vulnerable behavior.
- `docs/scans/SECURITY-REGRESSION-REGISTRY.md`: deterministic id per finding
  (AF-001..007); AF-001 + AF-006 implemented, AF-002/003/004/007 reserved for A/E/B/L.

Verify: fmt PASS; harness `--features security-regression` PASS (5 fixtures);
default `cargo test -p fabric-token` runs 0 of them (quarantined); clippy PASS.
Evidence: `test-evidence/security-remediation/M0/adversarial-fixtures/`.

Gate: every AF finding has a deterministic regression identifier (registry).

Commit: (pending — this change set).
