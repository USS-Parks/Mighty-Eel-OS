//! The admission choke point (K5) — the **only** path that writes desired state.
//!
//! Type invariant (the K5 gate, "no write reaches `aog-store` bypassing
//! admission, enforced by type"): [`Admission`] privately owns the sole writable
//! `RaftNode` handle in this crate, and [`Admission::admit`] is the only method
//! that reaches [`RaftNode::write`]. A CRUD handler is handed an `Admission` and
//! a read-only [`crate::reader::StoreReader`]; neither exposes the raw node, so
//! no handler can construct a store write that skips the chain.
//!
//! Chain order mirrors addendum A1.7:
//!   1. authenticate  (K6 — the front-door `crate::auth` middleware hands `admit` an already-verified Principal)
//!   2. validate      (structural, fail-closed + K7 policy deny-wins over HIPAA/ITAR/OCAP)
//!   3. mutate        (metadata stamp + K8 envelope-seal flagged fields + child-token attenuation)
//!   4. commit        (the sole `aog-store` write, guarded by a CAS precondition)
//!   5. receipt       (K9 — hash-chained `fabric-proof` receipt to `wsf-ledger`)
//!
//! Stages 1–4 do real work today; only stage 5 (receipt, K9) remains a seam. The
//! choke point is complete — every mutation traverses this one method.

use std::sync::Arc;

use aog_estate::{Kind, ResourceObject, TokenRef};
use aog_store::raft::RaftNode;
use aog_store::raft::types::RaftResponse;
use aog_store::{Op, Precondition, Revision};
use fabric_contracts::TrustToken;

use crate::codec::{decode, encode, store_key};
use crate::error::ApiError;
use crate::policy::AdmissionPolicy;
use crate::seal::Sealer;

/// The mutation a request asks admission to perform.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verb {
    Create,
    Update,
    Delete,
}

/// The authenticated caller, produced by the front-door authenticator (K6) from
/// a verified WSF trust token and carried through the chain so mutate/receipt can
/// stamp provenance and K8 can attenuate a child from the parent token.
#[derive(Debug, Clone)]
pub struct Principal {
    /// The token subject (`subject_hash`), or `system:apiserver`.
    pub subject: String,
    /// The tenant the token belongs to, when authenticated.
    pub tenant: Option<String>,
    /// The authorizing capability reference stamped onto mutated objects.
    pub token_ref: Option<TokenRef>,
    /// The verified trust token — carried for downstream stages (K8 attenuation).
    pub token: Option<TrustToken>,
}

impl Principal {
    /// A verified caller from an authenticated WSF trust token (K6).
    #[must_use]
    pub fn authenticated(token: TrustToken) -> Self {
        Self {
            subject: token.subject_hash.clone(),
            tenant: Some(token.tenant_id.clone()),
            token_ref: Some(TokenRef {
                token_id: token.token_id.clone(),
            }),
            token: Some(token),
        }
    }

    /// The system principal — for internal callers with no external request
    /// (later-phase controllers). Never minted from an inbound request.
    #[must_use]
    pub fn system() -> Self {
        Self {
            subject: "system:apiserver".to_owned(),
            tenant: None,
            token_ref: None,
            token: None,
        }
    }
}

/// One admission request: a verb against a named object of a kind.
pub struct AdmissionRequest {
    pub verb: Verb,
    pub kind: Kind,
    pub name: String,
    /// The desired object for create/update; `None` for delete.
    pub object: Option<ResourceObject>,
}

/// The result of an admitted mutation.
pub struct AdmissionOutcome {
    /// The stored object (metadata stamped, `resource_version` set); `None` on delete.
    pub object: Option<ResourceObject>,
    /// The store revision the mutation committed at.
    pub revision: Revision,
}

/// The sole writer to `aog-store`. Its `raft` handle is private; see the module
/// docs for the type invariant this enforces.
pub struct Admission {
    raft: Arc<RaftNode>,
    policy: AdmissionPolicy,
    sealer: Sealer,
}

impl Admission {
    #[must_use]
    pub fn new(raft: Arc<RaftNode>, sealer: Sealer) -> Self {
        Self {
            raft,
            policy: AdmissionPolicy::baseline(),
            sealer,
        }
    }

    /// Run the admission chain and, only if every stage passes, commit exactly
    /// one desired-state mutation. This is the one method in the crate that
    /// writes the store.
    ///
    /// # Errors
    /// The first stage to refuse: [`ApiError::Invalid`] (structural) or
    /// [`ApiError::Forbidden`] (K7 policy); [`ApiError::NotFound`] /
    /// [`ApiError::Conflict`] at commit; or [`ApiError::Store`] on backend failure.
    pub async fn admit(
        &self,
        req: AdmissionRequest,
        principal: &Principal,
    ) -> Result<AdmissionOutcome, ApiError> {
        // Stage 1 (authenticate) is the front-door middleware (`crate::auth`):
        // `principal` is already a verified token by the time admission runs (the
        // K6 gate). The chain here is validate -> mutate -> commit -> receipt.
        self.validate(&req, principal)?;
        let staged = self.mutate(&req, principal).await?;
        let outcome = self.commit(&req, staged).await?;
        self.receipt(&req, principal, &outcome);
        Ok(outcome)
    }

    // 2. validate — structural (fail-closed) + K7 policy (deny-wins over regimes).
    fn validate(&self, req: &AdmissionRequest, principal: &Principal) -> Result<(), ApiError> {
        if let Some(object) = &req.object {
            object.validate()?;
            self.policy.evaluate(object, principal)?;
        }
        Ok(())
    }

    // 3. mutate — stamp metadata, then finish_mutation attenuates + seals (K8).
    async fn mutate(
        &self,
        req: &AdmissionRequest,
        principal: &Principal,
    ) -> Result<Staged, ApiError> {
        let key = store_key(req.kind, &req.name);
        match req.verb {
            Verb::Create => {
                let mut object = req
                    .object
                    .clone()
                    .ok_or_else(|| ApiError::BadBody("create requires a body".to_owned()))?;
                stamp_create(&mut object, principal);
                self.finish_mutation(&mut object, principal)?;
                let value = encode(&object)?;
                Ok(Staged {
                    op: Op::Put {
                        key,
                        value,
                        expected: Precondition::Absent,
                    },
                    object: Some(object),
                })
            }
            Verb::Update => {
                let current = self.load(&key).await?.ok_or_else(|| ApiError::NotFound {
                    kind: req.kind.to_string(),
                    name: req.name.clone(),
                })?;
                let mut object = req
                    .object
                    .clone()
                    .ok_or_else(|| ApiError::BadBody("update requires a body".to_owned()))?;
                stamp_update(&mut object, &current.object, principal);
                self.finish_mutation(&mut object, principal)?;
                let value = encode(&object)?;
                Ok(Staged {
                    op: Op::Put {
                        key,
                        value,
                        expected: Precondition::Revision(current.revision),
                    },
                    object: Some(object),
                })
            }
            Verb::Delete => {
                let current = self.load(&key).await?.ok_or_else(|| ApiError::NotFound {
                    kind: req.kind.to_string(),
                    name: req.name.clone(),
                })?;
                Ok(Staged {
                    op: Op::Delete {
                        key,
                        expected: Precondition::Revision(current.revision),
                    },
                    object: None,
                })
            }
        }
    }

    // 3b. finish the mutation — K8 attenuate + seal (metadata already stamped).
    fn finish_mutation(
        &self,
        object: &mut ResourceObject,
        principal: &Principal,
    ) -> Result<(), ApiError> {
        // Authorize the object by a child token scoped to this action, not the
        // broad parent (attenuation; I-1/I-3). Fail closed if it cannot be minted.
        if let Some(parent) = &principal.token {
            let ceiling = crate::policy::classification_ceiling(object);
            let action = format!("{}/{}", object.kind(), object.name());
            let child = self.sealer.mint_child(parent, ceiling, &action)?;
            object.metadata_mut().token_ref = Some(TokenRef {
                token_id: child.token_id,
            });
        }
        // Envelope-seal flagged sensitive spec fields at rest (I-2).
        self.sealer.seal_fields(object)
    }

    // 4. commit — the sole store write.
    async fn commit(
        &self,
        req: &AdmissionRequest,
        staged: Staged,
    ) -> Result<AdmissionOutcome, ApiError> {
        let response = self
            .raft
            .write(staged.op)
            .await
            .map_err(|e| ApiError::Store(e.to_string()))?;
        match response {
            RaftResponse::Applied { revision, .. } => {
                let object = staged.object.map(|mut o| {
                    o.metadata_mut().resource_version = revision;
                    o
                });
                Ok(AdmissionOutcome { object, revision })
            }
            RaftResponse::Deleted { revision } => Ok(AdmissionOutcome {
                object: None,
                revision,
            }),
            // A failed precondition is a value, not a fault: create-on-existing or
            // a concurrent modification. Both surface as a client-visible conflict.
            RaftResponse::Rejected { reason } => Err(ApiError::Conflict {
                kind: req.kind.to_string(),
                name: req.name.clone(),
                reason,
            }),
            RaftResponse::Noop => Err(ApiError::Store(
                "store returned noop for a data mutation".to_owned(),
            )),
        }
    }

    // 5. receipt — K9 seam.
    #[allow(clippy::unused_self)]
    fn receipt(
        &self,
        _req: &AdmissionRequest,
        _principal: &Principal,
        _outcome: &AdmissionOutcome,
    ) {
        // K9: emit a hash-chained `fabric-proof` receipt (token, before/after
        // digest, decision) to `wsf-ledger` — provable off-host with the public
        // key alone, and physically separate from this intent store (A1.4).
    }

    // Read current committed state for a read-modify-write (update/delete).
    async fn load(&self, key: &str) -> Result<Option<Current>, ApiError> {
        match self
            .raft
            .get(key)
            .await
            .map_err(|e| ApiError::Store(e.to_string()))?
        {
            Some(versioned) => Ok(Some(Current {
                object: decode(&versioned)?,
                revision: versioned.mod_revision,
            })),
            None => Ok(None),
        }
    }
}

/// A staged, admitted mutation ready to commit.
struct Staged {
    op: Op,
    object: Option<ResourceObject>,
}

/// Current committed state of a key (for read-modify-write).
struct Current {
    object: ResourceObject,
    revision: Revision,
}

/// Stamp the identity/bookkeeping a fresh object gets on admission. `generation`
/// starts at 1; `resource_version` is set from the commit revision afterward.
fn stamp_create(object: &mut ResourceObject, principal: &Principal) {
    let meta = object.metadata_mut();
    meta.uid = new_uid();
    meta.generation = 1;
    meta.resource_version = 0;
    meta.created_at = Some(now_rfc3339());
    meta.token_ref.clone_from(&principal.token_ref);
}

/// Carry immutable identity (`uid`, `created_at`) forward on update and bump
/// `generation` for the new spec.
fn stamp_update(object: &mut ResourceObject, current: &ResourceObject, principal: &Principal) {
    let prior = current.metadata();
    let meta = object.metadata_mut();
    meta.uid.clone_from(&prior.uid);
    meta.created_at.clone_from(&prior.created_at);
    meta.generation = prior.generation + 1;
    meta.resource_version = 0;
    meta.token_ref = principal
        .token_ref
        .clone()
        .or_else(|| prior.token_ref.clone());
}

fn new_uid() -> String {
    uuid::Uuid::new_v4().to_string()
}

fn now_rfc3339() -> String {
    chrono::Utc::now().to_rfc3339()
}
