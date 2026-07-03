# Sovereignty Stack (WSF + AOG + Aeneas) — Session Handoff

**Written:** 2026-07-03, end of the first STS build session (context full).
**For:** the next session, to resume Stem-to-Stern execution with zero re-discovery.
**Read this, then read `docs/sessions/SOVEREIGNTY-DEVLOG.md` (every prompt logged) and the plan.**

---

## 0. TL;DR — where we are

- Building **two products** for Island Mountain's sales/CS division: **WSF** (Woven Sovereignty
  Fabric — trust plane, on OpenBao) + **AOG** (Agentic Orchestration Governance — control plane),
  with the live islandmountain.io/aeneas.html cloud-security console (build codename **Lamprey**) as
  WSF's first consumer. Built by **extending the existing Lamprey MAI Rust workspace** (`im-mighty-eel-mai`).
- **Mode:** STS (stem to stern). The user wants **relentless, continuous execution** — commit each
  prompt, verify each, **do NOT stop to ask permission**, **push only at the very end** after the full
  test suite/CI validate. The user is explicitly impatient with checkpoint-and-ask behavior.
- **DONE:** **Phase 0** (foundation + contracts + crypto + advisories + CI gate), **Phase F** (all 8
  fabric primitive crates), and **Phase W: W1 + W2** — 16 commits total. Both W-prompts were
  **genuinely live-verified** (real OpenBao Docker + real Moto STS), not mock-only.
  - **W1 `wsf-bridge`** (commit `3ead250`): OpenBao AppRole auth event + KV tenant read → ML-DSA-signed
    `fabric-token`; also policy-bundle + revocation signing + metadata-only audit correlation. Signs
    with **pure-Rust ML-DSA-87** (off-host/air-gap verifiable), NOT OpenBao Transit. Focused ~150-line
    OpenBao client adapted from `mai-api` (no mai-api dep → dodges the axum 0.7/0.8 conflict).
  - **W2 `wsf-broker`** (commit `f3e589f`): verify token → read broker root creds from OpenBao kv →
    STS `AssumeRole` with an inline session policy scoped to the token's `ResourcePrefix` caveats →
    scoped temp creds, duration = token TTL. **Hand-rolled AWS SigV4** over hmac/sha2 (no aws-sdk →
    no aws-lc-rs Windows-build risk), pinned to AWS's `aws4_testsuite` get-vanilla vector.
- **NEXT:** **Phase W, W3** (`wsf-seal`), then W4–W10 → G (AOG gateway) → T (tool governance) →
  C (console) → D (deploy/proof) → Z (ship/Bucket).

---

## 1. Resume protocol (do this first)

```
Worktree (do ALL work here):
  C:\Users\17076\Documents\Claude\Island Mountain\Island Mountain Mighty Eel OS\mai-worktrees\mai-SOV-1
Branch:  session/SOV-1   (HEAD = f3e589f)   — NOT pushed; push only at the very end
Toolchain: cargo 1.95.0 / rustc 1.95.0 present; node 24; Docker v29.4 up. Disk fine.
```

**Live-service bring-up (the no-mock gate needs these; both were left RUNNING at handoff on
8250/5566, but they are dev/in-mem so recreate freely):**
```bash
# OpenBao dev (AppRole/KV/Transit/PKI) — W1/W3/W4/W5 self-provision what they need:
docker run -d --name wsf-openbao-w1 --cap-add=IPC_LOCK -p 8250:8200 \
  -e BAO_DEV_ROOT_TOKEN_ID=root -e BAO_DEV_LISTEN_ADDRESS=0.0.0.0:8200 \
  openbao/openbao:latest server -dev -dev-root-token-id=root
# Moto (free AWS STS mock — LocalStack :latest now demands a paid license, exit 55):
docker run -d --name wsf-moto -p 5566:5000 motoserver/moto:latest
# Then run the live gates:
WSF_OPENBAO_ADDR=http://127.0.0.1:8250 WSF_OPENBAO_TOKEN=root \
  cargo test -p wsf-bridge --test live_openbao -- --nocapture
WSF_OPENBAO_ADDR=http://127.0.0.1:8250 WSF_OPENBAO_TOKEN=root WSF_AWS_ENDPOINT=http://127.0.0.1:5566 \
  cargo test -p wsf-broker --test live_localstack -- --nocapture
```

- **safe-edit skill:** the repo has a MANDATORY `safe-edit` skill for the CoWork Linux-mount
  truncation bug. This session is **native Windows** — Write/Edit hit NTFS via `C:\` paths, so that
  bug does not apply. It was **downgraded to advisory** (recorded in DEVLOG 0.1). Write worked
  reliably for all new files this session. Keep the good hygiene anyway (surgical Edits, stage
  individually, no `git add -A`).
- **Pre-commit hook** (`.integrity/hooks/pre-commit`, auto via `core.hooksPath`): checks null-bytes,
  >50% truncation, brace balance (.rs/.py/.js/.ts), and `cargo fmt --check` if any `.rs` staged.
  Markdown/toml commits pass clean. Run `cargo fmt` before committing Rust.
- **Commit footer (this repo's convention — use verbatim):**
  ```
  Copyright 2026 - Co-Authored by Basho Parks and Claude Fable 5 <basho@islandmountain.io> <claude@anthropic.com>
  ```
- **Per-prompt verify gate:** `cargo fmt -p <crate>` → `cargo test -p <crate>` →
  `cargo clippy -p <crate> -- -D warnings -A clippy::pedantic`. For extraction/reuse prompts also run
  `cargo test -p mai-compliance` (its 331+ tests are the regression guard) and `cargo check --workspace`.
- **New crate checklist:** create under `crates/<name>/`, add `"crates/<name>"` to the **root
  `Cargo.toml` `[workspace] members`** (kept roughly alphabetical), `[lints] workspace = true`,
  `version.workspace = true` etc.
- **DEVLOG:** append a `### <Prompt> — DONE` entry to `docs/sessions/SOVEREIGNTY-DEVLOG.md` for every
  prompt (files, verify result, commit id).

### Gotcha that bit twice this session
Integration tests in `tests/*.rs` that call trait methods (`signer.public_key()`, `.sign()`,
`.verify()`) **must import the trait**: `use fabric_crypto::Signer;` / `use fabric_crypto::Verifier;`.
The method won't resolve otherwise. (Normal deps of a crate ARE usable by name in its integration
tests — no dev-dep needed.)

---

## 2. Map of everything

| What | Where |
|---|---|
| **Plan (P-SPR)** | website repo `Island Mountain/PLANNING/AOG-WSF-SOVEREIGNTY-STACK-PSPR.md` (Phases 0/F/W/G/T/C/D/Z + Appendix D threat-model/enrichments). **Appendix D was blocked by a OneDrive file-lock at write time** — the authoritative threat mapping is in the MAI repo instead (below). |
| **Build DEVLOG** | this repo `docs/sessions/SOVEREIGNTY-DEVLOG.md` — every prompt, verbatim. |
| **Reuse map** | `docs/architecture/SOVEREIGNTY-REUSE-MAP.md` — which MAI file feeds which new crate + parked list. |
| **Threat model / design contract** | `docs/architecture/AGENTIC-SECURITY-MAP.md` — Basho's "Agentic Orchestration & Security Map" (his blog/infographic) adopted as the canonical AOG/WSF spec: 9 threats → controls → where-they-live → status + 6 enrichments E-A..E-F. |
| **Contracts (frozen v1)** | `contracts/{identity,trust-token,receipt,envelope}.md` + `crates/fabric-contracts` (tag `contracts-v1`). |
| **Memory (persists across sessions)** | `~/.claude/projects/.../memory/`: `aog-wsf-product-initiative.md` (master status), `mighty-eel-mai-asset.md` (the MAI codebase inventory), `agentic-orchestration-security-map.md` (the spec), `MEMORY.md` (index). |
| **OpenBao client to reuse (Phase W)** | `mai-api/src/openbao_client.rs` + `handlers/trust.rs` + `air_gap.rs` (the TLM AppRole/Transit/PKI code). |
| **MAI asset docs** | `docs/compliance/TRUST-MANIFOLD.md` (3 rings), `OPENBAO-INTEGRATION.md`, `SERVICE-IDENTITY.md`. |

**Do not touch / parked** (revive only with Summit): `mai-scheduler` (has known fake-metrics defects),
`mai-hil`, the Python inference adapters beyond what AOG's gateway needs, the L5 family-app scaffolds.
The stale VS-Code clone (`Documents/VS Code Lamprey Repo Clone/...`) was **deleted** — don't recreate.

---

## 3. What's DONE (14 commits, cdfb05f → edcfb8c)

**Phase 0 — foundation & contracts:**
- `SOV-0.1` reuse map + DEVLOG + baseline (1627 tests) + stale clone removed.
- `SOV-0.3..0.6` four wire-contract specs.
- `SOV-0.8` `fabric-contracts` crate (5 tests) + tag `contracts-v1`. An MAI claim deserializes as a
  budget-off trust token (superset proven).
- `SOV-0.2a` `fabric-crypto` — the signer abstraction: `Signer`/`Verifier` traits;
  `RustCryptoMlDsa87` (pure-Rust ML-DSA-87 default, mirrors mai-vault's proven `pqc-dev`);
  `TransitSigner` (OpenBao-Transit custody **seam**, fails closed until Phase W).
- `SOV-0.2b` dropped the `pqc-prod`/archived-`pqcrypto` backend from mai-vault (pure-Rust sole).
- `SOV-0.2c` fixed anyhow + quinn-proto advisories; **pyo3 waived** (non-reachable, grep-proven,
  mai-adapters-only/parked) in `.cargo/audit.toml` + `deny.toml` + `docs/compliance/INDEPENDENT-EVIDENCE-DEFERRALS.md`; `cargo audit` exit 0.
- `SOV-0.2d` axum 0.7/0.8 dual-`Handler` resolved by **isolation** (new service crates pin
  tonic 0.14 + axum 0.8; mai-api's legacy tonic migration deferred).
- `SOV-0.7` added a `cargo-audit + cargo-deny` **advisories job** to `.github/workflows/ci.yml`.
- `1ffe99b` adopted the Agentic Security Map as the canonical spec.

**Phase F — all 8 fabric primitive crates (in `crates/`):**
- `fabric-contracts` — the four wire types (identity, trust-token w/ budget+attenuation, receipt, envelope).
- `fabric-crypto` — Signer/Verifier + RustCrypto ML-DSA default + Transit seam.
- `fabric-proof` (`SOV-F1`) — canonical-JSON (byte-identical to mai-compliance), subject-hash, ML-DSA
  `BundleVerifier`, BLAKE3 hash chain. **mai-compliance now DELEGATES** its `subject_hash` +
  `bundle::write_canonical` to fabric-proof; its 331+ tests stayed green. Deeper audit-chain migration
  is **staged** (deeply integrated across ~23 files — fabric-proof is the source for new WSF/AOG code
  and is proven wire-compatible).
- `fabric-token` (`SOV-F3`) — issue/verify/**attenuate** (narrowing invariant on every axis, fails
  closed on widening)/`try_spend` (atomic budget metering).
- `fabric-identity` (`SOV-F2`) — mint/verify + Session/Task child derivation + pseudonymize.
- `fabric-envelope` (`SOV-F4-F6`) — seal (AES-256-GCM; `data_key_wrapped` = Phase-W transit seam) +
  label (readable un-sealed, AAD-bound so tampering breaks decrypt) + thread (ML-DSA provenance).
- `fabric-cache` (`SOV-F7`) — Ring-3 connectivity state machine → route ceiling (Expired/AirGapped → local-only).
- `fabric-revocation` (`SOV-F8`) — signed, offline-applicable revocation snapshots.

**Verified:** `cargo test --workspace` = **1668 passed / 0 failed** (93 binaries).

---

## 4. Decisions a new session MUST honor (don't re-litigate)

1. **Crypto:** one `Signer`/`Verifier` abstraction (`fabric-crypto`); pure-Rust RustCrypto ML-DSA-87
   is the offline default; OpenBao **Transit is a pluggable custody provider behind the same trait**,
   lit up in Phase W **when/if OSS OpenBao ships GA post-quantum Transit** (today only Vault Enterprise
   1.19 has it, experimentally — do NOT depend on Vault Enterprise). Air-gap needs local signing, so
   pure-Rust default is non-negotiable. The `ml-dsa 0.0.4` RUSTSEC-2025-0144 timing advisory is
   **waived** (air-gap-mitigated); the abstraction makes the eventual fix a one-line provider swap.
2. **FIPS-liboqs (`pqc-prod`):** dropped for now (user decision). Re-add later behind a new feature
   using the maintained `oqs` crate **only if an ITAR/defense deployment requires it**.
3. **AOG scope:** **govern-from-outside first** (sit at the hops over customers' existing agent
   frameworks — Claude/OpenAI SDKs, LangGraph, CrewAI, AutoGen, ADK, Temporal), **AND** ship its own
   orchestration runtime later. Both in scope; govern-external leads.
4. **Threat model = the spec.** Every AOG/WSF feature must trace to a threat on the Agentic Security
   Map. The 6 enrichments (E-A orchestration-pattern governance, E-B memory/RAG provenance, E-C session
   integrity/signed checkpoints, E-D tool supply-chain, E-F sandboxed exec, E-F OWASP evidence) fold
   into M2/M3 — see `docs/architecture/AGENTIC-SECURITY-MAP.md`.
5. **WSF naming:** "Woven Sovereignty Fabric" recommended (fits the WSF acronym; "trust tokens" stays
   the primitive name) — **still unconfirmed by Basho**. Don't hard-code a public name yet.
6. **Milestones:** M1 sovereign-shadow (AWS + HIPAA, shadow mode) / M2 enforce+agents / M3 estate.
   Keep them shippable in cuts.
7. **`fabric-proof::chain` is WSF's OWN receipt-ledger chain**, intentionally distinct from
   mai-compliance's audit-log chain (which stays in that crate).

---

## 5. What's NEXT — Phase W (WSF services)

Per the plan (Phase W, W1–W10). These are **services** (async, will use axum/tonic 0.14 + the OpenBao
client), and they hit **live OpenBao** — the **no-mock-only-closure gate applies**: token issuance /
envelope seal / receipts / cred brokering / policy must have ≥1 test against a **live OpenBao Docker
service** (Docker is available; there's no `scripts/start-openbao.ps1` in-tree yet — create a compose
or `docker run openbao/openbao` bring-up and wire it into CI as a service container per `SOV-0.7`'s
deferral note in `ci.yml`).

Recommended order — **W1 + W2 are DONE** (see §0). **Resume at W3.**
- ~~W1 `wsf-bridge`~~ DONE (`3ead250`). ~~W2 `wsf-broker`~~ DONE (`f3e589f`).
- **W3 `wsf-seal` (NEXT):** network service over `fabric-envelope`; the F4 `data_key_wrapped` becomes
  a **real OpenBao-Transit wrap** (`transit/encrypt|decrypt/<key>` — Transit *does* symmetric AEAD,
  it just lacks ML-DSA *sign*, so the seal seam lights up here without touching the signing decision).
  Seal on ingress; unseal only for a **token-authorized** op; **every op emits a receipt** (fabric-proof
  chain). Gate says "seal/unseal **over HTTP**" → this is the **first axum 0.8 service** (per 0.2d pin
  `axum 0.8` directly; tonic not needed until W6). Suggested shape: a `SealService` library
  (seal / unseal / token-auth / transit-wrap / receipt) + a thin axum app (`POST /seal`, `POST /unseal`);
  the live test spins the app on a port and drives it against live OpenBao Transit. Unauthorized unseal
  → deny + receipt.
- **W4 `wsf-ledger`:** append-only receipt ledger over `fabric-proof` + signed evidence-pack export
  (reuse `mai-compliance/src/reports/*`). **W5** Ring-3 cache daemon over `fabric-cache`+`fabric-revocation`.
  **W6** WSF REST/gRPC + SDK — **first tonic 0.14 crate** (verify the 0.2d pin here; extend `mai-sdk-rs`).
  **W7** GCP broker + **W8** Azure broker (same `wsf-broker` shape; add provider modules — the SigV4/STS
  code is AWS-specific, GCP/Azure use their own signing). **W9** tenant provisioning. **W10** HA/hardening.

### W-phase working notes (learned in W1/W2 — reuse these)
- **Shared OpenBao client:** `wsf_bridge::OpenBaoAuth` (`login` / `get_tenant` / `get_kv_data` /
  `health`). W3+ depend on `wsf-bridge` for it (or factor a `crates/wsf-openbao` + re-export if the
  peer-dep smell grows — deferred to avoid churn).
- **Live-test pattern:** env-gated (`WSF_OPENBAO_ADDR` [+ `WSF_AWS_ENDPOINT`]), **no `#[ignore]`**,
  `#![allow(clippy::print_stdout, clippy::print_stderr)]` at the top of the test file so the SKIP/PASS
  `eprintln!` doesn't trip the workspace `print_*` deny. Self-bootstrap OpenBao from the root token.
  Parse OpenBao responses via `.text()` + `serde_json::from_str` so reqwest's `json` feature isn't needed.
- **Trait-method gotcha (bit us again):** `signer.public_key()` in a test needs `use fabric_crypto::Signer;`.
- **CI:** the `wsf-live` job (`ci.yml`) brings up OpenBao + Moto via `docker run` and runs both live
  tests — add each new W-service's live test to that job's run block.
- **⚠ PUSH-BLOCKER for Basho (decide before the end-of-STS push):** `.github/workflows/commit-msg-check.yml`
  requires every commit footer to contain `Co-Authored by Basho Parks and Claude Opus 4.7 xHigh …`, but
  **every SOV commit** (Phase 0/F/W, per the handoff convention) uses `… Claude Fable 5 …`. None match →
  the check FAILS the whole branch on push. Reconcile: either update the CI regex to be model-agnostic
  (recommended — the model has changed twice) or rewrite footers. This session kept `Fable 5` for
  branch consistency; flagging, not silently switching.

Then: **Phase G** (AOG gateway: reuse `mai-router` + `mai-compliance` composer; NEW cloud provider
clients + metering; OpenAI/Anthropic-compatible surfaces; shadow/enforce), **Phase T** (MCP tool proxy,
approval inbox, provenance/egress, mission contracts + the E-A..E-D enrichments), **Phase C** (React
console — new `console/`, Vite+React 19+Tailwind, panels aesthetic; replaces the Jinja2
`compliance-dashboard/`), **Phase D** (docker-compose appliance, signed images/SBOM, HIPAA pack,
**external re-scan** = the J-14 that never ran, burn-in), **Phase Z** (version, release, **Bucket/push
— the ONLY point where pushing happens**).

---

## 6. State-verification commands (run to confirm the handoff)
```bash
cd "C:\Users\17076\Documents\Claude\Island Mountain\Island Mountain Mighty Eel OS\mai-worktrees\mai-SOV-1"
git branch --show-current            # session/SOV-1
git log --oneline 7a19c7b..HEAD      # 16 SOV commits, HEAD f3e589f
ls crates/                           # 8 fabric-* + wsf-bridge + wsf-broker
cargo test -p wsf-bridge -p wsf-broker   # offline suites green (live tests env-skip)
cargo check --workspace              # exit 0
cargo audit                          # exit 0, 0 vulnerabilities (1 accepted proc-macro-error2 warning)
```

**Nothing is uncommitted; nothing is pushed.** Pick up at **Phase W, W3** (`wsf-seal`).
