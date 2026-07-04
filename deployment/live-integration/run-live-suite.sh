#!/usr/bin/env bash
# D5 — the WSF/AOG live-integration suite.
#
# Brings up a live OpenBao (dev) + Moto (AWS STS mock), then runs EVERY
# trust-adjacent live test against them — the no-mock-only closure (§0.3.5). This
# is the same set the CI `wsf-live` job runs, so local and CI exercise one suite.
#
# Env:
#   SKIP_DOCKER=1   use an already-running openbao+moto pair (don't (re)start)
#   WSF_OPENBAO_ADDR / WSF_AWS_ENDPOINT   override the endpoints
set -uo pipefail

OB_ADDR="${WSF_OPENBAO_ADDR:-http://127.0.0.1:8200}"
AWS_EP="${WSF_AWS_ENDPOINT:-http://127.0.0.1:5566}"

wait_ready() { # url grep-token name
  for _ in $(seq 1 30); do
    if curl -sf --max-time 3 "$1" 2>/dev/null | grep -q "$2"; then
      echo "$3 ready"; return 0
    fi
    sleep 1
  done
  echo "$3 did not become ready" >&2; return 1
}

if [ "${SKIP_DOCKER:-0}" != "1" ]; then
  docker rm -f openbao moto >/dev/null 2>&1 || true
  docker run -d --name openbao --cap-add=IPC_LOCK -p 8200:8200 \
    -e BAO_DEV_ROOT_TOKEN_ID=root -e BAO_DEV_LISTEN_ADDRESS=0.0.0.0:8200 \
    openbao/openbao:latest server -dev -dev-root-token-id=root >/dev/null
  docker run -d --name moto -p 5566:5000 motoserver/moto:latest >/dev/null
  wait_ready "$OB_ADDR/v1/sys/seal-status" '"sealed":false' OpenBao || { docker logs openbao; exit 1; }
  wait_ready "$AWS_EP/" '.' Moto || { docker logs moto; exit 1; }
fi

export WSF_OPENBAO_ADDR="$OB_ADDR" WSF_OPENBAO_TOKEN=root WSF_AWS_ENDPOINT="$AWS_EP"

# crate :: test-binary  (every trust-adjacent live path)
TESTS=(
  "wsf-bridge live_openbao"       # token issue/verify/sign against live OpenBao (W1)
  "wsf-broker live_localstack"    # AWS STS cred broker via Moto (W2)
  "wsf-broker live_gcp"           # GCP cred broker (W7)
  "wsf-broker live_azure"         # Azure cred broker (W8)
  "wsf-seal live_seal"            # envelope seal/unseal over live Transit (W3)
  "wsf-ledger live_ledger"        # multi-service receipts + signed pack (W4)
  "wsf-cache live_cache"          # Ring-3 offline decisions from live token+revocation (W5)
  "wsf-tenants live_tenants"      # provision -> issue -> deprovision -> revoked-offline (W9)
  "wsf-api live_api"              # unified REST + SDK round-trip (W6)
  "aog-gateway live_gateway"      # virtual-key -> trust-token auth + budget preflight (G1)
  "aog-gateway kill_switch"       # budget exhaustion + revocation kill-switch (G9)
  "aog-gateway openai_surface"    # OpenAI surface + classify/route (G3/G5)
  "aog-gateway anthropic_surface" # Anthropic surface (G4)
  "aog-gateway policy_modes"      # deny-wins policy + shadow/report/enforce (G6)
  "aog-gateway metering"          # metering + verifiable receipt chain (G7)
)

fail=0
declare -a results
for entry in "${TESTS[@]}"; do
  # shellcheck disable=SC2086
  set -- $entry
  echo "==> $1 :: $2"
  if cargo test -p "$1" --test "$2" -- --nocapture; then
    results+=("PASS $1/$2")
  else
    results+=("FAIL $1/$2"); fail=1
  fi
done

echo ""
echo "===== live-integration summary ====="
printf '%s\n' "${results[@]}"
if [ "$fail" -eq 0 ]; then
  echo "LIVE SUITE GREEN — every trust-adjacent path verified against live services."
else
  echo "LIVE SUITE HAD FAILURES (above)."; exit 1
fi
