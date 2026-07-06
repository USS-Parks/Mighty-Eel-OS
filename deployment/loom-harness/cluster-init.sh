#!/bin/sh
# VH5 — form the 5-voter Loom control-plane cluster over the wire, from cp1:
# initialize cp1 as the sole voter, add cp2..cp5 as learners at their peer URLs,
# then promote all five to voters. The edges self-register once this completes.
set -eu

CP1="http://cp1:4600"

post() { # path json — retry with backoff; a transient hiccup must not abort formation
  n=0
  until curl -sf -X POST "$CP1$1" -H 'content-type: application/json' -d "$2" >/dev/null 2>&1; do
    n=$((n + 1))
    [ "$n" -ge 10 ] && {
      echo "POST $1 still failing after $n tries" >&2
      return 1
    }
    sleep 1
  done
}

# cp health already gates this service (depends_on), but re-confirm the admin
# surface answers before driving membership.
i=0
until curl -sf "$CP1/healthz" >/dev/null 2>&1; do
  i=$((i + 1))
  [ "$i" -gt 60 ] && {
    echo "cp1 never became ready" >&2
    exit 1
  }
  sleep 1
done

echo "initializing cp1 as the sole initial voter"
post /admin/initialize '{"members":[{"id":1,"addr":"http://cp1:4600"}]}'

for n in 2 3 4 5; do
  echo "adding cp$n as a learner over the wire"
  post /admin/add-learner "{\"id\":$n,\"addr\":\"http://cp$n:4600\"}"
done

echo "promoting cp1..cp5 to voters"
post /admin/change-membership '{"voters":[1,2,3,4,5]}'

echo "cluster formed; leader = $(curl -sf "$CP1/admin/leader")"
