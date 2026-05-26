"""NDJSON IPC subprocess protocol handler for MAI adapters."""

from __future__ import annotations

import asyncio
import contextlib
import importlib
import json
import logging
import sys
import time
import traceback
from typing import Any

from adapters.base import (
    AdapterBase,
    AdapterError,
    GenerationParams,
    get_adapter,
)

logger = logging.getLogger("mai.adapters.runner")

logging.basicConfig(
    stream=sys.stderr,
    level=logging.INFO,
    format="%(asctime)s [%(levelname)s] %(name)s: %(message)s",
)


class AdapterRunner:
    """NDJSON IPC protocol handler for a single adapter subprocess."""

    def __init__(self, adapter: AdapterBase, adapter_name: str, version: str) -> None:
        self._adapter = adapter
        self._adapter_name = adapter_name
        self._version = version
        self._handle = ""
        self._running = False
        self._start_time_ms = 0
        self._requests_served = 0
        self._reader: asyncio.StreamReader | None = None
        self._writer: asyncio.StreamWriter | None = None

    async def run(self, startup_config: dict[str, Any]) -> None:
        """Main lifecycle: handshake then request loop."""
        self._running = True
        self._start_time_ms = _now_ms()

        loop = asyncio.get_event_loop()
        self._reader = asyncio.StreamReader()
        protocol = asyncio.StreamReaderProtocol(self._reader)
        await loop.connect_read_pipe(lambda: protocol, sys.stdin)

        transport, _ = await loop.connect_write_pipe(
            asyncio.streams.FlowControlMixin, sys.stdout,
        )
        self._writer = asyncio.StreamWriter(
            transport, protocol, self._reader, loop,
        )

        # ── Phase 1: Initialize adapter and send handshake ──
        config = startup_config.get("config") or {}
        try:
            self._handle = await self._adapter.initialize(config, hil_handle=None)
        except Exception as e:
            logger.error(f"Adapter initialization failed: {e}")
            sys.exit(1)

        caps = self._adapter.capabilities()
        handshake = {
            "request_id": "",  # Not a request, but IpcEvent requires this field
            "type": "handshake",
            "adapter_name": self._adapter_name,
            "version": self._version,
            "handle": self._handle or f"{self._adapter_name}-{_now_ms()}",
            "capabilities": {
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
            },
        }
        await self._send_line(handshake)
        logger.info(f"Handshake sent for {self._adapter_name}")

        # ── Phase 2: Request loop ──
        logger.info("Entering request loop")
        while self._running:
            try:
                line = await self._reader.readline()
                if not line:
                    logger.info("stdin EOF, shutting down")
                    break

                line_str = line.decode("utf-8").strip()
                if not line_str:
                    continue

                request = json.loads(line_str)
                await self._dispatch(request)

            except json.JSONDecodeError as e:
                logger.error(f"Invalid JSON on stdin: {e}")
                continue
            except asyncio.CancelledError:
                break
            except Exception:
                logger.error(f"Runner loop error: {traceback.format_exc()}")
                break

        try:
            await self._adapter.shutdown()
        except Exception:
            logger.error(f"Shutdown error: {traceback.format_exc()}")

    async def _dispatch(self, request: dict[str, Any]) -> None:
        """Route an IPC request and emit NDJSON response events."""
        request_id = request.get("request_id", "")
        req_type = request.get("type", "")
        payload = request.get("payload", {})

        try:
            await self._handle_method(req_type, payload, request_id)
            self._requests_served += 1
        except AdapterError as e:
            await self._send_event(request_id, "error", {
                "code": e.code,
                "message": e.detail or str(e),
            })
            await self._send_event(request_id, "done", {})
        except Exception as e:
            logger.error(f"Unhandled error in {req_type}: {traceback.format_exc()}")
            await self._send_event(request_id, "error", {
                "code": "InternalError",
                "message": str(e),
            })
            await self._send_event(request_id, "done", {})

    async def _handle_method(
        self, method: str, payload: dict[str, Any], request_id: str,
    ) -> None:
        """Dispatch to the correct adapter method. Emits events directly."""
        if method == "inference":
            prompt = payload.get("prompt", "")
            params = payload.get("params", {})
            gen_params = _parse_generation_params(params)

            # Stream tokens
            token_index = 0
            finish_reason: str | None = None
            async for token in self._adapter.generate(prompt, gen_params):
                finish_reason = "stop" if token.is_end_of_text else None
                await self._send_event(request_id, "token", {
                    "text": token.text,
                    "logprob": token.logprob,
                    "index": token_index,
                    "finish_reason": finish_reason,
                })
                token_index += 1

            # If no tokens set finish_reason, use "stop"
            if finish_reason is None and token_index > 0:
                # Resend the last token with finish_reason set
                await self._send_event(request_id, "token", {
                    "text": "",
                    "logprob": None,
                    "index": token_index,
                    "finish_reason": "stop",
                })

            # Usage event (approximate; adapters should report real counts)
            await self._send_event(request_id, "usage", {
                "prompt_tokens": 0,  # Adapter should override
                "completion_tokens": token_index,
            })
            await self._send_event(request_id, "done", {})

        elif method == "health":
            status = await self._adapter.health_check()
            await self._send_event(request_id, "result", {
                "data": {
                    "status": status.kind.value,
                    "uptime_ms": status.uptime_ms or (_now_ms() - self._start_time_ms),
                    "requests_served": self._requests_served,
                },
            })
            await self._send_event(request_id, "done", {})

        elif method == "capabilities":
            caps = self._adapter.capabilities()
            await self._send_event(request_id, "result", {
                "data": {
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
                },
            })
            await self._send_event(request_id, "done", {})

        elif method == "shutdown":
            await self._adapter.shutdown()
            self._running = False
            await self._send_event(request_id, "result", {"data": {"ok": True}})
            await self._send_event(request_id, "done", {})

        elif method == "heartbeat":
            await self._send_event(request_id, "result", {
                "data": {"timestamp_ms": _now_ms()},
            })
            await self._send_event(request_id, "done", {})

        else:
            raise AdapterError(
                code="UnsupportedOperation",
                detail=f"Unknown method: {method}",
            )

    async def _send_line(self, message: dict[str, Any]) -> None:
        """Write a JSON line to stdout."""
        if self._writer is None:
            return
        line = json.dumps(message, separators=(",", ":")) + "\n"
        self._writer.write(line.encode("utf-8"))
        await self._writer.drain()

    async def _send_event(
        self, request_id: str, event_type: str, fields: dict[str, Any],
    ) -> None:
        """Write an NDJSON event with request_id and type."""
        event: dict[str, Any] = {
            "request_id": request_id,
            "type": event_type,
        }
        event.update(fields)
        await self._send_line(event)


def _parse_generation_params(raw: dict[str, Any]) -> GenerationParams:
    """Convert a dict to GenerationParams with defaults."""
    return GenerationParams(
        temperature=raw.get("temperature", 0.7),
        top_p=raw.get("top_p", 0.9),
        max_tokens=raw.get("max_tokens", 512),
        stop_sequences=raw.get("stop_sequences", []),
        structured_schema=raw.get("structured_schema"),
    )


def _now_ms() -> int:
    """Current time in milliseconds."""
    return int(time.time() * 1000)


def load_adapter(module_path: str, class_name: str) -> AdapterBase:
    """Dynamically load an adapter class from module path and class name."""
    try:
        module = importlib.import_module(module_path)
    except ImportError as e:
        logger.error(f"Failed to import adapter module '{module_path}': {e}")
        sys.exit(1)

    cls = getattr(module, class_name, None)
    if cls is None:
        logger.error(f"Class '{class_name}' not found in module '{module_path}'")
        sys.exit(1)

    if not issubclass(cls, AdapterBase):
        logger.error(f"'{class_name}' does not inherit from AdapterBase")
        sys.exit(1)

    return cls()


def _read_startup_config() -> dict[str, Any]:
    """Read the startup config JSON from stdin (first line, blocking)."""
    line = sys.stdin.readline()
    if not line:
        logger.error("No startup config received on stdin (EOF)")
        sys.exit(1)
    try:
        return json.loads(line.strip())
    except json.JSONDecodeError as e:
        logger.error(f"Invalid startup config JSON: {e}")
        sys.exit(1)


def main() -> None:
    """Entry point. Usage: python3 runner.py <adapter_name>."""
    if len(sys.argv) != 2:
        print(
            "Usage: python3 runner.py <adapter_name>",
            file=sys.stderr,
        )
        sys.exit(1)

    adapter_name = sys.argv[1]
    logger.info(f"Runner started for adapter: {adapter_name}")

    startup_config = _read_startup_config()
    logger.info(f"Received startup config: adapter_name={startup_config.get('adapter_name')}")

    module_path = startup_config.get("module_path", "")
    entry_class = startup_config.get("entry_class", "")

    adapter: AdapterBase | None = None
    version = "1.0.0"

    if module_path and entry_class:
        adapter = load_adapter(module_path, entry_class)
        version = getattr(adapter, "_mai_adapter_version", "1.0.0")
    else:
        with contextlib.suppress(ImportError):
            importlib.import_module(f"adapters.{adapter_name}.adapter")
        cls = get_adapter(adapter_name)
        if cls is None:
            logger.error(
                f"Adapter '{adapter_name}' not found in registry and no "
                f"module_path/entry_class provided in startup config"
            )
            sys.exit(1)
        adapter = cls()
        version = getattr(cls, "_mai_adapter_version", "1.0.0")

    runner = AdapterRunner(adapter, adapter_name, version)

    try:
        asyncio.run(runner.run(startup_config))
    except KeyboardInterrupt:
        logger.info("Runner interrupted")
    except Exception:
        logger.error(f"Runner fatal error: {traceback.format_exc()}")
        sys.exit(1)


if __name__ == "__main__":
    main()
