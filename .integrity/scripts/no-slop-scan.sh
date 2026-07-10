#!/usr/bin/env bash
# no-slop-scan.sh — CANON §11 enforcement: no build-process artifacts in committed source.
#
# Committed source is the product, not the transcript of how it was built. This scan
# blocks the residue an AI "session" workflow leaks into code:
#   PROV  roster step-codes    — "Session <N>", "BF-<N>", "SHIP-##", "VH#", "AF-##", "S<N> hookup",
#                                bare "K3" + phase-letters in context ("(H6)", "audit H7", "R6 live gate")
#   DEBT  untracked debt        — bare TODO/FIXME/XXX/HACK (must be TODO(owner): …)
#   UNFIN unfinished shipped code — todo!()/unimplemented!() outside tests
#   STUB  confessions in comments — leading "// Stub: …" / "# Placeholder: …", "for now,"
#   DOC   dangling references    — a cited .md absent from the tree: docs/<name>.md
#                                anywhere, or a bare <name>.md on a comment line
#                                (a bare name in executing code is data — fixture
#                                paths, generated names — not a citation)
#
# Usage:
#   no-slop-scan.sh staged   # default — ADDED lines of staged files   (pre-commit; fast)
#   no-slop-scan.sh full     # whole tracked tree                      (pre-push / audit)
#
# Escape hatch: append  slop-ok: <reason>  on the offending line for a vetted, visible
# exception. Exceptions stay grep-auditable.
#
# Exempt by design (legitimately carry the build taxonomy or describe the rule):
#   docs/  PLANNING/  **/sessions/  *DEVLOG*  *ROSTER*  *CHANGELOG*  CANON*  *CLAUDE.md
#   **/hooks/  this scanner + its self-test, and the ship pipeline + repo tooling that carries
#   the SHIP-## step vocabulary as data:  config/  deployment/  packaging/  scripts/
#   tools/*_tests/  tests/  .github/  pyproject.toml  deny.toml  .gitleaks.toml
set -uo pipefail

MODE="${1:-staged}"
ROOT="$(git rev-parse --show-toplevel 2>/dev/null || echo .)"
cd "$ROOT" || exit 2
RED=$'\033[0;31m'; YEL=$'\033[1;33m'; GRN=$'\033[0;32m'; NC=$'\033[0m'
VIOL="$(mktemp)"; trap 'rm -f "$VIOL"' EXIT

# Gated source/config types as git pathspecs (one grep process, not one per file —
# critical on Windows where process spawn is expensive). '*' matches across '/'.
SPECS=( '*.rs' '*.py' '*.js' '*.mjs' '*.ts' '*.tsx' '*.go' '*.toml' '*.proto'
        '*.sh' '*.bash' '*.ps1' '*.c' '*.cc' '*.cpp' '*.h' '*.hpp' '*.java' '*.rb' '*.yaml' '*.yml'
        ':(exclude)*no-slop-scan*.sh' ':(exclude)*CANON*' ':(exclude,glob)**/docs/**'
        ':(exclude,glob)**/PLANNING/**' ':(exclude,glob)**/sessions/**' ':(exclude)*DEVLOG*'
        ':(exclude)*ROSTER*' ':(exclude)*CHANGELOG*' ':(exclude)*CLAUDE.md' ':(exclude,glob)**/hooks/**'
        ':(exclude)*gitdoctor*'     # sibling slop-scanner: contains the vocabulary as data, like this file
        # Ship pipeline + repo tooling carry the SHIP-##/step taxonomy as DATA
        # (ship_session fields, carried_forward config, test subjects), not stray
        # comment provenance — exempt like docs/DEVLOG (Q2).
        ':(exclude,glob)config/**' ':(exclude,glob)deployment/**' ':(exclude,glob)packaging/**'
        ':(exclude,glob)scripts/**' ':(exclude,glob)tools/*_tests/**' ':(exclude,glob)tests/**'
        ':(exclude,glob).github/**' ':(exclude)*pyproject.toml' ':(exclude)*deny.toml'
        ':(exclude)*.gitleaks.toml' )
# todo!()/unimplemented!() are tolerated only in test code.
NOTEST=( ':(exclude,glob)**/tests/**' ':(exclude)*_test.rs' ':(exclude,glob)**/test_*.py' )

# Roster/finding step-codes (Q1/Q2): distinctive prefixes SHIP-##, VH#, AF-##, BF-#,
# Session <n>; the S(05-49) bare shorthand ("per S41"); bare K# (keygen phase, e.g. a
# planted "K3"); and H/N/R/U phase-letters only in a provenance CONTEXT — "(H6)",
# "(H8/U2)", "audit H7", "K1 gate", "N1 FIX", "R6 live gate". Domain vocab is left
# alone: H100/K8s/K80/S3 (word-boundary), U1 country codes, NVLink NV#. S1-S4 excluded
# (AWS S3); annotate any legitimate hit with slop-ok. The ship pipeline's own SHIP-##
# data lives in the exempt tooling paths above.
PROV='(\bSessions?[ -][0-9])|(\bBF-[0-9])|(\bS[0-9]+ hookup)|(plan-spec scaffold)|(\bS(0[5-9]|[1-4][0-9])[a-e]?\b)|(\bSHIP-[0-9])|(\bVH[0-9])|(\bAF-[0-9])|(\bK[0-9]\b)|(\((audit )?[HNRU][0-9]+(/[A-Z][0-9]+)*\))|(\b(audit|finding) [HKNRU][0-9])|(\b[HKNRU][0-9]+ (gate|FIX|hookup|convergence|live gate))'
DEBT='\b(TODO|FIXME|XXX|HACK)\b([^(]|$)'
UNFIN='\b(todo!|unimplemented!)\('
# Confession-shape only: a comment LEADING with "Stub:"/"Placeholder —", or "for
# now," anywhere. Mid-sentence domain usage (PHI placeholder tokens, the designed
# stub-vault mode, test doubles) is legitimate vocabulary and not flagged.
STUB='^[[:space:]]*(//+!?|#+|\*|--)[[:space:]]*(stub|placeholder)[[:space:]]*[:—-]|\bfor now,'

scan_full() {
  git grep -nE  "$PROV"  -- "${SPECS[@]}"              2>/dev/null | grep -v 'slop-ok:' | sed 's/^/PROV   /' >>"$VIOL" || true
  git grep -nE  "$DEBT"  -- "${SPECS[@]}"              2>/dev/null | grep -v 'slop-ok:' | sed 's/^/DEBT   /' >>"$VIOL" || true
  git grep -nE  "$UNFIN" -- "${SPECS[@]}" "${NOTEST[@]}" 2>/dev/null | grep -v 'slop-ok:' | sed 's/^/UNFIN  /' >>"$VIOL" || true
  git grep -niE "$STUB"  -- "${SPECS[@]}" "${NOTEST[@]}" 2>/dev/null | grep -v 'slop-ok:' | sed 's/^/STUB   /' >>"$VIOL" || true
  local tracked; tracked="$(git ls-files)"
  # docs/-prefixed citations count anywhere; BARE <name>.md citations count
  # only on comment lines outside test code — in code a bare name is data
  # (fixture paths, generated names), in a test comment it names a fixture,
  # but in a source comment it is a citation and must resolve.
  {
    git grep -hoIE 'docs/[A-Za-z0-9._/-]+\.md' -- "${SPECS[@]}" 2>/dev/null
    git grep -hIE '^[[:space:]]*(//+!?|#+|\*|--|<!--)' -- "${SPECS[@]}" "${NOTEST[@]}" 2>/dev/null \
      | grep -v 'slop-ok:' \
      | grep -oE '\b[A-Za-z0-9._-]+(/[A-Za-z0-9._-]+)*\.md\b'
  } | sort -u | while IFS= read -r ref; do
    [ -z "$ref" ] && continue
    b="$(basename "$ref")"
    printf '%s\n' "$tracked" | grep -q "/${b}$\|^${b}$" \
      || echo "DOC    dangling reference: $ref  (no tracked file named $b)" >>"$VIOL"
  done
}

gated_staged() {
  case "$1" in
    *no-slop-scan*.sh|*gitdoctor*|*/hooks/*|*CANON*|*/docs/*|*/PLANNING/*|*/sessions/*|*DEVLOG*|*ROSTER*|*CHANGELOG*|*CLAUDE.md) return 1 ;;
    config/*|deployment/*|packaging/*|scripts/*|tests/*|tools/*_tests/*|.github/*|*pyproject.toml|*deny.toml|*.gitleaks.toml) return 1 ;;  # ship-pipeline: SHIP-## is legit data (Q2)
  esac
  case "$1" in
    *.rs|*.py|*.js|*.mjs|*.ts|*.tsx|*.go|*.toml|*.proto|*.sh|*.bash|*.ps1|*.c|*.cc|*.cpp|*.h|*.hpp|*.java|*.rb|*.yaml|*.yml) return 0 ;;
    *) return 1 ;;
  esac
}

scan_staged() {
  local f added
  while IFS= read -r f; do
    gated_staged "$f" || continue
    added="$(git diff --cached -U0 -- "$f" 2>/dev/null | grep -E '^\+[^+]' | sed 's/^+//')" || true
    [ -z "$added" ] && continue
    printf '%s\n' "$added" | grep -E  "$PROV"  | grep -v 'slop-ok:' | sed "s|^|PROV   $f (+): |" >>"$VIOL" || true
    printf '%s\n' "$added" | grep -E  "$DEBT"  | grep -v 'slop-ok:' | sed "s|^|DEBT   $f (+): |" >>"$VIOL" || true
    printf '%s\n' "$added" | grep -E  "$UNFIN" | grep -v 'slop-ok:' | sed "s|^|UNFIN  $f (+): |" >>"$VIOL" || true
    case "$f" in */tests/*|*_test.rs|*/test_*.py) : ;;   # test doubles are legitimately named "stub"
      *) printf '%s\n' "$added" | grep -iE "$STUB" | grep -v 'slop-ok:' | sed "s|^|STUB   $f (+): |" >>"$VIOL" || true ;;
    esac
  done < <(git diff --cached --name-only --diff-filter=ACM)
}

[ "$MODE" = "full" ] && scan_full || scan_staged

N="$(grep -c . "$VIOL" 2>/dev/null)"; N="${N:-0}"
if [ "$N" -gt 0 ]; then
  echo "${RED}CANON §11 — build-process artifacts detected: ${N}${NC}" >&2
  head -60 "$VIOL" | sed 's/^/  /' >&2
  [ "$N" -gt 60 ] && echo "  … and $((N-60)) more" >&2
  echo "${YEL}Scrub them, or annotate a vetted exception inline with  slop-ok: <reason>.${NC}" >&2
  exit 1
fi
echo "${GRN}no-slop: clean (${MODE}).${NC}"
exit 0
