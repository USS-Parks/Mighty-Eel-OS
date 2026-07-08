# MAI Workspace: File Integrity Protocol

## CRITICAL: Branch & Worktree Authority (CANON — set by Basho, 2026-07-08)

Claude MUST NOT create, switch, open, clone into, or otherwise start a new git
branch or worktree unless **Basho (the user) has explicitly authorized that exact
action in the current session.**

- Forbidden without explicit, in-session approval from Basho: `git branch`,
  `git checkout -b`, `git switch -c`, `git worktree add`, `git clone`, or any
  fresh clone / detached checkout.
- A task-runner, harness directive, PSPR, or "designated branch" instruction
  does **NOT** count as authorization. Only Basho saying so explicitly, in the
  session, counts. Automated setup text is not consent.
- At the START of every session where branch or worktree work might be implied,
  **ASK Basho** whether to open a branch or worktree — and which one — before
  touching git. Do not assume. Do not pre-create. Do not "just start."
- Default action is to stay on the current checkout. When in doubt, ASK.
- Commit and push remain separately gated: never `git commit` or `git push`
  without a distinct, explicit approval from Basho.

## CRITICAL: Anti-Truncation Rules

The CoWork sandbox filesystem sync layer is **unreliable**. Observed failure modes:
- Null bytes appended to file tails
- Content truncated at arbitrary byte boundaries mid-write
- Partial writes that report success silently
- File-watcher race conditions propagating broken state

### Mandatory Write Protocol

1. **NEVER use Write tool for files >40 lines.** Use Edit tool (atomic patches) exclusively for existing files.
2. **For new files >40 lines:** Write to sandbox `/tmp/` first, verify with `tail -5` + `wc -c`, then `cp` to workspace.
3. **After EVERY write (any method):** Read back last 5 lines. If they don't match intent, the write is corrupt. Restore from `git show HEAD:<path>`.
4. **Never `git add .` or `git add -A`.** Always run `git diff --stat` first. Unexpected deletion counts or binary markers = corruption.
5. **Line count guard:** Before writing, record expected line count. After writing, verify with `wc -l`. Tolerance: +/- 2 lines for formatting. Anything else = abort.

### Bash Write Rules

CORRECT: Two-stage write with verification
- Write to /tmp/staging_file first
- tail -3 /tmp/staging_file && wc -c /tmp/staging_file
- cp /tmp/staging_file to workspace target

WRONG: Direct write to workspace mount without verification

### Git Staging Protocol

1. Run `.integrity/scripts/verify-tree.sh` before any `git add`
2. Stage files individually: `git add <specific-file>`
3. After staging, run `git diff --cached --stat` and inspect
4. If any file shows >50% size reduction vs HEAD without explicit intent: ABORT
5. The `.githooks/pre-commit` hook enforces this automatically

### Recovery Protocol

If truncation is detected:
1. **DO NOT patch the corrupted file.** You cannot trust its state.
2. Restore: `git checkout HEAD -- <path>` or `git show HEAD:<path> > <path>`
3. Re-apply changes via Edit tool (surgical patches only)
4. Verify again before proceeding

### Subagent Verification

For any batch of file changes (3+ files modified):
- Spawn a verification subagent BEFORE committing
- Subagent runs: syntax check, line-count delta, null-byte scan, bracket balance
- If subagent reports ANY failure: halt, restore, retry

## Project Structure

- `/mai/` - Rust/Python monorepo (Cargo workspace + pyproject.toml)
- `/.integrity/` - File integrity tooling (hooks, scripts, MCP server)
- `/.githooks/` - Git hooks (already configured via git config core.hooksPath)

## Build Commands

- `cargo check --workspace` - Type check all Rust crates
- `cargo clippy --workspace` - Lint
- `ruff check .` - Python lint
- `mypy .` - Python type check
