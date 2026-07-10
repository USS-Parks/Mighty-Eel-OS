# X4 / X5 Independent Revalidation — im-mighty-eel-mai

**Date (UTC):** 2026-07-09
**Tree:** `main` @ `803e85e` (all prior remediation CI-green at this tip)
**Method:** seven independent read-only reviewers (five X4 attack-surface slices + two X5 red-teams), each instructed to distrust the remediation's "all Critical/High fixed" claim and verify against source, not comments/DEVLOGs/test names. Every reachable High was then re-verified line-by-line by the orchestrator against source (citations below are confirmed, not relayed).

---

## Verdict

**CONDITIONAL — not a FINAL go.** The **ship/production posture is genuinely hardened and its controls hold** — six of seven reviewers independently confirmed the ship-path enforcement is real, and the large majority of the audit's original attacks are closed with landed controls and, in most cases, real tests (see "Genuinely closed"). But the revalidation surfaced **one reachable High an unauthenticated attacker can hit in a normal deployment (H9), which the prior remediation mis-dispositioned against the wrong service**, plus a coherent cluster of High-class gaps that are *fail-open-by-omission*, *config-asserted instead of runtime-probed*, or *hardware/multi-node deferred*.

- **X4 (independent re-scan) gate — "zero reachable Critical/High":** MET for the intended air-gapped ship-profile + systemd + loopback + reverse-proxy-TLS deployment — **except H9**, which is reachable in any gateway deployment because `/v1/status` is auth-exempt by design. Net: **one reachable High (availability).**
- **X5 (red-team) gate — "docs yield a safe install; every attack denied/audited":** **NOT MET.** The docs do not reliably yield a safe install (dev-posture steering + one shipped default credential), and the attack re-run leaves **8 OPEN** (4 known hardware/multi-node deferrals, 2 real landed-elsewhere code gaps, 1 mis-disposition [H9], 1 readiness coupling).

**What the independent pass added over the prior self-run's "no new reachable Critical/High":** it is optimistic. The honest answer is **one reachable High (H9)** plus several High-class controls that are *asserted by config flag* rather than *enforced at runtime* — precisely the class a self-review misses.

---

## Reachable now (the gate-relevant findings)

### R1 — HIGH (availability) — unauthenticated `/v1/status` DoS; prior remediation mis-dispositioned it
Reachable by an unauthenticated remote client in a normal gateway deployment — no operator omission, no hardware caveat.
- `/v1/status` is mounted with no auth layer (`crates/aog-gateway/src/surface_openai.rs:35`; docstring at `:306-309` confirms "Open … without a virtual key").
- `status()` (`:310-313`) takes `state.receipts.lock()` and calls `led.verify()` **inside** the locked region → `verify_chain(&self.links)` walks genesis→head **O(n)** (`meter.rs:172-175`).
- The receipt ledger is **unbounded** (`meter.rs:154-163`, `append` pushes with no cap/evict), so the O(n) grows for the process lifetime.
- The per-completion hot path `record` takes the **same** mutex (`meter.rs:288`). There is no rate limiter in the crate.
- **Failure scenario:** an attacker floods `GET /v1/status`; each call holds the receipts mutex across an ever-longer chain verification, starving legitimate inference completions of the lock they need to record receipts → throughput collapse.
- **Prior mis-disposition:** DEVLOG D1 declared H9 "already mitigated" but re-traced mai-api's health probes (bounded); the endpoint the finding names lives in aog-gateway and is unmitigated.
- **Fix (cheap):** cache the verify result / verify incrementally at `append` instead of O(n) on read; snapshot head+len under the lock and verify outside it; bound (cap/evict) the ledger; and/or rate-limit the status route.

### R2 — HIGH (conditional; fail-open-by-omission) — header-trusted admin when run without a ship profile
Two independent reviewers (gateway slice + docs red-team) converged on the same root.
- With no `MAI_SHIP_PROFILE` **and** no `config/auth_keys.toml`, `allow_internal_profile_header` defaults to **`true`** (`mai-api/src/server.rs:874-876`) and the entire production readiness guard is **skipped** (it is gated on a profile being present, `server.rs:442`).
- The auth middleware then trusts `X-IM-Profile: anyone:admin` with **no API key** (REST `auth.rs:476-489`; gRPC `grpc/mod.rs:184-188`).
- `ServerConfig::validate()` permits a routable **LAN bind** — it rejects only wildcard `0.0.0.0`/`::` (`config.rs:289-292`); the loopback-requiring `validate_with_connectivity()` only bites under air-gapped/expired state.
- **Reachable** on a fresh appliance brought up without a ship profile and bound to a LAN IP → unauthenticated REST+gRPC admin takeover. **Closed** by the default (loopback) and fully by the ship profile (bypass forced off, guard enforced, production first-boot fails closed). The defect is that the *insecure state is the default of omission* and nothing warns when the bypass is live on a non-loopback bind.
- **Fix:** default the bypass to `false`; refuse (or loudly warn) a non-loopback bind without a ship profile.

---

## Reachable under physical / insider / operator-misconfig access (High-class defense-layer gaps)

### R3 — HIGH (integrity) — production compliance audit chain runs on NullSigner
- Production builds the log with a sealer but **no signer** (`mai-api/src/server.rs:583`); `MlDsaChainSigner` is constructed only in tests. The "periodic PQC checkpoint signatures" (a documented Lamprey differentiator) are therefore never produced.
- Under 1000 entries there are no signature boundaries, so `GET /v1/compliance/audit/verify` returns `verified:true` on BLAKE3 linkage alone — a holder of the audit `sealer.key` (insider/root) can rewrite entries, relink, and re-seal, and the chain re-verifies clean. (Over 1000 entries the same endpoint returns `verified:false` permanently on a clean chain — unreliable both ways.)
- Coupled MEDIUM (sign-before-mutate, `mai-compliance/src/audit/api.rs:237-252`): `record()` mutates `lamprey_decision_id` after `finalize()` signs the content hash, so any real signer's boundary signatures would fail to verify — fail-closed, but proof the signing path was never exercised end-to-end.
- **Fix:** wire `MlDsaChainSigner` in production; fix the sign-before-mutate ordering alongside.

### R4 — MED-HIGH (supply-chain authenticity) — model-package signature bound to the vault's own ephemeral key
- `PqcEngine::initialize()` mints a **fresh ephemeral** ML-DSA keypair every boot, never persisted (`mai-vault/src/pqc.rs:372-386`). Model-package verification checks the manifest signature against *that* self-key (`models/verify.rs` → `pqc.rs:657`), and the manifest's `public_key_fingerprint` is parsed but never consulted. No distribution/factory trust anchor is loaded in the vault build.
- So "manifest authenticated" means "signed by this appliance's own boot key," not "signed by Island Mountain" — it is not a supply-chain origin control. (Requires USB/operator access; hash-tree binding, `model_id` path validation, and at-rest encryption still hold.)
- **Fix:** load and verify against a pinned distribution trust anchor (the machinery already exists for the Lamprey policy bundle).

### R5 — MED-HIGH (readiness honesty) — vault certifies a "sealed master key" it does not runtime-prove
- `PROD-VAULT-004` certifies the seal by checking a **config flag** (`production_guard.rs:522-529`); the runtime probe `PROD-VAULT-100` is a store→load round-trip that never asserts a key is sealed. `first_boot()` (the only sealing path) is called **only from its own test**, never the server boot path. The KEK is written as a **plaintext file** (`pqc.rs:289`).
- **Split honestly:** the confidentiality-at-rest layer (real TPM seal + real ZFS native encryption) is the **known, owner-gated hardware deferral** — the code says so in its own comments. The *new* code defect is that the sealing is **not wired to boot at all** and readiness **greens the seal property without probing it** — so even on real TPM hardware, today's code would not seal, and readiness would still pass.
- **Fix:** wire `first_boot` sealing on the production boot path; make `PROD-VAULT-004` a runtime seal probe that fails closed.

### R6 — MED-HIGH (multi-tenant integrity) — tenant-spoofed object create
- `admission::stamp_create` never binds `metadata.tenant` to `principal.tenant` (`aog-apiserver/src/admission.rs:519-529`); the A3 remediation covered **delete** only. `metadata.tenant` is attacker-controlled on create.
- **Fix:** bind the created object's tenant to the authenticated principal (mirror the delete-path check).

---

## Latent (real defect, not currently wired to a live surface)

### R7 — HIGH (latent) — Ring-3 offline cache ignores 5 of 7 revocation dimensions
- `Ring3Cache::verify_offline` (`wsf-cache/src/lib.rs:176-183`) checks only token-id + subject-hash; it ignores signing-key, issuer, bundle-version, tenant, and service-identity. The complete predicate `RevocationSnapshot::revokes()` (`fabric-revocation/src/lib.rs:150-175`) exists and every other consumer uses it.
- **Not reachable today** — grep confirms `Ring3Cache::decide` has no non-test caller; no daemon wires the Ring-3 cache yet. But it is the flagship air-gap enforcement primitive, so key-compromise and tenant-deprovision revocation would be silently ineffective offline the moment it is wired. **Untested:** the live test only exercises token-id revocation.
- **Fix (one line):** replace the two checks with `snap.revokes(token)`.

### R8 — MEDIUM — streaming budget bypass (D8)
- The streaming branch (`surface_openai.rs:175-198`, `surface_anthropic.rs:132-155`) returns without `meter::record`/`record_spend`; metering is only on the non-stream branch. A budgeted virtual key can stream past its cap (classified-egress is still blocked; only budget accrual is skipped). Fix was prototyped then reverted (owner-gated).
- **Fix:** meter on stream completion.

---

## Docs (X5-F) — "docs alone yield a safe install": conditional FAIL

A safe path **does exist and is code-backed** (INSTALL.md → deb → systemd sets `MAI_SHIP_PROFILE` + runs `mai-ship-validate` ExecStartPre; the guard then rejects StubVault/NullSealer/AcceptAll/synthetic-exchange and fails closed before `bind()`). It fails the gate because the doc set disagrees with itself and one credential leaks through the safe path:

- **D-1 MED (High if loopback reachable):** the dashboard ships a default admin token. `admin_token()` returns `"dashboard-dev"` when `MAI_DASHBOARD_ADMIN_TOKEN` is unset (`compliance-dashboard/util.py:44`), and the packaged `mai-dashboard.service` sets five env vars but **not** that one. The Rust "production guard" only validates a profile *field* in a *different* process, so the running console (127.0.0.1:8430) accepts `dashboard-dev` while `production.example.toml`/SHIP-PROFILE.md claim it is "rejected in production." Loopback-bound and behind mai-api's own token, hence Medium — but a shipped default credential + false contract. Fix: generate the token at install, or have `mai-ship-validate` assert the dashboard env.
- **D-2 MED:** `deployment/ship/README.md` documents `MAI_PROFILE=ship … --config profile.toml`; `MAI_PROFILE` is read **nowhere** in `mai-api/src` (only `MAI_SHIP_PROFILE` is), so that command is a dead-var dev dry-run that does not engage the guard. The README labels it a dry-run two lines down, and the systemd path is correct — cleanup + a clearer banner.
- **D-3 MED-HIGH:** `docs/operations/DEPLOYMENT.md` ("operator-facing Deployment Guide") defaults its Quick Start to `scripts/launch.sh` → `cargo run` with no `MAI_SHIP_PROFILE` (StubVault + MemoryAuditWriter + bypass-on), the production toggle buried below "Log Collection."
- **D-4 MED:** the buyer/acquisition guides never list the `ship` profile and carry a stale "swap the `exchange_token` handler body" instruction (superseded by the profile's `TrustExchangeMode`).
- **D-5 LOW:** `SECURITY.md` says the profile-header bypass is "disabled by default"; the no-profile code default is `true` (the same dev-path gap as R2).

---

## Deployment / CI Layer-3 (X4-D) — Medium/Low; no unsafe production artifact

The production compose (`deployment/wsf-ha`), ship profiles, Dockerfiles, and secret hygiene were verified **genuinely hardened** (OpenBao not dev-mode, TLS on, trust port internal-only, images digest-pinned, secrets via file provider, no committed secrets, non-blanket advisory ignores). The gaps are enforcement wiring:

- **MED:** `validate_profile.py` (the AF-12 containment control) is **not wired into any CI workflow** — it runs only in isolated pytest, never against the actual compose files; and it guards only `TRUST_PORT=8200` and keys "trust core" off image-name markers (a renamed image or a host-published `:8300` evades it).
- **MED:** `supply-chain.yml` `sbom-sign` has no `needs: [phone-home]` — it builds/pushes + cosign-signs + SBOM-attests the appliance image **regardless of whether phone-home fails**.
- **MED:** `gpu-release.yml` `gpu-bundle` uses `if: always() && gpu-build.success`, so the signed bundle assembles even if integration/benchmark/readiness gates failed (readiness is `continue-on-error`).
- **MED:** `.gitleaks.toml` allowlists all of `deployment/*-staging/`, including 9 git-tracked files (no live secret today — hash-only keys, public anchors — but a real blind spot).
- **LOW:** `no-phone-home.sh` scans only `crates/*`, missing the shipped `mai-*` runtime and its `https://updates.islandmountain.ai` default update URL (explicit `GET /v1/updates/check` only, no auto-beacon); `sign.sh` keyless verify accepts any identity/issuer.

---

## Python (X4-E) — clean at Critical/High

No unsafe deserialization (no `pickle`/`yaml.load`/`eval`), no command injection (list-argv only), no SSRF (model names ride the JSON body, not the URL), no secrets in logs. Two sub-High notes:
- **MED:** `adapters/runner.py` reads request lines with asyncio's default 64 KiB `StreamReader` limit while `base.py` advertises prompts up to 200 K chars — a prompt between those sizes crashes the (crash-isolated, restarted) adapter. The Rust orchestrator's F4 8 MiB frame cap sits *above* this, so the 64 KiB readline is the tighter limit; reachability depends on orchestrator framing.
- **LOW:** adapter clients read full backend responses with no size cap (trusted-local-backend assumption).

---

## Attack re-run (X5-G) — 8 OPEN, grouped by root cause

- **Known hardware/multi-node deferral lane (4)** — match the DEVLOG's own honest deferrals: **C2** raft mTLS (`aog-wire` `server_config()` built but never mounted; contained today only by the aogd loopback-bind guard — becomes Critical if `AOGD_ALLOW_INSECURE_BIND=1`), **A4** quorum fence (primitive exists, serve path doesn't call it), **H4** self-declared attestation (scheduler checks presence only; `TODO(basho)` for the signed quote), **H5/AF-06** TPM seal + readiness (= R5).
- **Landed-elsewhere real code gaps (2):** tenant-spoofed create (= R6), streaming budget bypass (= R8).
- **Mis-disposition (1):** **H9** (= R1) — the standout, proven reachable from source.
- **Readiness coupling (1):** AF-06 (= R5).

---

## Genuinely closed — verified against source (do not re-flag)

Controls landed **and** real tests assert the denial: attenuation widening on every axis incl. empty-set (H1, randomized per-axis), key_id+alg signature binding (H2), attenuation signing-oracle parent auth (AF-02), cross-tenant/audience envelope unseal (AF-04), AWS credential-broker grant scoping — no raw ARN (AF-05), revocation freshness/anti-rollback (AF-15) + fail-closed-on-absent (AF-15B), secret-key zeroize (K3), AOG fail-closed enforce default (AF-13), full-field vault audit hash (H6), stripped-boundary-signature fail-closed (H7), evicted-entry tamper + >8192 clean log (H8), tiny-frame STT flood (H10), poisoned-lock survival (D4), sustained-outage breaker (D5), session/tool flood + hung-tool timeout (D6), HIPAA-disabled PHI egress → local (G1), empty-classifier regulated-not-public (G2), clock-rollback cloud egress (P2), WebSocket role self-declare (P3), unauthenticated WSF issuance (AF-01), gRPC admin-metadata spoof (AF-03), WSF receipt-query tenant scoping (AF-14), planted-roster-code scanner (Q1-Q3). Controls landed + logic-confirmed (live/2-tenant/timing test deferred): C1 unauth `/admin/*`, H3 cross-tenant delete, AF-08/09 streaming classified-egress refusal, AF-10 legacy-completions pipeline, AF-17A/B usage/ROI scoping, AF-07A/B ZFS at-rest + snapshot seam, H11 provider timeouts. The gateway egress fail-closed guard (both surfaces, both stream and non-stream), the revocation kill-switch, tenant scoping, and the DF-01B `model_id` path-traversal fix were all independently re-confirmed solid.

*Test-assertion gaps worth a non-blocking regression pass:* C1 (no 403 integration assertion), H3 (cross-tenant delete unasserted), AF-10/AF-17 (no dedicated / 2-tenant test). *Scanner note (informational):* `apps/openbao-trust-demo/config.toml:5` cites a non-existent `BUILD-EXECUTION-PLAN-V2-UPDATED.md`; the no-slop DOC dangling-ref check only validates `docs/…\.md` paths, so a bare `.md` in a non-doc file slips through.

---

## Scope honesty

- **Owner/hardware lane (unchanged, still deferred, not counted as re-scan failures):** real ZFS native-dataset encryption, real TPM 2.0 seal + attestation, the ≥3-node Raft mTLS/quorum estate, and the 72-hour soak. `mai-vault`'s `TpmManager` is a software simulation and is not on the runtime path.
- This revalidation is code + docs review plus the already-green live gates (WSF Live-OpenBao + Loom 5+5 at `803e85e`). It did not run a live fuzz/soak (D9) or a real-hardware vault gate.

---

## Recommendation

**Do not issue a FINAL go yet.** Close the reachable and High-class code items — all fixable on this host without new hardware — then re-run this revalidation:

1. **R1 / H9** (headline, cheap) — bound the ledger + drop the lock-across-verify + cache/rate-limit `/v1/status`.
2. **R2** — default the profile-header bypass to `false`; refuse a non-loopback bind without a ship profile.
3. **R3** — wire `MlDsaChainSigner` in production; fix the sign-before-mutate ordering.
4. **R5** — runtime-probe the vault seal (fail closed) and wire `first_boot` on the boot path.
5. **R4, R6, R7, R8** — distribution trust anchor for model packages; bind tenant on create; the one-line `revokes()` fix in Ring3Cache; meter on stream completion.
6. **Docs (D-1…D-5)** — generate the dashboard token at install; add a production banner to DEPLOYMENT.md; list the `ship` profile in the buyer guides; drop the dead `MAI_PROFILE` var and the stale handler-swap instruction.
7. **CI Layer-3 (X4-D)** — wire `validate_profile.py` against the real compose files; gate `sbom-sign` behind phone-home; stop assembling release bundles on failed gates; scope the gitleaks/phone-home scanners to the shipped tree.

The owner/hardware lane (real ZFS+TPM vault gate, ≥3-node mTLS/quorum, 72-hour soak) remains as previously deferred. The core cryptographic trust plane, the gateway egress governance, and the audit-chain *verification* logic are genuinely sound — the gaps are enforcement wiring, readiness honesty, and a doc set that steers away from the hardened path.
