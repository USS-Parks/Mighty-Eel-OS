# OpenBao Incident Response — MAI Trust Core Operations
# ======================================================
#
# Automates common incident-response tasks: credential revocation,
# audit log interrogation, credential rotation, and health checks.
#
# Usage:
#   .\ir-respond.ps1 health-check
#   .\ir-respond.ps1 revoke-secret <secret_id>
#   .\ir-respond.ps1 audit-since <minutes>
#   .\ir-respond.ps1 audit-by-token <prefix>
#   .\ir-respond.ps1 rotate-appliance
#
# PowerShell 5.1 compatible.

param(
    [Parameter(Position=0)]
    [ValidateSet("revoke-secret","audit-since","audit-by-token","rotate-appliance","health-check")]
    [string]$Action,

    [Parameter(ValueFromRemainingArguments)]
    [string[]]$ActionArgs
)

$ErrorActionPreference = "Stop"
$baoAddr = if ($env:MAI_OPENBAO_ADDR) { $env:MAI_OPENBAO_ADDR } else { "http://localhost:8200" }
$rootToken = if ($env:MAI_OPENBAO_ROOT_TOKEN) { $env:MAI_OPENBAO_ROOT_TOKEN } else { "root" }
$baoHeaders = @{"X-Vault-Token"=$rootToken}
$containerName = "openbao-trust-core"
$roleId = "8053c291-8f60-381f-e283-5e645e5907f4"

function Write-IR($Level, $Message) {
    $ts = Get-Date -Format "yyyy-MM-ddTHH:mm:ssZ"
    Write-Host "[$ts] [$Level] $Message"
}

function Invoke-BaoAPI($Method="GET", $Path, $Body) {
    $uri = "$baoAddr/v1/$Path"
    $params = @{Uri=$uri; Headers=$baoHeaders; Method=$Method; ContentType="application/json"; TimeoutSec=10}
    if ($Body) { $params.Body = ($Body | ConvertTo-Json -Depth 5) }
    $r = Invoke-RestMethod @params
    return $r
}

# --- helpers ---

function Read-AuditLog {
    docker exec $containerName cat /var/log/openbao/audit.log 2>$null
}

function Parse-AuditEvents($logLines) {
    $logLines -split "`n" | Where-Object { $_ -match '^\{.*"time"' } | ForEach-Object {
        try { $_ | ConvertFrom-Json } catch { $null }
    } | Where-Object { $_ -ne $null }
}

# === ACTIONS ===

if ($Action -eq "revoke-secret") {
    $targetSecret = $ActionArgs[0]
    if (-not $targetSecret) {
        Write-Host "Usage: ir-respond.ps1 revoke-secret SECRET_ID"
        exit 1
    }
    Write-IR "INFO" "Revoking secret_id $targetSecret"
    Invoke-BaoAPI -Method Post -Path "auth/approle/role/mai-appliance/secret-id/destroy" -Body @{secret_id=$targetSecret} | Out-Null
    Write-IR "INFO" "Destroy called. Verifying..."

    $loginBody = @{role_id=$roleId; secret_id=$targetSecret}
    $revoked = $true
    try {
        Invoke-BaoAPI -Method Post -Path "auth/approle/login" -Body $loginBody | Out-Null
        $revoked = $false
    } catch { }
    if ($revoked) {
        Write-IR "INFO" "Secret successfully revoked."
    } else {
        Write-IR "WARN" "Secret may still be valid."
    }
    exit 0
}

if ($Action -eq "audit-since") {
    $minutes = if ($ActionArgs[0] -match '^\d+$') { [int]$ActionArgs[0] } else { 15 }
    Write-IR "INFO" "Audit scan: last $minutes minutes"

    $log = Read-AuditLog
    if (-not $log) {
        Write-IR "WARN" "No audit log found."
        exit 1
    }

    $events = Parse-AuditEvents $log
    $cutoff = [DateTimeOffset]::UtcNow.AddMinutes(-$minutes)
    $recent = $events | Where-Object {
        try { [DateTimeOffset]::Parse($_.time) -gt $cutoff } catch { $false }
    }

    Write-IR "INFO" "$($recent.Count) events in window ($($events.Count) total)"

    if ($recent.Count -eq 0) { exit 0 }

    $grouped = $recent | Group-Object { $_.request.path } | Sort-Object Count -Descending
    Write-Host "Activity by path:"
    $grouped | ForEach-Object { Write-Host "  $($_.Count)x  $($_.Name)" }

    $failures = $recent | Where-Object { $_.response.status -ge 400 }
    if ($failures.Count -gt 0) {
        Write-IR "WARN" "$($failures.Count) failed requests:"
        $failures | ForEach-Object { Write-Host "  $($_.time) $($_.request.method) $($_.request.path) -> $($_.response.status)" }
    }
    exit 0
}

if ($Action -eq "audit-by-token") {
    $prefix = $ActionArgs[0]
    if (-not $prefix) {
        Write-Host "Usage: ir-respond.ps1 audit-by-token TOKEN_PREFIX"
        exit 1
    }
    Write-IR "INFO" "Tracing events for token prefix: $prefix"

    $log = Read-AuditLog
    $matches = $log -split "`n" | Where-Object { $_ -match [regex]::Escape($prefix) }
    if ($matches.Count -eq 0) {
        Write-IR "INFO" "No events found."
        exit 0
    }
    Write-IR "INFO" "$($matches.Count) events:"
    $matches | ForEach-Object {
        $e = $_ | ConvertFrom-Json
        Write-Host "  $($e.time) $($e.type) $($e.request.method) $($e.request.path) [$($e.response.status)]"
    }
    exit 0
}

if ($Action -eq "rotate-appliance") {
    Write-IR "INFO" "Rotating MAI appliance credentials..."

    $resp = Invoke-BaoAPI -Method Post -Path "auth/approle/role/mai-appliance/secret-id" -Body @{}
    $newSecret = $resp.data.secret_id
    $newAccessor = $resp.data.secret_id_accessor
    Write-IR "INFO" "New secret_id issued (accessor: $newAccessor)"

    Write-Host ""
    Write-Host "=== Credential Rotation Complete ===" -ForegroundColor Green
    Write-Host "  New secret_id: $newSecret" -ForegroundColor Yellow
    Write-Host "  Accessor:      $newAccessor"
    Write-Host ""
    Write-Host "Set for mai-api:"
    Write-Host "  " -NoNewline
    Write-Host ('$env:MAI_OPENBAO_SECRET_ID = ''' + $newSecret + '''') -ForegroundColor Cyan
    Write-Host ""
    Write-Host "To revoke old credentials: .\ir-respond.ps1 revoke-secret OLD_SECRET_ID"
    exit 0
}

if ($Action -eq "health-check") {
    Write-IR "INFO" "Running trust core health check..."

    $seal = Invoke-BaoAPI -Method Get -Path "sys/seal-status"
    Write-Host "  Sealed:       $($seal.sealed)"
    Write-Host "  Version:      $($seal.version)"

    $mounts = Invoke-BaoAPI -Method Get -Path "sys/mounts"
    $mountKeys = ($mounts.data | Get-Member -MemberType NoteProperty).Name -join ', '
    Write-Host "  Mounts:       $mountKeys"

    $auths = Invoke-BaoAPI -Method Get -Path "sys/auth"
    $authKeys = ($auths.data | Get-Member -MemberType NoteProperty).Name -join ', '
    Write-Host "  Auth:         $authKeys"

    try {
        $audit = Invoke-BaoAPI -Method Get -Path "sys/audit"
        $auditKeys = ($audit.data | Get-Member -MemberType NoteProperty).Name -join ', '
        Write-Host "  Audit:        $auditKeys"
    } catch {
        Write-Host "  Audit:        NONE (WARNING)"
    }

    $policies = Invoke-BaoAPI -Method Get -Path "sys/policies/acl?list=true"
    Write-Host "  Policies:     $($policies.data.keys -join ', ')"

    try {
        $tenant = Invoke-BaoAPI -Method Get -Path "kv/data/tenants/tribal-health-demo"
        Write-Host "  Demo tenant:  present"
    } catch {
        Write-Host "  Demo tenant:  MISSING"
    }

    $claimKey = Invoke-BaoAPI -Method Get -Path "transit/keys/lamprey-claim-signer"
    Write-Host "  Claim key:    version $($claimKey.data.latest_version)"

    $logLines = docker exec $containerName sh -c "wc -l /var/log/openbao/audit.log 2>/dev/null || echo 'no audit log'" 2>$null
    Write-Host "  Audit log:    $($logLines.Trim())"

    Write-Host ""
    Write-Host "Health check complete." -ForegroundColor Green
    exit 0
}

Write-Host @"
OpenBao Incident Response — MAI Trust Core Operations

Usage:
  ir-respond.ps1 health-check
  ir-respond.ps1 revoke-secret SECRET_ID
  ir-respond.ps1 audit-since MINUTES
  ir-respond.ps1 audit-by-token TOKEN_PREFIX
  ir-respond.ps1 rotate-appliance

Environment:
  MAI_OPENBAO_ADDR        OpenBao address (default: http://localhost:8200)
  MAI_OPENBAO_ROOT_TOKEN  Root token  (default: root, dev mode)
"@
exit 1
