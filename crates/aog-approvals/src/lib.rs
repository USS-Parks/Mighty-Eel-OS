//! `aog-approvals` (T3) — the human-approval inbox for side-effecting tool calls.
//!
//! A mutating (`has_side_effects`) tool call is **submitted** to the inbox with a
//! diff preview and **blocks** on a [`Ticket`] until a human approves or denies it;
//! every decision — approve or deny, with the actor — is **receipted** into a
//! `fabric-proof` chain (the verifiable audit trail the AOG/WSF stack uses). This is
//! the same inbox WSF credential grants and Aeneas remediations route through.
//!
//! The proxy (T1/T4) routes side-effecting calls here via an `ApprovalGate`; T4's
//! rule — untrusted tool output cannot auto-trigger a mutating call — is exactly
//! "such a call must pass through this inbox first."

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use aog_toolproxy::{APPROVER_ROLE, ApprovalGate, ApprovalGrant, InvokeContext};
use async_trait::async_trait;
use fabric_contracts::{RequestOperation, VerifiedRequestContext};
use fabric_proof::{ChainLink, GENESIS_HASH, canonical_hash, chain_link, verify_chain};
use mai_agent::types::{ToolCall, ToolDefinition};
use serde::Serialize;
use serde_json::Value;
use tokio::sync::oneshot;

const DEFAULT_APPROVAL_TTL: Duration = Duration::from_secs(300);

/// A side-effecting tool call awaiting a human decision.
#[derive(Debug, Clone, Serialize)]
pub struct ApprovalRequest {
    pub id: String,
    pub call_id: String,
    pub tool_id: String,
    pub session_id: String,
    pub tenant_id: String,
    /// The authorizing subject / trust-token id that initiated the call.
    pub requested_by: String,
    /// Human-readable preview of what the mutating call will do.
    pub diff_preview: String,
    /// Canonical digest of the exact immutable arguments awaiting approval.
    pub args_digest: String,
    /// Server-generated single-use nonce.
    pub nonce: String,
    pub requested_at: String,
    pub expires_at: String,
}

/// Untrusted request material accepted by the inbox. Identity, nonce, id,
/// timestamps, expiry, and argument digest are derived server-side.
#[derive(Debug, Clone)]
pub struct NewApprovalRequest {
    pub call_id: String,
    pub tool_id: String,
    pub session_id: String,
    pub requested_by: String,
    pub tenant_id: String,
    pub diff_preview: String,
    pub arguments: Value,
}

/// A human decision on a pending request.
#[derive(Debug, Clone)]
pub enum Decision {
    Approved {
        grant: ApprovalGrant,
        at: String,
    },
    Denied {
        actor: String,
        actor_role: String,
        tenant_id: String,
        reason: String,
        at: String,
    },
}

impl Decision {
    #[must_use]
    pub fn is_approved(&self) -> bool {
        matches!(self, Decision::Approved { .. })
    }

    #[must_use]
    pub fn actor(&self) -> &str {
        match self {
            Decision::Approved { grant, .. } => &grant.actor_id,
            Decision::Denied { actor, .. } => actor,
        }
    }
}

/// A metadata-only receipt of one decision.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct DecisionReceipt {
    pub approval_id: String,
    pub call_id: String,
    pub tool_id: String,
    pub actor: String,
    pub actor_role: String,
    pub tenant_id: String,
    pub approved: bool,
    pub args_digest: String,
    pub nonce: String,
    pub expires_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    pub at: String,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ApprovalError {
    #[error("approval decision requires an authenticated AOG update context")]
    WrongOperation,
    #[error("approval actor lacks the required role")]
    MissingRole,
    #[error("approval actor tenant does not match the pending request")]
    WrongTenant,
    #[error("approval context is not bound to the pending approval id")]
    WrongResource,
}

/// Append-only decision ledger: a BLAKE3 chain over [`DecisionReceipt`]s.
struct DecisionChain {
    links: Vec<ChainLink>,
    receipts: Vec<DecisionReceipt>,
    last_hash: [u8; 32],
}

impl DecisionChain {
    fn new() -> Self {
        Self {
            links: Vec::new(),
            receipts: Vec::new(),
            last_hash: GENESIS_HASH,
        }
    }

    fn append(&mut self, receipt: DecisionReceipt) {
        let value = serde_json::to_value(&receipt).expect("decision receipt serializes");
        let entry_hash = canonical_hash(&value).expect("canonical hash of decision receipt");
        self.links.push(ChainLink {
            previous_hash: self.last_hash,
            entry_hash,
        });
        self.last_hash = chain_link(&self.last_hash, &entry_hash);
        self.receipts.push(receipt);
    }
}

struct Pending {
    request: ApprovalRequest,
    tx: oneshot::Sender<Decision>,
}

/// The approval inbox. Cheap to share behind an `Arc` — every method takes `&self`.
pub struct ApprovalInbox {
    pending: Arc<Mutex<HashMap<String, Pending>>>,
    decisions: Arc<Mutex<DecisionChain>>,
    sequence: AtomicU64,
    ttl: Duration,
}

impl Default for ApprovalInbox {
    fn default() -> Self {
        Self::new()
    }
}

impl ApprovalInbox {
    #[must_use]
    pub fn new() -> Self {
        Self::with_ttl(DEFAULT_APPROVAL_TTL)
    }

    #[must_use]
    pub fn with_ttl(ttl: Duration) -> Self {
        assert!(!ttl.is_zero(), "approval TTL must be non-zero");
        Self {
            pending: Arc::new(Mutex::new(HashMap::new())),
            decisions: Arc::new(Mutex::new(DecisionChain::new())),
            sequence: AtomicU64::new(0),
            ttl,
        }
    }

    /// Submit a side-effecting call for approval; returns a [`Ticket`] the caller
    /// awaits. The call must not proceed until the ticket resolves.
    #[must_use]
    pub fn submit(&self, new: NewApprovalRequest) -> Ticket {
        let sequence = self.sequence.fetch_add(1, Ordering::SeqCst);
        let requested_at = chrono::Utc::now();
        let expires_at =
            requested_at + chrono::TimeDelta::from_std(self.ttl).expect("bounded approval TTL");
        let args_digest =
            hex::encode(canonical_hash(&new.arguments).expect("approval arguments canonicalize"));
        let nonce_material = serde_json::json!({
            "sequence": sequence,
            "call_id": new.call_id,
            "tool_id": new.tool_id,
            "session_id": new.session_id,
            "tenant_id": new.tenant_id,
            "requested_by": new.requested_by,
            "args_digest": args_digest,
            "requested_at": requested_at.to_rfc3339(),
        });
        let nonce = hex::encode(canonical_hash(&nonce_material).expect("nonce canonicalizes"));
        let id = format!("approval-{nonce}");
        let request = ApprovalRequest {
            id: id.clone(),
            call_id: new.call_id,
            tool_id: new.tool_id,
            session_id: new.session_id,
            requested_by: new.requested_by,
            tenant_id: new.tenant_id,
            diff_preview: new.diff_preview,
            args_digest,
            nonce: nonce.clone(),
            requested_at: requested_at.to_rfc3339(),
            expires_at: expires_at.to_rfc3339(),
        };
        let (tx, rx) = oneshot::channel();
        let replaced = self
            .pending
            .lock()
            .expect("pending lock")
            .insert(id.clone(), Pending { request, tx });
        assert!(replaced.is_none(), "server-generated approval id collision");
        Ticket {
            id,
            nonce,
            rx,
            pending: Arc::downgrade(&self.pending),
            decisions: Arc::downgrade(&self.decisions),
            expires_at,
        }
    }

    /// The pending requests (oldest first) — the inbox surface the console renders.
    #[must_use]
    pub fn pending(&self) -> Vec<ApprovalRequest> {
        let mut v: Vec<ApprovalRequest> = self
            .pending
            .lock()
            .expect("pending lock")
            .values()
            .map(|p| p.request.clone())
            .collect();
        v.sort_by(|a, b| a.requested_at.cmp(&b.requested_at));
        v
    }

    #[must_use]
    pub fn pending_count(&self) -> usize {
        self.pending.lock().expect("pending lock").len()
    }

    /// Approve a pending request from a server-verified AOG approver. Returns
    /// `Ok(false)` if the id is unknown or already consumed.
    pub fn approve(&self, id: &str, actor: &VerifiedRequestContext) -> Result<bool, ApprovalError> {
        let Some((actor_id, actor_role, tenant_id)) = self.authenticate(id, actor)? else {
            return Ok(false);
        };
        Ok(self.resolve(id, true, actor_id, actor_role, tenant_id, None))
    }

    /// Deny a pending request: receipt the decision, release the call as blocked.
    /// Returns `false` if the id is unknown.
    pub fn deny(
        &self,
        id: &str,
        actor: &VerifiedRequestContext,
        reason: &str,
    ) -> Result<bool, ApprovalError> {
        let Some((actor_id, actor_role, tenant_id)) = self.authenticate(id, actor)? else {
            return Ok(false);
        };
        Ok(self.resolve(
            id,
            false,
            actor_id,
            actor_role,
            tenant_id,
            Some(reason.to_string()),
        ))
    }

    fn authenticate(
        &self,
        id: &str,
        actor: &VerifiedRequestContext,
    ) -> Result<Option<(String, String, String)>, ApprovalError> {
        if actor
            .require_operation(RequestOperation::AogUpdate)
            .is_err()
        {
            return Err(ApprovalError::WrongOperation);
        }
        if actor.resource().kind() != "approval" || actor.resource().name() != id {
            return Err(ApprovalError::WrongResource);
        }
        let pending = self.pending.lock().expect("pending lock");
        let Some(request) = pending.get(id).map(|pending| &pending.request) else {
            return Ok(None);
        };
        let principal = actor.principal();
        if principal.tenant_id != request.tenant_id {
            return Err(ApprovalError::WrongTenant);
        }
        if !principal.roles.iter().any(|role| role == APPROVER_ROLE) {
            return Err(ApprovalError::MissingRole);
        }
        Ok(Some((
            principal.principal_id.clone(),
            APPROVER_ROLE.to_string(),
            principal.tenant_id.clone(),
        )))
    }

    fn resolve(
        &self,
        id: &str,
        approve: bool,
        actor: String,
        actor_role: String,
        tenant_id: String,
        reason: Option<String>,
    ) -> bool {
        let Some(p) = self.pending.lock().expect("pending lock").remove(id) else {
            return false;
        };
        let at = now();
        let expired = chrono::DateTime::parse_from_rfc3339(&p.request.expires_at)
            .map_or(true, |expires| expires <= chrono::Utc::now());
        let decision = if approve && !expired {
            Decision::Approved {
                grant: ApprovalGrant {
                    approval_id: p.request.id.clone(),
                    actor_id: actor.clone(),
                    actor_role: actor_role.clone(),
                    tenant_id: tenant_id.clone(),
                    call_id: p.request.call_id.clone(),
                    args_digest: p.request.args_digest.clone(),
                    nonce: p.request.nonce.clone(),
                    expires_at: p.request.expires_at.clone(),
                },
                at: at.clone(),
            }
        } else {
            Decision::Denied {
                actor: if expired {
                    "system".to_string()
                } else {
                    actor.clone()
                },
                actor_role: if expired {
                    "system".to_string()
                } else {
                    actor_role.clone()
                },
                tenant_id: tenant_id.clone(),
                reason: if expired {
                    "approval expired".to_string()
                } else {
                    reason.unwrap_or_else(|| "approval denied".to_string())
                },
                at: at.clone(),
            }
        };
        let (approved, receipt_actor, receipt_role, receipt_reason) = match &decision {
            Decision::Approved { grant, .. } => {
                (true, grant.actor_id.clone(), grant.actor_role.clone(), None)
            }
            Decision::Denied {
                actor,
                actor_role,
                reason,
                ..
            } => (
                false,
                actor.clone(),
                actor_role.clone(),
                Some(reason.clone()),
            ),
        };
        self.decisions
            .lock()
            .expect("decisions lock")
            .append(DecisionReceipt {
                approval_id: p.request.id.clone(),
                call_id: p.request.call_id.clone(),
                tool_id: p.request.tool_id.clone(),
                actor: receipt_actor,
                actor_role: receipt_role,
                tenant_id: p.request.tenant_id.clone(),
                approved,
                args_digest: p.request.args_digest.clone(),
                nonce: p.request.nonce.clone(),
                expires_at: p.request.expires_at.clone(),
                reason: receipt_reason,
                at,
            });
        // The waiting call may have dropped its ticket; the decision is receipted
        // regardless.
        let _ = p.tx.send(decision);
        true
    }

    /// A snapshot of the decision receipts (for the audit surface).
    #[must_use]
    pub fn decisions(&self) -> Vec<DecisionReceipt> {
        self.decisions
            .lock()
            .expect("decisions lock")
            .receipts
            .clone()
    }

    /// The decision-chain head (hex).
    #[must_use]
    pub fn decision_head(&self) -> String {
        hex::encode(self.decisions.lock().expect("decisions lock").last_hash)
    }

    /// Verify the decision chain end-to-end.
    #[must_use]
    pub fn verify_decisions(&self) -> bool {
        verify_chain(&self.decisions.lock().expect("decisions lock").links).is_ok()
    }
}

/// A handle to a pending approval; `await` [`wait`](Ticket::wait) for the decision.
pub struct Ticket {
    pub id: String,
    nonce: String,
    rx: oneshot::Receiver<Decision>,
    pending: Weak<Mutex<HashMap<String, Pending>>>,
    decisions: Weak<Mutex<DecisionChain>>,
    expires_at: chrono::DateTime<chrono::Utc>,
}

impl Ticket {
    /// Block until a human approves or denies. If the inbox is dropped, the call is
    /// treated as **denied** (fail-closed) — a mutating call never proceeds without
    /// a positive decision.
    pub async fn wait(self) -> Decision {
        let Ticket {
            id,
            nonce,
            rx,
            pending,
            decisions,
            expires_at,
        } = self;
        let remaining = (expires_at - chrono::Utc::now())
            .to_std()
            .unwrap_or(Duration::ZERO);
        match tokio::time::timeout(remaining, rx).await {
            Ok(Ok(decision)) => decision,
            Ok(Err(_)) => Decision::Denied {
                actor: "system".to_string(),
                actor_role: "system".to_string(),
                tenant_id: String::new(),
                reason: "approval inbox closed".to_string(),
                at: now(),
            },
            Err(_) => {
                let expired = pending.upgrade().and_then(|pending| {
                    let mut pending = pending.lock().expect("pending lock");
                    if pending
                        .get(&id)
                        .is_some_and(|item| item.request.nonce == nonce)
                    {
                        pending.remove(&id)
                    } else {
                        None
                    }
                });
                if let Some(p) = expired {
                    let at = now();
                    if let Some(decisions) = decisions.upgrade() {
                        decisions
                            .lock()
                            .expect("decisions lock")
                            .append(DecisionReceipt {
                                approval_id: p.request.id.clone(),
                                call_id: p.request.call_id.clone(),
                                tool_id: p.request.tool_id.clone(),
                                actor: "system".to_string(),
                                actor_role: "system".to_string(),
                                tenant_id: p.request.tenant_id.clone(),
                                approved: false,
                                args_digest: p.request.args_digest.clone(),
                                nonce: p.request.nonce.clone(),
                                expires_at: p.request.expires_at.clone(),
                                reason: Some("approval expired".to_string()),
                                at: at.clone(),
                            });
                    }
                    Decision::Denied {
                        actor: "system".to_string(),
                        actor_role: "system".to_string(),
                        tenant_id: p.request.tenant_id,
                        reason: "approval expired".to_string(),
                        at,
                    }
                } else {
                    Decision::Denied {
                        actor: "system".to_string(),
                        actor_role: "system".to_string(),
                        tenant_id: String::new(),
                        reason: "approval no longer pending".to_string(),
                        at: now(),
                    }
                }
            }
        }
    }
}

fn now() -> String {
    chrono::Utc::now().to_rfc3339()
}

/// Adapts the [`ApprovalInbox`] to the tool proxy's [`ApprovalGate`] seam (T3): a
/// side-effecting call reviewed here **blocks** on the inbox until a human decides,
/// then proceeds (approved — returning the approver) or is refused (denied). Share
/// the inner `Arc` with the approving side (the console operator).
pub struct InboxGate(pub Arc<ApprovalInbox>);

#[async_trait]
impl ApprovalGate for InboxGate {
    async fn review(
        &self,
        _tool: &ToolDefinition,
        call: &ToolCall,
        ctx: &InvokeContext,
        diff_preview: &str,
    ) -> Result<ApprovalGrant, String> {
        let tenant_id = ctx
            .tenant_id()
            .ok_or_else(|| "approval requires authenticated tenant context".to_string())?;
        let ticket = self.0.submit(NewApprovalRequest {
            call_id: call.call_id.clone(),
            tool_id: call.tool_id.clone(),
            session_id: ctx.session_id.clone(),
            requested_by: ctx.profile_id.clone(),
            tenant_id: tenant_id.to_string(),
            diff_preview: diff_preview.to_string(),
            arguments: call.arguments.clone(),
        });
        match ticket.wait().await {
            Decision::Approved { grant, .. } => Ok(grant),
            Decision::Denied { reason, .. } => Err(reason),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use fabric_contracts::{
        Audience, AuthStrength, AuthenticatedFacts, CanonicalResource, IdentityKind,
        RequestOperation, VerifiedRequestContext, WsfPrincipal,
    };

    use super::*;

    fn req(call_id: &str) -> NewApprovalRequest {
        NewApprovalRequest {
            call_id: call_id.to_string(),
            tool_id: "write.record".to_string(),
            session_id: "s1".to_string(),
            requested_by: "tok_1".to_string(),
            tenant_id: "tenant-a".to_string(),
            diff_preview: "SET lights = on".to_string(),
            arguments: serde_json::json!({"value": 1}),
        }
    }

    fn actor(
        id: &str,
        principal_id: &str,
        tenant: &str,
        roles: Vec<&str>,
    ) -> VerifiedRequestContext {
        let principal = WsfPrincipal::establish(
            AuthenticatedFacts {
                principal_id: principal_id.to_string(),
                kind: IdentityKind::Human,
                tenant_id: tenant.to_string(),
                subject_hash: "subject".to_string(),
                service_identity: None,
                roles: roles.into_iter().map(str::to_string).collect(),
                token_lineage: Some("root-approver".to_string()),
                auth_strength: AuthStrength::MutualTls,
                audience: Audience::Aog,
            },
            format!("corr-{principal_id}"),
            now(),
        );
        VerifiedRequestContext::establish(
            principal,
            RequestOperation::AogUpdate,
            CanonicalResource::resolved("approval", id, Some(tenant.to_string())).unwrap(),
        )
        .unwrap()
    }

    fn call_context() -> VerifiedRequestContext {
        let principal = WsfPrincipal::establish(
            AuthenticatedFacts {
                principal_id: "tok_1".to_string(),
                kind: IdentityKind::Workload,
                tenant_id: "tenant-a".to_string(),
                subject_hash: String::new(),
                service_identity: Some("tool-runner".to_string()),
                roles: vec![],
                token_lineage: Some("root-call".to_string()),
                auth_strength: AuthStrength::WorkloadToken,
                audience: Audience::Aog,
            },
            "corr-call",
            now(),
        );
        VerifiedRequestContext::establish(
            principal,
            RequestOperation::AogUpdate,
            CanonicalResource::resolved("system", "write.record", Some("tenant-a".to_string()))
                .unwrap(),
        )
        .unwrap()
    }

    #[tokio::test]
    async fn mutating_call_blocks_until_approved_and_is_receipted() {
        let inbox = Arc::new(ApprovalInbox::new());
        let ticket = inbox.submit(req("c1"));
        assert_eq!(
            inbox.pending_count(),
            1,
            "the call is pending, not executed"
        );

        // The waiting call resolves only once a human approves.
        let id = ticket.id.clone();
        let waiter = tokio::spawn(async move { ticket.wait().await });
        let alice = actor(&id, "alice", "tenant-a", vec![APPROVER_ROLE]);
        assert!(inbox.approve(&id, &alice).unwrap());
        let decision = waiter.await.unwrap();

        assert!(decision.is_approved());
        assert_eq!(decision.actor(), "alice");
        // approval + actor receipted; chain verifies; no longer pending.
        let d = &inbox.decisions()[0];
        assert_eq!(d.actor, "alice");
        assert!(d.approved);
        assert_eq!(d.call_id, "c1");
        assert!(inbox.verify_decisions());
        assert_eq!(inbox.pending_count(), 0);
    }

    #[tokio::test]
    async fn denial_blocks_the_call_and_receipts_the_reason() {
        let inbox = ApprovalInbox::new();
        let ticket = inbox.submit(req("c1"));
        let bob = actor(&ticket.id, "bob", "tenant-a", vec![APPROVER_ROLE]);
        assert!(
            inbox
                .deny(&ticket.id, &bob, "not authorized for this system")
                .unwrap()
        );
        let decision = ticket.wait().await;

        assert!(!decision.is_approved(), "a denied call does not proceed");
        let d = &inbox.decisions()[0];
        assert!(!d.approved);
        assert_eq!(d.actor, "bob");
        assert_eq!(d.reason.as_deref(), Some("not authorized for this system"));
        assert!(inbox.verify_decisions());
    }

    #[tokio::test]
    async fn unknown_id_is_a_noop() {
        let inbox = ApprovalInbox::new();
        let alice = actor("ghost", "alice", "tenant-a", vec![APPROVER_ROLE]);
        assert!(!inbox.approve("ghost", &alice).unwrap());
        assert!(inbox.decisions().is_empty());
    }

    #[tokio::test]
    async fn dropped_inbox_fails_closed_to_denied() {
        let inbox = ApprovalInbox::new();
        let ticket = inbox.submit(req("c1"));
        drop(inbox); // the pending sender is dropped with the inbox
        let decision = ticket.wait().await;
        assert!(
            !decision.is_approved(),
            "a dropped inbox denies (fail-closed) — the call never proceeds"
        );
    }

    #[tokio::test]
    async fn reg_lsh_t5_authenticated_single_use_approval_decisions() {
        let inbox = ApprovalInbox::with_ttl(Duration::from_millis(30));
        let first = inbox.submit(req("same-call"));
        let second = inbox.submit(req("same-call"));
        assert_ne!(first.id, second.id, "server ids include a unique nonce");
        assert_eq!(
            inbox.pending_count(),
            2,
            "duplicate call ids cannot overwrite pending approvals"
        );

        let no_role = actor(&first.id, "mallory", "tenant-a", vec![]);
        assert_eq!(
            inbox.approve(&first.id, &no_role),
            Err(ApprovalError::MissingRole)
        );
        let wrong_tenant = actor(&first.id, "mallory", "tenant-b", vec![APPROVER_ROLE]);
        assert_eq!(
            inbox.approve(&first.id, &wrong_tenant),
            Err(ApprovalError::WrongTenant)
        );

        let alice = actor(&first.id, "alice", "tenant-a", vec![APPROVER_ROLE]);
        assert!(inbox.approve(&first.id, &alice).unwrap());
        assert!(
            !inbox.approve(&first.id, &alice).unwrap(),
            "a consumed approval id cannot be replayed"
        );
        let approved = first.wait().await;
        let Decision::Approved { grant, .. } = approved else {
            panic!("expected authenticated approval")
        };
        assert_eq!(grant.actor_id, "alice");
        assert_eq!(grant.actor_role, APPROVER_ROLE);
        assert_eq!(grant.tenant_id, "tenant-a");
        assert_eq!(grant.call_id, "same-call");
        assert!(!grant.args_digest.is_empty());
        assert!(!grant.nonce.is_empty());

        let second_id = second.id.clone();
        let late_actor = actor(&second_id, "alice", "tenant-a", vec![APPROVER_ROLE]);
        let expired = second.wait().await;
        assert!(!expired.is_approved());
        assert!(
            !inbox.approve(&second_id, &late_actor).unwrap(),
            "an expired decision is removed and cannot be replayed"
        );
        let receipts = inbox.decisions();
        assert_eq!(receipts.len(), 2);
        assert!(receipts[0].approved);
        assert!(!receipts[1].approved);
        assert_eq!(receipts[1].reason.as_deref(), Some("approval expired"));
        assert!(inbox.verify_decisions());
    }

    #[tokio::test]
    async fn proxy_routes_a_mutating_call_through_the_real_inbox() {
        use std::time::Duration;

        use aog_toolproxy::{MintedCredential, ToolExecutor, ToolProxy};
        use mai_agent::types::{ToolAccessRole, ToolResult};

        struct Exec;
        #[async_trait]
        impl ToolExecutor for Exec {
            async fn execute(
                &self,
                _t: &ToolDefinition,
                c: &ToolCall,
                _cred: Option<&MintedCredential>,
            ) -> ToolResult {
                ToolResult {
                    call_id: c.call_id.clone(),
                    tool_id: c.tool_id.clone(),
                    success: true,
                    output: serde_json::Value::Null,
                    error: None,
                    duration_ms: 1,
                }
            }
        }

        let inbox = Arc::new(ApprovalInbox::new());
        let proxy =
            Arc::new(ToolProxy::new().with_approvals(Box::new(InboxGate(Arc::clone(&inbox)))));
        proxy
            .register(ToolDefinition {
                id: "write.record".to_string(),
                name: "write".to_string(),
                description: "w".to_string(),
                parameters_schema: serde_json::json!({ "type": "object" }),
                return_schema: None,
                has_side_effects: true,
                timeout: Duration::from_secs(5),
                required_role: ToolAccessRole::Parent,
                supports_parallel: false,
            })
            .unwrap();

        // Drive the mutating call on a task; it blocks in the inbox until approval.
        let p = Arc::clone(&proxy);
        let call_task = tokio::spawn(async move {
            let call = ToolCall {
                call_id: "c1".to_string(),
                tool_id: "write.record".to_string(),
                arguments: serde_json::json!({ "v": 1 }),
                chain_step: 0,
                parallel_group: None,
            };
            let verified = call_context();
            let ctx = InvokeContext::from_verified_request("s1", ToolAccessRole::Parent, &verified);
            p.invoke(&call, &ctx, &Exec).await
        });

        // The human approves the pending request; the blocked call then proceeds.
        while inbox.pending_count() == 0 {
            tokio::task::yield_now().await;
        }
        let pending = inbox.pending().pop().unwrap();
        let carol = actor(&pending.id, "carol", "tenant-a", vec![APPROVER_ROLE]);
        assert!(inbox.approve(&pending.id, &carol).unwrap());

        let result = call_task.await.unwrap().unwrap();
        assert!(result.success, "the approved mutating call executed");
        // Both audit trails recorded it: the inbox decision + the proxy tool receipt.
        assert_eq!(inbox.decisions()[0].actor, "carol");
        assert!(inbox.verify_decisions());
        assert!(proxy.receipts()[0].approval_required);
        assert_eq!(proxy.receipts()[0].approved_by.as_deref(), Some("carol"));
        assert_eq!(
            proxy.receipts()[0]
                .approval
                .as_ref()
                .map(|approval| approval.args_digest.as_str()),
            Some(pending.args_digest.as_str())
        );
    }
}
