#!/bin/sh
# packaging/scripts/preinstall.sh - runs BEFORE files are installed.
#
# Responsibilities (idempotent):
#   * Verify the host meets the minimum compatibility bar (systemd present).
#   * If a previous mai package is installed, stop the running services so
#     the upgrade can drop new binaries in place without races.
#
# This script must succeed on a fresh host where mai has never been
# installed - missing services are not an error.

set -eu

PKG_NAME="mai"
ACTION="${1:-install}"

log() {
    printf "[%s preinst] %s\n" "${PKG_NAME}" "$*"
}

require_systemd() {
    if ! command -v systemctl >/dev/null 2>&1; then
        log "ERROR: systemctl not found. mai requires a systemd-managed host."
        exit 1
    fi
    if ! systemctl --version >/dev/null 2>&1; then
        log "ERROR: systemctl is present but not functional."
        exit 1
    fi
}

stop_if_running() {
    unit="$1"
    if systemctl list-unit-files "${unit}" >/dev/null 2>&1; then
        if systemctl is-active --quiet "${unit}"; then
            log "stopping ${unit}"
            systemctl stop "${unit}" || true
        fi
    fi
}

main() {
    log "preinstall (action=${ACTION})"
    require_systemd

    case "${ACTION}" in
        upgrade|install|configure)
            for unit in mai-api.service \
                        mai-dashboard.service \
                        mai-adapter-manager.service \
                        mai-healthcheck.timer \
                        mai-healthcheck.service; do
                stop_if_running "${unit}"
            done
            ;;
        *)
            log "unknown action ${ACTION}, skipping service stop"
            ;;
    esac

    log "preinstall complete"
}

main "$@"
