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

# Keyless verification binds the certificate to OUR signing workflow —
# never "any identity from any issuer". Inside GitHub Actions the exact
# identity is derived from the run itself (GITHUB_WORKFLOW_REF, e.g.
# `USS-Parks/im-mighty-eel-mai/.github/workflows/supply-chain.yml@refs/tags/v1.2.3`);
# outside it, a pinned regexp for the canonical release workflow applies.
# A signature minted by any other workflow, repo, or issuer fails verify.
OIDC_ISSUER="${COSIGN_CERT_OIDC_ISSUER:-https://token.actions.githubusercontent.com}"
IDENTITY_REGEXP="${COSIGN_CERT_IDENTITY_REGEXP:-^https://github\.com/USS-Parks/im-mighty-eel-mai/\.github/workflows/supply-chain\.yml@refs/(tags/v[0-9][A-Za-z0-9.+-]*|heads/main)$}"

verify_keyless() {
  img="$1"
  if [ -n "${GITHUB_WORKFLOW_REF:-}" ]; then
    COSIGN_EXPERIMENTAL=1 cosign verify "$img" \
      --certificate-identity "https://github.com/${GITHUB_WORKFLOW_REF}" \
      --certificate-oidc-issuer "$OIDC_ISSUER" >/dev/null
  else
    COSIGN_EXPERIMENTAL=1 cosign verify "$img" \
      --certificate-identity-regexp "$IDENTITY_REGEXP" \
      --certificate-oidc-issuer "$OIDC_ISSUER" >/dev/null
  fi
}

for img in "$@"; do
  if [ -z "${COSIGN_KEY:-}" ] && [ -n "${CI:-}" ]; then
    echo "==> keyless (OIDC) sign $img"
    COSIGN_EXPERIMENTAL=1 cosign sign --yes "$img"
    verify_keyless "$img"
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
