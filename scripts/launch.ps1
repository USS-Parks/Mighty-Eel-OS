# MAI one-command local launch — PowerShell variant.
#
# Usage:
#   scripts\launch.ps1                          # default config
#   scripts\launch.ps1 -Tier scout              # configs/scout.toml overlay
#   scripts\launch.ps1 -Release                 # build in release mode
#   $env:MAI_LOG_LEVEL = "debug"; scripts\launch.ps1
#
# First boot prints a one-time admin API key to stdout — save it before
# the server settles into normal logging.

[CmdletBinding()]
param(
    [string]$Tier = "",
    [switch]$Release,
    [Parameter(ValueFromRemainingArguments = $true)]
    [string[]]$ExtraArgs
)

$ErrorActionPreference = "Stop"

$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$RepoRoot = Resolve-Path (Join-Path $ScriptDir "..")
Set-Location $RepoRoot

if ($Tier -ne "") {
    $TierFile = Join-Path "configs" "$Tier.toml"
    if (-not (Test-Path $TierFile)) {
        Write-Error "tier config $TierFile not found"
        exit 2
    }
    $env:MAI_TIER_CONFIG = (Resolve-Path $TierFile).Path
    Write-Host "launch: using tier config $($env:MAI_TIER_CONFIG)"
}

if (-not $env:MAI_LOG_LEVEL) {
    $env:MAI_LOG_LEVEL = "info"
}

# BRAND-01 renamed the cargo bin to lamprey-mai-api.
$Binary = Join-Path "target" "release" "lamprey-mai-api.exe"

if ($Release) {
    Write-Host "launch: building release binary"
    cargo build --release -p mai-api
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
}

if ((Test-Path $Binary) -and $Release) {
    Write-Host "launch: running $Binary"
    & $Binary @ExtraArgs
} elseif (Test-Path $Binary) {
    Write-Host "launch: running $Binary"
    & $Binary @ExtraArgs
} else {
    Write-Host "launch: running via cargo (debug)"
    cargo run -p mai-api -- @ExtraArgs
}
