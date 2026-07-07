# PowerShell port of the commit-msg hook (.githooks/commit-msg).
# Stamps the MAI canonical footer and strips AI co-author credit. Idempotent and
# fail-safe (always exits 0). For Windows-native git without bash, wire via a
# one-line .githooks/commit-msg wrapper:
#   pwsh -NoProfile -File .integrity/scripts/commit-msg-check.ps1 "$@"
# Standalone: pwsh commit-msg-check.ps1 <commit-message-file>

[CmdletBinding()]
param([Parameter(Mandatory)][string]$MessageFile)

$ErrorActionPreference = 'SilentlyContinue'
if (-not (Test-Path -LiteralPath $MessageFile)) { exit 0 }

$footer = 'Authored and reviewed by Basho Parks, copyright 2026'
$strip  = '(?i)co-?authored[ -]?by.*(claude|anthropic|noreply@anthropic|claude@anthropic|gpt|copilot|cursor)'

$kept = New-Object System.Collections.Generic.List[string]
foreach ($l in (Get-Content -LiteralPath $MessageFile)) {
    if ($l -match $strip) { continue }
    if ($l -match '^Claude-Session:') { continue }
    if ($l -match 'Generated with (\[)?Claude') { continue }
    if ($l -match '^\p{So}') { continue }        # leading emoji/symbol line (e.g. robot)
    if ($l -match '^Authored and reviewed by') { continue }
    $kept.Add($l)
}
while ($kept.Count -gt 0 -and [string]::IsNullOrWhiteSpace($kept[$kept.Count - 1])) {
    $kept.RemoveAt($kept.Count - 1)
}
if ($kept.Count -gt 0) { $kept.Add('') }
$kept.Add($footer)

Set-Content -LiteralPath $MessageFile -Value $kept -Encoding utf8
exit 0
