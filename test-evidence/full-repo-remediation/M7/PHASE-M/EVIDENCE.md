# Phase M — Medium trust-plane + adapter hardening (X4/X5 close-out, milestone M7-M)

**Objective.** Close the six medium findings from the 2026-07-09 X4/X5 revalidation's
attack-surface (X4-A F2/F3/F4/F5/F6) and Python (X4-E) slices.

**Source.** `X4-X5-REVALIDATION-REPORT.md`; roster `PLANNING/X4-X5-CLOSEOUT-PSPR.md` Phase M.
Base for this change set: `57161aa` (Phase R + records). Executed M1→M6 STS.

In-code vocabulary carries no roster step-codes (CANON §11 / no-slop PROV gate); the
M#→finding mapping lives here, in the DEVLOG, and in git history.

---

## M1 — Bind `kind` into the signed credential preimage (X4-A F2)

**Defect.** `WorkloadCredential::kind` (`#[serde(default)]`) decides issuance-mode gating
downstream but was **not** in `signing_bytes()`, so a caller could flip `kind` on an
authority-signed credential and it still verified.

**Fix.** `crates/wsf-api/src/auth.rs` — the `kind` tag is now a length-prefixed field of the
preimage, and the domain tag bumped `…/v1`→`/v2` so a v1 signature is never read against the
v2 layout.

**Tests (new, `auth::tests`):**
- `flipping_kind_invalidates_the_signature` — mint a Workload credential, flip `kind`→Human
  without re-signing, authenticate → `UntrustedCredential` (401). Untampered baseline passes.
- `kind_participates_in_the_preimage` — two credentials identical but for `kind` have distinct
  `signing_bytes()`.

## M2 — Deprovision snapshot completeness (X4-A F3)

**Defect.** `TenantAdmin::deprovision` emitted a snapshot with `sequence: 0` and no
`revoked_tenants`, so a consumer holding any snapshot rejected it as a non-advancing rollback,
and tokens the caller did not enumerate stayed valid.

**Fix.** `crates/wsf-tenants/src/lib.rs` — snapshot construction extracted to a pure
`build_deprovision_snapshot(...)`: a monotonic sequence derived from the deprovision timestamp
(ms) and the tenant pushed into `revoked_tenants`. (ponytail: a per-store persistent counter is
the noted upgrade path if a non-wall-clock global publisher ordering is ever needed.)

**Tests:**
- `deprovision_snapshot_advances_sequence_and_revokes_the_tenant` (new, unit) — sequence > 0,
  strictly higher for a later `now`, `is_tenant_revoked` true for the tenant only, enumerated
  tokens still revoked.
- Live: `wsf-tenants --test live_tenants` (deprovision end-to-end) green against OpenBao.

## M3 — Broker parity via server-resolved grant scope (X4-A F4)

**Defect.** The Azure/GCP brokers took a **caller-named** cloud identity (Azure OAuth `scope`;
GCP `service_account` + `scopes`), while AWS took a server-resolved `GrantScope`.

**Fix.**
- `crates/wsf-broker/src/lib.rs` — new `AzureGrantScope { scope }` / `GcpGrantScope
  { service_account, scopes }` typed server-side scopes (parity with `GrantScope`).
- `azure.rs` / `gcp.rs` — `acquire_token` / `generate_access_token` take `&AzureGrantScope` /
  `&GcpGrantScope` instead of raw caller strings.
- `crates/wsf-api/src/grants.rs` — `CloudGrant` gains a `GrantCloud` target (default `Aws`)
  with `to_azure_scope()` / `to_gcp_scope()`, so the tenant-scoped `grant_id` → scope
  indirection now covers all three clouds; the AWS `to_scope()` path is untouched.
- `crates/wsf-broker/tests/{live_azure,live_gcp}.rs` — updated to present the scope types.

**Tests:**
- `grants::tests::azure_and_gcp_grants_resolve_to_broker_scopes` (new) + the existing AWS scope
  test extended (`to_azure_scope`/`to_gcp_scope` are `None` for an AWS grant).
- Live: `wsf-broker --test live_azure --test live_gcp` green (self-mocked endpoints); AWS
  `live_localstack` green (no regression).

## M4 — seal/unseal tenant binding (X4-A F5)

**Defect.** The `seal`/`unseal` handlers did not bind the presented token's tenant to the
authenticated principal, unlike `exchange`. A principal could seal/unseal under a token minted
for another tenant.

**Fix.** `crates/wsf-api/src/lib.rs` — `seal`/`unseal` take `Extension<WsfPrincipal>` and route
through a new `enforce_token_tenant(principal, token)` (WSF-plane + tenant-match, 403 on
mismatch); `exchange` refactored onto the same helper (parity, single source of truth).

**Tests:**
- `tenant_binding_tests::seal_unseal_refuse_a_cross_tenant_token` + `a_non_wsf_principal_is_refused`
  (new, unit) — cross-tenant token → 403; non-WSF principal → 403; same-tenant → Ok.
- Live: `wsf-api --test live_api` (seal/unseal + exchange, single tenant) green.

## M5 — Redacted `Debug` for HMAC keys (X4-A F6)

**Defect.** `TenantRecord` (wsf-tenants) and `TenantAttributes` (wsf-bridge) derived `Debug`,
so `{:?}` printed the raw `subject_hmac_key`.

**Fix.** Hand-written `Debug` on both, rendering `subject_hmac_key` as `<redacted>`
(`TenantAttributes` renders `None` when absent). Serialize/PartialEq/Eq/Clone unchanged.
*Files:* `crates/wsf-tenants/src/lib.rs`, `crates/wsf-bridge/src/openbao.rs`.

**Tests (new, unit, one per struct):** `debug_redacts_the_subject_hmac_key` — `{:?}` never
contains the key bytes, contains `<redacted>`, still renders non-secret fields.

## M6 — Bound the adapter IPC frame (X4-E)

**Defect.** `adapters/runner.py` read request lines with asyncio's default 64 KiB
`StreamReader` limit while `base.py` advertised prompts up to 200 K chars — a prompt between
those sizes overran the reader and crashed the (restarted) worker.

**Fix.**
- `adapters/base.py` — the prompt/embeds caps hoisted to named constants
  (`MAX_PROMPT_CHARS = 200_000`, `MAX_EMBED_TEXT_CHARS`, `MAX_EMBED_TOTAL_CHARS`).
- `adapters/runner.py` — `StreamReader(limit=MAX_FRAME_BYTES)` with `MAX_FRAME_BYTES = 8 MiB`
  (aligned to the Rust orchestrator's frame cap; asserted > `MAX_PROMPT_CHARS × 4`), and the
  request loop catches the overrun `ValueError` from `readline()` (buffer already cleared),
  logs a bounded error, and **continues** — the worker survives an oversize frame instead of
  terminating.

**Tests (new, `adapters/tests/test_runner_frame_limit.py`):**
- `test_max_prompt_frame_round_trips` — a 200 K-char prompt framed as one NDJSON line reads
  back intact under the cap.
- `test_oversize_frame_raises_and_reader_recovers` — an over-limit frame raises `ValueError`
  and the reader recovers so the next frame parses (the loop's survival contract).
- `test_frame_cap_exceeds_the_advertised_prompt_cap`.

---

## Files changed

`crates/wsf-api/src/{auth.rs, lib.rs, grants.rs}`,
`crates/wsf-tenants/src/lib.rs`, `crates/wsf-bridge/src/openbao.rs`,
`crates/wsf-broker/src/{lib.rs, azure.rs, gcp.rs}`,
`crates/wsf-broker/tests/{live_azure.rs, live_gcp.rs}`,
`adapters/{runner.py, base.py}`, `adapters/tests/test_runner_frame_limit.py` (new).
No dependency or lockfile changes.

## Commands and exit codes

| Command | Result | Exit |
|---|---|---|
| `ruff check adapters/{runner,base}.py adapters/tests/test_runner_frame_limit.py` | All checks passed | 0 |
| `pytest adapters/tests/test_runner_frame_limit.py adapters/tests/test_ipc_protocol.py` | 33 passed | 0 |
| `cargo fmt --check` (4 crates) | clean | 0 |
| `cargo clippy -p wsf-api -p wsf-tenants -p wsf-bridge -p wsf-broker --all-targets -- -D warnings -A clippy::pedantic` | no issues | 0 |
| `cargo test -p wsf-api -p wsf-tenants -p wsf-bridge -p wsf-broker` | 87 passed, 23 suites | 0 |
| Live: `wsf-api::live_api` + `wsf-tenants::live_tenants` (OpenBao+Moto) | 2 passed | 0 |
| Live: `wsf-broker::{live_azure, live_gcp, live_localstack}` | 3 passed | 0 |
| `cargo test --workspace` (live tests self-skipping) | 2286 passed, 0 failed, 8 ignored (230 suites) | 0 |
| `cargo audit` / `cargo deny check` | no vulnerabilities / ok, ok, ok | 0 |
| `gitleaks stdin` (change set) + `detect-secrets` | no leaks / no findings | 0 |
| `.integrity/scripts/no-slop-scan.sh full` + `verify-tree.sh` (13 files) | clean / 13-of-13 pass | 0 |
| Independent post-write integrity subagent | SAFE TO STAGE | — |

No `Cargo.lock` change (Phase M adds no dependencies).

## Live gates

Dockerized OpenBao (dev) at `127.0.0.1:8200` + Moto STS at `127.0.0.1:5566`, per the CI recipe.
M2's deprovision, M4's seal/unseal tenant binding, and M3's Azure/GCP brokers all exercised
against live services (mock-only would not close these trust-boundary prompts): `live_api` +
`live_tenants` = 2 passed; `live_azure` + `live_gcp` + `live_localstack` = 3 passed.

## Workspace-run notes (environment, not code)

Two `cargo test --workspace` artifacts were diagnosed to the host, not this change set:
1. **Live-test cross-contamination.** Run with the OpenBao env set, `aog-controller`'s two
   `live_deploy` tests (replica-token placement/scale-down — code Phase M does not touch)
   failed on shared-OpenBao state; both pass in isolation single-threaded against a fresh
   OpenBao (`cargo test -p aog-controller --test live_deploy -- --test-threads=1` = 2 passed).
   CI avoids this by running each live suite as its own invocation with fresh setup. The
   contention-free workspace number above is therefore run with the OpenBao env **unset**, so
   every live test self-skips; the live side is covered by the isolated gates above.
2. **MSVC PDB-link failures under a full disk.** `LNK1201`/`LNK1318` on the `target/` dir at
   194 GB with 2.1 GB free — a literal out-of-disk at link time. Cleared by removing the 44 GB
   `target/debug/incremental` cache (safe, regenerated); the run then links clean.

## Negative controls observed

- M1: a `kind`-flipped, authority-signed credential is refused 401 (not silently trusted).
- M2: a deprovision snapshot's sequence advances past the seq-0 baseline; the tenant is revoked
  on the tenant dimension (tokens the caller never enumerated are refused).
- M3: an AWS grant resolves to no Azure/GCP scope; the brokers no longer accept a caller string
  (compile-enforced) — only a grant-resolved scope.
- M4: a cross-tenant token is refused 403 at the seal/unseal handler; a non-WSF principal 403.
- M5: `{:?}` on either struct renders `<redacted>`, never the key bytes.
- M6: an oversize frame raises and the reader recovers — the worker keeps serving.

## Commits

Approved by Basho; pushed to `origin/main` (branch `session/AUDIT-FIX-2`, base `57161aa`):

- `76fb564` — remediation(M1): bind the credential kind into the signing preimage
- `c15631c` — remediation(M4): bind seal/unseal to the authenticated principal's tenant
- `30073bb` — remediation(M3): route the Azure/GCP brokers through server-resolved grant scopes
- `b8e4a6d` — remediation(M2,M5): complete the deprovision snapshot and redact HMAC-key Debug
- `8773e3d` — remediation(M6): bound the adapter IPC frame

(M2 and M5 share `crates/wsf-tenants/src/lib.rs`; non-interactive staging cannot split one
file's hunks across two commits, so they land together.)
