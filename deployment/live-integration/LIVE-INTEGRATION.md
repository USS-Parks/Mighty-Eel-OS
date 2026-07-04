# Live-integration suite (D5)

The suite MAI never had: every **trust-adjacent** path is verified against **live
backends** — a real OpenBao (dev) for the trust core, a real Moto for AWS STS,
local mock-emulators for GCP/Azure (no free emulator exists; a real-cloud run is
owner-gated). This closes the **no-mock-only** rule (§0.3.5): a guarantee proven
only against a mock is not proven.

## Run it

```bash
# One command: brings up OpenBao + Moto, runs the whole suite, prints a summary.
bash deployment/live-integration/run-live-suite.sh

# Reuse an already-running openbao+moto pair:
SKIP_DOCKER=1 bash deployment/live-integration/run-live-suite.sh
```

CI runs the identical set in the `wsf-live` job (`.github/workflows/ci.yml`) on
every push/PR, against a Dockerized OpenBao + Moto — so local and CI exercise one
suite, not two.

## Zero-mock-only coverage map (trust-adjacent path → live test → backend)

| Trust-adjacent path | Live test | Live backend |
|---|---|---|
| Token issue / verify / ML-DSA sign (bridge) | `wsf-bridge :: live_openbao` | OpenBao (AppRole + KV) |
| AWS STS credential broker | `wsf-broker :: live_localstack` | OpenBao + **Moto** STS |
| GCP credential broker | `wsf-broker :: live_gcp` | OpenBao + mock IAM-Credentials |
| Azure credential broker | `wsf-broker :: live_azure` | OpenBao + mock AD token |
| Envelope seal / unseal (Transit-wrapped key) | `wsf-seal :: live_seal` | OpenBao **Transit** |
| Receipt ledger + signed evidence pack | `wsf-ledger :: live_ledger` | OpenBao (real multi-service receipts) |
| Ring-3 offline decisions (air-gap) | `wsf-cache :: live_cache` | OpenBao (real token + revocation) |
| Tenant provision → deprovision → revoke-everywhere | `wsf-tenants :: live_tenants` | OpenBao (KV + revocation) |
| Unified REST API + typed SDK round-trip | `wsf-api :: live_api` | OpenBao + Moto |
| Gateway virtual-key → trust-token auth + budget pre-flight | `aog-gateway :: live_gateway` | OpenBao KV |
| Budget exhaustion + **revocation kill-switch** | `aog-gateway :: kill_switch` | OpenBao KV (bridge-signed snapshot) |
| OpenAI surface + classify/route | `aog-gateway :: openai_surface` | OpenBao |
| Anthropic surface | `aog-gateway :: anthropic_surface` | OpenBao |
| Deny-wins policy + shadow/report/enforce | `aog-gateway :: policy_modes` | OpenBao |
| Metering + verifiable receipt chain | `aog-gateway :: metering` | OpenBao |

Every path that issues a token, seals/unseals an envelope, brokers a credential,
writes a receipt, or makes a policy decision has a **live** test. There is **no**
trust-adjacent path whose only coverage is a mock.

## Pure-logic paths (real crypto, no live backend needed)

The M2 tool-governance and evidence layers are **not** trust-*backend* paths — they
are deterministic logic + hash-chains over the already-live-tested `fabric-proof` /
`fabric-token` / `fabric-crypto` primitives, so they are fully proven by their unit
suites with **real** BLAKE3 / ML-DSA-87 (not mocked), no OpenBao required:

- **T5–T8** (`aog-toolproxy`, `aog-approvals`) — egress redaction, mission contracts,
  session replay, guardrails: pure enforcement + real hash-chained receipts.
- **D4** (`hipaa-pack`) — PHI governance + §164.312 evidence mapping over the real
  ML-DSA-signed `wsf-ledger` pack.

## Owner-gated remainders (honest)

- **GCP / Azure real-cloud** — the live tests use local mock-emulators of the
  IAM-Credentials / AD token contracts (no free emulator exists). A real-cloud run
  needs owner credentials.
- **Live OpenBao HA** — the suite runs against **dev-mode** OpenBao. A live HA
  (Raft) topology + key-rotation-under-load is the D7 burn-in.
- **Real OpenAI/Anthropic** — the surface tests assert the wire contract those SDKs
  depend on against a mock upstream; a real-key run is owner-gated.
