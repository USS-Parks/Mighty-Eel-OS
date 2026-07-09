# M6 / X2 — Loom Phase-V Live Gates (>=3-node leg): STS run — GREEN 5/5

**Date (UTC):** 2026-07-09 13:55
**Repo state at run:** `4bf2046` + one uncommitted harness fix (`deployment/loom-harness/docker-compose.yml`)
**Plan context:** `docs/audits/2026-07-08-full-repo/REMEDIATION-PSPR.md` Phase X prompt **X2**,
the `>=3-node harness` leg (CI `loom-live` job). Companion record to `SUMMARY.md`
(the OpenBao + Moto leg, green 2026-07-09).

## Root cause of the CI red (`cp3 unhealthy`) and the fix

The wave's 0.2 loopback containment (`a38014b`) makes aogd refuse a non-loopback
`AOGD_LISTEN` unless `AOGD_ALLOW_INSECURE_BIND=1` (`crates/aogd/src/main.rs`). The estate
binds every cp at `0.0.0.0:4600` on the compose network, so all five cps exited at startup,
healthchecks never passed, and `docker compose up --wait` failed at the first reported
dependency (`cp3`). The compose network is precisely the guard's documented "trusted,
isolated network" opt-in case; the fix adds `AOGD_ALLOW_INSECURE_BIND: "1"` to the five cp
services plus a why-comment. **Harness config only — no product code changed.** The A1
admin auth is not armed in this estate (pre-anchor bootstrap posture, no anchor env), so
`cluster-init.sh` and the gates run unchanged.

## What ran (the CI `loom-live` steps, exact order, exit codes)

1. `docker build -f deployment/loom-harness/Dockerfile -t loom-harness:vh4 .` -> **exit 0**
2. `docker compose -f deployment/loom-harness/docker-compose.yml up -d --wait` -> **exit 0**
   — 11/11 healthy (cp1–cp5 including the CI casualty cp3, edge1–edge5, openbao);
   `cluster-init` formed the 5-voter Raft cluster and exited 0; edges self-registered.
3. The five gates in CI order -> **all PASS, chain exit 0**:

```
V5 PASS: under 100-object scale, the kill reached all 5 replicas within SLO
V8 PASS: 100 workloads fully replicated across all 5 replicas
V10 PASS: revocation reached every replica within 10s each round (worst 4s); the strict p99<=3s is the in-process gate
V4 PASS: majority commits under partition, isolated minority fences
V7 PASS: 5 kill/heal cycles survived; leader re-emerged + killed node caught up each round; all replicas converged to the identical rollout end state
ALL_GATES_GREEN
```

4. `docker compose down -v` -> estate and volumes removed.

## Scope honesty

- Dev-mode OpenBao (digest-pinned image) and a dev harness posture; the bind opt-in is
  **not** a production posture and the compose comment says so inline.
- V10's containerized assertion is <=10s per round (worst observed 4s); the strict
  p99<=3s SLO is the in-process gate, per the gate's own output.

## Disposition

**X2 leg 2 (>=3-node harness): GREEN.** With leg 1 (OpenBao + Moto) already green, both
X2 live legs are green on this host. Committing the compose fix un-reds the CI
`loom-live` job.

## Files

- `loom-gates-run.log` — verbatim per-gate output; **local artifact only** (`*.log`
  gitignored repo-wide)
- Harness fix: `deployment/loom-harness/docker-compose.yml` (uncommitted at record time;
  SHA to be recorded on commit)
