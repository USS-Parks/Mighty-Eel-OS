"""Integration: top-k retrieves the right doc when one is much more relevant."""

from __future__ import annotations

import importlib.util
import json
import sys
from pathlib import Path

import httpx
import pytest
from mai import MaiClient, MaiClientConfig
from mai.retry import RetryPolicy

APP_ROOT = Path(__file__).resolve().parents[1]


def _load_main():
    spec = importlib.util.spec_from_file_location(
        "rag_reference_main_int", APP_ROOT / "main.py",
    )
    assert spec is not None
    assert spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def test_top_k_retrieves_most_similar_chunk(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path,
    capsys: pytest.CaptureFixture[str],
) -> None:
    # Synthetic doc set: three docs whose embeddings clearly differentiate.
    docs_dir = tmp_path / "docs"
    docs_dir.mkdir()
    (docs_dir / "alpha.md").write_text("ALPHA topic content here")
    (docs_dir / "beta.md").write_text("BETA topic content here")
    (docs_dir / "gamma.md").write_text("GAMMA topic content here")

    # Deterministic embedding mapping per chunk text prefix.
    def embed_for(text: str) -> list[float]:
        if text.startswith("ALPHA"):
            return [1.0, 0.0, 0.0]
        if text.startswith("BETA"):
            return [0.0, 1.0, 0.0]
        if text.startswith("GAMMA"):
            return [0.0, 0.0, 1.0]
        # Query "tell me about GAMMA" should hit the GAMMA vector.
        if "GAMMA" in text:
            return [0.0, 0.0, 1.0]
        if "ALPHA" in text:
            return [1.0, 0.0, 0.0]
        return [0.33, 0.33, 0.33]

    captured_chat: list[dict] = []

    def handler(req: httpx.Request) -> httpx.Response:
        if req.url.path == "/v1/embeddings":
            body = json.loads(req.content.decode())
            inp = body["input"] if isinstance(body["input"], list) else [body["input"]]
            data = [{"object": "embedding", "index": i,
                     "embedding": embed_for(t), "input_tokens": 1}
                    for i, t in enumerate(inp)]
            return httpx.Response(200, json={
                "object": "list", "data": data, "model": body["model"],
                "usage": {"prompt_tokens": len(inp), "completion_tokens": 0,
                          "total_tokens": len(inp)},
            })
        if req.url.path == "/v1/chat/completions":
            captured_chat.append(json.loads(req.content.decode()))
            return httpx.Response(200, json={
                "id": "x", "object": "chat.completion", "created": 1,
                "model": "c", "choices": [{
                    "index": 0,
                    "message": {"role": "assistant", "content": "ok"},
                    "finish_reason": "stop",
                }],
                "usage": {"prompt_tokens": 1, "completion_tokens": 1,
                          "total_tokens": 2},
            })
        return httpx.Response(404, json={"error": {
            "code": "MAI-N", "message": "?", "type": "internal_error",
        }})

    cfg = tmp_path / "cfg.toml"
    cfg.write_text(
        f'[ingest]\ndocs_dir = "{docs_dir.as_posix()}"\n'
        'embed_model = "e"\nchunk_chars = 50\n'
        '[retrieval]\ntop_k = 1\n'
        '[generation]\nchat_model = "c"\n',
    )

    main = _load_main()
    from collections.abc import Callable as _C
    def _client(h: _C[[httpx.Request], httpx.Response]) -> MaiClient:
        c = MaiClient(MaiClientConfig(
            base_url="http://test/v1",
            retry=RetryPolicy(max_retries=0, base_delay=0.0, jitter=0.0),
        ))
        c._http = httpx.Client(
            base_url="http://test/v1", headers={},
            transport=httpx.MockTransport(h),
        )
        return c

    monkeypatch.setattr(main, "_make_client", lambda _cfg: _client(handler))

    rc = main.run("tell me about GAMMA", config_path=cfg)
    captured = capsys.readouterr()
    assert rc == 0, captured.err
    assert len(captured_chat) == 1
    system_msg = next(m["content"] for m in captured_chat[0]["messages"]
                      if m["role"] == "system")
    # top_k=1 plus a clear GAMMA-best match => only gamma.md should be in context
    assert "gamma.md" in system_msg
    assert "alpha.md" not in system_msg
    assert "beta.md" not in system_msg
