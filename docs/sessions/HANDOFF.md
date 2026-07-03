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
- **DONE:** **Phase 0 (foundation + contracts + crypto + advisories + CI gate) and Phase F (all 8
  fabric primitive crates)** — 14 commits, `cargo test --workspace` = **1668 passed / 0 failed**.
- **NEXT:** **Phase W** (WSF services on live OpenBao), then G (AOG gateway) → T (tool governance) →
  C (console) → D (deploy/proof) → Z (ship/Bucket).

---

## 1. Resume protocol (do this first)

```
Worktree (do ALL work here):
  C:\Users\17076\Documents\Claude\Island Mountain\Island Mountain Mighty Eel OS\mai-worktrees\mai-SOV-1
Branch:  session/SOV-1   (HEAD = edcfb8c)   — NOT pushed; push only at the very end
Toolchain: cargo 1.95.0 / rustc 1.95.0 present; node 24; Docker v29.4 up (for OpenBao). Disk fine.
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

Recommended order:
- **W1 `wsf-bridge` (Ring 2):** productize the TLM Trust Bridge. **Extract/adapt
  `mai-api/src/openbao_client.rs`** (AppRole auth, Transit sign, PKI issuance) into a form the bridge
  uses. Issue a `fabric-token` end-to-end against a live OpenBao; bundle signature verifies off-host.
- **W2 `wsf-broker` (STS, NEW):** exchange a verified trust token for **ephemeral cloud creds** —
  AWS STS `AssumeRole` + inline session policy first (test against **LocalStack**), then GCP/Azure
  (W7/W8). Root creds custodied in OpenBao `kv`.
- **W3 `wsf-seal`:** network service over `fabric-envelope`; the `data_key_wrapped` is now a real
  OpenBao-Transit wrap. **W4 `wsf-ledger`:** append-only receipt ledger over `fabric-proof` +
  signed evidence-pack export (reuse `mai-compliance/src/reports/*`). **W5** Ring-3 cache daemon over
  `fabric-cache`+`fabric-revocation`. **W6** WSF REST/gRPC + SDK. **W9** tenant provisioning. **W10** HA/hardening.

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
git log --oneline 7a19c7b..HEAD      # 14 SOV commits, HEAD edcfb8c
ls crates/                           # 8 fabric-* crates
cargo test --workspace 2>&1 | grep "test result:" | awk '{s+=$? } END{}'   # expect 1668 passed / 0 failed
cargo audit                          # exit 0, 0 vulnerabilities (1 accepted proc-macro-error2 warning)
```

**Nothing is uncommitted; nothing is pushed.** Pick up at Phase W, W1.
