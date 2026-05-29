# GitDoctor 75 Local Adapter Load-Balancing Design (GD75-09)

**Date:** 2026-05-25  
**Scope:** single-node, localhost-bound, air-gapped appliance deployments.

## Goal

When multiple instances (adapters) can serve the same model, route new requests to reduce overload **without** introducing multi-node/cloud assumptions.

## Current Primitives (already implemented)

- **Placement decision (new requests):** `mai-scheduler/src/placement.rs` (least-loaded + continuation affinity; pluggable scorer).
- **Cross-instance rebalance (existing sequences):** `mai-scheduler/src/balancer.rs` (KV-aware migration plan emitter; does not perform migration).
- **Integration evidence that routing excludes failed adapters / re-joins recovered adapters:** `mai-core/tests/integration_lifecycle.rs` and `mai-adapters/tests/e2e_integration.rs`.

## Design (local-only)

1. **Candidate set:** instances that (a) serve the resolved backend model and (b) are healthy / not overloaded.
2. **Sticky routing first:** if `continuation_of` is present and the prior instance remains viable, keep it (warm KV).
3. **Least-loaded routing:** otherwise choose the lowest load score (queue depth primary; VRAM pressure as tiebreak).
4. **Degraded mode:** if all candidates are overloaded, still route (best-effort) but mark `placement_reason`.
5. **Rebalancing:** under sustained imbalance, emit migration candidates via `LoadBalancer::evaluate(...)` and apply via the existing soft-eviction / offload path (keeps warm-cache benefits).

## Evidence To Show Reviewers

- `ScheduleDecision.placement_reason` is populated by placement and is observable in logs/telemetry.
- Tests above demonstrate pool distribution, failure exclusion, and recovery re-join behavior without live backends.

