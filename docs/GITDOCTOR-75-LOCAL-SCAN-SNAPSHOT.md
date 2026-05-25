# GitDoctor 75 Local Scan Snapshot

**Date:** 2026-05-25  
**Tool:** `tools/local_gitdoctor_scan.py`

Latest run (this session) summary:
- **Overall:** 93/100
- **Checks:** 58 total, 54 passed, 4 failed

This is an offline, stdlib-only heuristic scanner intended to mirror broad GitDoctor-style checks. It is not the external scanner used for the PDF; use it as a parity aid and to prevent regressions in obvious hygiene checks.

Re-run:
- `python tools/local_gitdoctor_scan.py --root . --format markdown --fail-on none`
