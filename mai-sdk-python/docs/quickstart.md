# MAI SDK Quickstart

Connect to a local MAI inference server, send a chat request, stream
tokens, and inspect the trust/compliance context around the request.

## Install

```powershell
pip install -e mai-sdk-python
```

That also installs the `mai` CLI on your PATH.

## Configure

Three configuration sources are supported. Highest precedence wins:

1. Constructor arguments
2. Environment variables (`MAI_BASE_URL`, `MAI_API_KEY`, ...)
3. TOML file (`$MAI_CONFIG` or `~/.config/mai/config.toml`)

```toml
# ~/.config/mai/config.toml
base_url = "http://localhost:8420/v1"
api_key  = "im-..."
timeout  = 60.0

[retry]
max_retries = 3
base_delay  = 1.0
jitter      = 0.25
```

Your first-boot admin API key is printed to stdout once. Capture it
from the startup terminal and store it in your local config or shell
environment before continuing.

## First Request

```python
from mai import ChatMessage, MaiClient

with MaiClient.load() as client:  # picks up env + file config
    response = client.chat(
        "qwen3-14b:Q4_K_M",
        [ChatMessage(role="user", content="Say hi")],
    )
    print(response.choices[0].message.content)
```

Expected shape:

```text
Hi! How can I help you today?
```

The response is plain text extracted from the first choice. No JSON
wrapper, no role prefix. If you see an exception instead, check
[Troubleshooting](#troubleshooting).

## Streaming

```python
from mai import ChatMessage, MaiClient

with MaiClient.load() as client:
    for chunk in client.chat_stream(
        "qwen3-14b:Q4_K_M",
        [ChatMessage(role="user", content="Tell me a story")],
    ):
        delta = chunk.choices[0].get("delta", {}).get("content", "")
        print(delta, end="", flush=True)
```

Tokens arrive incrementally and print without newlines between them.
The cursor stays at the end of the last token until the stream closes.
If nothing prints and no exception raises, the model may still be
warming up; wait a few seconds and retry.

## Async

```python
import asyncio

from mai import AsyncMaiClient, ChatMessage


async def main() -> None:
    async with AsyncMaiClient.load() as client:
        async for chunk in client.chat_stream(
            "qwen3-14b:Q4_K_M",
            [ChatMessage(role="user", content="Hello")],
        ):
            delta = chunk.choices[0].get("delta", {}).get("content", "")
            print(delta, end="", flush=True)


asyncio.run(main())
```

## CLI

```powershell
mai health
mai chat "Tell me a joke" --model qwen3-14b:Q4_K_M
mai models list
mai benchmark qwen3-14b:Q4_K_M
mai power state
```

`MAI_BASE_URL` and `MAI_API_KEY` are read from the environment. Pass
`--base-url`, `--api-key`, or `--config PATH` to override them for one
command.

Run `mai health` first. In human-readable mode it prints the server
status, power state, air-gap verification flag, and uptime. Use
`mai health --json` when you want the raw response for scripts.

If `mai health` succeeds but a model call fails, the model alias may not
be loaded. Run `mai models list` to see what is available.

## Trust And Compliance Context

Every request routes through Lamprey's trust and compliance surfaces
before it reaches the model. You can inspect the local trust state at
any time:

```python
from mai import MaiClient

with MaiClient.load() as client:
    status = client.trust.status()
    print(status.model_dump_json(indent=2))
```

Expected shape on a healthy local-dev node:

```json
{
  "mode": "connected",
  "bundle_version": "local-dev",
  "last_refresh_secs": 1779540000,
  "age_secs": 0,
  "claim_count": 1,
  "airgap": {},
  "offline_backlog": 0
}
```

`mode` tells you where the node stands in the trust state machine.
`connected` means the signed bundle is current. `degraded` or
`stale_not_expired` means the node is operating on a cached bundle and
may restrict certain routes until a fresh bundle arrives. `expired`
means regulated request types are blocked or constrained.

To inspect recent compliance decisions:

```python
from mai import MaiClient

with MaiClient.load() as client:
    audit = client.compliance.query_audit(limit=10)
    for row in audit.rows:
        entry = row.entry
        print(entry.get("decision"), entry.get("policy_version"))
```

For the full compliance surface, including policy templates,
HIPAA/ITAR/OCAP routing, signed reports, and buyer integration notes,
see the [API reference](api-reference.md) and the
[Buyer Integration Guide](../../docs/BUYER-INTEGRATION-GUIDE.md).

## Troubleshooting

### Connection Refused

```text
MaiError: connection refused at http://localhost:8420/v1
```

The `mai-api` server is not running or `MAI_BASE_URL` points to the
wrong host. Start the server, wait for it to finish startup, then retry:

```powershell
cargo run --bin mai-api
```

### Authentication Failure

```text
AuthenticationError: 401 Unauthorized
```

Your API key is missing or wrong. Set it in the shell:

```powershell
$env:MAI_API_KEY = "im-..."
```

Or add it to your TOML config:

```toml
api_key = "im-..."
```

Do not commit API keys to source control.

### Model Unavailable

If health succeeds but chat fails with a model error, list available
models and choose one that supports chat:

```powershell
mai models list
```

### Air-Gap Or Policy Refusal

If a request is refused under air-gap, HIPAA, ITAR/EAR, or OCAP policy,
the SDK is reporting a governance decision rather than a transport
failure. Check trust and compliance status:

```python
from mai import MaiClient

with MaiClient.load() as client:
    print(client.trust.status().model_dump_json(indent=2))
    print(client.compliance.get_status().model_dump_json(indent=2))
```

## Next Steps

- [API reference](api-reference.md)
- [Streaming patterns](streaming.md)
- [Error handling](error-handling.md)
- [Authentication](authentication.md)
- [Buyer Integration Guide](../../docs/BUYER-INTEGRATION-GUIDE.md)
- Examples in [`examples/`](examples/)
