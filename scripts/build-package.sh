#!/usr/bin/env bash
# scripts/build-package.sh - Assemble the MAI install staging tree and
# (optionally) drive dpkg-buildpackage to produce a .deb.
#
# This script is the single source of truth for the production install
# layout. It runs from a clean repo checkout and produces a tree under
# build/package-staging/ that mirrors what ends up on the target host.
# The Debian rules file (packaging/debian/rules) consumes this tree.
#
# Usage:
#   scripts/build-package.sh                      # stage only, no .deb
#   scripts/build-package.sh --deb                # also run dpkg-buildpackage
#   scripts/build-package.sh --validate-only      # skip cargo build, just stage docs/configs
#   scripts/build-package.sh --staging <PATH>     # override staging dir
#   scripts/build-package.sh --skip-dashboard     # don't bundle dashboard wheels
#
# Exit codes:
#   0   staging tree built, validator passed
#   1   build or staging failed
#   2   validator failed (package not ship-ready)
#   3   environment missing required tool

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
cd "${REPO_ROOT}"

STAGING_DIR="${REPO_ROOT}/build/package-staging"
BUILD_DEB=0
VALIDATE_ONLY=0
SKIP_DASHBOARD=0
PKG_VERSION="$(grep -m1 '^version' Cargo.toml | sed -E 's/.*"(.*)"/\1/')"
GIT_COMMIT="$(git rev-parse --short=12 HEAD 2>/dev/null || echo unknown)"
BUILD_TIME="$(date -u +%Y-%m-%dT%H:%M:%SZ)"

log() { printf "[build-package] %s\n" "$*" >&2; }
die() { log "ERROR: $*"; exit "${2:-1}"; }

while [[ $# -gt 0 ]]; do
    case "$1" in
        --deb)             BUILD_DEB=1; shift ;;
        --validate-only)   VALIDATE_ONLY=1; shift ;;
        --skip-dashboard)  SKIP_DASHBOARD=1; shift ;;
        --staging)         STAGING_DIR="$2"; shift 2 ;;
        -h|--help)
            sed -n '2,18p' "$0"
            exit 0 ;;
        *) die "unknown argument: $1" ;;
    esac
done

require() {
    command -v "$1" >/dev/null 2>&1 || die "missing required tool: $1" 3
}

require git
[[ "${VALIDATE_ONLY}" -eq 1 ]] || require cargo
[[ "${SKIP_DASHBOARD}" -eq 1 ]] || require python3

log "version=${PKG_VERSION} commit=${GIT_COMMIT} staging=${STAGING_DIR}"

# ---------------------------------------------------------------------------
# 1. Clean staging tree.
# ---------------------------------------------------------------------------
rm -rf "${STAGING_DIR}"
mkdir -p \
    "${STAGING_DIR}/usr/bin" \
    "${STAGING_DIR}/usr/lib/mai/adapters" \
    "${STAGING_DIR}/usr/lib/mai/compliance-dashboard" \
    "${STAGING_DIR}/usr/lib/mai/scripts" \
    "${STAGING_DIR}/usr/share/doc/mai" \
    "${STAGING_DIR}/lib/systemd/system" \
    "${STAGING_DIR}/etc/mai/policies" \
    "${STAGING_DIR}/etc/mai/trust-anchors" \
    "${STAGING_DIR}/DEBIAN"

# ---------------------------------------------------------------------------
# 2. Rust binaries (mai-api today; mai-admin will land at SHIP-09).
# ---------------------------------------------------------------------------
if [[ "${VALIDATE_ONLY}" -eq 0 ]]; then
    log "building release binaries"
    cargo build --release --workspace --locked
    install -m 0755 target/release/mai-api "${STAGING_DIR}/usr/bin/mai-api"
fi

install -m 0755 packaging/scripts/mai-ship-validate.sh \
    "${STAGING_DIR}/usr/bin/mai-ship-validate"
install -m 0755 packaging/scripts/mai-healthcheck.sh \
    "${STAGING_DIR}/usr/lib/mai/scripts/mai-healthcheck.sh"

# ---------------------------------------------------------------------------
# 3. Compliance dashboard (Python).
# ---------------------------------------------------------------------------
if [[ "${SKIP_DASHBOARD}" -eq 0 ]]; then
    log "staging compliance dashboard"
    rsync -a --delete \
        --exclude '__pycache__' --exclude '.venv' --exclude 'tests' \
        compliance-dashboard/ \
        "${STAGING_DIR}/usr/lib/mai/compliance-dashboard/"

    if [[ -f compliance-dashboard/requirements.txt ]]; then
        log "vendoring dashboard wheels"
        python3 -m pip download --quiet --disable-pip-version-check \
            -r compliance-dashboard/requirements.txt \
            -d "${STAGING_DIR}/usr/lib/mai/compliance-dashboard/wheels"
    fi
fi

# ---------------------------------------------------------------------------
# 4. systemd units.
# ---------------------------------------------------------------------------
install -m 0644 packaging/systemd/mai-api.service \
    "${STAGING_DIR}/lib/systemd/system/mai-api.service"
install -m 0644 packaging/systemd/mai-dashboard.service \
    "${STAGING_DIR}/lib/systemd/system/mai-dashboard.service"
install -m 0644 packaging/systemd/mai-adapter-manager.service \
    "${STAGING_DIR}/lib/systemd/system/mai-adapter-manager.service"
install -m 0644 packaging/systemd/mai-healthcheck.service \
    "${STAGING_DIR}/lib/systemd/system/mai-healthcheck.service"
install -m 0644 packaging/systemd/mai-healthcheck.timer \
    "${STAGING_DIR}/lib/systemd/system/mai-healthcheck.timer"

# ---------------------------------------------------------------------------
# 5. Config templates.
# ---------------------------------------------------------------------------
install -m 0640 config/production.example.toml \
    "${STAGING_DIR}/etc/mai/profile.toml"
install -m 0640 config/auth_keys.toml \
    "${STAGING_DIR}/etc/mai/auth_keys.toml"
cat > "${STAGING_DIR}/etc/mai/dashboard-logging.json" <<'EOF'
{
  "version": 1,
  "disable_existing_loggers": false,
  "formatters": {
    "json": {"format": "%(asctime)s %(levelname)s %(name)s %(message)s"}
  },
  "handlers": {
    "stdout": {"class": "logging.StreamHandler", "stream": "ext://sys.stdout", "formatter": "json"}
  },
  "root": {"level": "INFO", "handlers": ["stdout"]}
}
EOF

# ---------------------------------------------------------------------------
# 6. Docs + package metadata.
# ---------------------------------------------------------------------------
for doc in README.md docs/SHIP-PROFILE.md docs/SHIP-HARDENING-PLAN.md packaging/README.md; do
    if [[ -f "${doc}" ]]; then
        install -m 0644 "${doc}" \
            "${STAGING_DIR}/usr/share/doc/mai/$(basename "${doc}")"
    fi
done

cat > "${STAGING_DIR}/usr/share/doc/mai/PACKAGE_BUILD_INFO" <<EOF
name=mai
version=${PKG_VERSION}
git_commit=${GIT_COMMIT}
build_time=${BUILD_TIME}
profile=ship
host=$(hostname)
EOF

# ---------------------------------------------------------------------------
# 7. Maintainer scripts (copied for both Debian and tarball use).
# ---------------------------------------------------------------------------
install -m 0755 packaging/scripts/preinstall.sh  "${STAGING_DIR}/DEBIAN/preinst"
install -m 0755 packaging/scripts/postinstall.sh "${STAGING_DIR}/DEBIAN/postinst"
install -m 0755 packaging/scripts/preremove.sh   "${STAGING_DIR}/DEBIAN/prerm"
install -m 0755 packaging/scripts/postremove.sh  "${STAGING_DIR}/DEBIAN/postrm"

# ---------------------------------------------------------------------------
# 8. Validate the staged profile parses against the production guard.
#    `mai-api validate` is the SHIP-07 readiness gate; until a separate
#    `mai-ship-validate --offline --package-root` mode lands we run the
#    profile check directly here.
# ---------------------------------------------------------------------------
if [[ "${VALIDATE_ONLY}" -eq 0 ]]; then
    log "validating staged profile"
    if ! "${STAGING_DIR}/usr/bin/mai-api" validate \
            --profile "${STAGING_DIR}/etc/mai/profile.toml"; then
        die "production guard rejected staged profile" 2
    fi
fi

# ---------------------------------------------------------------------------
# 9. Optionally drive dpkg-buildpackage.
# ---------------------------------------------------------------------------
if [[ "${BUILD_DEB}" -eq 1 ]]; then
    require dpkg-buildpackage
    log "running dpkg-buildpackage -us -uc -b"
    cp -r packaging/debian debian
    STAGING_DIR="${STAGING_DIR}" dpkg-buildpackage -us -uc -b
fi

log "staging tree ready at ${STAGING_DIR}"
log "done"
