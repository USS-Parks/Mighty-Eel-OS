#!/usr/bin/env bash
# e2e_inference.sh - End-to-end inference path verification
#
# Validates that the MAI API server returns real model
# output (not placeholder content) for all inference endpoints.
#
# Prerequisites:
#   - MAI API server running on localhost:3000
#   - At least one adapter configured and started (e.g., Ollama with llama3)
#
# Usage:
#   ./tests/e2e_inference.sh [BASE_URL]
#
# Exit codes:
#   0 = all tests passed
#   1 = one or more tests failed

set -euo pipefail

BASE_URL="${1:-http://localhost:3000}"
PASS=0
FAIL=0

red()   { printf '\033[0;31m%s\033[0m\n' "$*"; }
green() { printf '\033[0;32m%s\033[0m\n' "$*"; }
bold()  { printf '\033[1m%s\033[0m\n' "$*"; }

check() {
    local name="$1"
    local condition="$2"
    if eval "$condition"; then
        green "  PASS: $name"
        PASS=$((PASS + 1))
    else
        red "  FAIL: $name"
        FAIL=$((FAIL + 1))
    fi
}

# ─── Test 1: Chat Completion (non-streaming) ──────────────────────

bold "Test 1: Chat Completion"
CHAT_RESP=$(curl -s -w '\n%{http_code}' \
    -X POST "${BASE_URL}/v1/chat/completions" \
    -H "Content-Type: application/json" \
    -H "X-Profile-Id: test-admin" \
    -H "X-Profile-Role: admin" \
    -d '{
        "model": "llama3",
        "messages": [{"role": "user", "content": "Say hello in one word."}],
        "max_tokens": 32,
        "temperature": 0.1
    }')

CHAT_HTTP=$(echo "$CHAT_RESP" | tail -1)
CHAT_BODY=$(echo "$CHAT_RESP" | sed '$d')

check "HTTP 200" "[ '$CHAT_HTTP' = '200' ]"
check "Has choices array" "echo '$CHAT_BODY' | jq -e '.choices' >/dev/null 2>&1"
check "Content is non-empty" "[ \$(echo '$CHAT_BODY' | jq -r '.choices[0].message.content' | wc -c) -gt 1 ]"
check "Has usage.completion_tokens > 0" "[ \$(echo '$CHAT_BODY' | jq '.usage.completion_tokens') -gt 0 ]"
check "finish_reason is stop" "[ \$(echo '$CHAT_BODY' | jq -r '.choices[0].finish_reason') = 'stop' ]"

# ─── Test 2: Embeddings ──────────────────────────────────────────

bold "Test 2: Embeddings"
EMBED_RESP=$(curl -s -w '\n%{http_code}' \
    -X POST "${BASE_URL}/v1/embeddings" \
    -H "Content-Type: application/json" \
    -H "X-Profile-Id: test-admin" \
    -H "X-Profile-Role: admin" \
    -d '{
        "model": "nomic-embed-text",
        "input": "The quick brown fox jumps over the lazy dog."
    }')

EMBED_HTTP=$(echo "$EMBED_RESP" | tail -1)
EMBED_BODY=$(echo "$EMBED_RESP" | sed '$d')

check "HTTP 200" "[ '$EMBED_HTTP' = '200' ]"
check "Has data array" "echo '$EMBED_BODY' | jq -e '.data' >/dev/null 2>&1"
check "Embedding vector is non-empty" "[ \$(echo '$EMBED_BODY' | jq '.data[0].embedding | length') -gt 0 ]"
check "Vector values are floats" "echo '$EMBED_BODY' | jq -e '.data[0].embedding[0] | type == \"number\"' >/dev/null 2>&1"

# ─── Test 3: Chat Completion (streaming SSE) ─────────────────────

bold "Test 3: SSE Streaming"
SSE_RESP=$(curl -s -N --max-time 30 \
    -X POST "${BASE_URL}/v1/chat/completions" \
    -H "Content-Type: application/json" \
    -H "X-Profile-Id: test-admin" \
    -H "X-Profile-Role: admin" \
    -d '{
        "model": "llama3",
        "messages": [{"role": "user", "content": "Count to 3."}],
        "stream": true,
        "max_tokens": 64,
        "temperature": 0.1
    }')

check "Has data: prefix events" "echo '$SSE_RESP' | grep -q '^data: '"
check "Has [DONE] sentinel" "echo '$SSE_RESP' | grep -q '\\[DONE\\]'"
check "Has chat.completion.chunk object" "echo '$SSE_RESP' | grep -q 'chat.completion.chunk'"
check "Has id: sequence numbers" "echo '$SSE_RESP' | grep -q '^id: '"

# ─── Test 4: Model Alias Resolution ──────────────────────────────

bold "Test 4: Model Alias Resolution"
ALIAS_RESP=$(curl -s -w '\n%{http_code}' \
    -X POST "${BASE_URL}/v1/chat/completions" \
    -H "Content-Type: application/json" \
    -H "X-Profile-Id: test-admin" \
    -H "X-Profile-Role: admin" \
    -d '{
        "model": "lamprey/fast",
        "messages": [{"role": "user", "content": "Hi"}],
        "max_tokens": 8,
        "temperature": 0.1
    }')

ALIAS_HTTP=$(echo "$ALIAS_RESP" | tail -1)
ALIAS_BODY=$(echo "$ALIAS_RESP" | sed '$d')

check "HTTP 200 for alias" "[ '$ALIAS_HTTP' = '200' ]"
check "Resolved to backend model" "echo '$ALIAS_BODY' | jq -e '.model' >/dev/null 2>&1"

# ─── Test 5: Error Cases ─────────────────────────────────────────

bold "Test 5: Error Handling"
ERR_RESP=$(curl -s -w '\n%{http_code}' \
    -X POST "${BASE_URL}/v1/chat/completions" \
    -H "Content-Type: application/json" \
    -H "X-Profile-Id: test-admin" \
    -H "X-Profile-Role: admin" \
    -d '{"model": "nonexistent-model-xyz", "messages": [{"role": "user", "content": "test"}]}')

ERR_HTTP=$(echo "$ERR_RESP" | tail -1)
ERR_BODY=$(echo "$ERR_RESP" | sed '$d')

check "Non-200 for bad model" "[ '$ERR_HTTP' != '200' ]"
check "Has error.code field" "echo '$ERR_BODY' | jq -e '.error.code' >/dev/null 2>&1"
check "Error code is MAI-prefixed" "echo '$ERR_BODY' | jq -r '.error.code' | grep -q '^MAI-'"
check "No backend names in error" "! echo '$ERR_BODY' | grep -qi 'ollama\\|vllm\\|llama.cpp'"

# ─── Summary ─────────────────────────────────────────────────────

echo ""
bold "Results: $PASS passed, $FAIL failed"

if [ "$FAIL" -gt 0 ]; then
    red "SOME TESTS FAILED"
    exit 1
else
    green "ALL TESTS PASSED"
    exit 0
fi
