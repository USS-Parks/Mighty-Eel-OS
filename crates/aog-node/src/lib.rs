//! `aog-node` — Loom's node / edge runtime (Phase N).
//!
//! The agent that runs on each estate node. It registers the node with a
//! verified identity and its attestation profile + capacity, heartbeats its
//! liveness and reconciled allocatable capacity, runs assigned workloads
//! through a pluggable driver, enforces admission at the edge with an
//! offline-safe cache, health- and attestation-liveness-probes them
//! and drains on eviction or revocation.
//!
//! Trust posture: the node is the edge, so it fails **static-restrictive** — an
//! unreachable control plane or a stale decision reduces privilege, never
//! extends it (doctrine I-4). Its identity is a `fabric-identity` leaf; a node
//! that cannot prove it does not join.

pub mod attest;
pub mod containerd;
pub mod drain;
pub mod driver;
pub mod edge;
pub mod heartbeat;
pub mod probes;
pub mod registration;
