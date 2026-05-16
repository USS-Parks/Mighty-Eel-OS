"""MAI SDK client implementations.

Provides sync and async HTTP clients for the MAI API. Uses httpx
for HTTP transport and Pydantic for request/response serialization.

Session 05 deliverable: client skeleton with method signatures.
Full implementation in Session 11.
"""

from __future__ import annotations

from collections.abc import AsyncIterator, Iterator
from typing import Any
from uuid import UUID

import httpx

from mai.types import (
    AuditLogResponse,
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
    MaiError,
    ModelDetail,
    ModelObject,
    PowerStateResponse,
    ProfileObject,
    RequestPriority,
    StructuredRequest,
    StructuredResponse,
)


# ---------------------------------------------------------------------------
# Configuration
# ---------------------------------------------------------------------------

class MaiClientConfig:
    """Client configuration."""

    def __init__(
        self,
        base_url: str = "http://localhost:8420/v1",
        profile_id: UUID | str | None = None,
        priority: RequestPriority = RequestPriority.NORMAL,
        timeout: float = 60.0,
        stream_timeout: float = 300.0,
    ) -> None:
        self.base_url = base_url.rstrip("/")
        self.profile_id = str(profile_id) if profile_id else None
        self.priority = priority
        self.timeout = timeout
        self.stream_timeout = stream_timeout

    def headers(self) -> dict[str, str]:
        """Build common request headers."""
        h: dict[str, str] = {"Content-Type": "application/json"}
        if self.profile_id:
            h["X-IM-Profile"] = self.profile_id
        h["X-IM-Priority"] = self.priority.value
        return h


# ---------------------------------------------------------------------------
# Sync client
# ---------------------------------------------------------------------------

class MaiClient:
    """Synchronous MAI API client.

    Usage::

        client = MaiClient()
        response = client.chat("qwen3-14b:Q4_K_M", [
            ChatMessage(role="user", content="Hello"),
        ])
        print(response.choices[0].message.content)

    For streaming::

        for chunk in client.chat_stream("qwen3-14b:Q4_K_M", messages):
            print(chunk.choices[0].get("delta", {}).get("content", ""), end="")
    """

    def __init__(self, config: MaiClientConfig | None = None) -> None:
        self._config = config or MaiClientConfig()
        self._http = httpx.Client(
            base_url=self._config.base_url,
            headers=self._config.headers(),
            timeout=self._config.timeout,
        )

    def close(self) -> None:
        """Close the underlying HTTP connection pool."""
        self._http.close()

    def __enter__(self) -> MaiClient:
        return self

    def __exit__(self, *_: Any) -> None:
        self.close()

    # --- Inference ---

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
        resp = self._http.post("/chat/completions", json=req.model_dump())
        self._check_error(resp)
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
        # Implementation: parse SSE events from httpx stream
        # Full implementation in Session 11
        raise NotImplementedError("Streaming client implemented in Session 11")

    def complete(self, model: str, prompt: str, **kwargs: Any) -> CompletionResponse:
        """Text completion."""
        req = CompletionRequest(model=model, prompt=prompt, stream=False, **kwargs)
        resp = self._http.post("/completions", json=req.model_dump())
        self._check_error(resp)
        return CompletionResponse.model_validate(resp.json())

    def embed(self, model: str, input_: str | list[str]) -> EmbeddingResponse:
        """Text embedding."""
        req = EmbeddingRequest(model=model, input=input_)
        resp = self._http.post("/embeddings", json=req.model_dump())
        self._check_error(resp)
        return EmbeddingResponse.model_validate(resp.json())

    def structured(
        self, model: str, prompt: str, schema: dict[str, Any], **kwargs: Any
    ) -> StructuredResponse:
        """JSON schema-constrained generation."""
        req = StructuredRequest(model=model, prompt=prompt, schema=schema, **kwargs)
        resp = self._http.post("/generate/structured", json=req.model_dump(by_alias=True))
        self._check_error(resp)
        return StructuredResponse.model_validate(resp.json())

    def function_call(
        self,
        model: str,
        messages: list[ChatMessage],
        functions: list[dict[str, Any]],
    ) -> FunctionCallResponse:
        """Function/tool calling."""
        req = FunctionCallRequest(model=model, messages=messages, functions=functions)
        resp = self._http.post("/generate/function_call", json=req.model_dump())
        self._check_error(resp)
        return FunctionCallResponse.model_validate(resp.json())

    # --- Models ---

    def list_models(self, **filters: Any) -> list[ModelObject]:
        """List available models."""
        resp = self._http.get("/models", params=filters)
        self._check_error(resp)
        data = resp.json()
        return [ModelObject.model_validate(m) for m in data.get("data", [])]

    def get_model(self, model_id: str) -> ModelDetail:
        """Get model detail."""
        resp = self._http.get(f"/models/{model_id}")
        self._check_error(resp)
        return ModelDetail.model_validate(resp.json())

    # --- Health ---

    def health(self) -> HealthResponse:
        """System health (no auth required)."""
        resp = self._http.get("/health")
        self._check_error(resp)
        return HealthResponse.model_validate(resp.json())

    def hardware_health(self) -> HardwareHealthResponse:
        """Hardware health."""
        resp = self._http.get("/health/hardware")
        self._check_error(resp)
        return HardwareHealthResponse.model_validate(resp.json())

    # --- Power ---

    def power_state(self) -> PowerStateResponse:
        """Current power state."""
        resp = self._http.get("/power/state")
        self._check_error(resp)
        return PowerStateResponse.model_validate(resp.json())

    # --- Error handling ---

    @staticmethod
    def _check_error(resp: httpx.Response) -> None:
        """Raise MaiError on non-2xx responses."""
        if resp.status_code >= 400:
            try:
                err = ErrorResponse.model_validate(resp.json())
            except Exception:
                raise MaiError(ErrorResponse(error={
                    "code": f"MAI-{resp.status_code}0",
                    "message": resp.text,
                    "type": "internal_error",
                }))
            raise MaiError(err)


# ---------------------------------------------------------------------------
# Async client
# ---------------------------------------------------------------------------

class AsyncMaiClient:
    """Asynchronous MAI API client.

    Usage::

        async with AsyncMaiClient() as client:
            response = await client.chat("qwen3-14b:Q4_K_M", messages)

    For streaming::

        async for chunk in client.chat_stream("qwen3-14b:Q4_K_M", messages):
            print(chunk)
    """

    def __init__(self, config: MaiClientConfig | None = None) -> None:
        self._config = config or MaiClientConfig()
        self._http = httpx.AsyncClient(
            base_url=self._config.base_url,
            headers=self._config.headers(),
            timeout=self._config.timeout,
        )

    async def close(self) -> None:
        """Close the underlying HTTP connection pool."""
        await self._http.aclose()

    async def __aenter__(self) -> AsyncMaiClient:
        return self

    async def __aexit__(self, *_: Any) -> None:
        await self.close()

    # --- Inference ---

    async def chat(
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
        resp = await self._http.post("/chat/completions", json=req.model_dump())
        self._check_error(resp)
        return ChatCompletionResponse.model_validate(resp.json())

    async def chat_stream(
        self,
        model: str,
        messages: list[ChatMessage],
        *,
        temperature: float = 0.7,
        top_p: float = 0.9,
        max_tokens: int = 2048,
        **kwargs: Any,
    ) -> AsyncIterator[ChatCompletionChunk]:
        """Streaming chat completion via SSE."""
        # Full implementation in Session 11
        raise NotImplementedError("Async streaming client implemented in Session 11")
        yield  # noqa: unreachable - makes this an async generator

    async def complete(self, model: str, prompt: str, **kwargs: Any) -> CompletionResponse:
        """Text completion."""
        req = CompletionRequest(model=model, prompt=prompt, stream=False, **kwargs)
        resp = await self._http.post("/completions", json=req.model_dump())
        self._check_error(resp)
        return CompletionResponse.model_validate(resp.json())

    async def embed(self, model: str, input_: str | list[str]) -> EmbeddingResponse:
        """Text embedding."""
        req = EmbeddingRequest(model=model, input=input_)
        resp = await self._http.post("/embeddings", json=req.model_dump())
        self._check_error(resp)
        return EmbeddingResponse.model_validate(resp.json())

    async def structured(
        self, model: str, prompt: str, schema: dict[str, Any], **kwargs: Any
    ) -> StructuredResponse:
        """JSON schema-constrained generation."""
        req = StructuredRequest(model=model, prompt=prompt, schema=schema, **kwargs)
        resp = await self._http.post("/generate/structured", json=req.model_dump(by_alias=True))
        self._check_error(resp)
        return StructuredResponse.model_validate(resp.json())

    async def function_call(
        self,
        model: str,
        messages: list[ChatMessage],
        functions: list[dict[str, Any]],
    ) -> FunctionCallResponse:
        """Function/tool calling."""
        req = FunctionCallRequest(model=model, messages=messages, functions=functions)
        resp = await self._http.post("/generate/function_call", json=req.model_dump())
        self._check_error(resp)
        return FunctionCallResponse.model_validate(resp.json())

    # --- Models ---

    async def list_models(self, **filters: Any) -> list[ModelObject]:
        """List available models."""
        resp = await self._http.get("/models", params=filters)
        self._check_error(resp)
        data = resp.json()
        return [ModelObject.model_validate(m) for m in data.get("data", [])]

    async def get_model(self, model_id: str) -> ModelDetail:
        """Get model detail."""
        resp = await self._http.get(f"/models/{model_id}")
        self._check_error(resp)
        return ModelDetail.model_validate(resp.json())

    # --- Health ---

    async def health(self) -> HealthResponse:
        """System health."""
        resp = await self._http.get("/health")
        self._check_error(resp)
        return HealthResponse.model_validate(resp.json())

    async def hardware_health(self) -> HardwareHealthResponse:
        """Hardware health."""
        resp = await self._http.get("/health/hardware")
        self._check_error(resp)
        return HardwareHealthResponse.model_validate(resp.json())

    # --- Power ---

    async def power_state(self) -> PowerStateResponse:
        """Current power state."""
        resp = await self._http.get("/power/state")
        self._check_error(resp)
        return PowerStateResponse.model_validate(resp.json())

    # --- Error handling ---

    @staticmethod
    def _check_error(resp: httpx.Response) -> None:
        """Raise MaiError on non-2xx responses."""
        if resp.status_code >= 400:
            try:
                err = ErrorResponse.model_validate(resp.json())
            except Exception:
                raise MaiError(ErrorResponse(error={
                    "code": f"MAI-{resp.status_code}0",
                    "message": resp.text,
                    "type": "internal_error",
                }))
            raise MaiError(err)
