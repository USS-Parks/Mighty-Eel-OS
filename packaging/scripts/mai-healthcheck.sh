#!/bin/sh
# packaging/scripts/mai-healthcheck.sh - periodic local probe.
#
# Invoked by mai-healthcheck.service (driven by mai-healthcheck.timer).
# Exits 0 when the API is healthy, non-zero otherwise. The systemd unit
# will surface failures via journald; an alerting rule on
# "FAILED state of mai-healthcheck.service" closes the loop.

set -eu

PKG_NAME="mai"
MAI_API_URL="${MAI_API_URL:-http://127.0.0.1:8420}"
TIMEOUT_SECS="${MAI_HEALTHCHECK_TIMEOUT:-5}"

log() {
    printf "[%s healthcheck] %s\n" "${PKG_NAME}" "$*"
}

probe() {
    path="$1"
    if command -v curl >/dev/null 2>&1; then
        curl --fail --silent --show-error \
             --max-time "${TIMEOUT_SECS}" \
             "${MAI_API_URL}${path}"
    elif command -v wget >/dev/null 2>&1; then
        wget --quiet --timeout="${TIMEOUT_SECS}" \
             -O - "${MAI_API_URL}${path}"
    else
        log "ERROR: neither curl nor wget available; cannot probe."
        return 4
    fi
}

main() {
    log "probing ${MAI_API_URL}/v1/health/ready"
    if ! probe /v1/health/ready >/dev/null; then
        log "FAIL: readiness probe non-2xx"
        exit 1
    fi
    log "probing ${MAI_API_URL}/v1/health/live"
    if ! probe /v1/health/live >/dev/null; then
        log "FAIL: liveness probe non-2xx"
        exit 1
    fi
    log "OK"
}

main "$@"
