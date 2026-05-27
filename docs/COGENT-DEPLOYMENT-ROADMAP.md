# MAI + Lamprey Cogent Deployment Roadmap

> **STATUS as of 2026-05-26**
> RC-01..RC-11 CLOSED. RC1.0 bundle (`dceaabc`) shipped to John Dougherty 2026-05-24 (`e2d9ea6`); DOUGHERTY remediation lane (J-01..J-26) CLOSED 2026-05-25 with all 26 sessions complete (J-23..J-26 landed under `a072634`); RC1.2 re-bundle + re-ship complete at freeze `e55c1ff` with local GitDoctor score 93/100. Two follow-up lanes are now in-flight: **GITDOCTOR-75** (GD75-01..GD75-16, sessions GD75-07/08/09/10/14/15 landed; target external rescan ≥95/100) and the loose **IGD** internal-scan lane (IGD-01/04/05/08 landed today). RC2 deployment rehearsal (RC2-01..RC2-08) is the next milestone; production-validation evidence already committed at `ee6eb13`. Latest local scan: see [`LOCAL-GITDOCTOR-EVIDENCE-2026-05-26.md`](LOCAL-GITDOCTOR-EVIDENCE-2026-05-26.md).

**Project:** Island Mountain Mighty Eel OS / MAI / Lamprey  
**Purpose:** Turn the completed Gate D codebase into one coherent, testable, and eventually shippable software product.  
**Audience:** project owner, vibe-coder operator, release engineers, testers, acquirer reviewers.  
**Status:** execution roadmap — RC-01..RC-11 CLOSED; GITDOCTOR-75 + IGD lanes in-flight; RC2 pending.  
**Last updated:** 2026-05-26.

---

## 0. Plain-English Starting Point

The project is not a random folder anymore. It is a working MAI +
Lamprey codebase with tests, docs, demos, deployment profiles, a Python
SDK, Rust services, and acquisition evidence.

The next job is not "build the product from scratch." The next job is:

1. Make a clean tester package.
2. Prove a fresh machine can run it.
3. Let qualified testers review it.
4. Harden the `ship` profile until demo defaults are impossible in
   production.
5. Build an installer or appliance-style release.
6. Run burn-in and produce release evidence.

Do not zip the full 60GB+ working folder. Most of that size is generated
Rust build output under `mai/target/debug`. That folder is rebuildable
trash from a release-packaging perspective.

---

## 1. Release Definitions

### RC1: Tester Bundle

RC1 is for trusted technical testers and acquirer reviewers.

It may include source code, docs, demo scripts, release binaries, config
examples, and test evidence. It must not pretend to be a one-click
customer installer.

### RC2: Hardened Release Candidate

RC2 is for serious deployment rehearsal.

It should start from a clean package, validate production posture, use
persistent state, and have enough runbooks for another technical person
to install, test, stop, restart, and inspect it.

### Production Appliance

The production appliance is the version that can be installed for a
regulated customer.

It requires real vault wiring, persistent audit, real trust anchors,
backup and restore, observability, service management, no demo defaults,
and a final validation command that fails closed.

---

## 2. What Not To Package

Exclude these from tester and release packages:

- `mai/target/debug/`
- `.pytest_cache/`
- `.mypy_cache/`
- `.ruff_cache/`
- temporary folders
- local generated logs
- local IDE/editor state
- stale test output unless copied intentionally into `test-evidence/`
- model weights unless creating a separate model pack

The release package should include source, docs, configs, deployment
profiles, demos, and optionally release binaries from `mai/target/release/`.

---

## 3. Session Roadmap

## Session RC-01: Inventory And Freeze Point

**Goal:** Establish the exact code snapshot that RC1 will come from.

**Work:**

- Record current git commit and branch.
- Confirm `mai/docs/acquisition/READY.md` is current.
- Confirm Gate D evidence still matches the code.
- List any dirty files and decide whether they belong in RC1.
- Create a short `RC1-FREEZE-NOTES.md` file under `mai/docs/`.

**Acceptance:**

- There is one named commit or snapshot for RC1.
- Freeze notes say what is included and what is intentionally excluded.
- No mystery local changes are silently packaged.

---

## Session RC-02: Clean Package Manifest

**Goal:** Define exactly what goes into the tester bundle.

**Work:**

- Create a package manifest listing included folders and excluded folders.
- Explicitly exclude `mai/target/debug/`.
- Decide whether RC1 includes a prebuilt `mai-api` release binary or
  expects testers to build it.
- Add a simple folder layout for `Lamprey-MAI-RC1/`.
- Create `mai/docs/RC1-PACKAGE-MANIFEST.md`.

**Acceptance:**

- A human can read the manifest and know what to copy.
- The package plan is much smaller than the 60GB+ working folder.
- Debug build artifacts are not part of the release package.

---

## Session RC-03: Build Release Binary

**Goal:** Produce the smallest practical runnable API artifact.

**Work:**

- Run a release build for `mai-api`.
- Confirm the release binary exists under `mai/target/release/`.
- Record binary size and build environment.
- Run the API locally from the release binary.
- Capture the first-boot key instructions in human language.

**Acceptance:**

- `mai-api` release binary starts locally.
- Basic health endpoint responds.
- Build notes are written in `test-evidence/` or `mai/docs/RC1-BUILD-NOTES.md`.

---

## Session RC-04: Beginner Quickstart

**Goal:** Make a tester-facing "start here" document that assumes no
prior deployment experience.

**Work:**

- Create `README-FIRST.md` for the RC1 package.
- Explain what the software is and is not.
- Add minimum hardware requirements.
- Add exact first-run steps.
- Add exact demo steps.
- Add "what success looks like" after each step.
- Add "what to send back if it fails."

**Acceptance:**

- A technical but new tester can follow the document without reading the
  whole repo.
- The document does not claim production readiness beyond the current
  state.

---

## Session RC-05: Test Evidence Refresh

**Goal:** Re-run the evidence that proves RC1 works.

**Work:**

- Run Rust workspace tests.
- Run compliance demo tests.
- Run Python SDK tests.
- Run dashboard tests.
- Run scaffold tests if time permits.
- Save logs under a clean evidence folder.
- Summarize pass/fail status in one markdown file.

**Acceptance:**

- `RC1-TEST-EVIDENCE.md` exists.
- It lists exact commands, dates, machine, and results.
- Any failure is documented as a blocker or known deferral.

---

## Session RC-06: Fresh Machine Rehearsal

**Goal:** Prove the package works outside the original development
folder.

**Work:**

- Copy the RC1 package to a clean directory or clean machine.
- Run the quickstart from scratch.
- Start the API.
- Run at least one Trust Manifold dry-run.
- Run at least one compliance demo test.
- Document every missing dependency or confusing step.

**Acceptance:**

- Fresh-machine notes exist.
- Quickstart is updated based on what actually happened.
- RC1 is not considered ready for outside testers until this passes.

---

## Session RC-07: Tester Instructions And Issue Form

**Goal:** Give testers a clear job, not a pile of files.

**Work:**

- Create `TESTER-INSTRUCTIONS.md`.
- Define three tester tracks:
  - local smoke tester
  - technical build/test reviewer
  - security/compliance reviewer
- Add expected hardware per track.
- Add what each tester should run.
- Add a simple issue report template.

**Acceptance:**

- Testers know what role they are playing.
- Testers know what hardware they need.
- Testers know what output to send back.

---

## Session RC-08: Create RC1 Bundle

**Goal:** Assemble the actual clean RC1 folder.

**Work:**

- Create `Lamprey-MAI-RC1/` outside the working tree or under a release
  staging directory.
- Copy only files listed in the package manifest.
- Include docs, source, deployment profiles, configs, demos, scripts,
  and optional release binary.
- Exclude debug artifacts and cache folders.
- Generate checksums for important files.
- Compress the RC1 folder only after verifying its contents.

**Acceptance:**

- RC1 bundle exists.
- Bundle size is explainable.
- Bundle contents match the manifest.
- There is no accidental `target/debug/` inside it.

---

## Session RC-09: First Outside Tester

**Goal:** Have one trusted technical person test RC1.

**Work:**

- Send RC1 to one tester, not a crowd.
- Ask them to follow `README-FIRST.md` and `TESTER-INSTRUCTIONS.md`.
- Collect their environment details.
- Collect failures, screenshots, logs, and confusion points.
- Triage issues into docs, packaging, code, and environment buckets.

**Acceptance:**

- At least one person besides the original builder has tried RC1.
- Feedback is captured in `RC1-TESTER-FEEDBACK.md`.
- Blockers are known before wider sharing.

---

## Session RC-10: RC1 Fix Pass

**Goal:** Fix the first wave of packaging and documentation problems.

**Work:**

- Patch quickstart docs.
- Patch package manifest.
- Fix small startup or config problems found by the first tester.
- Rebuild release binary if code changed.
- Re-run focused tests.
- Produce RC1.1 if needed.

**Acceptance:**

- First-tester blockers are resolved or explicitly deferred.
- Updated package has a clear version label.
- Test evidence is refreshed if code changed.

---

## Session SHIP-01: Production Posture Audit

**Goal:** Reconcile the existing `SHIP-HARDENING-PLAN.md` with the
current code state.

**Work:**

- Read `mai/docs/SHIP-HARDENING-PLAN.md`.
- Read `mai/docs/SHIP-PROFILE.md`.
- Confirm which SHIP sessions are already complete.
- Create a live checklist of remaining production blockers.
- Confirm `deployment/ship/profile.toml` still represents the desired
  production posture.

**Acceptance:**

- There is a current production-blocker checklist.
- Remaining SHIP sessions are clearly scoped.
- The team knows the difference between RC testing and production
  readiness.

---

## Session SHIP-02: Production Startup Guard Completion

**Goal:** Make production startup fail closed when unsafe defaults are
present.

**Work:**

- Ensure `ship` mode rejects demo token exchange.
- Ensure `ship` mode rejects accept-all trust verification.
- Ensure `ship` mode rejects memory-only audit storage.
- Ensure `ship` mode rejects stub vaults.
- Ensure validation output explains every failure.

**Acceptance:**

- `ship` cannot boot with demo-safe defaults.
- Validation has machine-readable and human-readable output.
- Tests cover unsafe production shapes.

---

## Session SHIP-03: Persistent Vault Wiring

**Goal:** Replace demo vault behavior with real production storage.

**Work:**

- Wire the configured vault backend into production startup.
- Require persistent vault root.
- Require sealed master material behavior.
- Add restart tests.
- Document first-boot and recovery behavior.

**Acceptance:**

- Production startup never constructs `StubVault`.
- Vault state survives restart.
- Operator docs explain initialization and recovery.

---

## Session SHIP-04: Persistent API Audit — **done 2026-05-23**

**Status:** Complete (builder commit `40108db`; convergence wiring `48c7d2e`; acceptance-test fixup `88cdb87`).

**Goal:** Make API audit entries survive restart and verify cleanly.

**What landed:**

- `mai-api/src/audit_wal.rs` — `WalAuditWriter` implements the existing `AuditWriter` trait against an append-only JSON-lines log under `audit.wal_dir`. `open()` replays and verifies the BLAKE3 hash chain; size-based rotation preserves chain continuity; retention metadata defaults to 7 years (HIPAA-aligned).
- SHIP-07 convergence wires the writer into `MaiServer::run()` whenever `MAI_SHIP_PROFILE` is set. `MemoryAuditWriter` is now the test/dev fallback only.
- 17 tests (10 unit + 7 integration in `mai-api/tests/audit_wal.rs`) cover survives-restart, chain-verifies-after-restart, chain-verifies-across-rotation, tampered-WAL-fails-open, missing-WAL-dir-fails-open, audit-write-failure-surfaces-error.
- Production guard ID `PROD-AUDIT-100` flips Deferred → Pass at runtime via `RuntimeChecks::api_audit_wal_ready`.

**Carried:** Operator-facing audit retention runbook lands in SHIP-15.

---

## Session SHIP-05: Persistent Compliance Audit And Sealing — **done 2026-05-23**

**Status:** Complete (builder commit `40108db`; convergence wiring `48c7d2e`).

**Goal:** Remove `NullSealer` from production compliance audit storage.

**What landed:**

- `mai-compliance/src/audit/sealer.rs` — `AeadSealer` (AES-256-GCM, 12-byte random nonce, `nonce || ciphertext || tag` framing) implements the `StoreSealer` trait; `Debug` impl never leaks key material.
- `mai-api/src/sealer_builder.rs` — `build_sealer` selects `AeadSealer` from `<audit.wal_dir>/sealer.key` in production (32-byte key file required), errors `NullSealerAllowedInProduction` / `KeyFileMissing` / `KeyFileIo` / `KeyFileLengthInvalid` cover the production rejection paths; local-dev uses `NullSealer` only when `allow_null_sealer=true`, else ephemeral `AeadSealer`.
- SHIP-07 convergence installs the sealer-backed `ComplianceAuditLog` via `AppState::with_compliance_audit`.
- 12 tests (5 unit + 7 integration in `mai-api/tests/sealer_bootstrap.rs`) cover round-trip, nonce uniqueness, wrong-key rejection, length validation, and Debug leak.
- Production guard ID `PROD-AUDIT-101` flips Deferred → Pass at runtime via `RuntimeChecks::compliance_sealer_real`.

**Carried:** Vault-managed key acquisition (currently the key file is the bring-up contract) ladders into SHIP-08 packaging.

---

## Session SHIP-06: Trust Bridge Production Mode — **done 2026-05-23**

**Status:** Complete (builder commit `5d6aebf`; convergence wiring `48c7d2e`).

**Goal:** Ensure production trust material is real, verified, and available at boot.

**What landed:**

- `mai-api/src/trust_builder.rs` — `build_trust_components` rejects every demo shortcut in production: `AcceptAllInProduction`, `AcceptAllAllowedInProduction`, `LocalDevExchangeInProduction`, `TrustAnchorNotRequired`, `BootBundleNotRequired`, plus anchors-dir / anchor-file failure modes. `TrustExchangeMode` enum (`LocalDevSynthetic` / `OpenBaoBridge` / `Disabled`) selects the production token-exchange shape.
- `verify_boot_bundle` loads `<bundle_cache_dir>/bundle.json` as a `SignedPolicyBundle` and verifies window + ML-DSA signature against the loaded anchors. Failure surfaces as `ServerError::Init` in `MaiServer::run()`.
- SHIP-07 convergence calls `verify_boot_bundle` in production mode before the readiness gate and installs the resulting verifier via `AppState::with_bundle_verifier`.
- 27 tests (2 unit + 25 integration in `mai-api/tests/trust_production.rs`).
- Production guard ID `PROD-TRUST-100` flips Deferred → Pass at runtime via `RuntimeChecks::trust_bundle_verified` (the runtime detail names the verified bundle version + anchor count).

**Carried:** Profile-aware `handlers/trust.rs::exchange_token` switch on `TrustExchangeMode` — currently the handler still always mints synthetic. Lands in SHIP-07-endpoint-and-cli.

---

## Session SHIP-07: Production Validation Command

The original session description had two distinct deliverables; in execution they split into two slices.

### Slice A — Bootstrap convergence + runtime guard wiring — **done 2026-05-23**

**Status:** Complete (commit `48c7d2e`).

**What landed:**

- `MaiServer::with_ship_profile(path)` + `MAI_SHIP_PROFILE` env-var fallback. `MaiServer::run()` branches on the resolved profile: when set, `build_vault` / `WalAuditWriter::open` / `build_sealer` / `build_trust_components` replace the demo defaults; `verify_boot_bundle` runs before the readiness gate in production.
- New public types `RuntimeChecks` + `RuntimeOutcome` and `ProductionReadinessReport::evaluate_with_runtime` flip the six runtime check IDs (`PROD-VAULT-100`, `PROD-AUDIT-100`, `PROD-AUDIT-101`, `PROD-TRUST-100`, `PROD-AUTH-100`, `PROD-POLICY-001`) from Deferred to Pass / Fail at startup.
- The server returns `ServerError::Init` (with the rendered report) on any Critical Fail and never reaches `bind()`.
- 4 new integration tests in `mai-api/tests/ship_convergence.rs` + 5 new unit tests in `mai-api/src/production_guard.rs`.

### Slice B — Readiness endpoint + standalone CLI — **pending**

**Goal:** Provide one command (and one URL) that answers whether a node is shippable, without inspecting source.

**Work:**

- `GET /v1/system/production-readiness` admin route returning `ProductionReadinessReport::to_json()` over the same introspection collected inside `apply_ship_profile`.
- Standalone `mai-ship-validate` binary accepting `--profile <PATH>` (and optional `--state-dir <PATH>` so the runtime checks can be exercised offline) and emitting human + JSON output with the exit codes from `mai/docs/SHIP-HARDENING-PLAN.md` §13.
- Profile-aware `handlers/trust.rs::exchange_token` switching on the `TrustExchangeMode` collected in `apply_ship_profile` (return 404/410 on `Disabled`, forward to OpenBao on `OpenBaoBridge`, mint synthetic only on `LocalDevSynthetic`).

**Acceptance:**

- Validation exits non-zero for every unsafe production state.
- Validation passes on a correctly staged production node.
- Operators do not need to inspect source code to know readiness.

---

## Session SHIP-08: Service And Installer Layout

**Goal:** Convert "run from repo" into an installable service shape.

**Work:**

- Define Linux directory layout:
  - `/etc/mai`
  - `/var/lib/mai`
  - `/var/log/mai`
  - `/run/mai`
  - `/var/backups/mai`
- Add service unit for `mai-api`.
- Add environment file template.
- Add install and uninstall scripts.
- Add upgrade-safe config behavior.

**Acceptance:**

- A technical operator can install the service.
- Service starts, stops, restarts, and logs predictably.
- Config and state are not mixed with source code.

---

## Session SHIP-09: Dashboard Production Readiness

**Goal:** Make the dashboard safe and useful for real operators.

**Work:**

- Reject default dashboard admin token in production.
- Document dashboard auth setup.
- Verify all dashboard routes against a live server.
- Add browser walkthrough evidence.
- Add screenshots or notes for operator-critical pages.

**Acceptance:**

- Dashboard cannot run with demo credentials in `ship`.
- Browser walkthrough passes.
- Operator docs identify what each important screen means.

---

## Session SHIP-10: Backup And Restore (done 2026-05-23, commit `0fe5f59`)

**Goal:** Prove a node can lose state and recover safely.

**Delivered:**

- `tools/mai-admin/src/restore.rs` (740 lines) implements `plan_restore`
  / `apply_restore` as a two-phase pipeline:
  - **Plan** is read-only: loads `manifest.json`, verifies ML-DSA-87
    signature (when a 2592-byte pubkey is supplied), recomputes every
    component sha3 against the backup-side files, replays the audit
    chain on WAL components and cross-checks against
    `ManifestComponent.last_entry_hash`. All source-side verification
    runs *before* the obstacle scan so a corrupt backup cannot ever
    touch the target.
  - **Apply** refuses populated targets without `--force` per §9.5,
    recomputes per-component sha3 after every write (catches in-flight
    corruption that bypassed the source check), replays the WAL chain
    in the restored tree, and drops `source-manifest.json` +
    `restore-report.json` as audit witnesses at the target root.
- `mai-admin restore plan --backup-dir <DIR> --target <DIR>
  [--verifying-key <PATH>] [--require-signed] [--json]` and
  `mai-admin restore apply [--force] …` CLI subcommands; §13 exit
  codes (0 ok / 1 verification failed / 2 inputs unreadable / 3
  manifest missing / 4 internal serializer error).
- Integration suite `tools/mai-admin/tests/restore_e2e.rs` (20 tests)
  including the §9.5 DR drills end-to-end: WAL tamper, missing trust
  bundle, missing model registry, signed-manifest tamper. Each drill
  asserts the target stays empty after a failed plan. Two round-trip
  drills prove the restored tree is byte-faithful and re-verifies
  clean.

**Acceptance:**

- [x] Backup artifact can restore a fresh node (`apply_unsigned_into_empty_target_round_trips`).
- [x] Restored node passes audit-chain replay (`restored_tree_passes_audit_chain_replay`).
- [x] Restored tree re-backs-up to byte-identical state (`restored_tree_re_backs_up_to_byte_identical_state`).
- [x] Tampered backups never reach the target (4 DR drill tests).
- [ ] Restored node passes `mai-ship-validate --state-dir <target>` end-to-end — operator-driven step that exercises the SHIP-07-endpoint-and-cli binary against a restored tree; documented in the SHIP-15 operator runbook lane.

**Carried forward:**

- 72-hour burn-in restore-during-load drill → SHIP-14.
- Operator restore runbook → SHIP-15.
- CI nightly that exercises `backup create` → `restore plan/apply`
  → re-`backup verify` → SHIP-12.

---

## Session SHIP-11: Observability And Alerts

**Goal:** Make the system visible to operators.

**Work:**

- Enable structured JSON logs.
- Add log rotation.
- Confirm metrics endpoint.
- Add alert rules for vault unavailable, audit failure, trust bundle
  stale, scheduler unavailable, adapter crash loop, disk capacity, rate
  limit abuse, and air-gap violation.
- Document expected monitoring setup.

**Acceptance:**

- Operators can see health, metrics, logs, and alerts.
- Production validation checks observability configuration.
- Alert examples are included in release docs.

---

## Session SHIP-12: Model And Adapter Deployment

**Goal:** Make model backend setup repeatable.

**Work:**

- Choose supported starter backends for production testing.
- Document model placement and adapter startup.
- Add a no-GPU path for basic validation.
- Add a GPU path for real inference validation.
- Confirm scheduler reports healthy backend state.

**Acceptance:**

- A tester knows how to run without a GPU.
- A serious tester knows how to run with a GPU.
- Model backend state appears in health and metrics.

---

## Session SHIP-13: Security Review Pass

**Goal:** Review the product like a buyer or regulated customer would.

**Work:**

- Review auth boundaries.
- Review trust boundary claims.
- Review audit-chain guarantees.
- Review HIPAA, ITAR/EAR, and OCAP policy behavior.
- Review secrets handling.
- Review default configs.
- Produce a security findings document.

**Acceptance:**

- Findings are documented with severity.
- Release blockers are fixed or explicitly deferred.
- No known critical security blocker remains for production appliance
  testing.

---

## Session SHIP-14: Burn-In

**Goal:** Prove the system survives time, restarts, and load.

**Work:**

- Run no-GPU burn-in.
- Run GPU burn-in on target hardware.
- Run restart tests.
- Run audit append and verify loops.
- Run trust bundle refresh and degraded-mode checks.
- Run compliance report generation under load.
- Collect logs, metrics, and results.

**Acceptance:**

- Burn-in duration and hardware are recorded.
- Failures are triaged.
- Passing burn-in evidence is attached to the release package.

---

## Session SHIP-15: Production Runbooks

**Goal:** Give operators the documents they need when something goes
wrong at 2 AM.

**Work:**

- Write install runbook.
- Write first-boot runbook.
- Write restart runbook.
- Write backup and restore runbook.
- Write audit verification runbook.
- Write trust bundle recovery runbook.
- Write dashboard operations runbook.
- Write support bundle collection instructions.

**Acceptance:**

- Operator docs are complete enough for a new technical operator.
- Runbooks match actual commands and file paths.
- Support bundle avoids leaking regulated payloads.

---

## Session SHIP-16: Final Production Gate

**Goal:** Decide whether this is a shippable production appliance.

**Work:**

- Build final release artifact.
- Run full validation.
- Run full tests.
- Run burn-in evidence check.
- Run security blocker check.
- Run restore drill check.
- Generate checksums.
- Generate final release notes.
- Produce go/no-go decision.

**Acceptance:**

- Final artifact exists.
- `ship` validation passes.
- Known deferrals are honest and non-blocking.
- Release notes say exactly what is ready and what is not.

---

## 4. Tester Types And Hardware

### Local Smoke Tester

**Purpose:** Confirm package starts and docs are followable.

**Needs:**

- Windows 11 or Linux
- 16GB RAM minimum
- 32GB RAM preferred
- Rust and Python if building from source
- No GPU required

### Technical Build Tester

**Purpose:** Run tests and inspect failures.

**Needs:**

- Windows 11 or Linux
- 32GB RAM preferred
- Rust toolchain
- Python
- Enough disk for release build output
- No GPU required for most tests

### AI Infrastructure Tester

**Purpose:** Connect real model backends and test inference behavior.

**Needs:**

- Linux preferred
- 32GB to 128GB RAM
- NVIDIA GPU recommended
- 12GB VRAM minimum for small local models
- 24GB+ VRAM preferred
- Fast NVMe storage

### Security And Compliance Reviewer

**Purpose:** Review trust boundary, policy behavior, audit chain,
regulated-data posture, and production hardening.

**Needs:**

- Source package
- Docs package
- Test evidence
- Optional running node
- OpenBao or equivalent only if testing live Trust Bridge behavior

---

## 5. Packaging Rule Of Thumb

If it can be regenerated by a build command, do not package it unless it
is the specific release binary you intend testers to run.

Package the clean story:

- source
- release binary if available
- docs
- configs
- deployment profiles
- demos
- test evidence
- checksums
- known issues
- quickstart

Do not package the entire working directory.

---

## 6. Immediate Next Move

Start with RC-01.

Do not begin production hardening by changing random code. First create
the freeze point, package manifest, quickstart, and tester instructions.
Once a fresh machine can run RC1, the project has crossed from "works on
the builder's machine" to "another human can evaluate this."

