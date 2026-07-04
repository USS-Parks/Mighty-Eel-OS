//! R3 — the Tenant controller: a declared `Tenant` becomes a live OpenBao
//! tenant record, and stays converged with it.
//!
//! Reconciles `Tenant` desired state against `kv/data/tenants/<id>` through
//! the M1 `wsf-tenants::TenantAdmin` (reused, not rebuilt): **provision** —
//! missing record → write it (compliance scopes, classification ceiling, a
//! freshly minted per-tenant subject-HMAC key) and mark the estate status
//! `Ready`; **rotate** — a record past its subject-HMAC rotation window
//! (spec days, or the 90-day default) gets a new key on the next wake-up
//! (drive with [`Controller::with_resync`](crate::Controller::with_resync) so
//! windows are checked on a heartbeat, not only on estate edits);
//! **deprovision** — a terminating tenant's record is deleted and a signed
//! revocation snapshot (revoking every control-plane token id enumerable from
//! the tenant's estate objects) is persisted to the path the Ring-3 caches
//! poll, before this controller's finalizer is released.
//!
//! Only a genuine not-found provisions; any other OpenBao failure retries
//! with backoff — never "assume missing and overwrite" (that would silently
//! rotate the tenant's HMAC key on a transient fault; doctrine I-4).

use std::collections::BTreeSet;
use std::future::Future;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use serde::Serialize;
use wsf_bridge::OpenBaoError;
use wsf_tenants::{TenantAdmin, TenantError, TenantRecord, TenantSpec as WireTenantSpec};

use aog_estate::{Kind, Phase, ResourceObject, TenantStatus};

use crate::objects::{EstateClient, parse_key};
use crate::runtime::{Action, ReconcileError, Reconciler};

/// The finalizer this controller owns on every `Tenant`: the estate object may
/// not vanish until its OpenBao record is deprovisioned and the revocation
/// snapshot is persisted.
pub const OPENBAO_FINALIZER: &str = "loom.aog/tenant-openbao";

/// Default subject-HMAC rotation window when the spec leaves it 0.
const DEFAULT_ROTATION_DAYS: i64 = 90;

/// The serde wire name of a fabric enum (`"hipaa"`, `"restricted"`, …) — the
/// vocabulary the OpenBao tenant record and the bridge share.
fn wire<T: Serialize>(value: &T) -> String {
    serde_json::to_value(value)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_default()
}

/// Tenant lifecycle controller. Run it on a `"Tenant/"` informer with a
/// resync heartbeat.
#[derive(Clone)]
pub struct TenantProvisioner {
    client: EstateClient,
    admin: Arc<TenantAdmin>,
}

impl TenantProvisioner {
    #[must_use]
    pub fn new(client: EstateClient, admin: Arc<TenantAdmin>) -> Self {
        Self { client, admin }
    }

    /// The KV data path a tenant's record lives at (reported in status).
    #[must_use]
    pub fn openbao_path(tenant: &str) -> String {
        format!("kv/data/tenants/{tenant}")
    }

    /// Every control-plane token id enumerable from the tenant's estate
    /// objects (the admission-minted scoped children) — what deprovision can
    /// concretely revoke in the snapshot. Tenant-wide coverage at the front
    /// door comes from the R2 `RevocationIntent`; R9 fans it estate-wide.
    async fn tenant_token_ids(&self, tenant: &str) -> Result<Vec<String>, ReconcileError> {
        let mut ids = BTreeSet::new();
        for kind in Kind::ALL {
            for object in self.client.list(kind).await? {
                let meta = object.metadata();
                let scoped = meta.tenant.as_deref() == Some(tenant)
                    || (kind == Kind::Tenant && object.name() == tenant);
                if scoped
                    && let Some(token_ref) = &meta.token_ref
                    && !token_ref.token_id.is_empty()
                {
                    ids.insert(token_ref.token_id.clone());
                }
            }
        }
        Ok(ids.into_iter().collect())
    }

    fn rotation_due(spec_days: u32, record: &TenantRecord, now: DateTime<Utc>) -> bool {
        let window = if spec_days == 0 {
            DEFAULT_ROTATION_DAYS
        } else {
            i64::from(spec_days)
        };
        match DateTime::parse_from_rfc3339(&record.hmac_rotated_at) {
            Ok(rotated) => {
                now.signed_duration_since(rotated.with_timezone(&Utc))
                    > chrono::Duration::days(window)
            }
            // An unreadable rotation timestamp resolves toward rotation.
            Err(_) => true,
        }
    }

    async fn reconcile_tenant(&self, name: &str) -> Result<Action, ReconcileError> {
        let Some(ResourceObject::Tenant(tenant)) = self.client.get(Kind::Tenant, name).await?
        else {
            return Ok(Action::Done); // gone (or not a tenant) — nothing owed
        };

        // Terminating: deprovision OpenBao, then release our finalizer.
        if tenant.metadata.deletion_timestamp.is_some() {
            if !tenant
                .metadata
                .finalizers
                .iter()
                .any(|f| f == OPENBAO_FINALIZER)
            {
                return Ok(Action::Done); // our leg already released
            }
            let revoke_tokens = self.tenant_token_ids(name).await?;
            self.admin
                .deprovision(name, revoke_tokens, Vec::new(), Utc::now())
                .await
                .map_err(|e| ReconcileError(e.to_string()))?;
            let mut released = tenant;
            released
                .metadata
                .finalizers
                .retain(|f| f != OPENBAO_FINALIZER);
            self.client.update(ResourceObject::Tenant(released)).await?;
            return Ok(Action::Done);
        }

        // Live: guard with our finalizer first (the update wakes us again).
        if !tenant
            .metadata
            .finalizers
            .iter()
            .any(|f| f == OPENBAO_FINALIZER)
        {
            let mut guarded = tenant;
            guarded
                .metadata
                .finalizers
                .push(OPENBAO_FINALIZER.to_owned());
            self.client.update(ResourceObject::Tenant(guarded)).await?;
            return Ok(Action::Done);
        }

        // Converge the OpenBao record.
        match self.admin.get(name).await {
            // Only a genuine not-found provisions.
            Err(TenantError::OpenBao(
                OpenBaoError::NotFound(_) | OpenBaoError::TenantNotFound(_),
            )) => {
                let spec = WireTenantSpec {
                    tenant_id: name.to_owned(),
                    display_name: tenant.spec.display_name.clone(),
                    compliance_scopes: tenant.spec.compliance_scopes.iter().map(wire).collect(),
                    default_allowed_routes: Vec::new(),
                    max_data_classification: wire(&tenant.spec.classification_ceiling),
                };
                self.admin
                    .provision(&spec, Utc::now())
                    .await
                    .map_err(|e| ReconcileError(e.to_string()))?;
            }
            // Any other failure is transient: retry with backoff, never
            // overwrite (a blind re-provision would silently rotate the key).
            Err(e) => return Err(ReconcileError(e.to_string())),
            Ok(record) => {
                if Self::rotation_due(tenant.spec.subject_hmac_rotation_days, &record, Utc::now()) {
                    self.admin
                        .rotate_subject_hmac(name, Utc::now())
                        .await
                        .map_err(|e| ReconcileError(e.to_string()))?;
                }
            }
        }

        // Reflect convergence in status (write only on change).
        let desired = TenantStatus {
            phase: Phase::Ready,
            openbao_path: Some(Self::openbao_path(name)),
            issued_tokens: tenant.status.as_ref().map_or(0, |s| s.issued_tokens),
        };
        if tenant.status.as_ref() != Some(&desired) {
            let mut converged = tenant;
            converged.status = Some(desired);
            self.client
                .update(ResourceObject::Tenant(converged))
                .await?;
        }
        Ok(Action::Done)
    }
}

impl Reconciler for TenantProvisioner {
    fn reconcile(&self, key: &str) -> impl Future<Output = Result<Action, ReconcileError>> + Send {
        let provisioner = self.clone();
        let key = key.to_owned();
        async move {
            let Some((Kind::Tenant, name)) = parse_key(&key) else {
                return Ok(Action::Done);
            };
            provisioner.reconcile_tenant(name).await
        }
    }
}
