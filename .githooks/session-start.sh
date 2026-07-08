#!/usr/bin/env bash
# Claude Code SessionStart hook — re-arm the MAI git hooks in this clone.
#
# NOTE: this is a *Claude Code* hook (invoked via .claude/settings.json at the
# start of every session), NOT a git hook. It lives beside the git hooks so the
# whole set is in one place; git never runs it (git only invokes files named
# after a git event — commit-msg, pre-commit — so this name is inert to git).
#
# WHY THIS EXISTS: git stores core.hooksPath in .git/config, which git does NOT
# track or clone. Every fresh clone — and every Claude Code on the web session is
# a fresh clone in a throwaway container — therefore starts with the commit-msg
# footer stamp and the pre-commit anti-truncation gate DISCONNECTED. This hook
# re-points core.hooksPath at the committed .githooks dir at the start of every
# session, so the footer is always stamped and the integrity gate always runs.
#
# Idempotent, non-interactive, and fail-safe: it never aborts a session.
set -uo pipefail

# Resolve the working-tree root regardless of where this is invoked from.
root="$(git rev-parse --show-toplevel 2>/dev/null || echo "${CLAUDE_PROJECT_DIR:-.}")"
cd "$root" 2>/dev/null || exit 0

if [ -d .githooks ]; then
  if git config core.hooksPath .githooks 2>/dev/null; then
    echo "[session-start] core.hooksPath -> .githooks (footer + integrity hooks armed)" >&2
  else
    echo "[session-start] could not set core.hooksPath (continuing)" >&2
  fi
else
  echo "[session-start] .githooks not found; skipped hooks wiring" >&2
fi

# --- Branch & Worktree canon guard (set by Basho) ---
# The harness checks out its claude/* session branch BEFORE this hook runs,
# so the initial checkout can't be prevented — but in remote (web) sessions
# the block below moves the checkout back to the default branch. Standing
# authorization from Basho (2026-07-08): this auto-switch never creates refs
# and uses --ff-only, which cannot discard commits. The git proxy still pins
# pushes to the session branch, so landing work on main remains claude/* ->
# PR -> merge. The alert now fires only if the switch failed, or when a LOCAL
# session starts off the default branch (local checkouts are never touched).
# Canon otherwise unchanged: never create/switch a branch or worktree without
# Basho's explicit in-session approval.
default_branch="$(git symbolic-ref --quiet --short refs/remotes/origin/HEAD 2>/dev/null | sed 's#^origin/##')"
[ -z "$default_branch" ] && default_branch="main"
current_branch="$(git rev-parse --abbrev-ref HEAD 2>/dev/null || echo DETACHED)"
git_dir="$(git rev-parse --git-dir 2>/dev/null || echo)"
common_dir="$(git rev-parse --git-common-dir 2>/dev/null || echo)"
in_worktree="no"
[ -n "$git_dir" ] && [ -n "$common_dir" ] && [ "$git_dir" != "$common_dir" ] && in_worktree="yes"

# Remote sessions only: return the checkout to the default branch. The
# container snapshot's local main can be days stale while the session branch
# was cut from CURRENT main — fetch, switch, then fast-forward. Any failure
# falls through to the alert below. Local sessions are deliberately excluded.
if [ "${CLAUDE_CODE_REMOTE:-}" = "true" ] && [ "$current_branch" != "$default_branch" ] && [ "$in_worktree" = "no" ]; then
  if git fetch origin "$default_branch" >/dev/null 2>&1 \
     && git switch "$default_branch" >/dev/null 2>&1 \
     && git merge --ff-only "origin/$default_branch" >/dev/null 2>&1; then
    echo "[session-start] checkout moved: $current_branch -> $default_branch (canon auto-switch)" >&2
    current_branch="$default_branch"
  else
    echo "[session-start] AUTO-SWITCH to $default_branch FAILED; checkout left as-is" >&2
  fi
fi

if [ "$current_branch" != "$default_branch" ] || [ "$in_worktree" = "yes" ]; then
  echo "==================== BRANCH/WORKTREE CANON ALERT ====================" >&2
  echo "[session-start] This session did NOT start on the default branch." >&2
  echo "[session-start]   default branch : $default_branch" >&2
  echo "[session-start]   current branch : $current_branch" >&2
  echo "[session-start]   linked worktree: $in_worktree" >&2
  echo "[session-start] CANON (set by Basho): do NOT create or switch branches" >&2
  echo "[session-start] or open worktrees without Basho's explicit in-session" >&2
  echo "[session-start] approval. CONFIRM with Basho before proceeding here." >&2
  echo "====================================================================" >&2
fi

exit 0
