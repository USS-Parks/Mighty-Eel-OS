# Lamprey Saddle WSF + AOG Hardening DEVLOG

Initiative: close the 2026-07-15 WSF/AOG workflow findings and complete the interrupted high-risk review.  
Repository: `im-mighty-eel-mai`.  
Worktree: `mai-worktrees/mai-LSH-1`; branch `session/LSH-1`.  
Plan of record: [`../../../PLANNING/LAMPREY-SADDLE-WSF-AOG-SECURITY-HARDENING-PSPR.md`](../../../PLANNING/LAMPREY-SADDLE-WSF-AOG-SECURITY-HARDENING-PSPR.md).  
Finding register: [`../scans/LAMPREY-SADDLE-HARDENING-FINDINGS.json`](../scans/LAMPREY-SADDLE-HARDENING-FINDINGS.json).  
Evidence root: `test-evidence/lamprey-saddle-hardening/`.

Every entry records the vulnerable path/invariant, legitimate behavior, changed files, focused and milestone gates, residual risk, and commit state. A prompt is not complete until its named gate passes. A narrowed pass is never reported as a full pass.

---

## M0 — Contained and reproducible

### LSH-00 — Execution lane bootstrap and drift check

Status: **PASS** (commit pending M0 checkpoint).

Objective: create an isolated execution lane at the exact assessed revision and preserve the interim scan evidence before implementation.

Source identity:

- HEAD: `361f70b272c0fbee6375f462c912bd8d5b5891bb`.
- Scan: `361f70b_20260715T130144Z`.
- Snapshot: `codex-security-snapshot/v1:sha256:2f504c2504ea119582f8981b2fa67c948906810eeab7a61cec73e74596695e80`.
- Fresh worktree status before lane writes: clean.
- Untracked `.opencode/` exists only in the original working tree and was preserved; it is absent from this isolated worktree.

Toolchain:

- `rustc 1.96.1 (31fca3adb 2026-06-26)`;
- `cargo 1.96.1 (356927216 2026-06-26)`;
- `git 2.54.0.windows.1`;
- Docker client `29.6.1`;
- `cargo-audit 0.22.1`; and
- `cargo-deny 0.19.7`.

Environment limitations recorded honestly:

- `Session-Worktree.ps1` attempted `git fetch origin --quiet`, but Windows Schannel returned `SEC_E_NO_CREDENTIALS`. This did not affect source identity because the exact assessed commit was already available locally and the worktree was created from that immutable revision.
- Docker client is installed, but reading the user Docker config returned access denied; live-engine availability is not claimed until the relevant live gate runs.
- Git cannot read the user's global ignore file in this sandbox; repository status still resolves and no global configuration was changed.

Evidence frozen under `test-evidence/lamprey-saddle-hardening/M0/source-scan/`:

- 81 candidate ledgers;
- nine completed file-review bundles;
- 141-row selected-file worklist;
- coverage ledger; and
- repository threat model.

Gate result: tracked revision matches the assessment; no relevant source drift exists. LSH-00 is complete.

### LSH-01 — Canonical finding and regression registry

Status: **PASS** (commit pending M0 checkpoint).

The machine-readable register imports all 81 raw instances through the frozen evidence root and maps them exactly once to 29 confirmed families or 10 deferred families. Each confirmed family has a stable regression ID and prompt owner; each deferred family has a reachability question and prompt owner.

`SECURITY-REMEDIATION-PSPR.md` and `docs/INDEX.md` now state the historical truth: the older lane's DEVLOG records execution, while its unchecked roster is preserved; the current Lamprey Saddle lane owns closure.

Gate: `python .integrity/scripts/lamprey-finding-register-check.py` PASS — 81 raw instances, 29 confirmed families, 10 deferred families, and 81 candidate ledgers reconcile exactly.

### LSH-02 — Immediate production containment

Status: **PASS** (commit pending M0 checkpoint).

Containment now fails before listener bind at the production startup seams:

- `aogd` defaults `AOGD_PROFILE` to production, requires trust, and refuses production while admin authorization and Raft peer mTLS are not yet actually wired. `AOGD_ALLOW_INSECURE_BIND=1` has no effect in production. The Loom harness must declare `AOGD_PROFILE=development` and then separately opt into its isolated non-loopback bind.
- `wsf-api` defaults `WSF_PROFILE` to production and requires hardened OpenBao material, workload authentication, and a mandatory revocation store even on loopback. Because the binary does not yet wire the shared revocation store, production remains intentionally unstartable until `LSH-W1`; appliance/shadow demos explicitly declare development plus the isolated-network bind opt-in.
- `aog-gateway` validates mandatory production revocation and provider endpoints before constructing OpenBao or binding. Credentialed production providers require HTTPS; plaintext local providers are confined to loopback; the default listener and local backend are loopback; provider redirects are disabled so credentials cannot follow a redirect.
- `deployment/wsf-ha` now states `WSF_PROFILE=production` explicitly. It remains contained until W1 supplies the mandatory revocation dependency; this is intentional and is not reported as production-ready.

Changed product/deployment files:

- `crates/aogd/src/main.rs`;
- `crates/wsf-api/src/main.rs`, `crates/wsf-api/src/posture.rs`;
- `crates/aog-gateway/src/lib.rs`, `main.rs`, `posture.rs`, `provider.rs`; and
- appliance, shadow, WSF-HA, Loom Compose, and Loom k3s manifests.

Focused gates:

- `cargo test -p aogd --bin aogd` — PASS, 4/4;
- `cargo test -p wsf-api posture` — PASS, 8/8;
- `cargo test -p aog-gateway posture` — PASS, 5/5; and
- `cargo clippy -p aogd -p wsf-api -p aog-gateway --all-targets -- -D warnings -A clippy::pedantic` — PASS.

Legitimate behavior retained: development harnesses still run, but only after an explicit development profile; non-loopback insecure harness binds require a second explicit opt-in. Production omission always resolves to the fail-closed profile.

Residual risk: the underlying admin authorization, Raft mTLS, and mandatory shared WSF revocation implementations are not falsely marked fixed. Production is deliberately unavailable at those seams until C1/C2/C3 and W1 replace containment with real controls.

### LSH-03 — Baseline and adversarial fixture freeze

Status: **PASS** (commit pending M0 checkpoint).

`test-evidence/lamprey-saddle-hardening/M0/regression-plan.json` maps all 29 confirmed families to a unique canonical regression ID, narrow boundary, fixture, execution mode, vulnerable red condition, and repaired green condition. Destructive mutation and external-state PoCs are request-fixture-only until an owning prompt creates disposable isolated state. All 10 deferred families have argv-form read-only `rg` reachability questions.

Gates:

- `python .integrity/scripts/lamprey-regression-plan-check.py --run-reachability` — PASS: 29 confirmed plans, 10 deferred executable questions; nine queries returned matches and `LSD-009` returned no matches, which is preserved as a reachability result rather than coerced into a finding.
- Initial `cargo test --workspace` — ENVIRONMENT BLOCKED after all preceding tests passed: two `aog-wire/tests/mtls.rs` setup failures reported `openssl on PATH: program not found`.
- Located existing prerequisite at `C:\Program Files\Git\usr\bin\openssl.exe`; no installation or persistent system change was made.
- `cargo test -p aog-wire --test mtls` with that directory prepended to the command-local `PATH` — PASS, 2/2.
- Full `cargo test --workspace` rerun with the same command-local prerequisite — PASS, exit 0, including all workspace and doctest lanes (repository-declared ignored/nightly/SLO tests remained ignored by the standard command).

M0 acceptance: **PASS**. Containment is active, all frozen evidence and plans are machine-reproducible, focused lint/tests pass, and the standard full workspace gate passes when its existing OpenSSL prerequisite is supplied. No files are staged and no commit exists; M0 is now stopped at its mandatory commit-authorization checkpoint.
