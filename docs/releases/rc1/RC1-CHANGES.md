# RC1 Changes

**Project:** Lamprey MAI
**Plan reference:** `docs/COGENT-DEPLOYMENT-ROADMAP.md` Session RC-10
**Tracks:** revisions to the RC1 tester bundle after the initial
`dceaabc` freeze.

This document is the canonical changelog for RC1. Each revision
gets a section noting the version label, what changed, and the
rationale. The binary freeze commit (`dceaabc`) is **only** bumped
when code changes; doc-only revisions ship under the same freeze.

---

## RC1.1-docs (2026-05-24)

**Freeze commit:** `dceaabc` (unchanged)
**Binary hashes:** unchanged
- `bin/lamprey-mai-api.exe`:
  `4e201a8498d3e46361c83fc4eff6e04c1021fca3187b04a4d9f55f398b1462b6`
- `bin/lamprey-mai-ship-validate.exe`:
  `a32ddc2891a7690cb015a9d1ed06cb84d4160f92976e61ac50cb14069e9ae8f8`

**Source of input:** `mai/docs/RC1-SELF-REVIEW-TRACK-C.md`
(RC-09 Track C self-review, commit `5be7d2b`). RC-10 worked from
self-review findings rather than waiting for outside-tester
feedback so the outside reviewer (still pending) sees the cleaner
bundle. When outside-tester feedback eventually arrives, an
RC1.2-docs (or RC1.2 if code changes) revision will land on top.

**Test evidence:** unchanged from RC-05 (`RC1-TEST-EVIDENCE.md`)
plus the perf re-run captured in the self-review memo §1.1
(composer P99 300 ns / audit 119 494/s / report 1.687 ms on the
extracted bundle, 2026-05-24).

### Patches by finding ID

| Finding | Severity | Bucket | What changed |
|---|---|---|---|
| H-1 | High | docs | Added "Status against RC1 freeze" header band to runbooks `05-verify-audit-chain.md`, `06-generate-compliance-report.md`, `11-trust-bundle-expired.md`, `12-audit-wal-tamper.md`, `13-air-gap-violation.md`, noting which cited `mai-admin` subcommands are stubbed (`audit`, `trust`, `vault`) or undeclared (`compliance`, `policy`) and pointing each to the live HTTP equivalent on the running daemon. |
| H-2 | High | docs | Rewrote all four acquisition demo narratives (`healthcare.md`, `defense.md`, `tribal.md`, `multi-domain.md`) to use `curl` against the live HTTP surface (`/v1/chat/completions`, `/v1/compliance/{status,audit,reports/generate,reports/{id}/download,policies/template}`, `/v1/system/airgap/engage`) with `X-IM-Auth-Token` header. Each demo now references the `compliance_demos.rs` test that exercises the same scenario as a fallback. |
| H-3 | High | docs | All four demos: REST port `8080` → `8420`, dashboard port `8081` flagged as a companion-process start (not auto-started by `lamprey-mai-api.exe`). |
| H-4 | High | docs | All four demos: replaced the hardcoded builder-workspace `cd` with `cd source` from `Lamprey-MAI-RC1/`. |
| H-5 | High | docs | `TESTER-INSTRUCTIONS.md` §4.C step 4 runbook numbers fixed: `04→05`, `05→06`, `09→11`, `10→12`, `11→13`. |
| M-1 | Medium | docs | `TESTER-INSTRUCTIONS.md` §4.C step 2 rewritten to note the router/policy/audit layers are described inline in `ARCHITECTURE.md`, not as separate files. Cross-pointed to `LAMPREY-BRIEF.md`, `TRUST-MANIFOLD.md`, `AUDIT-CORRELATION.md` for deeper coverage. |
| M-2 | Medium | docs | `README-FIRST.md` §5.C amended to match observed runtime: logs and banner both stream on stdout at the freeze; "a future RC will route logs to stderr" pending. Doc-side fix only; no runtime change. |
| M-3 | Medium | docs | Demos now lead with the bundled `bin/lamprey-mai-api.exe` and treat `cargo run --release --bin mai-api` as the source-path alternative. |
| M-4 | Medium | docs | `runbooks/README.md` gained an audience note explaining that the runbooks describe Linux systemd production posture; the RC1 bundle is Windows MSVC tester-only. |
| L-1 | Low | docs | `README-FIRST.md` "MAI server ready" line: hyphen → em-dash to match the runtime emission. |

**Not patched in RC1.1-docs (deferred):**

| Finding | Severity | Disposition | Reason |
|---|---|---|---|
| L-2 | Low | dismiss | `ARCHITECTURE.md` `mai/` path prefixes — readable in context (the doc was written when the workspace was `mai/`; inside the bundle `source/` is the workspace). Cosmetic; no behavior impact. |
| L-3 | Low | defer-to-next-RC | Topology log says `gpus=1` while `/v1/health` reports `"gpus":[]` — probably intentional layer divergence (placement-math placeholder vs queryable-devices). Needs a code-side review to decide doc vs unify; out of doc-only scope. |
| M-2 (code side) | Medium | defer-to-next-RC | A real fix would route logs through stderr at the tracing-subscriber level. Code change; would bump the freeze. RC1.1-docs documented the current behavior instead. |

### Files touched

```
docs/README-FIRST.md
docs/TESTER-INSTRUCTIONS.md
docs/RC1-PACKAGE-MANIFEST.md
docs/RC1-CHANGES.md                       (new)
docs/acquisition/demos/healthcare.md
docs/acquisition/demos/defense.md
docs/acquisition/demos/tribal.md
docs/acquisition/demos/multi-domain.md
docs/runbooks/README.md
docs/runbooks/05-verify-audit-chain.md
docs/runbooks/06-generate-compliance-report.md
docs/runbooks/11-trust-bundle-expired.md
docs/runbooks/12-audit-wal-tamper.md
docs/runbooks/13-air-gap-violation.md
```

### Bundle implication

The compressed archives at
`C:/Users/17076/Documents/Claude/Island-Mountain-RC1-release/`
were assembled at the `dceaabc` freeze and **do not contain
these RC1.1-docs patches.** A bundle re-assembly is recommended
before the outside Track C reviewer receives the package — re-run
RC-08's step 7 ("Copy RC1 docs into `source/docs/`") and step 11
("Write top-level CHECKSUMS"). The binaries do not need to be
rebuilt. New archive hashes will be needed.

### Acceptance vs RC-10 criteria

| Criterion | Status |
|---|---|
| First-tester blockers are resolved or explicitly deferred | **PARTIAL** — self-review's 5 H + 4 M findings resolved or deferred per the matrix above; outside-tester blockers still TBD |
| Updated package has a clear version label | RC1.1-docs (this doc) |
| Test evidence is refreshed if code changed | N/A — no code changed; RC-05 evidence and the self-review's perf re-run remain authoritative |

RC-10 is a doc-only pass that consumes the self-review. It does
not close RC-09 (still gated on an outside reviewer) and a future
RC-10 final-pass will land after the outside reviewer reports.
