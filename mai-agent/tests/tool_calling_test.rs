//! Integration test: Tool calling round-trip with chain management.
//!
//! Tests the full flow: register tools -> validate calls -> execute chain
//! steps -> audit trail -> model-compatible format export.

use std::time::Duration;

use mai_agent::{
    AgentError, ToolAccessRole, ToolCall, ToolChainState, ToolDefinition, ToolRegistry, ToolResult,
};
use uuid::Uuid;

fn sample_tools() -> Vec<ToolDefinition> {
    vec![
        ToolDefinition {
            id: "web_search".to_string(),
            name: "web_search".to_string(),
            description: "Search the web for information".to_string(),
            parameters_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string" }
                },
                "required": ["query"]
            }),
            return_schema: None,
            has_side_effects: false,
            timeout: Duration::from_millis(5000),
            required_role: ToolAccessRole::Parent,
            supports_parallel: true,
        },
        ToolDefinition {
            id: "read_file".to_string(),
            name: "read_file".to_string(),
            description: "Read a file from the local vault".to_string(),
            parameters_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string" }
                },
                "required": ["path"]
            }),
            return_schema: None,
            has_side_effects: false,
            timeout: Duration::from_millis(3000),
            required_role: ToolAccessRole::Teen,
            supports_parallel: false,
        },
        ToolDefinition {
            id: "admin_config".to_string(),
            name: "admin_config".to_string(),
            description: "Modify system configuration".to_string(),
            parameters_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "key": { "type": "string" },
                    "value": { "type": "string" }
                },
                "required": ["key", "value"]
            }),
            return_schema: None,
            has_side_effects: true,
            timeout: Duration::from_millis(1000),
            required_role: ToolAccessRole::Admin,
            supports_parallel: false,
        },
    ]
}

fn tool_call(call_id: impl Into<String>, tool_id: impl Into<String>) -> ToolCall {
    ToolCall {
        call_id: call_id.into(),
        tool_id: tool_id.into(),
        arguments: serde_json::json!({"query": "Island Mountain inference servers"}),
        chain_step: 0,
        parallel_group: None,
    }
}

#[test]
fn full_tool_chain_round_trip() {
    let mut registry = ToolRegistry::new();

    // Step 1: Register tools
    for tool in sample_tools() {
        registry.register_tool(tool).unwrap();
    }
    assert_eq!(registry.tool_count(), 3);

    // Step 2: Verify role filtering
    let teen_tools = registry.list_tools_for_role(&ToolAccessRole::Teen);
    assert_eq!(teen_tools.len(), 1, "Teen should only see read_file");
    assert_eq!(teen_tools[0].id, "read_file");

    let parent_tools = registry.list_tools_for_role(&ToolAccessRole::Parent);
    assert_eq!(parent_tools.len(), 2, "Parent sees web_search + read_file");

    let admin_tools = registry.list_tools_for_role(&ToolAccessRole::Admin);
    assert_eq!(admin_tools.len(), 3, "Admin sees all tools");

    // Step 3: Start a tool chain
    let request_id = Uuid::new_v4();
    let session_id = Uuid::new_v4();
    registry.start_chain(request_id, session_id, None).unwrap();

    // Step 4: Submit a tool call
    let call = tool_call("call-1", "web_search");

    registry
        .validate_tool_call(&call, &ToolAccessRole::Parent)
        .expect("Parent should be able to call web_search");

    let calls = vec![call];
    registry
        .submit_calls(&request_id, calls, &ToolAccessRole::Parent)
        .expect("Should accept calls for active chain");

    // Step 5: Record results
    let results = vec![ToolResult {
        call_id: "call-1".to_string(),
        tool_id: "web_search".to_string(),
        output: serde_json::json!({"results": ["result 1", "result 2"]}),
        success: true,
        error: None,
        duration_ms: 150,
    }];

    registry
        .record_results(&request_id, &results, "profile-parent")
        .expect("Should accept results");

    // Step 6: Chain should still be active (more steps possible)
    let chain = registry.get_chain(&request_id).unwrap();
    assert_eq!(chain.state, ToolChainState::AwaitingModel);
    assert_eq!(chain.completed_steps.len(), 1);

    // Step 7: Complete the chain
    registry.complete_chain(&request_id).unwrap();
    let chain = registry.get_chain(&request_id).unwrap();
    assert_eq!(chain.state, ToolChainState::Complete);

    // Step 8: Verify audit trail
    let audit = registry.recent_audit(10);
    assert!(!audit.is_empty(), "Should have audit entries");
    assert_eq!(audit[0].tool_id, "web_search");
    assert!(audit[0].success);
}

#[test]
fn role_access_denied() {
    let mut registry = ToolRegistry::new();
    for tool in sample_tools() {
        registry.register_tool(tool).unwrap();
    }

    // Teen tries to call admin_config
    let mut call = tool_call("call-bad", "admin_config");
    call.arguments = serde_json::json!({"key": "evil", "value": "hack"});

    let result = registry.validate_tool_call(&call, &ToolAccessRole::Teen);
    assert!(result.is_err(), "Teen should be denied admin_config");
}

#[test]
fn parallel_tool_calls() {
    let mut registry = ToolRegistry::new();
    for tool in sample_tools() {
        registry.register_tool(tool).unwrap();
    }

    // web_search supports parallel, read_file does not
    let parallel_calls = vec![tool_call("p1", "web_search"), tool_call("p2", "web_search")];

    let valid = registry.validate_parallel_calls(&parallel_calls, &ToolAccessRole::Parent);
    assert!(valid.is_ok(), "Parallel web_search calls should be valid");

    // Mix parallel and non-parallel
    let mut read_call = tool_call("m2", "read_file");
    read_call.arguments = serde_json::json!({"path": "/data/file.txt"});
    let mixed_calls = vec![tool_call("m1", "web_search"), read_call];

    let invalid = registry.validate_parallel_calls(&mixed_calls, &ToolAccessRole::Parent);
    assert!(
        invalid.is_err(),
        "Cannot parallel-execute non-parallel tool"
    );
}

#[test]
fn chain_step_limit_enforced() {
    let mut registry = ToolRegistry::new();

    for tool in sample_tools() {
        registry.register_tool(tool).unwrap();
    }

    let request_id = Uuid::new_v4();
    let session_id = Uuid::new_v4();
    registry
        .start_chain(request_id, session_id, Some(3))
        .unwrap();

    // Execute 3 steps
    for i in 0..3 {
        let mut call = tool_call(format!("call-{i}"), "web_search");
        call.arguments = serde_json::json!({"query": format!("step {i}")});
        call.chain_step = i;
        let calls = vec![call];
        registry
            .submit_calls(&request_id, calls, &ToolAccessRole::Admin)
            .unwrap();

        let results = vec![ToolResult {
            call_id: format!("call-{i}"),
            tool_id: "web_search".to_string(),
            output: serde_json::json!({"ok": true}),
            success: true,
            error: None,
            duration_ms: 10,
        }];
        registry
            .record_results(&request_id, &results, "admin")
            .unwrap();
    }

    // 4th step should be rejected
    let calls = vec![tool_call("call-overflow", "web_search")];
    let result = registry.submit_calls(&request_id, calls, &ToolAccessRole::Admin);
    assert!(
        matches!(result, Err(AgentError::ChainStepLimitExceeded { max: 3 })),
        "Should reject calls beyond step limit"
    );
}

#[test]
fn model_compatible_tool_format() {
    let mut registry = ToolRegistry::new();
    for tool in sample_tools() {
        registry.register_tool(tool).unwrap();
    }

    let arr = registry.tools_for_model(&ToolAccessRole::Parent);

    assert_eq!(arr.len(), 2); // Parent sees 2 tools

    // Verify OpenAI-compatible format
    let first = &arr[0];
    assert_eq!(first["type"], "function");
    assert!(first["function"]["name"].is_string());
    assert!(first["function"]["description"].is_string());
    assert!(first["function"]["parameters"].is_object());
}
