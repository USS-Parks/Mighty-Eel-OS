# MAI + Lamprey Cogent Deployment Roadmap

**Project:** Island Mountain Mighty Eel OS / MAI / Lamprey  
**Purpose:** Turn the completed Gate D codebase into one coherent, testable, and eventually shippable software product.  
**Audience:** project owner, vibe-coder operator, release engineers, testers, acquirer reviewers.  
**Status:** execution roadmap.  
**Last updated:** 2026-05-23.

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
- Add a simple folder layout for `MAI-Lamprey-RC1/`.
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

- Create `MAI-Lamprey-RC1/` outside the working tree or under a release
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

## Session SHIP-04: Persistent API Audit

**Goal:** Make API audit entries survive restart and verify cleanly.

**Work:**

- Wire WAL audit writer into production API startup.
- Reject `MemoryAuditWriter` in `ship`.
- Add append, replay, rotation, and corruption tests.
- Add operator notes for audit log location and retention.

**Acceptance:**

- API audit survives restart.
- Corruption is detected.
- Production validation fails if WAL is unavailable.

---

## Session SHIP-05: Persistent Compliance Audit And Sealing

**Goal:** Remove `NullSealer` from production compliance audit storage.

**Work:**

- Wire vault-backed AEAD sealing for compliance WAL records.
- Require hash-chain verification.
- Require PQC checkpoint signing where configured.
- Add restart and tamper tests.

**Acceptance:**

- Production compliance audit is persistent and sealed.
- Tampering is detected.
- `ship` validation rejects `NullSealer`.

---

## Session SHIP-06: Trust Bridge Production Mode

**Goal:** Ensure production trust material is real, verified, and
available at boot.

**Work:**

- Reject synthetic local-dev exchange in production.
- Require trust anchor directory.
- Require verified bundle on boot.
- Verify expired, unsigned, and tenant-mismatched bundles fail closed.
- Document OpenBao or Trust Bridge integration requirements.

**Acceptance:**

- Production trust cannot silently fall back to demo mode.
- Bundle failures block unsafe startup.
- Trust docs match actual startup behavior.

---

## Session SHIP-07: Production Validation Command

**Goal:** Provide one command that answers whether a node is shippable.

**Work:**

- Finalize `mai-ship-validate` or equivalent command.
- Include checks for config, vault, audit, trust, auth, dashboard,
  network, observability, paths, and permissions.
- Add JSON output for automation.
- Add plain-English output for operators.

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

## Session SHIP-10: Backup And Restore

**Goal:** Prove a node can lose state and recover safely.

**Work:**

- Implement or finalize backup command.
- Include vault, audit WAL, compliance WAL, trust cache, model registry,
  reports, and auth config.
- Implement restore procedure.
- Run restore onto a fresh node directory.
- Verify audit chain and trust bundle after restore.

**Acceptance:**

- Backup artifact can restore a fresh node.
- Restored node passes production validation.
- Restore drill is documented.

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

