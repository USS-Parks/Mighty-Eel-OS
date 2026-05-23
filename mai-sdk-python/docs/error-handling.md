# Error Handling

All SDK exceptions inherit from `mai.MaiError`. Catch the base class
when you only need to know "the call failed". Catch a subclass when
the response shape matters.

## Hierarchy

```
MaiError
    BadRequestError              HTTP 400 -- your request was invalid
    AuthenticationError          HTTP 401 -- missing or invalid credentials
        ClaimExpiredError        -- trust claim expired
    PermissionError              HTTP 403 -- not allowed
        AirGapViolationError     -- request refused by air-gap policy
    NotFoundError                HTTP 404
    RateLimitError               HTTP 429 -- `retry_after` is set
    ServerError                  HTTP 5xx
    PowerStateUnavailableError   server is in a low-power or sentinel state
    ConnectionError              network failed before reaching the server
    TimeoutError                 request exceeded its timeout
    TrustCacheStaleError         local trust cache stale or expired
```

## Catch by Class

```python
from mai import (
    MaiClient, MaiError, AuthenticationError, RateLimitError,
    ConnectionError as MaiConnectionError,
)

with MaiClient.load() as client:
    try:
        response = client.chat("...", messages)
    except AuthenticationError:
        # rotate credentials, prompt the user
        ...
    except RateLimitError as e:
        # honor server backoff hint
        time.sleep(e.retry_after or 1)
    except MaiConnectionError:
        # network down; try later or surface
        ...
    except MaiError as e:
        # anything else -- log e.code, e.message, e.request_id
        log.error("MAI %s: %s", e.code, e.message)
```

## Fields Available on Any MaiError

| Field         | Description                                        |
| ------------- | -------------------------------------------------- |
| `message`     | Human-readable string (same as `str(e)`)           |
| `code`        | MAI error code (`MAI-XYYY`)                        |
| `error_type`  | Server-side classification                         |
| `status_code` | HTTP status (if a response was returned)           |
| `retry_after` | Seconds (set on RateLimit and some 5xx)            |
| `request_id`  | Server request id, when present                    |
| `is_retryable`| Convenience boolean                               |

## Auto-Retry

The SDK's `RetryPolicy` retries 429/500/502/503/504, connection
errors, and timeouts. It does NOT retry 400/401/403/404. The error
that ultimately surfaces is the one from the last attempt.

## Common Failure Scenarios

### Auth token missing or invalid

`AuthenticationError` is raised when the server returns HTTP 401.
This means `MAI_API_KEY` was not set, was set to a value not present
in `config/auth_keys.toml`, or the trust claim attached to the request
has expired (`ClaimExpiredError`, a subclass).

```python
from mai import MaiClient, AuthenticationError
from mai._namespaces import ClaimExpiredError

try:
    response = client.chat("llama3", messages)
except ClaimExpiredError:
    # re-exchange the token and retry
    client.auth.exchange_token(refresh=True)
except AuthenticationError:
    # key missing or revoked -- operator action required
    raise SystemExit("MAI_API_KEY is missing or invalid. Check config/auth_keys.toml.")
```

### Server unreachable

`ConnectionError` is raised when the SDK cannot reach `mai-api` at
all -- the process is not running, the port is wrong, or the host is
not accessible. This is distinct from a server error (HTTP 5xx), which
means the server was reached but something failed internally.

```python
from mai import MaiClient
from mai import ConnectionError as MaiConnectionError

try:
    response = client.chat("llama3", messages)
except MaiConnectionError as e:
    print(f"Cannot reach MAI server: {e}")
    print("Verify mai-api is running: scripts/health-check.sh")
```

### Model unavailable

`NotFoundError` (HTTP 404) is raised when the requested model name
is not registered with the scheduler. This happens when the model
weights have not loaded, the model alias is wrong, or the adapter
serving that model has not started.

```python
from mai import MaiClient, NotFoundError

try:
    response = client.chat("llama3-70b", messages)
except NotFoundError:
    # list what is actually available
    models = client.models.list()
    available = [m.id for m in models.data]
    print(f"Model not found. Available: {available}")
```

### Air-gap policy refusal

`AirGapViolationError` (a subclass of `PermissionError`) is raised
when the request would require routing to a backend that violates the
active air-gap policy. The server refuses before any inference call is
made. This is expected behavior in hardened deployments -- it confirms
the policy is working, not that something is broken.

```python
from mai import MaiClient
from mai._namespaces import AirGapViolationError

try:
    response = client.chat("cloud-model", messages)
except AirGapViolationError as e:
    # the route was refused by policy -- use a local model instead
    print(f"Air-gap policy refused this route: {e.message}")
    response = client.chat("local-model", messages)
```

### Server in low-power or sentinel state

`PowerStateUnavailableError` is raised when the server is in a
reduced-power state (sentinel, sleep, or throttled) and cannot accept
full inference requests. The server will promote back to full inference
when workload or schedule conditions are met.

```python
from mai import MaiClient
from mai._namespaces import PowerStateUnavailableError
import time

for attempt in range(3):
    try:
        response = client.chat("llama3", messages)
        break
    except PowerStateUnavailableError:
        # server is warming up; wait and retry
        time.sleep(5)
else:
    raise RuntimeError("Server did not promote to full inference state.")
```

## Trust-Specific Errors

`client.trust.*` and `client.auth.exchange_token` raise
`TrustNotProvisionedError` (a subclass of `MaiError`) when the trust
namespace is not reachable or the bundle has not been initialized.
Production code can branch on this to fall back to API-key auth.

```python
from mai._namespaces import TrustNotProvisionedError

try:
    status = client.trust.bundle_status()
except TrustNotProvisionedError:
    status = None  # operate in API-key mode
```
