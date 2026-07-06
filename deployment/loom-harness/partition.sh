#!/bin/sh
# VH6 — real network partitions over the Loom estate.
#
# Cut a set of services off the compose network (they lose ALL peer connectivity —
# a real partition, not a simulated one) and heal them back. The heal restores the
# compose service alias: a plain `docker network connect` rejoins the container but
# does NOT restore the `<service>` DNS alias, so peers still can't resolve it — the
# heal would silently half-work. The split-brain / kill / chaos gates (V4/V5/V7)
# drive this. Observe a cut node via `docker exec` (bypasses the network), never via
# its now-unreachable published port.
#
# Args are compose SERVICE names (cp1, cp5, edge3); the container is
# ${LOOM_PROJECT}-<svc>-1 and the restored alias is <svc>.
#
# Usage:
#   ./partition.sh partition <service>...   # isolate each service
#   ./partition.sh heal      <service>...   # rejoin each (alias restored)
#   LOOM_NET / LOOM_PROJECT override the network / compose project.
set -eu

NET="${LOOM_NET:-loom-harness_default}"
PROJECT="${LOOM_PROJECT:-loom-harness}"
cmd="${1:-}"
[ "$#" -ge 1 ] && shift
[ "$#" -ge 1 ] || {
  echo "usage: $0 {partition|heal} <service>..." >&2
  exit 2
}

case "$cmd" in
  partition)
    for svc in "$@"; do
      docker network disconnect "$NET" "${PROJECT}-${svc}-1"
      echo "cut $svc off $NET"
    done
    ;;
  heal)
    for svc in "$@"; do
      docker network connect --alias "$svc" "$NET" "${PROJECT}-${svc}-1"
      echo "healed $svc onto $NET (alias restored)"
    done
    ;;
  *)
    echo "usage: $0 {partition|heal} <service>..." >&2
    exit 2
    ;;
esac
