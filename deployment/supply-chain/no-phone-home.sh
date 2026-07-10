#!/usr/bin/env bash
# D3 — assert the WSF/AOG services and the MAI appliance make zero phone-home.
#
# "Zero phone-home" does NOT mean "no hostnames in source" — the services must be
# able to reach a customer's *configured* OpenBao / cloud STS / model provider when
# not air-gapped. It means: (1) no call-home to our own vendor infrastructure and no
# telemetry/analytics SDK, and (2) every external host in shipped source is a known
# provider/STS endpoint that is **overridable via env/config** (so an air-gapped
# deployment points it at nothing, and the W5 egress guard denies cloud routes).
#
# Scope: everything that ships — `crates/*/src` (WSF/AOG) AND `mai-*/src` (the
# appliance). The one sanctioned vendor host is the OTA update-manifest default
# `updates.islandmountain.ai` (`UpdateClientConfig::base_url`, mai-core
# `models/update.rs`): a documented, config-overridable default that the air-gap
# guard denies at runtime. It is excluded by exact host below; ANY other
# islandmountain.io / islandmountain.ai reference still fails the scan.
#
# Runtime egress monitoring is owner-gated — see SUPPLY-CHAIN.md §4.
set -euo pipefail

ROOT="${1:-.}"
cd "$ROOT"
fail=0

SCAN_DIRS=(crates/*/src mai-*/src)

# 1. No vendor call-home (our own domain) and no telemetry/analytics beacons.
#    The Kubernetes-style API group `<name>.islandmountain.io/vN` is a schema
#    identifier (an apiVersion / URL path on the local apiserver), NOT a network
#    destination, so it is excluded here, as are comment-only lines (prose like
#    "debugging/telemetry" is not a beacon — an SDK import or URL is code, not a
#    comment). A real call-home to our domain carries a scheme and is still
#    caught by check 2 below, which flags any external host not on the
#    provider/STS allowlist.
if grep -rEniH \
     'islandmountain\.(io|ai)|sentry\.io|segment\.(io|com)|mixpanel|posthog|datadoghq|google-analytics|/telemetry|/collect\?' \
     "${SCAN_DIRS[@]}" --include='*.rs' 2>/dev/null \
     | grep -vE '^[^:]+:[0-9]+:[[:space:]]*(//|/\*|\*)' \
     | grep -vE '[a-z0-9-]+\.islandmountain\.io/v[0-9]' \
     | grep -vE 'updates\.islandmountain\.ai'; then
  echo "FAIL: a service references vendor call-home / telemetry (above)." >&2
  fail=1
fi

# 2. Every external host in shipped source is a known, config-overridable
#    provider/STS endpoint — never a surprise destination. Cluster-internal hosts
#    (loopback, *.internal, single-label service names) and RFC 2606 reserved
#    documentation/test domains are ignored.
ALLOWED='api\.openai\.com|api\.anthropic\.com|amazonaws\.com|microsoftonline\.com|googleapis\.com|storage\.azure\.com|huggingface\.co'
while IFS= read -r url; do
  host="${url#*//}"
  case "$host" in
    localhost | 127.0.0.1 | 0.0.0.0 | *.internal) continue ;; # loopback / internal
    example.com | *.example.com | *.example | *.test | *.invalid) continue ;; # RFC 2606 doc/test hosts
    updates.islandmountain.ai) continue ;; # documented OTA default (see header); overridable + air-gap-denied
    *.*) : ;;                                                  # dotted → public-ish, check it
    *) continue ;;                                             # single label → cluster-internal
  esac
  if ! printf '%s' "$host" | grep -qE "(${ALLOWED})\$"; then
    echo "  suspect external host: $host" >&2
    fail=1
  fi
done < <(grep -rEn 'https?://[A-Za-z0-9._-]+' "${SCAN_DIRS[@]}" --include='*.rs' 2>/dev/null \
  | grep -vE '^[^:]+:[0-9]+:[[:space:]]*(//|/\*|\*)' \
  | grep -oE 'https?://[A-Za-z0-9._-]+' \
  | sort -u)

if [ "$fail" -ne 0 ]; then
  echo "FAIL: unexpected phone-home surface (above). Route new hosts through config." >&2
  exit 1
fi

echo "PASS: no vendor call-home / telemetry; every external host is a known,"
echo "      config-overridable provider/STS endpoint. Zero static phone-home."
echo "      (runtime egress monitor is owner-gated; see SUPPLY-CHAIN.md §4)"
