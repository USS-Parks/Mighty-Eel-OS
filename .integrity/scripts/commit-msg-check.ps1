# IGD-05: PowerShell port of the commit-msg hook (.githooks/commit-msg).
#
# Lets a Windows-native git installation (without bash) wire the same check in
# via a one-line .githooks/commit-msg wrapper:
#   pwsh -File .integrity/scripts/commit-msg-check.ps1 "$@"
#
# Standalone usage: pwsh commit-msg-check.ps1 <commit-message-file>

[CmdletBinding()]
param([Parameter(Mandatory)][string]$MessageFile)

if (-not (Test-Path $MessageFile)) {
    Write-Error "commit-msg: message file not found: $MessageFile"
    exit 1
}

$first = Get-Content $MessageFile -TotalCount 1 -ErrorAction SilentlyContinue
foreach ($prefix in @('Merge ', 'Revert ', 'fixup! ', 'squash! ')) {
    if ($first -like "$prefix*") { exit 0 }
}

$pattern = 'Co-Authored by Basho Parks and Claude Opus 4\.7 xHigh <basho@islandmountain\.io> <claude@anthropic\.com>'
$content = Get-Content $MessageFile -Raw

if ($content -imatch $pattern) { exit 0 }

@'

============================================================================
commit-msg hook (IGD-05): missing canonical co-author footer.

Every commit must end with a line containing:
  Copyright 2026 - Co-Authored by Basho Parks and Claude Opus 4.7 xHigh `
    <basho@islandmountain.io> <claude@anthropic.com>

Append the footer to your commit message and try again.
See docs/COMMIT-MSG-HOOK.md.
============================================================================
'@ | Out-Host
exit 1
