# Release Gates

The list of checks that must pass before an appliance is
considered shippable, the commands that exercise them, and the
exit semantics. This is the operator-facing companion to
[SHIP-HARDENING-PLAN.md §13](../sessions/SHIP-HARDENING-PLAN.md) and
[SHIP-PROFILE.md](../operations/SHIP-PROFILE.md).

## Single source of truth

The validator is one binary, one schema:

```bash
sudo mai-ship-validate --profile /etc/mai/profile.toml
```

Same logic, exposed via HTTP for monitoring:

```bash
curl -fsS -H "X-IM-Auth-Token: $T" \
     http://127.0.0.1:8420/v1/system/production-readiness
```

Both paths share the `ProductionReadinessReport` evaluator. If
you ever see them disagree, that is a bug — report it before
shipping the appliance.

## Exit code contract

| Code | Meaning |
|---|---|
| 0 | All checks pass. Appliance is ship-ready against this profile. |
| 1 | One or more `Critical Fail` checks. Daemon will not bind sockets. |
| 2 | Configuration parse error. Profile file is malformed. |
| 3 | I/O error reading profile or anchors. |
| 4 | Internal validator error. Should never reach the operator. |

`mai-api.service` runs the validator in `ExecStartPre`. Any
non-zero exit blocks the service unit from starting. This is
the same exit-code contract used by `mai-admin restore plan`,
`mai-admin backup verify`, and `mai-admin audit verify`.

## Check families

Check IDs use the form `PROD-<FAMILY>-<NNN>`. Families:

| Family | What it checks |
|---|---|
| `PROD-CONFIG-*` | Profile mode, fail-closed flag, demo defaults |
| `PROD-PATHS-*` | Required directories exist with correct permissions |
| `PROD-VAULT-*` | Vault backend is real (no `StubVault`) and reachable |
| `PROD-AUDIT-*` | API and compliance WAL writable, chain verifies, AEAD sealer wired |
| `PROD-TRUST-*` | Anchors loaded, bundle verified, no `AcceptAllBundleVerifier` |
| `PROD-AUTH-*` | Key store non-empty; no internal-profile-header bypass |
| `PROD-DASH-*` | Dashboard enabled; no `dashboard-dev`, no default admin token |
| `PROD-NET-*` | Loopback bind, air-gap policy honored |
| `PROD-OBS-*` | JSON logs, rotation on, metrics endpoint enabled, alert rules wired |
| `PROD-POLICY-*` | Standard Lamprey policy modules loaded |

The full table of check IDs lives in
[SHIP-HARDENING-PLAN.md §3](../sessions/SHIP-HARDENING-PLAN.md). Each ID
names the rule it enforces; the validator output prints both
the ID and the rule.

## What the operator sees

A passing run:

```
$ sudo mai-ship-validate --profile /etc/mai/profile.toml
MAI Production Readiness: PASS
Profile: ship (mode=Production)
Checks: 40 pass / 0 fail / 0 deferred / 0 skipped

[PASS] PROD-CONFIG-001: profile.mode = production
[PASS] PROD-CONFIG-002: fail_closed = true
[PASS] PROD-CONFIG-003: allow_demo_defaults = false
[PASS] PROD-PATHS-001: /var/lib/mai exists (0750 mai:mai)
[PASS] PROD-PATHS-002: /var/lib/mai/audit writable
[PASS] PROD-VAULT-100: ZFS vault opened at /var/lib/mai/vault
[PASS] PROD-AUDIT-100: WAL opened at /var/lib/mai/audit (last_seq=124538)
[PASS] PROD-AUDIT-101: AEAD sealer wired from sealer.key
[PASS] PROD-TRUST-100: bundle 2026-05-23 verified against 3 anchors
[PASS] PROD-AUTH-100: 4 key(s) loaded
[PASS] PROD-DASH-001: dashboard enabled, no dev token
[PASS] PROD-NET-100: loopback bind on 127.0.0.1:8420, 127.0.0.1:50051
[PASS] PROD-OBS-100: JSON logs, rotation enabled, metrics on, 10 alert rules wired
[PASS] PROD-POLICY-001: standard policy modules loaded (hipaa, itar, ocap)
...
```

A failing run:

```
$ sudo mai-ship-validate --profile /etc/mai/profile.toml
MAI Production Readiness: FAIL
Profile: ship (mode=Production)
Checks: 38 pass / 2 fail / 0 deferred / 0 skipped

[FAIL] PROD-TRUST-100: bundle expired at 2026-05-22T00:00:00Z
[FAIL] PROD-AUTH-100: key store /etc/mai/auth_keys.toml is empty

Failed checks block service startup. Resolve and re-run.
```

Each failing check maps directly to a runbook — the validator
output is the entry point into the runbook index.

## Pre-release sequence

Before declaring a build shippable, run every command below.
If any exits non-zero, the build is not shippable.

### Developer workstation
```bash
cargo check --workspace
cargo fmt --check
cargo clippy --workspace -- -D warnings -A clippy::pedantic
cargo test --workspace
python -m pytest adapters/ mai-sdk-python/ tools/ \
                  compliance-dashboard/ apps/
ruff check adapters/ mai-sdk-python/ compliance-dashboard/ apps/
mypy --strict adapters/ mai-sdk-python/src/
```

### Package build
```bash
scripts/build-package.sh --deb
mai-ship-validate --package-root target/package-staging \
                  --profile deployment/ship/profile.toml
```

### Installed node
```bash
sudo apt install ./mai_*.deb
sudo mai-ship-validate --profile /etc/mai/profile.toml
sudo systemctl status mai-api.service
curl -fsS http://127.0.0.1:8420/v1/health/live
curl -fsS -H "X-IM-Auth-Token: $T" \
         http://127.0.0.1:8420/v1/health/ready
```

### Recovery proof
```bash
sudo -u mai mai-admin backup create --out /var/backups/mai/test
sudo -u mai mai-admin backup verify /var/backups/mai/test
sudo -u mai mai-admin restore plan /var/backups/mai/test \
                                   --target /tmp/mai-restore
sudo -u mai mai-admin restore apply /var/backups/mai/test \
                                    --target /tmp/mai-restore
sudo mai-ship-validate --state-dir /tmp/mai-restore \
                        --profile /tmp/mai-restore/etc/profile.toml
```

### Hardware burn-in
```bash
sudo scripts/burn-in.sh --output results/release-smoke
sudo scripts/burn-in-72h.sh --output results/release-72h
sudo mai-ship-validate --profile /etc/mai/profile.toml
```

A build is shippable when every command above exits 0 and the
72-hour burn-in produced a signed report. See SHIP-14 for the
report shape.

## Boundary

The release gates are necessary but not sufficient. They prove
the appliance starts, validates, backs up, restores, and serves
clean traffic. They do not prove the product strategy, the
hardware sourcing, the upstream policy authoring, or the
operator's training. Those are the operator's responsibility,
not the validator's.
