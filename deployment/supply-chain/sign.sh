#!/usr/bin/env bash
# D3 — cosign-sign + verify each appliance image.
#
# In CI (CI=true and no COSIGN_KEY): keyless OIDC signing — no key material on disk.
# Locally: key-based, COSIGN_KEY=<private key> (owner-gated — the key is Tier-0).
#
# Usage: sign.sh <image> [image ...]
set -euo pipefail

if [ $# -eq 0 ]; then
  echo "usage: sign.sh <image> [image ...]" >&2
  exit 2
fi

if ! command -v cosign >/dev/null 2>&1; then
  echo "ERROR: cosign not installed — https://github.com/sigstore/cosign" >&2
  exit 3
fi

for img in "$@"; do
  if [ -z "${COSIGN_KEY:-}" ] && [ -n "${CI:-}" ]; then
    echo "==> keyless (OIDC) sign $img"
    COSIGN_EXPERIMENTAL=1 cosign sign --yes "$img"
    COSIGN_EXPERIMENTAL=1 cosign verify "$img" \
      --certificate-identity-regexp '.+' \
      --certificate-oidc-issuer-regexp '.+' >/dev/null
  else
    : "${COSIGN_KEY:?set COSIGN_KEY to the signing key (owner-gated) or run in CI for keyless}"
    pub="${COSIGN_PUB:-${COSIGN_KEY%.key}.pub}"
    echo "==> key-based sign $img (key: $COSIGN_KEY)"
    cosign sign --yes --key "$COSIGN_KEY" "$img"
    cosign verify --key "$pub" "$img" >/dev/null
  fi
  echo "    signed + verified: $img"
done
echo "All images signed + verified."
