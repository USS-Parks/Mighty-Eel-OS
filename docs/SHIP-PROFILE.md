# MAI Profile Modes

This document is the operator-facing reference for the four MAI
deployment profiles. It lives next to the execution plan
([SHIP-HARDENING-PLAN.md](SHIP-HARDENING-PLAN.md)) and the code that
parses the new `ship` profile (`mai-api/src/ship_profile.rs`,
introduced in SHIP-01).

## Profile matrix

| Profile             | Audience            | Trust verifier              | Audit storage      | Vault backend | Demo defaults | Bind     |
|---------------------|---------------------|-----------------------------|--------------------|---------------|---------------|----------|
| `local-dev`         | laptop development  | `AcceptAllBundleVerifier`   | in-memory          | `StubVault`   | allowed       | loopback |
| `airgap-demo`       | offline demos       | `MlDsaBundleVerifier`       | local WAL          | `StubVault`*  | demo-scoped   | loopback |
| `cloud-trust-core`  | central Trust Bridge| `MlDsaBundleVerifier`       | central WAL        | OpenBao       | none          | listed   |
| `local-mai-node`    | integration appliance | `MlDsaBundleVerifier`     | local WAL          | local vault   | none          | listed   |
| `ship`              | regulated customer  | `MlDsaBundleVerifier`       | persistent WAL + PQC | real vault   | rejected      | loopback |

* `airgap-demo` historically used a stub vault for portability. The
hardening plan does not change `airgap-demo` semantics — it adds a
strictly stricter posture (`ship`) above it.

## `ship` — the only customer-facing profile

`ship` is the profile installed on appliances delivered to customers.
The hardening plan describes the full set of guarantees; the short
version is:

- `[profile] mode = "production"`, `fail_closed = true`,
  `allow_demo_defaults = false`. Parser rejects any deviation.
- Real vault backend (the reference deployment uses ZFS). `StubVault`
  is rejected.
- Persistent WAL for both API audit and compliance audit, with hash
  chain verification, PQC checkpoint signing, and AEAD encryption at
  rest. `MemoryAuditWriter` and `NullSealer` are rejected.
- ML-DSA trust verifier with anchors on disk and a verified bundle on
  boot. `AcceptAllBundleVerifier` and synthetic local-dev token
  exchange are rejected.
- Non-empty API key store and no internal-profile-header bypass.
- Dashboard enabled but `dashboard-dev` and any default admin token
  are rejected.
- Loopback bind with reverse-proxy TLS termination.
- JSON logs + log rotation + Prometheus metrics + alert rules wired.

The full contract — including the runtime check IDs the production
guard will emit — is in
[SHIP-HARDENING-PLAN.md §1.1](SHIP-HARDENING-PLAN.md) and §3.

## SHIP-01 scope

SHIP-01 introduced parsing only:

- `deployment/ship/profile.toml` — canonical profile.
- `deployment/ship/README.md` — operator-facing summary.
- `config/production.example.toml` — annotated operator template.
- `config/ship-validator.toml` — placeholder for the standalone
  `mai-ship-validate` CLI binary (the SHIP-07 remainder slice).
- `mai-api/src/ship_profile.rs` — typed schema, loader,
  parse-time validator.
- `mai-api/tests/ship_profile.rs` — integration test against the
  on-disk profile.

SHIP-01 explicitly does **not**:

- Wire the parsed profile into `ServerConfig` or `MaiServer` startup.
  That wiring landed in SHIP-07 convergence (see status table below).
- Check that the configured paths exist on disk. That's a runtime
  guard responsibility (SHIP-02 + SHIP-07).
- Ship the standalone `mai-ship-validate` binary. The runtime
  fail-closed gate now lives inside `MaiServer::run()`; the
  standalone binary + admin HTTP endpoint are the SHIP-07
  remainder slice and are still pending.

## Running the parser locally

```bash
# Verify the on-disk file parses + validates.
cargo test -p mai-api ship_profile
```

The test target name is `ship_profile`; both the unit tests in
`mai-api/src/ship_profile.rs` and the integration tests in
`mai-api/tests/ship_profile.rs` run under that filter.

## What changes after SHIP-01

| Session  | Status   | Adds to `ship` enforcement                                                |
|----------|----------|----------------------------------------------------------------------------|
| SHIP-02  | **done** | Centralised `production_guard.rs` with 40 `PROD-*` check IDs + stop-gap `mai-api validate --profile <PATH> [--json]` CLI. |
| SHIP-03  | **done** | `build_vault` selects a real backend; ship rejects `StubVault` at the builder. Wiring landed in SHIP-07 convergence. |
| SHIP-04  | **done** | `WalAuditWriter` (mai-api/src/audit_wal.rs) — JSON-lines append-only WAL, replay+verify on `open()`, rotation, 7-year retention metadata. Wiring landed in SHIP-07 convergence. |
| SHIP-05  | **done** | Compliance audit sealer builder; ship replaces `NullSealer` with vault-backed AEAD via `build_sealer`. Wiring landed in SHIP-07 convergence. |
| SHIP-06  | **done** | Trust production mode; `build_trust_components` + `verify_boot_bundle` reject synthetic exchange + accept-all verifier. Wiring landed in SHIP-07 convergence. |
| SHIP-07-convergence | **done** | `MaiServer::with_ship_profile()` + `MAI_SHIP_PROFILE` env var drive `build_vault` / `WalAuditWriter::open` / `build_sealer` / `build_trust_components` / `verify_boot_bundle` from `MaiServer::run()`. Fails closed via `ProductionReadinessReport::evaluate_with_runtime` before any socket binds. Six deferred checks (`PROD-VAULT-100`, `PROD-AUDIT-100`, `PROD-AUDIT-101`, `PROD-TRUST-100`, `PROD-AUTH-100`, `PROD-POLICY-001`) flip to Pass / Fail at runtime via new public `RuntimeChecks` + `RuntimeOutcome` types. |
| SHIP-07-endpoint-and-cli | **done** | `GET /v1/system/production-readiness` admin endpoint + standalone `mai-ship-validate` binary that loads a profile + state-dir and prints the report with §13 exit codes. Profile-aware `handlers/trust.rs::exchange_token` switches on `TrustExchangeMode` (synthetic / OpenBaoBridge / Disabled). Commit `7b746c0`. |
| SHIP-08  | **done** | Packaging skeleton: systemd units (`mai-api`, `mai-dashboard`, `mai-adapter-manager`, `mai-healthcheck`) with `NoNewPrivileges/PrivateTmp/ProtectSystem=strict`, Debian package layout, `scripts/build-package.{sh,ps1}`, 110 packaging static tests. Commit `0fec605`. |
| SHIP-09  | **done** | `tools/mai-admin` Cargo crate with `backup create` / `backup verify`; `BackupManifest` with per-component sha3-256 + ML-DSA-87 manifest signature; 10 component handlers (build_info, config_checksums, api_audit_wal, compliance_audit_wal, trust_bundle_cache, trust_anchors, vault_snapshot_ref, auth_key_hashes, model_registry, reports). Auth keys backed up as sha3 hashes, never raw secrets. Commit `7b746c0` (clippy polish `aa839cb`). |
| SHIP-10  | **done** | `mai-admin restore plan/apply` with full read-only verification before any target write: signature + per-component sha3 + WAL chain replay + last-entry-hash agreement. Apply refuses populated targets without `--force`, recomputes sha3 after every write, replays the WAL chain in the restored tree, drops `source-manifest.json` + `restore-report.json` witnesses. DR drills: WAL tamper, missing trust bundle, missing model registry, signed-manifest tamper — each asserts target stays empty after failed plan. Commit `0fe5f59`. |
| SHIP-11+ | pending  | Observability + alerting (SHIP-11), CI enforcement + nightly burn-in (SHIP-12), 72-hour burn-in scripts (SHIP-14), operator docs and runbooks (SHIP-15), final audit pass (SHIP-16). SHIP-13 GPU release workflow already done in commit `7b746c0`. |

### SHIP-02 readiness output (config-only pass — `mai-api validate`)

```
$ mai-api validate --profile deployment/ship/profile.toml
MAI Production Readiness: PASS
Profile: ship (mode=Production)
Checks: 34 pass / 0 fail / 6 deferred / 0 skipped

[PASS]     PROD-CONFIG-001: profile.mode = production
...
[DEFERRED] PROD-VAULT-100: vault opens, sealed master key loads, root directory is writable (lands in SHIP-03)
[DEFERRED] PROD-AUDIT-100: API audit WAL writable, chain verifies, append round-trip succeeds (lands in SHIP-04)
...
```

The config-only path keeps the six deferred IDs visible so operators see the known gaps. Each deferred check names the SHIP session that closes it.

### SHIP-07 convergence readiness output (runtime pass — inside `MaiServer::run()`)

When `MAI_SHIP_PROFILE` is set or `MaiServer::with_ship_profile(path)` is called, the server constructs the real vault / audit WAL / sealer / trust components, collects six `RuntimeOutcome`s into a `RuntimeChecks`, and runs `ProductionReadinessReport::evaluate_with_runtime` before binding sockets. The deferred IDs flip to live status:

```
[PASS]     PROD-VAULT-100: Zfs vault opened at /var/lib/mai/vault
[PASS]     PROD-AUDIT-100: WAL opened at /var/lib/mai/audit
[PASS]     PROD-AUDIT-101: AEAD sealer wired from sealer.key
[PASS]     PROD-TRUST-100: bundle v2026-05-23 verified against 3 anchors
[PASS]     PROD-AUTH-100: 4 key(s) loaded
[PASS]     PROD-POLICY-001: standard policy modules loaded
```

If any flips to `Fail` (e.g. missing trust anchor), `MaiServer::run()` returns `ServerError::Init` carrying the rendered report and never reaches `bind()`.

## Related docs

- [SHIP-HARDENING-PLAN.md](SHIP-HARDENING-PLAN.md) — the full execution plan.
- [`mai/deployment/README.md`](../deployment/README.md) — top-level profile index.
- [`mai/deployment/ship/README.md`](../deployment/ship/README.md) — ship profile operator notes.
- `mai/docs/KNOWN-ISSUES.md` — current production-path caveats; SHIP-02..SHIP-16 close these out.
