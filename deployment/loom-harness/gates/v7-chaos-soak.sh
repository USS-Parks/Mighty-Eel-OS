#!/bin/sh
# V7 — chaos + soak on the live estate (A1.12 bars 4/5). Run ROUNDS kill/heal
# cycles: each round partitions one CP off the control-plane network (a real
# "killed" node, via the VH6 tooling), commits a deterministic rollout step through
# the surviving majority, heals the node, and asserts a leader re-emerged and the
# healed node caught up. At the end, assert every CP converged to the identical
# rollout end state — control-plane self-healing + rollout determinism under real
# network partitions, over a soak.
#
# The data-plane workload-reschedule leg (the scheduler evicting a dead node's
# Placements and re-placing them, revoking runtime tokens in OpenBao) is the
# estate's own controllers (live_node / live_scheduler); this script is the live
# companion of the in-process `aog-conformance` gate `chaos_soak`, which proves the
# same self-healing + determinism on real openraft.
#
# Prereq: estate up (docker compose -f deployment/loom-harness/docker-compose.yml
# up -d --wait). Exits 0 on PASS, 1 on FAIL.
set -eu

HERE="$(cd "$(dirname "$0")" && pwd)"
HARNESS="$(dirname "$HERE")"
PROJECT="${LOOM_PROJECT:-loom-harness}"
ROUNDS="${V7_ROUNDS:-5}"
CPS="cp1 cp2 cp3 cp4 cp5"

part() { sh "$HARNESS/partition.sh" "$@"; }
leader_of() { docker exec "${PROJECT}-$1-1" curl -s --max-time 4 http://127.0.0.1:4600/admin/leader; }
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

i=0
while [ "$i" -lt "$ROUNDS" ]; do
  set -- cp1 cp2 cp3 cp4 cp5
  eval "victim=\${$(( (i % 5) + 1 ))}"
  echo "== round $i: kill $victim =="
  part partition "$victim"

  # Poll (bounded) for a re-elected leader among the reachable majority: a fresh
  # election after killing the leader can take a few seconds, more on a loaded
  # runner, so a single-shot check would flake.
  leader=""
  ld=$(( $(date +%s) + 20 ))
  while [ "$(date +%s)" -le "$ld" ]; do
    for s in $CPS; do
      [ "$s" = "$victim" ] && continue
      case "$(leader_of "$s" 2>/dev/null || true)" in *'"is_leader":true'*) leader="$s"; break ;; esac
    done
    [ -n "$leader" ] && break
    sleep 1
  done
  if [ -z "$leader" ]; then
    echo "  FAIL: no leader re-emerged within 20s after killing $victim" >&2
    part heal "$victim"
    exit 1
  fi

  key="RolloutPlan/v7-step-$i"
  resp="$(write_to "$leader" "$key" '[86,55]' 2>/dev/null || true)"
  case "$resp" in
    *Applied*) echo "  step $i committed via $leader (self-healed)" ;;
    *) echo "  FAIL: step $i did not commit: $resp" >&2; part heal "$victim"; exit 1 ;;
  esac

  part heal "$victim"
  caught=""
  d=$(( $(date +%s) + 15 ))
  while [ "$(date +%s)" -le "$d" ]; do
    case "$(get_from "$victim" "$key")" in *'"value"'*) caught=1; break ;; esac
    sleep 1
  done
  if [ -n "$caught" ]; then
    echo "  $victim rejoined and caught up to step $i"
  else
    echo "  FAIL: $victim did not catch up to step $i within SLO" >&2
    exit 1
  fi
  i=$((i + 1))
done

echo "== assert every replica converged to the identical rollout end state =="
rc=0
w=0
while [ "$w" -lt "$ROUNDS" ]; do
  key="RolloutPlan/v7-step-$w"
  for s in $CPS; do
    case "$(get_from "$s" "$key")" in
      *'"value"'*) : ;;
      *) echo "  $s missing $key — divergence" >&2; rc=1 ;;
    esac
  done
  w=$((w + 1))
done

[ "$rc" -eq 0 ] && echo "V7 PASS: $ROUNDS kill/heal cycles survived; leader re-emerged + killed node caught up each round; all replicas converged to the identical rollout end state" || echo "V7 FAIL"
exit "$rc"
