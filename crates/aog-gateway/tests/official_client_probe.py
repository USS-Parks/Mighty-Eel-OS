"""Official-SDK probe for the LSH-G9 live adversarial compatibility gate.

The Rust harness owns OpenBao, the gateway, provider fixtures, receipts, and
revocation sequencing. This child process proves that the public wire surfaces
remain consumable by the vendor-maintained OpenAI and Anthropic Python SDKs.
"""

from __future__ import annotations

import os
from collections.abc import Callable
from typing import Any

import anthropic
import openai

BASE_URL = os.environ["AOG_G9_BASE_URL"].rstrip("/")
PHASE = os.environ.get("AOG_G9_PHASE", "matrix")


def openai_client(key: str) -> openai.OpenAI:
    return openai.OpenAI(
        api_key=key,
        base_url=f"{BASE_URL}/v1",
        max_retries=0,
        timeout=5.0,
    )


def anthropic_client(key: str) -> anthropic.Anthropic:
    return anthropic.Anthropic(
        api_key=key,
        base_url=BASE_URL,
        max_retries=0,
        timeout=5.0,
    )


def expect_status(call: Callable[[], Any], expected: int, label: str) -> None:
    try:
        call()
    except Exception as error:  # SDK families expose different status subclasses.
        actual = getattr(error, "status_code", None)
        assert actual == expected, f"{label}: expected HTTP {expected}, got {actual}: {error!r}"
        return
    raise AssertionError(f"{label}: unexpectedly succeeded")


def expect_stream_failure(call: Callable[[], Any], label: str) -> None:
    try:
        stream = call()
        for _ in stream:
            pass
    except Exception:
        return
    raise AssertionError(f"{label}: truncated stream looked like normal completion")


def run_matrix() -> None:
    tenant_a = openai_client("vk_g9_tenant_a")
    tenant_b = anthropic_client("vk_g9_tenant_b")

    chat = tenant_a.chat.completions.create(
        model="gpt-4o-mini",
        messages=[{"role": "user", "content": "VALID_OPENAI_CHAT"}],
        max_tokens=8,
        extra_headers={"x-aog-workflow": "g9-openai-chat"},
    )
    assert chat.choices[0].message.content == "gateway-ok"

    chunks = tenant_a.chat.completions.create(
        model="gpt-4o-mini",
        messages=[{"role": "user", "content": "VALID_OPENAI_STREAM"}],
        max_tokens=8,
        stream=True,
        extra_headers={"x-aog-workflow": "g9-openai-stream"},
    )
    openai_text = "".join(
        chunk.choices[0].delta.content or "" for chunk in chunks if chunk.choices
    )
    assert openai_text == "gateway-ok"

    legacy = tenant_a.completions.create(
        model="gpt-4o-mini",
        prompt="VALID_OPENAI_LEGACY",
        max_tokens=8,
        extra_headers={"x-aog-workflow": "g9-openai-legacy"},
    )
    assert legacy.choices[0].text == "gateway-ok"

    message = tenant_b.messages.create(
        model="claude-3-5-sonnet",
        max_tokens=8,
        messages=[{"role": "user", "content": "VALID_ANTHROPIC_MESSAGE"}],
        extra_headers={"x-aog-workflow": "g9-anthropic-message"},
    )
    assert message.content[0].text == "gateway-ok"

    with tenant_b.messages.stream(
        model="claude-3-5-sonnet",
        max_tokens=8,
        messages=[{"role": "user", "content": "VALID_ANTHROPIC_STREAM"}],
        extra_headers={"x-aog-workflow": "g9-anthropic-stream"},
    ) as stream:
        anthropic_text = "".join(stream.text_stream)
        final_message = stream.get_final_message()
    assert anthropic_text == "gateway-ok"
    assert final_message.stop_reason == "end_turn"

    expect_status(
        lambda: tenant_a.chat.completions.create(
            model="gpt-4o-mini",
            messages=[
                {
                    "role": "user",
                    "content": "Patient John Doe SSN 123-45-6789 ROUTE_DENY",
                }
            ],
            max_tokens=2,
        ),
        403,
        "route denial",
    )

    budget = openai_client("vk_g9_budget")
    first = budget.chat.completions.create(
        model="gpt-4o-mini",
        messages=[{"role": "user", "content": "BUDGET"}],
        max_tokens=1,
    )
    assert first.choices[0].message.content == "ok"
    expect_status(
        lambda: budget.chat.completions.create(
            model="gpt-4o-mini",
            messages=[{"role": "user", "content": "BUDGET"}],
            max_tokens=1,
        ),
        402,
        "budget exhaustion",
    )

    expect_status(
        lambda: tenant_a.chat.completions.create(
            model="gpt-4o-mini",
            messages=[{"role": "user", "content": "FAULT_REDIRECT"}],
            max_tokens=2,
        ),
        307,
        "provider redirect",
    )
    expect_status(
        lambda: tenant_b.messages.create(
            model="claude-3-5-sonnet",
            max_tokens=2,
            messages=[{"role": "user", "content": "FAULT_MALFORMED"}],
        ),
        502,
        "malformed provider JSON",
    )
    expect_status(
        lambda: tenant_a.chat.completions.create(
            model="gpt-4o-mini",
            messages=[{"role": "user", "content": "FAULT_OVERSIZED"}],
            max_tokens=2,
        ),
        502,
        "oversized provider body",
    )
    expect_stream_failure(
        lambda: tenant_a.chat.completions.create(
            model="gpt-4o-mini",
            messages=[{"role": "user", "content": "FAULT_TRUNCATED"}],
            max_tokens=2,
            stream=True,
        ),
        "OpenAI truncated stream",
    )
    expect_stream_failure(
        lambda: tenant_b.messages.create(
            model="claude-3-5-sonnet",
            max_tokens=2,
            messages=[{"role": "user", "content": "FAULT_TRUNCATED"}],
            stream=True,
        ),
        "Anthropic truncated stream",
    )

    false_usage = tenant_a.chat.completions.create(
        model="gpt-4o-mini",
        messages=[{"role": "user", "content": "FALSE_USAGE"}],
        max_tokens=16,
        extra_headers={"x-aog-workflow": "g9-false-usage"},
    )
    assert false_usage.usage is not None
    assert false_usage.usage.total_tokens == 2, "provider evidence remains SDK-compatible"
    assert false_usage.choices[0].message.content == "x" * 32

    before_revoke = openai_client("vk_g9_revoke").chat.completions.create(
        model="gpt-4o-mini",
        messages=[{"role": "user", "content": "BEFORE_REVOKE"}],
        max_tokens=2,
    )
    assert before_revoke.choices[0].message.content == "gateway-ok"


def run_revoked() -> None:
    revoked = openai_client("vk_g9_revoke")
    expect_status(
        lambda: revoked.chat.completions.create(
            model="gpt-4o-mini",
            messages=[{"role": "user", "content": "AFTER_REVOKE"}],
            max_tokens=2,
        ),
        403,
        "revocation",
    )


if __name__ == "__main__":
    print(f"official SDKs: openai={openai.__version__} anthropic={anthropic.__version__}")
    if PHASE == "matrix":
        run_matrix()
    elif PHASE == "revoked":
        run_revoked()
    else:
        raise AssertionError(f"unknown phase: {PHASE}")
    print(f"official-client phase {PHASE}: PASS")
