"""Triton adapter text tensor helpers."""

from __future__ import annotations

from typing import Any

from adapters.base import BackendUnavailableError


def build_text_input(
    tensor_name: str, prompts: list[str],
) -> list[dict[str, Any]]:
    return [{
        "name": tensor_name,
        "shape": [len(prompts)],
        "datatype": "BYTES",
        "data": list(prompts),
    }]


def decode_text_outputs(resp_body: dict[str, Any], target: str) -> list[str]:
    outputs = resp_body.get("outputs", [])
    if not isinstance(outputs, list):
        raise BackendUnavailableError(
            detail="triton response missing outputs list",
        )
    for out in outputs:
        if not isinstance(out, dict):
            continue
        if out.get("name") != target:
            continue
        data = out.get("data", [])
        if not isinstance(data, list):
            return []
        return flatten_text_output(data)
    raise BackendUnavailableError(
        detail=f"triton response missing output tensor '{target}'",
    )


def flatten_text_output(data: list[Any]) -> list[str]:
    flat: list[str] = []
    for item in data:
        if isinstance(item, list):
            flat.extend(str(x) for x in item)
        elif isinstance(item, (bytes, bytearray)):
            flat.append(item.decode("utf-8", errors="replace"))
        else:
            flat.append(str(item))
    return flat
