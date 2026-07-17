---
name: safe-edit
description: "MANDATORY skill for ALL file edits in the Mighty-Eel-OS monorepo. This skill MUST be used any time Claude needs to modify a file that lives in the workspace-mounted folder (the user's Windows filesystem). It prevents the sandbox truncation bug that corrupts files when sed/Write/bash-redirect is used through the Linux mount. Use this skill EVERY TIME you are about to edit .rs, .py, .toml, .yml, or any other file in the MAI repo. If you catch yourself reaching for sed -i, Write tool on a >40 line file, or any bash redirect targeting the workspace mount: STOP and use this skill instead. No exceptions."
---

# Safe Edit Protocol for MAI Monorepo

## The Problem

The CoWork sandbox mounts the user's Windows filesystem at `/sessions/*/mnt/`. File writes through this mount are unreliable:

- `sed -i` through bash truncates files silently (confirmed every session)
- The `Write` tool on files >40 lines triggers the same sync corruption
- Partial writes report success but produce 10-30% of original content
- The pre-commit hook catches it, but by then the damage is done and recovery wastes 10-20 minutes

## The Rules (HARD STOPS - NO EXCEPTIONS)

### NEVER DO (will corrupt files):

1. `sed -i` on any file in the workspace mount
2. `Write` tool on any existing file >40 lines in the workspace
3. Bash redirects (`>`, `>>`) targeting workspace-mounted paths
4. Any bash command that modifies files in `/sessions/*/mnt/Island Mountain Mighty Eel OS/`
5. `Edit` tool with large replacement blocks (>50 lines of new_string)

### ALWAYS DO INSTEAD:

For files ≤40 lines that need full replacement:
- Use the `Edit` tool (atomic patches via the Windows file path, e.g. `C:\Users\...\`)

For surgical edits (1-5 lines changed) on ANY size file:
- Use the `Edit` tool with small, precise `old_string` -> `new_string` patches
- Use the WINDOWS path (C:\Users\...), never the sandbox mount path

For edits that require multiple changes or are complex:
- Generate PowerShell commands for the user to run
- Use `[System.IO.File]::ReadAllText()` and `::WriteAllText()` (PS 5.1 compatible, no BOM)
- Always include a size verification step: `(Get-Item $file).Length`

For new files:
- Write to sandbox `/tmp/` first
- Verify with `wc -l` and `tail -5`
- Then copy to workspace OR use `Write` tool to the Windows path if ≤40 lines

## Decision Flowchart

```
Need to edit a file in the MAI repo?
|
+-- Is it a NEW file (doesn't exist yet)?
|   +-- ≤40 lines? -> Write tool directly to C:\Users\...\path
|   +-- >40 lines? -> Write to /tmp/, verify, then copy
|
+-- Is it an EXISTING file?
|   +-- Change is ≤5 lines and old_string is unique?
|   |   -> Edit tool (Windows path, small atomic patch)
|   |
|   +-- Change is 5-20 lines?
|   |   -> Multiple sequential Edit tool calls (Windows path)
|   |
|   +-- Change is >20 lines or structural?
|       -> Generate PowerShell commands for user
|       -> Include [System.IO.File]::ReadAllText/WriteAllText
|       -> Include (Get-Item $file).Length verification
|       -> NEVER attempt through sandbox
|
+-- Is it a git operation?
    -> Run git diff --stat BEFORE git add
    -> Stage files individually (never git add .)
    -> If any file shows >50% size reduction: ABORT
```

## PowerShell Command Templates (PS 5.1 Compatible)

### Single-line deletion:
```powershell
$file = "$PWD\path\to\file.rs"
$content = [System.IO.File]::ReadAllText($file)
$content = $content -replace "exact line to remove\r?\n", ""
[System.IO.File]::WriteAllText($file, $content)
(Get-Item $file).Length  # verify
```

### Single-line replacement:
```powershell
$file = "$PWD\path\to\file.rs"
$content = [System.IO.File]::ReadAllText($file)
$content = $content -replace "old_pattern", "new_replacement"
[System.IO.File]::WriteAllText($file, $content)
(Get-Item $file).Length  # verify
```

### Multi-line block replacement:
```powershell
$file = "$PWD\path\to\file.rs"
$content = [System.IO.File]::ReadAllText($file)
$old = @"
line 1
line 2
line 3
"@
$new = @"
replacement line 1
replacement line 2
"@
$content = $content.Replace($old, $new)
[System.IO.File]::WriteAllText($file, $content)
(Get-Item $file).Length  # verify
```

## Pre-Edit Checklist (run mentally before EVERY edit)

1. Am I using a sandbox path (`/sessions/*/mnt/`)? -> STOP. Use Windows path or PowerShell.
2. Is the file >40 lines? -> STOP. Use Edit tool (small patches) or PowerShell commands.
3. Is my Edit tool `new_string` >50 lines? -> STOP. Break into multiple patches or use PowerShell.
4. Am I about to use `sed -i`? -> STOP. Always. No exceptions. Ever.
5. Did I READ the file first (via Read tool or bash cat)? -> If not, read it.

## Post-Edit Verification

After ANY edit completes (whether via Edit tool or user-run PowerShell):

1. Read back the edited region to confirm the change applied
2. For PowerShell edits: user reports file size from `(Get-Item $file).Length`
3. Before git staging: `git diff --stat` must show reasonable line counts
4. If any file shows >50% size reduction vs HEAD without explicit intent: RESTORE from HEAD

## Common Failure Patterns to Watch For

| Symptom | Cause | Prevention |
|---------|-------|-----------|
| File goes from 200+ lines to 30 | sed -i through mount | Never use sed -i on mount |
| Null bytes at end of file | Write tool sync race | Use Edit tool instead |
| Git hook blocks with "TRUNCATED" | Any of the above | Follow this skill |
| "Everything up-to-date" after push | Commit was rejected, user didn't notice | Always check commit output |

## When Reading Files is Safe

Reading through the sandbox mount is fine. The corruption only happens on WRITES. So:

- `cat`, `grep`, `sed -n` (print only), `head`, `tail` through bash: SAFE
- `Read` tool on Windows paths: SAFE
- `wc -l`, `diff`, `git diff`: SAFE

The danger is exclusively in the write path.
