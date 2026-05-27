# RC1.2 re-bundle checklist (RC-10 prep)

> **STATUS — COMPLETE (2026-05-25)**
> RC-10 ran and produced the RC1.2 bundle at freeze `e55c1ff`. RC-11 re-ship also closed 2026-05-25 ([`RC1.2-RESHIP.md`](RC1.2-RESHIP.md)). Kept as the canonical record of the gating prerequisites that had to be met. All prerequisites below are now satisfied.

**Purpose:** prerequisite checklist for the RC-10 re-bundle session, which
follows the DOUGHERTY lane (J-01..J-15). RC-10 produces the RC1.2 tester
bundle that supersedes the RC1.1-docs assembly at
`Island-Mountain-RC1-release/`.

**Distinct from the earlier RC-10:** the original RC-10 (commit `b0fcdee`)
was the RC1.1-docs self-review fix pass and shipped before John's review
landed. This RC-10 is the post-DOUGHERTY re-bundle. Naming collision is
documented in `project_rc_release_lane.md`.

## Prerequisites met (verify before opening RC-10)

- [x] J-01..J-13 committed and pushed to `origin/main` (all 2026-05-24)
- [x] J-16 + J-16b + J-17 committed; SDK `todo!()` count = 0
- [x] J-10b independent-evidence probes PASS or signed deferral
- [x] J-18..J-26 W3 adapter completion matrix committed
- [x] J-14 rescan evidence committed (`b899a84` on `session/J-14`)
- [x] J-15 response doc committed (this session)
- [x] `session/J-14` and `session/J-15` merged to `origin/main` (merged via `059a6e3` 2026-05-25)

## Freeze-commit decision

The RC1.1-docs bundle pins freeze at `dceaabc` (SHIP-17). RC1.2 must
advance to the post-DOUGHERTY merge commit on `main`. RC-10's first
action is to record the new freeze in `RC1-FREEZE-NOTES.md` with a
diff summary against `dceaabc`.

## What changes vs the RC1.1-docs assembly

| Area              | Δ                                                                  |
|:------------------|:-------------------------------------------------------------------|
| Binaries          | Rebuild required — Cargo.lock changed (J-16 added `reqwest`)       |
| Source tree       | 26 J-session commits (`6621c02` … `b899a84`) on top of `dceaabc`   |
| Lock files        | New `requirements-lock.txt`, new `.integrity/mcp-server/package-lock.json` |
| Dockerfile        | New `Dockerfile` + `.dockerignore` + `.env.example`                |
| Adapters          | 4 new (openai_compat, onnxruntime, mlx, triton); 7 existing hardened |
| Tests             | New `tests/integrity/`, `tests/e2e/`, `mai-sdk-rs/tests/`           |
| Docs              | New `ADAPTER-COMPLETION-MATRIX.md`, `ERROR-PATH-AUDIT.md`, `INDEPENDENT-EVIDENCE-DEFERRALS.md`, `RC1-TESTER-RESPONSE-DOUGHERTY.md`, `RC1.2-REBUNDLE-CHECKLIST.md` |

## RC-10 execution outline (for the session that opens it)

1. Confirm both J-14 and J-15 branches merged to `main`; record new freeze SHA.
2. Rebuild release binaries (`cargo build --release -p mai-api -p mai-ship-validate`)
   on `x86_64-pc-windows-msvc`. Publish new sha256 in `bin/SHA256SUMS`.
3. Run RC-05-equivalent test evidence pass at the new freeze; emit
   `RC1.2-TEST-EVIDENCE.md`. Per the test-evidence-literalism rule, include
   the "what was NOT exercised" section.
4. Copy post-DOUGHERTY tree into a fresh `Lamprey-MAI-RC1/` per the existing
   RC-08 recipe in `RC1-BUNDLE-NOTES.md`. Apply the 4 packaging gaps from
   RC-06 frictions (no `et HEAD`, no `.claude/`, no `pytest-cache-files-*/`,
   forward the RC1-era docs).
5. Regenerate `CHECKSUMS.txt`, rebuild both archives, publish new canonical
   hashes in the external `Island-Mountain-RC1-release/SHA256SUMS`.
6. Update `RC1-TESTER-FEEDBACK.md` §2 with new artefact table; bump version
   label from "RC1.1-docs" to "RC1.2".
7. Open RC-11 (re-ship) with the new bundle hashes and invitation templates.

## Out of scope for RC-10

- No code changes (the J-lane already made them; RC-10 is packaging only).
- No new tester recruitment (that is RC-11).
- No GitDoctor re-scan (J-14 covered the rescan; do not re-run for RC-10).
