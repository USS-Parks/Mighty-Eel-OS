//! The RevocationIntent fan-out: a declarative `RevocationIntent` becomes
//! a signed `fabric-revocation` snapshot on the channel every gateway replica
//! polls **and** on removable media for an air-gapped node — the bounded,
//! provable kill (doctrine I-9), effective on every replica and offline.
//!
//! Complements the front-door indexer (the in-process apiserver kill view):
//! this controller publishes the *signed snapshot* the data-path gateway's kill
//! switch (G9) reads and an air-gap node imports from media. `Token` and
//! `Subject` targets fan out here; tenant-wide kill is the deprovision +
//! front-door leg, and `Ring` darkness is the TrustRing leg. Level-triggered and
//! idempotent — the snapshot is a pure function of the current intents, so a
//! duplicate or dropped event cannot skew it.

use std::collections::BTreeSet;
use std::future::Future;
use std::path::PathBuf;
use std::sync::Arc;

use chrono::Utc;
use serde_json::json;

use fabric_crypto::Signer;
use fabric_crypto::providers::MlDsa87Verifier;
use fabric_revocation::{RevocationSnapshot, sign};
use wsf_bridge::OpenBaoAuth;

use aog_estate::{Kind, Phase, ResourceObject, RevocationTarget};

use crate::objects::EstateClient;
use crate::runtime::{Action, ReconcileError, Reconciler};

/// A stable id for the estate-wide revocation snapshot.
const SNAPSHOT_ID: &str = "loom-estate-revocation";
/// The filename an air-gapped node imports the signed snapshot from.
const MEDIA_FILE: &str = "estate-revocation.json";

/// Fans RevocationIntents out to a signed snapshot: online (the KV path every
/// gateway replica's kill switch polls) and, optionally, removable media.
#[derive(Clone)]
pub struct RevocationController {
    client: EstateClient,
    openbao: Arc<OpenBaoAuth>,
    signer: Arc<dyn Signer>,
    online_path: String,
    media_dir: Option<PathBuf>,
}

impl RevocationController {
    /// `online_path` is the KV path the gateway's kill switch reads
    /// (its `revocation_kv_path`); `signer` is the anchor the gateway verifies
    /// the snapshot against.
    #[must_use]
    pub fn new(
        client: EstateClient,
        openbao: Arc<OpenBaoAuth>,
        signer: Arc<dyn Signer>,
        online_path: impl Into<String>,
    ) -> Self {
        Self {
            client,
            openbao,
            signer,
            online_path: online_path.into(),
            media_dir: None,
        }
    }

    /// Also export the signed snapshot to `dir/estate-revocation.json`, for
    /// air-gap transport on removable media.
    #[must_use]
    pub fn with_media_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.media_dir = Some(dir.into());
        self
    }

    /// The (tokens, subjects) the current non-terminating intents revoke, sorted.
    async fn desired(&self) -> Result<(Vec<String>, Vec<String>), ReconcileError> {
        let mut tokens = BTreeSet::new();
        let mut subjects = BTreeSet::new();
        for object in self.client.list(Kind::RevocationIntent).await? {
            let ResourceObject::RevocationIntent(intent) = &object else {
                continue;
            };
            if intent.metadata.deletion_timestamp.is_some() {
                continue;
            }
            match &intent.spec.target {
                RevocationTarget::Token(id) => {
                    tokens.insert(id.clone());
                }
                RevocationTarget::Subject(hash) => {
                    subjects.insert(hash.clone());
                }
                // Tenant-wide is the deprovision / front-door leg; Ring is the TrustRing's.
                RevocationTarget::Tenant(_) | RevocationTarget::Ring(_) => {}
            }
        }
        Ok((tokens.into_iter().collect(), subjects.into_iter().collect()))
    }

    /// The snapshot currently published online, if any.
    async fn published(&self, vault: &str) -> Result<Option<RevocationSnapshot>, ReconcileError> {
        match self.openbao.get_kv_data(vault, &self.online_path).await {
            Ok(data) => match data.get("snapshot") {
                Some(value) => Ok(Some(
                    serde_json::from_value(value.clone())
                        .map_err(|e| ReconcileError(e.to_string()))?,
                )),
                None => Ok(None),
            },
            Err(wsf_bridge::OpenBaoError::NotFound(_)) => Ok(None),
            Err(e) => Err(ReconcileError(e.to_string())),
        }
    }

    async fn rebuild(&self) -> Result<Action, ReconcileError> {
        let (tokens, subjects) = self.desired().await?;
        let vault = self
            .openbao
            .login()
            .await
            .map_err(|e| ReconcileError(e.to_string()))?;

        // Idempotent: republish only when the revoked set drifts from what is
        // live (or the live snapshot no longer verifies under the anchor).
        let in_sync = self.published(&vault).await?.is_some_and(|s| {
            s.revoked_tokens == tokens
                && s.revoked_subjects == subjects
                && fabric_revocation::verify(&s, &MlDsa87Verifier, self.signer.public_key()).is_ok()
        });
        if !in_sync {
            let now = Utc::now();
            let mut snapshot = RevocationSnapshot::new(
                SNAPSHOT_ID,
                now.to_rfc3339(),
                (now + chrono::Duration::days(3650)).to_rfc3339(),
            );
            snapshot.revoked_tokens.clone_from(&tokens);
            snapshot.revoked_subjects.clone_from(&subjects);
            let signed =
                sign(snapshot, self.signer.as_ref()).map_err(|e| ReconcileError(e.to_string()))?;

            // Online: the KV path every gateway replica's kill switch polls.
            self.openbao
                .put_kv_data(&vault, &self.online_path, json!({ "snapshot": signed }))
                .await
                .map_err(|e| ReconcileError(e.to_string()))?;

            // Removable media: a signed artifact an air-gapped node imports and
            // verifies offline, with the public key alone.
            if let Some(dir) = &self.media_dir {
                tokio::fs::create_dir_all(dir)
                    .await
                    .map_err(|e| ReconcileError(e.to_string()))?;
                let bytes = serde_json::to_vec_pretty(&signed)
                    .map_err(|e| ReconcileError(e.to_string()))?;
                tokio::fs::write(dir.join(MEDIA_FILE), bytes)
                    .await
                    .map_err(|e| ReconcileError(e.to_string()))?;
            }
        }

        self.ack(&tokens, &subjects).await
    }

    /// Acknowledge each covered intent as propagated (skip the already-done).
    async fn ack(&self, tokens: &[String], subjects: &[String]) -> Result<Action, ReconcileError> {
        for object in self.client.list(Kind::RevocationIntent).await? {
            let ResourceObject::RevocationIntent(intent) = object else {
                continue;
            };
            let covered = match &intent.spec.target {
                RevocationTarget::Token(id) => tokens.contains(id),
                RevocationTarget::Subject(hash) => subjects.contains(hash),
                _ => false,
            };
            if !covered || intent.status.as_ref().is_some_and(|s| s.propagated) {
                continue;
            }
            let mut acked = intent;
            let status = acked.status.get_or_insert_with(Default::default);
            status.phase = Phase::Ready;
            status.propagated = true;
            status.replicas_denied = 1;
            self.client
                .update(ResourceObject::RevocationIntent(acked))
                .await?;
        }
        Ok(Action::Done)
    }
}

impl Reconciler for RevocationController {
    fn reconcile(&self, _key: &str) -> impl Future<Output = Result<Action, ReconcileError>> + Send {
        let controller = self.clone();
        async move { controller.rebuild().await }
    }
}
