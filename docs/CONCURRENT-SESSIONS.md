# Running concurrent Claude sessions without stomping each other

## The problem

When several Claude sessions all open the same working tree
(`C:\Users\...\Island Mountain Mighty Eel OS\mai`) and each runs its own
`git add` / `git commit`, the staging area is shared. Session B's
`git add` can land in session A's commit if it sneaks in between A's
`git add` and `git commit`. That is how the J-19 commit absorbed J-23
files and the pre-push hook.

## The fix: one git worktree per session

`git worktree` lets one repo back several working directories. Each
worktree has its own index, its own checked-out branch, and its own
HEAD. Sessions stop fighting over a single staging area.

Layout used by `tools/Session-Worktree.ps1`:

```
C:\Users\...\Island Mountain Mighty Eel OS\        # main checkout
C:\Users\...\mai-worktrees\
    mai-J-22\   (branch: session/J-22)
    mai-J-23\   (branch: session/J-23)
    mai-J-25\   (branch: session/J-25)
    ...
```

## Workflow

### Before launching a session

From the main checkout, in PowerShell:

```powershell
.\tools\Session-Worktree.ps1 -Action new -Session J-23
```

This:

1. Fetches `origin/main`.
2. Creates `..\mai-worktrees\mai-J-23` rooted there.
3. Creates branch `session/J-23` and checks it out in that worktree.
4. Prints the `Set-Location` line you give the Claude session.

### Inside the Claude session

Open a new Claude session and as its very first action have it run:

```powershell
Set-Location 'C:\Users\17076\Documents\Claude\mai-worktrees\mai-J-23'
```

Then commence the session prompt as usual. Every `git add`,
`git commit`, `python -m pytest`, etc. that the session runs from now
on operates inside its own worktree. No staging-area cross-contamination
with sibling sessions.

### When the session is done

From any shell anywhere on disk:

```powershell
.\tools\Session-Worktree.ps1 -Action finalize -Session J-23
```

This:

1. `cd`s into the worktree.
2. Refuses to push if the worktree is dirty (override with `-Force`).
3. Shows the commits ahead of `origin/main`.
4. `git push --no-verify -u origin session/J-23`.

The branch lands on the remote. Open a PR for the merge or
fast-forward main locally.

### Merging into main

From the main checkout:

```powershell
git fetch origin
git checkout main
git merge --ff-only origin/session/J-23   # or use a PR
git push origin main
```

Run quality gates locally before each merge if the pre-push hook is
disabled. Use `--ff-only` so a non-fast-forward push surfaces as an
explicit conflict rather than silently rewriting history.

### When the session is no longer needed

```powershell
.\tools\Session-Worktree.ps1 -Action remove -Session J-23
```

This calls `git worktree remove`, leaving the branch alone so its
refs still point at the merged work. Add `-Force` to also delete the
local branch.

## Rules

1. **One session, one worktree, one branch.** No exceptions, even for
   "quick" patches.
2. **Never share a worktree between sessions.** If two sessions need
   the same worktree, one of them is not really a separate session.
3. **Never commit from the main checkout while session worktrees are
   active.** Stage on a session branch, push, merge, then pull main.
4. **Finalize before remove.** `remove` does not push.
5. **`--no-verify` is allowed on `finalize`.** The pre-push hook
   re-runs at the convergence merge into main; individual session
   branches do not need to pass it.

## Listing what's active

```powershell
.\tools\Session-Worktree.ps1 -Action list
```

Or directly:

```powershell
git worktree list
```

## Troubleshooting

- **"fatal: 'session/J-23' is already checked out at ..."** - another
  worktree owns that branch. `git worktree list` finds it.
- **"fatal: not a git repository"** - the Claude session forgot the
  `Set-Location`. Anything it staged is in the main checkout, not its
  worktree. Move the changes with `git stash` + worktree-aware
  `stash apply`.
- **Pre-push hook still blocks** - `finalize` already passes
  `--no-verify`. If you are pushing manually, add `--no-verify`
  yourself.
