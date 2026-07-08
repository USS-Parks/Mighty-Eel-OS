# Full-Repository Security & Integrity Audit — `im-mighty-eel-mai`

**Date:** 2026-07-08 · **Auditor:** agentic review (8 delegated read-only reviewers +
objective gates), every Critical/High re-verified against source by the lead.
**Revision audited:** `main` @ `700cf2b` (post Phase F/X remediation).
**Method:** whole-tree fan-out by subsystem (AOG control plane, AOG edge/scheduler,
fabric-* crypto, WSF internals, MAI runtime, Python/tools/deploy/CI) + two whole-tree
broader-lens sweeps (AI-slop/hygiene, edge-case/panic-safety); ~504 Rust files + 248
Python across ~50 crates. Prior AF-001..007 and Phase F findings were treated as out of
scope (re-confirmed intact, not re-reported).

## Bottom line

The WSF trust plane and the `mai-api` REST/gRPC surface hardened in Phase F/X **held up
under independent re-audit** (real PQC, fail-closed verifies, AF-03 caller-role refusal
intact). New, serious issues cluster in five areas the remediation never touched:
(1) the AOG/"Loom" orchestration tier — two verified Criticals; (2) two core-crypto
attenuation/revocation misses; (3) the vault's "sovereign key custody" + audit
tamper-evidence being partly disguised-incomplete; (4) audit-chain verification
fail-opens; (5) reachable DoS/OOM/panic. Plus systemic CANON §11 roster-code slop that
slipped past the mechanical scanner.

## Objective health (all green)

Workspace `cargo clippy -D warnings`, `ruff`, `cargo audit` (0 vulns / 518 deps),
`cargo deny`, `gitleaks`, `detect-secrets`, and the full-tree no-slop scan all pass. No
`panic = "abort"`. Default 2 MB HTTP body cap intact.

## CRITICAL (verified in code)

- **C1 — `aogd` unauthenticated `/admin/*` on the production socket.** `crates/aogd/src/admin.rs:46-54` mounts `/admin/write|get|change-membership|add-learner` with no auth layer; `src/lib.rs:198-206` merges it onto the same socket as the authenticated `/apis/**`. `write` commits a caller-chosen `Op{key,value,Precondition::Any}` straight into the Raft state machine — no token, no admission, no receipt. Forge a `Capability`, plant a plaintext `TrustRing` key, delete a `RevocationIntent` (reverse a kill), or hijack quorum.
- **C2 — Raft transport + admin served in plaintext, unauthenticated; mTLS is dead code.** `crates/aogd/src/main.rs:20-29` binds a plain `TcpListener` + `axum::serve`; `aog-wire/src/tls.rs::NodeTls` (mutually-authenticated rustls, "VH5b") is never wired into the serve path. Forged `vote`/`append-entries`/`install-snapshot` = consensus takeover. Team-defers ("VH5b") but ships live.

## HIGH (verified in code)

**Trust-plane crypto (Phase-T/R primitives):**
- **H1 — Attenuation monotonicity bypass via empty model list.** `fabric-token/src/lib.rs:541-552`: `allowed_models: Some(vec![])` passes the subset test vacuously; `allows_model` (`wsf-api/src/policy.rs:86`) treats empty as unrestricted = all models. A restricted parent mints an all-models child.
- **H2 — Signing-key revocation bypass.** `fabric-token/src/lib.rs:91-96` strips the whole `signature` object before hashing, so `signature.key_id` is unsigned; `fabric-revocation` matches `is_key_revoked` on it while verifiers use a fixed anchor. Rewrite `key_id` → evade the signing-key kill dimension.

**AOG authorization:**
- **H3 — Delete path skips authorization.** `aog-apiserver/src/admission.rs:168-173` runs policy only `if let Some(object)`; `handlers.rs:160-165` builds deletes with `object: None`. Any authenticated principal deletes any object, incl. `RevocationIntent` (kill reversal).
- **H4 — "Attested placement" trusts a self-declared, unverified attestation string.** `aog-scheduler/src/filters.rs` gates on node-supplied `attestation`; `AttestationProfile` carries a bare `pcr: Option<String>` with no quote/AK-cert/nonce; no code verifies a hardware quote. `aog-node/src/attest.rs` documents an eviction/revoke loop nothing calls. Team-deferred (VH5) but no data path for a real quote exists.

**Vault — disguised incompleteness (contradicts shipped security docs):**
- **H5 — Master signing key never TPM-sealed.** `mai-vault/src/init.rs:93-106` signs a fixed challenge and seals *that signature*, not the key; no production branch.
- **H6 — Vault audit hash covers 5 of 13 fields.** `mai-vault/src/audit.rs:139-154` hashes only `previous_hash,timestamp,profile_id,action,status`; `model_id`/tokens/`error_code`/`ip_source` are editable undetected in a "tamper-evident" HIPAA trail (doc also says SHA3 while code uses BLAKE3).

**Audit-chain verification fail-open (`mai-compliance`):**
- **H7 — `verify_chain` skips entries with a stripped signature** (`audit/chain.rs:329`) while `previous_hash` links are keyless BLAKE3; with the default `NullSigner`, a rewritten log still verifies `Ok`.
- **H8 — `verify_full` checks only the in-memory tail, not the persisted WAL** (`audit/api.rs:308`); evicted entries are untamper-checkable and >8192 entries returns `HeadHashNonZero` on a clean log.

**Reachable DoS / OOM:**
- **H9 — Unauthenticated `/v1/status` runs O(n) full-chain verify under the global receipt lock** (`aog-gateway/src/surface_openai.rs:310-314`).
- **H10 — STT `AudioBuffer` unbounded** (`mai-agent/src/stt.rs:78-114`): `frame_duration_ms` integer-divides to 0 for sub-16-sample frames, so the duration cap never trips; WS audio OOMs the host.
- **H11 — Provider HTTP clients have no timeout** (`aog-gateway/src/provider/{openai,anthropic}.rs`); a hung backend piles up tasks forever.

## MEDIUM (grounded)

- Compliance/routing fail-open: composer returns allow on empty/filtered module set → disabling the HIPAA module sends PHI to cloud in Enforce mode (`mai-compliance/src/policy/composer.rs:396`); router ignores `upstream_flags` and drops `EntityKind::Medical` (`mai-router/src/router.rs`); classifier fails open on empty config (`classifier.rs`).
- Fail-open under tamper/misconfig: wsf-cache treats a pre-epoch clock as fresh → cloud egress (`wsf-cache/src/lib.rs:132`); wsf-api silently drops to `LocalDevAuthenticator` (authorizes any request) with no production guard (`wsf-api/src/main.rs:108`, `wsf-hardening` guard uncalled).
- Resource/lifecycle: streaming bypasses budget metering (`surface_openai.rs:194`); SSE releases the scheduler slot on a 300s timer (`streaming/sse.rs:290`); toolproxy `task_usage` unbounded + no execution timeout (`aog-toolproxy/src/lib.rs`); circuit breaker `powf`→inf→`from_secs_f64` panics on sustained outage (`mai-core/src/circuit_breaker.rs:248`).
- Systemic latent panic: `.lock().unwrap()/.expect()` on request hot paths (no `panic=abort`; one panic-under-lock poisons the service) across rate_limit/metrics/gateway/apiserver/wsf-*/router.
- AOG consistency/durability: minority-partition + front-door revocation serve unfenced local reads (`confirm_leadership` called only in tests); K9 receipt ledger in-process/per-node/non-durable, admin writes unrecorded.
- Latent auth/integrity: WS `auth.handshake` trusts client-declared role (`mai-api/src/streaming/ws.rs:444`, not reachable today — WS ops stubbed); approval inbox keyed on caller `call_id` (review-vs-authorize mismatch); compliance decision-cache key omits ITAR `ActorContext`.
- Canned-success endpoints (wired, return indistinguishable success, TODO(basho)-owned): WS inference `inference_complete(0 tokens)` (`ws.rs:571`); gRPC embeddings returns `Vec::new()` with token usage; `list_profiles` "admin sees all" returns only requester.
- Dead/misdocumented: `mai-hil` secure-load (`unseal_tpm_key`/`decrypt_and_verify`) returns `NotImplemented` on all 3 drivers; `mai-hil` crate-wide `#![allow(unused_variables, dead_code, missing_docs)]`; `manager.rs:588 adapter_in_flight` dead; `health.rs:484 subscribe()` returns a count not a receiver; vault `vectors.rs` backup/restore no-op stubs.

## LOW / hygiene / slop

- **CANON §11 roster-code slop (systemic):** ~100 `K#/H#/N#/S#/R#/VH5b/SHIP-##` step-codes across ~82 shipped `src/` files (worst: `aog-scheduler/src/filters.rs:1-3` narrates future roster work); `mai-admin --help` leaks "SHIP-09/SHIP-10/Pending session". These **passed the mechanical no-slop scanner** (its PROV pattern excludes these short codes) — a scanner gap.
- **Dangling references:** ~20 doc-path-drift comments (docs moved into `docs/<category>/`); `audit/entry.rs:122` cites a non-existent `BUILD-EXECUTION-PLAN-V2-UPDATED.md`; `fabric-proof/src/bundle.rs:75` cites "(finding F6-N7)" — an audit-internal ID introduced by the Phase-F remediation itself, resolving to nothing in-tree.
- **Misc:** SHA3-vs-BLAKE3 doc mismatch (`vault/audit.rs:140`); de-id `{idx}` template token never substituted (`deid.rs`); ML-DSA secret key + seed never zeroized (`fabric-crypto`, no `Zeroize`); `AcceptAllBundleVerifier` `pub`/re-exported (mitigated in mai-api by `production_guard`); operator-config integer-overflow panics (retention math, broker `clamp`, grpc pagination); heuristic stubs (`requires_vision=false`, `output_tokens=input/2`, `hotswap` gpu_id-as-adapter_id).

## Calibration — genuinely sound

`fabric-*`/`wsf-*` crypto + verify paths (real ML-DSA/ML-KEM/AES-GCM, fail-closed,
consistent canonical-JSON→BLAKE3→sign, no manual tag compares); `aog-federation`
(signature-verify + anti-rollback before apply); `aog-store` CAS (deterministic,
fail-closed); `mai-api` HTTP+gRPC auth (AF-03 holds); gateway fail-closed Enforce
default; the `mai-compliance` audit chain (full-canonical) — the correct model the vault
one should converge to.

## Severity note

The AOG/"Loom" tier is newer than the remediated WSF appliance; C2 (mTLS), H4
(attestation), the durable-receipt and TPM-key-sealing gaps are explicitly phase-gated
by code comments (VH5b/VH5/Phase-W) — but several ship with present-tense "live" docs
over stubbed bodies, and C1 (unauth admin), H3 (delete authz), and the crypto/audit
findings are concrete reachable defects, not deferrals.