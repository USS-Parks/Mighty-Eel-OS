#!/bin/sh
# V8 — scale on the live estate (A1.12 bar 6): N control-plane replicas + M
# workloads reconcile within SLO. Ingest SCALE Workload objects across the
# control plane, then assert every object is readable on EVERY one of the five
# CP replicas — the N-node fan-out at the aggressive profile (5 CP, 100 objects).
#
# The reconcile-to-convergence timing bound is proven precisely in the in-process
# `aog-conformance` bar `scale_target` (real aog-controller reconcile runtime),
# where wall-clock is the estate's, not the harness's. Here the gate is
# completeness: no replica may be missing an object under scale. Ingest wall-clock
# is reported as the estate's commit throughput (the per-object docker-exec
# round-trip dominates the harness side, so it is a report, not the bound).
#
# Prereq: estate up (docker compose -f deployment/loom-harness/docker-compose.yml
# up -d --wait). Exits 0 on PASS, 1 on FAIL.
set -eu

PROJECT="${LOOM_PROJECT:-loom-harness}"
SCALE="${V8_SCALE:-100}"
CPS="cp1 cp2 cp3 cp4 cp5"

write_to() { # svc key value-json-array — bounded retry: an election window on a
  # contended runner must not fail a must-succeed write one-shot.
  body='{"Put":{"key":"'"$2"'","value":'"$3"',"expected":"Any"}}'
  wt_n=0
  while :; do
    wt_r="$(docker exec "${PROJECT}-$1-1" curl -s --max-time 6 -X POST \
      -H 'content-type: application/json' -d "$body" http://127.0.0.1:4600/admin/write)" || wt_r=""
    case "$wt_r" in *Applied*) printf '%s' "$wt_r"; return 0 ;; esac
    wt_n=$((wt_n + 1))
    if [ "$wt_n" -ge 10 ]; then printf '%s' "$wt_r"; return 0; fi
    sleep 1
  done
}
get_from() { # svc key
  docker exec "${PROJECT}-$1-1" curl -s --max-time 6 -X POST \
    -H 'content-type: application/json' -d '{"key":"'"$2"'"}' http://127.0.0.1:4600/admin/get
}

echo "== ingest $SCALE workloads round-robin across the control plane =="
t0=$(date +%s)
i=0
while [ "$i" -lt "$SCALE" ]; do
  set -- cp1 cp2 cp3 cp4 cp5
  eval "svc=\${$(( (i % 5) + 1 ))}"
  r="$(write_to "$svc" "Workload/v8-scale-$i" '[87,76]')"
  case "$r" in *Applied*) : ;; *) echo "FAIL: workload $i did not commit: $r" >&2; exit 1 ;; esac
  i=$((i + 1))
done
echo "  ingested $SCALE workloads in $(( $(date +%s) - t0 ))s (estate commit throughput)"

echo "== assert every object replicated to every CP replica (completeness) =="
rc=0
for s in $CPS; do
  missing=0
  w=0
  while [ "$w" -lt "$SCALE" ]; do
    r="$(get_from "$s" "Workload/v8-scale-$w")"
    case "$r" in *'"value"'*) : ;; *) missing=$((missing + 1)) ;; esac
    w=$((w + 1))
  done
  if [ "$missing" -eq 0 ]; then
    echo "  $s: all $SCALE present"
  else
    echo "  $s: missing $missing/$SCALE objects — incomplete replication under scale" >&2
    rc=1
  fi
done

[ "$rc" -eq 0 ] && echo "V8 PASS: $SCALE workloads fully replicated across all 5 replicas" || echo "V8 FAIL"
exit "$rc"
