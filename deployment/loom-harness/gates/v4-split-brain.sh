#!/bin/sh
# V4 — split-brain safety on the live estate (doctrine I-4 / addendum H2; A1.12 bar 3).
#
# Partition a 2-of-5 minority off the control-plane network; assert the majority
# (still a quorum) elects/keeps a leader and COMMITS a write, while an isolated
# minority node has NO leader and CANNOT commit (it serves no authoritative allow —
# it fences). Then heal and let the cluster reconverge to one leader.
#
# Prereq: the estate is up (docker compose -f deployment/loom-harness/docker-compose.yml
# up -d --wait). Exits 0 on PASS, 1 on FAIL.
set -eu

HERE="$(cd "$(dirname "$0")" && pwd)"
HARNESS="$(dirname "$HERE")"
PROJECT="${LOOM_PROJECT:-loom-harness}"

part() { sh "$HARNESS/partition.sh" "$@"; }
leader_of() { docker exec "${PROJECT}-$1-1" curl -s --max-time 4 http://127.0.0.1:4600/admin/leader; }
write_to() { # svc key
  body='{"Put":{"key":"'"$2"'","value":[118,52],"expected":"Any"}}'
  docker exec "${PROJECT}-$1-1" curl -s --max-time 6 -X POST \
    -H 'content-type: application/json' -d "$body" http://127.0.0.1:4600/admin/write
}

echo "== partition minority {cp1,cp2} =="
part partition cp1 cp2
sleep 6

maj=""
for s in cp3 cp4 cp5; do
  l="$(leader_of "$s")"
  echo "  $s: $l"
  case "$l" in *'"is_leader":true'*) maj="$s" ;; esac
done

major_resp=""
if [ -n "$maj" ]; then
  major_resp="$(write_to "$maj" Workload/v4-major 2>/dev/null || true)"
fi
# The isolated minority either refuses fast ("no leader") or, if cp1 still knows a
# now-unreachable leader, its forward hangs until curl's --max-time — both mean
# "did not commit". Tolerate the failure (|| true) so `set -e` does not abort the
# gate; the assertions below (major must be Applied, minor must not) are the judge.
minor_resp="$(write_to cp1 Workload/v4-minor 2>/dev/null || true)"
echo "  majority ($maj) write: $major_resp"
echo "  minority (cp1) write:  $minor_resp"

echo "== heal {cp1,cp2} =="
part heal cp1 cp2
sleep 6
for s in cp1 cp2 cp3 cp4 cp5; do echo "  post-heal $s: $(leader_of "$s")"; done

rc=0
case "$major_resp" in *Applied*) : ;; *) echo "FAIL: majority (quorum) did not commit" >&2; rc=1 ;; esac
case "$minor_resp" in *Applied*) echo "FAIL: isolated minority committed — split-brain" >&2; rc=1 ;; esac
[ "$rc" -eq 0 ] && echo "V4 PASS: majority commits under partition, isolated minority fences" || echo "V4 FAIL"
exit "$rc"
