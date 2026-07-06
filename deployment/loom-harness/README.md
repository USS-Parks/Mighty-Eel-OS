# Loom Phase-V conformance harness

A containerized multi-node Loom estate for the Phase-V live gates
(V4/V5/V7/V8/V10): **5 `aogd` control-plane nodes** (a Raft cluster over the
`aog-wire` transport) + **5 `aog-noded` edge nodes** (register + heartbeat) +
**OpenBao** (dev), from the `loom-harness` image
(`deployment/loom-harness/Dockerfile`, VH4).

## Run

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

## Scope

**VH5 (this):** the 5+5 estate topology + OpenBao service + cluster formation, all
over the wire — the substrate the live gates run on.

**VH5b (next):** the trust hardening — authenticated `aog-apiserver` CRUD (via a new
`AppState::from_raft` seam), per-node mTLS on the wire transport (doctrine I-3), and
the OpenBao-provisioned anchor + `Sealer`/`Authenticator`. Until then the admin API
is unauthenticated (consistent with VH2/VH3) and the wire transport is plain HTTP.
