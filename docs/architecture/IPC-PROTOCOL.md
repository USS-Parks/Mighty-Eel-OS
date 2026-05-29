# MAI Adapter IPC Protocol Specification

**Version:** 1.0
**Session:** 14a
**Date:** 2026-05-19
**Status:** Active

---

## Overview

The MAI adapter framework communicates with Python adapter subprocesses over
stdin/stdout using newline-delimited JSON (NDJSON). Each adapter runs as a
long-lived child process. The Rust `AdapterProcess` writes requests to the
child's stdin and reads NDJSON events from the child's stdout. Stderr is
reserved for Python logging and is never parsed by Rust.

## Process Lifecycle

### Spawning

The Rust side spawns:

```
python3 runner.py <adapter_name>
```

One positional argument only: the adapter name (e.g., `ollama`, `vllm`).
No flags, no module path, no class name. The runner resolves those internally
via the `@mai_adapter` registry.

Environment variables set by the Rust side:

| Variable | Value | Purpose |
|---|---|---|
| PYTHONPATH | `<adapters_dir>` | Module resolution |
| MAI_ADAPTER_NAME | `<adapter_name>` | Redundant, for logging |
| MAI_LOG_LEVEL | `INFO` | Stderr log level |

### Startup Handshake (Phase 1)

After spawn, the Rust side sends a JSON config object on stdin (one line):

```json
{"adapter_name": "ollama", "module_path": "adapters.ollama.adapter", "entry_class": "OllamaAdapter", "config": {"host": "127.0.0.1", "port": 11434}}
```

Fields:

| Field | Type | Required | Description |
|---|---|---|---|
| adapter_name | string | yes | Adapter identifier |
| module_path | string | yes | Python import path to the adapter module |
| entry_class | string | yes | Class name to instantiate |
| config | object | no | Adapter-specific configuration dict |

The Python runner reads this line, imports the module, instantiates the class,
calls `initialize(config)`, then writes a capability handshake response on
stdout (one line):

```json
{"type": "handshake", "adapter_name": "ollama", "version": "1.0.0", "handle": "ollama-abc123", "capabilities": {"max_context_window": 131072, "supports_streaming": true, "supports_embedding": true, "supports_batching": false, "supports_structured_output": false, "supports_vision": false, "supports_tool_calling": false, "supports_continuous_batching": false, "supports_hot_swap": false, "supported_quantizations": [], "backend_version": "0.5.4"}}
```

The Rust side reads this first line, parses it, caches capabilities, and marks
the process as Running.

**Timeout:** If no handshake line arrives within 30 seconds, the Rust side
kills the process and reports `InitFailed`.

### Request Loop (Phase 2)

After the handshake, the process enters a request-response loop. The Rust side
writes one JSON request per line on stdin. The Python side writes one or more
NDJSON response events per line on stdout.

## Request Format

Every request is a single JSON line on stdin:

```json
{"request_id": "<uuid>", "type": "<method>", "payload": {}}
```

| Field | Type | Description |
|---|---|---|
| request_id | string (UUID) | Unique per-request. Used to correlate response events. |
| type | string | Method name: `inference`, `health`, `capabilities`, `shutdown`, `heartbeat` |
| payload | object | Method-specific parameters |

### Inference Payload

```json
{
  "prompt": "What is the capital of France?",
  "params": {
    "temperature": 0.7,
    "top_p": 0.9,
    "max_tokens": 512,
    "stop_sequences": [],
    "structured_schema": null
  },
  "stream": true
}
```

### Health Payload

Empty object: `{}`

### Capabilities Payload

Empty object: `{}`

### Shutdown Payload

Empty object: `{}`

### Heartbeat Payload

Empty object: `{}`

## Response Format (NDJSON Events)

Every response is one or more NDJSON lines on stdout. Each line includes the
`request_id` so the Rust side can correlate events to pending requests.

### Token Event

Emitted zero or more times during streaming inference.

```json
{"request_id": "<uuid>", "type": "token", "text": "Paris", "logprob": -0.5, "index": 0, "finish_reason": null}
```

| Field | Type | Description |
|---|---|---|
| text | string | Token text |
| logprob | float or null | Log probability |
| index | int | Token position in the stream |
| finish_reason | string or null | `null` while streaming, `"stop"` or `"max_tokens"` on final token |

### Usage Event

Emitted once per inference request, after all tokens.

```json
{"request_id": "<uuid>", "type": "usage", "prompt_tokens": 42, "completion_tokens": 7}
```

### Result Event

Emitted once for non-streaming methods (health, capabilities, heartbeat).
Contains the complete response payload.

```json
{"request_id": "<uuid>", "type": "result", "data": {"status": "healthy", "uptime_ms": 12345}}
```

### Done Event

Emitted exactly once per request. Signals request completion.

```json
{"request_id": "<uuid>", "type": "done"}
```

The Rust side removes the request from its pending map upon receiving `done`.

### Error Event

Emitted when a request fails. Followed by `done`.

```json
{"request_id": "<uuid>", "type": "error", "code": "OOM", "message": "CUDA out of memory"}
```

| Field | Type | Description |
|---|---|---|
| code | string | Error code matching AdapterError taxonomy |
| message | string | Human-readable description |

Error codes: `Timeout`, `OutOfMemory`, `ModelNotFound`, `BackendCrashed`,
`BackendUnavailable`, `ContextExceeded`, `RateLimited`, `HardwareFault`,
`ValidationError`, `UnsupportedOperation`, `InternalError`.

## Event Ordering Guarantees

For inference requests:
1. Zero or more `token` events (in order)
2. Exactly one `usage` event
3. Exactly one `done` event

For non-streaming methods:
1. Exactly one `result` event
2. Exactly one `done` event

For errors:
1. Exactly one `error` event
2. Exactly one `done` event

## Process Crash Detection

The Rust side detects process death via:

1. **EOF on stdout:** The BufReader returns `None`. Process is marked Crashed.
2. **`try_wait()` polling:** Periodic check returns `Some(ExitStatus)`.
3. **Heartbeat timeout:** If `heartbeat` request gets no response within the
   configured timeout, the health monitor declares the adapter dead.

Crash recovery uses exponential backoff: base 1s, cap 60s, max 10 attempts.
After max attempts, the adapter is marked Failed permanently.

## Graceful Shutdown

1. Rust sends `{"request_id": "...", "type": "shutdown", "payload": {}}`
2. Python calls `adapter.shutdown()`, sends `{"request_id": "...", "type": "result", "data": {"ok": true}}` then `done`
3. Rust waits up to 5 seconds for the done event
4. If no done within 5s, Rust kills the process

## Implementing a New Adapter

To create a new adapter that works with this IPC protocol:

1. Create `adapters/<name>/adapter.py`
2. Inherit from `AdapterBase`
3. Decorate with `@mai_adapter(name="<name>", version="x.y.z")`
4. Implement all abstract methods (`initialize`, `generate`, `embed`, etc.)
5. The runner handles all IPC serialization. The adapter never reads stdin
   or writes stdout directly.

## Wire Compatibility

The NDJSON protocol is versioned. The handshake response includes the adapter
version. Future protocol changes will be negotiated during the handshake by
adding a `protocol_version` field. Version 1.0 is defined by this document.

## Stderr Convention

All Python logging goes to stderr via the standard `logging` module. The Rust
side does not parse stderr. Stderr is inherited or piped to the parent's log
aggregator for debugging.

---

*Session 14a deliverable | Island Mountain AI | Confidential*
