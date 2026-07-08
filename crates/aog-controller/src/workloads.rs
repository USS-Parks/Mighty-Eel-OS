//! X2 — the Workload controller: brings `aog-gateway` under Loom as a managed
//! `Workload`. In the M3a kernel it reconciles a gateway workload's **health
//! and readiness** into status and reflects the placements bound to it, so the
//! estate carries the gateway as a first-class managed object — with no change
//! to the gateway's data-path API (an existing OpenAI/Anthropic client is
//! unaffected: management touches the estate, never the request path).
//!
//! Scope honesty: attested **placement** is the scheduler's (Phase S) — this
//! controller *reflects* the `Placement`s bound to a workload but never mints
//! them; and the node runtime that actually starts/stops the process and runs
//! the live health probe is Phase N. In M3a the gateway runs alongside the
//! kernel and this controller observes it through a [`WorkloadProbe`]; M3b's
//! node agent supplies the authoritative probe.

use std::collections::HashMap;
use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use reqwest::Client;

use aog_estate::{Kind, Phase, ResourceObject, Workload, WorkloadKind, WorkloadStatus};

use crate::objects::{EstateClient, parse_key};
use crate::runtime::{Action, ReconcileError, Reconciler};

/// Answers whether a managed workload's replicas are live. Fail-closed: any
/// uncertainty is "not ready".
pub trait WorkloadProbe: Send + Sync {
    fn healthy(&self, workload: &Workload) -> impl Future<Output = bool> + Send;
}

/// The M3a default: trust that the co-running workload is live. The Phase-N node
/// agent replaces this with a probe driven by the actual process.
#[derive(Debug, Clone, Copy)]
pub struct StaticWorkloadProbe(pub bool);

impl WorkloadProbe for StaticWorkloadProbe {
    fn healthy(&self, _workload: &Workload) -> impl Future<Output = bool> + Send {
        let healthy = self.0;
        async move { healthy }
    }
}

/// A live HTTP liveness probe: a workload with a configured health URL is ready
/// iff `GET <url>` returns 2xx within the timeout. A workload with no URL cannot
/// be confirmed live, so it is **not** ready (fail-closed).
pub struct HttpWorkloadProbe {
    http: Client,
    health_urls: HashMap<String, String>,
}

impl HttpWorkloadProbe {
    /// Map a workload name → its liveness URL (e.g. the gateway's `/healthz`).
    ///
    /// # Errors
    /// [`ReconcileError`] if the HTTP client cannot be built.
    pub fn new(health_urls: HashMap<String, String>) -> Result<Self, ReconcileError> {
        let http = Client::builder()
            .timeout(Duration::from_secs(5))
            .build()
            .map_err(|e| ReconcileError(e.to_string()))?;
        Ok(Self { http, health_urls })
    }
}

impl WorkloadProbe for HttpWorkloadProbe {
    fn healthy(&self, workload: &Workload) -> impl Future<Output = bool> + Send {
        let url = self
            .health_urls
            .get(workload.metadata.name.as_str())
            .cloned();
        let http = self.http.clone();
        async move {
            match url {
                Some(url) => match http.get(url).send().await {
                    Ok(resp) => resp.status().is_success(),
                    Err(_) => false,
                },
                None => false,
            }
        }
    }
}

/// Brings gateway `Workload`s under management: reconciles health/readiness and
/// reflects their placements. Run it on a `"Workload/"` informer with a resync
/// heartbeat (so readiness is re-checked on a cadence).
pub struct WorkloadController<P: WorkloadProbe> {
    client: EstateClient,
    probe: Arc<P>,
}

// Hand-written so the controller clones without requiring `P: Clone`.
impl<P: WorkloadProbe> Clone for WorkloadController<P> {
    fn clone(&self) -> Self {
        Self {
            client: self.client.clone(),
            probe: Arc::clone(&self.probe),
        }
    }
}

impl<P: WorkloadProbe> WorkloadController<P> {
    #[must_use]
    pub fn new(client: EstateClient, probe: Arc<P>) -> Self {
        Self { client, probe }
    }

    /// The nodes this workload is bound to — the `Placement`s the scheduler
    /// (Phase S) has written for it, reflected into status.
    async fn placements(&self, workload: &str) -> Result<Vec<String>, ReconcileError> {
        let mut nodes = Vec::new();
        for object in self.client.list(Kind::Placement).await? {
            if let ResourceObject::Placement(placement) = &object
                && placement.spec.workload == workload
            {
                nodes.push(placement.spec.node.clone());
            }
        }
        nodes.sort();
        nodes.dedup();
        Ok(nodes)
    }

    async fn reconcile_workload(&self, name: &str) -> Result<Action, ReconcileError> {
        let Some(ResourceObject::Workload(workload)) =
            self.client.get(Kind::Workload, name).await?
        else {
            return Ok(Action::Done);
        };
        if workload.metadata.deletion_timestamp.is_some() {
            return Ok(Action::Done); // teardown is the GC's job (R2)
        }
        // X2 manages the gateway kind; the other workload kinds land with the
        // scheduler + node runtime (M3b).
        if workload.spec.workload_kind != WorkloadKind::Gateway {
            return Ok(Action::Done);
        }

        let placements = self.placements(name).await?;
        let healthy = self.probe.healthy(&workload).await;
        // Unplaced → Pending; placed + healthy → Ready with its replicas; placed
        // but unhealthy → Degraded, nothing ready.
        let (phase, ready_replicas) = if placements.is_empty() {
            (Phase::Pending, 0)
        } else if healthy {
            (Phase::Ready, workload.spec.replicas)
        } else {
            (Phase::Degraded, 0)
        };

        let desired = WorkloadStatus {
            phase,
            ready_replicas,
            placements,
        };
        if workload.status.as_ref() != Some(&desired) {
            let mut converged = workload;
            converged.status = Some(desired);
            self.client
                .update(ResourceObject::Workload(converged))
                .await?;
        }
        Ok(Action::Done)
    }
}

impl<P: WorkloadProbe + 'static> Reconciler for WorkloadController<P> {
    fn reconcile(&self, key: &str) -> impl Future<Output = Result<Action, ReconcileError>> + Send {
        let controller = self.clone();
        let key = key.to_owned();
        async move {
            let Some((Kind::Workload, name)) = parse_key(&key) else {
                return Ok(Action::Done);
            };
            controller.reconcile_workload(name).await
        }
    }
}
