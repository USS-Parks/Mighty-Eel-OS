//! The admission choke point — the **only** path that writes desired state.
//!
//! Type invariant ("no write reaches `aog-store` bypassing
//! admission, enforced by type"): [`Admission`] privately owns the sole writable
//! `RaftNode` handle in this crate, and [`Admission::admit`] is the only method
//! that reaches [`RaftNode::write`]. A CRUD handler is handed an `Admission` and
//! a read-only [`crate::reader::StoreReader`]; neither exposes the raw node, so
//! no handler can construct a store write that skips the chain.
//!
//! Chain order mirrors addendum A1.7:
//!   1. authenticate  (the front-door `crate::auth` middleware hands `admit` an already-verified Principal)
//!   2. validate      (structural, fail-closed + policy deny-wins over HIPAA/ITAR/OCAP)
//!   3. mutate        (metadata stamp + envelope-seal flagged fields + child-token attenuation)
//!   4. commit        (the sole `aog-store` write, guarded by a CAS precondition)
//!   5. receipt       (hash-chained `fabric-proof` receipt to `wsf-ledger`)
//!
//! All five stages do real work now. The choke point is complete — every mutation
//! traverses this one method, and each admitted one is receipted.

use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use aog_estate::{Kind, ResourceObject, TokenRef};
use aog_store::raft::RaftNode;
use aog_store::raft::types::RaftResponse;
use aog_store::{Op, Precondition, Revision};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use fabric_contracts::{
    Audience, AuthStrength, AuthenticatedFacts, CanonicalResource, EstateScope, IdentityKind,
    PrivilegedCapability, RequestOperation, TenantScope, TrustToken, VerifiedRequestContext,
    WsfPrincipal,
};
use fabric_crypto::providers::RustCryptoMlDsa87;
use wsf_ledger::{EvidencePack, Ledger};

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

impl Verb {
    fn as_str(self) -> &'static str {
        match self {
            Verb::Create => "create",
            Verb::Update => "update",
            Verb::Delete => "delete",
        }
    }
}

/// The authenticated caller, produced by the front-door authenticator from
/// a verified WSF trust token and carried through the chain so mutate/receipt can
/// stamp provenance and can attenuate a child from the parent token.
#[derive(Debug, Clone)]
pub struct Principal {
    request_principal: WsfPrincipal,
    system: bool,
    /// The authorizing capability reference stamped onto mutated objects.
    token_ref: Option<TokenRef>,
    /// The verified trust token — carried for downstream stages (attenuation).
    token: Option<TrustToken>,
}

impl Principal {
    /// A verified caller from an authenticated WSF trust token.
    #[must_use]
    pub fn authenticated(token: TrustToken) -> Self {
        let principal_id = token
            .service_identity
            .clone()
            .unwrap_or_else(|| token.subject_hash.clone());
        let kind = if token.service_identity.is_some() {
            IdentityKind::Workload
        } else {
            IdentityKind::Human
        };
        let request_principal = WsfPrincipal::establish(
            AuthenticatedFacts {
                principal_id,
                kind,
                tenant_id: token.tenant_id.clone(),
                subject_hash: token.subject_hash.clone(),
                service_identity: token.service_identity.clone(),
                roles: token.roles.clone(),
                token_lineage: Some(fabric_token::lineage_key(&token).to_string()),
                auth_strength: AuthStrength::WorkloadToken,
                audience: Audience::Aog,
            },
            uuid::Uuid::new_v4().to_string(),
            chrono::Utc::now().to_rfc3339(),
        );
        Self {
            request_principal,
            system: false,
            token_ref: Some(TokenRef {
                token_id: token.token_id.clone(),
            }),
            token: Some(token),
        }
    }

    /// The system principal — for internal callers with no external request
    /// (later-phase controllers). Never minted from an inbound request.
    #[must_use]
    fn system() -> Self {
        Self {
            request_principal: WsfPrincipal::establish(
                AuthenticatedFacts {
                    principal_id: "system:apiserver".to_owned(),
                    kind: IdentityKind::Workload,
                    tenant_id: String::new(),
                    subject_hash: "system:apiserver".to_owned(),
                    service_identity: Some("aog-controller".to_owned()),
                    roles: vec!["estate-system".to_owned()],
                    token_lineage: None,
                    auth_strength: AuthStrength::MutualTls,
                    audience: Audience::Aog,
                },
                uuid::Uuid::new_v4().to_string(),
                chrono::Utc::now().to_rfc3339(),
            ),
            system: true,
            token_ref: None,
            token: None,
        }
    }

    #[must_use]
    pub fn subject(&self) -> &str {
        &self.request_principal.subject_hash
    }

    #[must_use]
    pub fn tenant(&self) -> Option<&str> {
        (!self.system)
            .then_some(self.request_principal.tenant_id.as_str())
            .filter(|tenant| !tenant.is_empty())
    }

    /// Explicit estate-wide reader authority. Ordinary tenant principals never
    /// receive global read behavior merely because their token is valid.
    #[must_use]
    pub fn is_estate_reader(&self) -> bool {
        self.system
            || self
                .request_principal
                .roles
                .iter()
                .any(|role| role == "estate:read")
    }

    #[must_use]
    pub fn token_ref(&self) -> Option<&TokenRef> {
        self.token_ref.as_ref()
    }

    #[must_use]
    pub fn token(&self) -> Option<&TrustToken> {
        self.token.as_ref()
    }

    fn request_context(&self, req: &AdmissionRequest) -> Result<VerifiedRequestContext, ApiError> {
        let operation = match req.verb {
            Verb::Create => RequestOperation::AogCreate,
            Verb::Update => RequestOperation::AogUpdate,
            Verb::Delete => RequestOperation::AogDelete,
        };
        let resource = CanonicalResource::resolved(req.kind.to_string(), &req.name, None)
            .map_err(|e| ApiError::Forbidden(e.to_string()))?;
        VerifiedRequestContext::establish(self.request_principal.clone(), operation, resource)
            .map_err(|e| ApiError::Forbidden(e.to_string()))
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
    ledger: Arc<Mutex<Ledger>>,
    delivered_receipts: Mutex<HashSet<String>>,
}

impl Admission {
    #[must_use]
    pub fn new(raft: Arc<RaftNode>, sealer: Sealer) -> Self {
        // The receipt ledger signs its evidence pack; ML-DSA keygen is infallible
        // (fabric-crypto), so this construction cannot fail.
        let ledger_signer = RustCryptoMlDsa87::generate("aog-apiserver-receipts")
            .expect("ML-DSA keygen infallible");
        Self {
            raft,
            policy: AdmissionPolicy::baseline(),
            sealer,
            ledger: Arc::new(Mutex::new(Ledger::new(Arc::new(ledger_signer)))),
            delivered_receipts: Mutex::new(HashSet::new()),
        }
    }

    /// Run the admission chain and, only if every stage passes, commit exactly
    /// one desired-state mutation. This is the one method in the crate that
    /// writes the store.
    ///
    /// # Errors
    /// The first stage to refuse: [`ApiError::Invalid`] (structural) or
    /// [`ApiError::Forbidden`] (policy); [`ApiError::NotFound`] /
    /// [`ApiError::Conflict`] at commit; or [`ApiError::Store`] on backend failure.
    pub async fn admit(
        &self,
        req: AdmissionRequest,
        principal: &Principal,
    ) -> Result<AdmissionOutcome, ApiError> {
        // Stage 1 (authenticate) is the front-door middleware (`crate::auth`):
        // `principal` is already a verified token by the time admission runs.
        // The chain here is validate -> mutate -> commit -> receipt.
        let context = principal.request_context(&req)?;
        self.validate(&req, principal, &context)?;
        let staged = self.mutate(&req, principal, &context).await?;
        let before_digest = staged.before_digest.clone();
        let mutated = staged.op.is_some();
        let audit_intent = if mutated {
            Some(
                self.persist_audit_intent(&req, principal, &context, &staged)
                    .await?,
            )
        } else {
            None
        };
        let outcome = self.commit(&req, staged).await?;
        // Receipts are 1:1 with *mutations*: an idempotent no-op (a repeat
        // delete of an already-terminating object) changed nothing and writes none.
        if mutated {
            let intent = audit_intent.as_ref().expect("mutated intent");
            let receipt = self.receipt_value(
                &req,
                principal,
                before_digest.as_deref(),
                &outcome,
                &intent.id,
            );
            self.finalize_audit_intent(intent, &receipt).await?;
            self.deliver_receipt(receipt);
        }
        Ok(outcome)
    }

    async fn persist_audit_intent(
        &self,
        req: &AdmissionRequest,
        principal: &Principal,
        context: &VerifiedRequestContext,
        staged: &Staged,
    ) -> Result<AuditIntent, ApiError> {
        let intent_id = uuid::Uuid::new_v4().to_string();
        let key = format!("AuditOutbox/{intent_id}");
        let value = serde_json::to_vec(&serde_json::json!({
            "schema": "aog.audit-intent/v1",
            "intent_id": intent_id,
            "correlation_id": context.principal().correlation_id,
            "tenant_id": principal.tenant().unwrap_or_default(),
            "subject_hash": principal.subject(),
            "operation": context.operation(),
            "resource": context.resource(),
            "verb": req.verb.as_str(),
            "before_digest": staged.before_digest,
            "after_digest": staged.object.as_ref().and_then(digest),
            "planned_op": staged.op,
            "created_at": now_rfc3339(),
        }))
        .map_err(|e| ApiError::Store(format!("serialize audit intent: {e}")))?;
        match self
            .raft
            .write(Op::Put {
                key,
                value,
                expected: Precondition::Absent,
            })
            .await
            .map_err(|e| ApiError::Store(format!("persist audit intent: {e}")))?
        {
            RaftResponse::Applied { revision, .. } => Ok(AuditIntent {
                id: intent_id,
                revision,
            }),
            RaftResponse::Rejected { reason } => Err(ApiError::Store(format!(
                "audit intent rejected before mutation: {reason}"
            ))),
            other => Err(ApiError::Store(format!(
                "unexpected audit intent response before mutation: {other:?}"
            ))),
        }
    }

    async fn finalize_audit_intent(
        &self,
        intent: &AuditIntent,
        receipt: &serde_json::Value,
    ) -> Result<(), ApiError> {
        let value = serde_json::to_vec(&serde_json::json!({
            "schema": "aog.audit-outbox/v1",
            "intent_id": intent.id,
            "status": "ready",
            "receipt": receipt,
        }))
        .map_err(|error| ApiError::Store(format!("serialize audit outbox: {error}")))?;
        match self
            .raft
            .write(Op::Put {
                key: format!("AuditOutbox/{}", intent.id),
                value,
                expected: Precondition::Revision(intent.revision),
            })
            .await
            .map_err(|error| ApiError::Store(format!("finalize audit outbox: {error}")))?
        {
            RaftResponse::Applied { .. } => Ok(()),
            other => Err(ApiError::Store(format!(
                "audit outbox finalization rejected after mutation: {other:?}"
            ))),
        }
    }

    /// Admit an internal controller mutation with a server-created estate
    /// principal. Controllers never receive a constructor for that authority.
    pub async fn admit_system(&self, req: AdmissionRequest) -> Result<AdmissionOutcome, ApiError> {
        self.admit(req, &Principal::system()).await
    }

    // 2. validate — structural (fail-closed) + policy (deny-wins over regimes).
    fn validate(
        &self,
        req: &AdmissionRequest,
        principal: &Principal,
        context: &VerifiedRequestContext,
    ) -> Result<(), ApiError> {
        let expected_operation = match req.verb {
            Verb::Create => RequestOperation::AogCreate,
            Verb::Update => RequestOperation::AogUpdate,
            Verb::Delete => RequestOperation::AogDelete,
        };
        context
            .require_operation(expected_operation)
            .map_err(|e| ApiError::Forbidden(e.to_string()))?;
        debug_assert_eq!(context.resource().kind(), req.kind.to_string());
        debug_assert_eq!(context.resource().name(), req.name);
        if let Some(object) = &req.object {
            object.validate()?;
            self.policy.evaluate(object, principal)?;
            if let ResourceObject::RevocationIntent(intent) = object {
                self.authorize_revocation(req, principal, &intent.spec.target)?;
            }
            if matches!(object, ResourceObject::PolicyBundle(_)) && principal.tenant().is_none() {
                EstateScope::authorize(context, PrivilegedCapability::PolicyPublication)
                    .map_err(|error| ApiError::Forbidden(error.to_string()))?;
            }
        }
        Ok(())
    }

    fn authorize_revocation(
        &self,
        req: &AdmissionRequest,
        principal: &Principal,
        target: &aog_estate::RevocationTarget,
    ) -> Result<(), ApiError> {
        if let Some(tenant) = principal.tenant() {
            if matches!(target, aog_estate::RevocationTarget::Ring(_)) {
                return Err(ApiError::Forbidden(
                    "tenant principal cannot create an estate-wide ring revocation".to_owned(),
                ));
            }
            if let aog_estate::RevocationTarget::Tenant(target_tenant) = target
                && target_tenant != tenant
            {
                return Err(ApiError::Forbidden(
                    "tenant principal cannot revoke another tenant".to_owned(),
                ));
            }
            let resource = CanonicalResource::resolved(
                req.kind.to_string(),
                &req.name,
                Some(tenant.to_owned()),
            )
            .map_err(|error| ApiError::Forbidden(error.to_string()))?;
            let context = VerifiedRequestContext::establish(
                principal.request_principal.clone(),
                match req.verb {
                    Verb::Create => RequestOperation::AogCreate,
                    Verb::Update => RequestOperation::AogUpdate,
                    Verb::Delete => RequestOperation::AogDelete,
                },
                resource,
            )
            .map_err(|error| ApiError::Forbidden(error.to_string()))?;
            TenantScope::authorize(&context, PrivilegedCapability::TenantRevocation)
                .map_err(|error| ApiError::Forbidden(error.to_string()))?;
        } else {
            let context = principal.request_context(req)?;
            EstateScope::authorize(&context, PrivilegedCapability::EstateRevocation)
                .map_err(|error| ApiError::Forbidden(error.to_string()))?;
            if matches!(target, aog_estate::RevocationTarget::Ring(_)) {
                EstateScope::authorize(&context, PrivilegedCapability::RingKeyDestruction)
                    .map_err(|error| ApiError::Forbidden(error.to_string()))?;
            }
        }
        Ok(())
    }

    // 3. mutate — stamp metadata, then finish_mutation attenuates + seals.
    async fn mutate(
        &self,
        req: &AdmissionRequest,
        principal: &Principal,
        context: &VerifiedRequestContext,
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
                    op: Some(Op::Put {
                        key,
                        value,
                        expected: Precondition::Absent,
                    }),
                    object: Some(object),
                    before_digest: None,
                })
            }
            Verb::Update => {
                let current = self.load(&key).await?.ok_or_else(|| ApiError::NotFound {
                    kind: req.kind.to_string(),
                    name: req.name.clone(),
                })?;
                if current.object.metadata().tenant.is_none() {
                    EstateScope::authorize(context, PrivilegedCapability::GlobalObjectMutation)
                        .map_err(|error| ApiError::Forbidden(error.to_string()))?;
                }
                // A tenant-scoped principal may not overwrite an object owned by
                // another tenant; a global principal is unrestricted (mirror the
                // delete guard).
                if let (Some(pt), Some(ot)) = (
                    principal.tenant(),
                    current.object.metadata().tenant.as_deref(),
                ) && pt != ot
                {
                    return Err(ApiError::Forbidden(format!(
                        "principal tenant {pt} may not update {}/{} owned by tenant {ot}",
                        req.kind, req.name
                    )));
                }
                let mut object = req
                    .object
                    .clone()
                    .ok_or_else(|| ApiError::BadBody("update requires a body".to_owned()))?;
                // Client-side optimistic concurrency: a body carrying a non-zero
                // resource_version asserts "I read this revision" — a stale one
                // is refused rather than silently overwriting a newer write.
                let asserted = object.metadata().resource_version;
                if asserted != 0 && asserted != current.revision {
                    return Err(ApiError::Conflict {
                        kind: req.kind.to_string(),
                        name: req.name.clone(),
                        reason: format!(
                            "stale resource_version {asserted}, current is {}",
                            current.revision
                        ),
                    });
                }
                let terminating = current.object.metadata().deletion_timestamp.is_some();
                if terminating {
                    Self::check_terminating_update(req, &object, &current.object)?;
                }
                let before_digest = digest(&current.object);
                stamp_update(&mut object, &current.object, principal);
                // Finalization: removing the last finalizer from a terminating
                // object completes its two-phase delete — the update commits as
                // the hard delete the earlier soft delete promised.
                if terminating && object.metadata().finalizers.is_empty() {
                    return Ok(Staged {
                        op: Some(Op::Delete {
                            key,
                            expected: Precondition::Revision(current.revision),
                        }),
                        object: None,
                        before_digest,
                    });
                }
                self.finish_mutation(&mut object, principal)?;
                let value = encode(&object)?;
                Ok(Staged {
                    op: Some(Op::Put {
                        key,
                        value,
                        expected: Precondition::Revision(current.revision),
                    }),
                    object: Some(object),
                    before_digest,
                })
            }
            Verb::Delete => {
                let current = self.load(&key).await?.ok_or_else(|| ApiError::NotFound {
                    kind: req.kind.to_string(),
                    name: req.name.clone(),
                })?;
                let meta = current.object.metadata();
                if req.kind == Kind::RevocationIntent && principal.tenant().is_some() {
                    return Err(ApiError::Forbidden(
                        "tenant principals may not delete revocation/kill records".to_owned(),
                    ));
                }
                if meta.tenant.is_none() {
                    EstateScope::authorize(context, PrivilegedCapability::GlobalObjectMutation)
                        .map_err(|error| ApiError::Forbidden(error.to_string()))?;
                }
                // A3: authorize the delete against the loaded target. The
                // policy gate is skipped for deletes (validate sees object=None),
                // which let any authenticated principal delete any object incl. a
                // RevocationIntent (reversing a live kill). Run the same policy here,
                // and bind a tenant-scoped principal to its own tenant.
                self.policy.evaluate(&current.object, principal)?;
                if let (Some(pt), Some(ot)) = (principal.tenant(), meta.tenant.as_deref())
                    && pt != ot
                {
                    return Err(ApiError::Forbidden(format!(
                        "principal tenant {pt} may not delete {}/{} owned by tenant {ot}",
                        req.kind, req.name
                    )));
                }
                let before_digest = digest(&current.object);
                // Two-phase delete. No finalizers: remove now. Finalizers
                // present: stamp deletion_timestamp (soft delete) and let the
                // finalizing controllers tear down, then finalize via update.
                if meta.finalizers.is_empty() {
                    Ok(Staged {
                        op: Some(Op::Delete {
                            key,
                            expected: Precondition::Revision(current.revision),
                        }),
                        object: None,
                        before_digest,
                    })
                } else if meta.deletion_timestamp.is_some() {
                    // Already terminating — a repeat delete is idempotent: no
                    // mutation, no receipt, the terminating object returned.
                    Ok(Staged {
                        op: None,
                        object: Some(current.object.clone()),
                        before_digest: None,
                    })
                } else {
                    let mut object = current.object.clone();
                    let meta = object.metadata_mut();
                    meta.deletion_timestamp = Some(now_rfc3339());
                    // Provenance of who requested the deletion.
                    if let Some(token_ref) = principal.token_ref() {
                        meta.token_ref = Some(token_ref.clone());
                    }
                    let value = encode(&object)?;
                    Ok(Staged {
                        op: Some(Op::Put {
                            key,
                            value,
                            expected: Precondition::Revision(current.revision),
                        }),
                        object: Some(object),
                        before_digest,
                    })
                }
            }
        }
    }

    // While an object is terminating, its spec is frozen and its finalizer
    // set may only shrink — teardown converges, it is never redirected. The
    // deletion timestamp itself is carried forward by `stamp_update` (an update
    // can never resurrect a terminating object).
    fn check_terminating_update(
        req: &AdmissionRequest,
        object: &ResourceObject,
        current: &ResourceObject,
    ) -> Result<(), ApiError> {
        let conflict = |reason: &str| ApiError::Conflict {
            kind: req.kind.to_string(),
            name: req.name.clone(),
            reason: reason.to_owned(),
        };
        if spec_of(object) != spec_of(current) {
            return Err(conflict("spec is frozen while the object is terminating"));
        }
        let prior = &current.metadata().finalizers;
        if object
            .metadata()
            .finalizers
            .iter()
            .any(|f| !prior.contains(f))
        {
            return Err(conflict(
                "finalizers may only be removed while the object is terminating",
            ));
        }
        Ok(())
    }

    // 3b. finish the mutation — attenuate + seal (metadata already stamped).
    fn finish_mutation(
        &self,
        object: &mut ResourceObject,
        principal: &Principal,
    ) -> Result<(), ApiError> {
        // Authorize the object by a child token scoped to this action, not the
        // broad parent (attenuation; I-1/I-3). Fail closed if it cannot be minted.
        if let Some(parent) = principal.token() {
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
        // An idempotent no-op (repeat delete of a terminating object) commits
        // nothing: the current object is returned at its stored revision.
        let Some(op) = staged.op else {
            let revision = staged
                .object
                .as_ref()
                .map_or(0, |o| o.metadata().resource_version);
            return Ok(AdmissionOutcome {
                object: staged.object,
                revision,
            });
        };
        let response = self
            .raft
            .write(op)
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

    // 5. receipt — one hash-chained receipt per admitted mutation.
    fn receipt_value(
        &self,
        req: &AdmissionRequest,
        principal: &Principal,
        before_digest: Option<&str>,
        outcome: &AdmissionOutcome,
        audit_intent_id: &str,
    ) -> serde_json::Value {
        let token_id = outcome
            .object
            .as_ref()
            .and_then(|o| o.metadata().token_ref.as_ref())
            .or(principal.token_ref())
            .map(|t| t.token_id.clone())
            .unwrap_or_default();
        let after_digest = outcome.object.as_ref().and_then(digest);
        serde_json::json!({
            "receipt_id": format!("rcpt:{}:{}:{}", req.kind, req.name, outcome.revision),
            "audit_intent_id": audit_intent_id,
            "token_id": token_id,
            "tenant_id": principal.tenant().unwrap_or_default(),
            "kind": req.kind.to_string(),
            "name": req.name,
            "verb": req.verb.as_str(),
            "decision": "admit",
            "before_digest": before_digest,
            "after_digest": after_digest,
            "revision": outcome.revision,
            "recorded_at": now_rfc3339(),
        })
    }

    fn deliver_receipt(&self, receipt: serde_json::Value) {
        let Some(receipt_id) = receipt.get("receipt_id").and_then(|value| value.as_str()) else {
            return;
        };
        if !self
            .delivered_receipts
            .lock()
            .expect("delivered receipt lock")
            .insert(receipt_id.to_owned())
        {
            return;
        }
        // Physically separate from the intent store (A1.4); provable off-host with
        // the ledger's public key alone. A canonical-JSON receipt cannot fail to
        // hash, so ingest is effectively infallible here.
        let _ = self
            .ledger
            .lock()
            .expect("receipt ledger lock")
            .ingest("aog-apiserver", receipt);
    }

    /// Rebuild the in-memory/off-host-verifiable sink from Raft-durable outbox
    /// records. Finalized mutations replay their exact receipt. A pre-commit
    /// intent left pending by a crash is preserved as an explicit,
    /// indeterminate recovery receipt: it proves the durable audit barrier and
    /// planned before/after digests without falsely claiming that the mutation
    /// committed.
    pub async fn recover_receipts(&self) -> Result<usize, ApiError> {
        let entries = self
            .raft
            .range("AuditOutbox/")
            .await
            .map_err(|error| ApiError::Store(error.to_string()))?;
        let mut recovered = 0;
        for (_, versioned) in entries {
            let value: serde_json::Value = serde_json::from_slice(&versioned.value)
                .map_err(|error| ApiError::Store(format!("decode audit outbox: {error}")))?;
            let receipt = match value.get("schema").and_then(|field| field.as_str()) {
                Some("aog.audit-outbox/v1") => value.get("receipt").cloned().ok_or_else(|| {
                    ApiError::Store("finalized audit outbox is missing its receipt".to_owned())
                })?,
                Some("aog.audit-intent/v1") => {
                    let intent_id = value
                        .get("intent_id")
                        .and_then(|field| field.as_str())
                        .ok_or_else(|| {
                            ApiError::Store("pending audit intent is missing its id".to_owned())
                        })?;
                    serde_json::json!({
                        "receipt_id": format!("audit-intent:{intent_id}"),
                        "audit_intent_id": intent_id,
                        "tenant_id": value.get("tenant_id").cloned().unwrap_or_default(),
                        "subject_hash": value.get("subject_hash").cloned().unwrap_or_default(),
                        "operation": value.get("operation").cloned().unwrap_or_default(),
                        "resource": value.get("resource").cloned().unwrap_or_default(),
                        "verb": value.get("verb").cloned().unwrap_or_default(),
                        "decision": "audit-intent-recovered",
                        "mutation_status": "indeterminate",
                        "before_digest": value.get("before_digest").cloned().unwrap_or_default(),
                        "after_digest": value.get("after_digest").cloned().unwrap_or_default(),
                        "intent_revision": versioned.mod_revision,
                        "recorded_at": value.get("created_at").cloned().unwrap_or_default(),
                    })
                }
                other => {
                    return Err(ApiError::Store(format!(
                        "unknown audit outbox schema: {other:?}"
                    )));
                }
            };
            self.deliver_receipt(receipt);
            recovered += 1;
        }
        Ok(recovered)
    }

    /// Number of receipts in the ledger — one per admitted mutation.
    #[must_use]
    pub fn receipts_len(&self) -> usize {
        self.ledger.lock().expect("receipt ledger lock").len()
    }

    /// Number of durable audit intents retained for recovery/delivery.
    pub async fn audit_intents_len(&self) -> Result<usize, ApiError> {
        self.raft
            .range("AuditOutbox/")
            .await
            .map(|entries| entries.len())
            .map_err(|e| ApiError::Store(e.to_string()))
    }

    /// The receipt ledger's public key — verifies an exported pack off-host.
    #[must_use]
    pub fn receipts_public_key(&self) -> Vec<u8> {
        self.ledger
            .lock()
            .expect("receipt ledger lock")
            .public_key()
            .to_vec()
    }

    /// Export a signed evidence pack over the receipt chain.
    ///
    /// # Errors
    /// [`ApiError::Store`] on hashing/signing failure.
    pub fn export_receipts(&self, generated_at: &str) -> Result<EvidencePack, ApiError> {
        self.ledger
            .lock()
            .expect("receipt ledger lock")
            .export_pack(generated_at)
            .map_err(|e| ApiError::Store(e.to_string()))
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

/// A staged, admitted mutation ready to commit. `op: None` is an admitted
/// no-op — nothing to write, nothing to receipt.
struct Staged {
    op: Option<Op>,
    object: Option<ResourceObject>,
    before_digest: Option<String>,
}

/// Current committed state of a key (for read-modify-write).
struct Current {
    object: ResourceObject,
    revision: Revision,
}

struct AuditIntent {
    id: String,
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
    // Only a DELETE sets the deletion timestamp — a create body cannot smuggle
    // a terminating state in.
    meta.deletion_timestamp = None;
    meta.token_ref = principal.token_ref().cloned();
    // Bind a tenant-scoped principal's object to its own tenant — a create body
    // cannot smuggle a different metadata.tenant (the delete path enforces the
    // same binding). A global (untenanted) principal may still create for any
    // tenant.
    if let Some(pt) = principal.tenant() {
        meta.tenant = Some(pt.to_owned());
    }
}

/// Carry immutable identity (`uid`, `created_at`) forward on update and bump
/// `generation` for the new spec.
fn stamp_update(object: &mut ResourceObject, current: &ResourceObject, principal: &Principal) {
    if !principal.system {
        preserve_status(object, current);
    }
    let prior = current.metadata();
    let meta = object.metadata_mut();
    meta.uid.clone_from(&prior.uid);
    meta.created_at.clone_from(&prior.created_at);
    meta.generation = prior.generation + 1;
    meta.resource_version = 0;
    // Immutable-after-create / after-delete metadata: the deletion timestamp is
    // carried forward (an update can never resurrect a terminating object) and
    // owner references are frozen (ownership cannot be hijacked by update).
    meta.deletion_timestamp
        .clone_from(&prior.deletion_timestamp);
    meta.owner_refs.clone_from(&prior.owner_refs);
    // Tenant is fixed at create; carry it forward so an update body cannot
    // reassign the object to another tenant.
    meta.tenant.clone_from(&prior.tenant);
    meta.token_ref = principal
        .token_ref
        .clone()
        .or_else(|| prior.token_ref.clone());
}

/// Status/counters are controller-owned authoritative state. External update
/// bodies may change desired spec but cannot lower usage, replay versions, or
/// manufacture readiness.
fn preserve_status(object: &mut ResourceObject, current: &ResourceObject) {
    macro_rules! copy_status {
        ($new:ident, $old:ident) => {
            $new.status.clone_from(&$old.status)
        };
    }
    match (object, current) {
        (ResourceObject::Tenant(new), ResourceObject::Tenant(old)) => copy_status!(new, old),
        (ResourceObject::TrustRing(new), ResourceObject::TrustRing(old)) => copy_status!(new, old),
        (ResourceObject::VirtualKey(new), ResourceObject::VirtualKey(old)) => {
            copy_status!(new, old)
        }
        (ResourceObject::Capability(new), ResourceObject::Capability(old)) => {
            copy_status!(new, old)
        }
        (ResourceObject::PolicyBundle(new), ResourceObject::PolicyBundle(old)) => {
            copy_status!(new, old)
        }
        (ResourceObject::ProviderPool(new), ResourceObject::ProviderPool(old)) => {
            copy_status!(new, old)
        }
        (ResourceObject::Workload(new), ResourceObject::Workload(old)) => copy_status!(new, old),
        (ResourceObject::Placement(new), ResourceObject::Placement(old)) => copy_status!(new, old),
        (ResourceObject::Node(new), ResourceObject::Node(old)) => copy_status!(new, old),
        (ResourceObject::MissionContract(new), ResourceObject::MissionContract(old)) => {
            copy_status!(new, old)
        }
        (ResourceObject::ToolGrant(new), ResourceObject::ToolGrant(old)) => copy_status!(new, old),
        (ResourceObject::RolloutPlan(new), ResourceObject::RolloutPlan(old)) => {
            copy_status!(new, old)
        }
        (ResourceObject::RevocationIntent(new), ResourceObject::RevocationIntent(old)) => {
            copy_status!(new, old)
        }
        _ => {}
    }
}

/// The `spec` sub-value of an object, for the terminating spec-freeze check.
fn spec_of(object: &ResourceObject) -> Option<serde_json::Value> {
    object.to_value().ok().and_then(|v| v.get("spec").cloned())
}

/// The canonical digest of an object, for a receipt's before/after fields.
fn digest(object: &ResourceObject) -> Option<String> {
    let value = object.to_value().ok()?;
    let hash = fabric_proof::canonical_hash(&value).ok()?;
    Some(BASE64.encode(hash))
}

fn new_uid() -> String {
    uuid::Uuid::new_v4().to_string()
}

fn now_rfc3339() -> String {
    chrono::Utc::now().to_rfc3339()
}
