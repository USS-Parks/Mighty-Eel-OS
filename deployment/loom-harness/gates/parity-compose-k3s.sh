#!/bin/sh
# Packaging-parity gate — "same binaries orchestrate under Compose and k3s; no
# config fork."
#
# Leg 1 stands the estate up under docker compose; leg 2 stands it up under a
# real k3s (rancher/k3s in Docker), from the SAME loom-harness image imported
# into the k3s containerd and the SAME cluster-init script (shipped to the Job
# via a ConfigMap created from the file). Each leg asserts the same two facts:
# a leader is elected across the 5-voter control plane, and every edge has
# self-registered its Node record into the replicated estate.
#
# Requires: docker (with compose), kubectl, the loom-harness:vh4 image built
# from this tree. Run from the workspace root:
#   sh deployment/loom-harness/gates/parity-compose-k3s.sh
set -eu

HARNESS="deployment/loom-harness"
COMPOSE="$HARNESS/docker-compose.yml"
K3S_NAME="loom-k3s-parity"
# Third-party substrate pinned by tag; the estate's own images are digest-pinned
# in the manifests they ship in.
K3S_IMAGE="rancher/k3s:v1.31.5-k3s1"
KCFG="$(mktemp)"
KUBECTL="kubectl --kubeconfig=$KCFG -n loom"

fail() {
  echo "PARITY GATE FAIL: $1" >&2
  exit 1
}

wait_leader() { # $1 = curl-command prefix ending in the base URL — echoes the
  # leader view once a leader id is present (election may lag formation).
  wl_n=0
  while :; do
    wl_r="$(eval "$1/admin/leader" 2>/dev/null || true)"
    case "$wl_r" in
    *'"leader":'[0-9]*)
      echo "$wl_r"
      return 0
      ;;
    esac
    wl_n=$((wl_n + 1))
    [ "$wl_n" -ge 30 ] && return 1
    sleep 2
  done
}

edge_registered() { # $1 = curl-command prefix, $2 = node name — the replicated
  # store returns the Node record's bytes under "value" (a JSON byte array), so
  # presence of a non-null value is the registration signal, not the name string.
  case "$(eval "$1/admin/get -XPOST -H content-type:application/json -d '{\"key\":\"Node/$2\"}'")" in
  *'"value":['*) return 0 ;;
  *) return 1 ;;
  esac
}

cleanup() {
  docker compose -f "$COMPOSE" down -v >/dev/null 2>&1 || true
  docker rm -f "$K3S_NAME" >/dev/null 2>&1 || true
  rm -f "$KCFG" /tmp/loom-harness-vh4.tar
}
trap cleanup EXIT

# ── Leg 1: compose ──────────────────────────────────────────────────────────
echo "[compose] estate up"
docker compose -f "$COMPOSE" up -d --wait --quiet-pull
CURL_C="curl -sf http://localhost:4601"

L="$(wait_leader "$CURL_C")" || fail "compose: no leader"
echo "[compose] leader elected: $L"

for e in edge1 edge2 edge3 edge4 edge5; do
  n=0
  until edge_registered "$CURL_C" "$e"; do
    n=$((n + 1))
    [ "$n" -ge 30 ] && fail "compose: $e never registered"
    sleep 2
  done
  echo "[compose] $e registered"
done

echo "[compose] estate down"
docker compose -f "$COMPOSE" down -v >/dev/null 2>&1

# ── Leg 2: k3s — same image, same init script, same assertions ──────────────
echo "[k3s] server up ($K3S_IMAGE)"
docker rm -f "$K3S_NAME" >/dev/null 2>&1 || true
docker run -d --name "$K3S_NAME" --privileged -p 6443:6443 "$K3S_IMAGE" \
  server --disable=traefik --disable=metrics-server --disable=servicelb \
  >/dev/null

n=0
until docker exec "$K3S_NAME" cat /etc/rancher/k3s/k3s.yaml >"$KCFG" 2>/dev/null &&
  kubectl --kubeconfig="$KCFG" get nodes >/dev/null 2>&1; do
  n=$((n + 1))
  [ "$n" -ge 60 ] && fail "k3s: apiserver never came up"
  sleep 2
done
echo "[k3s] apiserver ready"

echo "[k3s] importing the one artifact set into the k3s containerd"
docker save loom-harness:vh4 -o /tmp/loom-harness-vh4.tar
docker cp /tmp/loom-harness-vh4.tar "$K3S_NAME":/tmp/img.tar
docker exec "$K3S_NAME" ctr images import /tmp/img.tar >/dev/null

echo "[k3s] applying the estate"
kubectl --kubeconfig="$KCFG" create namespace loom >/dev/null 2>&1 || true
$KUBECTL create configmap cluster-init \
  --from-file=cluster-init.sh="$HARNESS/cluster-init.sh" >/dev/null
$KUBECTL apply -f "$HARNESS/k3s/loom.yaml" >/dev/null

echo "[k3s] waiting for the control plane"
$KUBECTL rollout status statefulset/cp --timeout=300s >/dev/null
$KUBECTL wait --for=condition=complete job/cluster-init --timeout=300s >/dev/null
echo "[k3s] cluster formed"

echo "[k3s] waiting for the edges"
$KUBECTL rollout status statefulset/edge --timeout=300s >/dev/null

CURL_K="$KUBECTL exec cp-0 -- curl -sf http://127.0.0.1:4600"

L="$(wait_leader "$CURL_K")" || fail "k3s: no leader"
echo "[k3s] leader elected: $L"

for e in edge-0 edge-1 edge-2 edge-3 edge-4; do
  n=0
  until edge_registered "$CURL_K" "$e"; do
    n=$((n + 1))
    [ "$n" -ge 30 ] && fail "k3s: $e never registered"
    sleep 2
  done
  echo "[k3s] $e registered"
done

echo "PARITY GATE PASS: the same binaries formed the estate under compose and k3s"
