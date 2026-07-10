"""IPC frame-limit tests for the adapter runner.

The runner reads request lines with an asyncio StreamReader. Its default 64 KiB
limit sat below the advertised MAX_PROMPT_CHARS (200 K), so a legitimate large
prompt overran the reader and crashed the (restarted) worker. The runner now
sizes the reader off MAX_FRAME_BYTES and treats a genuine overrun as a bounded
error the worker survives. These tests exercise the StreamReader mechanism the
runner's loop depends on, standalone (no subprocess).
"""

from __future__ import annotations

import asyncio
import json

import pytest

from adapters.base import MAX_PROMPT_CHARS
from adapters.runner import MAX_FRAME_BYTES


def test_frame_cap_exceeds_the_advertised_prompt_cap() -> None:
    # The reader must be able to hold a max-size prompt even at 4 bytes/char
    # UTF-8 plus the JSON envelope; the runner asserts this at import too.
    assert MAX_FRAME_BYTES > MAX_PROMPT_CHARS * 4


def test_max_prompt_frame_round_trips() -> None:
    # A prompt at the advertised 200 K-char cap, wrapped as one NDJSON line,
    # reads back intact instead of overrunning the reader.
    prompt = "x" * MAX_PROMPT_CHARS
    line = (
        json.dumps(
            {"request_id": "r", "type": "inference", "payload": {"prompt": prompt}}
        ).encode("utf-8")
        + b"\n"
    )
    assert len(line) < MAX_FRAME_BYTES, "the framed prompt fits under the cap"

    async def _run() -> bytes:
        reader = asyncio.StreamReader(limit=MAX_FRAME_BYTES)
        reader.feed_data(line)
        reader.feed_eof()
        return await reader.readline()

    got = asyncio.run(_run())
    parsed = json.loads(got.decode("utf-8"))
    assert len(parsed["payload"]["prompt"]) == MAX_PROMPT_CHARS


def test_oversize_frame_raises_and_reader_recovers() -> None:
    # A frame past the limit raises ValueError (what the runner's loop catches),
    # and the reader recovers so the *next* frame still parses — the worker is
    # never terminated by one oversize line. A tiny limit forces the overrun
    # deterministically without allocating 8 MiB.
    async def _run() -> bytes:
        reader = asyncio.StreamReader(limit=64)
        reader.feed_data(b"x" * 200 + b"\n")  # exceeds the 64-byte cap
        reader.feed_data(b'{"ok":1}\n')  # a valid frame right behind it
        reader.feed_eof()
        with pytest.raises(ValueError):
            await reader.readline()
        # readline cleared the oversize frame from the buffer on overrun.
        return await reader.readline()

    nxt = asyncio.run(_run())
    assert json.loads(nxt.decode("utf-8")) == {"ok": 1}
