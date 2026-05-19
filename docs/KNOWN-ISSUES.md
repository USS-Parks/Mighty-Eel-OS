# MAI Known Issues

**Project:** Island Mountain Model Abstraction Interface (MAI)
**Last Updated:** 2026-05-18

---

## Active Issues

### 1. No Rust Toolchain in Cowork Sandbox

**Severity:** Low (development workflow only)
**Affects:** Sessions 11a-11e, all future sessions
**Status:** Workaround in place

The Cowork sandbox does not include a Rust toolchain. `cargo check`, `cargo clippy`, and `cargo fmt` cannot run in-session. All Rust code is verified by manual cross-reference (imports, types, field names) during audit passes. Full compilation verification must be done locally after each session.

**Action:** Run `cargo check --workspace && cargo clippy --workspace && cargo fmt --all` locally after every session.

### 2. cargo fmt Drift

**Severity:** Low (cosmetic)
**Affects:** All Rust files
**Status:** Open

Formatting drift has accumulated across sessions. No functional impact but `cargo fmt` will produce diffs when run locally.

**Action:** Run `cargo fmt --all` once before Session 12.

### 3. Sglang Adapter self._raw_config Reference

**Severity:** Medium (runtime crash on initialize())
**Affects:** `adapters/sglang/adapter.py`
**Status:** Open since Session 10 CI fix (2026-05-17)

The Sglang adapter references `self._raw_config` in its `initialize()` method, but `AdapterBase` stores config as `self._config`. Will raise `AttributeError` when `initialize()` is called.

**Action:** Change `self._raw_config` to `self._config` in `adapters/sglang/adapter.py`.

### 4. StubVault in Server Bootstrap

**Severity:** Expected (placeholder)
**Affects:** `mai-api/src/server.rs`
**Status:** RESOLVED (Session 12, 2026-05-18)

The server uses a `StubVault` that returns `ModelNotFound` for all weight loads and `Ok(true)` for all signature verifications. This is intentional for Session 11e. Real ZfsVault now available in mai-vault crate.

### 5. Placeholder Token Producers in Streaming

**Severity:** Expected (placeholder)
**Affects:** `mai-api/src/streaming/sse.rs`, `mai-api/src/handlers/inference.rs`
**Status:** By design, resolved when adapter IPC pipeline is wired

Streaming handlers use simulated token producers that generate placeholder tokens. Real adapter output requires the JSON-RPC IPC bridge (Session 08) to be wired into the streaming channel. Full integration deferred until adapter processes are managed by the server.

### 6. Registry scan_models Placeholder

**Severity:** Expected (placeholder)
**Affects:** `mai-api/src/grpc/registry.rs`
**Status:** By design, resolved in Session 15

`ModelRegistry` has no `scan_models()` method. The gRPC `ScanModels` RPC returns an empty list. Session 15 (Model Management) adds the real model scanning and discovery pipeline.

---

## Deferred Items (Out of Scope)

These items are explicitly excluded from the current build. See PROJECT.md for scope boundaries.

- L6 UI (React dashboard, onboarding wizard)
- Remote support tunnel (network service, not MAI)
- Landfall Council (multi-user chat variant)
- Full L4 agent logic (RAG pipeline internals, tool implementations)
- Full L5 application logic (only scaffolds built in Session 16)
- TetraMem adapter implementation (stub interface only via HIL)
- Photonic adapter implementation (stub interface only via HIL)
- Audio/STT binary frame processing (acknowledged in WebSocket, deferred to Session 13)
- Tool calling execution (acknowledged in WebSocket, deferred to Session 13)

---

## Resolved Issues (Historical)

### Session 03 Audit: FFI Blocking Issues (RESOLVED)

Three blocking FFI issues in the Backend Adapter Framework spec. All fixed during Session 03 audit. See SESSION-LOG-ARCHIVE-01.md for details.

### Session 10 CI: pytest Collection Failures (RESOLVED)

Missing `adapters/__init__.py` and AdapterBase constructor signature mismatch. Fixed 2026-05-17. See SESSION-LOG.md maintenance log.

### Session 11d: Invented mai-core APIs (RESOLVED)

All 6 gRPC service files initially coded against non-existent APIs. All rewritten from scratch against verified interfaces during audit. See SESSION-LOG.md Session 11d notes.

### Session 11e: Proto Message Type Mismatches (RESOLVED)

Integration tests used `LoadModelRequest` (doesn't exist), empty `ListModelsRequest` (has profile_id field), ChatMessage with `tool_calls`/`tool_call_id` (proto only has role/content/name). All fixed during Audit Pass 1.

---

*Document derived from MAI-BUILD-PROMPT-ROSTER.md | 2026-05-15 | Island Mountain AI | Confidential*
