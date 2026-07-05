//! S7 — the scheduler controller: it turns a `Workload`'s desired replicas into
//! attested `Placement`s. For each replica still unplaced it runs the Phase-S
//! [`attested_scheduler`] over the estate's `Node`s (spreading replicas via the
//! placements already made), mints a runtime trust token scoped to the
//! workload's `Capability`, persists that token to OpenBao so the node can fetch
//! it, and creates the `Placement` through the admission choke point — which
//! validates, seals, and **receipts** the binding (K9), so no separate receipt
//! step is needed here. A replica with no attestation-satisfying node stays
//! Pending; it is never force-placed.
//!
//! This controller *mints* placements; the X2 `WorkloadController` reflects them
//! into `Workload` status. One replica per node (a placement is keyed by
//! `<workload>-<node>`); packing several replicas onto one node is Phase-O work.

use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use serde_json::json;

use fabric_contracts::{Attenuation, Budget, RevocationStatus, Signature, TrustToken};
use fabric_crypto::Signer;
use wsf_bridge::OpenBaoAuth;

use aog_estate::{
    CapabilitySpec, Kind, OwnerRef, Placement, PlacementSpec, Resource, ResourceObject, Workload,
};
use aog_scheduler::{NodeSnapshot, ScheduleRequest, Scheduler, attested_scheduler};

use crate::objects::{EstateClient, parse_key};
use crate::runtime::{Action, ReconcileError, Reconciler};

/// Runtime-token lifetime when the workload declares no `Capability` (still
/// non-zero: zero standing privilege, I-1).
const DEFAULT_TTL_SECONDS: u64 = 3600;
/// How long to wait before retrying a workload with replicas still unplaced.
const REQUEUE: Duration = Duration::from_secs(15);

/// Attested placement controller. Run it on a `"Workload/"` informer.
#[derive(Clone)]
pub struct SchedulerController {
    client: EstateClient,
    openbao: Arc<OpenBaoAuth>,
    signer: Arc<dyn Signer>,
    token_prefix: String,
    scheduler: Arc<Scheduler>,
}

impl SchedulerController {
    /// `token_prefix` is the OpenBao KV path runtime tokens are written under
    /// (e.g. `kv/data/aog/runtime-tokens`); `signer` is the trust anchor a node
    /// verifies a fetched runtime token against.
    #[must_use]
    pub fn new(
        client: EstateClient,
        openbao: Arc<OpenBaoAuth>,
        token_prefix: impl Into<String>,
        signer: Arc<dyn Signer>,
    ) -> Self {
        Self {
            client,
            openbao,
            signer,
            token_prefix: token_prefix.into(),
            scheduler: Arc::new(attested_scheduler()),
        }
    }

    /// The KV path the runtime token for `placement` is stored at.
    fn token_path(&self, placement: &str) -> String {
        format!("{}/{placement}", self.token_prefix)
    }

    /// The nodes already hosting a replica of `workload` (from its `Placement`s).
    async fn placed_nodes(&self, workload: &str) -> Result<Vec<String>, ReconcileError> {
        let mut nodes = Vec::new();
        for object in self.client.list(Kind::Placement).await? {
            if let ResourceObject::Placement(placement) = object
                && placement.spec.workload == workload
            {
                nodes.push(placement.spec.node);
            }
        }
        Ok(nodes)
    }

    /// Project every estate `Node` into a scheduler snapshot.
    async fn node_snapshots(&self) -> Result<Vec<NodeSnapshot>, ReconcileError> {
        let mut snapshots = Vec::new();
        for object in self.client.list(Kind::Node).await? {
            if let ResourceObject::Node(node) = object {
                snapshots.push(NodeSnapshot::from_node(&node));
            }
        }
        Ok(snapshots)
    }

    /// The scope the runtime token is minted from — the workload's named
    /// `Capability`, when it exists and is live. A missing or terminating
    /// capability yields `None`: the token is then minimal (less privilege, the
    /// fail-closed direction, I-4), never broader than declared.
    async fn resolve_capability(
        &self,
        workload: &Workload,
    ) -> Result<Option<CapabilitySpec>, ReconcileError> {
        let Some(cap_name) = &workload.spec.capability else {
            return Ok(None);
        };
        match self.client.get(Kind::Capability, cap_name).await? {
            Some(ResourceObject::Capability(cap)) if cap.metadata.deletion_timestamp.is_none() => {
                Ok(Some(cap.spec))
            }
            _ => Ok(None),
        }
    }

    /// Mint a runtime token scoped to `cap` (or minimal, bounded by the
    /// workload's classification ceiling, when it declares none) and sign it.
    fn mint_runtime_token(
        &self,
        id: &str,
        workload: &Workload,
        cap: Option<&CapabilitySpec>,
    ) -> Result<TrustToken, ReconcileError> {
        let (allowed_routes, allowed_models, max_class, budget, caveats, ttl_seconds) = match cap {
            Some(c) => (
                c.allowed_routes.clone(),
                c.allowed_models.clone(),
                c.max_classification,
                c.budget.clone(),
                c.caveats.clone(),
                c.ttl_seconds,
            ),
            None => (
                Vec::new(),
                Vec::new(),
                workload.spec.classification_ceiling,
                Budget::default(),
                Vec::new(),
                DEFAULT_TTL_SECONDS,
            ),
        };
        let now = Utc::now();
        let ttl = chrono::Duration::seconds(i64::try_from(ttl_seconds).unwrap_or(i64::MAX));
        let token = TrustToken {
            token_id: id.to_owned(),
            issued_at: now.to_rfc3339(),
            expires_at: (now + ttl).to_rfc3339(),
            issuer: "aog-scheduler".to_owned(),
            trust_bundle_version: "loom".to_owned(),
            tenant_id: workload.metadata.tenant.clone().unwrap_or_default(),
            subject_id: None,
            subject_hash: id.to_owned(),
            service_identity: None,
            identity_id: None,
            roles: vec![],
            compliance_scopes: vec![],
            allowed_routes,
            allowed_models,
            max_data_classification: max_class,
            country: None,
            person_type: None,
            offline_mode: false,
            revocation_status: RevocationStatus::Valid,
            budget: Some(budget),
            attenuation: Attenuation {
                parent_id: None,
                caveats,
            },
            signature: Signature {
                alg: String::new(),
                key_id: String::new(),
                value: String::new(),
            },
        };
        fabric_token::issue(token, self.signer.as_ref()).map_err(|e| ReconcileError(e.to_string()))
    }

    /// Build the `Placement` resource for a binding, owned by its workload so the
    /// GC reclaims it when the workload is deleted (R2/W9).
    fn build_placement(
        workload: &Workload,
        placement_name: &str,
        node: &str,
        token_id: &str,
    ) -> Placement {
        let mut placement = Resource::new(
            placement_name.to_owned(),
            PlacementSpec {
                workload: workload.metadata.name.clone(),
                node: node.to_owned(),
                token_id: token_id.to_owned(),
            },
        );
        placement.metadata.owner_refs.push(OwnerRef {
            kind: Kind::Workload,
            name: workload.metadata.name.clone(),
            uid: workload.metadata.uid.clone(),
        });
        placement
            .metadata
            .tenant
            .clone_from(&workload.metadata.tenant);
        placement
    }

    async fn reconcile_workload(&self, name: &str) -> Result<Action, ReconcileError> {
        let Some(ResourceObject::Workload(workload)) =
            self.client.get(Kind::Workload, name).await?
        else {
            return Ok(Action::Done);
        };
        // A terminating workload's placements are reclaimed by the GC (owner
        // refs); the scheduler makes no new bindings for it.
        if workload.metadata.deletion_timestamp.is_some() {
            return Ok(Action::Done);
        }

        let desired = usize::try_from(workload.spec.replicas).unwrap_or(usize::MAX);
        let mut placed = self.placed_nodes(name).await?;
        if placed.len() >= desired {
            return Ok(Action::Done);
        }

        let snapshots = self.node_snapshots().await?;
        let cap = self.resolve_capability(&workload).await?;
        let vault = self
            .openbao
            .login()
            .await
            .map_err(|e| ReconcileError(e.to_string()))?;

        while placed.len() < desired {
            let mut request = ScheduleRequest::from_workload(&workload);
            request.already_placed_on = placed.clone();
            let decision = self.scheduler.schedule(&request, &snapshots);
            let Some(node) = decision.scheduled_node() else {
                break; // no attestation-satisfying node — the rest stay Pending
            };
            if placed.iter().any(|n| n.as_str() == node) {
                break; // best node already hosts a replica; no distinct node free
            }
            let node = node.to_owned();
            let placement_name = format!("{name}-{node}");
            let token_id = format!("rt:{placement_name}");
            let token = self.mint_runtime_token(&token_id, &workload, cap.as_ref())?;
            self.openbao
                .put_kv_data(
                    &vault,
                    &self.token_path(&placement_name),
                    json!({ "token": token }),
                )
                .await
                .map_err(|e| ReconcileError(e.to_string()))?;
            let placement = Self::build_placement(&workload, &placement_name, &node, &token_id);
            self.client
                .ensure_created(ResourceObject::Placement(placement))
                .await?;
            placed.push(node);
        }

        if placed.len() < desired {
            Ok(Action::RequeueAfter(REQUEUE))
        } else {
            Ok(Action::Done)
        }
    }
}

impl Reconciler for SchedulerController {
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
