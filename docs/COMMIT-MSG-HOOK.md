# Commit-msg hook (IGD-05)

`.githooks/commit-msg` enforces the MAI canonical co-author footer on every commit. Before IGD-05 the footer was a convention (138/303 commits as of 2026-05-26); afterwards it is a gate.

## Required pattern

The hook accepts any commit whose message contains a line matching (case-insensitive):

```
Co-Authored by Basho Parks and Claude Opus 4.7 xHigh <basho@islandmountain.io> <claude@anthropic.com>
```

The canonical form (used in every existing in-spec commit) is:

```
Copyright 2026 - Co-Authored by Basho Parks and Claude Opus 4.7 xHigh <basho@islandmountain.io> <claude@anthropic.com>
```

The leading `Copyright 2026 - ` is recommended but not required by the regex.

## Skipped commit types

Auto-generated subjects starting with `Merge `, `Revert `, `fixup! `, or `squash! ` skip the check — those messages are typically rewritten by `git rebase --autosquash` or by the host (GitHub) and shouldn't fail an honest contributor's local commit.

## Activation

The repo already runs `git config core.hooksPath .githooks` (used by the pre-commit anti-truncation hook). Dropping `.githooks/commit-msg` in place is all that's needed. If your clone is missing the config (the pre-commit hook also wouldn't fire), run it once.

## Windows-native git (no bash)

The bash hook runs fine under Git Bash on Windows. If your installation lacks bash, use the PowerShell port at `.integrity/scripts/commit-msg-check.ps1` directly, or replace `.githooks/commit-msg` with a one-line wrapper that calls `pwsh -NoProfile -File .integrity/scripts/commit-msg-check.ps1 "$1"` (or `powershell.exe` on a Windows machine without PowerShell 7).

## CI mirror

`.github/workflows/commit-msg-check.yml` re-runs the same regex against every commit in an incoming PR's range, so the gate also catches commits that bypassed the local hook with `--no-verify`.
