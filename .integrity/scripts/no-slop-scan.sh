#!/usr/bin/env bash
# no-slop-scan.sh — CANON §11 enforcement: no build-process artifacts in committed source.
#
# Committed source is the product, not the transcript of how it was built. This scan
# blocks the residue an AI "session" workflow leaks into code:
#   PROV  build-phase markers  — "Session <N>", "BF-<N>", "S<N> hookup", "plan-spec scaffold"
#   DEBT  untracked debt        — bare TODO/FIXME/XXX/HACK (must be TODO(owner): …)
#   UNFIN unfinished shipped code — todo!()/unimplemented!() outside tests
#   STUB  confessions in comments — leading "// Stub: …" / "# Placeholder: …", "for now,"
#   DOC   dangling references    — docs/<name>.md cited but absent from the tree
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
#   **/hooks/  and this scanner itself.
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
        ':(exclude)*no-slop-scan.sh' ':(exclude)*CANON*' ':(exclude,glob)**/docs/**'
        ':(exclude,glob)**/PLANNING/**' ':(exclude,glob)**/sessions/**' ':(exclude)*DEVLOG*'
        ':(exclude)*ROSTER*' ':(exclude)*CHANGELOG*' ':(exclude)*CLAUDE.md' ':(exclude,glob)**/hooks/**'
        ':(exclude)*gitdoctor*' )   # sibling slop-scanner: contains the vocabulary as data, like this file
# todo!()/unimplemented!() are tolerated only in test code.
NOTEST=( ':(exclude,glob)**/tests/**' ':(exclude)*_test.rs' ':(exclude,glob)**/test_*.py' )

# S(05-49): bare roster shorthand ("per S41 acceptance criteria"). S1-S4 excluded
# (AWS S3 etc.); annotate any legitimate S## with slop-ok.
PROV='(\bSessions?[ -][0-9])|(\bBF-[0-9])|(\bS[0-9]+ hookup)|(plan-spec scaffold)|(\bS(0[5-9]|[1-4][0-9])[a-e]?\b)'
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
  git grep -hoIE 'docs/[A-Za-z0-9._/-]+\.md' -- "${SPECS[@]}" 2>/dev/null | sort -u | while IFS= read -r ref; do
    [ -z "$ref" ] && continue
    b="$(basename "$ref")"
    printf '%s\n' "$tracked" | grep -q "/${b}$\|^${b}$" \
      || echo "DOC    dangling reference: $ref  (no tracked file named $b)" >>"$VIOL"
  done
}

gated_staged() {
  case "$1" in
    *no-slop-scan.sh|*gitdoctor*|*/hooks/*|*CANON*|*/docs/*|*/PLANNING/*|*/sessions/*|*DEVLOG*|*ROSTER*|*CHANGELOG*|*CLAUDE.md) return 1 ;;
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
