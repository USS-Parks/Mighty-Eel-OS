"""Root conftest — anchors pytest rootdir at mai/ so adapter imports resolve.

Also registers the `live_backend` mark and the per-backend availability
fixtures used by `adapters/*/tests/test_integration_live.py`. The marks
and fixtures are SAFE TO IMPORT — they no-op (and skip cleanly) when
the corresponding backend is not reachable, so the default `pytest`
command still works on a machine with no LLM backends installed.

Added under DOUGHERTY J-06 (Ollama live tests are the first consumer).
"""

from __future__ import annotations

import json
import os
import shutil
import tempfile
import urllib.error
import urllib.request
import uuid
from pathlib import Path
from typing import Any

import pytest

REPO_ROOT = Path(__file__).resolve().parent


def _configure_workspace_temp_root() -> None:
    """Keep pytest scratch space out of ACL-hostile system temp roots."""
    default_root = REPO_ROOT / ".tmp" / f"pytest-{uuid.uuid4().hex}"
    root = Path(os.environ.get("MAI_PYTEST_TMP", default_root))
    root.mkdir(parents=True, exist_ok=True)
    os.environ["TMP"] = str(root)
    os.environ["TEMP"] = str(root)
    tempfile.tempdir = str(root)


_configure_workspace_temp_root()


@pytest.fixture
def tmp_path() -> Path:
    """Cross-platform writable scratch directory for Python tests."""
    root = Path(tempfile.gettempdir())
    root.mkdir(parents=True, exist_ok=True)
    path = root / f"mai-test-{uuid.uuid4().hex}"
    path.mkdir()
    try:
        yield path
    finally:
        shutil.rmtree(path, ignore_errors=True)

# ─── Marks ──────────────────────────────────────────────────────────────────


def pytest_configure(config: pytest.Config) -> None:
    """Register the live_backend mark so pytest does not warn about it.

    Tests marked `@pytest.mark.live_backend` only run when a real backend
    is reachable; the per-backend availability fixture handles the skip.
    """
    config.addinivalue_line(
        "markers",
        "live_backend: opt-in test that hits a real LLM backend; "
        "skips cleanly when the backend env var is unset or the "
        "backend is unreachable.",
    )


# ─── Ollama availability ────────────────────────────────────────────────────


def _http_get_json(url: str, timeout_s: float) -> dict[str, Any] | None:
    """Single-shot GET that returns parsed JSON or None on any failure.

    Stdlib-only (no httpx) to keep this fixture air-gap-policy compliant
    per `docs/ADAPTER-COMPLETION-MATRIX.md` §1.
    """
    try:
        with urllib.request.urlopen(url, timeout=timeout_s) as resp:
            if resp.status != 200:
                return None
            return json.loads(resp.read().decode("utf-8"))
    except (urllib.error.URLError, TimeoutError, json.JSONDecodeError, OSError):
        return None


@pytest.fixture(scope="session")
def ollama_available() -> dict[str, Any] | None:
    """Session-scoped check for a reachable Ollama backend.

    Returns a dict with `host` (str), `models` (list[str]), and `model`
    (str — a model that is actually pulled, suitable for test use) when
    Ollama is reachable; returns None otherwise.

    Tests use this fixture to skip cleanly when the backend is not
    available — see `adapters/ollama/tests/test_integration_live.py`.

    Honoured env vars:
      OLLAMA_HOST       — base URL of the Ollama server (e.g.
                          `http://127.0.0.1:11434`). When unset, this
                          fixture returns None and live tests skip.
      OLLAMA_LIVE_MODEL — specific model to use in tests. When unset,
                          the first model returned by `/api/tags` wins.
    """
    host = os.environ.get("OLLAMA_HOST")
    if not host:
        return None

    tags = _http_get_json(f"{host.rstrip('/')}/api/tags", timeout_s=2.0)
    if tags is None or not isinstance(tags.get("models"), list):
        return None

    models = [m.get("name", "") for m in tags["models"] if isinstance(m, dict)]
    models = [m for m in models if m and not m.endswith("-cloud")]
    if not models:
        return None

    preferred = os.environ.get("OLLAMA_LIVE_MODEL")
    model = preferred if preferred in models else models[0]

    return {"host": host, "models": models, "model": model}


@pytest.fixture(scope="session")
def ollama_embedding_model(ollama_available: dict[str, Any] | None) -> str | None:
    """Pick an embedding-capable model if one is pulled, else None.

    Heuristic: a model whose name contains `embed`, `bge`, `e5`, or
    `nomic` is treated as an embedding model. The default Ollama
    embedding model (`nomic-embed-text`) matches the last bucket. When
    no embedding model is pulled, the embedding live test skips.
    """
    if ollama_available is None:
        return None
    embed_hints = ("embed", "bge", "e5", "nomic")
    for name in ollama_available["models"]:
        if any(h in name.lower() for h in embed_hints):
            return name
    return None


# ─── llama.cpp availability ─────────────────────────────────────────────────


@pytest.fixture(scope="session")
def llamacpp_available() -> dict[str, Any] | None:
    """Session-scoped check for a reachable llama-server backend.

    Returns a dict with `host` (str) and optional `model` (str — the
    discovered model path or empty string) when llama-server is
    reachable; returns None otherwise. The adapter discovers the
    actual model via /props during initialize, so the fixture does
    not need to know the model name a priori.

    Tests use this fixture to skip cleanly when the backend is not
    available — see
    `adapters/llamacpp/tests/test_integration_live.py`.

    Honoured env vars:
      LLAMACPP_HOST — base URL of the llama-server (e.g.
                      `http://127.0.0.1:8081`). When unset, this
                      fixture returns None and live tests skip.

    Probe path: `GET /health`. llama-server returns a JSON object
    like `{"status": "ok"}` when ready. Anything else (including
    plain-text 503 from a still-loading server) is treated as
    unavailable.
    """
    host = os.environ.get("LLAMACPP_HOST")
    if not host:
        return None

    health = _http_get_json(f"{host.rstrip('/')}/health", timeout_s=2.0)
    if health is None or health.get("status") != "ok":
        return None

    return {"host": host, "model": ""}
