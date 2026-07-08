# Full-Repo Security Remediation — Plan / Sequential Prompt Roster (P-SPR)

**Initiative:** Repository-wide remediation of the 2026-07-08 full-repo audit.
**Products:** MAI appliance, WSF trust plane, AOG "Loom" orchestration tier.
**Source audit:** `docs/audits/2026-07-08-full-repo/FULL-REPO-SECURITY-AUDIT.md` (rev `700cf2b`).
**Author:** Basho Parks + agentic review · **Created:** 2026-07-08
**Status:** **READY FOR STS APPROVAL — execution has not started.** Drafting this P-SPR
authorizes nothing (CANON I.1 / §0.2). Execution begins only on an explicit "run it STS"
or per-milestone approval from Basho.

---

## §0 — Mission, authority, stop conditions

### 0.1 Mission
Close every reachable Critical/High from the audit and reconcile the shipped security
docs with what the code actually does. Complete only when: (1) no unauthenticated caller
can mutate or read the AOG control plane; (2) attenuation and revocation are complete and
monotonic across every axis; (3) the vault's key-custody and audit tamper-evidence match
their claims (or fail closed with honest docs + a production guard); (4) audit-chain
verification is fail-closed; (5) reachable DoS/OOM/panic vectors are bounded; (6) the
full verify + live-service surface is green; (7) an independent re-scan reports zero
Critical/High.

### 0.2 STS meaning
Stem to stern: execute the roster in order, keep moving while a safe next prompt remains,
verify every prompt, record evidence, and stop only for a genuine authority boundary, a
destructive migration decision, an unavailable external credential (real ZFS+TPM,
multi-node host, signing infra), or a failed gate that cannot be repaired in-prompt.

### 0.3 Git & workspace discipline
Work only in the standalone `mai/` repo; preserve untracked `.opencode/`. One prompt =
one reviewable change set (security-contract migrations may use two commits: contract,
then consumers). Before each commit: list staged files, summarize, and ask **Shall I
commit?** Before push: enumerate outgoing commits and ask **Shall I push?** Commit footer
verbatim: `Authored and reviewed by Basho Parks, copyright 2026` — never an AI co-author.
Record each prompt in `docs/sessions/FULL-REPO-REMEDIATION-DEVLOG.md`.

### 0.4 Universal verify gate (every implementation prompt)
`cargo fmt --check` -> `cargo clippy --workspace -- -D warnings -A clippy::pedantic` ->
focused crate tests -> `cargo test --workspace` before a milestone closes -> `cargo audit`
-> `cargo deny check` -> `ruff check .` / `mypy` for Python prompts -> `gitleaks` +
`detect-secrets` before a milestone closes -> the no-slop full scan. Trust-touching
prompts additionally require a live gate (Dockerized OpenBao; Moto for broker; a >=3-node
harness for consensus/kill-switch). A mock-only test does not close a trust-boundary prompt.

### 0.5 Security regression rules
No new `#[ignore]` on trust-adjacent tests; no test-only auth branch compiled into prod;
no secret/key/plaintext in logs, receipts, panic text, or evidence; every denial receipted
without sensitive payload; every new production mode fail-closed on missing identity, stale
state, missing key material, or unavailable audit persistence; compatibility explicit and
versioned (never silently accept a weaker legacy artifact); no shipped-source doc may claim
a control the code does not implement (docs-vs-reality is a gate, not a nicety).

### 0.6 Stop-ship conditions (any one blocks release)
Any Critical/High open; the AOG control plane reachable without an authenticated admin
principal; the Raft transport unauthenticated on a non-loopback bind; attenuation widenable
or a revoked key/token usable; the vault certifying key-custody/audit claims the code does
not meet; audit-chain verification fail-open; a required live gate skipped; the final scan
lacking high-impact coverage without an owner-signed deferral.

---

## §1 — Finding baseline (audit -> phase)

| ID | Sev | Finding | Owner phase |
|----|-----|---------|-------------|
| C1 | Critical | `aogd` `/admin/*` unauthenticated on production socket | A |
| C2 | Critical | Raft transport + admin plaintext/unauthenticated; mTLS unwired | A |
| H1 | High | Attenuation empty `allowed_models` widening | K |
| H2 | High | Signing-key revocation bypass (unsigned `key_id`) | K |
| H3 | High | AOG delete path skips authorization (kill reversal) | A |
| H4 | High | Attested placement trusts self-declared attestation | S |
| H5 | High | Master signing key never TPM-sealed (init challenge-seal) | V |
| H6 | High | Vault audit hash covers 5/13 fields; SHA3 vs BLAKE3 doc | V |
| H7 | High | `verify_chain` skips stripped-signature entries | U |
| H8 | High | `verify_full` checks in-memory tail only; >8192 head bug | U |
| H9 | High | Unauthenticated `/v1/status` O(n) verify under global lock | D |
| H10 | High | STT `AudioBuffer` unbounded (zero-duration frames) | D |
| H11 | High | Provider HTTP clients have no timeout | D |
| M* | Medium | composer/classifier/router fail-open; wsf-api dev-auth; wsf-cache clock; lock-poison; breaker; toolproxy; SSE slot; stream budget; minority fence; receipts durability; WS role; cache ITAR key; canned-success endpoints; mai-hil stubs | G/P/D/A |
| L* | Low | CANON §11 roster codes (systemic); scanner gap; dangling refs; doc-path drift; zeroize; heuristics; config overflows | Q |

Full per-finding detail: the audit report (same folder). Appendix A is the closure matrix.

---

## §2 — Target invariants (what "fixed" means)
- **AOG control plane:** every `/admin/*` and CRUD mutation authenticates an admin-scoped
  principal and traverses admission (validate + policy + seal + receipt); the Raft/admin
  transport requires CA-signed mTLS or a loopback-only bind; deletes authorize against the
  target; reads and the front-door revocation decision are quorum-fenced.
- **Trust primitives:** attenuation only narrows on every axis (incl. inverted-semantics
  empty sets); `alg` + `key_id` are inside the signed payload; secret key material zeroizes.
- **Attestation:** placement consumes a control-plane-verified hardware quote (vendor root
  + pinned PCRs + nonce), never a node-supplied string; unverifiable => `Pending`, never placed.
- **Vault:** the real master key is TPM-sealed (or fail-closed + honest doc + prod guard);
  the audit hash covers every security-relevant field; the labeled primitive matches the code.
- **Audit chain:** verification asserts interval-boundary signatures and verifies the
  persisted WAL from the true head; production rejects null crypto.
- **Robustness:** untrusted-reachable panics/OOM/hang are bounded; locks fail closed, not poison.
- **Compliance/routing:** fail-closed on empty/errored policy, empty classifier config,
  medical/PHI context; caller sensitivity hints raise the floor.
- **Hygiene:** no roster step-codes in shipped source; the no-slop scanner catches them;
  no dangling references; docs match reality.

---

## §3 — Milestones & critical path
| Milestone | Outcome | Phases |
|-----------|---------|--------|
| M0 Contained | Reachable Criticals network-contained; baseline + fixtures frozen | 0 |
| M1 Control plane | AOG auth/transport/authz/fence/receipts | A |
| M2 Primitives | Attenuation + revocation + key hygiene | K |
| M3 Custody & audit truth | Attestation, vault, audit-chain verification | S, V, U |
| M4 Robustness & guardrails | DoS/panic, compliance fail-closed, posture | D, G, P |
| M5 Hygiene | Slop, scanner, dangling refs, docs | Q |
| M6 Re-ship | Migration, live suite, re-scan, go/no-go | X |
Critical path: 0 -> A -> K -> (S,V,U) -> (D,G,P) -> Q -> X.

---

## Phase 0 — Containment & lane
- [ ] 0.1 Lane artifacts + baseline freeze: create the DEVLOG, finding register, evidence dirs (M0..M6); record HEAD, toolchain, and current gate results. *Gate: snapshot reproducible.*
- [ ] 0.2 Emergency containment (C1/C2): bind the `aogd` admin + raft surface to loopback (or refuse a non-loopback bind) until A1/A2 land; keep an explicit opt-in dev flag. *Gate: an off-host request cannot reach `/admin/*` or `/raft/*` in the default posture.*
- [ ] 0.3 Adversarial regression fixtures: one deterministic failing test per audit ID (C1..H11 + the reachable Mediums), asserting current vulnerable behavior in a quarantined harness; product tests flip to repaired behavior in-phase. *Gate: every finding has a regression id.*
- [ ] 0.4 Docs-vs-reality + threat-model reconciliation: enumerate every shipped doc/comment that claims a control (TPM seal, tamper-evident audit, attestation-liveness, "auth interceptor") and mark supported / blocked. *Gate: each production claim has a code+test owner or a BLOCKED tag.*
- [ ] 0.5 M0 review: focused tests + effective-listener/port inspection + containment report. *Gate: M0 repeatable on a clean local stack.*

## Phase A — AOG control-plane auth & transport (C1, C2, H3)
- [ ] A1 Authenticate `/admin/*`: require an admin-scoped principal (same authenticator as `/apis/**`); route legitimate writes through `Admission::admit`, not raw `node.write`; fail-closed. *Gate: unauth/underscoped `/admin/*` -> 401/403 before any store write.*
- [ ] A2 Wire mTLS (`aog-wire::NodeTls`) into the `aogd` serve path + `https` peer URLs; require a CA-signed client cert on `/raft/*` and admin. *Gate: a peer without a valid cert cannot vote/append/snapshot or reach admin.*
- [ ] A3 Delete-path authorization + tenant binding: authorize deletes against the loaded target (policy + caller authority; `RevocationIntent` delete is privileged); bind `metadata.tenant` to `principal.tenant` on create and freeze it. *Gate: cross-tenant/underscoped delete and tenant-spoofed create both denied.*
- [ ] A4 Quorum-fenced reads + front-door revocation: gate authoritative reads and the revocation decision behind `confirm_leadership` (linearizable); drive `SharedGate` from the quorum check. *Gate: a partitioned minority serves no authoritative allow and honors a majority-side kill.*
- [ ] A5 Durable, replicated receipt ledger: persist/replicate the K9 chain (or write to `wsf-ledger`); emit a receipt for every committed mutation incl. admin-path writes. *Gate: receipts survive restart and verify off-host; admin write is receipted.*
- [ ] A6 Live multi-node gate: >=3-node harness proves unauth admin/raft refused, delete authz, fence under partition, kill-switch honored. *Gate: C1/C2/H3 closed black-box.*

## Phase K — Trust primitives (H1, H2, + key zeroization)
- [ ] K1 Attenuation empty-set monotonicity: treat an empty child `allowed_models` as a widening when the parent is non-empty; audit every other inverted-semantics axis the same way. *Gate: property test rejects widening on each axis incl. empty-set.*
- [ ] K2 Sign `alg` + `key_id`: include them in the signed payload (strip only `signature.value` before hashing) so revocation-by-key and algorithm identity are tamper-evident. *Gate: rewriting `key_id`/`alg` breaks verification; revoked-key token denied.*
- [ ] K3 Zeroize secret material: `Zeroizing`/`ZeroizeOnDrop` for the ML-DSA secret key + keygen seed in `fabric-crypto`. *Gate: no plaintext key in a post-drop memory assertion test.*
- [ ] K4 Attenuation property suite: generate a widening on every scalar/set/budget/caveat/identity/offline axis and prove rejection. *Gate: 10^3 randomized attenuations, zero widenings pass.*
- [ ] K5 Live attenuation/revocation gate against OpenBao. *Gate: H1/H2 closed black-box.*

## Phase S — Attested scheduling (H4)
- [ ] S1 Quote contract: add a signed hardware-quote field (quote + AK cert chain + CP nonce + PCR set) to `AttestationProfile`. *Gate: schema round-trips; a bare-`pcr` profile is rejected for `>= Restricted`.*
- [ ] S2 CP verification: verify the quote against the platform vendor root + pinned reference PCRs + a fresh nonce before the attestation floor is accepted; node-supplied floor is never trusted. *Gate: forged/replayed/wrong-PCR quotes rejected.*
- [ ] S3 Scheduler consumes the CP-verified floor only. *Gate: a self-declared `Tpm/pcr:"x"` node cannot receive a Ring-3/Restricted workload; it stays Pending.*
- [ ] S4 Attestation-liveness: wire a real `Measurer` (drift -> evict + revoke) or, until hardware lands, make the control explicitly deny + reconcile the present-tense docs to "deferred". *Gate: drift path is exercised or honestly gated; no live-sounding doc over a stub.*

## Phase V — Vault integrity truth (H5, H6)
- [ ] V1 Master-key custody: TPM-seal the real signing key (or fail-closed with an honest doc + a production guard that refuses to certify readiness without a real seal). *Gate: readiness proves a recoverable TPM-sealed master key, or refuses.*
- [ ] V2 Full-field audit hash: hash every security-relevant `VaultAuditEntry` field (canonical), and sign the full entry, not just `entry_hash`. *Gate: editing any persisted field breaks `verify_chain`.*
- [ ] V3 Fix the primitive label (SHA3 vs BLAKE3) so the doc matches the code (choose one, state it). *Gate: doc == code.*
- [ ] V4 KEK custody: TPM-seal the key-encryption key (or fail-closed + honest doc). *Gate: no plaintext KEK on disk in the production posture.*
- [ ] V5 Converge the vault audit chain onto the `mai-compliance` full-canonical model (dedupe the two divergent implementations). *Gate: one audit-chain implementation, correctly documented.*
- [ ] V6 Live gate on a real ZFS+TPM host (env-gated). *Gate: H5/H6 closed on hardware (owner lane if unavailable here).*

## Phase U — Audit-chain verification (H7, H8)
- [ ] U1 Enforce interval-boundary signatures: `verify_chain` asserts a signature on every `(id+1) % interval == 0` entry when a verifier is configured (new `SignatureMissing`). *Gate: a stripped-signature boundary entry fails verification.*
- [ ] U2 Verify the persisted WAL from the true head (per-rotation segment), not the in-memory tail. *Gate: tampering with an evicted entry is detected; a clean >8192-entry log verifies OK.*
- [ ] U3 Fix the >8192-entry `HeadHashNonZero` false positive. *Gate: steady-state clean log verifies.*
- [ ] U4 Production crypto guard: refuse `NullSigner`/`NullSealer`/`AcceptAll*` in production. *Gate: prod boot fails on null crypto.*
- [ ] U5 Restart + tamper gate. *Gate: H7/H8 closed with restart evidence.*

## Phase D — DoS / panic-safety (H9, H10, H11, + M-robustness)
- [ ] D1 `/v1/status`: require auth (or strip the chain-verify), cache the last verify, bound/rotate the in-memory ledger, never hold the lock across a full verify. *Gate: unauth flood cannot stall completions.*
- [ ] D2 STT: absolute `data` byte cap + reject zero-duration / non-whole-sample frames. *Gate: a tiny-frame flood is refused, memory bounded.*
- [ ] D3 Provider clients: `timeout` + `connect_timeout` on the OpenAI/Anthropic reqwest clients (mirror `spend.rs`). *Gate: a hung backend errors within the bound.*
- [ ] D4 Lock-poison hardening: `unwrap_or_else(|e| e.into_inner())` (or `parking_lot`) on request-hot-path std locks; keep fallible work out of locked regions. *Gate: a poisoned lock does not wedge the path.*
- [ ] D5 Circuit breaker: clamp `cooldown_cycles`/guard `is_finite()` before `from_secs_f64`. *Gate: a multi-day outage does not panic.*
- [ ] D6 Toolproxy: evict `task_usage` per session/TTL; wrap `execute`/`review` in `tokio::time::timeout`. *Gate: session-id flood bounded; hung tool times out.*
- [ ] D7 SSE: release the scheduler slot on actual stream drop/complete, not a 300s timer. *Gate: abandoned streams free their slot promptly.*
- [ ] D8 Streaming budget accrual: meter + `record_spend` on the terminal usage frame (or refuse streaming with a budget). *Gate: `stream:true` cannot bypass the budget cap.*
- [ ] D9 Fuzz/soak gate on the untrusted-input surfaces. *Gate: no reachable panic/OOM under the fuzz matrix.*

## Phase G — Compliance/routing fail-closed (M-guardrails)
- [ ] G1 Composer: empty/errored post-filter decision set -> deny / `route=Local`; never map `route=None` to CloudAllowed. *Gate: disabling the HIPAA module does not egress PHI in Enforce.*
- [ ] G2 Classifier: fail construction when a required tier has zero patterns. *Gate: a mis-keyed config errors, never classifies regulated as Public.*
- [ ] G3 Router: force Local (or elevate) on `EntityKind::Medical`, mirroring ExportControlled/Tribal. *Gate: a medical-context query routes Local.*
- [ ] G4 Router: honor `upstream_flags` as a classification floor (or delete the field + its doc claim). *Gate: an upstream `phi` hint raises the floor.*
- [ ] G5 De-id: substitute `{idx}` (or drop it from the template doc). *Gate: template output has no literal `{idx}`.*
- [ ] G6 Compliance cache: fold `ActorContext` (country/person_type) into `DecisionKey`. *Gate: two identical-bundle different-actor requests do not collide.*
- [ ] G7 Detector input normalization (NFKC/homoglyph/whitespace) before regex; fix the `entities.rs` offset-drift on non-ASCII. *Gate: obfuscated PHI/ITAR still detected; audit offsets correct.*
- [ ] G8 Negative-control gate for every fail-closed path above. *Gate: each proves deny on error/empty.*

## Phase P — Posture & auth hardening (M-posture)
- [ ] P1 `wsf-api` production guard: call `wsf-hardening::assert_production_ready`; refuse a non-loopback bind (or `http://` OpenBao) without a workload authority key; no silent `LocalDevAuthenticator`. *Gate: a public bind without the key refuses to start.*
- [ ] P2 `wsf-cache` clock fail-closed: a nonsensical/pre-epoch clock saturates staleness to `Expired`/`LocalOnly`. *Gate: clock rollback cannot re-open cloud egress.*
- [ ] P3 WebSocket identity: derive the connection role from the middleware-authenticated `ProfileInfo`, not the in-band handshake (AF-03 parity + regression test). *Gate: a Guest key cannot self-declare admin over WS.*
- [ ] P4 Canned-success endpoints: implement, feature-gate, or return an explicit not-implemented status (never a fabricated success) for WS inference, gRPC embeddings/stream, `scan_models`, `list_profiles`. *Gate: no wired endpoint returns indistinguishable fake success.*
- [ ] P5 Posture gate: startup-refusal + endpoint-honesty tests. *Gate: M-posture closed.*

## Phase Q — Hygiene / slop / docs (L-*)
- [ ] Q1 Scrub CANON §11 roster step-codes (`K#/H#/N#/S#/R#/VH#/SHIP-##`) from shipped `src/` (~82 files); move provenance to git/PLANNING. *Gate: zero roster codes in non-exempt source.*
- [ ] Q2 Tighten the no-slop scanner PROV pattern to catch those codes in `src/` (keeping docs/PLANNING/sessions exemptions), so the mechanical gate covers what it missed. *Gate: the scanner flags a planted `K3`/`SHIP-09` in src; passes on docs.*
- [ ] Q3 Fix dangling references: `fabric-proof/bundle.rs:75` "(finding F6-N7)", `audit/entry.rs:122` `BUILD-EXECUTION-PLAN-V2-UPDATED.md`, `aog-scheduler/filters.rs:2` `mai-scheduler`. *Gate: no comment cites a non-existent file/id/crate.*
- [ ] Q4 Doc-path drift: repoint the ~20 flat `docs/FOO.md` comments at their `docs/<category>/` homes. *Gate: every cited doc path resolves.*
- [ ] Q5 Remove the `mai-hil` crate-wide `#![allow(unused_variables, dead_code, missing_docs)]`; fix the underlying unused/dead (or feature-gate the unimplemented secure-load honestly). *Gate: `mai-hil` clippy-clean under CI flags without the blanket allow.*
- [ ] Q6 Honest heuristics: convert `requires_vision=false`, `output_tokens=input/2`, `hotswap` gpu_id-as-adapter_id, and the no-op vault `vectors` backup/restore into implemented behavior or an explicit `TODO(owner):` that does not fake success; guard operator-config integer-overflow panics (`checked_mul`/`saturating_add`). *Gate: no stub returns fake success; no operator config panics.*
- [ ] Q7 Full-tree hygiene gate. *Gate: no-slop (full) + doc-ref + clippy clean, no crate-wide allows.*

## Phase X — Migration, live validation, independent re-scan, re-ship
- [ ] X1 Full verify ladder from a clean checkout (§0.4). *Gate: all gates green cold.*
- [ ] X2 Full live suite (OpenBao + Moto + >=3-node harness); archive logs + versions. *Gate: zero mock-only trust closure.*
- [ ] X3 Failure-injection + soak (partition, revocation delay, disk full, key rotation, 72h on target hardware). *Gate: no fail-open; recovery evidenced.*
- [ ] X4 Independent security re-scan (delegated reviewers + runtime API testing). *Gate: zero Critical/High; Mediums triaged.*
- [ ] X5 Buyer/operator red-team from clean docs; re-run every audit attack. *Gate: docs alone yield a safe install; every attack denied/audited.*
- [ ] X6 Final go/no-go: signed artifacts, SBOM, `mai-ship-validate`, evidence review, written decision. *Gate: all stop-ship cleared.*
- [ ] X7 Re-ship + claim alignment (release notes, checksums, advisory, site claims match shipped controls). *Gate: external claims map 1:1 to verified controls.*

---

## Appendix A — Finding -> prompt closure matrix
| Finding | Contain | Root fix | Live proof | Final |
|---------|---------|----------|------------|-------|
| C1 | 0.2 | A1 | A6 | X4 |
| C2 | 0.2 | A2 | A6 | X4 |
| H3 | 0.2 | A3 | A6 | X4 |
| A-fence/receipts | - | A4,A5 | A6 | X4 |
| H1,H2 | 0.3 | K1,K2 | K5 | X4 |
| H4 | 0.3 | S1-S3 | X2 | X4 |
| H5,H6 | 0.4 | V1-V5 | V6 | X3/X4 |
| H7,H8 | 0.3 | U1-U4 | U5 | X4 |
| H9,H10,H11 + robustness | 0.3 | D1-D8 | D9 | X4 |
| guardrails | 0.3 | G1-G7 | G8 | X4 |
| posture | 0.2 | P1-P4 | P5 | X4/X5 |
| slop/docs | - | Q1-Q6 | Q7 | X4 |

## Appendix B — Required adversarial tests
Unauth `/admin/*` + forged raft RPC; cross-tenant/underscoped delete + tenant-spoof create;
minority-partition allow + kill under partition; attenuation widening on every axis incl.
empty-set; revoked-key `key_id` rewrite; TPM-seal recovery + tampered-field audit edit;
stripped-boundary-signature + evicted-entry tamper + >8192 clean log; `/v1/status` flood;
tiny-frame STT flood; hung-provider timeout; poisoned-lock survival; sustained-outage
breaker; session-id flood; stream-only budget bypass; HIPAA-module-disabled PHI egress;
empty-classifier-config regulated-as-public; clock-rollback cloud egress; WS role self-declare;
planted roster code / dangling ref caught by the scanner.

## Appendix C — Evidence bundle contract
Each prompt records: id + objective; pre-change failing test / static proof; changed files;
exact commands + exit codes; focused + workspace test counts; live-service versions +
endpoints; negative-control evidence; migration/compat effect; residual risks; proposed
commit scope and, after approval, the SHA. Evidence lives under
`test-evidence/full-repo-remediation/<milestone>/`.

## Appendix D — Definition of done
All checkboxes closed; DEVLOG complete; all universal gates green from a clean checkout;
the live suite green; migrations + rollback proven; the independent re-scan reports zero
Critical/High; every shipped security doc matches the code; and the owner signs the final
go/no-go. No finding is closed by documentation alone, a unit test where a live boundary
exists, or a readiness flag that does not measure the claimed runtime property.

---
*DRAFT — authorizes nothing. Execute only on an explicit "run it STS" (whole roster) or
per-milestone approval (M0..M6). CANON Parts I-II apply in full.*