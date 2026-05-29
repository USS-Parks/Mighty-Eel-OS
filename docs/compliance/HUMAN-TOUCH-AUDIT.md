# Human-Touch Audit

**Project:** Island Mountain Model Abstraction Interface (MAI)
**Date:** 2026-05-23
**Purpose:** Identify which near-demo files should remain clinical/static and which files deserve language, narrative, demo-flow, or operator-experience polish before acquisition demos and testing.
**Change policy:** This document is an audit artifact only. No product, docs, CLI, API, demo, or config behavior should be changed from these recommendations until the user explicitly approves a proposed change package.

---

## Classification Key

| Classification | Meaning |
|---|---|
| Clinical/static is appropriate | The file is mainly a contract, ledger, spec, or machine-facing config. Human touch should be limited to clarity, correctness, and removing friction. |
| Needs polish | The file is useful but too dry, stale, internally framed, or uneven for its audience. It should be tightened before demos but is not the first impression. |
| High-impact human-touch candidate | The file shapes the first impression for reviewers, operators, buyers, or demo users. It deserves careful voice, structure, examples, and narrative flow. |

## Priority Key

| Priority | Urgency |
|---|---|
| P0 | Fix before any serious demo or external review. |
| P1 | Fix before acquisition-ready package / Session 46 demo suite. |
| P2 | Improve when touching the surrounding area. |
| P3 | Leave mostly as-is unless a specific user-facing gap appears. |

---

## Executive Findings

The codebase has moved beyond bare bones. The strongest parts are the acquisition-oriented technical briefs, the compliance architecture story, and the explicit trust-boundary language. The main polish gap is not lack of content; it is that several high-visibility files still read like build-session artifacts instead of finished product surfaces.

The highest-leverage improvements are:

1. Repair visible encoding artifacts across demo/acquisition docs and app READMEs. These artifacts break confidence immediately because arrows, dashes, section symbols, and checkmarks render as mojibake.
2. Reframe demo-facing READMEs from "Session 30 reference scaffold" and "placeholder" language into outcome-oriented product demos.
3. Add a stronger first-run path to the top-level README and SDK quickstart so a new reviewer understands what MAI is, why it matters, and what they can run next.
4. Improve CLI and demo script output wording so failures are explanatory and operator-friendly, without becoming cute or chatty.
5. Keep config examples mostly clinical, but add concise operator intent comments where defaults affect safety, compliance, or demo posture.

---

## High-Impact Candidates

| File | Classification | Priority | Why It Matters | Recommended Direction | Approval Status |
|---|---|---:|---|---|---|
| `mai/README.md` | High-impact human-touch candidate | P0 | This is the first repo-level impression, but it is currently terse and mostly structural. | Add a short "what this proves" section, demo entry points, trust-boundary positioning, and a clearer quick start without marketing fluff. | Approval required |
| `mai/docs/DEMO-SUITE.md` | High-impact human-touch candidate | P0 | This is the demo script catalog. It contains strong substance but visible encoding artifacts and a procedural tone. | Clean encoding, add a tighter demo operator path, make each scenario read like a proof with expected audience reaction and verification commands. | Approval required |
| `mai/docs/ACQUISITION-PACKAGE.md` | High-impact human-touch candidate | P0 | Buyer-facing thesis. Strong claims are present, but encoding artifacts and dense session language reduce polish. | Clean encoding, sharpen the opening narrative, keep code citations, and make defensible points easier to scan. | Approval required |
| `mai/docs/BUYER-INTEGRATION-GUIDE.md` | High-impact human-touch candidate | P0 | Security architects will use this to judge integration maturity. It already has the right backbone. | Clean encoding, add a concise "30-minute review path", and clarify what can be swapped without changing client code. | Approval required |
| `mai/docs/HANDOFF.md` | High-impact human-touch candidate | P1 | It is the living orientation for incoming engineers and reviewers. It has personality but is very long and session-heavy. | Keep the memorable "Five Things That Will Bite You" section, but add an upfront reviewer route and reduce chronological overload. | Approval required |
| `mai/mai-sdk-python/docs/quickstart.md` | High-impact human-touch candidate | P1 | This is the first developer success path. It is functional but thin. | Add expected output, troubleshooting for auth/server unavailable, and a trust/compliance next step. | Approval required |
| `mai/apps/openbao-trust-demo/README.md` | High-impact human-touch candidate | P0 | Primary Trust Manifold demo. It still explains implementation steps more than audience value. | Rename sections around the eight proof moments, remove stale "simulated here" ambiguity where later live endpoints exist, and add a crisp runbook. | Approval required |
| `mai/apps/tribal-sovereignty/README.md` | High-impact human-touch candidate | P0 | This is the unique OCAP differentiator. It deserves the most careful human language. | Make the purpose dignified and precise, explain local-only refusal as a sovereignty guarantee, and avoid making tribal data sound like a generic test fixture. | Approval required |
| `mai/compliance-dashboard/README.md` | High-impact human-touch candidate | P1 | The dashboard is the one explicit compliance UI. The README is accurate but utilitarian. | Add operator personas, review workflows, and what each page lets a compliance officer prove. | Approval required |
| `mai/apps/operator/README.md` | High-impact human-touch candidate | P1 | This is likely used during demo monitoring. It still mentions BF-6 pending even though BF-6 landed. | Remove stale pending language, make sample output current, and explain how operators act on each panel. | Approval required |
| `mai/docs/SCHEDULER-BRIEF.md` | High-impact human-touch candidate | P1 | This is one of the strongest narrative docs already. It may only need cleanup. | Clean encoding, preserve the confident voice, add a one-page skim path if missing later in the file. | Approval required |

---

## Needs Polish

| File / Area | Classification | Priority | Observed Issue | Recommended Direction | Approval Status |
|---|---|---:|---|---|---|
| `mai/docs/INDEX.md` | Needs polish | P1 | Useful, but the first screen feels like internal governance, and links appear to assume root-relative docs that may confuse readers. Encoding artifacts appear in status text. | Add "Start here by role" and separate governance docs from external-review docs. | Approval required |
| `mai/docs/SESSION-LOG.md` | Needs polish | P2 | Appropriate as a ledger, but huge blocks of prose make status hard to skim. Encoding artifacts are frequent. | Keep ledger tone, add compact current-status summary and avoid narrative polish beyond readability. | Approval required |
| `MAI-BUILD-PROMPT-ROSTER-v2.md` | Needs polish | P2 | Governing artifact, not a demo doc, but its scale and session mechanics can leak into user-facing docs. | Leave mostly intact; create curated extracts rather than rewriting the source of truth. | Approval required |
| `BUILD-EXECUTION-PLAN-V2-UPDATED.md` | Needs polish | P2 | Important planning source, likely too dense for demo readers. | Leave as plan record; link to polished Session 45/46 docs instead of sending reviewers here first. | Approval required |
| `mai/docs/DEPLOYMENT.md` | Needs polish | P1 | Operator-facing docs should be warmer where things fail and more decisive about the golden path. | Audit next pass for install/run/troubleshooting language and expected outputs. | Approval required |
| `mai/docs/API-REFERENCE.md` and `mai/docs/api/MAI-API-SURFACE-SPEC.md` | Needs polish | P2 | API references should stay precise, but examples and error guidance may need better developer empathy. | Add small "common workflows" examples and make errors actionable. | Approval required |
| `mai/docs/SDK-REFERENCE.md` | Needs polish | P2 | Likely useful as reference, less so as onboarding. | Keep reference shape; cross-link to quickstart and streaming patterns. | Approval required |
| `mai/mai-sdk-python/docs/error-handling.md` | Needs polish | P1 | Error handling is a major UX surface for developers and demo operators. | Add common failure examples: auth missing, server unreachable, model unavailable, air-gap refusal. | Approval required |
| `mai/apps/local-secure-inference/README.md` | Needs polish | P1 | Functional but scaffold-framed. Some symbol encoding is broken in precedence list. | Reframe as "local first request" demo and add expected output. | Approval required |
| `mai/apps/rag-reference/README.md` | Needs polish | P1 | Good scaffold honesty, but first sentence is minimal and encoding is broken in the flow line. | Reframe as a local retrieval proof; clarify limitations as deliberate design choices. | Approval required |
| `mai/apps/compliance-routed/README.md` | Needs polish | P1 | Still calls itself a placeholder and mock router. That may be correct historically but weak for demos. | Recast as "policy-shape reference demo" and clearly state what is real vs mocked today. | Approval required |
| `mai/deployment/*/README.md` | Needs polish | P1 | Deployment profile docs are close to operator-facing. `airgap-demo` is strong but still has encoding artifacts. | Standardize profile docs: purpose, when to use, verification, failure modes, demo moments. | Approval required |
| `mai/apps/*/config.toml` | Needs polish | P2 | Demo configs are user-editable and should explain the safe defaults. | Add concise comments only where changing a value changes safety, route, trust, or demo behavior. | Approval required |
| `mai/config/*.toml` | Needs polish | P2 | Some are well-commented (`router.toml`, `policy.toml`), others are bare (`scoring.toml`). | Add operator-intent comments to sparse configs, especially scoring and demo-sensitive controls. | Approval required |
| `mai/mai-sdk-python/src/mai/cli.py` | Needs polish | P1 | CLI output is terse and machine-like. Errors say "failed" but rarely suggest next steps. | Improve non-JSON output labels and error hints while preserving stable exit codes and JSON behavior. | Approval required |
| `mai/apps/operator/main.py` | Needs polish | P1 | Operator output is functional and plain-text, but can be clearer during demos. | Improve labels, panel ordering, and fallback messages; keep cron-friendly output. | Approval required |
| `mai/apps/openbao-trust-demo/main.py` | Needs polish | P1 | Step output exists, but it reads like implementation tracing. | Make step labels match the demo story: claim, cache, route, audit, degradation. | Approval required |
| `mai/apps/tribal-sovereignty/main.py` | Needs polish | P1 | Refusal messages are direct but could better explain protected local-only behavior. | Make denial copy precise, respectful, and audit-aware. | Approval required |
| `mai-api/src/errors.rs` | Needs polish | P2 | API errors are safely sanitized, but several messages are generic. | Add client-action hints where safe, possibly via docs rather than response body if API contract should stay stable. | Approval required |
| `mai-api/src/streaming/ws.rs` and `mai-api/src/streaming/sse.rs` | Needs polish | P2 | Streaming errors are technical. | Ensure errors are consistent with SDK docs and do not expose backend internals. | Approval required |

---

## Clinical / Static Is Appropriate

| File / Area | Classification | Priority | Why Minimal Human Touch Is Better | Recommended Direction | Approval Status |
|---|---|---:|---|---|---|
| `AGENTS.md` | Clinical/static is appropriate | P3 | Agent governance should be explicit and operational. | Only update when workflow rules change. | Approval required |
| `CONVENTIONS.md` | Clinical/static is appropriate | P3 | Engineering conventions should stay directive. | Fix clarity/encoding only if encountered. | Approval required |
| `SESSION-RULES.md` | Clinical/static is appropriate | P3 | Session protocol needs precision more than warmth. | Leave as governance. | Approval required |
| `mai/docs/architecture/01-*.md` | Clinical/static is appropriate | P2 | Architecture components are reference material. | Clean encoding and add diagrams only where they improve comprehension. | Approval required |
| `mai/docs/api/openapi.yaml` | Clinical/static is appropriate | P3 | Machine-readable contract. | Do not humanize beyond descriptions/examples. | Approval required |
| `mai/config/compliance/*.toml` | Clinical/static is appropriate | P2 | Compliance configs should be unambiguous, reviewed like code. | Keep concise, add comments only for operator risk. | Approval required |
| `mai/mai-api/config/*.toml` | Clinical/static is appropriate | P2 | Runtime config should be predictable and grep-friendly. | Add comments sparingly; avoid narrative prose. | Approval required |
| `mai/Cargo.toml`, crate `Cargo.toml`, `pyproject.toml` | Clinical/static is appropriate | P3 | Build metadata should stay mechanical. | No polish unless metadata is wrong or missing. | Approval required |
| `mai/tests/**`, crate tests, fixture files | Clinical/static is appropriate | P3 | Tests need clarity and determinism, not brand voice. | Improve test names only if they obscure behavior. | Approval required |
| `mai/proto/**` | Clinical/static is appropriate | P3 | Wire contracts should remain terse and stable. | Only add comments that clarify field semantics. | Approval required |
| `mai/tools/rule-tester/examples/*.toml` | Clinical/static is appropriate | P2 | Scenario fixtures should be readable but exact. | Keep examples crisp; no narrative layer. | Approval required |

---

## Cross-Cutting Issues

### P0: Encoding Artifacts

Many Markdown files display mojibake such as `â€”`, `â†’`, `Â§`, and corrupted checkmark symbols. This appears in high-visibility docs including:

- `mai/docs/DEMO-SUITE.md`
- `mai/docs/ACQUISITION-PACKAGE.md`
- `mai/docs/BUYER-INTEGRATION-GUIDE.md`
- `mai/docs/INDEX.md`
- `mai/docs/HANDOFF.md`
- several `mai/apps/*/README.md` files

Recommended change package: a controlled encoding cleanup pass over Markdown only, replacing corrupted symbols with ASCII-safe equivalents or valid UTF-8 where the file already intentionally uses Unicode. This should be reviewed separately because it touches many files.

### P0: Stale Scaffold Language

Several demo READMEs still frame themselves as session scaffolds or placeholders after later sessions landed real endpoints. This lowers perceived maturity even when the code is stronger than the prose.

Recommended change package: rewrite demo READMEs around what they prove now, with a short note for any intentionally mocked dependency.

### P1: First-Run Reviewer Path

The repo has many entry points but not enough role-based guidance. A buyer, security architect, SDK developer, and demo operator should not all start in the same long index.

Recommended change package: add a compact "Start here" block to `mai/README.md` and `mai/docs/INDEX.md` with role-based links.

### P1: Error and Refusal UX

The product's most distinctive behavior is refusing unsafe routes, preserving local-only guarantees, and degrading safely. Those moments should feel deliberate, not like generic failure.

Recommended change package: improve CLI/demo refusal copy and pair it with docs examples. Preserve exit codes, JSON schemas, and safe server-side sanitization.

### P2: Config Intent Comments

Some configs already explain operator intent well. Sparse configs like `config/scoring.toml` are harder to tune safely.

Recommended change package: add comments that explain the consequence of changing each major group, especially for routing, scoring, cache TTLs, and compliance templates.

---

## Proposed Approval Packages

### Package A: Encoding Cleanup

**Priority:** P0
**Scope:** Markdown docs and READMEs only.
**Files likely touched:** `mai/docs/*.md`, `mai/apps/*/README.md`, `mai/deployment/*/README.md`, `mai/README.md`, possibly root planning docs.
**Risk:** Low behavior risk, medium review noise due to many files.
**User approval needed before edits:** Yes.

### Package B: First-Impression Docs

**Priority:** P0
**Scope:** `mai/README.md`, `mai/docs/INDEX.md`, `mai/docs/DEMO-SUITE.md`, `mai/docs/ACQUISITION-PACKAGE.md`, `mai/docs/BUYER-INTEGRATION-GUIDE.md`.
**Risk:** Medium narrative risk because wording affects positioning.
**User approval needed before edits:** Yes.

### Package C: Demo READMEs

**Priority:** P0/P1
**Scope:** `apps/openbao-trust-demo`, `apps/tribal-sovereignty`, `apps/compliance-routed`, `apps/local-secure-inference`, `apps/rag-reference`, `apps/operator`.
**Risk:** Low code risk, medium product-voice risk.
**User approval needed before edits:** Yes.

### Package D: CLI and Demo Output Copy

**Priority:** P1
**Scope:** `mai-sdk-python/src/mai/cli.py`, selected `apps/*/main.py`, tests that assert exact output.
**Risk:** Medium because tests may assert stderr/stdout wording.
**User approval needed before edits:** Yes.

### Package E: Config Comment Pass

**Priority:** P2
**Scope:** sparse TOML examples under `mai/config`, `mai/configs`, `mai/apps/*/config.toml`, and deployment profiles.
**Risk:** Low runtime risk if comments only.
**User approval needed before edits:** Yes.

---

## Recommended First Move

Start with Package A plus a small part of Package B:

1. Clean encoding artifacts in the five highest-visibility docs.
2. Update `mai/README.md` so it gives a reviewer a confident first path.
3. Update `mai/docs/DEMO-SUITE.md` so the Trust Manifold scenario reads like a finished proof rather than a build artifact.

I recommend holding CLI/API wording changes until after the docs voice is approved, because the product voice should be established in the docs first and then reflected in runtime messages.

