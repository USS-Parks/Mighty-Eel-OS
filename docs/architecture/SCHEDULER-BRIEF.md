# Scheduler Technical Brief

**Project:** Island Mountain Model Abstraction Interface (MAI) **Audience:** Acquirer scheduling architects, distributed-systems reviewers, ML-platform leads **Status:** Session 45 acquisition documentation **Last Updated:** 2026-05-23

The MAI scheduler is the placement engine that survives hardware refresh. It is not a queue, it is not a round-robin balancer, and it is not bolted on top of an inference runtime \-- it is the inference runtime's *placement contract*. Every adapter, every backend, every model, every request flows through one decision pipeline.

This brief is a deep technical reference. For the acquisition positioning, see [`acquisition/ARCHITECTURE.md`](http://acquisition/ARCHITECTURE.md) and [`ACQUISITION-PACKAGE.md`](http://ACQUISITION-PACKAGE.md). For full system context, see [`MAI-MASTER-ARCHITECTURE.md`](http://MAI-MASTER-ARCHITECTURE.md).

---

## 15-minute skim path

For a reviewer who wants to confirm the scheduler is real and defensible before committing to the full deep read:

| What to do | Where | What it confirms |
| :---- | :---- | :---- |
| Read `schedule()` downward | `mai-scheduler/src/default.rs` | The six-stage pipeline exists and produces a `ScoreBreakdown` on every decision \-- no silent round-robin fallback |
| Read one file per subsystem | `scoring/mod.rs`, `topology/mod.rs`, `kv/mod.rs`, `batch/mod.rs` | Each subsystem exposes a clean trait; the placement code does not reach into implementation details |
| Run the test suite | `cargo test -p mai-scheduler --lib` | 324+ green tests across topology, KV, batching, scoring, balancer, and the full placement pipeline |
| Replay a trace | `python tools/simulator/replay_compare.py --trace examples/sample-trace.ndjson --policies default,least-loaded` | The same scheduler build produces bit-identical decisions at `(trace, seed, policy)` \-- deterministic and auditable |

The sections below cover each subsystem in depth.

---

## Why a scheduler is the moat

A model gateway routes by URL. A load balancer routes by request count or memory watermark. Neither survives the hardware refresh that Island Mountain expects between 2026 and 2030+: RTX 5090 → TetraMem MX100 → photonic memristor SoCs. The placement strategy that wins on NVIDIA does not necessarily win on memristor fabric, and a scheduler that bakes assumptions about CUDA or NVLink into its placement code becomes the bottleneck on day one of the next hardware generation.

MAI's scheduler reasons about an abstract topology graph fed by the Hardware Interface Layer (HIL). When the HIL gains a TetraMem driver, the topology graph absorbs the new edge weights. Placement policy does not change. Tests continue to pass. The scheduler is the layer that lets the model registry, the API, the auth surface, and the compliance engine outlive any one hardware generation.

---

## The decision pipeline

Every `schedule(request)` call walks the same six-stage pipeline:

1\. alias resolution         (mai-scheduler/src/aliases.rs)

2\. eligibility filter       (instances supporting the model \+ role)

3\. KV cache hint            (warm-cache preference)

4\. multi-factor scoring     (mai-scheduler/src/scoring/)

5\. batching admission       (mai-scheduler/src/batch/)

6\. preemption check         (mai-scheduler/src/preemption.rs)

Output: a `Placement` carrying instance ID, decision rationale, and a serialisable `ScoreBreakdown` for the audit trail. There are no silent allows and no "default round-robin" fallbacks; every placement either has a justified score or rejects with a typed overload reason.

---

## Topology graph

Source: `mai-scheduler/src/topology/` (5 files, 41 unit tests, 16 integration tests).

- **Collector** (`collector.rs`) parses `nvidia-smi -q -x` and ROCm equivalents into a normalised inventory of GPUs, NVLink/PCIe edges, and CPU affinity groups.
- **Graph** (`graph.rs`) builds an undirected weighted graph keyed on GPU index, with edge weights derived from link bandwidth and a NUMA-affinity penalty. NVLink edges and PCIe edges have distinct cost classes \-- a four-way NVLink clique can carry tensor-parallel traffic that PCIe-bridged GPUs cannot.
- **Analysis** (`analysis.rs`) computes all-pairs shortest paths (Floyd-Warshall), best GPU pairs / quads for tensor-parallel placement, NVLink clique detection, and per-CPU affinity groupings.
- **Refresh** (`refresh.rs`) absorbs thermal-throttle anomalies, VRAM exhaustion, and metrics deltas without rebuilding the full graph from scratch.
- **Config** (`mod.rs`) drives the whole subsystem from `config/topology.toml` \-- link weights, refresh interval, anomaly thresholds, and single-GPU penalty.

The topology graph is consulted at scoring time, not at admission time, so a request never blocks on graph recomputation.

---

## KV cache reuse

Source: `mai-scheduler/src/kv/` (6 files, 53 unit tests, 5 integration tests).

KV cache reuse is a first-class placement input. Warm-cache routing is preferred over cold even when the cold instance is less loaded, because the cost of re-prefilling a 16k-token KV cache is measured in hundreds of GPU-milliseconds \-- strictly worse than absorbing a slight queue-depth imbalance on the instance that already holds the sequence.

Major modules:

- `sequence.rs` \-- per-sequence accounting: memory estimation, touch tracking, exponential-moving-average reuse gap.
- `eviction.rs` \-- multi-factor eviction scoring (idle time, size, priority, predicted reuse). System sequences are immune.
- `guard.rs` \-- minimum residency, readmit penalty, rate limiting, eviction-history-aware decisions.
- `triggers.rs` \-- proactive / eviction / emergency thresholds with boundary-case handling.
- `mod.rs` \-- the `HeuristicKvCacheManager` implementation: allocate, deallocate, can\_fit, emergency bypass.

The cache manager is `Send + Sync` and exposed through a trait, so adapters that ship their own KV manager (e.g., vLLM with its block- based KV) can be wrapped without changes to the placement code.

---

## Continuous batching

Source: `mai-scheduler/src/batch/` (5 files, 52 tests).

Continuous batching admits requests into an in-flight batch with explicit admission, eviction, and preemption rules:

- **Admission** (`admission.rs`) uses dual-threshold regions: a comfort region below the soft limit accepts any priority; a contention region between soft and hard thresholds gates by priority and sequence length; the hard threshold rejects with an overload reason.
- **Preemption** (`preemption.rs`) selects victims by combined idle time \+ priority \+ last-step age, with a strict hierarchy (System \> High \> Normal \> Background) and immunity for System sequences.
- **Builder** (`builder.rs`) wires admission, preemption, and the per-batch step accounting; rejects on model mismatch or queue full with typed errors.
- **Metrics** (`metrics.rs`) maintains a rolling window of admission, preemption, and step counts with P50/P95/P99 percentiles.

A continuous-batching scheduler is not a queue \-- requests are interleaved at the token level, and the scheduler decides per step which in-flight sequence to advance. The MAI batch builder makes that decision in O(log n) using a priority-aware heap.

---

## Multi-factor scoring

Source: `mai-scheduler/src/scoring/` (41 tests across sub-files).

The scorer takes a candidate instance and returns a normalised score in \[0, 1\] with five components, weighted by `config/scoring.toml`:

| Component | Source | Default weight |
| :---- | :---- | :---- |
| Latency | EMA of recent inference latency | 0.30 |
| Memory | Free VRAM / total VRAM headroom | 0.25 |
| Topology | Edge cost to neighbour instances | 0.20 |
| Eviction risk | Inverse of recent KV-eviction rate | 0.15 |
| Batching slack | Free admission capacity | 0.10 |

Component contributions are returned alongside the final score as a `ScoreBreakdown`, which the audit log retains so a regulator can inspect exactly why instance A was preferred over instance B for a specific request.

---

## Cross-instance balancer

Source: `mai-scheduler/src/balancer.rs`.

The balancer runs out-of-band and performs net-benefit migration between instances. It considers:

- Current load distribution and EMA trends
- Topology distance between source and destination
- KV cache warmth on each side
- Cost of soft eviction across hot, warm, and cold KV tiers

A migration only fires if the projected benefit exceeds a configurable threshold. The balancer never preempts user-facing sequences; it operates at the placement-policy layer above the running batches.

---

## Decision cache

Source: `mai-scheduler/src/decision_cache.rs`.

For the high-volume case of "the same model alias, the same priority, the same load bucket," the scheduler caches the placement decision keyed on `(model_alias, priority, load_bucket)`. The cache has explicit hit / miss counters and a small TTL (the underlying load distribution does not stay stable for long), but it removes the multi-factor scoring step from the hot path when nothing has changed.

The cache is invalidated whenever:

- An instance is registered or removed
- The topology graph rebuilds
- The KV manager reports an emergency eviction

---

## Power state interaction

Source: `mai-scheduler/src/power.rs`.

The scheduler is the only component that can request a power-state transition for an instance. The transitions are:

- Deep Vault Sleep → Sentinel (cold start, warmup required)
- Sentinel → Full Inference (eager warm)
- Full Inference → Sentinel (idle threshold reached)
- Any → Deep Vault Sleep (operator request)

Placement decisions honour the current power state: a Sentinel instance can serve a single low-latency request before being considered for promotion; a Deep Vault Sleep instance is invisible until explicitly woken. Sentinel-first preload hands the scheduler the model registry's affinity ordering at startup.

---

## Trace replay and offline simulation

Source: `mai-scheduler/src/traces/` \+ `tools/simulator/`.

The replay harness reproduces production traces deterministically at `(trace, seed, policy)` \-- given the same inputs, the same scheduler build produces bit-identical placement decisions. This is the property a regulator needs to re-run a compliance investigation on historical traffic.

- `traces/recorder.rs` writes NDJSON traces with one record per scheduling event: request metadata, candidate set, score breakdown, chosen instance, batching outcome.
- `tools/simulator/replay_compare.py` re-plays a trace against alternative policies and emits a Markdown \+ JSON comparison report.
- `tools/simulator/report.py` renders the comparison for acquisition-grade review (regret, throughput, P95 latency, eviction count, fairness).

The harness is the way an acquirer answers "would a different scheduling policy have served this hospital workload better?" without shipping the new policy to production.

---

## Test posture

- `mai-scheduler` lib tests: 324+ across topology, KV, batching, scoring, balancer, decision cache, power, preemption, placement, registry, default scheduler.
- Topology integration: `mai-scheduler/tests/topology_integration.rs` (16 tests).
- Scheduling pipeline tests in `default.rs` (8 full scenarios covering topology, KV, batching, score breakdown, overload fallback, runtime rebuild).

Run with:

cargo test \-p mai-scheduler \--lib

---

## What an acquirer can verify in 15 minutes

1. Read `mai-scheduler/src/default.rs` from `schedule()` downward \-- the entire pipeline is visible without chasing 30 files.
2. Read `mai-scheduler/src/scoring/mod.rs` and `mod.rs` of `topology/`, `kv/`, `batch/` \-- one file per subsystem with the public API.
3. Run `cargo test -p mai-scheduler --lib` \-- 324+ green tests.
4. Replay a sample trace: `python tools/simulator/replay_compare.py --trace examples/sample-trace.ndjson --policies default,least-loaded` \-- compare scheduling policies on the same workload.

The scheduler is the layer most likely to be the acquirer's primary technical interest. It is also the layer that is most stable across hardware generations, which is why it occupies the centre of the architecture diagram in [`acquisition/ARCHITECTURE.md`](http://acquisition/ARCHITECTURE.md).
