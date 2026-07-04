#!/usr/bin/env bash
# D3 — generate an SBOM per appliance image (syft), in SPDX + CycloneDX JSON.
#
# Usage: sbom.sh [image ...]     (default: im-appliance:latest im-console:latest)
# Env:   SBOM_OUT   output dir (default: deployment/supply-chain/sbom)
set -euo pipefail

IMAGES=("$@")
if [ ${#IMAGES[@]} -eq 0 ]; then
  IMAGES=(im-appliance:latest im-console:latest)
fi
OUT="${SBOM_OUT:-deployment/supply-chain/sbom}"

if ! command -v syft >/dev/null 2>&1; then
  echo "ERROR: syft not installed — https://github.com/anchore/syft" >&2
  exit 3
fi

mkdir -p "$OUT"
for img in "${IMAGES[@]}"; do
  name="$(printf '%s' "$img" | tr '/:' '__')"
  echo "==> SBOM for $img"
  syft "$img" \
    -o "spdx-json=$OUT/${name}.spdx.json" \
    -o "cyclonedx-json=$OUT/${name}.cdx.json"
  echo "    wrote $OUT/${name}.spdx.json + .cdx.json"
done
echo "SBOMs complete in $OUT"
