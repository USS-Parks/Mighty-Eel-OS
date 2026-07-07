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

exit 0
