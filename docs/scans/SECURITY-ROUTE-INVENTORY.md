# Security Route Inventory & Privilege Matrix

Every externally-reachable entry point across WSF / AOG / MAI, with its authentication,
tenant, and audit posture. The machine-readable source of truth for the enforced HTTP
subset is [`.integrity/route-policy.tsv`](../../.integrity/route-policy.tsv), gated at
pre-push by `.integrity/scripts/route-policy-check.sh`: a production HTTP route with no
policy row fails the push (negative-control verified). gRPC, SSE, WebSocket, and CLI
surfaces are inventoried below; extending the automated gate to them is noted at the end.

Enforced HTTP surface: **79 routes** across three services.

## WSF trust plane — `crates/wsf-api` (9 HTTP routes)

The privileged plane. No HTTP-layer authentication — authorization is payload-token based
(a pre-signed `TrustToken` in the request body), and several routes have the gaps the audit
flagged. Network-contained in 0.2 (no longer host-published in production/HA).

| Method + Path | Handler | Auth / tenant | Finding |
|---|---|---|---|
| POST /v1/tokens/issue | `issue` (lib.rs:225) | none; mints a token from caller-supplied tenant/subject/roles | AF-002 |
| POST /v1/tokens/verify | `verify` (lib.rs:256) | none; signature + expiry check only (read) | — |
| POST /v1/tokens/attenuate | `attenuate` (lib.rs:279) | none; signs a caller-supplied child with no parent verification | AF-001 (Critical) |
| POST /v1/envelopes/seal | `seal` (lib.rs:288) | payload token; no tenant/owner binding | AF-003 |
| POST /v1/envelopes/unseal | `unseal` (lib.rs:311) | payload token; no tenant/owner binding | AF-003 |
| POST /v1/credentials/exchange | `exchange` (lib.rs:333) | token verified; caller-supplied `role_arn` | AF-004 |
| GET /v1/receipts | `receipts` (lib.rs:361) | none; arbitrary field/value query, no tenant filter | AF-007 |
| GET /openapi.json | `openapi` | public | — |
| GET /healthz | inline | public | — |

## AOG gateway — `crates/aog-gateway` (10 HTTP routes)

Per-handler `authorize()`: extracts `Authorization: Bearer <virtual-key>`, resolves it to a
token, verifies the signature, checks the budget. No global middleware, but every non-health
route calls `authorize()` before work; `/healthz` is public.

OpenAI surface (`surface_openai.rs`): /v1/chat/completions, /v1/completions, /v1/embeddings,
/v1/models, /v1/usage, /v1/roi, /v1/status, plus /v1/preflight (`http.rs`). Anthropic surface
(`surface_anthropic.rs`): /v1/messages. Public: /healthz.

## MAI API — `mai-api` (60 HTTP + ~20 gRPC + SSE + WebSocket)

Global `auth_middleware` (`routes.rs`) requires `X-IM-Auth-Token` validated against the
`ApiKeyStore`; per-route `check_permission(profile, "...")` gates privileged actions.
`/v1/health*` and `/v1/metrics` are exempt by path prefix (metrics is additionally
host-local). Grouped:

- **Inference:** /v1/chat/completions, /v1/completions, /v1/embeddings, /v1/generate/structured, /v1/generate/function_call (`inference`).
- **Models (admin):** /v1/models, /v1/models/{id}, /v1/models/{id}/{load,unload,remove,benchmark}, /v1/models/install, /v1/models/discover (`manage_models` for mutating ops).
- **System / power:** /v1/system/airgap, /v1/system/production-readiness, /v1/power, /v1/power/state, /v1/power/transition (`power_control`), /v1/registry, /v1/registry/scan, /v1/adapters, /v1/audit/log, /v1/profiles, /v1/profiles/{id}.
- **Trust:** /v1/trust/{status,claims,bundle_status,revocation_status,openbao_health}, /v1/trust/refresh, /v1/admin/rotate-credentials (admin).
- **Compliance:** /v1/compliance/{status,policies,policies/*,modules/*,audit,audit/*,reports,reports/*,feed} (admin for reload/template/enable/disable/update/delete).
- **Updates:** /v1/updates/{check,download,status}.
- **Scheduler telemetry:** /v1/scheduler/{metrics,anomalies,instances/{id}/metrics,instances/{id}/health}.
- **Exempt (no auth):** /v1/health, /v1/health/{adapters,hardware,system,resources,live,ready,production}, /v1/metrics (host-local).
- **gRPC (tonic):** Mai{Inference,Models,Health,Power,Registry,Audit}Service methods — `extract_grpc_profile()` + `role_has_permission(...)`; standard `grpc.health.v1.Health` unauthenticated by design.
- **Streaming:** SSE on POST /v1/chat/completions (`stream=true`) — `inference`; WebSocket /v1/ws — upgrade succeeds pre-auth, then requires an `auth.handshake` first message before any inference.

Flags carried forward:
- `/v1/auth/exchange_token` (`routes.rs`) is a local-dev stub with no visible validation (`stub-no-validation` in the policy file). Must be gated or removed for production — folded into the Phase-A / F1 review.
- `/v1/ws` authenticates post-upgrade (a client can hold an open socket before the handshake) — noted for the F1 MAI-stream audit.

## CLI / administrative (local, profile-file gated)

| Tool | Action | Gate |
|---|---|---|
| `mai-api validate` | validate a ship profile | `--profile` path |
| `mai-admin backup {create,verify}` | backup + verify | `--profile`, signing key |
| `mai-admin restore {plan,apply}` | restore | `--backup_dir` / `--target`, `--force`, `--require-signed` |
| `mai-admin demo {all,run}` | run compliance demos | none (demo) |
| `wsf-seed` | one-shot OpenBao provision | env vars (`WSF_OPENBAO_*`) |

Not network-reachable; each requires a local profile / backup path.

## Enforcement + gaps

- **Enforced now:** the 79-route HTTP policy file + pre-push gate — a new `.route(...)` with no policy row fails the push.
- **Inventoried, not yet auto-gated:** gRPC methods, SSE, WebSocket, CLI. The gate is scoped to axum HTTP route literals; extending it to tonic services and clap subcommands is future hardening (F-phase).
- **Enforcement points:** the gate runs in GitHub Actions (`.github/workflows/ci.yml`, `config-check` job) and at pre-push (beside the no-slop scan). The repo has a full CI suite — `ci.yml` (check / clippy `-D warnings` / fmt / test / cargo-audit / cargo-deny), a live-OpenBao + Moto trust gate (`wsf-live`), plus ship-validation / supply-chain / commit-msg-check workflows — on top of the `core.hooksPath` hook layer.
