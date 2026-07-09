//! Request handler modules for the MAI REST API.
//!
//! Each sub-module implements handlers for a route group.
//! All handlers accept AppState via axum's State extractor
//! and ProfileInfo via request extensions (injected by auth middleware).

pub mod compliance;
pub mod health;
pub mod inference;
// GET /v1/metrics — Prometheus text-format exposition.
pub mod metrics;
pub mod models;
pub mod system;
pub mod telemetry;
pub mod trust;
pub mod updates;
