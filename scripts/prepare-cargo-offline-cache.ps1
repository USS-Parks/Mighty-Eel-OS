# Prepare and verify the Cargo cache required for offline evidence runs.
#
# Usage:
#   scripts\prepare-cargo-offline-cache.ps1
#   scripts\prepare-cargo-offline-cache.ps1 -VerifyOnly
#
# Default mode may use the network through `cargo fetch --locked`.
# VerifyOnly mode never prepares; it only proves the current cache can
# satisfy Cargo.lock with CARGO_NET_OFFLINE=true.

[CmdletBinding()]
param(
    [switch]$VerifyOnly,
    [switch]$RunGates
)

$ErrorActionPreference = "Stop"
if (Get-Variable -Name PSNativeCommandUseErrorActionPreference -ErrorAction SilentlyContinue) {
    $PSNativeCommandUseErrorActionPreference = $false
}

$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$RepoRoot = Resolve-Path (Join-Path $ScriptDir "..")
Set-Location $RepoRoot

function Invoke-CargoStep {
    param([string]$Name, [string[]]$CargoArgs)
    Write-Host "==> $Name"
    $global:LASTEXITCODE = 0
    & cargo @CargoArgs
    if ($LASTEXITCODE -ne 0) {
        throw "$Name failed with exit code $LASTEXITCODE"
    }
}

if (-not (Test-Path "Cargo.toml") -or -not (Test-Path "Cargo.lock")) {
    throw "Cargo.toml and Cargo.lock must exist in the repository root"
}

if (-not $VerifyOnly) {
    Remove-Item Env:\CARGO_NET_OFFLINE -ErrorAction SilentlyContinue
    Invoke-CargoStep -Name "prepare Cargo cache from Cargo.lock" -CargoArgs @("fetch", "--locked")
}

$env:CARGO_NET_OFFLINE = "true"
Invoke-CargoStep -Name "verify Cargo.lock resolves from offline cache" -CargoArgs @("fetch", "--locked")

if ($RunGates) {
    Invoke-CargoStep -Name "cargo check --workspace" -CargoArgs @("check", "--workspace")
    Invoke-CargoStep -Name "cargo clippy --workspace" -CargoArgs @("clippy", "--workspace", "--", "-D", "warnings", "-A", "clippy::pedantic")
    Invoke-CargoStep -Name "cargo test --workspace" -CargoArgs @("test", "--workspace")
    Invoke-CargoStep -Name "cargo deny check" -CargoArgs @("deny", "check")
}

Write-Host "OK: Cargo offline cache verified"
