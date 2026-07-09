#!/usr/bin/env bash
# verify-tree.sh - Scan working tree for corruption before staging
# Usage: .integrity/scripts/verify-tree.sh [path...]
# If no paths given, checks all modified files in working tree.

set -uo pipefail

RED='\033[0;31m'
YELLOW='\033[1;33m'
GREEN='\033[0;32m'
NC='\033[0m'

ERRORS=0
WARNINGS=0
CHECKED=0

# Determine files to check
if [ $# -gt 0 ]; then
    FILES="$@"
else
    FILES=$(git diff --name-only 2>/dev/null || find . -name "*.rs" -o -name "*.py" -o -name "*.toml" -o -name "*.json" -o -name "*.md" | head -100)
fi

for FILE in $FILES; do
    [ -f "$FILE" ] || continue
    CHECKED=$((CHECKED + 1))

    # Skip binary
    if file "$FILE" | grep -q "binary\|executable\|image"; then
        continue
    fi

    # CHECK 1: Null bytes
    FILE_BYTES=$(wc -c < "$FILE")
    CLEAN_BYTES=$(tr -d '\0' < "$FILE" | wc -c)
    if [ "$FILE_BYTES" -ne "$CLEAN_BYTES" ]; then
        echo -e "${RED}FAIL${NC} [null-bytes] $FILE"
        ERRORS=$((ERRORS + 1))
        continue
    fi

    # CHECK 2: Truncation vs HEAD (if tracked)
    if git ls-files --error-unmatch "$FILE" &>/dev/null; then
        HEAD_LINES=$(git show "HEAD:$FILE" 2>/dev/null | wc -l || echo 0)
        CURRENT_LINES=$(wc -l < "$FILE")

        if [ "$HEAD_LINES" -gt 10 ] && [ "$CURRENT_LINES" -gt 0 ]; then
            RATIO=$((CURRENT_LINES * 100 / HEAD_LINES))
            if [ "$RATIO" -lt 50 ]; then
                echo -e "${RED}FAIL${NC} [truncated] $FILE (${HEAD_LINES} -> ${CURRENT_LINES} lines, ${RATIO}%)"
                ERRORS=$((ERRORS + 1))
            elif [ "$RATIO" -lt 75 ]; then
                echo -e "${YELLOW}WARN${NC} [shrunk] $FILE (${HEAD_LINES} -> ${CURRENT_LINES} lines, ${RATIO}%)"
                WARNINGS=$((WARNINGS + 1))
            fi
        fi
    fi

    # CHECK 3: Bracket/brace balance for code files
    case "$FILE" in
        *.rs|*.py|*.js|*.ts|*.json|*.toml)
            # Count occurrences, not lines-containing (grep -c): code style that
            # clusters opens mid-line and isolates closes on their own lines
            # otherwise reads as corruption on a perfectly balanced file.
            OPEN_BRACES=$(grep -o '{' "$FILE" | wc -l || true)
            CLOSE_BRACES=$(grep -o '}' "$FILE" | wc -l || true)
            if [ "$OPEN_BRACES" -ne "$CLOSE_BRACES" ]; then
                if [ "$OPEN_BRACES" -gt 0 ] || [ "$CLOSE_BRACES" -gt 0 ]; then
                    DIFF=$((OPEN_BRACES - CLOSE_BRACES))
                    ABS=${DIFF#-}
                    if [ "$ABS" -gt 3 ]; then
                        echo -e "${RED}FAIL${NC} [unbalanced-braces] $FILE ({:$OPEN_BRACES }:$CLOSE_BRACES delta:$DIFF)"
                        ERRORS=$((ERRORS + 1))
                    elif [ "$ABS" -gt 0 ]; then
                        echo -e "${YELLOW}WARN${NC} [brace-mismatch] $FILE ({:$OPEN_BRACES }:$CLOSE_BRACES)"
                        WARNINGS=$((WARNINGS + 1))
                    fi
                fi
            fi
            ;;
    esac

    # CHECK 4: File ends with newline (POSIX compliance)
    if [ -s "$FILE" ] && [ "$(tail -c 1 "$FILE" | wc -l)" -eq 0 ]; then
        echo -e "${YELLOW}WARN${NC} [no-trailing-newline] $FILE"
        WARNINGS=$((WARNINGS + 1))
    fi
done

echo ""
echo -e "Checked: ${CHECKED} files | ${GREEN}Passed${NC}: $((CHECKED - ERRORS - WARNINGS)) | ${YELLOW}Warnings${NC}: ${WARNINGS} | ${RED}Errors${NC}: ${ERRORS}"

if [ "$ERRORS" -gt 0 ]; then
    echo -e "${RED}INTEGRITY CHECK FAILED. Do not stage these files.${NC}"
    exit 1
fi

if [ "$WARNINGS" -gt 0 ]; then
    echo -e "${YELLOW}Warnings present. Review before staging.${NC}"
    exit 0
fi

echo -e "${GREEN}All files passed integrity checks.${NC}"
exit 0
