//! `aog-node` — Loom's node / edge runtime (Phase N).
//!
//! The agent that runs on each estate node. It registers the node with a
//! verified identity and its attestation profile + capacity (N1), heartbeats its
//! liveness and reconciled allocatable capacity (N2), runs assigned workloads
//! through a pluggable driver (N3–N5), enforces admission at the edge with an
//! offline-safe cache (N6), health- and attestation-liveness-probes them
//! (N7/N8), and drains on eviction or revocation (N9).
//!
//! Trust posture: the node is the edge, so it fails **static-restrictive** — an
//! unreachable control plane or a stale decision reduces privilege, never
//! extends it (doctrine I-4). Its identity is a `fabric-identity` leaf; a node
//! that cannot prove it does not join.

pub mod driver;
pub mod heartbeat;
pub mod registration;
