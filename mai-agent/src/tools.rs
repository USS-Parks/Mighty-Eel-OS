//! Tool Calling / Function Calling Interface for the MAI.
//!
//! Manages the tool registry, validates tool calls from model output,
//! tracks multi-step chains, handles parallel tool dispatch, and
//! produces audit entries for every tool invocation.
//!
//! # Architecture
//!
//! L4 registers tool definitions with the MAI. When model output contains
//! tool calls, the MAI validates them, checks permissions, and returns
//! structured ToolCall objects. L4 executes the tools and returns ToolResult
//! objects. The MAI injects results into the conversation context and
//! continues the chain if the model requests more tool calls.
//!
//! # Air-Gap Safety
//!
//! All tool definitions are local. No external tool registries.
//! Tool execution happens in L4, not in the MAI.

use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value as JsonValue;
use tracing::{debug, info, warn};
use mai_core::types::RequestId;

use crate::types::{
    AgentError, SessionId, ToolAccessRole, ToolAuditEntry, ToolCall,
    ToolChain, ToolChainState, ToolDefinition, ToolId, ToolResult,
};

// ============================================================================
// Tool Registry
// ============================================================================

/// Registry of available tools that L4 components have registered.
///
/// Thread safety: NOT internally synchronized. Wrap in Arc<RwLock<_>>.
pub struct ToolRegistry {
    /// Registered tools indexed by tool ID
    tools: HashMap<ToolId, ToolDefinition>,
    /// Active tool chains indexed by request ID
    chains: HashMap<RequestId, ToolChain>,
    /// Audit log for tool calls (in-memory, bounded)
    audit_log: Vec<ToolAuditEntry>,
    /// Maximum audit log entries before oldest are dropped
    max_audit_entries: usize,
    /// Maximum steps allowed in any tool chain
    global_max_chain_steps: u32,
}

impl ToolRegistry {
    /// Create a new tool registry.
    pub fn new() -> Self {
        Self {
            tools: HashMap::new(),
            chains: HashMap::new(),
            audit_log: Vec::new(),
            max_audit_entries: 10_000,
            global_max_chain_steps: 20,
        }
    }

    /// Register a tool definition. Overwrites if tool_id already exists.
    pub fn register_tool(&mut self, tool: ToolDefinition) -> Result<(), AgentError> {
        // Validate the parameters schema is a valid JSON Schema object
        if !tool.parameters_schema.is_object() {
            return Err(AgentError::InvalidToolArguments(format!(
                "Tool {} parameters_schema must be a JSON object",
                tool.id
            )));
        }

        info!(tool_id = %tool.id, name = %tool.name, "Registered tool");
        self.tools.insert(tool.id.clone(), tool);
        Ok(())
    }

    /// Unregister a tool by ID.
    pub fn unregister_tool(&mut self, tool_id: &str) -> bool {
        let removed = self.tools.remove(tool_id).is_some();
        if removed {
            info!(tool_id, "Unregistered tool");
        }
        removed
    }

    /// Get a tool definition by ID.
    pub fn get_tool(&self, tool_id: &str) -> Option<&ToolDefinition> {
        self.tools.get(tool_id)
    }

    /// List all registered tools.
    pub fn list_tools(&self) -> Vec<&ToolDefinition> {
        self.tools.values().collect()
    }

    /// List tools accessible by a given role.
    pub fn list_tools_for_role(&self, role: &ToolAccessRole) -> Vec<&ToolDefinition> {
        self.tools
            .values()
            .filter(|t| *role >= t.required_role)
            .collect()
    }

    /// Get tool count.
    pub fn tool_count(&self) -> usize {
        self.tools.len()
    }

    /// Generate tool definitions in the format expected by chat models
    /// (OpenAI-compatible function calling schema).
    pub fn tools_for_model(&self, role: &ToolAccessRole) -> Vec<JsonValue> {
        self.list_tools_for_role(role)
            .into_iter()
            .map(|t| {
                serde_json::json!({
                    "type": "function",
                    "function": {
                        "name": t.id,
                        "description": t.description,
                        "parameters": t.parameters_schema,
                    }
                })
            })
            .collect()
    }

    // ── Tool Call Validation ──────────────────────────────────────────

    /// Validate a tool call from model output against the registry.
    ///
    /// Checks: tool exists, role has access, arguments have correct shape.
    /// Does NOT execute the tool (that is L4's job).
    pub fn validate_tool_call(
        &self,
        call: &ToolCall,
        caller_role: &ToolAccessRole,
    ) -> Result<&ToolDefinition, AgentError> {
        let tool = self.tools.get(&call.tool_id).ok_or_else(|| {
            AgentError::ToolNotRegistered(call.tool_id.clone())
        })?;

        // Permission check
        if *caller_role < tool.required_role {
            return Err(AgentError::ToolAccessDenied {
                tool_id: call.tool_id.clone(),
                required: tool.required_role.clone(),
                actual: caller_role.clone(),
            });
        }

        // Validate arguments are a JSON object (basic shape check)
        if !call.arguments.is_object() && !call.arguments.is_null() {
            return Err(AgentError::InvalidToolArguments(format!(
                "Tool {} arguments must be a JSON object, got {}",
                call.tool_id,
                call.arguments
            )));
        }

        Ok(tool)
    }

    /// Validate a batch of parallel tool calls.
    pub fn validate_parallel_calls(
        &self,
        calls: &[ToolCall],
        caller_role: &ToolAccessRole,
    ) -> Result<(), AgentError> {
        for call in calls {
            let tool = self.validate_tool_call(call, caller_role)?;
            if !tool.supports_parallel {
                return Err(AgentError::InvalidToolArguments(format!(
                    "Tool {} does not support parallel calling",
                    call.tool_id
                )));
            }
        }
        Ok(())
    }

    // ── Tool Chain Management ─────────────────────────────────────────

    /// Start a new tool chain for a multi-step tool calling sequence.
    pub fn start_chain(
        &mut self,
        request_id: RequestId,
        session_id: SessionId,
        max_steps: Option<u32>,
    ) -> Result<(), AgentError> {
        let max = max_steps
            .unwrap_or(self.global_max_chain_steps)
            .min(self.global_max_chain_steps);

        let chain = ToolChain {
            request_id,
            session_id,
            completed_steps: Vec::new(),
            pending_calls: Vec::new(),
            state: ToolChainState::AwaitingModel,
            max_steps: max,
            tokens_consumed: 0,
        };

        self.chains.insert(request_id, chain);
        debug!(%request_id, max_steps = max, "Started tool chain");
        Ok(())
    }

    /// Submit tool calls detected in model output to the chain.
    ///
    /// Returns the validated calls for L4 to execute.
    pub fn submit_calls(
        &mut self,
        request_id: &RequestId,
        calls: Vec<ToolCall>,
        caller_role: &ToolAccessRole,
    ) -> Result<Vec<ToolCall>, AgentError> {
        // Validate all calls first (before mutable chain borrow)
        for call in &calls {
            self.validate_tool_call(call, caller_role)?;
        }

        // Check for parallel calls (before mutable chain borrow)
        let is_parallel = calls.len() > 1;
        if is_parallel {
            self.validate_parallel_calls(&calls, caller_role)?;
        }

        let chain = self.chains.get_mut(request_id).ok_or_else(|| {
            AgentError::Internal(format!("No active chain for request {request_id}"))
        })?;

        // Check step limit
        let next_step = chain.completed_steps.len() as u32 + 1;
        if next_step > chain.max_steps {
            chain.state = ToolChainState::Aborted {
                reason: format!("Exceeded max chain steps: {}", chain.max_steps),
            };
            return Err(AgentError::ChainStepLimitExceeded {
                max: chain.max_steps,
            });
        }

        chain.pending_calls = calls.clone();
        chain.state = ToolChainState::AwaitingExecution;

        debug!(
            %request_id,
            call_count = calls.len(),
            step = next_step,
            parallel = is_parallel,
            "Submitted tool calls"
        );

        Ok(calls)
    }

    /// Record results from L4 tool execution and decide chain next state.
    ///
    /// Returns true if the chain should continue (model needs to see results
    /// and potentially make more calls). Returns false if chain is complete.
    pub fn record_results(
        &mut self,
        request_id: &RequestId,
        results: Vec<ToolResult>,
        profile_id: &str,
    ) -> Result<bool, AgentError> {
        let chain = self.chains.get_mut(request_id).ok_or_else(|| {
            AgentError::Internal(format!("No active chain for request {request_id}"))
        })?;

        if chain.state != ToolChainState::AwaitingExecution {
            return Err(AgentError::Internal(format!(
                "Chain {request_id} not awaiting execution, state: {:?}",
                chain.state
            )));
        }

        // Match results to pending calls, collecting audits separately
        // to avoid conflicting borrows on self
        let now = now_epoch_secs();
        let mut pending_audits = Vec::new();
        for result in &results {
            // Find matching pending call
            let call_idx = chain
                .pending_calls
                .iter()
                .position(|c| c.call_id == result.call_id);

            if let Some(idx) = call_idx {
                let call = chain.pending_calls.remove(idx);

                // Build audit entry (deferred push)
                let audit = ToolAuditEntry {
                    timestamp: now,
                    profile_id: profile_id.to_string(),
                    tool_id: result.tool_id.clone(),
                    call_id: result.call_id.clone(),
                    success: result.success,
                    duration_ms: result.duration_ms,
                    chain_step: call.chain_step,
                    error: result.error.clone(),
                    session_id: chain.session_id.to_string(),
                };
                pending_audits.push(audit);

                chain.completed_steps.push((call, result.clone()));
            } else {
                warn!(
                    call_id = %result.call_id,
                    "Tool result for unknown call_id"
                );
            }
        }

        // Check if any calls failed
        let any_failed = results.iter().any(|r| !r.success);
        if any_failed {
            // Continue chain but model will see the error
            debug!(%request_id, "Some tool calls failed, continuing chain with error context");
        }

        // All pending calls resolved: back to model for next step
        let should_continue = if chain.pending_calls.is_empty() {
            chain.state = ToolChainState::AwaitingModel;
            true // Continue: model needs to see results
        } else {
            false // Still waiting for more results
        };

        // Now safe to push audits (chain borrow ended at last use above)
        for audit in pending_audits {
            self.push_audit(audit);
        }

        Ok(should_continue)
    }

    /// Mark a chain as complete (model generated final response, no more calls).
    pub fn complete_chain(&mut self, request_id: &RequestId) -> Result<(), AgentError> {
        let chain = self.chains.get_mut(request_id).ok_or_else(|| {
            AgentError::Internal(format!("No active chain for request {request_id}"))
        })?;
        chain.state = ToolChainState::Complete;
        info!(
            %request_id,
            steps = chain.completed_steps.len(),
            "Tool chain completed"
        );
        Ok(())
    }

    /// Abort a chain.
    pub fn abort_chain(&mut self, request_id: &RequestId, reason: String) -> Result<(), AgentError> {
        let chain = self.chains.get_mut(request_id).ok_or_else(|| {
            AgentError::Internal(format!("No active chain for request {request_id}"))
        })?;
        chain.state = ToolChainState::Aborted { reason: reason.clone() };
        warn!(%request_id, %reason, "Tool chain aborted");
        Ok(())
    }

    /// Get chain state.
    pub fn get_chain(&self, request_id: &RequestId) -> Option<&ToolChain> {
        self.chains.get(request_id)
    }

    /// Remove a completed/aborted chain (cleanup).
    pub fn remove_chain(&mut self, request_id: &RequestId) -> Option<ToolChain> {
        self.chains.remove(request_id)
    }

    /// Count active chains.
    pub fn active_chain_count(&self) -> usize {
        self.chains
            .values()
            .filter(|c| {
                matches!(
                    c.state,
                    ToolChainState::AwaitingModel | ToolChainState::AwaitingExecution
                )
            })
            .count()
    }

    // ── Audit ─────────────────────────────────────────────────────────

    /// Get recent audit entries.
    pub fn recent_audit(&self, count: usize) -> &[ToolAuditEntry] {
        let start = self.audit_log.len().saturating_sub(count);
        &self.audit_log[start..]
    }

    /// Get audit entries for a specific profile.
    pub fn audit_by_profile(&self, profile_id: &str, limit: usize) -> Vec<&ToolAuditEntry> {
        self.audit_log
            .iter()
            .rev()
            .filter(|e| e.profile_id == profile_id)
            .take(limit)
            .collect()
    }

    /// Total audit entry count.
    pub fn audit_count(&self) -> usize {
        self.audit_log.len()
    }

    /// Push an audit entry, dropping oldest if at capacity.
    fn push_audit(&mut self, entry: ToolAuditEntry) {
        if self.audit_log.len() >= self.max_audit_entries {
            self.audit_log.remove(0);
        }
        self.audit_log.push(entry);
    }
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// Current epoch time in seconds.
fn now_epoch_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use uuid::Uuid;

    fn sample_tool() -> ToolDefinition {
        ToolDefinition {
            id: "homebase.lights.set".into(),
            name: "Set Lights".into(),
            description: "Turn lights on/off or set brightness".into(),
            parameters_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "room": { "type": "string" },
                    "brightness": { "type": "integer", "minimum": 0, "maximum": 100 }
                },
                "required": ["room"]
            }),
            return_schema: None,
            has_side_effects: true,
            timeout: Duration::from_secs(5),
            required_role: ToolAccessRole::Child,
            supports_parallel: true,
        }
    }

    fn admin_tool() -> ToolDefinition {
        ToolDefinition {
            id: "system.config.update".into(),
            name: "Update Config".into(),
            description: "Update system configuration".into(),
            parameters_schema: serde_json::json!({"type": "object"}),
            return_schema: None,
            has_side_effects: true,
            timeout: Duration::from_secs(10),
            required_role: ToolAccessRole::Admin,
            supports_parallel: false,
        }
    }

    #[test]
    fn test_register_and_list_tools() {
        let mut reg = ToolRegistry::new();
        reg.register_tool(sample_tool()).unwrap();
        reg.register_tool(admin_tool()).unwrap();

        assert_eq!(reg.tool_count(), 2);
        assert!(reg.get_tool("homebase.lights.set").is_some());
    }

    #[test]
    fn test_unregister_tool() {
        let mut reg = ToolRegistry::new();
        reg.register_tool(sample_tool()).unwrap();
        assert!(reg.unregister_tool("homebase.lights.set"));
        assert_eq!(reg.tool_count(), 0);
    }

    #[test]
    fn test_role_filtering() {
        let mut reg = ToolRegistry::new();
        reg.register_tool(sample_tool()).unwrap();
        reg.register_tool(admin_tool()).unwrap();

        let child_tools = reg.list_tools_for_role(&ToolAccessRole::Child);
        assert_eq!(child_tools.len(), 1);

        let admin_tools = reg.list_tools_for_role(&ToolAccessRole::Admin);
        assert_eq!(admin_tools.len(), 2);

        let guest_tools = reg.list_tools_for_role(&ToolAccessRole::Guest);
        assert_eq!(guest_tools.len(), 0);
    }

    #[test]
    fn test_validate_tool_call() {
        let mut reg = ToolRegistry::new();
        reg.register_tool(sample_tool()).unwrap();

        let call = ToolCall {
            call_id: "c1".into(),
            tool_id: "homebase.lights.set".into(),
            arguments: serde_json::json!({"room": "kitchen", "brightness": 80}),
            chain_step: 0,
            parallel_group: None,
        };

        // Child can call
        assert!(reg.validate_tool_call(&call, &ToolAccessRole::Child).is_ok());
        // Guest cannot
        assert!(reg.validate_tool_call(&call, &ToolAccessRole::Guest).is_err());
    }

    #[test]
    fn test_validate_unknown_tool() {
        let reg = ToolRegistry::new();
        let call = ToolCall {
            call_id: "c1".into(),
            tool_id: "nonexistent".into(),
            arguments: serde_json::json!({}),
            chain_step: 0,
            parallel_group: None,
        };
        let result = reg.validate_tool_call(&call, &ToolAccessRole::Admin);
        assert!(matches!(result, Err(AgentError::ToolNotRegistered(_))));
    }

    #[test]
    fn test_tool_chain_lifecycle() {
        let mut reg = ToolRegistry::new();
        reg.register_tool(sample_tool()).unwrap();

        let request_id = Uuid::new_v4();
        let session_id = Uuid::new_v4();

        // Start chain
        reg.start_chain(request_id, session_id, Some(5)).unwrap();
        assert_eq!(reg.active_chain_count(), 1);

        // Submit calls
        let calls = vec![ToolCall {
            call_id: "c1".into(),
            tool_id: "homebase.lights.set".into(),
            arguments: serde_json::json!({"room": "kitchen"}),
            chain_step: 0,
            parallel_group: None,
        }];
        let validated = reg.submit_calls(&request_id, calls, &ToolAccessRole::Parent).unwrap();
        assert_eq!(validated.len(), 1);

        // Record results
        let results = vec![ToolResult {
            call_id: "c1".into(),
            tool_id: "homebase.lights.set".into(),
            success: true,
            output: serde_json::json!({"status": "ok"}),
            error: None,
            duration_ms: 42,
        }];
        let should_continue = reg.record_results(&request_id, results, "dad-profile").unwrap();
        assert!(should_continue);

        // Complete chain
        reg.complete_chain(&request_id).unwrap();
        let chain = reg.get_chain(&request_id).unwrap();
        assert_eq!(chain.state, ToolChainState::Complete);
        assert_eq!(chain.completed_steps.len(), 1);
    }

    #[test]
    fn test_chain_step_limit() {
        let mut reg = ToolRegistry::new();
        reg.register_tool(sample_tool()).unwrap();

        let request_id = Uuid::new_v4();
        let session_id = Uuid::new_v4();
        reg.start_chain(request_id, session_id, Some(1)).unwrap();

        // First call succeeds
        let calls = vec![ToolCall {
            call_id: "c1".into(),
            tool_id: "homebase.lights.set".into(),
            arguments: serde_json::json!({"room": "kitchen"}),
            chain_step: 0,
            parallel_group: None,
        }];
        reg.submit_calls(&request_id, calls, &ToolAccessRole::Admin).unwrap();
        reg.record_results(
            &request_id,
            vec![ToolResult {
                call_id: "c1".into(),
                tool_id: "homebase.lights.set".into(),
                success: true,
                output: serde_json::json!({}),
                error: None,
                duration_ms: 10,
            }],
            "admin",
        ).unwrap();

        // Second call exceeds limit
        let calls2 = vec![ToolCall {
            call_id: "c2".into(),
            tool_id: "homebase.lights.set".into(),
            arguments: serde_json::json!({"room": "bedroom"}),
            chain_step: 1,
            parallel_group: None,
        }];
        let result = reg.submit_calls(&request_id, calls2, &ToolAccessRole::Admin);
        assert!(matches!(result, Err(AgentError::ChainStepLimitExceeded { max: 1 })));
    }

    #[test]
    fn test_parallel_tool_calls() {
        let mut reg = ToolRegistry::new();
        reg.register_tool(sample_tool()).unwrap();

        let request_id = Uuid::new_v4();
        let session_id = Uuid::new_v4();
        reg.start_chain(request_id, session_id, None).unwrap();

        let calls = vec![
            ToolCall {
                call_id: "c1".into(),
                tool_id: "homebase.lights.set".into(),
                arguments: serde_json::json!({"room": "kitchen"}),
                chain_step: 0,
                parallel_group: Some("batch-1".into()),
            },
            ToolCall {
                call_id: "c2".into(),
                tool_id: "homebase.lights.set".into(),
                arguments: serde_json::json!({"room": "bedroom"}),
                chain_step: 0,
                parallel_group: Some("batch-1".into()),
            },
        ];

        let validated = reg.submit_calls(&request_id, calls, &ToolAccessRole::Parent).unwrap();
        assert_eq!(validated.len(), 2);
    }

    #[test]
    fn test_parallel_rejected_for_non_parallel_tool() {
        let mut reg = ToolRegistry::new();
        reg.register_tool(admin_tool()).unwrap(); // supports_parallel: false

        let request_id = Uuid::new_v4();
        reg.start_chain(request_id, Uuid::new_v4(), None).unwrap();

        let calls = vec![
            ToolCall {
                call_id: "c1".into(),
                tool_id: "system.config.update".into(),
                arguments: serde_json::json!({}),
                chain_step: 0,
                parallel_group: Some("batch".into()),
            },
            ToolCall {
                call_id: "c2".into(),
                tool_id: "system.config.update".into(),
                arguments: serde_json::json!({}),
                chain_step: 0,
                parallel_group: Some("batch".into()),
            },
        ];

        let result = reg.submit_calls(&request_id, calls, &ToolAccessRole::Admin);
        assert!(result.is_err());
    }

    #[test]
    fn test_audit_trail() {
        let mut reg = ToolRegistry::new();
        reg.register_tool(sample_tool()).unwrap();

        let request_id = Uuid::new_v4();
        reg.start_chain(request_id, Uuid::new_v4(), None).unwrap();

        let calls = vec![ToolCall {
            call_id: "c1".into(),
            tool_id: "homebase.lights.set".into(),
            arguments: serde_json::json!({"room": "kitchen"}),
            chain_step: 0,
            parallel_group: None,
        }];
        reg.submit_calls(&request_id, calls, &ToolAccessRole::Parent).unwrap();
        reg.record_results(
            &request_id,
            vec![ToolResult {
                call_id: "c1".into(),
                tool_id: "homebase.lights.set".into(),
                success: true,
                output: serde_json::json!({"status": "ok"}),
                error: None,
                duration_ms: 50,
            }],
            "dad-profile",
        ).unwrap();

        assert_eq!(reg.audit_count(), 1);
        let recent = reg.recent_audit(10);
        assert_eq!(recent.len(), 1);
        assert_eq!(recent[0].tool_id, "homebase.lights.set");
        assert!(recent[0].success);
        assert_eq!(recent[0].duration_ms, 50);
    }

    #[test]
    fn test_tools_for_model_format() {
        let mut reg = ToolRegistry::new();
        reg.register_tool(sample_tool()).unwrap();

        let model_tools = reg.tools_for_model(&ToolAccessRole::Parent);
        assert_eq!(model_tools.len(), 1);
        assert_eq!(model_tools[0]["type"], "function");
        assert_eq!(model_tools[0]["function"]["name"], "homebase.lights.set");
    }

    #[test]
    fn test_abort_chain() {
        let mut reg = ToolRegistry::new();
        let request_id = Uuid::new_v4();
        reg.start_chain(request_id, Uuid::new_v4(), None).unwrap();
        reg.abort_chain(&request_id, "User cancelled".into()).unwrap();

        let chain = reg.get_chain(&request_id).unwrap();
        assert!(matches!(chain.state, ToolChainState::Aborted { .. }));
        assert_eq!(reg.active_chain_count(), 0);
    }

    #[test]
    fn test_remove_chain() {
        let mut reg = ToolRegistry::new();
        let request_id = Uuid::new_v4();
        reg.start_chain(request_id, Uuid::new_v4(), None).unwrap();
        let chain = reg.remove_chain(&request_id);
        assert!(chain.is_some());
        assert!(reg.get_chain(&request_id).is_none());
    }
}
