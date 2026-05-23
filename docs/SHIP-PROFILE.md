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
- `config/ship-validator.toml` — placeholder for the SHIP-07 CLI.
- `mai-api/src/ship_profile.rs` — typed schema, loader,
  parse-time validator.
- `mai-api/tests/ship_profile.rs` — integration test against the
  on-disk profile.

SHIP-01 explicitly does **not**:

- Wire the parsed profile into `ServerConfig` or `MaiServer` startup.
  That work belongs to SHIP-02..SHIP-05.
- Check that the configured paths exist on disk. That's a runtime
  guard responsibility (SHIP-02).
- Ship the `mai-ship-validate` CLI. That lands in SHIP-07.

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
| SHIP-03  | **done** | `build_vault` selects a real backend; ship rejects `StubVault` at the builder; wiring deferred to SHIP-07 convergence. |
| SHIP-04  | **done** | `WalAuditWriter` (mai-api/src/audit_wal.rs) — JSON-lines append-only WAL, replay+verify on `open()`, rotation, 7-year retention metadata. Wiring deferred to SHIP-07 convergence. |
| SHIP-05  | parallel | Compliance audit sealer builder; ship replaces `NullSealer` with vault-backed AEAD (parallel session). |
| SHIP-06  | pending  | Trust production mode; ship rejects synthetic exchange + accept-all verifier. |
| SHIP-07  | pending  | `/v1/system/production-readiness` endpoint + full `mai-ship-validate` binary. |
| SHIP-08+ | pending  | Packaging, backup/restore, observability, burn-in, docs, final gate.      |

### SHIP-02 readiness output

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

Deferred checks are surfaced (not silently skipped) so operators see the known gaps. Each deferred check names the SHIP session that closes it.

## Related docs

- [SHIP-HARDENING-PLAN.md](SHIP-HARDENING-PLAN.md) — the full execution plan.
- [`mai/deployment/README.md`](../deployment/README.md) — top-level profile index.
- [`mai/deployment/ship/README.md`](../deployment/ship/README.md) — ship profile operator notes.
- `mai/docs/KNOWN-ISSUES.md` — current production-path caveats; SHIP-02..SHIP-16 close these out.
