# Local Secure Inference

The smallest end-to-end path through MAI: a local model is selected,
a chat request is sent, the reply streams back token by token, and the
scheduler reports what it did. No cloud call, no external dependency --
everything runs on the local node.

This is the baseline proof that the SDK, API server, and a local adapter
are correctly wired before adding compliance routing, trust claims, or
governance overlays.

## What It Demonstrates

- `MaiClientConfig.load()` precedence: overrides -> env -> file -> defaults
- `client.health_check()` reachability probe before any inference call
- `client.models.list()` with a capability filter for chat-capable models
- `client.chat_stream()` SSE streaming with per-token output
- `client.scheduler.metrics()` as an observability footer after the call

## Run

```powershell
$env:MAI_API_KEY = "im-..."
python apps/local-secure-inference/main.py "Tell me a joke."
```

Expected output:

```
[health] OK -- mai-api reachable at localhost:8420
[model]  selected: qwen3-14b:Q4_K_M (chat-capable, local)

Why don't scientists trust atoms?
Because they make up everything.

[scheduler] queue=0 active=0 p95=18.4ms gpu=RTX4090 vram_used=9.1GB
```

If `[health]` fails, verify `mai-api` is running with
`scripts/health-check.sh`. If `[model]` shows no models, at least one
adapter must be started and have a model loaded.

## Configure

Edit [`config.toml`](config.toml). All keys are optional. The most
common edit is setting an explicit `[chat] model = "qwen3-14b:Q4_K_M"`
instead of `"auto"`.

## Tests

```powershell
pytest apps/local-secure-inference/tests/
```

`test_smoke.py` -- starts, hits a mocked health endpoint, lists models,
sends one chat. Verifies config loads correctly and the model selection
logic picks a chat-capable model.

`test_integration.py` -- full pick-model and streaming round trip using
`httpx.MockTransport` for the server. Confirms token-by-token streaming
and the scheduler metrics footer.

## Extending

This demo is a clean starting point for more complex integrations:

- **Multi-turn history:** track `messages` across `run()` calls to
  maintain conversation context.
- **Profile selection:** pass `profile_id` via `MaiClientConfig` to
  scope requests to a specific user or role.
- **Power-state-aware routing:** call `client.power.get_state()` and
  drop to short-prompt mode when the server is in Sentinel state.
