# Air-Gap Enforcement Brief

**Project:** Island Mountain Model Abstraction Interface (MAI)
**Audience:** Acquirer security architects, regulated-deployment
operators, network-policy reviewers
**Status:** Session 45 acquisition documentation
**Last Updated:** 2026-05-23

Air-gap in MAI is a routing input, not a deployment flag. The router
consults the canonical `ConnectivityState`; the policy composer
refuses cloud routes when air-gapped; the audit log records every
refusal with policy version and credential correlation. This brief
explains the contract and how to verify it from source.

For broader system context, see
[`MAI-MASTER-ARCHITECTURE.md`](../architecture/MAI-MASTER-ARCHITECTURE.md) §16. For
the trust-cache interaction, see
[`LOCAL-TRUST-CACHE.md`](../compliance/LOCAL-TRUST-CACHE.md).

---

## The `ConnectivityState` enum

Source: `mai-core/src/airgap/mod.rs` (Session 28).

```rust
pub enum ConnectivityState {
    Connected,
    Degraded { since: Instant, reason: String },
    AirGapped,
}
```

Every connectivity surface — the trust cache, the router, the policy
composer, the dashboard, the SDK — consumes this single enum. There
is no "is_offline()" boolean and no "offline mode" toggle hidden in
config; the state is the contract.

Transitions:

- `Connected → Degraded` — fired when the trust cache cannot reach
  the OpenBao Trust Bridge for `cache.degrade_after_seconds`
  (default 30s).
- `Degraded → AirGapped` — fired when the operator sets
  `[airgap] enabled = true` in the deployment profile, OR when the
  hardware air-gap switch reports `engaged` (when wired).
- `AirGapped → Degraded → Connected` — recovery requires both
  software flag and switch reading to clear.

The state is read by the router on every request, not cached.

---

## Loopback bind enforcement

Source: `mai-api/src/server.rs` and `mai-api/src/config.rs`.

When the deployment profile sets `[airgap] enabled = true`, the API
server refuses to bind to any external interface. Specifically:

- `bind_addr = "0.0.0.0:8080"` is rejected at startup with a typed
  `ConfigError::AirGapBindViolation`.
- Wildcard binds (`0.0.0.0`, `::`, `[::]`) are blocked.
- Only `127.0.0.1`, `::1`, and explicit loopback interfaces succeed.
- The check runs *after* config load but *before* the listener
  opens — there is no race window.

The same check runs against the gRPC and WebSocket binds. An
operator who tries to override this with a CLI flag still hits the
config-layer guard.

---

## The `/v1/system/airgap` surface

Source: `mai-api/src/handlers/system.rs:get_airgap_status`.

```http
GET /v1/system/airgap
```

Returns:

```json
{
  "state": "air_gapped",
  "enabled": true,
  "switch_engaged": true,
  "last_external_contact": "2026-05-22T23:14:51Z",
  "policy_routes_blocked": ["cloud_only", "external_api"],
  "since": "2026-05-22T23:14:53Z"
}
```

The compliance dashboard reads this endpoint on every poll. The trust
panel surfaces `state` and `switch_engaged`. The operator scaffold
(`apps/operator/`) renders it on the system panel.

---

## Interaction with the policy composer

Source: `mai-compliance/src/policy/composer.rs` (Session 41).

The composer treats air-gap as a most-restrictive route input.
Concretely:

- If `connectivity == AirGapped` and a module would have returned
  `cloud_allowed`, the composer downgrades to `local_only_allowed`.
- If `connectivity == AirGapped` and a module would have returned
  `local_only_allowed`, no change.
- If `connectivity == AirGapped` and a module would have returned
  `deny`, no change.
- If the trust bundle's `allowed_routes` does not include
  `local_only`, the composer denies with reason
  `airgap.no_local_route_allowed`.

Combined with deny-wins, this means a request that arrives during
air-gap can produce one of three outcomes: `local_only_allowed` (the
common case), `deny` (the trust bundle prohibits local-only), or a
typed `OcapError` / `HipaaError` / `ItarError` that has higher
precedence than the air-gap downgrade.

---

## Interaction with the trust cache

Source: `mai-compliance/src/trust_cache.rs` (BF-4, Session 28).

The trust cache surfaces a five-state connectivity model:

| Trust mode | When | Compliance behaviour |
|---|---|---|
| `connected` | Bridge reachable, bundle fresh | All routes per claim |
| `degraded` | Bridge unreachable, bundle valid | Reduced refresh; routes preserved |
| `stale_not_expired` | Bundle within grace window | Read-only; flag in audit |
| `expired` | Bundle past `expires_at` | Local-only routes; reject cloud |
| `air-gapped` | Air-gap switch or flag set | Local-only routes; cloud refused |

These map onto `ConnectivityState` like this:

- `connected` → `Connected`
- `degraded`, `stale_not_expired`, `expired` → `Degraded { reason }`
- `air-gapped` → `AirGapped`

The two-axis model (`Connected/Degraded/AirGapped` for the network
side, `connected/degraded/stale/expired/air-gapped` for the trust
side) is intentional: the network can recover while the trust state
remains stale, and vice versa. The composer reads both axes and
chooses the most restrictive outcome.

---

## Expired-bundle degraded mode

Source: `apps/openbao-trust-demo/tests/test_degraded_bundle_marks_signature_unverified`.

The integration test exercises the path that proves air-gap is a
first-class concept and not a deployment afterthought:

1. The local trust cache is populated with a signed bundle.
2. The bundle's `expires_at` is set to a past time.
3. A request arrives requesting a cloud route.
4. The cache reports `expired`; the composer downgrades; the audit
   log writes a `RoutingDecision::LocalOnlyAllowed` (or a `Deny`
   depending on the claim's `allowed_routes`) with reason
   `airgap.expired_bundle`.
5. The dashboard alert page surfaces the event.

A regulator can replay this test against a fresh checkout; the
behaviour is deterministic.

---

## Audit-log coverage of air-gap events

Every air-gap-related state change writes an entry:

- `airgap.switch_engaged` — hardware switch toggled to engaged.
- `airgap.switch_released` — hardware switch toggled to released.
- `airgap.config_enabled` — operator flipped `[airgap] enabled`.
- `airgap.config_disabled` — operator cleared `[airgap] enabled`.
- `airgap.route_refused` — a request was denied because of air-gap
  policy.
- `airgap.route_downgraded` — a request had its route restricted
  from cloud to local-only.

All entries carry the BF-5 correlation fields, so an investigator can
join an air-gap refusal back to the originating credential event.

---

## What an acquirer can verify in 10 minutes

1. Read `mai-core/src/airgap/mod.rs` — the enum and state machine fit
   on one screen.
2. Read the loopback-bind guard in `mai-api/src/server.rs` — search
   for `AirGapBindViolation`.
3. Start `mai-api` with `[airgap] enabled = true` and
   `bind_addr = "0.0.0.0:8080"` — startup refuses with the typed
   error.
4. Run `pytest apps/openbao-trust-demo/tests/ -k degraded` — see the
   expired-bundle path go to local-only.
5. Open the dashboard, toggle the air-gap config, watch the alert
   page record the state change with correlation IDs.

This is the property that makes Island Mountain a legitimate
candidate for tribal, defence, and rural-hospital deployments where
the network is unreliable by design rather than by accident.
