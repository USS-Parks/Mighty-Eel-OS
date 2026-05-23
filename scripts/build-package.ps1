# scripts/build-package.ps1 - Windows orchestration helper for SHIP-08.
#
# Native .deb production requires a Linux build host. This script does
# the part of the work that's portable across platforms: builds Rust
# release binaries, runs the production guard against the ship profile,
# and stages an install layout under .\build\package-staging\. To
# produce a .deb a Linux host must run scripts/build-package.sh, but a
# Windows developer can use this script to validate the package layout
# locally before pushing.

[CmdletBinding()]
param(
    [string]$StagingDir = "",
    [switch]$ValidateOnly,
    [switch]$SkipDashboard
)

$ErrorActionPreference = 'Stop'

$RepoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
Set-Location -Path $RepoRoot

if ([string]::IsNullOrEmpty($StagingDir)) {
    $StagingDir = Join-Path $RepoRoot 'build\package-staging'
}

function Write-Log($msg) {
    Write-Host "[build-package] $msg"
}

function Require-Tool($name) {
    $cmd = Get-Command $name -ErrorAction SilentlyContinue
    if ($null -eq $cmd) {
        Write-Host "ERROR: missing required tool: $name" -ForegroundColor Red
        exit 3
    }
}

Require-Tool git
if (-not $ValidateOnly) { Require-Tool cargo }
if (-not $SkipDashboard) { Require-Tool python }

$Version = (Select-String -Path 'Cargo.toml' -Pattern '^version' | Select-Object -First 1).Line -replace '.*"([^"]+)".*', '$1'
try {
    $Commit = (git rev-parse --short=12 HEAD 2>$null)
} catch {
    $Commit = 'unknown'
}
$BuildTime = (Get-Date -Format 'yyyy-MM-ddTHH:mm:ssZ').ToString()

Write-Log "version=$Version commit=$Commit staging=$StagingDir"

# 1. Clean staging tree.
if (Test-Path $StagingDir) { Remove-Item -Recurse -Force $StagingDir }
$dirs = @(
    'usr/bin',
    'usr/lib/mai/adapters',
    'usr/lib/mai/compliance-dashboard',
    'usr/lib/mai/scripts',
    'usr/share/doc/mai',
    'lib/systemd/system',
    'etc/mai/policies',
    'etc/mai/trust-anchors',
    'DEBIAN'
)
foreach ($d in $dirs) {
    New-Item -ItemType Directory -Force -Path (Join-Path $StagingDir $d) | Out-Null
}

# 2. Rust release binary.
if (-not $ValidateOnly) {
    Write-Log "building release binaries"
    cargo build --release --workspace --locked
    if ($LASTEXITCODE -ne 0) { exit 1 }
    $bin = if ($IsWindows -or $env:OS -match 'Windows') { 'target\release\mai-api.exe' } else { 'target/release/mai-api' }
    Copy-Item -Force $bin (Join-Path $StagingDir 'usr/bin/mai-api')
}

Copy-Item -Force packaging/scripts/mai-ship-validate.sh `
    (Join-Path $StagingDir 'usr/bin/mai-ship-validate')
Copy-Item -Force packaging/scripts/mai-healthcheck.sh `
    (Join-Path $StagingDir 'usr/lib/mai/scripts/mai-healthcheck.sh')

# 3. Compliance dashboard.
if (-not $SkipDashboard) {
    Write-Log "staging compliance dashboard"
    $dashDest = Join-Path $StagingDir 'usr/lib/mai/compliance-dashboard'
    robocopy compliance-dashboard $dashDest /MIR /XD __pycache__ .venv tests /NFL /NDL /NJH /NJS | Out-Null
    if ($LASTEXITCODE -ge 8) { exit 1 }

    if (Test-Path 'compliance-dashboard/requirements.txt') {
        Write-Log "vendoring dashboard wheels"
        python -m pip download --quiet --disable-pip-version-check `
            -r compliance-dashboard/requirements.txt `
            -d (Join-Path $dashDest 'wheels')
    }
}

# 4. systemd units.
$units = @('mai-api.service', 'mai-dashboard.service', 'mai-adapter-manager.service', 'mai-healthcheck.service', 'mai-healthcheck.timer')
foreach ($u in $units) {
    Copy-Item -Force (Join-Path 'packaging/systemd' $u) `
        (Join-Path $StagingDir "lib/systemd/system/$u")
}

# 5. Config templates.
Copy-Item -Force 'config/production.example.toml' (Join-Path $StagingDir 'etc/mai/profile.toml')
Copy-Item -Force 'config/auth_keys.toml' (Join-Path $StagingDir 'etc/mai/auth_keys.toml')

@'
{
  "version": 1,
  "disable_existing_loggers": false,
  "formatters": {
    "json": {"format": "%(asctime)s %(levelname)s %(name)s %(message)s"}
  },
  "handlers": {
    "stdout": {"class": "logging.StreamHandler", "stream": "ext://sys.stdout", "formatter": "json"}
  },
  "root": {"level": "INFO", "handlers": ["stdout"]}
}
'@ | Set-Content -Path (Join-Path $StagingDir 'etc/mai/dashboard-logging.json') -Encoding utf8

# 6. Docs + metadata.
foreach ($doc in 'README.md','docs/SHIP-PROFILE.md','docs/SHIP-HARDENING-PLAN.md','packaging/README.md') {
    if (Test-Path $doc) {
        Copy-Item -Force $doc (Join-Path $StagingDir "usr/share/doc/mai/$(Split-Path $doc -Leaf)")
    }
}

@"
name=mai
version=$Version
git_commit=$Commit
build_time=$BuildTime
profile=ship
host=$env:COMPUTERNAME
"@ | Set-Content -Path (Join-Path $StagingDir 'usr/share/doc/mai/PACKAGE_BUILD_INFO') -Encoding utf8

# 7. Maintainer scripts.
foreach ($pair in @(
    @{src='packaging/scripts/preinstall.sh';  dst='DEBIAN/preinst'},
    @{src='packaging/scripts/postinstall.sh'; dst='DEBIAN/postinst'},
    @{src='packaging/scripts/preremove.sh';   dst='DEBIAN/prerm'},
    @{src='packaging/scripts/postremove.sh';  dst='DEBIAN/postrm'}
)) {
    Copy-Item -Force $pair.src (Join-Path $StagingDir $pair.dst)
}

# 8. Validate staged profile.
if (-not $ValidateOnly) {
    Write-Log "validating staged profile"
    $api = Join-Path $StagingDir 'usr/bin/mai-api'
    if ($IsWindows -or $env:OS -match 'Windows') {
        & (Join-Path 'target/release' 'mai-api.exe') validate --profile (Join-Path $StagingDir 'etc/mai/profile.toml')
    } else {
        & $api validate --profile (Join-Path $StagingDir 'etc/mai/profile.toml')
    }
    if ($LASTEXITCODE -ne 0) {
        Write-Host "ERROR: production guard rejected staged profile" -ForegroundColor Red
        exit 2
    }
}

Write-Log "staging tree ready at $StagingDir"
Write-Log "done"
