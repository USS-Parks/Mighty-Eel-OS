# MAI Security Posture (Session 26: Auth Hardening)

This document describes the authentication, authorization, and rate-limiting
guarantees implemented in `mai-api`. It is the trust floor for everything
built on top — the compliance governance layer (Lamprey, Sessions 36-46)
inherits these primitives.

## Authentication

### API Key Authentication

Every non-health endpoint requires a valid API key sent via the
`X-IM-Auth-Token` header. Keys are 32 bytes of OS-CSPRNG entropy
(`rand::rngs::OsRng`), hex-encoded, and prefixed with `im-` for
identifiability. The raw key is exposed exactly once (at first boot) and
never written to disk by the server.

Keys are stored as SHA3-256 hashes in `config/auth_keys.toml`. The server
hashes incoming keys at validation time and compares to the stored hash.
A forgotten key cannot be recovered; it must be rotated.

### First-Boot Admin Key

If `config/auth_keys.toml` is absent on startup, the server generates an
admin key and prints both the raw key and its hash to stdout. The operator
must:

1. Save the raw key in a secure location (password manager, vault).
2. Add the hash to `config/auth_keys.toml` under `[[keys]]`.
3. Restart the server.

The raw key never appears in the tracing log, audit log, or any persistent
artifact.

### Profile Header Spoofing

The legacy `X-IM-Profile` header (Session 14c compatibility) is **disabled
by default**. It is honored only when `settings.allow_internal_profile_header
= true` is set in `config/auth_keys.toml`. Set this only for trusted
service-to-service calls inside a hardened internal network.

## Authorization

### Role Hierarchy

Five roles, in decreasing trust:

| Role  | Inference | List Models | Manage Models | Power Control | Manage Profiles |
|-------|:---------:|:-----------:|:-------------:|:-------------:|:---------------:|
| Admin | ✓         | ✓           | ✓             | ✓             | ✓               |
| Adult | ✓         | ✓           |               |               |                 |
| Teen  | ✓ (filtered) | ✓        |               |               |                 |
| Child | ✓ (filtered) | ✓        |               |               |                 |
| Guest | sentinel only |          |               |               |                 |

`check_permission(profile, "permission_name")` is the single entry point for
authorization. Handlers must call it before mutating state or accessing
restricted endpoints. Unknown permission names default to deny.

### Model Access Filtering

Profiles may carry a `model_filter` that constrains which models they can
target: `TeenSafe`, `ChildSafe`, or `DefaultOnly`. `can_access_model()` is
called by inference handlers before placement.

## Rate Limiting

Per-key sliding-window rate limiter (default: 60 requests per 60-second
window). Excess requests return `429 Too Many Requests` with a
`Retry-After` value derived from the oldest timestamp in the window.

Rate limits are independent per key, so revoking and reissuing a key resets
the budget.

## Acceptance Criteria (Gate A)

All of the following are verified by `mai-api/tests/auth_gate_a.rs`:

- [x] Missing `X-IM-Auth-Token` returns `401 Unauthorized`.
- [x] Invalid `X-IM-Auth-Token` returns `401 Unauthorized`.
- [x] Valid token reaches authorized endpoints (no 401/403 from middleware).
- [x] Burst beyond the rate-limit returns `429 Too Many Requests`.
- [x] `X-IM-Profile` alone is rejected when
      `allow_internal_profile_header = false`.
- [x] `/v1/health` is auth-exempt in strict mode.
- [x] First-boot admin key is printed once, never logged.

## SDK Integration

### Python SDK (`mai-sdk-python`)

`MaiClient(MaiClientConfig(api_key="im-..."))` sets the
`X-IM-Auth-Token` header on every request.

### Rust SDK (`mai-sdk-rs`)

`MaiClientConfig::api_key` is `Option<String>`. The HTTP layer (Session 11
finish-out) reads `MaiClientConfig::auth_headers()` and applies the
returned pairs to every outbound request. Either `api_key` or `profile_id`
must be set or `MaiClient::new()` returns a config error.

## What This Does Not Cover

Vault crypto and air-gap enforcement are Sessions 27 and 28 respectively.
This document covers only the authentication and authorization surface.
Audit logging of auth decisions (success and failure) is wired through
`mai-api::audit` but its tamper-evident hash chain is the Session 42
deliverable.
