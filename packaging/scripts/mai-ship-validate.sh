#!/bin/sh
# packaging/scripts/mai-ship-validate.sh - thin /usr/bin/mai-ship-validate.
#
# Until SHIP-07's standalone `mai-ship-validate` binary ships as a
# separate Cargo target, the production guard is reachable via the
# `mai-api validate` subcommand. This wrapper preserves the operator UX
# (and the ExecStartPre line in mai-api.service) so SHIP-08 packages
# install something at /usr/bin/mai-ship-validate today and a later
# release can swap it for a real binary at the same path.

set -eu

MAI_API_BIN="${MAI_API_BIN:-/usr/bin/mai-api}"

if [ ! -x "${MAI_API_BIN}" ]; then
    echo "ERROR: ${MAI_API_BIN} not found or not executable." >&2
    echo "       mai-ship-validate requires the mai-api binary to be installed." >&2
    exit 4
fi

exec "${MAI_API_BIN}" validate "$@"
