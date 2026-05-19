//! Integration test: Tool calling round-trip with chain management.
//!
//! Tests the full flow: register tools -> validate calls -> execute chain
//! steps -> audit trail -> model-compatible format export.

use mai_agent::{
    AgentError, ToolAccessRole, ToolAuditEntry, ToolCall, ToolChainState,
    ToolDefinition, ToolRegistry, ToolResult,
};

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
            required_role: ToolAccessRole::Parent,
            supports_parallel: true,
            timeout_ms: 5000,
            category: "search".to_string(),
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
            required_role: ToolAccessRole::Teen,
            supports_parallel: false,
            timeout_ms: 3000,
            category: "filesystem".to_string(),
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
            required_role: ToolAccessRole::Admin,
            supports_parallel: false,
            timeout_ms: 1000,
            category: "admin".to_string(),
        },
    ]
}

#[test]
fn full_tool_chain_round_trip() {
    let mut registry = ToolRegistry::new(20, 10);

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
    let chain_id = registry
        .start_chain("session-123", "profile-parent")
        .unwrap();

    // Step 4: Submit a tool call
    let call = ToolCall {
        id: "call-1".to_string(),
        tool_id: "web_search".to_string(),
        arguments: serde_json::json!({"query": "Island Mountain inference servers"}),
    };

    registry
        .validate_tool_call(&call, &ToolAccessRole::Parent)
        .expect("Parent should be able to call web_search");

    let calls = vec![call];
    registry
        .submit_calls(&chain_id, &calls)
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
        .record_results(&chain_id, &results)
        .expect("Should accept results");

    // Step 6: Chain should still be active (more steps possible)
    let chain = registry.get_chain(&chain_id).unwrap();
    assert_eq!(chain.state, ToolChainState::AwaitingCalls);
    assert_eq!(chain.steps_completed, 1);

    // Step 7: Complete the chain
    registry.complete_chain(&chain_id).unwrap();
    let chain = registry.get_chain(&chain_id).unwrap();
    assert_eq!(chain.state, ToolChainState::Completed);

    // Step 8: Verify audit trail
    let audit = registry.recent_audit(10);
    assert!(!audit.is_empty(), "Should have audit entries");
    assert_eq!(audit[0].tool_id, "web_search");
    assert!(audit[0].success);
}

#[test]
fn role_access_denied() {
    let mut registry = ToolRegistry::new(20, 10);
    for tool in sample_tools() {
        registry.register_tool(tool).unwrap();
    }

    // Teen tries to call admin_config
    let call = ToolCall {
        id: "call-bad".to_string(),
        tool_id: "admin_config".to_string(),
        arguments: serde_json::json!({"key": "evil", "value": "hack"}),
    };

    let result = registry.validate_tool_call(&call, &ToolAccessRole::Teen);
    assert!(result.is_err(), "Teen should be denied admin_config");
}

#[test]
fn parallel_tool_calls() {
    let mut registry = ToolRegistry::new(20, 10);
    for tool in sample_tools() {
        registry.register_tool(tool).unwrap();
    }

    // web_search supports parallel, read_file does not
    let parallel_calls = vec![
        ToolCall {
            id: "p1".to_string(),
            tool_id: "web_search".to_string(),
            arguments: serde_json::json!({"query": "test 1"}),
        },
        ToolCall {
            id: "p2".to_string(),
            tool_id: "web_search".to_string(),
            arguments: serde_json::json!({"query": "test 2"}),
        },
    ];

    let valid = registry.validate_parallel_calls(&parallel_calls);
    assert!(valid.is_ok(), "Parallel web_search calls should be valid");

    // Mix parallel and non-parallel
    let mixed_calls = vec![
        ToolCall {
            id: "m1".to_string(),
            tool_id: "web_search".to_string(),
            arguments: serde_json::json!({"query": "test"}),
        },
        ToolCall {
            id: "m2".to_string(),
            tool_id: "read_file".to_string(),
            arguments: serde_json::json!({"path": "/data/file.txt"}),
        },
    ];

    let invalid = registry.validate_parallel_calls(&mixed_calls);
    assert!(invalid.is_err(), "Cannot parallel-execute non-parallel tool");
}

#[test]
fn chain_step_limit_enforced() {
    let mut registry = ToolRegistry::new(3, 10); // max 3 steps

    for tool in sample_tools() {
        registry.register_tool(tool).unwrap();
    }

    let chain_id = registry.start_chain("sess", "profile").unwrap();

    // Execute 3 steps
    for i in 0..3 {
        let calls = vec![ToolCall {
            id: format!("call-{i}"),
            tool_id: "web_search".to_string(),
            arguments: serde_json::json!({"query": format!("step {i}")}),
        }];
        registry.submit_calls(&chain_id, &calls).unwrap();

        let results = vec![ToolResult {
            call_id: format!("call-{i}"),
            tool_id: "web_search".to_string(),
            output: serde_json::json!({"ok": true}),
            success: true,
            error: None,
            duration_ms: 10,
        }];
        registry.record_results(&chain_id, &results).unwrap();
    }

    // 4th step should be rejected
    let calls = vec![ToolCall {
        id: "call-overflow".to_string(),
        tool_id: "web_search".to_string(),
        arguments: serde_json::json!({"query": "too many"}),
    }];
    let result = registry.submit_calls(&chain_id, &calls);
    assert!(result.is_err(), "Should reject calls beyond step limit");
}

#[test]
fn model_compatible_tool_format() {
    let mut registry = ToolRegistry::new(20, 10);
    for tool in sample_tools() {
        registry.register_tool(tool).unwrap();
    }

    let json = registry.tools_for_model(&ToolAccessRole::Parent);
    let arr: Vec<serde_json::Value> = serde_json::from_value(json).unwrap();

    assert_eq!(arr.len(), 2); // Parent sees 2 tools

    // Verify OpenAI-compatible format
    let first = &arr[0];
    assert_eq!(first["type"], "function");
    assert!(first["function"]["name"].is_string());
    assert!(first["function"]["description"].is_string());
    assert!(first["function"]["parameters"].is_object());
}
