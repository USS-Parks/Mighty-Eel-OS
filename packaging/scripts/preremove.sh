#!/bin/sh
# packaging/scripts/preremove.sh - runs BEFORE files are removed.
#
# Stop running services and disable autostart so removal can proceed
# cleanly. We never touch customer data here - that decision belongs to
# postremove.sh and is gated on --purge.

set -eu

PKG_NAME="mai"
ACTION="${1:-remove}"

log() {
    printf "[%s prerm] %s\n" "${PKG_NAME}" "$*"
}

stop_and_disable() {
    unit="$1"
    if systemctl list-unit-files "${unit}" >/dev/null 2>&1; then
        if systemctl is-active --quiet "${unit}"; then
            log "stopping ${unit}"
            systemctl stop "${unit}" || true
        fi
        if systemctl is-enabled --quiet "${unit}" 2>/dev/null; then
            log "disabling ${unit}"
            systemctl disable "${unit}" || true
        fi
    fi
}

main() {
    log "preremove (action=${ACTION})"
    if ! command -v systemctl >/dev/null 2>&1; then
        log "systemctl missing, skipping service teardown"
        exit 0
    fi
    case "${ACTION}" in
        remove|purge|upgrade|deconfigure)
            for unit in mai-healthcheck.timer \
                        mai-healthcheck.service \
                        mai-dashboard.service \
                        mai-adapter-manager.service \
                        mai-api.service; do
                stop_and_disable "${unit}"
            done
            ;;
        *)
            log "action ${ACTION} not handled, skipping"
            ;;
    esac
    log "preremove complete"
}

main "$@"
