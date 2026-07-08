//! R2 — the garbage collector: cascading delete + orphan collection.
//!
//! Watches the whole estate. Two duties, both level-triggered off a single
//! key wake-up:
//!
//! * **Owner side** — when an object is terminating or gone, its dependents
//!   are swept: every object holding an [`OwnerRef`](aog_estate::OwnerRef) to
//!   it (by kind+name, and by uid when recorded), and — when the owner is a
//!   `Tenant` — every object scoped to it via `metadata.tenant`.
//! * **Child side** — a live object whose owner is missing, terminating, or a
//!   recreated namesake (uid mismatch) is an orphan and is deleted.
//!
//! Both directions run on every wake-up, so a dropped event on one side is
//! recovered by the other (and by the informer's re-list). All deletes go
//! through admission — receipted teardown, never a silent reap (I-5).
//! Deliberately out of scope here: `Tenant` objects themselves — their
//! two-phase teardown (revocation intent, children-gone wait, finalizer
//! release) is the tenant-teardown reconciler's job.

use std::future::Future;

use aog_estate::{Kind, ResourceObject};

use crate::objects::{EstateClient, is_terminating, parse_key};
use crate::runtime::{Action, ReconcileError, Reconciler};

/// The garbage collector. Run it on a whole-estate informer (prefix `""`).
#[derive(Clone)]
pub struct GarbageCollector {
    client: EstateClient,
}

impl GarbageCollector {
    #[must_use]
    pub fn new(client: EstateClient) -> Self {
        Self { client }
    }

    /// Delete every object that depends on owner `(kind, name)` — by owner
    /// reference, and by tenant scope when the owner is a Tenant. `uid` is the
    /// owner's current uid when it still exists (so a dependent recorded
    /// against a *different* incarnation is also swept).
    async fn sweep_dependents(&self, kind: Kind, name: &str) -> Result<(), ReconcileError> {
        for dependent_kind in Kind::ALL {
            for object in self.client.list(dependent_kind).await? {
                if Self::depends_on(&object, kind, name) {
                    self.client.delete(dependent_kind, object.name()).await?;
                }
            }
        }
        Ok(())
    }

    /// Does `object` depend on the owner `(kind, name)`?
    fn depends_on(object: &ResourceObject, kind: Kind, name: &str) -> bool {
        let meta = object.metadata();
        if kind == Kind::Tenant && meta.tenant.as_deref() == Some(name) {
            return true;
        }
        meta.owner_refs
            .iter()
            .any(|r| r.kind == kind && r.name == name)
    }

    /// Is this live object an orphan — any owner missing, terminating, or a
    /// different incarnation (uid mismatch)?
    async fn is_orphan(&self, object: &ResourceObject) -> Result<bool, ReconcileError> {
        let meta = object.metadata();
        if let Some(tenant) = &meta.tenant {
            match self.client.get(Kind::Tenant, tenant).await? {
                Some(owner) if !is_terminating(&owner) => {}
                _ => return Ok(true),
            }
        }
        for r in &meta.owner_refs {
            match self.client.get(r.kind, &r.name).await? {
                Some(owner)
                    if !is_terminating(&owner)
                        && (r.uid.is_empty() || owner.metadata().uid == r.uid) => {}
                _ => return Ok(true),
            }
        }
        Ok(false)
    }
}

impl Reconciler for GarbageCollector {
    fn reconcile(&self, key: &str) -> impl Future<Output = Result<Action, ReconcileError>> + Send {
        let gc = self.clone();
        let key = key.to_owned();
        async move {
            let Some((kind, name)) = parse_key(&key) else {
                return Ok(Action::Done);
            };
            match gc.client.get(kind, name).await? {
                // Gone or terminating: sweep everything that depended on it.
                None => {
                    gc.sweep_dependents(kind, name).await?;
                    Ok(Action::Done)
                }
                Some(object) if is_terminating(&object) => {
                    gc.sweep_dependents(kind, name).await?;
                    Ok(Action::Done)
                }
                // Live: collect it if its own owners are gone. Tenants have no
                // owners and tear down via their own reconciler.
                Some(object) => {
                    if kind != Kind::Tenant && gc.is_orphan(&object).await? {
                        gc.client.delete(kind, name).await?;
                    }
                    Ok(Action::Done)
                }
            }
        }
    }
}
