# RC2 Hardened Release Candidate — Session Plan

> **STATUS as of 2026-05-26 — IN-FLIGHT (production-validation evidence committed)**
> Predecessor lanes done: SHIP-HARDENING (closed 2026-05-23), DOUGHERTY (closed 2026-05-25 via `a072634`), RC1.2 re-ship (closed 2026-05-25 at freeze `e55c1ff`). RC2 production validation evidence committed at `ee6eb13` ([`RC2-PRODUCTION-VALIDATION.md`](RC2-PRODUCTION-VALIDATION.md)) — labelled "all gates GO." Concurrent GITDOCTOR-75 + IGD lanes are still in-flight and may push origin/main past `e55c1ff` before RC2 starts a clean-install rehearsal (RC2-01); RC2 itself remains anchored to a future re-frozen commit, not `e55c1ff` verbatim.

**Project:** Lamprey MAI
**Phase:** RC2 Deployment Rehearsal
**Predecessor:** RC1.2 (re-bundled, DOUGHERTY closed)
**Freeze:** e55c1ff (Memorial Day 2026-05-25) — RC1.2 reference; RC2 will rebase to a post-GITDOCTOR-75 freeze
**Sessions:** RC2-01 .. RC2-07
**Audience:** release engineers, deployment operators

---

## RC2 Definition

RC2 is the Hardened Release Candidate. Per docs/COGENT-DEPLOYMENT-ROADMAP.md:

> RC2 is for serious deployment rehearsal. It should start from a clean package,
> validate production posture, use persistent state, and have enough runbooks for
> another technical person to install, test, stop, restart, and inspect it.

All SHIP-01..SHIP-17 hardening work is complete. RC2 takes that hardened code
and proves it works outside the development environment.

---

## Session Roster

| Session | Title | Depends On | Effort |
|---|---|---|---|
| RC2-01 | Clean-install deployment rehearsal | — | M |
| RC2-02 | Production posture validation (vault, audit, trust) | RC2-01 | L |
| RC2-03 | Service management + observability wiring | RC2-01 | M |
| RC2-04 | Backup/restore drill in production profile | RC2-02 | M |
| RC2-05 | 72-hour burn-in with production posture | RC2-02, RC2-04 | L |
| RC2-06 | Operator runbook finalisation | RC2-01..RC2-04 | M |
| RC2-07 | Final production gate + go/no-go | RC2-01..RC2-06 | M |

---

## Session RC2-01: Clean-Install Deployment Rehearsal

**Goal:** Prove a clean machine can install, start, and run the Lamprey MAI from
the release package without source-code access.

**Deliverables:**
1. Create Lamprey-MAI-RC2/ staging directory with clean package layout
2. Copy release binaries (lamprey-mai-api.exe, lamprey-mai.exe, lamprey-mai-admin.exe, lamprey-mai-ship-validate.exe)
3. Copy ship profile template, config templates, .env.example
4. Copy operator quickstart: first-boot, health check, demo run
5. Copy runbook stubs for install, start, stop, restart, inspect
6. Regenerate CHECKSUMS.txt for the RC2 bundle
7. Produce RC2-INSTALL-NOTES.md with step-by-step fresh-machine walkthrough
8. Verify: install, start, health-check, stop, restart, inspect on same machine

**Acceptance:**
- [ ] RC2 package exists at Lamprey-MAI-RC2/
- [ ] Fresh-machine install instructions are complete and tested
- [ ] lamprey-mai-api starts from release binary
- [ ] GET /v1/health/system returns healthy
- [ ] Stop/restart cycle preserves state
- [ ] lamprey-mai-ship-validate runs and reports

---

## Session RC2-02: Production Posture Validation

**Goal:** Confirm production-mode wiring (vault, audit WAL, compliance sealer, trust bundle) works end-to-end outside the development tree.

**Deliverables:**
1. Configure MAI_SHIP_PROFILE pointing to a production profile.toml
2. Verify MaiServer::run() constructs real vault (not StubVault)
3. Verify WAL audit writer opens and chains correctly
4. Verify AeadSealer is active (not NullSealer)
5. Verify trust bundle ML-DSA verification runs at boot
6. Verify the server refuses to bind on Critical Fail
7. Run lamprey-mai-ship-validate --profile <PATH> and confirm zero Critical Fail
8. Produce RC2-PRODUCTION-POSTURE.md evidence document

**Acceptance:**
- [ ] Real vault constructed on production boot
- [ ] WAL audit chain verifies across restart
- [ ] AeadSealer round-trips correctly
- [ ] Trust bundle verification passes
- [ ] Ship validator exits 0 on clean state
- [ ] Ship validator exits non-zero on tampered state (negative test)

---

## Session RC2-03: Service Management + Observability

**Goal:** Wire structured logging, metrics, health endpoints, and service lifecycle into production operation.

**Deliverables:**
1. Confirm structured JSON logging is active in production mode
2. Verify log rotation configuration
3. Verify GET /metrics (or equivalent metrics endpoint)
4. Verify GET /v1/health/system aggregator
5. Verify alert rules are documented (vault unavailable, audit failure, trust stale, scheduler down, adapter crash loop, disk, rate limit, air-gap violation)
6. Verify admin CLI can query health/metrics
7. Produce RC2-OBSERVABILITY.md

**Acceptance:**
- [ ] JSON log output verified
- [ ] Metrics endpoint reachable
- [ ] Health aggregator fans out to all adapters
- [ ] Admin CLI reports system status
- [ ] Observability runbook is complete

---

## Session RC2-04: Backup/Restore Drill (Production Profile)

**Goal:** Prove a production-mode node can be backed up, destroyed, restored, and re-verified.

**Deliverables:**
1. Create a backup manifest of production state (vault, audit, trust, config)
2. Destroy the staging deployment directory
3. Restore from backup using lamprey-mai-admin restore plan/apply
4. Verify restored node passes audit-chain replay
5. Verify restored node passes lamprey-mai-ship-validate
6. Run tampered-backup negative test (corrupted manifest, tampered WAL, missing trust bundle)
7. Produce RC2-BACKUP-RESTORE.md

**Acceptance:**
- [ ] Backup manifest verifies clean
- [ ] Restore produces byte-faithful tree
- [ ] Restored node passes ship-validate
- [ ] Tampered backups never reach target
- [ ] Backup/restore runbook is complete

---

## Session RC2-05: 72-Hour Burn-In (Production Profile)

**Goal:** Prove the production-mode system survives extended runtime, load, and restarts.

**Deliverables:**
1. Run 72-hour burn-in driver (mai/.integrity/scripts/burn-in-72h.sh)
2. Run under production profile with real vault and audit WAL
3. Exercise: restart loop, audit append, compliance report generation, health polling
4. Collect logs, metrics, and burn-in report
5. Verify ML-DSA-87 signed burn-in report
6. Triaged any failures
7. Produce RC2-BURN-IN-REPORT.md

**Acceptance:**
- [ ] 72-hour burn-in completes without Critical Fail
- [ ] Burn-in report is signed (ML-DSA-87)
- [ ] All failures triaged and dispositioned
- [ ] Burn-in evidence attached to release package

---

## Session RC2-06: Operator Runbook Finalisation

**Goal:** Complete all operator-facing documentation for a production deployment.

**Deliverables:**
1. Install runbook (final)
2. First-boot runbook
3. Restart runbook
4. Backup and restore runbook
5. Audit verification runbook
6. Trust bundle recovery runbook
7. Dashboard operations runbook
8. Support bundle collection instructions
9. Config reference
10. Troubleshooting guide

**Acceptance:**
- [ ] 10 runbooks exist and match actual commands/paths
- [ ] Each runbook tested against RC2 staging deployment
- [ ] Support bundle avoids leaking regulated payloads

---

## Session RC2-07: Final Production Gate

**Goal:** Go/no-go decision on whether RC2 is shippable as a production appliance.

**Deliverables:**
1. Build final release artifact
2. Run full validation (lamprey-mai-ship-validate)
3. Run full test suite (cargo test --workspace)
4. Check burn-in evidence
5. Check security blocker list
6. Generate final CHECKSUMS.txt
7. Generate release notes
8. Produce go/no-go decision document (RC2-GO-NOGO.md)

**Acceptance:**
- [ ] Final artifact exists
- [ ] Ship validation passes
- [ ] Full test suite passes
- [ ] Known deferrals are honest and non-blocking
- [ ] Release notes say exactly what is ready and what is not
- [ ] Go/no-go decision is recorded

---

## Dependencies

`
RC1.2 Re-bundle (RC-10/RC-11)
    |
    v
RC2-01 (Clean-install rehearsal)
    +-- RC2-02 (Production posture) ----+
    +-- RC2-03 (Service + observability) |
    |                                    v
    |                                 RC2-04 (Backup/restore drill)
    |                                    |
    v                                    v
RC2-05 (72-hour burn-in) <--------------+
    |
    v
RC2-06 (Runbook finalisation)
    |
    v
RC2-07 (Final gate + go/no-go)
    |
    v
Production Appliance
`

---

*Copyright 2026 — Co-Authored by Basho Parks and Claude (DeepSeek v4 Pro)*
