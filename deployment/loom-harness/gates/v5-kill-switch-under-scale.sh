#!/bin/sh
# V5 — kill-switch-under-scale on the live estate (A1.12 bar 7; doctrine I-9).
#
# Under estate scale (SCALE workload objects written across the control plane), a
# revocation published to the leader must replicate to EVERY control-plane
# replica's committed state — the substrate each replica's kill switch (G9) polls
# before it serves the next call. Assert all five CPs read the revocation back
# within the SLO; a replica that has not is one that would still serve an
# authoritative allow (a missed kill).
#
# Scope: this proves the revocation fans out to every replica over real Raft,
# under scale. The signature-verified kill-switch DECISION and its fail-closed
# behavior (rogue/stale snapshot denies) are proven in the in-process
# `aog-conformance` bar `kill_switch_under_scale` (real ML-DSA-87 anchor +
# fabric-revocation), which the aggressive profile runs at 5 replicas x 100.
#
# Prereq: estate up (docker compose -f deployment/loom-harness/docker-compose.yml
# up -d --wait). Exits 0 on PASS, 1 on FAIL.
set -eu

PROJECT="${LOOM_PROJECT:-loom-harness}"
SCALE="${V5_SCALE:-100}"
SLO_SECS="${V5_SLO_SECS:-3}"
REV_KEY="wsf/revocation/estate"
CPS="cp1 cp2 cp3 cp4 cp5"

write_to() { # svc key value-json-array
  body='{"Put":{"key":"'"$2"'","value":'"$3"',"expected":"Any"}}'
  docker exec "${PROJECT}-$1-1" curl -s --max-time 6 -X POST \
    -H 'content-type: application/json' -d "$body" http://127.0.0.1:4600/admin/write
}
get_from() { # svc key
  docker exec "${PROJECT}-$1-1" curl -s --max-time 6 -X POST \
    -H 'content-type: application/json' -d '{"key":"'"$2"'"}' http://127.0.0.1:4600/admin/get
}

echo "== scale: write $SCALE workloads round-robin across the control plane =="
i=0
while [ "$i" -lt "$SCALE" ]; do
  set -- cp1 cp2 cp3 cp4 cp5
  eval "svc=\${$(( (i % 5) + 1 ))}"
  write_to "$svc" "Workload/v5-scale-$i" '[87,76]' >/dev/null
  i=$((i + 1))
done
echo "  wrote $SCALE workloads"

echo "== publish the kill: revoke via cp1 (leader-transparent) =="
pub="$(write_to cp1 "$REV_KEY" '[82,69,86]')"
echo "  publish: $pub"
case "$pub" in *Applied*) : ;; *) echo "FAIL: revocation write did not commit: $pub" >&2; exit 1 ;; esac

echo "== assert the kill reached every replica within ${SLO_SECS}s =="
deadline=$(( $(date +%s) + SLO_SECS + 2 ))
rc=0
for s in $CPS; do
  seen=""
  while [ "$(date +%s)" -le "$deadline" ]; do
    r="$(get_from "$s" "$REV_KEY")"
    case "$r" in *'"value"'*) seen=1; break ;; esac
    sleep 1
  done
  if [ -n "$seen" ]; then
    echo "  $s: kill present"
  else
    echo "  $s: kill ABSENT past SLO — replica would serve an authoritative allow" >&2
    rc=1
  fi
done

[ "$rc" -eq 0 ] && echo "V5 PASS: under $SCALE-object scale, the kill reached all 5 replicas within SLO" || echo "V5 FAIL"
exit "$rc"
