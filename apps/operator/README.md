# Operator/Admin Dashboard

Read-only snapshot of every plane of a local MAI instance: models,
scheduler, adapters, power, air-gap status, audit-log preview, and
trust-bundle state. It emits plain text or JSON and is designed for
cron, on-call use, and demo monitoring rather than as a full TUI.

## What It Demonstrates

- Coverage across the SDK's read-side namespaces: `client.models`,
  `client.scheduler`, `client.admin`, `client.power`, `client.system`,
  and `client.trust`.
- Graceful degradation: each panel catches its own errors via the
  `_safe()` helper; one failing area does not kill the dashboard.
- Monitoring-friendly exit behavior: non-zero exit only if a core panel
  fails (`models`, `scheduler`, `power`, or `airgap`).

## Run

```powershell
python apps/operator/main.py
python apps/operator/main.py --json | jq '.panels[].name'
```

Sample output:

```text
=== MAI Operator Dashboard ===
[OK ] models     2/3 loaded
[OK ] scheduler  queue=0 active=1 p95=2.5ms
[OK ] adapters   1 adapter(s)
[OK ] power      state=full_inference ~220W
[OK ] airgap     enabled=True verified=True net=air_gap_compliant
[OK ] audit      120 total, showing 10
[OK ] trust      bundle=provisioned ttl=86400s policies=3
```

## Reading The Panels

Each panel maps to a specific operator action when it shows a non-OK
state.

**models:** `loaded` count below total means one or more model weights
failed to load. Check the model path in `config.toml` and confirm disk
access. A model at `0/N loaded` blocks inference entirely.

**scheduler:** elevated `p95` latency, roughly above 50ms, or a growing
queue depth signals resource pressure. Check active GPU utilization and
whether a competing process has claimed device memory.

**adapters:** `0 adapter(s)` means no LoRA adapter mounted. This is
expected if no adapter is configured; it is a problem if the demo
scenario requires one.

**power:** states below `full_inference`, such as `throttled` or
`battery_only`, indicate thermal or power constraints that will affect
throughput. Verify cooling and power supply before a demo session.

**airgap:** `enabled=True verified=True net=air_gap_compliant` is the
expected state for a hardened deployment. Any deviation, including
`enabled=False`, `verified=False`, or a non-compliant network
designation, is a compliance failure rather than a configuration
choice. Do not proceed with a demo in a non-compliant state.

**audit:** the preview shows recent entry count. A count of zero when
inference has been running indicates the audit log is not receiving
writes. Check the audit sink configuration.

**trust:** `bundle=provisioned` confirms the trust manifold is active
and policies are loaded. `ttl` shows seconds until the next bundle
refresh. A `not-provisioned` state means the trust endpoint did not
return a valid bundle; check that `mai-api` is running and the trust
namespace is reachable.

## Configure

Edit [config.toml](config.toml). Each `[display].<panel>` flag toggles a
section. `audit_limit` caps the audit-log preview rows.

## Tests

```powershell
pytest apps/operator/tests/
```

`test_smoke.py` confirms each `_safe()` panel renders correctly with
mocked endpoints, graceful error rendering works for failed panels, and
the trust bundle flow handles a provisioned response.

`test_integration.py` renders the full dashboard under a synthetic
server with all panels populated; exit code 0 when all core panels are
green; exit code 5 when a core panel errors.

## Trust And Audit Endpoint Behavior

`panel_trust` calls `client.trust.bundle_status()` and reports the live
`TrustBundleStatus` returned by `mai-api`. `panel_audit` calls
`client.admin.audit_log()` and picks up BF-5 correlation fields
automatically; no code change is required when audit log entries carry
additional correlation metadata.
