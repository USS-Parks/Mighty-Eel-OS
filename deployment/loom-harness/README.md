# Loom estate — packagings + Phase-V conformance harness

A containerized multi-node Loom estate: **5 `aogd` control-plane nodes** (a
Raft cluster over the `aog-wire` transport) + **5 `aog-noded` edge nodes**
(register + heartbeat) + **OpenBao** (dev), from the one `loom-harness` image
(`deployment/loom-harness/Dockerfile`). The Phase-V live gates
(V4/V5/V7/V8/V10) run on the compose packaging of this estate.

**One artifact set, two packagings.** The same image, the same two binaries,
the same environment contract, and the same `cluster-init.sh` stand the estate
up under **docker compose** (`docker-compose.yml`) or under **k3s**
(`k3s/loom.yaml`) — k3s/k0s is an optional packaging of the estate for cluster
customers; the Loom control plane is the trust plane regardless of the
substrate beneath it. Nothing about the daemons or their configuration forks;
`gates/parity-compose-k3s.sh` is the executable proof.

**The hand-managed path is retired.** Running `aogd` / `aog-noded` by hand as
loose processes is not a supported way to operate an estate: pick a packaging.
The daemons remain plain binaries (the air-gap appliance runs them under
systemd through the same environment contract), but estate formation, health,
and lifecycle are the packaging's job — not an operator's shell history.

## Run (compose)

```sh
# from the workspace root
docker build -f deployment/loom-harness/Dockerfile -t loom-harness:vh4 .
docker compose -f deployment/loom-harness/docker-compose.yml up -d --wait
```

`cluster-init` forms the 5-voter cluster from `cp1` (initialize → add-learner ×4 →
change-membership); the edges self-register once it completes. `cp1`'s admin API is
published on host `:4601`:

```sh
curl -s localhost:4601/admin/leader
curl -s -XPOST localhost:4601/admin/get -H content-type:application/json \
  -d '{"key":"Node/edge1"}'
```

Teardown: `docker compose -f deployment/loom-harness/docker-compose.yml down -v`.

## Run (k3s)

```sh
# from the workspace root, against any k3s/k0s cluster whose containerd holds
# the loom-harness:vh4 image (rancher/k3s in Docker works: see the parity gate)
kubectl create namespace loom
kubectl -n loom create configmap cluster-init \
  --from-file=cluster-init.sh=deployment/loom-harness/cluster-init.sh
kubectl -n loom apply -f deployment/loom-harness/k3s/loom.yaml
kubectl -n loom wait --for=condition=complete job/cluster-init --timeout=300s
```

The `cp` StatefulSet's stable DNS names (`cp-0.cp` … `cp-4.cp`) replace the
compose service names; `cluster-init` receives them through
`LOOM_CP_ADDR_TEMPLATE` — the same script, parameterized, not forked. Edges
take their node name from their pod name (`edge-0` … `edge-4`).

Parity proof (both packagings from the one artifact set, end to end):

```sh
sh deployment/loom-harness/gates/parity-compose-k3s.sh
```

## Scope

**VH5 (this):** the 5+5 estate topology + OpenBao service + cluster formation, all
over the wire — the substrate the live gates run on.

**VH5b (next):** the trust hardening — authenticated `aog-apiserver` CRUD (via a new
`AppState::from_raft` seam), per-node mTLS on the wire transport (doctrine I-3), and
the OpenBao-provisioned anchor + `Sealer`/`Authenticator`. Until then the admin API
is unauthenticated (consistent with VH2/VH3) and the wire transport is plain HTTP.
