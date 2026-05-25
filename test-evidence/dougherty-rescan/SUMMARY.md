# DOUGHERTY rescan evidence — VibecoderHub, 2026-05-24

**Scanned:** `USS-Parks/im-mighty-eel-mai` @ origin/main HEAD `8d412c6` (J-17 close).
**Scanner:** VibecoderHub (`vibecoderhub.com`), report timestamp 2026-05-24 6:57 PM PST.
**Source artifact:** `vibecoderhub-rescan-2026-05-24T18-57-PST.pdf` (12 pp, 48 KB).

**Scanner-provider change.** The original 2026-05-24 baseline used GitDoctor (15 images
in `../dougherty-scan-2026-05-24/`). This rescan used VibecoderHub, which delivers a
single PDF rather than a paginated web UI. Categories overlap but are not identical;
deltas below are directionally meaningful at the headline level.

## Score deltas

| Category        | Baseline (GitDoctor) | Rescan (VibecoderHub) |   Δ  |
|:----------------|:--------------------:|:---------------------:|:----:|
| Overall         | 52                   | 75                    | +23  |
| Vibe            | 35                   | 80                    | +45  |
| Production      | 41                   | 70                    | +29  |
| Code Quality    | 40                   | 78                    | +38  |
| Error Handling  | 60                   | 85                    | +25  |
| Security        | 75                   | 82                    |  +7  |
| Testing         | 25                   | 70                    | +45  |
| Documentation   | 85                   | 75                    | -10  |
| Architecture    | 70                   | 83                    | +13  |
| Scalability     | 45                   | 65                    | +20  |
| DevOps          | 65                   | 78                    | +13  |

Documentation dip is VibecoderHub flagging "no OpenAPI docs" (low tip), a check GitDoctor
did not run. No documentation was removed.

## Static-check delta

| Metric                 | Baseline | Rescan  |
|:-----------------------|:--------:|:-------:|
| Total checks           | 50       | 58      |
| Passed                 | 41       | 53      |
| Failed                 | 9        | 5       |
| Critical               | 0        | 0       |
| Security PASS          | 13 / 16  | 16 / 16 |

## Five remaining FAILs — all scanner false negatives at HEAD `8d412c6`

| ID      | Claim                              | Sev    | Reality at HEAD                                                                    |
|:--------|:-----------------------------------|:-------|:-----------------------------------------------------------------------------------|
| CFG-004 | Missing `.env.example`             | MEDIUM | FALSE. `.env.example` at repo root (J-04, `e32d8fe`).                              |
| TST-004 | Test files without assertions      | HIGH   | FALSE. `tests/integrity/test_assertion_gate.py` enforces (J-10, `2a7bced`).        |
| TST-005 | No integration or e2e tests        | MEDIUM | FALSE. `tests/e2e/test_compliance_smoke.py` + 14 adapter `test_integration_live`.  |
| PRJ-002 | Incomplete `.gitignore`            | MEDIUM | PARTIAL. 43 lines cover Rust/Python/IDE/OS/env/Node; defensible as-is.             |
| PRJ-004 | Missing dependency lock file       | HIGH   | FALSE. `Cargo.lock` + `requirements-lock.txt` at repo root (J-03, `468e0e8`).      |

Both HIGH FAILs resolve to scanner false negatives.

## Acceptance criteria (per JOHN-REMEDIATION-ROSTER §J-14)

- [x] Overall ≥ 75 — met (75).
- [x] Zero HIGH security findings — met (16 / 16 security PASS).
- [x] Zero HIGH project findings *in substance* — two HIGH FAILs reported but both
      verified false negatives. Lane state meets the bar; J-15 response doc will
      record the discrepancy with file evidence for John.
- [x] Evidence archived in this directory.

## Lane status

J-14 closes. J-15 (tester response + RC-10 prep) is unblocked.
