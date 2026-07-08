# Internal GitDoctor (IGD) Remediation Plan

**Lane:** IGD — Internal GitDoctor remediation, sessions `IGD-01` through `IGD-12`
**Companion roster:** `IGD-REMEDIATION-ROSTER.md` (open per-session prompts)
**Source scan:** `docs/INTERNAL-GITDOCTOR-SCAN-2026-05-26.md` (HEAD = `ee6eb13`, overall 86/100)
**Started:** 2026-05-26
**Owner:** Basho Parks
**Co-author footer (every commit, no exceptions):**
```
Copyright 2026 - Co-Authored by Basho Parks and Claude Opus 4.7 xHigh <basho@islandmountain.io> <claude@anthropic.com>
```

---

## 1 · Goal

Close every defensible finding from the 2026-05-26 internal GitDoctor scan and lift the overall score from **86 → 95+**. The scan identified 3 HIGH, 4 MEDIUM, and 5 LOW findings against the live `mai/` tree. None are critical, but H-1 (Vault token in an un-gitignored path) is one accidental `git add` away from leaking, and H-2/H-3 are unpatched RustSec advisories with known one-line fixes.

## 2 · Workstreams

| WS  | Title                                | Drives | Sessions |
|:----|:-------------------------------------|:------:|:---------|
| W1  | Secret containment + git hygiene      | H-1    | IGD-01 |
| W2  | RustSec advisory remediation          | H-2, H-3 | IGD-02, IGD-03 |
| W3  | Lint debt cleanup                     | M-1, M-3 | IGD-04, IGD-06 |
| W4  | Governance enforcement                | M-2    | IGD-05 |
| W5  | Cosmetic + transitive dep hygiene     | L-1, L-3 | IGD-07, IGD-08 |
| W6  | Dev environment baseline              | M-4    | IGD-09 |
| W7  | In-source TODO triage                 | L-2    | IGD-10 |
| W8  | Verification + close-out              | all    | IGD-11, IGD-12 |

## 3 · Session index

| ID     | Title                                              | WS  | Files | Depends on | Effort |
|:-------|:---------------------------------------------------|:---:|:------|:-----------|:-------|
| IGD-01 | Gitignore openbao-staging + rotate Vault token     | W1  | 2     | —          | XS     |
| IGD-02 | Upgrade pyo3 to ≥0.24.1 (RUSTSEC-2025-0020)         | W2  | 2-3   | —          | S      |
| IGD-03 | Upgrade url to ≥2.5.4, lift idna ≥1.0.3 (RUSTSEC-2024-0421) | W2 | 1-2 | IGD-02 (lock sync) | XS |
| IGD-04 | Fix 3 clippy collapsible_if in mai-vault            | W3  | 1     | —          | XS     |
| IGD-05 | Install commit-msg hook enforcing co-author footer | W4  | 2-3   | —          | S      |
| IGD-06 | cargo fmt the in-flight gen-trust-staging crate     | W3  | 1     | —          | XS     |
| IGD-07 | Upgrade validator to 0.18 (drops proc-macro-error)  | W5  | 1-2   | IGD-02     | S      |
| IGD-08 | Add trailing newlines to 3 apps/ files + editorconfig | W5 | 4     | —          | XS     |
| IGD-09 | Dev-machine pip upgrade note + CI baseline check    | W6  | 1     | —          | XS     |
| IGD-10 | Triage 4 in-source TODOs (close or accept-and-track)| W7  | 1-5   | —          | S      |
| IGD-11 | Re-run full scan suite, capture green evidence      | W8  | 1     | IGD-01..IGD-10 | S |
| IGD-12 | Update MEMORY.md, close lane, plan next            | W8  | 2-3   | IGD-11     | XS     |

**XS** ≤30 min · **S** ≤1 hr · **M** ≤2 hr · **L** ≤4 hr.

## 4 · Order of operations

Run in this order (most are independent, but the chain matters where it does):

```
IGD-01  ──┐
IGD-02 ───┼──> IGD-03 ──> IGD-07
IGD-04  ──┤
IGD-05  ──┤
IGD-06  ──┤
IGD-08  ──┤
IGD-09  ──┤
IGD-10  ──┴──> IGD-11 ──> IGD-12
```

`IGD-01` is highest priority — it is the only finding that is one keystroke away from data exfiltration. `IGD-02` blocks `IGD-03` and `IGD-07` only because they all touch `Cargo.lock`; doing them in order avoids merge thrash.

## 5 · Per-session gates (must all hold to mark a session complete)

1. The change is in its **own session worktree** per `mai/docs/CONCURRENT-SESSIONS.md` (`Session-Worktree.ps1 -Action new -Session IGD-XX`).
2. Every commit ends with the canonical co-author footer.
3. `.integrity/scripts/verify-tree.sh` (or the PowerShell port if WSL bash is unavailable) returns 0 errors and 0 warnings on staged files.
4. Any batch of 3+ file changes is followed by a verification subagent spawn (per `mai/.claude/CLAUDE.md`).
5. The session's specific quality-gate run-of-record is captured: cargo audit / cargo deny / cargo clippy / cargo fmt / gitleaks / mypy / ruff — whichever the session is meant to flip.

## 6 · Exit criteria for the IGD lane

- `cargo audit` exits 0 (or only on the 3 already-ignored advisories).
- `cargo deny check` exits 0 across advisories, bans, licenses, sources.
- `cargo clippy --workspace -- -D warnings -A clippy::pedantic` exits 0.
- `cargo fmt --all -- --check` exits 0 across the **whole** tree (including any new crates).
- `gitleaks detect --no-git` returns no leaks against the working tree.
- `gitleaks detect` (full history) returns no leaks — unchanged from current baseline.
- A `commit-msg` hook is installed and rejects any commit missing the co-author footer; CI mirrors the check.
- A fresh internal GitDoctor scan run is captured in `docs/INTERNAL-GITDOCTOR-SCAN-<date>.md` and scores **≥95** overall.
- `MEMORY.md` updated with the IGD lane closure (single line in the index pointing to a per-lane memory file).

## 7 · Out of scope

- The 7 deferred SHIP runtime checks (covered by SHIP-03..SHIP-17).
- 72-hour burn-in (requires staged environment).
- GPU paths, real Vault, real trust anchors (RC2-staging work).
- The `tools/gen-trust-staging` crate's *functional* completion — IGD-06 only formats it. Functional work belongs to its own future session.
- Bundle re-roll / RC release packaging — separate lane (RC-12 or post-RC2).

---

*IGD Remediation Plan — 2026-05-26 — Authored and reviewed by Basho Parks, copyright 2026*
