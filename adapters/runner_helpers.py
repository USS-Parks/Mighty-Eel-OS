"""NDJSON runner payload helpers."""

from __future__ import annotations

import time
from typing import Any

from adapters.base import AdapterCapabilities, GenerationParams, HealthStatus


def capability_payload(caps: AdapterCapabilities) -> dict[str, Any]:
    return {
        "max_context_window": caps.max_context_window,
        "supported_quantizations": caps.supported_quantizations,
        "supports_streaming": caps.supports_streaming,
        "supports_batching": caps.supports_batching,
        "supports_structured_output": caps.supports_structured_output,
        "supports_vision": caps.supports_vision,
        "supports_tool_calling": caps.supports_tool_calling,
        "supports_continuous_batching": caps.supports_continuous_batching,
        "supports_embedding": caps.supports_embedding,
        "supports_hot_swap": caps.supports_hot_swap,
        "backend_version": caps.backend_version,
    }


def handshake_payload(
    *,
    adapter_name: str,
    version: str,
    handle: str,
    caps: AdapterCapabilities,
) -> dict[str, Any]:
    return {
        "request_id": "",
        "type": "handshake",
        "adapter_name": adapter_name,
        "version": version,
        "handle": handle,
        "capabilities": capability_payload(caps),
    }


def health_payload(
    status: HealthStatus, start_time_ms: int, requests_served: int,
) -> dict[str, Any]:
    return {
        "status": status.kind.value,
        "uptime_ms": status.uptime_ms or (now_ms() - start_time_ms),
        "requests_served": requests_served,
    }


def parse_generation_params(raw: dict[str, Any]) -> GenerationParams:
    return GenerationParams(
        temperature=raw.get("temperature", 0.7),
        top_p=raw.get("top_p", 0.9),
        max_tokens=raw.get("max_tokens", 512),
        stop_sequences=raw.get("stop_sequences", []),
        structured_schema=raw.get("structured_schema"),
    )


def now_ms() -> int:
    return int(time.time() * 1000)
