//! `aog-toolproxy` (T1) — the governed tool-execution proxy.
//!
//! Agents register tools here (reusing mai-agent's `ToolRegistry` + tool types);
//! tool calls execute **through** the proxy, never direct. Every invocation is
//! validated (tool exists, caller role suffices, argument shape), executed via a
//! pluggable [`ToolExecutor`], and **receipted** into a `fabric-proof` chain (plus
//! the metadata for a mai-agent-style audit trail).
//!
//! This fills the seam mai-agent left open ("tool execution happens in L4, not in
//! the MAI") with AOG governance. The later T-phase prompts layer on this one
//! `invoke` path: T2 mints per-call credentials inside the executor boundary, T3
//! gates a side-effecting call on human approval, T4 tags tool results untrusted so
//! injected instructions can't auto-trigger a mutating call.

pub mod receipt;

use std::sync::{Mutex, RwLock};

use async_trait::async_trait;
use mai_agent::ToolRegistry;
use mai_agent::types::{AgentError, ToolAccessRole, ToolCall, ToolDefinition, ToolResult};

use crate::receipt::{ToolReceipt, ToolReceiptChain};

/// A tool call the proxy refused to broker.
#[derive(Debug, thiserror::Error)]
pub enum ProxyError {
    /// The registry rejected the call (unknown tool, role denied, bad arguments).
    #[error("tool governance: {0}")]
    Rejected(#[from] AgentError),
}

/// The seam an agent / L4 implements to actually run a validated tool call. AOG
/// owns the path around it — validate before, receipt after — so a tool never runs
/// un-governed. T2 mints the call's ephemeral credentials at this boundary.
#[async_trait]
pub trait ToolExecutor: Send + Sync {
    /// Execute a validated call. Implementations should honour `tool.timeout` and
    /// return a [`ToolResult`] (success or a captured error) rather than panicking.
    async fn execute(&self, tool: &ToolDefinition, call: &ToolCall) -> ToolResult;
}

/// Who/what authorized a tool call — recorded on the receipt (never the credential).
#[derive(Debug, Clone)]
pub struct InvokeContext {
    pub session_id: String,
    /// The authorizing subject / trust-token id.
    pub profile_id: String,
    /// The caller's role (checked against the tool's `required_role`).
    pub role: ToolAccessRole,
}

/// The governed tool proxy. Cheap to share behind an `Arc` — registration and
/// invocation both take `&self` (the registry + receipt chain lock internally).
pub struct ToolProxy {
    registry: RwLock<ToolRegistry>,
    receipts: Mutex<ToolReceiptChain>,
}

impl Default for ToolProxy {
    fn default() -> Self {
        Self::new()
    }
}

impl ToolProxy {
    #[must_use]
    pub fn new() -> Self {
        Self {
            registry: RwLock::new(ToolRegistry::new()),
            receipts: Mutex::new(ToolReceiptChain::new()),
        }
    }

    /// Register a tool the proxy will broker.
    ///
    /// # Errors
    /// [`ProxyError::Rejected`] if the tool's `parameters_schema` is not an object.
    pub fn register(&self, tool: ToolDefinition) -> Result<(), ProxyError> {
        self.registry
            .write()
            .expect("registry lock")
            .register_tool(tool)
            .map_err(ProxyError::from)
    }

    /// Number of registered tools.
    #[must_use]
    pub fn tool_count(&self) -> usize {
        self.registry.read().expect("registry lock").tool_count()
    }

    /// Invoke a tool **through** the proxy: validate (exists + role + argument
    /// shape), execute via `executor`, then receipt the call (metadata-only,
    /// chained). The receipt is recorded whether the tool succeeds or fails; a call
    /// that fails *validation* never executes and is not receipted.
    ///
    /// # Errors
    /// [`ProxyError::Rejected`] if the call fails validation.
    pub async fn invoke(
        &self,
        call: &ToolCall,
        ctx: &InvokeContext,
        executor: &dyn ToolExecutor,
    ) -> Result<ToolResult, ProxyError> {
        // Validate and clone the definition so the registry lock is not held across
        // the executor's `.await`.
        let tool = {
            let reg = self.registry.read().expect("registry lock");
            reg.validate_tool_call(call, &ctx.role)?.clone()
        };

        let result = executor.execute(&tool, call).await;

        self.receipts
            .lock()
            .expect("receipt lock")
            .append(ToolReceipt {
                call_id: call.call_id.clone(),
                tool_id: call.tool_id.clone(),
                session_id: ctx.session_id.clone(),
                profile_id: ctx.profile_id.clone(),
                has_side_effects: tool.has_side_effects,
                success: result.success,
                duration_ms: result.duration_ms,
                chain_step: call.chain_step,
                at: chrono::Utc::now().to_rfc3339(),
                error: result.error.clone(),
            });
        Ok(result)
    }

    /// The receipt-chain head (hex).
    #[must_use]
    pub fn receipt_head(&self) -> String {
        self.receipts.lock().expect("receipt lock").head_hex()
    }

    /// Number of receipted calls.
    #[must_use]
    pub fn receipt_count(&self) -> usize {
        self.receipts.lock().expect("receipt lock").len()
    }

    /// Verify the receipt chain end-to-end.
    #[must_use]
    pub fn verify_receipts(&self) -> bool {
        self.receipts.lock().expect("receipt lock").verify()
    }

    /// A snapshot of the receipts (for a governance surface / audit query).
    #[must_use]
    pub fn receipts(&self) -> Vec<ToolReceipt> {
        self.receipts
            .lock()
            .expect("receipt lock")
            .receipts()
            .to_vec()
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use mai_agent::types::{ToolAccessRole, ToolCall, ToolDefinition, ToolResult};

    use super::*;

    /// An executor that echoes the call's arguments back as a successful result.
    struct EchoExecutor;

    #[async_trait]
    impl ToolExecutor for EchoExecutor {
        async fn execute(&self, _tool: &ToolDefinition, call: &ToolCall) -> ToolResult {
            ToolResult {
                call_id: call.call_id.clone(),
                tool_id: call.tool_id.clone(),
                success: true,
                output: call.arguments.clone(),
                error: None,
                duration_ms: 1,
            }
        }
    }

    fn tool(id: &str, side_effects: bool, role: ToolAccessRole) -> ToolDefinition {
        ToolDefinition {
            id: id.to_string(),
            name: id.to_string(),
            description: "test tool".to_string(),
            parameters_schema: serde_json::json!({ "type": "object" }),
            return_schema: None,
            has_side_effects: side_effects,
            timeout: Duration::from_secs(5),
            required_role: role,
            supports_parallel: false,
        }
    }

    fn call(call_id: &str, tool_id: &str) -> ToolCall {
        ToolCall {
            call_id: call_id.to_string(),
            tool_id: tool_id.to_string(),
            arguments: serde_json::json!({ "x": 1 }),
            chain_step: 0,
            parallel_group: None,
        }
    }

    fn ctx(role: ToolAccessRole) -> InvokeContext {
        InvokeContext {
            session_id: "s1".to_string(),
            profile_id: "tok_1".to_string(),
            role,
        }
    }

    #[tokio::test]
    async fn invoked_tool_executes_and_is_receipted() {
        let proxy = ToolProxy::new();
        proxy
            .register(tool("read.file", false, ToolAccessRole::Guest))
            .unwrap();
        assert_eq!(proxy.receipt_count(), 0);

        let r = proxy
            .invoke(
                &call("c1", "read.file"),
                &ctx(ToolAccessRole::Guest),
                &EchoExecutor,
            )
            .await
            .unwrap();
        assert!(r.success);
        assert_eq!(r.output, serde_json::json!({ "x": 1 }));

        // The gate: the call appears in the receipt ledger, chained + verifiable.
        assert_eq!(proxy.receipt_count(), 1);
        let rec = &proxy.receipts()[0];
        assert_eq!(rec.tool_id, "read.file");
        assert_eq!(rec.call_id, "c1");
        assert_eq!(rec.profile_id, "tok_1");
        assert!(!rec.has_side_effects);
        assert!(rec.success);
        assert!(proxy.verify_receipts());
        assert_ne!(
            proxy.receipt_head(),
            hex::encode([0u8; 32]),
            "head advanced past genesis"
        );
    }

    #[tokio::test]
    async fn unregistered_tool_is_rejected_before_execution() {
        let proxy = ToolProxy::new();
        let err = proxy
            .invoke(
                &call("c1", "ghost"),
                &ctx(ToolAccessRole::Admin),
                &EchoExecutor,
            )
            .await
            .unwrap_err();
        assert!(matches!(err, ProxyError::Rejected(_)));
        assert_eq!(
            proxy.receipt_count(),
            0,
            "a call that fails validation never executes or receipts"
        );
    }

    #[tokio::test]
    async fn role_below_required_is_denied_and_not_receipted() {
        let proxy = ToolProxy::new();
        proxy
            .register(tool("admin.wipe", true, ToolAccessRole::Admin))
            .unwrap();
        let err = proxy
            .invoke(
                &call("c1", "admin.wipe"),
                &ctx(ToolAccessRole::Guest),
                &EchoExecutor,
            )
            .await
            .unwrap_err();
        assert!(matches!(err, ProxyError::Rejected(_)));
        assert_eq!(proxy.receipt_count(), 0);
    }

    #[tokio::test]
    async fn receipt_chain_links_multiple_calls() {
        let proxy = ToolProxy::new();
        proxy
            .register(tool("read.file", false, ToolAccessRole::Guest))
            .unwrap();
        proxy
            .invoke(
                &call("c1", "read.file"),
                &ctx(ToolAccessRole::Guest),
                &EchoExecutor,
            )
            .await
            .unwrap();
        let head1 = proxy.receipt_head();
        proxy
            .invoke(
                &call("c2", "read.file"),
                &ctx(ToolAccessRole::Guest),
                &EchoExecutor,
            )
            .await
            .unwrap();
        let head2 = proxy.receipt_head();
        assert_ne!(head1, head2, "each call advances the chain");
        assert_eq!(proxy.receipt_count(), 2);
        assert!(proxy.verify_receipts());
    }
}
