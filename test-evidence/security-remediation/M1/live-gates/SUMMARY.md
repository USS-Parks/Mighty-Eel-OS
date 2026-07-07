# M1 — Live trust-plane gates (A5 / T7 / E7 / B6 / R6 / L4): live black-box run

**Date (UTC):** 2026-07-07
**Repo state at run:** `4af8b8b0daad47968a7f5c3c66e298c6aa53cc8a` on `claude/live-gates-docker-zfs-qoe2bc` (clean tree)
**Objective:** run the repository's canonical live black-box suite — every trust-adjacent
path against a **live OpenBao** and a **live Moto (AWS STS)** — closing the live leg the
2026-07-06 audit flagged as un-run (mechanisms wired and offline-proven; live proofs pending).

## What ran (exact commands + exit codes)

1. `SKIP_DOCKER=1 bash deployment/live-integration/run-live-suite.sh` → **exit 0** (15/15 PASS, "LIVE SUITE GREEN")
2. `cargo test -p aogd --test live_openbao_anchor -- --nocapture` → **exit 0** (CI `wsf-live` parity — the 16th entry)

Together this is exactly the CI `wsf-live` job set (`.github/workflows/ci.yml` lines 139–160).

## Live services (black-box over HTTP)

| Service | Endpoint | Version / provenance |
|---|---|---|
| OpenBao (dev mode) | `http://127.0.0.1:8200` | built from upstream source `github.com/openbao/openbao` @ commit `c3fd558bbc13c25eafa72d345a496c72ed0da92a` (main, 2026-07-07, post-v2.5.5); run as `bao server -dev`; unsealed, inmem storage |
| Moto (AWS STS) | `http://127.0.0.1:5566` | moto 5.2.2 (`moto[server]` from PyPI), run as `moto_server` |

**Dockerization note (honest):** this runner's egress policy blocks every container-registry
blob CDN (docker.io's cloudfront, ghcr blob storage, quay.io), so `docker pull` cannot
complete here. The pair therefore ran as native processes via the suite's supported
`SKIP_DOCKER=1` path instead of the `openbao/openbao:latest` + `motoserver/moto:latest`
containers CI uses. The proofs are black-box HTTP against the same wire surface; nothing in
the suite distinguishes container from process. OpenBao's banner reads `v2.0.0-HEAD` because
a plain `go build` does not stamp release ldflags; the source commit above is the version of
record.

## Results — 16/16 PASS

Verbatim runner summary (raw log is a local runner artifact; `*.log` is gitignored repo-wide):

```
PASS wsf-bridge/live_openbao
PASS wsf-broker/live_localstack
PASS wsf-broker/live_gcp
PASS wsf-broker/live_azure
PASS wsf-seal/live_seal
PASS wsf-ledger/live_ledger
PASS wsf-cache/live_cache
PASS wsf-tenants/live_tenants
PASS wsf-api/live_api
PASS aog-gateway/live_gateway
PASS aog-gateway/kill_switch
PASS aog-gateway/openai_surface
PASS aog-gateway/anthropic_surface
PASS aog-gateway/policy_modes
PASS aog-gateway/metering
LIVE SUITE GREEN — every trust-adjacent path verified against live services.
SUITE_EXIT=0
AOGD_EXIT=0
```

| Gate id | Test target | Live proof line (from run log) |
|---|---|---|
| W1 | `wsf-bridge::live_openbao` | issue + verify + ML-DSA sign against live OpenBao |
| W2 | `wsf-broker::live_localstack` | brokered scoped STS credential via Moto (key id redacted) |
| W7 | `wsf-broker::live_gcp` | scoped GCP token minted (scope+TTL enforced); fail-closed on bad scope/token |
| W8 | `wsf-broker::live_azure` | scoped Azure token minted; effective TTL capped to the trust token |
| W3 | `wsf-seal::live_seal` | transit-wrapped seal + HTTP unseal + **deny receipt** for under-cleared unseal |
| W4 | `wsf-ledger::live_ledger` | 3-entry cross-service ledger; signed pack head verifies off-host |
| W5 | `wsf-cache::live_cache` | real token + revocation synced; cloud→local under air-gap; **revoked denied offline** |
| W9 | `wsf-tenants::live_tenants` | provision → issue (per-tenant HMAC) → deprovision → **revoked offline** |
| W6 | `wsf-api::live_api` | SDK round-tripped every endpoint (incl. issue→attenuate→verify); OpenAPI published |
| G1 | `aog-gateway::live_gateway` | virtual key → token resolution; **over-budget → 402 pre-flight** |
| G9 | `aog-gateway::kill_switch` | budget exhaustion mid-session + **revocation snapshot halts next call** |
| G3 | `aog-gateway::openai_surface` | OpenAI-wire chat + stream + models + auth + G5 route tag |
| G4 | `aog-gateway::anthropic_surface` | Anthropic-wire message + stream + x-api-key auth |
| G6 | `aog-gateway::policy_modes` | shadow never blocks; **enforce blocks PHI→cloud; deny-wins** |
| G7 | `aog-gateway::metering` | cost-per-task aggregation across multi-call chain; receipt chain verifies |
| VH5b-c | `aogd::live_openbao_anchor` | daemon sources trust anchor + field-seal material from live OpenBao; authed CRUD |

## Reproducibility — second full pass on the same live pair (found + fixed one flake)

A verification rerun of `aog-gateway::kill_switch` against the *same* OpenBao instance
initially failed: the test leaves a revocation snapshot signed by its per-run throwaway
anchor at the fixed KV path `kv/data/aog/revocation`; on rerun the gateway fails closed on
the now-unverifiable signature (`Unauthorized("revocation snapshot signature: signature
failed verification")`) — the *mechanism* behaving correctly, the *test* not idempotent
against a reused instance. CI never sees this (fresh container per run); the suite's
documented `SKIP_DOCKER=1` reuse mode does. Fixed in this change: the test now destroys the
stale snapshot's KV metadata during provision, restoring its "no snapshot yet" precondition
(`crates/aog-gateway/tests/kill_switch.rs`). After the fix: kill_switch green 3× consecutively,
offline skip path unchanged, and the **entire 16-test set green a second time end-to-end
against the reused live pair** (`SUITE_EXIT=0`, `AOGD_EXIT=0`), proving the suite is
rerun-safe. No product code was changed.

## PSPR live-gate mapping (docs/scans/SECURITY-REMEDIATION-PSPR.md)

| PSPR gate | Live proof(s) green | Live negative control observed |
|---|---|---|
| **A5** live issuance | W1, W6, VH5b-c | AppRole-authenticated issuance only; unauthenticated/wrong-credential paths rejected per suite assertions |
| **T7** live attenuation | W6 (issue→attenuate→verify via SDK against live OpenBao) | lineage/parent binding asserted in-suite |
| **E7** live envelope | W3 | under-cleared unseal → 403 + deny receipt on the live receipt chain |
| **B6** live broker | W2 (AWS/Moto), W7 (GCP emulator), W8 (Azure emulator) | fail-closed on bad scope/bad token; TTL capped to trust token |
| **R6** live revocation | W5, W9, G9 | revoked token denied offline; deprovision revokes everywhere; kill-switch halts next privileged call |
| **L4** live ledger | W4 (+ G7 receipt chain) | signed evidence pack verifies off-host |

**Scope honesty:** green here means the canonical live suite's assertions passed against live
services. Where a PSPR bullet's full sub-matrix (e.g. A5's "two tenants × two workload
identities", T7's complete adversarial matrix) exceeds what the suite currently asserts,
closing the plan checkbox remains the remediation owner's call — this record is the live-leg
evidence, not the checkbox. The GCP/Azure legs use the repo's local mock-emulators of the
IAM-Credentials/AD contracts (no free emulator exists); a real-cloud run stays owner-gated,
as documented in `deployment/live-integration/LIVE-INTEGRATION.md`.

## Redaction

One Moto-issued demo AccessKeyId in the raw log is redacted (`ASIA…REDACTED`) per the
evidence hard rule (no cloud-credential material in evidence), plan §0.5.

## Files

- `service-versions.txt` — service/toolchain/endpoint fingerprint captured at run time
- `live-suite-run.log` — full runner output; **local artifact only** (`*.log` gitignored),
  key lines embedded verbatim above; rerun `SKIP_DOCKER=1
  bash deployment/live-integration/run-live-suite.sh` against a live pair to regenerate
