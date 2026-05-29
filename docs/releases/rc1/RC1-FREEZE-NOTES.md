# RC1 Freeze Notes

**Project:** Lamprey MAI
**Release:** RC1 (Tester Bundle)
**Date:** 2026-05-23
**Audience:** release engineers, RC1 testers, acquirer reviewers
**Plan reference:** `docs/COGENT-DEPLOYMENT-ROADMAP.md` Session RC-01

This document records the exact code snapshot RC1 will be cut from.
It says what RC1 includes, what it intentionally excludes, and what
dirty changes exist at the freeze moment so nothing ships silently.

---

## 1. Freeze Point

| Field | Value |
|---|---|
| Repository | `mai/` (Cargo workspace + pyproject.toml monorepo) |
| Branch | `main` |
| HEAD commit | `dceaabcbaa0cbad1bf73dd14a6e3eba610e8c042` (short: `dceaabc`) |
| Subject | `SHIP-17 hotfix: drop allow_demo_defaults from test TOML helper` |
| Author | USS-Parks `<basho.parks@gmail.com>` |
| Authored | 2026-05-23T20:07:16-07:00 |

RC1 will be cut from this commit. The SHIP-17 lane closed in three
commits during the RC-01 audit window:

- `6e027db` — SHIP-17 code (auth bypass consistency guard,
  `PROD-AUTH-101`).
- `697fa27` — SHIP-17 docs (closes Issue 13 in `KNOWN-ISSUES.md`;
  records SHIP-17 across `SESSION-LOG.md`, `SHIP-HARDENING-PLAN.md`,
  and `SHIP-PROFILE.md`).
- `dceaabc` — SHIP-17 hotfix (drops `allow_demo_defaults` from the
  `ship17_baseline_toml` test helper so the SHIP-12 forbidden-term
  allowlist keeps shrinking).

All three are part of the SHIP hardening lane. No code beyond
`dceaabc` is in scope for RC1.

## 2. What RC1 Covers

Sessions complete at the freeze point:

- **MAI core (S1–S35)** — inference engine, scheduler, audit, trust
  bridge, Python SDK surface.
- **Lamprey Phase L (S36–S46)** — router, HIPAA / ITAR / OCAP policy,
  hash-chained audit, compliance dashboard, four acquisition demos,
  Gate D acceptance.
- **Trust Manifold backfill (BF-1..BF-7)** — closed.
- **Ship hardening lane (SHIP-01..SHIP-17)** — production posture for
  the `ship` profile: secrets, validators, restore + DR drills, GPU
  release workflow, 72h burn-in driver + ML-DSA-87 signed report,
  operator docs + 14 runbooks, mypy strict CI job, forbidden-term
  sweep, auth-bypass consistency guard.

Headline performance numbers measured against the post-S46 evidence
and unchanged on the SHIP lane:

- Compliance composer P99: 1.5 µs
- Audit append throughput: 9 003 events / s
- Certified compliance report build: 16.7 ms

All numbers are reproducible from `cargo test -p mai-compliance --test
compliance_perf` on the freeze commit.

## 3. Working Tree At Freeze

`git status` reports a clean tree at the freeze instant on `dceaabc`.
No modified files. No deletions. Only one untracked file — this
freeze-notes document, `docs/RC1-FREEZE-NOTES.md`, which is the
intentional RC-01 output and lands in its own commit.

The four documentation files that were dirty when RC-01 opened
(`docs/KNOWN-ISSUES.md`, `docs/SESSION-LOG.md`,
`docs/SHIP-HARDENING-PLAN.md`, `docs/SHIP-PROFILE.md`, totalling
+177/-13 lines) were committed during the audit window as `697fa27`
("SHIP-17 docs: close Issue 13; record SHIP-17 across governance
docs"). The `mai-api/src/server.rs` edit observed briefly during the
window — a one-line removal of `allow_demo_defaults = false` from the
`ship17_baseline_toml` test helper — was a parallel SHIP-17 hotfix
lane and landed as `dceaabc`. Both are now part of HEAD; neither is
in conflict with RC1.

No mystery local changes are silently packaged. The freeze point is
one named commit whose code, docs, and CI surface (forbidden-term
scanner included) all agree.

## 4. What RC1 Excludes

Excluded per `docs/COGENT-DEPLOYMENT-ROADMAP.md` §2 ("What Not To
Package"):

- `mai/target/debug/` — debug build artifacts (rebuildable, multi-GB).
- `mai/target/release/` — optional, deferred to Session RC-03.
- `.pytest_cache/`, `.mypy_cache/`, `.ruff_cache/` — tool caches.
- Local generated logs and stale test output (anything not promoted
  into `test-evidence/`).
- Model weights — packaged separately if a model pack is built.
- Local IDE state, editor scratch, `/tmp/` staging artifacts.
- Cowork session state under `/sessions/*/`.

## 5. Gate D Evidence Currency

`docs/acquisition/READY.md` captures the Gate D / Session 46
acquisition snapshot (≈1 557 runnable tests, 0 failing). It is
current as of S46 and is the document an acquirer reviewer reads
first.

`READY.md` does **not** fold in the SHIP-01..SHIP-17 hardening lane
additions (the +6 SHIP-17 tests, the SHIP-14 burn-in suite, the
SHIP-10 restore tests, the SHIP-12 mypy regression, etc.). That
separation is intentional: `READY.md` is the acquisition evidence
pinned to S46; `docs/SHIP-HARDENING-PLAN.md` is the
production-hardening evidence stacked on top of it. RC1 testers
should treat them as additive.

The RC1 test-evidence refresh under Session RC-05 will produce a
single combined surface count at the freeze commit. This freeze note
does not pre-state that number.

## 6. Acceptance Checklist (RC-01)

| Criterion | Status |
|---|---|
| Named commit or snapshot for RC1 | `dceaabc` on `main` |
| Freeze notes describe inclusions | §2 |
| Freeze notes describe exclusions | §4 |
| Dirty files audited and dispositioned | §3 (working tree clean; prior dirty docs committed as `697fa27`; parallel SHIP-17 hotfix landed as `dceaabc`) |
| No mystery local changes silently packaged | Confirmed — working tree clean except for `docs/RC1-FREEZE-NOTES.md` (the RC-01 output itself) |
| Gate D evidence currency assessed | §5 |

## 7. Next Session

Session RC-02 (Clean Package Manifest) consumes this freeze point and
emits `docs/RC1-PACKAGE-MANIFEST.md` listing the exact folder set for
the RC1 tester bundle and the build-or-ship decision for `mai-api`
release binaries.
