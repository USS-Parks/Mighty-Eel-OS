#!/usr/bin/env pwsh
# Session-Worktree.ps1 - create / list / finalize / remove per-session git
# worktrees so parallel Claude sessions never collide in the staging area.
#
# Usage:
#   .\tools\Session-Worktree.ps1 -Action new -Session J-23
#   .\tools\Session-Worktree.ps1 -Action list
#   .\tools\Session-Worktree.ps1 -Action finalize -Session J-23
#   .\tools\Session-Worktree.ps1 -Action remove -Session J-23
#
# Worktrees live at:
#   <repo-parent>/mai-worktrees/mai-<session>
#
# Branch name:
#   session/<session>
#
# Each Claude session is told to Set-Location into its worktree path and
# do all git work there. Independent index, independent working tree,
# independent staging area - no more cross-session contamination.

param(
    [Parameter(Mandatory)][ValidateSet('new', 'list', 'finalize', 'remove')]
    [string]$Action,

    [string]$Session,
    [string]$Base = "origin/main",
    [switch]$NoPush,
    [switch]$Force
)

$ErrorActionPreference = 'Stop'

function Get-RepoRoot {
    $root = git rev-parse --show-toplevel 2>$null
    if (-not $root) { throw "Not inside a git repo." }
    return $root.Trim()
}

function Get-WorktreesRoot([string]$repoRoot) {
    $parent = Split-Path $repoRoot -Parent
    return Join-Path $parent "mai-worktrees"
}

function Get-WorktreePath([string]$repoRoot, [string]$session) {
    return Join-Path (Get-WorktreesRoot $repoRoot) "mai-$session"
}

function Get-BranchName([string]$session) {
    return "session/$session"
}

function Require-Session() {
    if (-not $Session) { throw "This action needs -Session J-XX." }
    if ($Session -notmatch '^[A-Za-z]+-[0-9]+[a-z]?$') {
        throw "Session id should look like 'J-23' or 'J-10b' (got '$Session')."
    }
}

$repoRoot = Get-RepoRoot

switch ($Action) {
    'new' {
        Require-Session
        $wtPath = Get-WorktreePath $repoRoot $Session
        $branch = Get-BranchName $Session
        $wtRoot = Get-WorktreesRoot $repoRoot

        if (-not (Test-Path $wtRoot)) {
            New-Item -ItemType Directory -Path $wtRoot | Out-Null
        }
        if (Test-Path $wtPath) {
            throw "Worktree already exists at $wtPath. Use -Action remove first, or pick a different session id."
        }

        Write-Host "Fetching origin..." -ForegroundColor DarkGray
        git fetch origin --quiet

        # If the branch already exists locally, reuse it; otherwise create from Base.
        $existing = git branch --list $branch
        if ($existing) {
            Write-Host "Reusing existing branch $branch" -ForegroundColor Yellow
            git worktree add $wtPath $branch
        } else {
            Write-Host "Creating branch $branch from $Base" -ForegroundColor DarkGray
            git worktree add $wtPath -b $branch $Base
        }

        Write-Host ""
        Write-Host "Worktree ready:" -ForegroundColor Green
        Write-Host "  Path:   $wtPath"
        Write-Host "  Branch: $branch"
        Write-Host ""
        Write-Host "Tell your Claude session to start with:" -ForegroundColor Cyan
        Write-Host "  Set-Location '$wtPath'"
        Write-Host ""
        Write-Host "When the session is done, finalize from anywhere with:" -ForegroundColor Cyan
        Write-Host "  .\tools\Session-Worktree.ps1 -Action finalize -Session $Session"
    }

    'list' {
        git worktree list
    }

    'finalize' {
        Require-Session
        $wtPath = Get-WorktreePath $repoRoot $Session
        $branch = Get-BranchName $Session

        if (-not (Test-Path $wtPath)) {
            throw "No worktree at $wtPath. Did you create it with -Action new?"
        }

        Push-Location $wtPath
        try {
            $dirty = git status --porcelain
            if ($dirty) {
                Write-Host "Worktree has uncommitted changes:" -ForegroundColor Red
                Write-Host $dirty
                if (-not $Force) {
                    throw "Refusing to finalize a dirty worktree. Commit/discard first, or pass -Force to ignore (untracked files only)."
                }
            }

            $unpushed = git log "$Base..HEAD" --oneline 2>$null
            if (-not $unpushed) {
                Write-Host "No commits ahead of $Base on $branch. Nothing to push." -ForegroundColor Yellow
                return
            }

            Write-Host "Commits to push on $branch :" -ForegroundColor Cyan
            Write-Host $unpushed

            if ($NoPush) {
                Write-Host "Skipping push (-NoPush)." -ForegroundColor Yellow
                return
            }

            Write-Host "Pushing $branch to origin..." -ForegroundColor Cyan
            git push --no-verify -u origin $branch
            Write-Host ""
            Write-Host "Pushed. Open a PR or merge to main from origin." -ForegroundColor Green
        } finally {
            Pop-Location
        }
    }

    'remove' {
        Require-Session
        $wtPath = Get-WorktreePath $repoRoot $Session
        $branch = Get-BranchName $Session

        if (-not (Test-Path $wtPath)) {
            Write-Host "No worktree at $wtPath (already removed?)." -ForegroundColor Yellow
        } else {
            git worktree remove $wtPath $(if ($Force) { "--force" })
            Write-Host "Removed worktree at $wtPath." -ForegroundColor Green
        }

        # Branch removal is opt-in via -Force - finalize already pushed it,
        # so the default is to keep the local ref pointing at the merged work.
        if ($Force) {
            $existing = git branch --list $branch
            if ($existing) {
                git branch -D $branch
                Write-Host "Deleted local branch $branch." -ForegroundColor Green
            }
        }
    }
}
