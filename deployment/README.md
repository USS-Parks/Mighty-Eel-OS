# MAI Deployment Profiles (BF-6)

Each subdirectory is a Trust-Manifold-aware deployment posture. The
profile name selects which connectivity mode the platform expects, which
compliance template the policy runtime should boot under, and whether
the air-gap switch is enforced or simulated.

| Profile             | Trust mode           | Compliance template | Air-gap | Cloud route allowed |
|---------------------|----------------------|---------------------|---------|---------------------|
| `local-dev`         | local, no Trust Bridge | Standard          | off     | yes (loopback)      |
| `cloud-trust-core`  | Trust Bridge host    | Standard            | off     | n/a — central node  |
| `local-mai-node`    | connected → degraded → air-gapped (ladder) | per-tenant | switch-driven | yes when connected  |
| `airgap-demo`       | air-gapped (preloaded bundle)              | Defense            | always on | never             |
| `ship`              | production (ML-DSA, anchors on disk, bundle-on-boot) | per-tenant | per-site | per-site |

These profiles satisfy BF-6 §A.10 — the operator-visible artefact that
proves the Trust Manifold has been planned for and that each posture is
reachable from configuration alone. The `ship` row is the
customer-facing posture introduced by the hardening lane — see
[../docs/SHIP-PROFILE.md](../docs/SHIP-PROFILE.md) and
[../docs/SHIP-HARDENING-PLAN.md](../docs/SHIP-HARDENING-PLAN.md).

## Applying a profile

> **Non-production launch.** The command below runs a demo posture and
> does not engage the production guard. For a customer node, follow
> [`ship/README.md`](ship/README.md) — the guard engages only when
> `MAI_SHIP_PROFILE` points at the profile TOML.

```bash
# Bash
cargo run -p mai-api -- --config deployment/airgap-demo/profile.toml

# PowerShell
cargo run -p mai-api -- --config deployment/airgap-demo/profile.toml
```

The server reads `profile.toml`, wires the trust cache, bundle verifier,
policy template, and connectivity policy accordingly, then exposes the
result via `GET /v1/trust/status` and `GET /v1/compliance/status`.

## Where to look next

- `mai/docs/TRUST-MANIFOLD.md` — architecture overview.
- `mai/docs/OPENBAO-INTEGRATION.md` — Trust Bridge wire format.
- `mai/docs/LOCAL-TRUST-CACHE.md` — connectivity ladder + thresholds.
- `mai/docs/AUDIT-CORRELATION.md` — credential ↔ decision linkage.
