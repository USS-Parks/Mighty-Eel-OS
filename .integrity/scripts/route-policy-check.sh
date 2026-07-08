#!/usr/bin/env bash
# route-policy-check.sh — security route-inventory gate.
#
# Asserts every production HTTP route defined in source is declared in
# .integrity/route-policy.tsv. A new privileged endpoint cannot ship without a
# policy row: CI and pre-push fail when a code route has no declared row.
#
# Scope: axum `.route(...)` / `.route_service(...)` path literals in the network
# service crates, multi-line aware, tests/benches/mocks excluded. gRPC, SSE,
# WebSocket, and CLI surfaces are inventoried in
# docs/scans/SECURITY-ROUTE-INVENTORY.md; extending the automated gate to them
# is tracked there.
set -uo pipefail

ROOT="$(git rev-parse --show-toplevel)"
POLICY="$ROOT/.integrity/route-policy.tsv"
SCAN_DIRS="crates/wsf-api crates/aog-gateway crates/aog-approvals crates/aog-toolproxy mai-api"

if ! command -v perl >/dev/null 2>&1; then
  echo "route-policy: perl is required" >&2
  exit 2
fi
if [ ! -f "$POLICY" ]; then
  echo "route-policy: missing policy file $POLICY" >&2
  exit 2
fi

extract_code_routes() {
  cd "$ROOT" || exit 2
  # multi-line aware (perl slurp mode), tests/benches/mocks excluded
  # shellcheck disable=SC2086
  find $SCAN_DIRS -name '*.rs' -not -path '*/tests/*' -not -path '*/benches/*' \
    -not -name '*mock*' 2>/dev/null \
    | while IFS= read -r f; do
        perl -0777 -ne 'while(/\.route(?:_service)?\(\s*"(\/[^"]*)"/g){print "$1\n"}' "$f"
      done | sort -u
}

declared_routes() {
  # column 1 = path; skip the header row and any comment lines
  awk -F'\t' '$1 !~ /^#/ && $1 != "" && $1 != "path" {print $1}' "$POLICY" | sort -u
}

tmpd="$(mktemp -d)"
trap 'rm -rf "$tmpd"' EXIT
extract_code_routes > "$tmpd/code"
declared_routes    > "$tmpd/declared"

undeclared="$(comm -23 "$tmpd/code" "$tmpd/declared")"
stale="$(comm -13 "$tmpd/code" "$tmpd/declared")"

rc=0
if [ -n "$undeclared" ]; then
  echo "route-policy: FAIL — code routes with no row in route-policy.tsv:" >&2
  # shellcheck disable=SC2086
  printf '  + %s\n' $undeclared >&2
  rc=1
fi
if [ -n "$stale" ]; then
  echo "route-policy: WARN — declared routes not present in source (stale?):" >&2
  # shellcheck disable=SC2086
  printf '  - %s\n' $stale >&2
fi
if [ "$rc" -eq 0 ]; then
  n="$(declared_routes | wc -l | tr -d ' ')"
  echo "route-policy: OK — $n routes declared, every source route has a policy row."
fi
exit $rc
