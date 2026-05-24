# MAI File Integrity Suite

Anti-truncation, anti-corruption tooling for CoWork sandbox sessions.

## Problem

The CoWork sandbox filesystem sync layer introduces three failure modes:
1. **Mid-file truncation** - files cut off at arbitrary byte boundaries
2. **Null-byte injection** - \x00 appended to file tails
3. **Watcher race conditions** - incomplete writes propagated to builds/git

## Components

### 1. CLAUDE.md Directives (`../.claude/CLAUDE.md`)
Mandatory rules for any AI agent writing files in this workspace:
- Never use Write tool for files >40 lines
- Two-stage write protocol (write to /tmp, verify, copy)
- Post-write verification on every operation
- Git staging protocol (never `git add .`)

### 2. Subagent Configuration (`subagent-config.md`)
Template and rules for spawning a verification subagent after batch writes.
Independent verification prevents confirmation bias from the writing agent.

### 3. Hook Scripts

| Script | Purpose |
|--------|---------|
| `hooks/pre-commit` | Blocks commits with null bytes, truncation, syntax errors |
| `scripts/verify-tree.sh` | Scan modified files for corruption before staging |
| `scripts/safe-write.sh` | Atomic two-stage write with verification |
| `scripts/post-write-verify.sh` | Verify single file after write operation |
| `scripts/quick-check.sh` | Fast single-file check (no dependencies) |

### 4. MCP Server (`mcp-server/`)
Node.js MCP server providing structured tools:
- `validate_file` - Full integrity check on one file
- `safe_write` - Atomic write with built-in verification
- `verify_batch` - Check multiple files at once
- `line_count_guard` - Expected vs actual line count
- `pre_stage_check` - Full tree verification before git add

## Installation

```bash
# Set git to use the integrity hooks
cd mai/
git config core.hooksPath .integrity/hooks

# The hook directory includes:
# - pre-commit: file corruption, syntax, and formatting guard

# Install MCP server dependencies (if using Node tools)
cd .integrity/mcp-server && npm install

# Or just use the bash scripts directly (no deps needed)
chmod +x .integrity/scripts/*.sh
```

## Usage in CoWork Sessions

### Before writing:
```
Record expected line count for target file
```

### After writing:
```bash
.integrity/scripts/quick-check.sh <file> <expected-lines>
# or
.integrity/scripts/post-write-verify.sh <file> <expected-lines>
```

### Before git add:
```bash
.integrity/scripts/verify-tree.sh
```

### For batch operations:
Spawn verification subagent per `subagent-config.md` template.

## Recovery

If corruption is detected:
1. DO NOT attempt to patch the corrupted file
2. Restore: `git checkout HEAD -- <path>`
3. Re-apply changes via Edit tool (surgical patches only)
4. Verify again before proceeding
