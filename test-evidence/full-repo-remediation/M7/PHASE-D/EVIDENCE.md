# Phase D — Docs / safe-install truthfulness (X4/X5 close-out, M7-D)

Executed D1→D5 STS on 2026-07-10. Objective: the shipped docs alone yield a
safe install (invariant A9); no shipped default credential.

## D1 — Dashboard token at install
- The stock install ran the dashboard on the `util.py` local-dev default
  (`MAI_DASHBOARD_ADMIN_TOKEN` unset in the unit). Now: `postinstall.sh`
  generates a 64-hex CSPRNG token into `/etc/mai/dashboard.env` (root:mai,
  0640, idempotent across upgrades); `mai-dashboard.service` hard-requires it
  (`EnvironmentFile` without `-` — a missing file refuses startup, never the
  default); the profile declares it (`dashboard.admin_token_file` in
  `deployment/ship/profile.toml` + `config/production.example.toml`); and the
  guard measures it: new deferred runtime check **PROD-DASH-100**
  (`probe_dashboard_admin_token` — file exists, sets the var, non-empty,
  not the local-dev default; token value never echoed), populated by both
  `mai-ship-validate --state-dir` and `MaiServer` boot, so a default token
  fails production readiness closed. CI's config-only validate posture is
  unchanged (the check stays Deferred there, like every `PROD-*-100`).
- Files: `mai-api/src/{ship_profile.rs, production_guard.rs, server.rs,
  bin/mai_ship_validate.rs}`, `mai-api/tests/{ship_convergence.rs,
  ship_07b_endpoints.rs, sealer_bootstrap.rs, vault_bootstrap.rs,
  trust_production.rs}` (fixture field), `packaging/scripts/postinstall.sh`,
  `packaging/systemd/mai-dashboard.service`, both profiles,
  `tools/packaging_tests/{test_systemd_units.py, test_maintainer_scripts.py,
  test_layout.py}`.
- Gate: guard tests (probe unit tests: missing file / default token / empty /
  absent key / disabled / undeclared → FAIL paths; generated token → PASS;
  `default_dashboard_token_blocks_ship_ready` — the config flag alone no
  longer certifies the token). Packaging static tests assert the generation
  (idempotent, urandom, 0640) and the hard EnvironmentFile.

## D2 — Dead `MAI_PROFILE` var killed
- `MAI_PROFILE` is read nowhere (`MAI_SHIP_PROFILE` is the selection
  mechanism — `server.rs::resolve_ship_profile`). Removed it from all four
  systemd units and every operator doc. **Real defect found while fixing:**
  `mai-adapter-manager.service` ran `mai-api --role adapter-manager` with
  only the dead var — no `MAI_SHIP_PROFILE`, so the guard never engaged on
  that process. It now sets `MAI_SHIP_PROFILE=/etc/mai/profile.toml`.
- `deployment/ship/README.md`: the launch example now engages the guard
  (`MAI_SHIP_PROFILE=…`) and documents that a workstation launch fails
  closed by design; `deployment/README.md`: demo launch labeled
  non-production with a pointer at the ship path; `deployment/ship/
  profile.toml` selection comment corrected.
- Gate: `grep -rn 'MAI_PROFILE\b'` over the tree → remaining hits only in
  session plans / audit evidence / scan records (provenance, not operator
  surfaces). Zero in units, deployment docs, packaging.

## D3 — DEPLOYMENT.md production banner
- Top-of-file banner: the Quick Start is the developer posture; production
  follows the ship posture (package install → review profile →
  `mai-ship-validate` → systemd). Quick Start retitled "(developer posture —
  not production)" with an inline warning. Links resolve
  (`deployment/ship/README.md`, `docs/operations/SHIP-PROFILE.md`).

## D4 — Buyer/acquisition docs
- `BUYER-INTEGRATION-GUIDE.md`: deployment-postures matrix gains the `ship`
  row (five profiles, guard-enforced, `MAI_SHIP_PROFILE` under systemd); the
  stale "swap the handler body of `exchange_token`" instruction replaced
  everywhere with the real mechanism — the profile-selected
  `TrustExchangeMode` switch (production always selects the OpenBao bridge;
  no handler edit); SDK touchpoints table updated to match.
- `ACQUISITION-PACKAGE.md`: endpoint list + "not in the box" section updated
  to the `TrustExchangeMode` story; posture list now names all five
  profiles including `deployment/ship`.
- `DEMO-SUITE.md`: audited — carries no profile matrix and no handler-swap
  instruction; no change needed.
- Gate: `grep -riE 'handler.?body|swap.*handler'` over `docs/product/` → zero.

## D5 — SECURITY.md parity (verify-only)
- After R2, "the profile header is **disabled by default**" is TRUE:
  `ApiKeyStore::new()` defaults `allow_internal_profile_header: false` and
  the auth_keys.toml loader is `unwrap_or(false)` under `[settings]` —
  exactly as SECURITY.md describes it. doc == code; **no edit required**.

## Verify
- `cargo fmt --check` clean; `cargo clippy -p mai-api --all-targets -- -D
  warnings -A clippy::pedantic` clean.
- Focused: `cargo test -p mai-api` → 370 passed / 0 failed.
- `pytest tools/packaging_tests tools/ship12_tests` → 170 passed, 1 skipped.
- `python scripts/ci_forbidden_terms.py` → PASS (209 files, 0 disallowed).
- Workspace (OpenBao env unset): `cargo test --workspace` → 2293 passed / 0
  failed at phase close (2297 at run close with Phase T's tests).
- Negative controls: probe tests prove default/missing/empty tokens FAIL;
  `default_dashboard_token_blocks_ship_ready` proves the runtime outcome
  blocks readiness while PROD-DASH-001 alone still passes.

Commits: gated — recorded in the DEVLOG once Basho approves the commit plan.
