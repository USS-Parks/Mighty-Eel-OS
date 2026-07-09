#!/usr/bin/env bash
# Regression gate for no-slop-scan.sh (Q2): a planted roster step-code in normal
# source is flagged; the docs / ship-pipeline exemptions and the slop-ok escape
# hatch pass. Builds a throwaway git repo so the check is hermetic (no dependence
# on the surrounding tree or the user's global git config / hooks).
set -uo pipefail
SCANNER="$(cd "$(dirname "$0")" && pwd)/no-slop-scan.sh"
TMP="$(mktemp -d)"; trap 'rm -rf "$TMP"' EXIT
export GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_SYSTEM=/dev/null
cd "$TMP"
git init -q
git config core.hooksPath /dev/null
git config user.email t@example.com
git config user.name test

pass=0; fail=0
expect() { # <got-exit> <want-exit> <label>
  if [ "$1" = "$2" ]; then pass=$((pass + 1)); else fail=$((fail + 1)); echo "FAIL: $3 (want exit $2, got $1)"; fi
}
run() { git add -A >/dev/null 2>&1; git commit -qm snap >/dev/null 2>&1; bash "$SCANNER" full >/dev/null 2>&1; echo $?; }

# 1. A planted "K3" + "SHIP-09" in normal source is flagged (exit 1).
mkdir -p src; printf 'fn f() {} // K3 keygen; SHIP-09 wires it\n' > src/lib.rs
expect "$(run)" 1 "planted K3/SHIP-09 in src/lib.rs is flagged"

# 2. The same codes in a ship-pipeline path (config/) pass — legit step data.
rm -f src/lib.rs; mkdir -p config; printf 'carried_forward = "SHIP-09" # K3\n' > config/x.toml
expect "$(run)" 0 "SHIP-09/K3 in config/ is exempt"

# 3. The same codes in an exempt docs/ path pass.
rm -f config/x.toml; mkdir -p docs; printf 'fn f() {} // K3 and SHIP-09 history\n' > docs/note.rs
expect "$(run)" 0 "K3/SHIP-09 in docs/ is exempt"

# 4. A slop-ok annotation on an otherwise-flagged line passes.
rm -f docs/note.rs; mkdir -p src; printf 'fn f() {} // SHIP-09 // slop-ok: intentional\n' > src/lib.rs
expect "$(run)" 0 "slop-ok annotation is honored"

echo "no-slop-scan.test: ${pass} passed, ${fail} failed"
[ "$fail" -eq 0 ]
