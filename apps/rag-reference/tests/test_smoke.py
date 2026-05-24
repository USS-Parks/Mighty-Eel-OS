"""Smoke tests for the RAG scaffold."""

from __future__ import annotations

import importlib.util
import json
import sys
from collections.abc import Callable
from pathlib import Path

import httpx
import pytest
from mai import MaiClient, MaiClientConfig
from mai.retry import RetryPolicy

APP_ROOT = Path(__file__).resolve().parents[1]


def _load_main():
    spec = importlib.util.spec_from_file_location(
        "rag_reference_main", APP_ROOT / "main.py",
    )
    assert spec is not None
    assert spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def _mock_client(handler: Callable[[httpx.Request], httpx.Response]) -> MaiClient:
    cfg = MaiClientConfig(
        base_url="http://test/v1",
        retry=RetryPolicy(max_retries=0, base_delay=0.0, jitter=0.0),
    )
    client = MaiClient(cfg)
    client._http = httpx.Client(
        base_url=cfg.base_url, headers=cfg.headers(),
        timeout=cfg.timeout, transport=httpx.MockTransport(handler),
    )
    return client


# --- unit-ish: cosine and chunking ----------------------------------------

def test_cosine_basic() -> None:
    main = _load_main()
    assert main.cosine([1.0, 0.0], [1.0, 0.0]) == 1.0
    assert main.cosine([1.0, 0.0], [0.0, 1.0]) == 0.0
    assert main.cosine([], [1.0]) == 0.0
    assert main.cosine([0.0, 0.0], [1.0, 0.0]) == 0.0


def test_chunk_text_splits_evenly() -> None:
    main = _load_main()
    chunks = main.chunk_text("abcdefghij", 4)
    assert chunks == ["abcd", "efgh", "ij"]
    assert main.chunk_text("   ", 5) == []


def test_vector_store_top_k_ranking() -> None:
    main = _load_main()
    store = main.VectorStore()
    store.add(main.Chunk(doc_id="a", text="cats", embedding=[1.0, 0.0]))
    store.add(main.Chunk(doc_id="b", text="dogs", embedding=[0.0, 1.0]))
    hits = store.top_k([1.0, 0.0], k=1)
    assert len(hits) == 1
    assert hits[0][1].text == "cats"


# --- end-to-end with mocked server ----------------------------------------

def test_run_completes_with_sample_docs(
    monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str],
) -> None:
    embed_calls: list[list[str]] = []
    chat_calls: list[dict] = []

    def handler(req: httpx.Request) -> httpx.Response:
        if req.url.path == "/v1/embeddings":
            body = json.loads(req.content.decode())
            inp = body["input"] if isinstance(body["input"], list) else [body["input"]]
            embed_calls.append(inp)
            data = [{"object": "embedding", "index": i,
                     "embedding": [1.0 / (i + 1), 0.5], "input_tokens": 1}
                    for i in range(len(inp))]
            return httpx.Response(200, json={
                "object": "list", "data": data, "model": body["model"],
                "usage": {"prompt_tokens": len(inp), "completion_tokens": 0,
                          "total_tokens": len(inp)},
            })
        if req.url.path == "/v1/chat/completions":
            chat_calls.append(json.loads(req.content.decode()))
            return httpx.Response(200, json={
                "id": "x", "object": "chat.completion", "created": 1,
                "model": "qwen3-14b:Q4_K_M",
                "choices": [{"index": 0, "message": {
                    "role": "assistant", "content": "Port 8420.",
                }, "finish_reason": "stop"}],
                "usage": {"prompt_tokens": 50, "completion_tokens": 5,
                          "total_tokens": 55},
            })
        return httpx.Response(404, json={"error": {
            "code": "MAI-N", "message": "?", "type": "internal_error",
        }})

    main = _load_main()
    monkeypatch.setattr(main, "_make_client", lambda _cfg: _mock_client(handler))

    rc = main.run("What port?", config_path=APP_ROOT / "config.toml")
    captured = capsys.readouterr()
    assert rc == 0, captured.err
    assert "Port 8420." in captured.out
    # ingest embedded both sample docs + query embedded once
    assert len(embed_calls) >= 2
    assert len(chat_calls) == 1
    # chat got a system prompt that includes retrieved context
    assert any(m["role"] == "system" and "Context:" in m["content"]
               for m in chat_calls[0]["messages"])


def test_no_documents_returns_clean_exit_code(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path,
    capsys: pytest.CaptureFixture[str],
) -> None:
    def handler(_: httpx.Request) -> httpx.Response:
        return httpx.Response(404, json={"error": {
            "code": "MAI-N", "message": "?", "type": "internal_error",
        }})

    main = _load_main()
    monkeypatch.setattr(main, "_make_client", lambda _cfg: _mock_client(handler))

    cfg = tmp_path / "cfg.toml"
    cfg.write_text(
        f'[ingest]\ndocs_dir = "{tmp_path.as_posix()}"\nembed_model = "e"\n'
        '[generation]\nchat_model = "c"\n',
    )

    rc = main.run("x", config_path=cfg)
    err = capsys.readouterr().err
    assert rc == 3
    assert "no documents" in err
