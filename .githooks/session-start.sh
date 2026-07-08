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
# The harness checks out the session BEFORE this hook runs, so this can only
# DETECT and loudly flag an unexpected branch/worktree at session start — it
# cannot prevent the initial checkout. Canon: never create/switch a branch or
# worktree without Basho's explicit in-session approval. This alert forces a
# confirmation when a session did not start on the default branch.
default_branch="$(git symbolic-ref --quiet --short refs/remotes/origin/HEAD 2>/dev/null | sed 's#^origin/##')"
[ -z "$default_branch" ] && default_branch="main"
current_branch="$(git rev-parse --abbrev-ref HEAD 2>/dev/null || echo DETACHED)"
git_dir="$(git rev-parse --git-dir 2>/dev/null || echo)"
common_dir="$(git rev-parse --git-common-dir 2>/dev/null || echo)"
in_worktree="no"
[ -n "$git_dir" ] && [ -n "$common_dir" ] && [ "$git_dir" != "$common_dir" ] && in_worktree="yes"
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
