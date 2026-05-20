//! Integration test: Agentic task lifecycle with resource budgets.
//!
//! Tests submit -> start -> progress -> tool calls -> complete/cancel/budget
//! flow, verifying resource tracking, concurrency limits, and audit trails.

use std::time::Duration;

use mai_agent::{
    AgentTaskRequest, AgentTaskStatus, ResourceBudgetRequest, TaskConfig, TaskManager,
    ToolAuditEntry,
};

fn test_config() -> TaskConfig {
    TaskConfig {
        max_concurrent_per_profile: 2,
        default_token_budget: 50_000,
        default_max_tool_calls: 20,
        default_timeout: Duration::from_secs(120),
        max_timeout: Duration::from_secs(600),
        progress_interval: Duration::from_secs(2),
    }
}

fn make_request(profile: &str, instruction: &str) -> AgentTaskRequest {
    AgentTaskRequest {
        instruction: instruction.to_string(),
        profile_id: profile.to_string(),
        session_id: Some("session-42".to_string()),
        available_tools: Some(vec![
            "web_search".to_string(),
            "read_file".to_string(),
            "summarize".to_string(),
        ]),
        budget: None,
        model: Some("phi-4-mini".to_string()),
        stream_progress: true,
    }
}

fn make_tool_audit(profile: &str, tool: &str, success: bool) -> ToolAuditEntry {
    ToolAuditEntry {
        timestamp: 1000,
        profile_id: profile.to_string(),
        tool_id: tool.to_string(),
        call_id: uuid::Uuid::new_v4().to_string(),
        success,
        duration_ms: 50,
        chain_step: 1,
        error: if success {
            None
        } else {
            Some("timeout".to_string())
        },
        session_id: "session-42".to_string(),
    }
}

#[test]
fn full_task_lifecycle_happy_path() {
    let mut mgr = TaskManager::new(test_config());

    // 1. Submit task
    let task_id = mgr
        .submit(make_request("admin", "Summarize all vault documents"))
        .unwrap();

    let progress = mgr.get_progress(task_id).unwrap();
    assert_eq!(progress.status, AgentTaskStatus::Pending);

    // 2. Start the task
    mgr.start_task(task_id).unwrap();
    let progress = mgr.get_progress(task_id).unwrap();
    assert_eq!(progress.status, AgentTaskStatus::Running);

    // 3. Record inference calls
    mgr.record_inference(task_id, 1500).unwrap();
    mgr.record_inference(task_id, 2000).unwrap();

    // 4. Update progress
    let p = mgr
        .update_progress(
            task_id,
            "Reading vault documents".to_string(),
            Some(25),
            1,
            Some(4),
            None,
        )
        .unwrap();
    assert_eq!(p.current_step, 1);
    assert_eq!(p.percent_complete, Some(25));

    // 5. Transition to awaiting tool results
    mgr.await_tool_results(task_id).unwrap();
    let progress = mgr.get_progress(task_id).unwrap();
    assert_eq!(progress.status, AgentTaskStatus::AwaitingToolResults);

    // 6. Record tool calls
    mgr.record_tool_call(task_id, make_tool_audit("admin", "read_file", true))
        .unwrap();
    mgr.record_tool_call(task_id, make_tool_audit("admin", "summarize", true))
        .unwrap();

    // 7. Resume from tools
    mgr.resume_from_tools(task_id).unwrap();

    // 8. More progress
    mgr.update_progress(
        task_id,
        "Generating summary".to_string(),
        Some(90),
        3,
        Some(4),
        Some("Partial summary: The vault contains...".to_string()),
    )
    .unwrap();

    // 9. Complete the task
    let response = mgr
        .complete(
            task_id,
            "Full summary of all 12 vault documents.".to_string(),
        )
        .unwrap();

    assert_eq!(response.status, AgentTaskStatus::Completed);
    assert_eq!(
        response.result.as_deref(),
        Some("Full summary of all 12 vault documents.")
    );
    assert_eq!(response.inference_count, 2);
    assert_eq!(response.tool_calls.len(), 2);
    assert!(response.duration_ms > 0);
    assert_eq!(response.budget.tokens_used, 3500);
    assert_eq!(response.budget.tool_calls_used, 2);
}

#[test]
fn concurrency_enforcement_per_profile() {
    let mut mgr = TaskManager::new(test_config());

    // Admin can have 2 concurrent tasks
    let t1 = mgr.submit(make_request("admin", "Task 1")).unwrap();
    let _t2 = mgr.submit(make_request("admin", "Task 2")).unwrap();

    // Third admin task rejected
    let err = mgr.submit(make_request("admin", "Task 3"));
    assert!(err.is_err());

    // But a different profile can still submit
    let ok = mgr.submit(make_request("parent", "Task for parent"));
    assert!(ok.is_ok());

    // Complete one admin task, then submit should work again
    mgr.start_task(t1).unwrap();
    mgr.complete(t1, "done".to_string()).unwrap();

    let ok = mgr.submit(make_request("admin", "Task 3 retry"));
    assert!(ok.is_ok());
}

#[test]
fn token_budget_exhaustion_stops_task() {
    let mut mgr = TaskManager::new(test_config());

    let task_id = mgr
        .submit(AgentTaskRequest {
            instruction: "Big analysis".to_string(),
            profile_id: "admin".to_string(),
            session_id: None,
            available_tools: None,
            budget: Some(ResourceBudgetRequest {
                max_tokens: Some(5000),
                max_tool_calls: None,
                max_duration_secs: None,
            }),
            model: None,
            stream_progress: false,
        })
        .unwrap();

    mgr.start_task(task_id).unwrap();

    // Use 4000 tokens
    mgr.record_inference(task_id, 4000).unwrap();

    // This pushes over the 5000 limit
    let result = mgr.record_inference(task_id, 1500);
    assert!(result.is_err());

    let progress = mgr.get_progress(task_id).unwrap();
    assert!(matches!(
        progress.status,
        AgentTaskStatus::BudgetExhausted { .. }
    ));

    // Task is now terminal; further operations should fail
    let fail = mgr.record_inference(task_id, 100);
    assert!(fail.is_err());
}

#[test]
fn tool_call_budget_exhaustion() {
    let mut mgr = TaskManager::new(test_config());

    let task_id = mgr
        .submit(AgentTaskRequest {
            instruction: "Tool-heavy task".to_string(),
            profile_id: "parent".to_string(),
            session_id: None,
            available_tools: Some(vec!["search".to_string()]),
            budget: Some(ResourceBudgetRequest {
                max_tokens: None,
                max_tool_calls: Some(3),
                max_duration_secs: None,
            }),
            model: None,
            stream_progress: false,
        })
        .unwrap();

    mgr.start_task(task_id).unwrap();

    // Use 2 tool calls fine
    mgr.record_tool_call(task_id, make_tool_audit("parent", "search", true))
        .unwrap();
    mgr.record_tool_call(task_id, make_tool_audit("parent", "search", true))
        .unwrap();

    // 3rd exhausts the budget
    let result = mgr.record_tool_call(task_id, make_tool_audit("parent", "search", true));
    assert!(result.is_err());

    let progress = mgr.get_progress(task_id).unwrap();
    assert!(matches!(
        progress.status,
        AgentTaskStatus::BudgetExhausted { .. }
    ));
}

#[test]
fn cancel_and_fail_paths() {
    let mut mgr = TaskManager::new(test_config());

    // Cancel path
    let t1 = mgr.submit(make_request("admin", "Cancel me")).unwrap();
    mgr.start_task(t1).unwrap();
    let resp = mgr
        .cancel(t1, "User requested cancellation".to_string())
        .unwrap();
    assert!(matches!(resp.status, AgentTaskStatus::Cancelled { .. }));

    // Cannot cancel a cancelled task
    assert!(mgr.cancel(t1, "again".to_string()).is_err());

    // Fail path
    let t2 = mgr.submit(make_request("admin", "Fail me")).unwrap();
    mgr.start_task(t2).unwrap();
    let resp = mgr.fail(t2, "Model crashed".to_string()).unwrap();
    assert!(matches!(resp.status, AgentTaskStatus::Failed { .. }));

    // Get response for terminal tasks
    let final_resp = mgr.get_response(t2).unwrap();
    assert!(matches!(final_resp.status, AgentTaskStatus::Failed { .. }));
}

#[test]
fn task_audit_trail_integrity() {
    let mut mgr = TaskManager::new(test_config());

    let task_id = mgr.submit(make_request("admin", "Audit me")).unwrap();
    mgr.start_task(task_id).unwrap();

    // Record mixed success/failure tool calls
    mgr.record_tool_call(task_id, make_tool_audit("admin", "search", true))
        .unwrap();
    mgr.record_tool_call(task_id, make_tool_audit("admin", "read_file", false))
        .unwrap();
    mgr.record_tool_call(task_id, make_tool_audit("admin", "search", true))
        .unwrap();

    let audit = mgr.task_tool_audit(task_id).unwrap();
    assert_eq!(audit.len(), 3);
    assert!(audit[0].success);
    assert!(!audit[1].success);
    assert_eq!(audit[1].error.as_deref(), Some("timeout"));
    assert!(audit[2].success);

    // Verify available tools preserved
    let tools = mgr.available_tools(task_id).unwrap();
    assert_eq!(tools.len(), 3);
    assert!(tools.contains(&"web_search".to_string()));
}

#[test]
fn profile_task_listing() {
    let mut mgr = TaskManager::new(test_config());

    let t1 = mgr.submit(make_request("admin", "Admin task 1")).unwrap();
    let _t2 = mgr.submit(make_request("admin", "Admin task 2")).unwrap();
    let _t3 = mgr.submit(make_request("parent", "Parent task")).unwrap();

    // All admin tasks
    let admin_tasks = mgr.list_for_profile("admin", None);
    assert_eq!(admin_tasks.len(), 2);

    // Complete one, filter by status
    mgr.start_task(t1).unwrap();
    mgr.complete(t1, "done".to_string()).unwrap();

    let running = mgr.list_for_profile("admin", Some(&AgentTaskStatus::Pending));
    assert_eq!(running.len(), 1);

    let completed = mgr.list_for_profile("admin", Some(&AgentTaskStatus::Completed));
    assert_eq!(completed.len(), 1);
}
