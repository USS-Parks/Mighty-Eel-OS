# Phase T — Test-assertion backfill (X4/X5 close-out, M7-T)

Executed T1→T4 STS on 2026-07-10. Objective: every denial the audit relied on
is asserted by a test that fails if the control regresses.

## T1 — Admin mutation without the admin role → 403 (`aogd`)
- New `crates/aogd/tests/admin_auth.rs`
  (`admin_mutation_without_admin_role_is_403`): a real daemon with a
  provisioned anchor; `POST /admin/write` refused 401 with no token; refused
  **403 naming `aog-admin`** with a VALID authenticated token lacking the
  role; an `aog-admin` token passes the trust gate and fails 503 (no leader
  on the unformed node) — proving the 403 was the role check, not a blanket
  refusal; read-only `/admin/leader` stays open by design.
- Result: 1 passed (in-process, no live dependency).

## T2 — Cross-tenant delete / kill-reversal → denied (`aog-apiserver`)
- New `cross_tenant_delete_is_denied` in `crates/aog-apiserver/tests/crud.rs`:
  tenant-loom creates a `RevocationIntent` (the sharpest instance — deleting
  another tenant's intent would reverse a live kill); tenant-mallory's
  DELETE is refused **403**; the intent still reads 200 for its owner
  (nothing changed); the owner's own delete proceeds (the denial is the
  tenant binding, not a blanket delete refusal).
- Result: crud suite 7 passed (in-process).

## T3 — AF-10: legacy `/v1/completions` through the full governed pipeline (`aog-gateway`)
- New `crates/aog-gateway/tests/completions_legacy.rs`
  (`legacy_completions_runs_the_full_governed_pipeline`), env-gated on
  `WSF_OPENBAO_ADDR` like every gateway live gate (self-skips in the unit
  lane): 401 unauthenticated; 200 in the legacy wire shape
  (`text_completion`, `choices[0].text`, usage totals) carrying the
  governance headers (`x-aog-route`, `x-aog-policy-mode`); the call is
  metered (`/v1/usage`: 1 call / 45¢, receipt chain verifies); a PHI-shaped
  prompt reaches the SAME policy outcome (status + policy headers) on the
  legacy path as on `/v1/chat/completions` — the parity that makes the
  legacy endpoint governed rather than a side door; and no upstream request
  ever carries the raw SSN (mock upstream captures every body).
- LIVE RESULT (the AF-10 regression test): **legacy-completions live gate
  PASSED** against `http://127.0.0.1:18200` (Dockerized OpenBao dev, isolated
  container `openbao-t34`). (The finding ID AF-10 is the audit reference; the
  test's own output carries no finding ID, per the no-slop provenance rule.)

## T4 — AF-17: usage/ROI two-tenant scoping (`aog-gateway`)
- New `crates/aog-gateway/tests/tenant_isolation.rs`
  (`usage_and_roi_are_scoped_to_the_calling_tenant`), same live gating: two
  virtual keys under tenant-a / tenant-b against ONE shared gateway; 2 calls
  as a, 1 as b. tenant-a's `/v1/usage` shows only tenant-a aggregates
  (2 calls / 90¢), tenant-b's only its own (1 call / 45¢); `/v1/roi` is
  computed over the caller's own spend only (`cloud_spend_cents` 90 vs 45).
  No cross-tenant leakage in either view.
- LIVE RESULT (the AF-17 regression test): **tenant-isolation live gate
  PASSED** against `http://127.0.0.1:18200`.

## Verify
- `cargo fmt --check` clean; clippy (-D warnings -A pedantic) clean on
  `aogd`, `aog-apiserver`, `aog-gateway`.
- Live gates: both new tests PASSED against Dockerized OpenBao
  (`openbao/openbao` dev, loopback :18200, root token, fresh mounts per run).
- Workspace (OpenBao env unset, live tests self-skip):
  `cargo test --workspace` → **2297 passed / 0 failed / 8 ignored**
  (233 suites).
- `cargo audit` clean; `cargo deny check` advisories/bans/licenses/sources
  ok; ruff clean; detect-secrets baseline pass; gitleaks full-tree 0;
  no-slop full-tree clean.

Commits: gated — recorded in the DEVLOG once Basho approves the commit plan.
