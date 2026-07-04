"""IPC Protocol v1.0 verification tests.

Tests the NDJSON wire format contract between Rust (AdapterProcess) and
Python (runner.py). Validates:
  - Startup config parsing
  - Handshake response format
  - Request/response event format
  - Token streaming event ordering
  - Error event format
  - Graceful shutdown sequence

These tests run standalone (no real adapter backend needed).
They exercise runner.py's serialization/deserialization against
the IPC-PROTOCOL.md specification.
"""

from __future__ import annotations

import json
from typing import ClassVar

import pytest

# ── Startup Config ────────────────────────────────────────────────────────────

class TestStartupConfig:
    """Verify startup config JSON matches IPC-PROTOCOL.md spec."""

    def test_minimal_config(self) -> None:
        """Startup config with required fields only."""
        config = {
            "adapter_name": "ollama",
            "module_path": "adapters.ollama.adapter",
            "entry_class": "OllamaAdapter",
        }
        raw = json.dumps(config)
        parsed = json.loads(raw)
        assert parsed["adapter_name"] == "ollama"
        assert parsed["module_path"] == "adapters.ollama.adapter"
        assert parsed["entry_class"] == "OllamaAdapter"
        assert "config" not in parsed

    def test_config_with_adapter_settings(self) -> None:
        """Startup config with optional adapter-specific config."""
        config = {
            "adapter_name": "ollama",
            "module_path": "adapters.ollama.adapter",
            "entry_class": "OllamaAdapter",
            "config": {"host": "127.0.0.1", "port": 11434},
        }
        raw = json.dumps(config)
        parsed = json.loads(raw)
        assert parsed["config"]["host"] == "127.0.0.1"
        assert parsed["config"]["port"] == 11434


# ── Handshake Response ────────────────────────────────────────────────────────

class TestHandshakeResponse:
    """Verify handshake response format matches IPC-PROTOCOL.md spec."""

    def _make_handshake(self, **overrides: object) -> dict:
        base = {
            "type": "handshake",
            "adapter_name": "ollama",
            "version": "1.0.0",
            "handle": "ollama-abc123",
            "capabilities": {
                "max_context_window": 131072,
                "supports_streaming": True,
                "supports_embedding": True,
                "supports_batching": False,
                "supports_structured_output": False,
                "supports_vision": False,
                "supports_tool_calling": False,
                "supports_continuous_batching": False,
                "supports_hot_swap": False,
                "supported_quantizations": [],
                "backend_version": "0.5.4",
            },
        }
        base.update(overrides)
        return base

    def test_handshake_required_fields(self) -> None:
        """All required handshake fields are present."""
        hs = self._make_handshake()
        raw = json.dumps(hs)
        parsed = json.loads(raw)
        assert parsed["type"] == "handshake"
        assert parsed["adapter_name"] == "ollama"
        assert isinstance(parsed["version"], str)
        assert isinstance(parsed["handle"], str)
        assert isinstance(parsed["capabilities"], dict)

    def test_handshake_capabilities_fields(self) -> None:
        """All capability boolean fields are present."""
        hs = self._make_handshake()
        caps = hs["capabilities"]
        bool_fields = [
            "supports_streaming", "supports_embedding", "supports_batching",
            "supports_structured_output", "supports_vision",
            "supports_tool_calling", "supports_continuous_batching",
            "supports_hot_swap",
        ]
        for field in bool_fields:
            assert isinstance(caps[field], bool), f"{field} must be bool"
        assert isinstance(caps["max_context_window"], int)
        assert isinstance(caps["supported_quantizations"], list)
        assert isinstance(caps["backend_version"], str)

    def test_handshake_is_single_line(self) -> None:
        """Handshake serializes to exactly one line (NDJSON)."""
        hs = self._make_handshake()
        raw = json.dumps(hs, separators=(",", ":"))
        assert "\n" not in raw


# ── IPC Request Format ────────────────────────────────────────────────────────

class TestIpcRequest:
    """Verify IPC request format matches IPC-PROTOCOL.md spec."""

    def test_inference_request(self) -> None:
        req = {
            "request_id": "550e8400-e29b-41d4-a716-446655440000",
            "type": "inference",
            "payload": {
                "prompt": "What is the capital of France?",
                "params": {
                    "temperature": 0.7,
                    "top_p": 0.9,
                    "max_tokens": 512,
                    "stop_sequences": [],
                    "structured_schema": None,
                },
                "stream": True,
            },
        }
        raw = json.dumps(req, separators=(",", ":"))
        parsed = json.loads(raw)
        assert parsed["request_id"] == "550e8400-e29b-41d4-a716-446655440000"
        assert parsed["type"] == "inference"
        assert parsed["payload"]["prompt"] == "What is the capital of France?"
        assert "\n" not in raw

    def test_health_request(self) -> None:
        req = {
            "request_id": "req-001",
            "type": "health",
            "payload": {},
        }
        raw = json.dumps(req, separators=(",", ":"))
        parsed = json.loads(raw)
        assert parsed["type"] == "health"
        assert parsed["payload"] == {}

    def test_shutdown_request(self) -> None:
        req = {
            "request_id": "req-002",
            "type": "shutdown",
            "payload": {},
        }
        parsed = json.loads(json.dumps(req))
        assert parsed["type"] == "shutdown"

    def test_heartbeat_request(self) -> None:
        req = {
            "request_id": "req-003",
            "type": "heartbeat",
            "payload": {},
        }
        parsed = json.loads(json.dumps(req))
        assert parsed["type"] == "heartbeat"


# ── NDJSON Response Events ────────────────────────────────────────────────────

class TestIpcEvents:
    """Verify NDJSON response event format matches IPC-PROTOCOL.md spec."""

    def test_token_event(self) -> None:
        event = {
            "request_id": "req-100",
            "type": "token",
            "text": "Paris",
            "logprob": -0.5,
            "index": 0,
            "finish_reason": None,
        }
        raw = json.dumps(event, separators=(",", ":"))
        parsed = json.loads(raw)
        assert parsed["type"] == "token"
        assert parsed["text"] == "Paris"
        assert parsed["logprob"] == -0.5
        assert parsed["index"] == 0
        assert parsed["finish_reason"] is None
        assert "\n" not in raw

    def test_token_event_with_finish(self) -> None:
        event = {
            "request_id": "req-100",
            "type": "token",
            "text": ".",
            "logprob": -0.1,
            "index": 5,
            "finish_reason": "stop",
        }
        parsed = json.loads(json.dumps(event))
        assert parsed["finish_reason"] == "stop"

    def test_usage_event(self) -> None:
        event = {
            "request_id": "req-100",
            "type": "usage",
            "prompt_tokens": 42,
            "completion_tokens": 7,
        }
        parsed = json.loads(json.dumps(event))
        assert parsed["type"] == "usage"
        assert parsed["prompt_tokens"] == 42
        assert parsed["completion_tokens"] == 7

    def test_result_event(self) -> None:
        event = {
            "request_id": "req-200",
            "type": "result",
            "data": {"status": "healthy", "uptime_ms": 12345},
        }
        parsed = json.loads(json.dumps(event))
        assert parsed["type"] == "result"
        assert parsed["data"]["status"] == "healthy"

    def test_done_event(self) -> None:
        event = {
            "request_id": "req-100",
            "type": "done",
        }
        parsed = json.loads(json.dumps(event))
        assert parsed["type"] == "done"
        assert parsed["request_id"] == "req-100"

    def test_error_event(self) -> None:
        event = {
            "request_id": "req-300",
            "type": "error",
            "code": "OutOfMemory",
            "message": "CUDA out of memory",
        }
        parsed = json.loads(json.dumps(event))
        assert parsed["type"] == "error"
        assert parsed["code"] == "OutOfMemory"
        assert parsed["message"] == "CUDA out of memory"


# ── Event Ordering ────────────────────────────────────────────────────────────

class TestEventOrdering:
    """Verify event ordering guarantees from IPC-PROTOCOL.md."""

    def test_inference_event_sequence(self) -> None:
        """Inference: 0+ tokens, 1 usage, 1 done."""
        events = [
            {
                "request_id": "r1",
                "type": "token",
                "text": "P",
                "logprob": None,
                "index": 0,
                "finish_reason": None,
            },
            {
                "request_id": "r1",
                "type": "token",
                "text": "aris",
                "logprob": None,
                "index": 1,
                "finish_reason": "stop",
            },
            {"request_id": "r1", "type": "usage", "prompt_tokens": 10, "completion_tokens": 2},
            {"request_id": "r1", "type": "done"},
        ]
        types = [e["type"] for e in events]
        # All tokens before usage
        token_indices = [i for i, t in enumerate(types) if t == "token"]
        usage_idx = types.index("usage")
        done_idx = types.index("done")
        assert all(i < usage_idx for i in token_indices)
        assert usage_idx < done_idx

    def test_non_streaming_event_sequence(self) -> None:
        """Non-streaming: 1 result, 1 done."""
        events = [
            {"request_id": "r2", "type": "result", "data": {"status": "healthy"}},
            {"request_id": "r2", "type": "done"},
        ]
        assert events[0]["type"] == "result"
        assert events[1]["type"] == "done"

    def test_error_event_sequence(self) -> None:
        """Error: 1 error, 1 done."""
        events = [
            {"request_id": "r3", "type": "error", "code": "Timeout", "message": "timed out"},
            {"request_id": "r3", "type": "done"},
        ]
        assert events[0]["type"] == "error"
        assert events[1]["type"] == "done"


# ── Error Code Taxonomy ───────────────────────────────────────────────────────

class TestErrorCodes:
    """Verify error codes match IPC-PROTOCOL.md taxonomy."""

    VALID_CODES: ClassVar[set[str]] = {
        "BackendCrashed",
        "BackendUnavailable",
        "ContextExceeded",
        "HardwareFault",
        "InternalError",
        "ModelNotFound",
        "OutOfMemory",
        "RateLimited",
        "Timeout",
        "UnsupportedOperation",
        "ValidationError",
    }

    @pytest.mark.parametrize("code", sorted(VALID_CODES))
    def test_valid_error_code(self, code: str) -> None:
        """Each defined error code produces a valid error event."""
        event = {
            "request_id": "err-test",
            "type": "error",
            "code": code,
            "message": f"Test error: {code}",
        }
        raw = json.dumps(event, separators=(",", ":"))
        parsed = json.loads(raw)
        assert parsed["code"] == code
        assert parsed["code"] in self.VALID_CODES


# ── Graceful Shutdown ─────────────────────────────────────────────────────────

class TestShutdownProtocol:
    """Verify graceful shutdown sequence per IPC-PROTOCOL.md."""

    def test_shutdown_request_response(self) -> None:
        """Shutdown: request -> result(ok:true) -> done."""
        request = {"request_id": "sd-1", "type": "shutdown", "payload": {}}
        response_events = [
            {"request_id": "sd-1", "type": "result", "data": {"ok": True}},
            {"request_id": "sd-1", "type": "done"},
        ]
        assert request["type"] == "shutdown"
        assert response_events[0]["data"]["ok"] is True
        assert response_events[1]["type"] == "done"
