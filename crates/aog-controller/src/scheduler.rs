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
//! into `Workload` status. O1 makes it the full replica-set manager: placements
//! are replica-indexed (`<workload>-r<ordinal>`, [`crate::deploy`]), so a node
//! may host more than one replica of one workload (packing), and scale-down is a
//! precise drop of the ordinals at or beyond the declared `replicas` — each
//! dropped replica's runtime token revoked from OpenBao as its `Placement` goes.

use std::collections::BTreeMap;
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

use crate::deploy::{placement_name, plan_replicas, replica_index};
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

    /// Every live `Placement` for `workload`, matched by `spec.workload` (the
    /// authoritative link, independent of the placement's name).
    async fn existing_placements(&self, workload: &str) -> Result<Vec<Placement>, ReconcileError> {
        let mut out = Vec::new();
        for object in self.client.list(Kind::Placement).await? {
            if let ResourceObject::Placement(placement) = object
                && placement.spec.workload == workload
            {
                out.push(placement);
            }
        }
        Ok(out)
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

        // Index the live placements by their replica ordinal (parsed from the
        // name) so the planner can decide creates, deletes, and shortfall.
        let mut by_ordinal: BTreeMap<usize, Placement> = BTreeMap::new();
        for placement in self.existing_placements(name).await? {
            if let Some(ordinal) = replica_index(name, &placement.metadata.name) {
                by_ordinal.insert(ordinal, placement);
            }
        }
        let existing_nodes: BTreeMap<usize, String> = by_ordinal
            .iter()
            .map(|(ordinal, placement)| (*ordinal, placement.spec.node.clone()))
            .collect();

        let snapshots = self.node_snapshots().await?;
        let request = ScheduleRequest::from_workload(&workload);
        let plan = plan_replicas(
            desired,
            &existing_nodes,
            &self.scheduler,
            &request,
            &snapshots,
        );

        // Converged — nothing to add or remove. A lingering shortfall (no free
        // node) still requeues so a later node join heals it.
        if !plan.mutates() {
            return if plan.short > 0 {
                Ok(Action::RequeueAfter(REQUEUE))
            } else {
                Ok(Action::Done)
            };
        }

        let cap = self.resolve_capability(&workload).await?;
        let vault = self
            .openbao
            .login()
            .await
            .map_err(|e| ReconcileError(e.to_string()))?;

        // Scale down first: free capacity before packing. Each dropped replica's
        // runtime token is deleted from OpenBao (revoked) and its `Placement`
        // removed; the node runtime drains the replica (N9).
        for ordinal in &plan.delete {
            if let Some(placement) = by_ordinal.get(ordinal) {
                let pname = placement.metadata.name.clone();
                // An already-absent token is convergence, not error.
                let _ = self
                    .openbao
                    .delete_kv(&vault, &self.token_path(&pname))
                    .await;
                self.client.delete(Kind::Placement, &pname).await?;
            }
        }

        // Scale up: mint a scoped runtime token per new ordinal, persist it to
        // OpenBao for the node to fetch, and create the attested `Placement`
        // through admission (which receipts the binding, K9).
        for (ordinal, node) in &plan.create {
            let pname = placement_name(name, *ordinal);
            let token_id = format!("rt:{pname}");
            let token = self.mint_runtime_token(&token_id, &workload, cap.as_ref())?;
            self.openbao
                .put_kv_data(&vault, &self.token_path(&pname), json!({ "token": token }))
                .await
                .map_err(|e| ReconcileError(e.to_string()))?;
            let placement = Self::build_placement(&workload, &pname, node, &token_id);
            self.client
                .ensure_created(ResourceObject::Placement(placement))
                .await?;
        }

        if plan.short > 0 {
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
