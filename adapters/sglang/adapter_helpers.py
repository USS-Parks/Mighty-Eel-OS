"""SGLang adapter request and response normalization helpers."""

from __future__ import annotations

from typing import Any

from adapters.base import (
    AdapterError,
    AdapterTimeoutError,
    BackendCrashedError,
    FinishReason,
    GenerationParams,
    GenerationResult,
    maybe_await,
)
from adapters.sglang.client import SglangClient, SglangResponse, SglangStreamChunk


def build_kwargs(params: GenerationParams) -> dict[str, Any]:
    kwargs: dict[str, Any] = {}
    if params.max_tokens is not None:
        kwargs["max_tokens"] = params.max_tokens
    if params.temperature is not None:
        kwargs["temperature"] = params.temperature
    if params.top_p is not None:
        kwargs["top_p"] = params.top_p
    if params.stop:
        kwargs["stop"] = list(params.stop)
    extra = params.extra or {}
    if "json_schema" in extra:
        kwargs["json_schema"] = extra["json_schema"]
    if "regex" in extra:
        kwargs["regex"] = extra["regex"]
    return kwargs


def unwrap_body(resp: Any) -> dict[str, Any]:
    """Return the response body for SglangResponse or raw dict test doubles."""
    if isinstance(resp, SglangResponse):
        return resp.body
    if isinstance(resp, dict):
        return resp
    raise BackendCrashedError(
        f"unexpected sglang response type: {type(resp).__name__}",
    )


def result_from_response(resp: Any) -> GenerationResult:
    body = unwrap_body(resp)
    choices = body.get("choices") or []
    if not choices:
        raise BackendCrashedError("sglang response missing choices")
    first = choices[0] if isinstance(choices[0], dict) else {}
    message = first.get("message", {}) if isinstance(first.get("message"), dict) else {}
    usage = body.get("usage", {}) if isinstance(body.get("usage"), dict) else {}
    finish = first.get("finish_reason", "stop")
    reason = FinishReason.MAX_TOKENS if finish == "length" else FinishReason.STOP
    return GenerationResult(
        text=str(message.get("content", "")),
        tokens_generated=int(usage.get("completion_tokens", 0)),
        finish_reason=reason,
    )


def chunk_content(chunk: Any) -> str:
    if isinstance(chunk, SglangStreamChunk):
        return chunk.content or ""
    if isinstance(chunk, dict):
        choices = chunk.get("choices") or []
        if choices and isinstance(choices[0], dict):
            delta = choices[0].get("delta", {})
            if isinstance(delta, dict):
                return str(delta.get("content") or "")
    return ""


def chunk_finish_reason(chunk: Any) -> str | None:
    if isinstance(chunk, SglangStreamChunk):
        return chunk.finish_reason
    if isinstance(chunk, dict):
        choices = chunk.get("choices") or []
        if choices and isinstance(choices[0], dict):
            fr = choices[0].get("finish_reason")
            if fr is not None:
                return str(fr)
    return None


def build_native_kwargs(
    params: GenerationParams,
    json_schema: dict[str, Any] | None,
    regex: str | None,
) -> dict[str, Any]:
    kwargs: dict[str, Any] = {}
    if params.max_tokens is not None:
        kwargs["max_tokens"] = params.max_tokens
    if params.temperature is not None:
        kwargs["temperature"] = params.temperature
    if json_schema is not None:
        kwargs["json_schema"] = json_schema
    if regex is not None:
        kwargs["regex"] = regex
    return kwargs


def native_result_from_response(resp: Any) -> GenerationResult:
    body = unwrap_body(resp)
    meta = body.get("meta_info", {}) if isinstance(body.get("meta_info"), dict) else {}
    finish = meta.get("finish_reason", "stop")
    reason = FinishReason.MAX_TOKENS if finish == "length" else FinishReason.STOP
    return GenerationResult(
        text=str(body.get("text", "")),
        tokens_generated=int(meta.get("completion_tokens", 0)),
        finish_reason=reason,
    )


async def run_native_generate(
    client: SglangClient,
    prompt: str,
    params: GenerationParams,
    *,
    json_schema: dict[str, Any] | None,
    regex: str | None,
) -> GenerationResult:
    kwargs = build_native_kwargs(params, json_schema, regex)
    try:
        resp = await maybe_await(client.generate, prompt, **kwargs)
    except AdapterError:
        raise
    except TimeoutError as exc:
        raise AdapterTimeoutError(str(exc)) from exc
    except OSError as exc:
        raise BackendCrashedError(str(exc)) from exc
    return native_result_from_response(resp)
