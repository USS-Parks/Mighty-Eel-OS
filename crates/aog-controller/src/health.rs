//! Provider health probing: the signal the [`ProviderPoolController`]
//! (crate::providers) folds into a pool's schedulable set.
//!
//! A [`HealthProbe`] answers "is this model endpoint reachable right now?".
//! [`HttpHealthProbe`] is the live impl: for a provider with a configured base
//! URL it issues a liveness GET and treats any non-2xx or transport error as
//! **unhealthy** (fail-closed, doctrine I-4); for a provider with no URL — a
//! local, air-gapped model with no HTTP surface — it falls back to the
//! endpoint's declared `healthy` flag. Provider base URLs are deployment
//! configuration, not signed desired-state, so they live here rather than in
//! the estate.

use std::collections::HashMap;
use std::future::Future;
use std::time::Duration;

use reqwest::Client;

use aog_estate::ModelEndpoint;

use crate::runtime::ReconcileError;

/// Answers whether a model endpoint of a provider is healthy (schedulable).
/// Fail-closed: any uncertainty resolves to `false`.
pub trait HealthProbe: Send + Sync {
    /// Is `endpoint` of `provider` healthy right now?
    fn healthy(
        &self,
        provider: &str,
        endpoint: &ModelEndpoint,
    ) -> impl Future<Output = bool> + Send;
}

/// Live HTTP liveness probe. Providers are matched to a base URL; the endpoint
/// is healthy iff `GET <base>/<health_path>` returns 2xx within the timeout.
/// A provider with no configured URL falls back to the endpoint's declared
/// `healthy` flag (local / air-gapped models).
pub struct HttpHealthProbe {
    http: Client,
    base_urls: HashMap<String, String>,
    health_path: String,
}

impl HttpHealthProbe {
    /// Probe `base/healthz` for each provider in `base_urls`.
    ///
    /// # Errors
    /// [`ReconcileError`] if the HTTP client cannot be built.
    pub fn new(base_urls: HashMap<String, String>) -> Result<Self, ReconcileError> {
        Self::with_path(base_urls, "healthz")
    }

    /// Probe a custom health path (e.g. `v1/models`).
    ///
    /// # Errors
    /// [`ReconcileError`] if the HTTP client cannot be built.
    pub fn with_path(
        base_urls: HashMap<String, String>,
        health_path: impl Into<String>,
    ) -> Result<Self, ReconcileError> {
        let http = Client::builder()
            .timeout(Duration::from_secs(5))
            .build()
            .map_err(|e| ReconcileError(e.to_string()))?;
        Ok(Self {
            http,
            base_urls,
            health_path: health_path.into(),
        })
    }
}

impl HealthProbe for HttpHealthProbe {
    fn healthy(
        &self,
        provider: &str,
        endpoint: &ModelEndpoint,
    ) -> impl Future<Output = bool> + Send {
        let target = self
            .base_urls
            .get(provider)
            .map(|base| format!("{base}/{}", self.health_path));
        let declared = endpoint.healthy;
        let http = self.http.clone();
        async move {
            match target {
                // Live provider: a liveness GET must return 2xx. A non-2xx or
                // any transport error (unreachable, timeout) is unhealthy.
                Some(url) => match http.get(url).send().await {
                    Ok(resp) => resp.status().is_success(),
                    Err(_) => false,
                },
                // No probe target: trust the endpoint's declared availability.
                None => declared,
            }
        }
    }
}
