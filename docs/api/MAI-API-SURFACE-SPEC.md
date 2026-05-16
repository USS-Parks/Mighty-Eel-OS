# MAI API Surface Specification

## Session 05 Deliverable | Island Mountain MAI OS

**Document Version**: 0.1.0
**Date**: 2026-05-16
**Phase**: A (Specification)
**Depends On**: Sessions 03 (Adapters), 04 (Core Kernel)
**Blocks**: Sessions 11, 12, 15, 16

---

## 1. Overview

The MAI API Surface is the syscall interface in the Tock trust model: the stable boundary between untrusted applications (L4-L5) and the trusted inference kernel (L3). Every application, from Summit Chat to Legacy Engine to MedRecord Vault, calls the MAI through this contract. It never breaks backward compatibility.

This specification defines:

- REST API endpoints (OpenAI-compatible where applicable)
- Streaming protocols (SSE and WebSocket)
- gRPC service definitions for high-performance internal use
- Authentication and family profile system (local-only)
- Error contract with code taxonomy
- Air-gap verification protocol

### 1.1 Design Principles

1. **OpenAI Compatibility**: `/v1/chat/completions` and `/v1/embeddings` match the OpenAI request/response schema so ecosystem tools (LangChain, LlamaIndex, OpenWebUI) work without modification.
2. **Air-Gap First**: Every endpoint functions identically whether the device is air-gapped or connected. No endpoint depends on external network access. The API refuses to start if air-gap policy is violated.
3. **Backend Opacity**: Error responses never leak backend-specific details. Applications see MAI error codes, never Ollama/vLLM/TGI internals.
4. **Local-Only Auth**: No OAuth, no cloud tokens, no external identity providers. Authentication is local TPM-backed profile tokens.
5. **Audit Everything**: Every API call is logged with profile, timestamp, model, token count, latency, and response status. The audit trail is append-only, hash-chained, and PQC-signed.
6. **Streaming First**: Chat completions default to streaming. Non-streaming is the opt-in path.

### 1.2 Type Alignment with Internal Contracts

All API request/response types are projections of the internal types defined in `mai-core`. The API server (Session 11) converts between API types and internal types at the boundary. This table maps the critical alignments:

| API Type | Internal Type | Source |
|---|---|---|
| `ChatCompletionRequest.messages` | `scheduler::ChatMessage` | scheduler.rs |
| `ChatCompletionRequest.temperature/top_p/max_tokens` | `adapter::GenerationParams` | adapter.rs |
| `ChatCompletionResponse.choices[].finish_reason` | `adapter::FinishReason` | adapter.rs |
| `EmbeddingResponse.data[].embedding` | `adapter::Embedding.vector` | adapter.rs |
| `ModelObject.id` | `types::ModelId` | types.rs |
| `ModelObject.capabilities` | `registry::CapabilityInfo` | registry.rs |
| `HealthResponse` | `health::HealthSnapshot` | health.rs |
| `PowerStateResponse.state` | `power::PowerState` | power.rs |
| `ErrorResponse.code` | `errors::CoreError` variant | errors.rs |
| `X-IM-Priority header` | `scheduler::RequestPriority` | scheduler.rs |
| `X-IM-Profile header` | `types::ProfileId` | types.rs |

---

## 2. REST API Specification

### 2.1 Base URL

```
http://localhost:8420/v1
```

Port 8420 is the default. Configurable via `mai.toml` (`[api] port = 8420`). Binds to `127.0.0.1` only. No TLS required for localhost (data never leaves the device).

### 2.2 Common Headers

**Request Headers:**

| Header | Type | Required | Description |
|---|---|---|---|
| `Content-Type` | string | Yes | `application/json` |
| `X-IM-Profile` | UUID | No | Family profile ID. If omitted, uses default adult profile. |
| `X-IM-Priority` | string | No | `low`, `normal`, `high`, `critical`. Maps to `scheduler::RequestPriority`. Default: derived from profile. |
| `X-IM-Request-Id` | UUID | No | Client-supplied request ID for correlation. Server generates one if omitted. |

**Response Headers:**

| Header | Type | Description |
|---|---|---|
| `X-IM-Request-Id` | UUID | Echoed or server-generated request ID |
| `X-IM-Model` | string | Actual model used (may differ from requested if fallback occurred) |
| `X-IM-Adapter` | string | Adapter class that served the request (e.g., `ollama`, `vllm`) |
| `X-IM-Tokens-In` | integer | Input token count |
| `X-IM-Tokens-Out` | integer | Output token count |
| `X-IM-Latency-Ms` | integer | Server-side processing time |
| `Retry-After` | integer | Seconds to wait before retry (only on 429/503) |

### 2.3 Inference Endpoints

#### 2.3.1 POST /v1/chat/completions

OpenAI-compatible chat completion endpoint. Primary inference entry point.

**Request Body:**

```json
{
  "model": "qwen3-14b:Q4_K_M",
  "messages": [
    {"role": "system", "content": "You are a helpful assistant."},
    {"role": "user", "content": "What is the capital of France?"}
  ],
  "temperature": 0.7,
  "top_p": 0.9,
  "max_tokens": 2048,
  "stream": true,
  "stop": ["\n\n"],
  "response_format": {"type": "json_object", "schema": {}},
  "tools": [
    {
      "type": "function",
      "function": {
        "name": "get_weather",
        "description": "Get current weather",
        "parameters": {"type": "object", "properties": {"city": {"type": "string"}}}
      }
    }
  ],
  "tool_choice": "auto"
}
```

| Field | Type | Required | Default | Description |
|---|---|---|---|---|
| `model` | string | Yes | -- | Model ID from registry. Format: `name:quantization` or `name:version:quantization` |
| `messages` | array | Yes | -- | Chat messages. Each: `{role, content}`. Roles: `system`, `user`, `assistant`, `tool` |
| `temperature` | float | No | 0.7 | Sampling temperature. 0.0 = deterministic |
| `top_p` | float | No | 0.9 | Nucleus sampling threshold |
| `max_tokens` | integer | No | 2048 | Maximum tokens to generate |
| `stream` | boolean | No | true | Enable SSE streaming |
| `stop` | array[string] | No | [] | Stop sequences |
| `response_format` | object | No | null | Structured output constraint. `type`: `json_object` or `json_schema` |
| `tools` | array | No | null | Tool/function definitions for function calling |
| `tool_choice` | string/object | No | `auto` | `auto`, `none`, or `{"type":"function","function":{"name":"..."}}` |

**Non-Streaming Response (stream=false):**

```json
{
  "id": "mai-req-550e8400-e29b-41d4-a716-446655440000",
  "object": "chat.completion",
  "created": 1747612800,
  "model": "qwen3-14b:Q4_K_M",
  "choices": [
    {
      "index": 0,
      "message": {
        "role": "assistant",
        "content": "The capital of France is Paris."
      },
      "finish_reason": "stop"
    }
  ],
  "usage": {
    "prompt_tokens": 24,
    "completion_tokens": 8,
    "total_tokens": 32
  }
}
```

**Streaming Response (stream=true):**

SSE format. Each event is a JSON delta:

```
data: {"id":"mai-req-...","object":"chat.completion.chunk","created":1747612800,"model":"qwen3-14b:Q4_K_M","choices":[{"index":0,"delta":{"role":"assistant","content":"The"},"finish_reason":null}]}

data: {"id":"mai-req-...","object":"chat.completion.chunk","created":1747612800,"model":"qwen3-14b:Q4_K_M","choices":[{"index":0,"delta":{"content":" capital"},"finish_reason":null}]}

data: {"id":"mai-req-...","object":"chat.completion.chunk","created":1747612800,"model":"qwen3-14b:Q4_K_M","choices":[{"index":0,"delta":{},"finish_reason":"stop"}],"usage":{"prompt_tokens":24,"completion_tokens":8,"total_tokens":32}}

data: [DONE]
```

**Status Codes:**

| Code | Condition | CoreError Mapping |
|---|---|---|
| 200 | Success | -- |
| 400 | Invalid request (bad JSON, missing model) | -- |
| 401 | Invalid or expired profile token | -- |
| 403 | Profile lacks permission for requested model | -- |
| 404 | Model not found in registry | `ModelUnavailable` |
| 422 | Structured output schema invalid | -- |
| 429 | Profile rate limit exceeded | -- |
| 503 | System overloaded or air-gap violation | `Overloaded` / `AirGapViolation` |
| 500 | Internal error (details not exposed) | `Internal` / `RequestFailed` |

#### 2.3.2 POST /v1/completions

Raw text completion (non-chat). Same parameter set minus `messages`, plus `prompt`.

**Request Body:**

```json
{
  "model": "qwen3-14b:Q4_K_M",
  "prompt": "Once upon a time",
  "temperature": 0.7,
  "top_p": 0.9,
  "max_tokens": 2048,
  "stream": true,
  "stop": ["\n\n"]
}
```

**Response:** Same structure as chat completions but `object` is `"text_completion"` and `choices[].text` replaces `choices[].message`.

#### 2.3.3 POST /v1/embeddings

OpenAI-compatible embedding endpoint.

**Request Body:**

```json
{
  "model": "nomic-embed-text:latest",
  "input": ["The capital of France is Paris.", "Berlin is the capital of Germany."]
}
```

| Field | Type | Required | Description |
|---|---|---|---|
| `model` | string | Yes | Embedding model ID |
| `input` | string or array[string] | Yes | Text(s) to embed |

**Response:**

```json
{
  "object": "list",
  "data": [
    {
      "object": "embedding",
      "index": 0,
      "embedding": [0.0023, -0.0091, ...],
      "input_tokens": 8
    },
    {
      "object": "embedding",
      "index": 1,
      "embedding": [0.0041, -0.0012, ...],
      "input_tokens": 9
    }
  ],
  "model": "nomic-embed-text:latest",
  "usage": {
    "prompt_tokens": 17,
    "total_tokens": 17
  }
}
```

Note: `input_tokens` per embedding is an Island Mountain extension not in the OpenAI spec. It maps to `adapter::Embedding.input_tokens`. Ecosystem tools ignore unknown fields.

#### 2.3.4 POST /v1/generate/structured

JSON schema-constrained generation. Separate from chat completions for explicit intent.

**Request Body:**

```json
{
  "model": "qwen3-14b:Q4_K_M",
  "prompt": "Extract the person's name and age from: John Smith is 42 years old.",
  "schema": {
    "type": "object",
    "properties": {
      "name": {"type": "string"},
      "age": {"type": "integer"}
    },
    "required": ["name", "age"]
  },
  "temperature": 0.0
}
```

**Response:**

```json
{
  "id": "mai-req-...",
  "object": "structured_output",
  "model": "qwen3-14b:Q4_K_M",
  "output": {"name": "John Smith", "age": 42},
  "usage": {"prompt_tokens": 28, "completion_tokens": 12, "total_tokens": 40},
  "schema_valid": true
}
```

Maps internally to `scheduler::RequestType::Structured` with `GenerationParams.structured_schema` set.

#### 2.3.5 POST /v1/generate/function_call

Explicit tool/function calling endpoint. Applications can also use `tools` parameter on `/v1/chat/completions`; this endpoint exists for single-shot function extraction.

**Request Body:**

```json
{
  "model": "qwen3-14b:Q4_K_M",
  "messages": [
    {"role": "user", "content": "What's the weather in Portland?"}
  ],
  "functions": [
    {
      "name": "get_weather",
      "description": "Get current weather for a city",
      "parameters": {
        "type": "object",
        "properties": {
          "city": {"type": "string"},
          "units": {"type": "string", "enum": ["celsius", "fahrenheit"]}
        },
        "required": ["city"]
      }
    }
  ]
}
```

**Response:**

```json
{
  "id": "mai-req-...",
  "object": "function_call",
  "model": "qwen3-14b:Q4_K_M",
  "function_call": {
    "name": "get_weather",
    "arguments": "{\"city\": \"Portland\", \"units\": \"fahrenheit\"}"
  },
  "usage": {"prompt_tokens": 45, "completion_tokens": 18, "total_tokens": 63}
}
```

### 2.4 Model Management Endpoints

#### 2.4.1 GET /v1/models

List available models with capabilities.

**Query Parameters:**

| Parameter | Type | Description |
|---|---|---|
| `backend` | string | Filter by compatible backend |
| `capability` | string | Filter by capability (chat, embedding, vision, structured_output) |
| `status` | string | Filter by status (cold_storage, loaded, active) |
| `max_vram` | integer | Max VRAM budget in bytes |

**Response:**

```json
{
  "object": "list",
  "data": [
    {
      "id": "qwen3-14b:v1.0:Q4_K_M",
      "object": "model",
      "created": 1747612800,
      "owned_by": "island-mountain",
      "name": "qwen3-14b",
      "version": "1.0",
      "format": "GGUF",
      "quantization": "Q4_K_M",
      "size_bytes": 8589934592,
      "required_vram_bytes": 10737418240,
      "status": "loaded",
      "capabilities": {
        "chat": true,
        "completion": true,
        "embedding": false,
        "vision": false,
        "structured_output": true,
        "max_context_tokens": 32768,
        "supported_languages": ["en", "zh", "ja", "ko", "de", "fr", "es"]
      },
      "compatible_backends": ["ollama", "llama.cpp", "exllamav2"],
      "security": {
        "signature_algorithm": "ML-DSA-87",
        "integrity_verified": true
      }
    }
  ]
}
```

Maps to `registry::ModelSummary` and `registry::CapabilityInfo`.

The `id`, `object`, `created`, `owned_by` fields maintain OpenAI compatibility for `/v1/models`.

#### 2.4.2 GET /v1/models/{model_id}

Detailed model information including current adapter assignment and VRAM usage.

**Response:** Single model object (same schema as list item) plus additional fields:

```json
{
  "...": "...same as list item...",
  "adapter_assignment": {
    "adapter_id": "ollama:0",
    "gpu_id": "nvidia:h100:00000000:01:00.0"
  },
  "vram_allocated_bytes": 10737418240,
  "request_count": 1542,
  "last_used": "2026-05-16T14:30:00Z"
}
```

#### 2.4.3 POST /v1/models/{model_id}/load

Request explicit model loading from cold storage to VRAM.

**Request Body:**

```json
{
  "target_adapter": "ollama:0",
  "priority": "normal"
}
```

**Response:**

```json
{
  "model_id": "qwen3-14b:v1.0:Q4_K_M",
  "status": "loading",
  "progress_percent": 0,
  "estimated_time_seconds": 45
}
```

Requires admin profile. Maps to `registry::ModelRegistry::load_model()`.

#### 2.4.4 POST /v1/models/{model_id}/unload

Unload model from VRAM back to cold storage.

**Request Body:**

```json
{
  "force": false
}
```

If `force` is false, waits for in-flight requests to complete (drain). If true, interrupts immediately. Requires admin profile. Maps to `registry::ModelRegistry::unload_model()`.

**Response:**

```json
{
  "model_id": "qwen3-14b:v1.0:Q4_K_M",
  "status": "evicting",
  "drained_requests": 3
}
```

### 2.5 Health Endpoints

#### 2.5.1 GET /v1/health

System health summary. No authentication required (for monitoring tools).

**Response:**

```json
{
  "status": "healthy",
  "air_gap_verified": true,
  "power_state": "full_inference",
  "uptime_seconds": 86400,
  "adapters": {
    "total": 3,
    "healthy": 2,
    "degraded": 1,
    "unhealthy": 0
  },
  "hardware": {
    "gpus": 2,
    "total_vram_bytes": 161061273600,
    "used_vram_bytes": 42949672960,
    "thermal_state": "normal"
  },
  "system": {
    "cpu_load_percent": 23.5,
    "ram_used_bytes": 34359738368,
    "ram_total_bytes": 137438953472,
    "disk_vault_free_bytes": 1099511627776
  }
}
```

Maps to `health::HealthSnapshot`.

#### 2.5.2 GET /v1/health/adapters

Per-adapter health details.

**Response:**

```json
{
  "adapters": {
    "ollama:0": {
      "status": "healthy",
      "last_heartbeat": "2026-05-16T14:30:00Z",
      "missed_heartbeats": 0,
      "avg_latency_ms": 124.5,
      "error_rate_5min": 0.002,
      "vram_usage_bytes": 10737418240,
      "active_requests": 2
    },
    "vllm:0": {
      "status": "degraded",
      "last_heartbeat": "2026-05-16T14:29:55Z",
      "missed_heartbeats": 1,
      "avg_latency_ms": 450.2,
      "error_rate_5min": 0.05,
      "vram_usage_bytes": 21474836480,
      "active_requests": 8
    }
  }
}
```

Maps to `health::AdapterHealth` and `health::AdapterStatus`.

#### 2.5.3 GET /v1/health/hardware

GPU and accelerator health details.

**Response:**

```json
{
  "gpus": {
    "nvidia:h100:00000000:01:00.0": {
      "temperature_celsius": 62.0,
      "fan_speed_percent": 45,
      "vram_used_bytes": 21474836480,
      "vram_total_bytes": 85899345920,
      "power_limit_watts": 350,
      "compute_utilization_percent": 78
    }
  },
  "power_draw_watts": 385.0,
  "thermal_state": "normal",
  "network_state": "air_gap_compliant"
}
```

Maps to `health::HardwareHealth` and `health::GpuHealth`.

### 2.6 Power Management Endpoints

#### 2.6.1 GET /v1/power/state

Current power state and transition info.

**Response:**

```json
{
  "state": "sentinel",
  "estimated_power_watts": 8.0,
  "auto_demotion": {
    "enabled": true,
    "idle_minutes_remaining": 45,
    "next_state": "deep_vault_sleep"
  },
  "promotion_available": true,
  "promotion_latency_target_ms": 8000
}
```

Maps to `power::PowerState` and `power::AutoDemotionConfig`.

Valid `state` values: `off`, `deep_vault_sleep`, `sentinel`, `full_inference`, `thermal_throttle`.

#### 2.6.2 POST /v1/power/transition

Request a power state transition. Requires admin profile.

**Request Body:**

```json
{
  "target_state": "full_inference",
  "reason": "manual_wake"
}
```

**Response:**

```json
{
  "transition_id": "550e8400-e29b-41d4-a716-446655440000",
  "from": "sentinel",
  "to": "full_inference",
  "status": "in_progress",
  "estimated_latency_ms": 8000
}
```

Non-admin profiles can trigger implicit promotion via inference requests (Sentinel detects complexity exceeding its capability and promotes automatically). Explicit `/power/transition` is admin-only.

### 2.7 Registry Management Endpoints

#### 2.7.1 GET /v1/registry/manifest

Full model registry dump. Requires admin profile.

**Response:**

```json
{
  "models": [
    {
      "model_id": "qwen3-14b:v1.0:Q4_K_M",
      "manifest": {
        "model": {"name": "qwen3-14b", "version": "1.0", "format": "GGUF", "quantization": "Q4_K_M", "size_bytes": 8589934592, "required_vram_bytes": 10737418240},
        "compatibility": {"min_mai_version": "0.1.0", "supported_backends": ["ollama", "llama.cpp"], "hardware_classes": ["nvidia_ampere", "nvidia_hopper", "cpu"]},
        "capabilities": {"chat": true, "completion": true, "embedding": false, "vision": false, "structured_output": true, "max_context_tokens": 32768, "supported_languages": ["en"]},
        "security": {"signature_algorithm": "ML-DSA-87", "public_key_fingerprint": "ab12cd34...", "integrity_hash_tree": "sha3-256:..."},
        "metadata": {"license": "Apache-2.0", "source": "https://huggingface.co/Qwen/Qwen3-14B", "changelog": "Initial release"}
      },
      "status": "loaded",
      "vault_path": "/vault/models/qwen3-14b/v1.0/Q4_K_M/"
    }
  ]
}
```

Maps directly to `registry::ModelManifest`.

#### 2.7.2 POST /v1/registry/install

Install a model from a local package (USB drive or local path). Requires admin profile.

**Request Body:**

```json
{
  "source_path": "/mnt/usb/mai-models/qwen3-14b-Q4_K_M.mai",
  "verify_signature": true
}
```

**Response:**

```json
{
  "model_id": "qwen3-14b:v1.0:Q4_K_M",
  "status": "installed",
  "integrity_verified": true,
  "signature_verified": true,
  "installed_at": "2026-05-16T15:00:00Z"
}
```

Maps to `registry::ModelRegistry::install_from_usb()`.

#### 2.7.3 POST /v1/registry/uninstall

Remove a model from the registry and vault. Requires admin profile.

**Request Body:**

```json
{
  "model_id": "qwen3-14b:v1.0:Q4_K_M",
  "wipe_vault": true
}
```

If `wipe_vault` is true, securely erases model weights from ZFS vault. If false, removes registry entry but leaves encrypted weights.

**Response:**

```json
{
  "model_id": "qwen3-14b:v1.0:Q4_K_M",
  "status": "uninstalled",
  "vault_wiped": true
}
```

### 2.8 Audit Endpoints

#### 2.8.1 GET /v1/audit/log

Read the audit trail. Requires admin profile. Paginated.

**Query Parameters:**

| Parameter | Type | Default | Description |
|---|---|---|---|
| `limit` | integer | 100 | Max entries per page (max 1000) |
| `offset` | integer | 0 | Pagination offset |
| `profile_id` | UUID | -- | Filter by profile |
| `model` | string | -- | Filter by model |
| `start_time` | ISO8601 | -- | Entries after this time |
| `end_time` | ISO8601 | -- | Entries before this time |
| `status_code` | integer | -- | Filter by HTTP status code |

**Response:**

```json
{
  "total_entries": 15234,
  "offset": 0,
  "limit": 100,
  "entries": [
    {
      "timestamp": "2026-05-16T14:30:00.123Z",
      "request_id": "550e8400-e29b-41d4-a716-446655440000",
      "profile_id": "660e8400-e29b-41d4-a716-446655440001",
      "endpoint": "/v1/chat/completions",
      "method": "POST",
      "model": "qwen3-14b:Q4_K_M",
      "adapter": "ollama:0",
      "tokens_in": 24,
      "tokens_out": 128,
      "latency_ms": 1450,
      "status_code": 200,
      "priority": "normal",
      "hash": "sha3-256:abc123...",
      "prev_hash": "sha3-256:def456..."
    }
  ]
}
```

Each entry includes `hash` (SHA3-256 of entry content) and `prev_hash` (hash of previous entry) forming the tamper-evident chain. The chain is PQC-signed (ML-DSA) at configurable intervals (default: every 100 entries).

---

## 3. Streaming Protocol Specification

### 3.1 Server-Sent Events (SSE)

Primary streaming protocol for HTTP clients. Used by `/v1/chat/completions` and `/v1/completions` when `stream=true`.

**Connection:**

```
POST /v1/chat/completions HTTP/1.1
Content-Type: application/json
Accept: text/event-stream

{"model": "qwen3-14b:Q4_K_M", "messages": [...], "stream": true}
```

**Event Format:**

```
event: message
data: {"id":"mai-req-...","object":"chat.completion.chunk","choices":[{"index":0,"delta":{"content":"token"},"finish_reason":null}]}

event: message
data: {"id":"mai-req-...","object":"chat.completion.chunk","choices":[{"index":0,"delta":{},"finish_reason":"stop"}],"usage":{...}}

event: done
data: [DONE]
```

**Event Types:**

| Event | Description |
|---|---|
| `message` | Token delta or completion signal |
| `error` | Error during generation (includes error object) |
| `done` | Stream complete, `data: [DONE]` |
| `heartbeat` | Keepalive ping (every 15s during generation, `data: {"type":"heartbeat"}`) |

**Backpressure:** The MAI server buffers up to 64 token events per stream. If the client is too slow to consume, the server pauses the adapter's token stream (channel backpressure via tokio mpsc bounded channel). This is transparent to the client.

**Timeout:** If no token is produced for 30 seconds (configurable), the server sends an error event and closes the stream.

**Resume Protocol:** Each streaming chunk includes a monotonically increasing `sequence` field (integer, starts at 0). If the SSE connection drops, the client can reconnect with the `Last-Event-ID` header set to the last received sequence number. The server replays from the next sequence if the request is still in flight and the replay buffer has not been evicted (buffer holds last 256 events, evicted 60 seconds after stream completion).

```
event: message
id: 42
data: {"id":"mai-req-...","sequence":42,"object":"chat.completion.chunk","choices":[...]}
```

### 3.2 WebSocket Protocol

Bidirectional streaming for speech-to-text, interactive agents, and real-time tool calling.

**Connection:**

```
GET /v1/ws HTTP/1.1
Upgrade: websocket
Connection: Upgrade
X-IM-Profile: <profile-uuid>
```

**Message Format (JSON over WebSocket frames):**

Client-to-Server:

```json
{
  "type": "inference.request",
  "id": "client-req-001",
  "payload": {
    "model": "qwen3-14b:Q4_K_M",
    "messages": [{"role": "user", "content": "Hello"}],
    "stream": true
  }
}
```

```json
{
  "type": "inference.cancel",
  "id": "client-req-001"
}
```

```json
{
  "type": "audio.chunk",
  "id": "client-req-002",
  "payload": {
    "format": "pcm_s16le",
    "sample_rate": 16000,
    "data": "<base64-encoded-audio>"
  }
}
```

Server-to-Client:

```json
{
  "type": "inference.token",
  "id": "client-req-001",
  "sequence": 0,
  "payload": {
    "delta": {"content": "Hello"},
    "finish_reason": null
  }
}
```

```json
{
  "type": "inference.complete",
  "id": "client-req-001",
  "payload": {
    "usage": {"prompt_tokens": 5, "completion_tokens": 12, "total_tokens": 17},
    "finish_reason": "stop"
  }
}
```

```json
{
  "type": "inference.error",
  "id": "client-req-001",
  "payload": {
    "code": "MAI-5003",
    "message": "Model overloaded",
    "retry_after_seconds": 5
  }
}
```

```json
{
  "type": "transcription.partial",
  "id": "client-req-002",
  "payload": {
    "text": "What is the weather",
    "is_final": false
  }
}
```

**Message Types:**

| Direction | Type | Description |
|---|---|---|
| C->S | `inference.request` | Start inference (streaming or non-streaming) |
| C->S | `inference.cancel` | Cancel in-flight request |
| C->S | `audio.chunk` | Send audio data for speech-to-text |
| C->S | `tool.result` | Return tool execution result to model |
| C->S | `ping` | Client keepalive |
| S->C | `inference.token` | Streaming token delta |
| S->C | `inference.complete` | Request complete with usage |
| S->C | `inference.error` | Error during processing |
| S->C | `tool.request` | Model requests tool execution |
| S->C | `transcription.partial` | Partial speech transcription |
| S->C | `transcription.final` | Final speech transcription |
| S->C | `pong` | Server keepalive response |

**Multiplexing:** Multiple concurrent requests over a single WebSocket connection, distinguished by `id`. The server processes them independently.

**Connection Lifecycle:**

1. Client connects with profile header
2. Server validates profile, sends `{"type":"connected","profile_id":"..."}`
3. Client sends requests, server streams responses
4. Keepalive: client sends `ping` every 30s, server responds with `pong`
5. Idle timeout: 5 minutes with no messages = server closes
6. Reconnection: client reconnects and re-sends any incomplete requests

---

## 4. gRPC API Specification

For high-performance internal use by L4 components (agent orchestrator, RAG pipeline). Lower latency than REST for tool calling chains and batch operations.

### 4.1 Service Definitions

See `proto/mai/v1/inference.proto` for complete Proto3 definitions.

**Services:**

| Service | Description |
|---|---|
| `MaiInference` | Chat completion, text completion, embedding |
| `MaiModels` | Model listing, loading, unloading |
| `MaiHealth` | Health checks (standard gRPC health protocol) |
| `MaiPower` | Power state queries and transitions |
| `MaiRegistry` | Model install/uninstall |
| `MaiAudit` | Audit trail queries |

**Key RPCs:**

- `ChatCompletion` (unary): Non-streaming chat
- `ChatCompletionStream` (server-streaming): Streaming chat, maps to SSE equivalent
- `Embed` (unary): Batch embedding
- `AgentStream` (bidirectional streaming): For agent orchestrator tool calling loops
- `ListModels` (unary): Model listing with filters
- `GetHealth` (unary): Standard gRPC health check

### 4.2 Metadata (gRPC equivalent of HTTP headers)

| Key | Value | Description |
|---|---|---|
| `x-im-profile` | UUID string | Family profile ID |
| `x-im-priority` | string | Request priority |
| `x-im-request-id` | UUID string | Request correlation ID |

---

## 5. Authentication and Authorization

### 5.1 Local-Only Authentication

No external identity providers. No OAuth. No cloud tokens. Authentication is entirely local.

**Profile Token:** A 256-bit token stored in the TPM 2.0 chip, sealed to the device's PCR measurements. Each family profile has a unique token. Tokens are generated during profile creation and cannot be extracted from the TPM.

**Token Format:** `im-profile-v1.<base64url-encoded-token>` (opaque to clients, validated server-side against TPM-sealed store).

**Token Delivery:** Passed in the `X-IM-Profile` header on every request. Applications store the token in their local configuration (never transmitted off-device).

**Default Profile:** If no `X-IM-Profile` header is provided, the request runs under the default adult profile. The default profile is created during first-boot setup.

### 5.2 Family Profile System

Profiles control model access, priority levels, content safety settings, and usage limits.

**Profile Schema:**

```json
{
  "profile_id": "660e8400-e29b-41d4-a716-446655440001",
  "name": "Dad",
  "role": "admin",
  "created_at": "2026-05-16T10:00:00Z",
  "permissions": {
    "model_access": ["*"],
    "max_context_tokens": null,
    "allowed_endpoints": ["*"],
    "can_manage_models": true,
    "can_manage_power": true,
    "can_view_audit": true,
    "can_manage_profiles": true
  },
  "priority": "high",
  "rate_limits": {
    "requests_per_minute": null,
    "tokens_per_hour": null
  },
  "content_safety": {
    "enabled": false,
    "filter_level": "none"
  }
}
```

**Profile Roles:**

| Role | Description | Default Priority | Model Access |
|---|---|---|---|
| `admin` | Full system access | `high` | All models |
| `adult` | Standard inference access | `normal` | All models |
| `teen` | Filtered access, moderate limits | `normal` | Filtered list |
| `child` | Restricted access, strict limits | `low` | Restricted list |
| `guest` | Minimal access, heavy limits | `low` | Minimal list |

**Profile Endpoints (admin only):**

| Endpoint | Method | Description |
|---|---|---|
| `/v1/profiles` | GET | List all profiles |
| `/v1/profiles` | POST | Create new profile |
| `/v1/profiles/{id}` | GET | Get profile details |
| `/v1/profiles/{id}` | PUT | Update profile |
| `/v1/profiles/{id}` | DELETE | Delete profile |

**Content Safety Filters:**

Child and teen profiles have content safety filters applied at the API boundary (before the request reaches the scheduler). Filter levels:

| Level | Description |
|---|---|
| `none` | No filtering (admin/adult default) |
| `moderate` | Block explicit content, allow educational/medical |
| `strict` | Block explicit and violent content, restrict topics |

Filters are applied as system prompt injections and output post-processing. Implementation details in Session 11.

### 5.3 Air-Gap Verification at API Startup

The API server performs air-gap verification before accepting any requests:

1. **Read air-gap switch state** via HIL (`health::NetworkState`)
2. **If `AirGapCompliant`:** Verify all network interfaces are down. Start normally.
3. **If `Connected`:** Air-gap switch not engaged. Start normally with `air_gap_mode: false` in health response.
4. **If `NonCompliant`:** Air-gap switch engaged but interfaces are up. **Refuse to start.** Log critical alert. Return `CoreError::AirGapViolation`.

During operation, the health monitor continuously verifies air-gap compliance (configurable interval, default 60s). If compliance is lost while the switch is engaged, the API returns 503 on all inference endpoints until compliance is restored.

---

## 6. Error Contract

### 6.1 Error Response Format

All error responses use a consistent JSON structure:

```json
{
  "error": {
    "code": "MAI-4004",
    "message": "Model 'nonexistent-model' not found in registry",
    "type": "model_unavailable",
    "retry_after_seconds": null,
    "request_id": "550e8400-e29b-41d4-a716-446655440000"
  }
}
```

| Field | Type | Description |
|---|---|---|
| `code` | string | MAI error code (see taxonomy below) |
| `message` | string | Human-readable description (never contains backend internals) |
| `type` | string | Error category for programmatic handling |
| `retry_after_seconds` | integer/null | Seconds to wait before retry (for retryable errors) |
| `request_id` | UUID | Request correlation ID |

### 6.2 Error Code Taxonomy

Error codes use the format `MAI-XYYY` where X is the HTTP status code class and YYY is the specific error.

**4xx Client Errors:**

| Code | HTTP | Type | CoreError Mapping | Description |
|---|---|---|---|---|
| MAI-4001 | 400 | `invalid_request` | -- | Malformed JSON or missing required field |
| MAI-4002 | 400 | `invalid_request` | -- | Invalid parameter value (temperature out of range, etc.) |
| MAI-4003 | 401 | `authentication_failed` | -- | Invalid or expired profile token |
| MAI-4004 | 404 | `model_unavailable` | `ModelUnavailable` | Model not found in registry |
| MAI-4005 | 403 | `permission_denied` | -- | Profile lacks permission for this operation |
| MAI-4006 | 422 | `validation_error` | -- | Structured output schema invalid |
| MAI-4007 | 429 | `rate_limited` | -- | Profile rate limit exceeded |
| MAI-4008 | 400 | `context_exceeded` | -- | Prompt exceeds model's max context window |

**5xx Server Errors:**

| Code | HTTP | Type | CoreError Mapping | Description |
|---|---|---|---|---|
| MAI-5001 | 500 | `internal_error` | `Internal` | Unexpected server error (details logged, not exposed) |
| MAI-5002 | 500 | `request_failed` | `RequestFailed` | Request processing failed |
| MAI-5003 | 503 | `overloaded` | `Overloaded` | All adapters at capacity, retry later |
| MAI-5004 | 503 | `air_gap_violation` | `AirGapViolation` | Air-gap compliance lost |
| MAI-5005 | 503 | `power_state_unavailable` | -- | System in sleep state, promotion in progress |
| MAI-5006 | 504 | `timeout` | `RequestFailed` | Request exceeded timeout |

**Retry Semantics:**

| Type | Retryable | Retry Strategy |
|---|---|---|
| `invalid_request` | No | Fix request and resend |
| `authentication_failed` | No | Re-authenticate |
| `model_unavailable` | No | Check model registry |
| `permission_denied` | No | Use authorized profile |
| `validation_error` | No | Fix schema |
| `rate_limited` | Yes | Wait `retry_after_seconds` |
| `context_exceeded` | No | Reduce prompt length |
| `internal_error` | Maybe | Retry with backoff, report if persistent |
| `request_failed` | Yes | Retry with backoff |
| `overloaded` | Yes | Wait `retry_after_seconds`, then retry |
| `air_gap_violation` | No | Physical intervention required |
| `power_state_unavailable` | Yes | Wait for promotion, auto-retry |
| `timeout` | Yes | Retry with longer timeout or simpler request |

### 6.3 Backend Opacity Rule

Error responses NEVER contain:

- Backend engine names (no "Ollama returned...", "vLLM error:...")
- Backend-specific error codes or stack traces
- Internal file paths or configuration details
- GPU hardware identifiers (these are in `/v1/health/hardware`, not error messages)

The API server (Session 11) maps `AdapterError` variants to `CoreError` variants to MAI error codes. Each mapping strips backend-specific detail. The original backend error is logged server-side in the audit trail for debugging.

---

## 7. Product Tier API Differences

The API surface is identical across all product tiers. The differences are in the default configurations loaded at startup:

| Setting | Scout | Ranger | Pack Leader |
|---|---|---|---|
| Max concurrent requests | 4 | 16 | 64 |
| Default max_tokens | 1024 | 4096 | 8192 |
| Rate limit (default profile) | 10 req/min | 60 req/min | 240 req/min |
| WebSocket connections | 2 | 8 | 32 |
| Audit retention | 7 days | 30 days | 365 days |
| gRPC enabled | No | Yes | Yes |
| Max loaded models | 1 | 4 | 16 |

These are configurable overrides in `scout.toml`, `ranger.toml`, `pack-leader.toml`. The API server reads them at startup. All endpoints exist on all tiers; capacity limits are enforced at runtime.

---

## 8. Versioning and Backward Compatibility

### 8.1 URL Versioning

All endpoints are under `/v1/`. The version prefix is part of the URL path, not a header.

### 8.2 Compatibility Rules

1. New fields may be added to response objects (clients must ignore unknown fields)
2. New optional fields may be added to request objects
3. Existing fields are never removed or renamed
4. Existing endpoints are never removed
5. Enum values are never removed (new values may be added)
6. Error codes are never reused for different meanings

### 8.3 Breaking Changes

If a breaking change is ever required (unlikely given the Tock stability contract), it ships under `/v2/` with `/v1/` maintained for a minimum of 12 months.

---

## 9. Appendix: Endpoint Summary

| Method | Path | Auth | Description |
|---|---|---|---|
| POST | /v1/chat/completions | profile | Chat completion (OpenAI-compatible) |
| POST | /v1/completions | profile | Text completion |
| POST | /v1/embeddings | profile | Text embedding (OpenAI-compatible) |
| POST | /v1/generate/structured | profile | Schema-constrained generation |
| POST | /v1/generate/function_call | profile | Function/tool calling |
| GET | /v1/models | profile | List models |
| GET | /v1/models/{id} | profile | Model detail |
| POST | /v1/models/{id}/load | admin | Load model to VRAM |
| POST | /v1/models/{id}/unload | admin | Unload model from VRAM |
| GET | /v1/health | none | System health summary |
| GET | /v1/health/adapters | profile | Per-adapter health |
| GET | /v1/health/hardware | profile | Hardware health |
| GET | /v1/power/state | profile | Current power state |
| POST | /v1/power/transition | admin | Request state transition |
| GET | /v1/registry/manifest | admin | Full registry dump |
| POST | /v1/registry/install | admin | Install model from USB/local |
| POST | /v1/registry/uninstall | admin | Remove model |
| GET | /v1/audit/log | admin | Read audit trail |
| GET | /v1/profiles | admin | List profiles |
| POST | /v1/profiles | admin | Create profile |
| GET | /v1/profiles/{id} | admin | Get profile |
| PUT | /v1/profiles/{id} | admin | Update profile |
| DELETE | /v1/profiles/{id} | admin | Delete profile |
| GET | /v1/ws | profile | WebSocket upgrade |

---

*Document: MAI API Surface Specification | Session 05 | 2026-05-16 | Island Mountain AI | Confidential*
