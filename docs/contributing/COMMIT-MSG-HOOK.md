# Commit-msg hook

`.githooks/commit-msg` stamps the MAI **canonical footer** on every commit and
strips any AI co-author credit. It is a *rewriter*, not a gate: it never rejects
a commit — it fixes the message and always exits 0.

## What it stamps

Every commit message ends with exactly one footer line:

```
Authored and reviewed by Basho Parks, copyright 2026
```

Any pre-existing AI-attribution trailers — `Co-Authored-By: … Claude/Anthropic`,
`Claude-Session:`, `Generated with Claude`, a leading 🤖 line, or the legacy
`Copyright 2026 - Co-Authored by Basho Parks and Claude …` footer — are removed
first, so re-stamping is idempotent. The shared filter lives in
`.githooks/footer-filter.awk`.

> **History:** before this, IGD-05 *required* an AI co-author footer. That
> mandate is retired — the canon no longer credits an AI co-author (CANON §3).

## Activation

`git config core.hooksPath .githooks` must be set for this hook (and the
pre-commit anti-truncation gate) to fire. That config is **not** carried by a
fresh clone, so `.githooks/session-start.sh` (a Claude Code SessionStart hook
registered in `.claude/settings.json`) re-arms it at the start of every session.
To wire it by hand: `git config core.hooksPath .githooks`.

## Skipped commit types

Subjects starting with `Merge `, `Revert `, `fixup! `, or `squash! ` are left
untouched — those are typically rewritten by `git rebase --autosquash` or the host.

## Windows-native git (no bash)

Use the PowerShell port `.integrity/scripts/commit-msg-check.ps1`, or a one-line
`.githooks/commit-msg` wrapper: `pwsh -NoProfile -File .integrity/scripts/commit-msg-check.ps1 "$1"`.

## CI mirror

`.github/workflows/commit-msg-check.yml` verifies every commit in a PR/push range
carries the canonical footer and carries **no** AI co-author credit, so the policy
holds even for commits made without the local hook.
