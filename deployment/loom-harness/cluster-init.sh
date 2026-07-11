#!/bin/sh
# Form the 5-voter Loom control-plane cluster over the wire, from cp 1:
# initialize it as the sole voter, add the other four as learners at their peer
# URLs, then promote all five to voters. The edges self-register once this
# completes.
set -eu

# Peer address template: {id} is the 1-based voter id, {ordinal} is id-1.
# Defaults to the compose network's service names; the k3s packaging overrides
# it with the StatefulSet's stable DNS names. One script, either substrate —
# the daemons and their configuration surface never fork. (The default is
# assigned outside the ${...:-...} expansion: a literal } inside the default
# would terminate the expansion in POSIX sh.)
ADDR_TEMPLATE="${LOOM_CP_ADDR_TEMPLATE:-}"
[ -n "$ADDR_TEMPLATE" ] || ADDR_TEMPLATE='http://cp{id}:4600'
addr_of() {
  echo "$ADDR_TEMPLATE" | sed "s/{id}/$1/g; s/{ordinal}/$(($1 - 1))/g"
}
CP1="$(addr_of 1)"

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

echo "initializing cp 1 as the sole initial voter"
post /admin/initialize "{\"members\":[{\"id\":1,\"addr\":\"$(addr_of 1)\"}]}"

for n in 2 3 4 5; do
  echo "adding cp $n as a learner over the wire"
  post /admin/add-learner "{\"id\":$n,\"addr\":\"$(addr_of "$n")\"}"
done

echo "promoting cp1..cp5 to voters"
post /admin/change-membership '{"voters":[1,2,3,4,5]}'

echo "cluster formed; leader = $(curl -sf "$CP1/admin/leader")"
