"""MAI SDK Integration Tests.

These tests run the Python SDK against a live MAI server. They verify
that every SDK method reaches the correct endpoint, streaming works
end-to-end, and auth/rate-limiting behave correctly.

Prerequisites:
    - MAI server running on localhost:8420
    - At least one model loaded (or use test adapter)
    - Admin API key available as MAI_TEST_API_KEY env var
    - A known-invalid key for rejection tests

Usage:
    MAI_TEST_API_KEY=im-... MAI_TEST_MODEL=qwen3-14b:Q4_K_M pytest tests/sdk_integration.py -v

Environment Variables:
    MAI_TEST_API_KEY    - Valid admin API key for authenticated tests
    MAI_TEST_BASE_URL   - Server base URL (default: http://localhost:8420/v1)
    MAI_TEST_MODEL      - Model ID to use for inference tests (default: test-model)
"""

from __future__ import annotations

import os
import time

import pytest
from mai.client import AsyncMaiClient, MaiClient, MaiClientConfig
from mai.types import ChatMessage, MaiError

# ---------------------------------------------------------------------------
# Fixtures
# ---------------------------------------------------------------------------

BASE_URL = os.environ.get("MAI_TEST_BASE_URL", "http://localhost:8420/v1")
API_KEY = os.environ.get("MAI_TEST_API_KEY", "")
TEST_MODEL = os.environ.get("MAI_TEST_MODEL", "test-model")
BAD_API_KEY = os.environ.get("MAI_TEST_BAD_API_KEY", "invalid-key")


@pytest.fixture
def client() -> MaiClient:
    """Sync client with valid auth."""
    cfg = MaiClientConfig(
        base_url=BASE_URL,
        api_key=API_KEY,
        max_retries=0,  # No retries in tests; fail fast
    )
    c = MaiClient(cfg)
    yield c
    c.close()


@pytest.fixture
def unauthenticated_client() -> MaiClient:
    """Sync client with NO auth token."""
    cfg = MaiClientConfig(
        base_url=BASE_URL,
        api_key=None,
        max_retries=0,
    )
    c = MaiClient(cfg)
    yield c
    c.close()


@pytest.fixture
def bad_token_client() -> MaiClient:
    """Sync client with an invalid auth token."""
    cfg = MaiClientConfig(
        base_url=BASE_URL,
        api_key=BAD_API_KEY,
        max_retries=0,
    )
    c = MaiClient(cfg)
    yield c
    c.close()


# ---------------------------------------------------------------------------
# 1. Chat Completion (non-streaming)
# ---------------------------------------------------------------------------

class TestChatCompletion:
    """Test non-streaming chat completions via the SDK."""

    def test_basic_chat(self, client: MaiClient) -> None:
        """SDK chat() reaches /v1/chat/completions and returns a valid response."""
        messages = [ChatMessage(role="user", content="Say hello")]
        response = client.chat(TEST_MODEL, messages, max_tokens=32)

        assert response.id, "Response must have an ID"
        assert response.model, "Response must include model name"
        assert len(response.choices) >= 1, "Must have at least one choice"
        assert response.usage is not None, "Usage must be populated"
        assert response.usage.total_tokens > 0

    def test_chat_with_system_message(self, client: MaiClient) -> None:
        """Chat with system + user message pair."""
        messages = [
            ChatMessage(role="system", content="You are a test assistant."),
            ChatMessage(role="user", content="What are you?"),
        ]
        response = client.chat(TEST_MODEL, messages, max_tokens=64)
        assert len(response.choices) >= 1

    def test_completions_endpoint(self, client: MaiClient) -> None:
        """SDK complete() reaches /v1/completions (route alias)."""
        response = client.complete(TEST_MODEL, "The meaning of life is", max_tokens=32)
        assert response.id
        assert len(response.choices) >= 1


# ---------------------------------------------------------------------------
# 2. Chat Completion (streaming)
# ---------------------------------------------------------------------------

class TestStreamingChat:
    """Test SSE streaming chat completions via the SDK."""

    def test_chat_stream_yields_chunks(self, client: MaiClient) -> None:
        """chat_stream() yields ChatCompletionChunk objects via SSE."""
        messages = [ChatMessage(role="user", content="Count to 3")]
        chunks = list(client.chat_stream(TEST_MODEL, messages, max_tokens=64))

        assert len(chunks) > 0, "Must yield at least one chunk"
        for chunk in chunks:
            assert chunk.id, "Each chunk must have an ID"
            assert chunk.model, "Each chunk must include model"

    def test_stream_completions_convenience(self, client: MaiClient) -> None:
        """stream_completions() wraps chat_stream for text prompts."""
        chunks = list(client.stream_completions(TEST_MODEL, "Hello", max_tokens=16))
        assert len(chunks) > 0


# ---------------------------------------------------------------------------
# 3. Embeddings
# ---------------------------------------------------------------------------

class TestEmbeddings:
    """Test embedding generation via the SDK."""

    def test_single_embedding(self, client: MaiClient) -> None:
        """embed() returns embedding vectors for a single string."""
        response = client.embed(TEST_MODEL, "test input")
        assert response.data, "Must return embedding data"
        assert len(response.data) >= 1

    def test_batch_embedding(self, client: MaiClient) -> None:
        """embed() with a list of strings returns multiple embeddings."""
        response = client.embed(TEST_MODEL, ["hello", "world"])
        assert len(response.data) >= 2


# ---------------------------------------------------------------------------
# 4. Model List
# ---------------------------------------------------------------------------

class TestModelList:
    """Test model listing via the SDK."""

    def test_list_models(self, client: MaiClient) -> None:
        """list_models() reaches /v1/models and returns a list."""
        models = client.list_models()
        # Even with no models loaded, we should get an empty list (not an error)
        assert isinstance(models, list)

    def test_get_model_not_found(self, client: MaiClient) -> None:
        """get_model() with a nonexistent ID raises MaiError."""
        with pytest.raises(MaiError) as exc_info:
            client.get_model("nonexistent-model-id-xyz")
        assert "MAI-2001" in str(exc_info.value) or "not found" in str(exc_info.value).lower()


# ---------------------------------------------------------------------------
# 5. Health Check
# ---------------------------------------------------------------------------

class TestHealthCheck:
    """Test health endpoints via the SDK."""

    def test_health_no_auth(self) -> None:
        """Health endpoint works without authentication."""
        cfg = MaiClientConfig(base_url=BASE_URL, api_key=None, max_retries=0)
        c = MaiClient(cfg)
        try:
            response = c.health()
            assert response.status in {"healthy", "degraded", "unhealthy"}
        finally:
            c.close()

    def test_health_check_convenience(self, client: MaiClient) -> None:
        """health_check() returns True when server is reachable."""
        assert client.health_check() is True

    def test_health_check_unreachable(self) -> None:
        """health_check() returns False for unreachable server."""
        cfg = MaiClientConfig(
            base_url="http://localhost:1/v1",  # Almost certainly not running
            timeout=1.0,
            max_retries=0,
        )
        c = MaiClient(cfg)
        try:
            assert c.health_check() is False
        finally:
            c.close()


# ---------------------------------------------------------------------------
# 6. Auth Rejection
# ---------------------------------------------------------------------------

class TestAuthRejection:
    """Test that auth correctly rejects bad/missing tokens."""

    def test_missing_token_rejected(self, unauthenticated_client: MaiClient) -> None:
        """Request without API key returns 401 (when internal header disabled)."""
        with pytest.raises(MaiError) as exc_info:
            unauthenticated_client.list_models()
        err = exc_info.value
        assert err.code in ("MAI-4002", "MAI-4004"), f"Expected auth error, got {err.code}"

    def test_invalid_token_rejected(self, bad_token_client: MaiClient) -> None:
        """Request with wrong API key returns 401."""
        with pytest.raises(MaiError) as exc_info:
            bad_token_client.list_models()
        err = exc_info.value
        assert err.code == "MAI-4004", f"Expected TokenInvalid, got {err.code}"

    def test_invalid_token_on_chat(self, bad_token_client: MaiClient) -> None:
        """Inference with wrong API key is rejected."""
        messages = [ChatMessage(role="user", content="hello")]
        with pytest.raises(MaiError) as exc_info:
            bad_token_client.chat(TEST_MODEL, messages)
        assert exc_info.value.code in ("MAI-4002", "MAI-4004")


# ---------------------------------------------------------------------------
# 7. Rate Limiting
# ---------------------------------------------------------------------------

class TestRateLimiting:
    """Test per-key rate limiting.

    The default rate limit is 60 requests/minute. These tests use a
    tight burst to trigger the limiter. Adjust MAI_TEST_RATE_LIMIT
    if the server is configured differently.
    """

    @pytest.mark.slow
    def test_rate_limit_triggers(self) -> None:
        """Burst requests beyond the rate limit return 429 with retry_after."""
        # Create a client with a key that will hit the limit
        cfg = MaiClientConfig(
            base_url=BASE_URL,
            api_key=API_KEY,
            max_retries=0,  # Don't retry, we want to see the 429
        )
        c = MaiClient(cfg)

        rate_limited = False
        try:
            # Send requests until we hit the limit.
            # Default is 60/min, so 65 requests should trigger it.
            for _i in range(70):
                try:
                    c.health()  # health is exempt from auth but...
                    # Use an authed endpoint instead
                    c.list_models()
                except MaiError as e:
                    if e.code == "MAI-4005":
                        rate_limited = True
                        assert e.retry_after is not None, "Rate limit must include retry_after"
                        assert e.retry_after > 0, "retry_after must be positive"
                        break
        finally:
            c.close()

        assert rate_limited, (
            "Expected rate limiting after burst requests. "
            "If the server rate limit is higher than 70/min, adjust this test."
        )

    @pytest.mark.slow
    def test_rate_limit_recovery(self) -> None:
        """After rate limit, waiting retry_after seconds allows requests again."""
        cfg = MaiClientConfig(
            base_url=BASE_URL,
            api_key=API_KEY,
            max_retries=0,
        )
        c = MaiClient(cfg)

        try:
            # First, trigger rate limit
            retry_after = None
            for _ in range(70):
                try:
                    c.list_models()
                except MaiError as e:
                    if e.code == "MAI-4005":
                        retry_after = e.retry_after
                        break

            if retry_after is None:
                pytest.skip("Could not trigger rate limit (server limit may be high)")

            # Wait the specified duration (cap at 5s for test speed)
            wait_time = min(retry_after + 1, 5)
            time.sleep(wait_time)

            # Should succeed again
            models = c.list_models()
            assert isinstance(models, list)
        finally:
            c.close()


# ---------------------------------------------------------------------------
# Async tests (mirror the sync tests for core paths)
# ---------------------------------------------------------------------------

@pytest.mark.asyncio
class TestAsyncClient:
    """Verify the async client works for core operations."""

    async def test_async_chat(self) -> None:
        """Async chat() works end-to-end."""
        cfg = MaiClientConfig(base_url=BASE_URL, api_key=API_KEY, max_retries=0)
        async with AsyncMaiClient(cfg) as client:
            messages = [ChatMessage(role="user", content="Hello")]
            response = await client.chat(TEST_MODEL, messages, max_tokens=16)
            assert response.id
            assert len(response.choices) >= 1

    async def test_async_health_check(self) -> None:
        """Async health_check() returns True."""
        cfg = MaiClientConfig(base_url=BASE_URL, api_key=API_KEY)
        async with AsyncMaiClient(cfg) as client:
            result = await client.health_check()
            assert result is True

    async def test_async_streaming(self) -> None:
        """Async chat_stream() yields chunks."""
        cfg = MaiClientConfig(base_url=BASE_URL, api_key=API_KEY, max_retries=0)
        async with AsyncMaiClient(cfg) as client:
            messages = [ChatMessage(role="user", content="Count to 2")]
            chunks = []
            async for chunk in client.chat_stream(TEST_MODEL, messages, max_tokens=32):
                chunks.append(chunk)
            assert len(chunks) > 0
