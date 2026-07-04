//! Shared gateway application state + the auth seam every inference surface
//! funnels through.
//!
//! The OpenAI (G3) and Anthropic (G4) surfaces are thin translators in front of
//! the same [`AppState`]: authorize the virtual key (G1), map the requested model
//! to a provider target, dispatch through the [`Registry`] (G2), and translate the
//! neutral response back to the caller's wire format. A [`ModelMap`] is the
//! model-alias → provider routing config; G5 layers classify-and-route on top of
//! it (e.g. PHI forces a local target regardless of the requested model).

use std::collections::HashMap;
use std::sync::Arc;

use axum::http::{HeaderMap, StatusCode};
use chrono::Utc;

use crate::provider::Registry;
use crate::{Gateway, ResolvedContext};

/// Where a requested model is dispatched: which registered provider, under what
/// upstream model id.
#[derive(Debug, Clone)]
pub struct Target {
    /// The [`crate::provider::Provider::name`] to dispatch to.
    pub provider: String,
    /// The upstream model id the provider should be called with.
    pub model: String,
}

impl Target {
    pub fn new(provider: impl Into<String>, model: impl Into<String>) -> Self {
        Self {
            provider: provider.into(),
            model: model.into(),
        }
    }
}

/// Model-alias → [`Target`] routing, with an optional default for unmapped ids.
#[derive(Debug, Clone, Default)]
pub struct ModelMap {
    map: HashMap<String, Target>,
    default: Option<Target>,
}

impl ModelMap {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Route an inbound model id to a provider target.
    #[must_use]
    pub fn route(mut self, inbound: impl Into<String>, target: Target) -> Self {
        self.map.insert(inbound.into(), target);
        self
    }

    /// Set the fallback target for any unmapped model id.
    #[must_use]
    pub fn default_target(mut self, target: Target) -> Self {
        self.default = Some(target);
        self
    }

    /// Resolve an inbound model to its target (explicit mapping, else default).
    #[must_use]
    pub fn resolve(&self, model: &str) -> Option<&Target> {
        self.map.get(model).or(self.default.as_ref())
    }

    /// The explicitly-mapped model ids, sorted (for `/v1/models`).
    #[must_use]
    pub fn model_ids(&self) -> Vec<String> {
        let mut ids: Vec<String> = self.map.keys().cloned().collect();
        ids.sort();
        ids
    }
}

/// Everything an inference surface needs: auth (Gateway), dispatch (Registry),
/// routing (ModelMap). Cheap to clone (all `Arc`).
#[derive(Clone)]
pub struct AppState {
    pub gateway: Arc<Gateway>,
    pub registry: Arc<Registry>,
    pub models: Arc<ModelMap>,
}

impl AppState {
    #[must_use]
    pub fn new(gateway: Arc<Gateway>, registry: Arc<Registry>, models: Arc<ModelMap>) -> Self {
        Self {
            gateway,
            registry,
            models,
        }
    }
}

/// Authorize a request: extract the bearer virtual key, resolve + verify its
/// trust token, and run the pre-flight budget check (all of G1). Returns the
/// verified [`ResolvedContext`] or an HTTP error tuple.
pub(crate) async fn authorize(
    state: &AppState,
    headers: &HeaderMap,
) -> Result<ResolvedContext, (StatusCode, String)> {
    let key = crate::http::bearer_key(headers)?;
    state
        .gateway
        .resolve_and_check(key, Utc::now())
        .await
        .map_err(crate::http::to_http)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_map_routes_and_defaults() {
        let m = ModelMap::new()
            .route("gpt-4o", Target::new("openai", "gpt-4o"))
            .default_target(Target::new("local", "llama3"));
        assert_eq!(m.resolve("gpt-4o").unwrap().provider, "openai");
        // unmapped → default.
        assert_eq!(m.resolve("mystery").unwrap().provider, "local");
        assert_eq!(m.model_ids(), vec!["gpt-4o".to_string()]);
    }

    #[test]
    fn model_map_no_default_misses() {
        let m = ModelMap::new().route("gpt-4o", Target::new("openai", "gpt-4o"));
        assert!(m.resolve("mystery").is_none());
    }
}
