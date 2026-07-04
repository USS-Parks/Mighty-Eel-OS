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
//!   1. authenticate  (K6 — front-door WSF token verify; system principal until then)
//!   2. validate      (structural now, fail-closed; policy deny-wins is K7)
//!   3. mutate        (metadata stamp now; envelope-seal + token attenuation are K8)
//!   4. commit        (the sole `aog-store` write, guarded by a CAS precondition)
//!   5. receipt       (K9 — hash-chained `fabric-proof` receipt to `wsf-ledger`)
//!
//! Stages 2/3/4 do real work today; stage 1, stage 5, and the seal/policy parts
//! of 2/3 are wired in the named later prompts. The choke point itself is
//! complete now — every mutation already traverses this one method.

use std::sync::Arc;

use aog_estate::{Kind, ResourceObject, TokenRef};
use aog_store::raft::RaftNode;
use aog_store::raft::types::RaftResponse;
use aog_store::{Op, Precondition, Revision};

use crate::codec::{decode, encode, store_key};
use crate::error::ApiError;

/// The mutation a request asks admission to perform.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verb {
    Create,
    Update,
    Delete,
}

/// The authenticated caller. K6 fills this from a verified WSF capability token;
/// until then the front door runs as the system principal. Carried through the
/// chain so mutate/receipt can stamp provenance (`token_ref`, receipt subject).
#[derive(Debug, Clone)]
pub struct Principal {
    pub subject: String,
    pub token_ref: Option<TokenRef>,
}

impl Principal {
    /// The kernel's standing identity until front-door authN (K6) lands.
    #[must_use]
    pub fn system() -> Self {
        Self {
            subject: "system:apiserver".to_owned(),
            token_ref: None,
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
}

impl Admission {
    #[must_use]
    pub fn new(raft: Arc<RaftNode>) -> Self {
        Self { raft }
    }

    /// Run the admission chain and, only if every stage passes, commit exactly
    /// one desired-state mutation. This is the one method in the crate that
    /// writes the store.
    ///
    /// # Errors
    /// The first stage to refuse: [`ApiError::Invalid`] (structural), later
    /// [`ApiError::Unauthenticated`]/[`ApiError::Forbidden`] (K6/K7),
    /// [`ApiError::NotFound`]/[`ApiError::Conflict`] at commit, or
    /// [`ApiError::Store`] on backend failure.
    pub async fn admit(&self, req: AdmissionRequest) -> Result<AdmissionOutcome, ApiError> {
        let principal = self.authenticate(&req)?;
        Self::validate(&req)?;
        let staged = self.mutate(&req, &principal).await?;
        let outcome = self.commit(&req, staged).await?;
        self.receipt(&req, &principal, &outcome);
        Ok(outcome)
    }

    // 1. authenticate — K6 seam.
    #[allow(clippy::unused_self, clippy::unnecessary_wraps)]
    fn authenticate(&self, _req: &AdmissionRequest) -> Result<Principal, ApiError> {
        // K6: verify the WSF `fabric-token` at the front door — budget, caveats,
        // revocation — and reject unauth / over-budget / revoked *before* the
        // chain proceeds (fail-closed, doctrine I-3/I-4). Until K6 the kernel
        // runs as the system principal.
        Ok(Principal::system())
    }

    // 2. validate — structural now (fail-closed); policy deny-wins is K7.
    fn validate(req: &AdmissionRequest) -> Result<(), ApiError> {
        if let Some(object) = &req.object {
            object.validate()?;
            // K7: run the mai-compliance deny-wins composer (HIPAA/ITAR/OCAP) +
            // per-kind resource policy here; a single deny refuses the mutation.
        }
        Ok(())
    }

    // 3. mutate — stamp metadata now; envelope-seal + token attenuation are K8.
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
                // K8: envelope-seal flagged spec fields (F4/F6) and attenuate a
                // child token scoped to exactly this action before the bytes land.
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
