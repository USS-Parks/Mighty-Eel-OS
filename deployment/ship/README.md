# Profile: `ship`

Customer-running production posture. This is the only profile that is
sold or installed on a node delivered to a regulated end-user. Use it
on every appliance the operator does not personally re-image between
debugging sessions.

When a ship profile is loaded (`MAI_SHIP_PROFILE` pointing at the
profile TOML), the production guard rejects every demo-safe default
before the API server is allowed to listen. Parsing landed in SHIP-01; the runtime guard, vault
wiring, audit WAL, trust components, packaging, backup/restore,
observability, CI enforcement, GPU release workflow, 72-hour
burn-in, and operator docs and runbooks all landed across
SHIP-02..SHIP-15. The standalone `mai-ship-validate` binary is
the single gate (SHIP-07-endpoint-and-cli).

## What this profile contracts

- `[profile] mode = "production"`, `fail_closed = true`,
  `allow_demo_defaults = false`. Any deviation is a parse-time error.
- Persistent paths for state, config, log, run, and backup directories
  must all be present.
- Vault: real backend (`zfs` is the reference; `file-dev` only with an
  explicit local-dev profile override). `StubVault` is rejected.
- Audit: persistent WAL writer for both API and compliance audit.
  `MemoryAuditWriter` and `NullSealer` are rejected. Hash chain and
  PQC checkpoint signing are required.
- Trust: ML-DSA bundle verifier with a trust-anchor directory present
  on disk and a verifiable bundle on boot. `AcceptAllBundleVerifier`
  and synthetic local-dev token exchange are rejected.
- Auth: non-empty API key store at a configured path. The internal
  profile header bypass is rejected.
- Dashboard: enabled, but `dashboard-dev` and any default admin token
  are rejected.
- Network: loopback bind, reverse-proxy TLS termination is the
  contracted shape. Public binds are guarded.
- Observability: JSON logs, rotation on, Prometheus metrics, alert
  rules wired.

## How to use this profile

`MAI_SHIP_PROFILE` is the variable the server reads; it engages the
production guard. `--config` alone does **not**.

```bash
# Bash
MAI_SHIP_PROFILE=deployment/ship/profile.toml cargo run -p mai-api -- --config deployment/ship/profile.toml

# PowerShell
$env:MAI_SHIP_PROFILE = "deployment/ship/profile.toml"; cargo run -p mai-api -- --config deployment/ship/profile.toml
```

On a developer workstation this launch is expected to **fail closed**:
the guard demands the real vault, audit WAL, trust anchors, and key
store at the paths the profile names (`/var/lib/mai`, `/etc/mai`).
That refusal is the guard working, not a bug. On a real installed node
the operator points at `/etc/mai/profile.toml` and `mai-api` runs
under systemd (SHIP-08), which sets `MAI_SHIP_PROFILE` in the unit.

## What this profile is NOT

- Not a developer convenience. Use `local-dev` for that.
- Not a demo. Use `airgap-demo` for offline demos and
  `local-mai-node` for connectivity-ladder demos.
- Not a central Trust Bridge host. Use `cloud-trust-core` for that
  role.

## Validation

Run the standalone validator against the on-disk profile:

```bash
sudo mai-ship-validate --profile /etc/mai/profile.toml
```

Exit 0 means every `PROD-*` check passes against the real
vault, audit WAL, sealer, and trust components. The full check
matrix lives in [`docs/RELEASE-GATES.md`](../../docs/RELEASE-GATES.md);
the runbook index that maps failing checks to operator
procedures lives at
[`docs/runbooks/README.md`](../../docs/runbooks/README.md).

For parser-only verification on a developer workstation:

```bash
cargo test -p mai-api ship_profile
```

## Where to look next

- `mai/docs/SHIP-PROFILE.md` — comparison of all four MAI profiles and
  what each guarantee means in code.
- `mai/docs/SHIP-HARDENING-PLAN.md` — full execution plan, including
  the workstreams and check-ID conventions referenced above.
- `mai/deployment/README.md` — top-level profile index.
