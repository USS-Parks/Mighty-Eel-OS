//! R2 — the revocation indexer: `RevocationIntent` objects → the front door's
//! live kill view.
//!
//! Every reconcile rebuilds the [`RevocationView`] from the **full** intent
//! list (level-triggered: the view is a pure function of current desired
//! state, so duplicate or dropped intent events cannot skew it), swaps it into
//! the authenticator the K6 front door consults on every request, and then
//! marks each newly-enforced intent `Ready`/propagated through admission — a
//! receipted acknowledgment that the kill is live on this replica.
//!
//! `Ring` targets are indexed nowhere yet: ring darkness is the R4 TrustRing
//! controller's mechanism. Their intents stay `Pending` — an unenforced kill
//! is never reported as propagated (doctrine: enforced, not asserted).

use std::future::Future;
use std::sync::{Arc, RwLock};

use aog_apiserver::auth::RevocationView;
use aog_estate::{Kind, ResourceObject, RevocationTarget};

use crate::objects::EstateClient;
use crate::runtime::{Action, ReconcileError, Reconciler};

/// Folds revocation intents into the front door's live kill view.
#[derive(Clone)]
pub struct RevocationIndexer {
    client: EstateClient,
    view: Arc<RwLock<RevocationView>>,
}

impl RevocationIndexer {
    /// `view` is the authenticator's live-revocation handle
    /// (`Authenticator::live_revocation`).
    #[must_use]
    pub fn new(client: EstateClient, view: Arc<RwLock<RevocationView>>) -> Self {
        Self { client, view }
    }

    async fn rebuild(&self) -> Result<Action, ReconcileError> {
        let intents = self.client.list(Kind::RevocationIntent).await?;

        // 1. Rebuild the view from current desired state and swap it in. The
        //    kill takes effect at the front door before any status is written.
        let mut view = RevocationView::default();
        let mut enforced = Vec::new();
        for object in &intents {
            let ResourceObject::RevocationIntent(intent) = object else {
                continue;
            };
            match &intent.spec.target {
                RevocationTarget::Token(id) => {
                    view.tokens.insert(id.clone());
                    enforced.push(intent);
                }
                RevocationTarget::Subject(hash) => {
                    view.subjects.insert(hash.clone());
                    enforced.push(intent);
                }
                RevocationTarget::Tenant(id) => {
                    view.tenants.insert(id.clone());
                    enforced.push(intent);
                }
                // Ring darkness is enforced by the TrustRing controller (R4);
                // until then a ring intent is honestly not propagated.
                RevocationTarget::Ring(_) => {}
            }
        }
        *self
            .view
            .write()
            .map_err(|_| ReconcileError("revocation view lock poisoned".to_owned()))? = view;

        // 2. Acknowledge: mark each newly-enforced intent Ready/propagated —
        //    an admitted, receipted status write (skip the ones already done).
        for intent in enforced {
            if intent.status.as_ref().is_some_and(|s| s.propagated) {
                continue;
            }
            let mut acked = intent.clone();
            let status = acked.status.get_or_insert_with(Default::default);
            status.phase = aog_estate::Phase::Ready;
            status.propagated = true;
            status.replicas_denied = 1; // this replica; R9 counts the estate
            self.client
                .update(ResourceObject::RevocationIntent(acked))
                .await?;
        }
        Ok(Action::Done)
    }
}

impl Reconciler for RevocationIndexer {
    fn reconcile(&self, _key: &str) -> impl Future<Output = Result<Action, ReconcileError>> + Send {
        let indexer = self.clone();
        async move { indexer.rebuild().await }
    }
}
