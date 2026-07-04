"""MAI SDK sync client.

rewrite:
    * exception hierarchy in :mod:`mai.errors`
    * retry policy in :mod:`mai.retry`
    * config loader in :mod:`mai.config`
    * namespace classes in :mod:`mai._namespaces` (``.models``, ``.power``,
      ``.system``, ``.scheduler``, ``.updates``, ``.admin``, ``.auth``,
      ``.trust``, ``.compliance``)
    * async client extracted to :mod:`mai.async_client`

Top-level convenience methods (chat, complete, embed, stream_chat,
health, …) stay on ``MaiClient`` directly so existing app code works.
"""

from __future__ import annotations

import json
import time
from collections.abc import Iterator
from typing import TYPE_CHECKING, Any

import httpx

from mai._namespaces import (
    Admin,
    Auth,
    Compliance,
    Models,
    Power,
    Scheduler,
    System,
    Trust,
    Updates,
)
from mai.config import MaiClientConfig
from mai.errors import MaiError, from_response, from_transport
from mai.types import (
    ChatCompletionChunk,
    ChatCompletionRequest,
    ChatCompletionResponse,
    ChatMessage,
    CompletionRequest,
    CompletionResponse,
    EmbeddingRequest,
    EmbeddingResponse,
    ErrorResponse,
    FunctionCallRequest,
    FunctionCallResponse,
    HardwareHealthResponse,
    HealthResponse,
    ModelDetail,
    ModelObject,
    PowerStateResponse,
    StructuredRequest,
    StructuredResponse,
)

if TYPE_CHECKING:
    from types import TracebackType

_HTTP_ERROR_STATUS = 400


def _parse_sse_line(line: str) -> str | None:
    """Parse a single SSE data line, returning the data payload or None."""
    line = line.strip()
    if not line or line.startswith(":"):
        return None
    if line.startswith("data: "):
        data = line[6:]
        if data == "[DONE]":
            return None
        return data
    return None


def _build_error(resp: httpx.Response) -> MaiError:
    """Build a typed MaiError from an httpx error response."""
    try:
        err_resp = ErrorResponse.model_validate(resp.json())
    except Exception:
        err_resp = ErrorResponse.model_validate({"error": {
            "code": f"MAI-{resp.status_code}0",
            "message": resp.text or f"HTTP {resp.status_code}",
            "type": "internal_error",
        }})
    return from_response(err_resp, resp.status_code)


class MaiClient:
    """Synchronous MAI API client.

    Construction::

        from mai import MaiClient, MaiClientConfig

        # explicit config
        client = MaiClient(MaiClientConfig(api_key="im-..."))

        # from env / file / defaults
        client = MaiClient.from_env()
        client = MaiClient.from_file("~/.config/mai/config.toml")

    Inference example::

        response = client.chat("qwen3-14b:Q4_K_M", [
            ChatMessage(role="user", content="Hello"),
        ])
        print(response.choices[0].message.content)

    Namespaces:
        ``client.models``       - model lifecycle (list, load, unload, …)
        ``client.power``        - power state and transitions
        ``client.system``       - air-gap, system/hardware health
        ``client.scheduler``    - scheduler metrics & instances
        ``client.updates``      - OTA update channel
        ``client.admin``        - profiles, audit log, registry
        ``client.auth``         - token exchange APIs
        ``client.trust``        - trust claims/bundle APIs
        ``client.compliance``   - Lamprey compliance APIs
    """

    def __init__(self, config: MaiClientConfig | None = None) -> None:
        self._config = config or MaiClientConfig()
        self._http = httpx.Client(
            base_url=self._config.base_url,
            headers=self._config.headers(),
            timeout=self._config.timeout,
        )
        self.models = Models(self)
        self.power = Power(self)
        self.system = System(self)
        self.scheduler = Scheduler(self)
        self.updates = Updates(self)
        self.admin = Admin(self)
        self.auth = Auth(self)
        self.trust = Trust(self)
        self.compliance = Compliance(self)

    # --- Factories -----------------------------------------------------

    @classmethod
    def from_env(cls, **overrides: Any) -> MaiClient:
        """Construct from env vars (see :mod:`mai.config`)."""
        return cls(MaiClientConfig.from_env(**overrides))

    @classmethod
    def from_file(cls, path: str, **overrides: Any) -> MaiClient:
        """Construct from a TOML config file."""
        return cls(MaiClientConfig.from_file(path, **overrides))

    @classmethod
    def load(cls, path: str | None = None, **overrides: Any) -> MaiClient:
        """Full precedence: overrides > env > file > defaults."""
        return cls(MaiClientConfig.load(path, **overrides))

    # --- Context manager ----------------------------------------------

    def close(self) -> None:
        """Close the underlying HTTP connection pool."""
        self._http.close()

    def __enter__(self) -> MaiClient:
        return self

    def __exit__(
        self,
        exc_type: type[BaseException] | None,
        exc: BaseException | None,
        tb: TracebackType | None,
    ) -> None:
        self.close()

    # --- Inference (top-level convenience) -----------------------------

    def chat(
        self,
        model: str,
        messages: list[ChatMessage],
        *,
        temperature: float = 0.7,
        top_p: float = 0.9,
        max_tokens: int = 2048,
        **kwargs: Any,
    ) -> ChatCompletionResponse:
        """Non-streaming chat completion."""
        req = ChatCompletionRequest(
            model=model,
            messages=messages,
            temperature=temperature,
            top_p=top_p,
            max_tokens=max_tokens,
            stream=False,
            **kwargs,
        )
        resp = self._request_with_retry(
            "POST", "/chat/completions", json=req.model_dump(),
        )
        return ChatCompletionResponse.model_validate(resp.json())

    def chat_stream(
        self,
        model: str,
        messages: list[ChatMessage],
        *,
        temperature: float = 0.7,
        top_p: float = 0.9,
        max_tokens: int = 2048,
        **kwargs: Any,
    ) -> Iterator[ChatCompletionChunk]:
        """Streaming chat completion via SSE."""
        req = ChatCompletionRequest(
            model=model,
            messages=messages,
            temperature=temperature,
            top_p=top_p,
            max_tokens=max_tokens,
            stream=True,
            **kwargs,
        )
        with self._http.stream(
            "POST",
            "/chat/completions",
            json=req.model_dump(),
            timeout=self._config.stream_timeout,
        ) as response:
            if response.status_code >= _HTTP_ERROR_STATUS:
                response.read()
                raise _build_error(response)
            for line in response.iter_lines():
                data = _parse_sse_line(line)
                if data is not None:
                    yield ChatCompletionChunk.model_validate(json.loads(data))

    # Alias kept for the spec name "stream_chat".
    stream_chat = chat_stream

    def stream_completions(
        self,
        model: str,
        prompt: str,
        **kwargs: Any,
    ) -> Iterator[ChatCompletionChunk]:
        """Streaming text completion (wraps prompt as a single user msg)."""
        messages = [ChatMessage(role="user", content=prompt)]
        yield from self.chat_stream(model, messages, **kwargs)

    def complete(
        self, model: str, prompt: str, **kwargs: Any,
    ) -> CompletionResponse:
        """Non-streaming text completion."""
        req = CompletionRequest(model=model, prompt=prompt, stream=False, **kwargs)
        resp = self._request_with_retry(
            "POST", "/completions", json=req.model_dump(),
        )
        return CompletionResponse.model_validate(resp.json())

    # Alias to match spec name "completions".
    completions = complete

    def embed(self, model: str, input_: str | list[str]) -> EmbeddingResponse:
        """Text embedding."""
        req = EmbeddingRequest(model=model, input=input_)
        resp = self._request_with_retry(
            "POST", "/embeddings", json=req.model_dump(),
        )
        return EmbeddingResponse.model_validate(resp.json())

    # Alias to match spec name "embeddings".
    embeddings = embed

    def structured(
        self,
        model: str,
        prompt: str,
        schema: dict[str, Any],
        **kwargs: Any,
    ) -> StructuredResponse:
        """JSON schema-constrained generation."""
        req = StructuredRequest(model=model, prompt=prompt, schema=schema, **kwargs)
        resp = self._request_with_retry(
            "POST", "/generate/structured", json=req.model_dump(by_alias=True),
        )
        return StructuredResponse.model_validate(resp.json())

    structured_generation = structured

    def function_call(
        self,
        model: str,
        messages: list[ChatMessage],
        functions: list[dict[str, Any]],
    ) -> FunctionCallResponse:
        """Function/tool calling."""
        req = FunctionCallRequest(
            model=model, messages=messages, functions=functions,
        )
        resp = self._request_with_retry(
            "POST", "/generate/function_call", json=req.model_dump(),
        )
        return FunctionCallResponse.model_validate(resp.json())

    # --- Top-level model convenience shortcuts -------------------------

    def list_models(self, **filters: Any) -> list[ModelObject]:
        """Convenience: alias for ``client.models.list(...)``."""
        return self.models.list(**filters)

    def get_model(self, model_id: str) -> ModelDetail:
        """Convenience: alias for ``client.models.get(...)``."""
        return self.models.get(model_id)

    # --- Health --------------------------------------------------------

    def health(self) -> HealthResponse:
        """GET /v1/health — no auth required."""
        resp = self._http.get("/health")
        self._check_error(resp)
        return HealthResponse.model_validate(resp.json())

    def health_check(self) -> bool:
        """Quick reachability check. Returns False on any failure."""
        try:
            resp = self._http.get("/health")
            return resp.status_code < _HTTP_ERROR_STATUS
        except Exception:
            return False

    def hardware_health(self) -> HardwareHealthResponse:
        """Convenience: alias for ``client.system.hardware_health()``."""
        return self.system.hardware_health()

    def power_state(self) -> PowerStateResponse:
        """Convenience: alias for ``client.power.get_state()``."""
        return self.power.get_state()

    # --- Transport core ------------------------------------------------

    def _request_with_retry(
        self, method: str, url: str, **kwargs: Any,
    ) -> httpx.Response:
        """HTTP request with retry per the client's RetryPolicy."""
        last_error: MaiError | None = None
        policy = self._config.retry
        for attempt in range(policy.max_retries + 1):
            try:
                resp = self._http.request(method, url, **kwargs)
            except httpx.TransportError as exc:
                last_error = from_transport(exc)
                delay = policy.should_retry(last_error, attempt)
                if delay is None:
                    raise last_error from exc
                time.sleep(delay)
                continue

            if resp.status_code < _HTTP_ERROR_STATUS:
                return resp

            last_error = _build_error(resp)
            delay = policy.should_retry(last_error, attempt)
            if delay is None:
                raise last_error
            time.sleep(delay)

        if last_error is not None:
            raise last_error
        raise RuntimeError("Retry loop exited without result")

    @staticmethod
    def _check_error(resp: httpx.Response) -> None:
        """Raise the right MaiError subclass on non-2xx responses."""
        if resp.status_code >= _HTTP_ERROR_STATUS:
            raise _build_error(resp)


# Async client lives in mai.async_client now.
from mai.async_client import AsyncMaiClient  # noqa: E402, I001 — late import to expose top-level alias

__all__ = [
    "AsyncMaiClient",
    "MaiClient",
    "MaiClientConfig",
]
