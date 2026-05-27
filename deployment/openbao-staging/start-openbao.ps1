# OpenBao Trust Core — Staging Environment Setup
# =============================================
#
# Starts a dev-mode OpenBao container with file audit enabled from boot,
# then configures the full Trust Manifold surface: KV, Transit, PKI,
# AppRole auth, ACL policies, and demo tenant.
#
# Usage:
#   pwsh deploy/openbao-staging/start-openbao.ps1
#
# Outputs:
#   - MAI_OPENBAO_SECRET_ID  (env var for mai-api staging)
#   - Audit log at openbao-audit\audit.log on host
#
# Prerequisites: Docker Desktop running

param(
    [string]$Port = "8200",
    [string]$AuditDir = "$PSScriptRoot\openbao-audit",
    [switch]$KeepExisting
)

$ErrorActionPreference = "Stop"
$containerName = "openbao-trust-core"

# ── Stop existing container if requested ─────────────────────────────
if (-not $KeepExisting) {
    docker rm -f $containerName 2>$null
}

# ── Create audit directory on host ───────────────────────────────────
New-Item -ItemType Directory -Path $AuditDir -Force | Out-Null

# ── Start OpenBao with file audit enabled at boot ───────────────────
# The BAO_LOCAL_CONFIG env var injects an audit device declaratively.
# Dev mode (inmem storage) is fine for staging — audit survives on host.
Write-Host "Starting OpenBao with audit enabled..." -ForegroundColor Cyan
docker run -d `
    --name $containerName `
    --cap-add=IPC_LOCK `
    -p ${Port}:8200 `
    -v "${AuditDir}:/var/log/openbao" `
    -e BAO_DEV_ROOT_TOKEN_ID=root `
    -e BAO_DEV_LISTEN_ADDRESS=0.0.0.0:8200 `
    -e BAO_LOCAL_CONFIG='{
        "audit": [{
            "type": "file",
            "options": {
                "file_path": "/var/log/openbao/audit.log",
                "log_raw": "true"
            }
        }]
    }' `
    openbao/openbao:latest `
    server -dev -dev-root-token-id=root

# ── Wait for OpenBao to be ready ────────────────────────────────────
Write-Host "Waiting for OpenBao to unseal..." -ForegroundColor Yellow
for ($i = 0; $i -lt 30; $i++) {
    try {
        $status = Invoke-RestMethod -Uri "http://localhost:${Port}/v1/sys/seal-status" -TimeoutSec 2
        if (-not $status.sealed) { break }
    } catch { }
    Start-Sleep -Seconds 1
}
Write-Host "OpenBao ready (version $($status.version))" -ForegroundColor Green

# ── Helper: bao CLI via docker exec ──────────────────────────────────
$baoToken = @{"X-Vault-Token"="root"}
function Invoke-Bao {
    param([string]$Method="GET", [string]$Path, $Body, [switch]$NoBody)
    $uri = "http://localhost:${Port}/v1/${Path}"
    $params = @{Uri=$uri; Headers=$baoToken; Method=$Method; ContentType="application/json"; TimeoutSec=10}
    if (-not $NoBody -and $Body) { $params.Body = ($Body | ConvertTo-Json -Depth 5) }
    try {
        $r = Invoke-RestMethod @params
        if ($r -and -not $NoBody) { return $r }
    } catch {
        Write-Warning "Bao call failed: $Method $Path — $_"
        throw
    }
}

# ── 1. Enable secrets engines ────────────────────────────────────────
Write-Host "Enabling secrets engines..." -ForegroundColor Cyan
Invoke-Bao -Method Post -Path "sys/mounts/kv" -Body @{type="kv"; options=@{version="2"}} | Out-Null
Invoke-Bao -Method Post -Path "sys/mounts/transit" -Body @{type="transit"} | Out-Null
Invoke-Bao -Method Post -Path "sys/mounts/pki" -Body @{type="pki"} | Out-Null

# ── 2. Transit signing keys ─────────────────────────────────────────
Write-Host "Creating transit signing keys..." -ForegroundColor Cyan
Invoke-Bao -Method Post -Path "transit/keys/lamprey-claim-signer" -Body @{type="ed25519"} | Out-Null
Invoke-Bao -Method Post -Path "transit/keys/lamprey-bundle-signer" -Body @{type="ed25519"} | Out-Null
Invoke-Bao -Method Post -Path "transit/keys/lamprey-revocation-signer" -Body @{type="ed25519"} | Out-Null

# ── 3. PKI root CA and appliance role ────────────────────────────────
Write-Host "Configuring PKI..." -ForegroundColor Cyan
Invoke-Bao -Method Post -Path "pki/root/generate/internal" -Body @{common_name="island-mountain-root"; ttl="87600h"} | Out-Null
Invoke-Bao -Method Post -Path "pki/roles/mai-appliance" -Body @{allow_localhost=$true; allow_any_name=$true; ttl="24h"} | Out-Null

# ── 4. AppRole auth ──────────────────────────────────────────────────
Write-Host "Configuring AppRole auth..." -ForegroundColor Cyan
Invoke-Bao -Method Post -Path "sys/auth/approle" -Body @{type="approle"} | Out-Null
Invoke-Bao -Method Post -Path "auth/approle/role/mai-appliance" -Body @{token_policies="default,mai-appliance"; token_ttl="15m"; token_max_ttl="1h"} | Out-Null

# ── 5. ACL policy ─────────────────────────────────────────────────────
Write-Host "Creating ACL policy..." -ForegroundColor Cyan
$policy = @'
path "kv/data/tenants/*" {
  capabilities = ["read"]
}
path "kv/metadata/tenants/*" {
  capabilities = ["read"]
}
path "kv/data/revocations/*" {
  capabilities = ["read"]
}
path "kv/metadata/revocations/*" {
  capabilities = ["read"]
}
path "kv/metadata/tenants/*" {
  capabilities = ["list","read"]
}
path "transit/sign/lamprey-claim-signer" {
  capabilities = ["update"]
}
path "transit/sign/lamprey-bundle-signer" {
  capabilities = ["update"]
}
path "transit/sign/lamprey-revocation-signer" {
  capabilities = ["update"]
}
path "pki/issue/mai-appliance" {
  capabilities = ["update"]
}
'@
Invoke-Bao -Method Put -Path "sys/policies/acl/mai-appliance" -Body @{policy=$policy} | Out-Null

# ── 6. Demo tenant ───────────────────────────────────────────────────
Write-Host "Writing demo tenant..." -ForegroundColor Cyan
$tenantAttrs = @{
    tenant_id = "tribal-health-demo"
    display_name = "Tribal Health Demonstration"
    compliance_scopes = @("hipaa","ocap")
    default_allowed_routes = @("local_only")
    max_data_classification = "restricted"
    governance_metadata = @{ocap="Demo treaty THD-001"; hipaa="Demo BAA THD-BAA-001"}
}
$wrapper = @{data=@{attributes=($tenantAttrs | ConvertTo-Json -Compress)}}
Invoke-Bao -Method Post -Path "kv/data/tenants/tribal-health-demo" -Body $wrapper | Out-Null

# ── 6.5. Revocation path ──────────────────────────────────────────────
Write-Host "Writing initial revocation list..." -ForegroundColor Cyan
$revocationsEntry = @{data=@{snapshots=@()}}
Invoke-Bao -Method Post -Path "kv/data/revocations/tribal-health-demo" -Body $revocationsEntry | Out-Null

# ── 7. Generate appliance secret_id ──────────────────────────────────
Write-Host "Generating appliance secret_id..." -ForegroundColor Cyan
$secretResp = Invoke-Bao -Method Post -Path "auth/approle/role/mai-appliance/secret-id" -Body @{} -NoBody
$secretId = Invoke-RestMethod -Uri "http://localhost:${Port}/v1/auth/approle/role/mai-appliance/secret-id" -Headers $baoToken -Method Post -Body '{}' -ContentType "application/json"
$freshSecret = $secretId.data.secret_id

# ── 8. Output ────────────────────────────────────────────────────────
Write-Host ""
Write-Host "==============================================" -ForegroundColor Green
Write-Host "  OpenBao Trust Core — Ready"                  -ForegroundColor Green
Write-Host "==============================================" -ForegroundColor Green
Write-Host "  Address:        http://localhost:${Port}"     -ForegroundColor White
Write-Host "  Root token:     root (dev mode)"              -ForegroundColor White
Write-Host "  Audit log:      ${AuditDir}\audit.log"        -ForegroundColor White
Write-Host "  Role ID:        8053c291-8f60-381f-e283-5e645e5907f4" -ForegroundColor White
Write-Host "  Secret ID:      ${freshSecret}"               -ForegroundColor Yellow
Write-Host "==============================================" -ForegroundColor Green
Write-Host ""
Write-Host "Set for mai-api:" -ForegroundColor Cyan
Write-Host "`$env:MAI_OPENBAO_SECRET_ID = '${freshSecret}'" -ForegroundColor Yellow
Write-Host ""
Write-Host "Audit log interrogation:" -ForegroundColor Cyan
Write-Host "  docker exec ${containerName} wc -l /var/log/openbao/audit.log"
Write-Host "  docker exec ${containerName} tail -20 /var/log/openbao/audit.log"
