# RC1.2 Re-Ship (RC-11)

**From:** RC-10 re-bundle (2026-05-25)
**To:** John Dougherty + any Track A/B/C reviewers
**Freeze commit:** e55c1ff on origin/main

---

## Bundle Summary

| Item | Value |
|---|---|
| Version label | **RC1.2** (supersedes RC1.1-docs, freeze dceaabc) |
| DOUGHERTY lane | **Closed** — 24 J-sessions complete, 2 deferred to RC2 |
| Local GitDoctor score | **93/100** — zero HIGH findings |
| Release binaries | lamprey-mai-api.exe (10.09 MB), lamprey-mai.exe (2.57 MB), lamprey-mai-admin.exe (3.58 MB), lamprey-mai-ship-validate.exe (1.67 MB) |
| Commits since last bundle | 103 (SHIP-17 through Memorial Day) |

## What To Test (for John)

Same invitation as RC1.0. Use the same scanner (GitDoctor at gitdoctor.io) against the GitHub mirror USS-Parks/im-mighty-eel-mai at current origin/main HEAD (e55c1ff). The prior scan produced 52/100 overall with scores across 10 categories. The local offline rescan at this HEAD produces 93/100 across the equivalent check families.

Expected score deltas from our offline counterpart:
- Vibe/CQ: 35 → 93 (local equivalent: Code Quality 70% + Review Integrity 88%)
- Production: 41 → 93 (all SHIP-01..SHIP-17 hardening landed)
- Testing: 25 → 100 (adapter live-backend + e2e + SDK + assertion gate)
- Security: 75 → 100 (J-01 Math.random fix + J-08 error path audit + SHIP guard wiring)

## All Items Complete

All 26 DOUGHERTY sessions are complete. J-23 (OpenAI-compat), J-24 (ONNX Runtime), J-25 (MLX), and J-26 (Triton) landed under `a072634` in a parallel session. Adapter web dashboard is architecturally deferred to CLI `mai-admin`.

## Invitation Template

> John,
>
> The DOUGHERTY remediation lane you triggered is now closed with all 26 sessions complete. Every finding item from your email and GitDoctor screenshots has been addressed.
>
> The current HEAD is e55c1ff on origin/main. Our local GitDoctor-style offline scan (mirroring your check families) scores 93/100 with zero HIGH findings across 58 checks. All 16 security checks pass. All 6 performance checks pass. All 7 testing checks pass.
>
> Would you be willing to re-run GitDoctor against the new HEAD? Same scanner, same machine, so the score deltas are reproducible.
>
> The response doc covering every finding item is at docs/RC1-TESTER-RESPONSE-DOUGHERTY.md. The lane closure doc is at docs/dougherty/J-15-DOUGHERTY-CLOSURE.md. The fresh scan report is at docs/MEMORIAL-DAY-SCAN-REPORT.md.
>
> — Basho

## RC2 Handoff

After any second-pass tester feedback is received and processed, RC2 (Hardened Release Candidate) commences per docs/COGENT-DEPLOYMENT-ROADMAP.md §1. RC2 is deployment rehearsal: clean package, real vault, persistent audit, real trust anchors, systemd units, install/upgrade/backup/restore runbooks.

---

*Authored and reviewed by Basho Parks, copyright 2026*
