#![allow(unused_variables, dead_code, missing_docs)]
//! # MAI API Server
//!
//! REST (axum) + gRPC (tonic) + SSE/WebSocket streaming API server.
//! This is the syscall interface in the Tock trust model: the stable
//! contract that applications compile against. It never breaks backward
//! compatibility.
//!
//! ## Endpoints
//!
//! - `/v1/chat/completions` - Streaming and non-streaming chat
//! - `/v1/completions` - Text completion
//! - `/v1/embeddings` - Vector embedding
//! - `/v1/models` - Model listing and management
//! - `/v1/health` - System health
//! - `/v1/power` - Power state queries and transitions
//! - `/v1/admin/*` - Configuration and registry management
//!
//! ## Trust Model
//!
//! This crate is TRUSTED. It sits inside the Tock kernel boundary.
//! All external input is validated at this layer before reaching
//! mai-core. Backend adapter names are never exposed in responses.

#![forbid(unsafe_code)]

// Session 11a: Foundation + Middleware modules
pub mod types;
pub mod errors;
pub mod config;
pub mod auth;
pub mod audit;
pub mod air_gap;

// Session 11b: REST API Endpoints
pub mod state;
pub mod routes;
pub mod handlers;

// Session 11c: SSE Streaming + WebSocket
pub mod streaming;
