# Local Trust Cache

**Status:** BF-4 design landed (in-memory model). On-disk format and signed-bundle verification land with BF-3.
**Source of truth:** [`mai-compliance/src/trust_cache.rs`](../mai-compliance/src/trust_cache.rs) and [`mai-core/src/airgap/mod.rs`](../mai-core/src/airgap/mod.rs).
**Plan reference:** Appendix A §A.8 of `BUILD-EXECUTION-PLAN-V2-UPDATED.md`.

## 1. Purpose

The Lamprey Trust Bridge issues short-lived signed claims that Lamprey policy engines consume via `TrustContext`. When the bridge is unreachable — partition, outage, scheduled disconnect, or hardware air-gap — the appliance falls back to the most recently received signed policy bundle and revocation snapshot held in this local cache. This document defines the cache's state model, the connectivity ladder it drives, the revocation snapshot shape, the emergency-access behaviour, and the offline audit queue.

## 2. Connectivity state ladder

The canonical `ConnectivityState` enum lives in `mai_core::airgap::ConnectivityState` and is shared by `mai-adapters` (host validation), `mai-api` (bind enforcement and the `/v1/system/airgap` endpoint), and `mai-compliance` (`TrustContext`). The cache's `evaluate(switch_state, live_link, now_secs)` method returns the runtime state:

| State | Trigger | Behaviour |
|---|---|---|
| `Connected` | `live_link=true`, cache age < `warn_after` | live validation, cloud routes permitted |
| `Degraded` | `live_link=false`, cache age < `warn_after` | cached validation against signed bundle, cloud routes permitted |
| `StaleNotExpired` | `warn_after` ≤ age < `expire_after` | warn + continue, cloud routes restricted |
| `Expired` | age ≥ `expire_after` (or never refreshed) | emergency local-admin only |
| `AirGapped` | hardware switch engaged | local-only inference, no cloud route |

The hardware air-gap switch always wins. When the caller passes `ConnectivityState::AirGapped`, the cache returns `AirGapped` regardless of bundle freshness.

## 3. Default thresholds

```rust
CacheThresholds {
    warn_after: 1 hour,
    expire_after: 24 hours,
}
```

Operators override per deployment profile. The constructor refuses `expire_after < warn_after` (would create an empty warn band). Refresh timestamps in the future are refused to prevent clock-skew exploits.

## 4. Trust status endpoint design

The cache surfaces the live state through the existing `mai-api` system route group. Session 28 added `GET /v1/system/airgap` (returns the canonical `ConnectivityState` plus the derived flags). BF-6 (SDK) will extend this with `GET /v1/system/trust`:

```json
{
  "connectivity": "degraded",
  "bundle_version": "bundle-2026.05.22.001",
  "last_refresh_secs": 1748000000,
  "age_secs": 540,
  "warn_after_secs": 3600,
  "expire_after_secs": 86400,
  "live_link": false,
  "revocation_snapshot_count": 1247,
  "offline_audit_backlog": 0
}
```

The endpoint requires the same authenticated profile as `/v1/system/airgap`; no role check beyond `valid auth token` because the values it exposes are already operational invariants every component depends on.

## 5. Revocation snapshot model

```rust
struct RevocationSnapshot {
    claim_id: String,
    status: SnapshotStatus,          // Valid | Revoked | Unknown
    recorded_at_secs: u64,
}
```

Snapshots are keyed by `claim_id` in a `BTreeMap`. Lookups for unknown claim ids return `SnapshotStatus::Unknown` (pessimistic — the policy runtime treats this as `revoked` for ITAR content and `stale` for uncontrolled content, per `docs/SERVICE-IDENTITY.md` §4.5). The on-disk format and the signed-bundle envelope land with BF-3.

## 6. Emergency access

`LocalTrustCache::is_emergency_only(switch_state, live_link, now_secs)` returns true exactly when `evaluate` returns `Expired`. Callers that gate maintenance endpoints on this method MUST additionally require:

1. Explicit admin-role authentication (Session 26 auth gate).
2. A loopback bind address (Session 28 `validate_with_connectivity`).
3. Audit log entry tagged `emergency_access=true` (Session 42 will extend the audit schema).

The emergency mode is deliberately narrow — it exists so an operator can rotate credentials and re-establish trust, not to provide a routine fallback path.

## 7. Offline audit queue

While the cache is `Degraded`, `StaleNotExpired`, `Expired`, or `AirGapped`, callers enqueue audit events via `enqueue_offline_audit(event: impl Into<String>)`. The queue is in-memory and FIFO; the cache stores opaque strings and does not impose a schema. Session 42's audit subsystem owns the format and the flush path. On connectivity return, the audit subsystem calls `drain_offline_backlog()` and re-queues on flush failure.

## 8. Integration points

- `TrustContext.connectivity` (was `offline_mode: bool` pre-Session-28) is the per-decision projection of the cache's current state.
- `TrustContext::offline_mode()` is a backwards-compatible getter (`true` for everything except `Connected`).
- `TrustContext::permits_cloud_route()` consults `connectivity.permits_cloud_route()` AND the routing ceiling AND the revocation status.
- `TrustContext::requires_local_only()` mirrors `connectivity.requires_local_only()` (true for `Expired` and `AirGapped`).
- `mai-api::config::ServerConfig::validate_with_connectivity` rejects non-loopback bind addresses when `requires_local_only()`.
- `mai-adapters::validation::validate_adapter_host` rejects non-loopback adapter hosts under the same condition.

## 9. What is NOT in scope here

- **Bundle signature verification.** BF-3 (`docs/TRUST-BUNDLE-SPEC.md`, pending) defines the signed envelope; this cache stores already-verified snapshots.
- **On-disk persistence.** The cache is in-memory only; persistence is a follow-up after BF-3 lands the signed format.
- **Network reachability detection.** The `live_link` boolean is supplied by the caller (typically the air-gap switch monitor or the cloud bridge probe); the cache does not initiate any network traffic itself.
- **Audit transmission.** The cache queues events but never sends them; Session 42 owns the flush.

## 10. Cross-references

- Canonical enum: `mai_core::airgap::ConnectivityState`
- `AirGapPolicy` (watch-channel state holder): `mai_core::airgap::AirGapPolicy`
- TrustContext: `mai_compliance::trust::TrustContext`
- Session 28 prompt: `MAI-BUILD-PROMPT-ROSTER-v2.md` §Session 28
- BF-4 plan entry: `BUILD-EXECUTION-PLAN-V2-UPDATED.md` Appendix A §A.8
- TRUST-MANIFOLD.md (BF-1): trust boundary diagram
- SERVICE-IDENTITY.md (BF-2): claim → TrustContext mapping
