#!/usr/bin/env bash
# MAI one-command local launch.
#
# Usage:
#   scripts/launch.sh                       # default config (config/*.toml)
#   scripts/launch.sh --tier scout          # use configs/scout.toml as overlay
#   scripts/launch.sh --release             # build in release mode
#   MAI_LOG_LEVEL=debug scripts/launch.sh   # override log level
#
# The launcher prefers an existing release binary if present; otherwise it
# falls back to `cargo run -p mai-api`. First boot prints a one-time admin
# API key to stdout — save it before the server settles into normal logging.

set -euo pipefail

# Resolve repo root from the script directory.
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
cd "${REPO_ROOT}"

TIER=""
RELEASE=0
EXTRA_ARGS=()

while [[ $# -gt 0 ]]; do
    case "$1" in
        --tier)
            TIER="$2"
            shift 2
            ;;
        --release)
            RELEASE=1
            shift
            ;;
        --)
            shift
            EXTRA_ARGS+=("$@")
            break
            ;;
        *)
            EXTRA_ARGS+=("$1")
            shift
            ;;
    esac
done

if [[ -n "${TIER}" ]]; then
    TIER_FILE="configs/${TIER}.toml"
    if [[ ! -f "${TIER_FILE}" ]]; then
        echo "error: tier config ${TIER_FILE} not found" >&2
        exit 2
    fi
    export MAI_TIER_CONFIG="${REPO_ROOT}/${TIER_FILE}"
    echo "launch: using tier config ${MAI_TIER_CONFIG}"
fi

export MAI_LOG_LEVEL="${MAI_LOG_LEVEL:-info}"

# BRAND-01 renamed the cargo bin to lamprey-mai-api.
BINARY="target/release/lamprey-mai-api"
if [[ "${RELEASE}" -eq 1 ]]; then
    echo "launch: building release binary"
    cargo build --release -p mai-api
elif [[ ! -x "${BINARY}" ]]; then
    BINARY=""
fi

if [[ -n "${BINARY}" && -x "${BINARY}" ]]; then
    echo "launch: running ${BINARY}"
    exec "${BINARY}" "${EXTRA_ARGS[@]}"
else
    echo "launch: running via cargo (debug)"
    exec cargo run -p mai-api -- "${EXTRA_ARGS[@]}"
fi
