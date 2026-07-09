//! Tenant teardown: deprovisioning that propagates revocation.
//!
//! A live `Tenant` is guarded with the teardown finalizer, so deleting it
//! starts a two-phase teardown instead of dropping state on the floor. While
//! the tenant is terminating, this reconciler:
//!
//! 1. declares the kill — a `RevocationIntent` targeting the tenant, so every
//!    token of that tenant dies at the front door the moment the indexing
//!    controller folds it in (and estate-wide via signed snapshots);
//! 2. waits for the garbage collector to finish sweeping the tenant's scoped
//!    objects (no dangling capability outlives its tenant);
//! 3. releases the finalizer — which completes the two-phase delete.
//!
//! The revocation intent deliberately has **no** tenant scoping of its own:
//! the kill record must survive the tenant it kills.

use std::future::Future;
use std::time::Duration;

use aog_estate::{Kind, Resource, ResourceObject, RevocationIntentSpec, RevocationTarget, Tenant};

use crate::objects::{EstateClient, parse_key};
use crate::runtime::{Action, ReconcileError, Reconciler};

/// The finalizer this reconciler owns on every `Tenant`.
pub const TENANT_FINALIZER: &str = "loom.aog/tenant-teardown";

/// Tenant lifecycle reconciler. Run it on a `"Tenant/"` informer.
#[derive(Clone)]
pub struct TenantTeardown {
    client: EstateClient,
    /// How long to wait before re-checking that the GC has finished sweeping.
    requeue: Duration,
}

impl TenantTeardown {
    #[must_use]
    pub fn new(client: EstateClient) -> Self {
        Self {
            client,
            requeue: Duration::from_millis(200),
        }
    }

    /// Adjust the children-gone re-check interval (tests use ~1ms).
    #[must_use]
    pub fn with_requeue(mut self, requeue: Duration) -> Self {
        self.requeue = requeue;
        self
    }

    /// The name of the revocation intent teardown declares for `tenant`.
    #[must_use]
    pub fn intent_name(tenant: &str) -> String {
        format!("revoke-tenant-{tenant}")
    }

    /// Does any object still carry this tenant's scope?
    async fn children_remain(&self, tenant: &str) -> Result<bool, ReconcileError> {
        for kind in Kind::ALL {
            let scoped = self
                .client
                .list(kind)
                .await?
                .iter()
                .any(|o| o.metadata().tenant.as_deref() == Some(tenant));
            if scoped {
                return Ok(true);
            }
        }
        Ok(false)
    }

    async fn reconcile_tenant(&self, name: &str) -> Result<Action, ReconcileError> {
        let Some(object) = self.client.get(Kind::Tenant, name).await? else {
            return Ok(Action::Done); // already gone
        };
        let ResourceObject::Tenant(tenant) = object else {
            return Ok(Action::Done);
        };

        if tenant.metadata.deletion_timestamp.is_none() {
            return self.ensure_finalizer(tenant).await;
        }

        // 1. Declare the kill: the intent that revokes every token of this
        //    tenant. Creating it is idempotent (already-exists = converged).
        let mut intent = Resource::new(
            Self::intent_name(name),
            RevocationIntentSpec {
                target: RevocationTarget::Tenant(name.to_owned()),
                reason: format!("tenant {name} deprovisioned"),
            },
        );
        // The kill record survives the tenant it kills: no tenant scope.
        intent.metadata.tenant = None;
        self.client
            .ensure_created(ResourceObject::RevocationIntent(intent))
            .await?;

        // 2. Wait for the GC sweep: nothing scoped to this tenant may survive.
        if self.children_remain(name).await? {
            return Ok(Action::RequeueAfter(self.requeue));
        }

        // 3. Release the finalizer — completing the two-phase delete.
        let mut done = tenant;
        done.metadata.finalizers.retain(|f| f != TENANT_FINALIZER);
        self.client.update(ResourceObject::Tenant(done)).await?;
        Ok(Action::Done)
    }

    /// Guard a live tenant with the teardown finalizer.
    async fn ensure_finalizer(&self, tenant: Tenant) -> Result<Action, ReconcileError> {
        if tenant
            .metadata
            .finalizers
            .iter()
            .any(|f| f == TENANT_FINALIZER)
        {
            return Ok(Action::Done);
        }
        let mut guarded = tenant;
        guarded
            .metadata
            .finalizers
            .push(TENANT_FINALIZER.to_owned());
        self.client.update(ResourceObject::Tenant(guarded)).await?;
        Ok(Action::Done)
    }
}

impl Reconciler for TenantTeardown {
    fn reconcile(&self, key: &str) -> impl Future<Output = Result<Action, ReconcileError>> + Send {
        let teardown = self.clone();
        let key = key.to_owned();
        async move {
            let Some((Kind::Tenant, name)) = parse_key(&key) else {
                return Ok(Action::Done);
            };
            teardown.reconcile_tenant(name).await
        }
    }
}
