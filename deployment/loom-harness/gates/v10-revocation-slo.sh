#!/bin/sh
# V10 — revocation-to-denial SLO on the live estate ("the kill number"; RC-KILL,
# doctrine I-9). Publish a revocation to the leader and time how long until EVERY
# control-plane replica reflects it in committed state. Repeat ITERS rounds; the
# gate is that the kill reaches all five CPs within the SLO every round, with the
# worst reported.
#
# The strict p99 <= 3s bound is asserted precisely in the in-process
# `aog-conformance` gate `revocation_to_denial_slo` (real openraft + a real
# ML-DSA-87-signed fabric-revocation snapshot; measured there p50 ~0.20s / p99
# ~0.37s over 100 revocations across 5 replicas), where wall-clock is the estate's.
# On this live harness each poll is a docker-exec round-trip (~sub-second), so the
# live SLO is padded for harness overhead — this proves the fan-out end-to-end on
# the containerized estate, the in-process gate proves the number.
#
# Prereq: estate up (docker compose -f deployment/loom-harness/docker-compose.yml
# up -d --wait). Exits 0 on PASS, 1 on FAIL.
set -eu

PROJECT="${LOOM_PROJECT:-loom-harness}"
ITERS="${V10_ITERS:-5}"
SLO_SECS="${V10_SLO_SECS:-10}"
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

worst=0
rc=0
i=0
while [ "$i" -lt "$ITERS" ]; do
  key="wsf/revocation/kill-$i"
  t0=$(date +%s)
  pub="$(write_to cp1 "$key" '[75,73,76,76]')"
  case "$pub" in *Applied*) : ;; *) echo "FAIL round $i: publish did not commit: $pub" >&2; exit 1 ;; esac

  deadline=$(( t0 + SLO_SECS ))
  pending="$CPS"
  while [ -n "$pending" ] && [ "$(date +%s)" -le "$deadline" ]; do
    next=""
    for s in $pending; do
      r="$(get_from "$s" "$key")"
      case "$r" in *'"value"'*) : ;; *) next="$next $s" ;; esac
    done
    pending="$(printf '%s' "$next" | sed 's/^ *//')"
    [ -n "$pending" ] && sleep 1
  done

  elapsed=$(( $(date +%s) - t0 ))
  if [ -n "$pending" ]; then
    echo "  round $i: kill NOT reflected on:$pending within ${SLO_SECS}s" >&2
    rc=1
  else
    echo "  round $i: kill reached all 5 replicas in ${elapsed}s"
    [ "$elapsed" -gt "$worst" ] && worst=$elapsed
  fi
  i=$((i + 1))
done

if [ "$rc" -eq 0 ]; then
  echo "V10 PASS: revocation reached every replica within ${SLO_SECS}s each round (worst ${worst}s); the strict p99<=3s is the in-process gate"
else
  echo "V10 FAIL"
fi
exit "$rc"
