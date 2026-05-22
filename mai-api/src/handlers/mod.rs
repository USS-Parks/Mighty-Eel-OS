//! Request handler modules for the MAI REST API.
//!
//! Each sub-module implements handlers for a route group.
//! All handlers accept AppState via axum's State extractor
//! and ProfileInfo via request extensions (injected by auth middleware).

pub mod health;
pub mod inference;
pub mod models;
pub mod system;
pub mod telemetry;
