#!/bin/sh
# packaging/scripts/postremove.sh - runs AFTER files are removed.
#
# Default action: reload systemd, leave customer data alone.
# On `--purge` (Debian) or `purge` action: remove state directories and
# the `mai` system user. We refuse to wipe state unless explicitly told.

set -eu

PKG_NAME="mai"
ACTION="${1:-remove}"
MAI_USER="mai"
MAI_GROUP="mai"

log() {
    printf "[%s postrm] %s\n" "${PKG_NAME}" "$*"
}

reload_systemd() {
    if command -v systemctl >/dev/null 2>&1; then
        log "reloading systemd"
        systemctl daemon-reload || true
    fi
}

purge_state() {
    log "PURGE: removing customer data from /var/lib/mai, /var/log/mai,"
    log "       /var/backups/mai, /etc/mai, /run/mai"
    rm -rf /var/lib/mai /var/log/mai /run/mai /var/backups/mai \
           /etc/mai/policies /etc/mai/trust-anchors \
           /usr/lib/mai/compliance-dashboard/.venv || true

    if getent passwd "${MAI_USER}" >/dev/null 2>&1; then
        log "removing system user ${MAI_USER}"
        deluser --system "${MAI_USER}" || true
    fi
    if getent group "${MAI_GROUP}" >/dev/null 2>&1; then
        log "removing system group ${MAI_GROUP}"
        delgroup --system "${MAI_GROUP}" || true
    fi
}

main() {
    log "postremove (action=${ACTION})"
    reload_systemd

    case "${ACTION}" in
        purge)
            purge_state
            ;;
        remove|upgrade|failed-upgrade|abort-install|abort-upgrade|disappear)
            log "customer data preserved (action=${ACTION})."
            log "Run 'apt purge mai' (Debian) to wipe /var/lib/mai etc."
            ;;
        *)
            log "action ${ACTION} not handled, skipping"
            ;;
    esac
    log "postremove complete"
}

main "$@"
