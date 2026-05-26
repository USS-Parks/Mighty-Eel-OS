"""ONNX Runtime client import and generation helpers."""

from __future__ import annotations

import importlib
from collections.abc import Iterator
from typing import Any

from adapters.onnxruntime.types import OnnxRuntimeClientError, OnnxStreamChunk


def safe_import(module_name: str) -> Any:
    try:
        return importlib.import_module(module_name)
    except ImportError:
        return None


def generate_genai_chunks(
    genai: Any,
    model: Any,
    tokenizer: Any,
    prompt: str,
    *,
    max_tokens: int,
    temperature: float,
    top_p: float,
) -> Iterator[OnnxStreamChunk]:
    try:
        params = genai.GeneratorParams(model)
        params.set_search_options(
            max_length=max_tokens,
            temperature=temperature,
            top_p=top_p,
        )
        params.input_ids = tokenizer.encode(prompt)

        generator = genai.Generator(model, params)
        stream = tokenizer.create_stream()
        yielded_any = False
        try:
            while not generator.is_done():
                generator.compute_logits()
                generator.generate_next_token()
                new_token = generator.get_next_tokens()[0]
                text = stream.decode(new_token)
                if text:
                    yielded_any = True
                    yield OnnxStreamChunk(text=text, is_final=False)
        finally:
            _ = yielded_any
            yield OnnxStreamChunk(text="", is_final=True)
    except MemoryError as exc:
        raise OnnxRuntimeClientError(
            "OutOfMemory", f"ONNX Runtime memory exhausted: {exc}",
        ) from exc
    except OnnxRuntimeClientError:
        raise
    except Exception as exc:
        raise OnnxRuntimeClientError(
            "BackendCrashed", f"ONNX Runtime generation failed: {exc}",
        ) from exc
