# 0.2 — Emergency WSF exposure containment

Baseline `6ffaaee`; on `session/SEC-1` after 0.1 (`21efec1`).

Gate ladder (raw cargo):
- cargo fmt --check ............... PASS (exit 0)
- cargo check --workspace ......... PASS (exit 0)
- cargo clippy -p wsf-api ......... PASS (exit 0) — no new lint. Workspace clippy
  stays red on the pre-existing AQ-001 (`mai-core/src/cache.rs:109`), owned by Q1.
- cargo test -p wsf-api ........... PASS (exit 0). wsf-api has no crate-local
  tests (0 ran); the full workspace suite was green at baseline and re-runs at
  the M0 close.
YAML validity (yaml.safe_load): wsf-ha OK, appliance OK, shadow OK.

Containment applied:
- crates/wsf-api/src/main.rs: default bind 0.0.0.0:8300 -> 127.0.0.1:8300
  (production fail-safe; explicit WSF_LISTEN widens it behind an ingress).
- deployment/wsf-ha (production/HA): removed host publication of wsf-api (8300)
  and openbao (8200); wsf-api sets WSF_LISTEN=0.0.0.0:8300 to bind the internal
  compose network for the load balancer only.
- deployment/appliance + deployment/shadow (opt-in demos): all host ports
  loopback-bound (127.0.0.1); dev-root-token stack is localhost-only, headers
  marked insecure opt-in demos.

Gate (plan 0.2): an unauthenticated host request cannot reach token issue /
attenuate, seal / unseal, credential exchange, or receipts in the production/HA
posture — those routes are no longer host-published. Static + config proof
(git diff `containment.diff`, YAML render). The live black-box proof (spin the
stack, curl the now-unpublished port) rides the Phase-A ingress gate (A5) once
the authenticated ingress exists and the images are built.
