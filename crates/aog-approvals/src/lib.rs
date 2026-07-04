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
use std::sync::{Arc, Mutex};

use aog_toolproxy::{ApprovalGate, InvokeContext};
use async_trait::async_trait;
use fabric_proof::{ChainLink, GENESIS_HASH, canonical_hash, chain_link, verify_chain};
use mai_agent::types::{ToolCall, ToolDefinition};
use serde::Serialize;
use tokio::sync::oneshot;

/// A side-effecting tool call awaiting a human decision.
#[derive(Debug, Clone, Serialize)]
pub struct ApprovalRequest {
    pub id: String,
    pub call_id: String,
    pub tool_id: String,
    pub session_id: String,
    /// The authorizing subject / trust-token id that initiated the call.
    pub requested_by: String,
    /// Human-readable preview of what the mutating call will do.
    pub diff_preview: String,
    pub requested_at: String,
}

/// A human decision on a pending request.
#[derive(Debug, Clone)]
pub enum Decision {
    Approved {
        actor: String,
        at: String,
    },
    Denied {
        actor: String,
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
            Decision::Approved { actor, .. } | Decision::Denied { actor, .. } => actor,
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
    pub approved: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    pub at: String,
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
    pending: Mutex<HashMap<String, Pending>>,
    decisions: Mutex<DecisionChain>,
}

impl Default for ApprovalInbox {
    fn default() -> Self {
        Self::new()
    }
}

impl ApprovalInbox {
    #[must_use]
    pub fn new() -> Self {
        Self {
            pending: Mutex::new(HashMap::new()),
            decisions: Mutex::new(DecisionChain::new()),
        }
    }

    /// Submit a side-effecting call for approval; returns a [`Ticket`] the caller
    /// awaits. The call must not proceed until the ticket resolves.
    #[must_use]
    pub fn submit(&self, request: ApprovalRequest) -> Ticket {
        let (tx, rx) = oneshot::channel();
        let id = request.id.clone();
        self.pending
            .lock()
            .expect("pending lock")
            .insert(id.clone(), Pending { request, tx });
        Ticket { id, rx }
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

    /// Approve a pending request: receipt the decision + actor, release the waiting
    /// call. Returns `false` if the id is unknown.
    pub fn approve(&self, id: &str, actor: &str) -> bool {
        self.resolve(
            id,
            Decision::Approved {
                actor: actor.to_string(),
                at: now(),
            },
        )
    }

    /// Deny a pending request: receipt the decision, release the call as blocked.
    /// Returns `false` if the id is unknown.
    pub fn deny(&self, id: &str, actor: &str, reason: &str) -> bool {
        self.resolve(
            id,
            Decision::Denied {
                actor: actor.to_string(),
                reason: reason.to_string(),
                at: now(),
            },
        )
    }

    fn resolve(&self, id: &str, decision: Decision) -> bool {
        let Some(p) = self.pending.lock().expect("pending lock").remove(id) else {
            return false;
        };
        let (approved, reason, at) = match &decision {
            Decision::Approved { at, .. } => (true, None, at.clone()),
            Decision::Denied { reason, at, .. } => (false, Some(reason.clone()), at.clone()),
        };
        self.decisions
            .lock()
            .expect("decisions lock")
            .append(DecisionReceipt {
                approval_id: p.request.id.clone(),
                call_id: p.request.call_id.clone(),
                tool_id: p.request.tool_id.clone(),
                actor: decision.actor().to_string(),
                approved,
                reason,
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
    rx: oneshot::Receiver<Decision>,
}

impl Ticket {
    /// Block until a human approves or denies. If the inbox is dropped, the call is
    /// treated as **denied** (fail-closed) — a mutating call never proceeds without
    /// a positive decision.
    pub async fn wait(self) -> Decision {
        self.rx.await.unwrap_or_else(|_| Decision::Denied {
            actor: "system".to_string(),
            reason: "approval inbox closed".to_string(),
            at: now(),
        })
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
    ) -> Result<String, String> {
        let ticket = self.0.submit(ApprovalRequest {
            id: call.call_id.clone(),
            call_id: call.call_id.clone(),
            tool_id: call.tool_id.clone(),
            session_id: ctx.session_id.clone(),
            requested_by: ctx.profile_id.clone(),
            diff_preview: diff_preview.to_string(),
            requested_at: now(),
        });
        match ticket.wait().await {
            Decision::Approved { actor, .. } => Ok(actor),
            Decision::Denied { reason, .. } => Err(reason),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;

    fn req(id: &str) -> ApprovalRequest {
        ApprovalRequest {
            id: id.to_string(),
            call_id: "c1".to_string(),
            tool_id: "write.record".to_string(),
            session_id: "s1".to_string(),
            requested_by: "tok_1".to_string(),
            diff_preview: "SET lights = on".to_string(),
            requested_at: "2026-07-03T00:00:00Z".to_string(),
        }
    }

    #[tokio::test]
    async fn mutating_call_blocks_until_approved_and_is_receipted() {
        let inbox = Arc::new(ApprovalInbox::new());
        let ticket = inbox.submit(req("a1"));
        assert_eq!(
            inbox.pending_count(),
            1,
            "the call is pending, not executed"
        );

        // The waiting call resolves only once a human approves.
        let id = ticket.id.clone();
        let waiter = tokio::spawn(async move { ticket.wait().await });
        assert!(inbox.approve(&id, "alice"));
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
        let ticket = inbox.submit(req("a1"));
        assert!(inbox.deny("a1", "bob", "not authorized for this system"));
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
        assert!(!inbox.approve("ghost", "alice"));
        assert!(inbox.decisions().is_empty());
    }

    #[tokio::test]
    async fn dropped_inbox_fails_closed_to_denied() {
        let inbox = ApprovalInbox::new();
        let ticket = inbox.submit(req("a1"));
        drop(inbox); // the pending sender is dropped with the inbox
        let decision = ticket.wait().await;
        assert!(
            !decision.is_approved(),
            "a dropped inbox denies (fail-closed) — the call never proceeds"
        );
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
            let ctx = InvokeContext {
                session_id: "s1".to_string(),
                profile_id: "tok_1".to_string(),
                role: ToolAccessRole::Parent,
                untrusted: false,
            };
            p.invoke(&call, &ctx, &Exec).await
        });

        // The human approves the pending request; the blocked call then proceeds.
        while inbox.pending_count() == 0 {
            tokio::task::yield_now().await;
        }
        assert!(inbox.approve("c1", "carol"));

        let result = call_task.await.unwrap().unwrap();
        assert!(result.success, "the approved mutating call executed");
        // Both audit trails recorded it: the inbox decision + the proxy tool receipt.
        assert_eq!(inbox.decisions()[0].actor, "carol");
        assert!(inbox.verify_decisions());
        assert!(proxy.receipts()[0].approval_required);
        assert_eq!(proxy.receipts()[0].approved_by.as_deref(), Some("carol"));
    }
}
