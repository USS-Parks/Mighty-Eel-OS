# MAI Simulation Framework

A discrete-event simulation framework for testing scheduling and KV cache policies
offline before hardware deployment.

## Quick Start

```
# Run all predefined experiments (results written to results/)
python experiments.py

# Run with a specific random seed
python experiments.py 12345
```

## Architecture

- **engine.py** - Discrete-event simulation engine. Pluggable components
  (RequestGenerator, Scheduler, KvManager, BatchBuilder, GpuModel).
- **gpu.py** - GPU resource model. Supports single and multi-GPU with topology cost.
- **workload.py** - Synthetic workload generators: Chat, Batch, Mixed.
- **kv_policy.py** - KV cache policy interface and 4 implementations:
  - LRU (least recently used)
  - Size-based (evict largest first)
  - HeuristicScored (multi-factor: idle time, size, priority)
  - BatchAware (HeuristicScored + batch protection)
- **metrics.py** - Measurement framework. Throughput, latency (P50/P95/P99),
  eviction rate, batch utilization, KV utilization.
- **experiments.py** - Predefined experiment runner.

## Experiments

1. **Policy Comparison** - Same workload, 4 KV policies. Which gives best throughput?
2. **Memory Pressure Sweep** - Vary VRAM from 40GB to 120GB. How does each policy degrade?
3. **Workload Mix Sweep** - Vary chat/batch ratio from 100/0 to 0/100.
4. **Burst Load Test** - 5x traffic spike for 10 seconds. How does latency recover?
5. **Weight Sensitivity** - Vary heuristic scoring weights. How do eviction patterns change?

## Determinism

Same seed produces identical results. All random number generators are seeded.
Set seed in config.toml or pass as CLI argument.

## Configuration

Edit config.toml to change hardware, workload, KV policy, and experiment settings.
No code changes needed to run experiments.
