//! Agentic task management for long-running inference workflows.
//!
//! Manages submit/poll/cancel lifecycle for multi-step tasks that may
//! involve tool calling chains, RAG retrieval, and iterative inference.
//! Each task gets a resource budget (tokens, tool calls, duration) and
//! progress is tracked through discrete steps.
//!
//! All tasks are profile-scoped and audited. No network access required.

use std::collections::HashMap;
use std::time::Duration;

use chrono::Utc;
use uuid::Uuid;

use crate::types::{
    AgentError, AgentTaskRequest, AgentTaskResponse, AgentTaskStatus, ResourceBudget,
    ResourceBudgetRequest, TaskConfig, TaskId, TaskProgress, ToolAuditEntry,
};

// ============================================================================
// Internal Task State
// ============================================================================

/// Internal representation of a managed agentic task.
#[derive(Debug, Clone)]
struct ManagedTask {
    /// Unique task identifier
    id: TaskId,
    /// Profile that submitted the task
    profile_id: String,
    /// Original instruction
    instruction: String,
    /// Current status
    status: AgentTaskStatus,
    /// Resource budget tracking
    budget: ResourceBudget,
    /// Tools available to this task
    available_tools: Vec<String>,
    /// Progress updates history
    progress_log: Vec<TaskProgress>,
    /// Tool calls made during this task
    tool_audit: Vec<ToolAuditEntry>,
    /// Number of inference calls made
    inference_count: u32,
    /// Current step index
    current_step: u32,
    /// Estimated total steps (may change)
    total_steps: Option<u32>,
    /// Intermediate result text
    intermediate_result: Option<String>,
    /// Final result text
    result: Option<String>,
    /// Task start timestamp (epoch ms, 0 if not started)
    started_at: u64,
    /// Task completion timestamp (epoch ms, 0 if not done)
    completed_at: u64,
}

// ============================================================================
// Task Manager
// ============================================================================

/// Manages the lifecycle of agentic tasks with resource budgets.
///
/// Tasks are submitted, polled for progress, and either complete
/// or are cancelled/budget-exhausted. Each task is profile-scoped
/// with concurrency limits enforced per-profile.
pub struct TaskManager {
    /// Active and recent tasks indexed by ID
    tasks: HashMap<TaskId, ManagedTask>,
    /// Default configuration
    config: TaskConfig,
    /// Maximum completed tasks to retain for audit
    max_retained: usize,
}

impl TaskManager {
    /// Create a new task manager with the given configuration.
    pub fn new(config: TaskConfig) -> Self {
        Self {
            tasks: HashMap::new(),
            config,
            max_retained: 100,
        }
    }

    /// Submit a new agentic task. Returns the task ID.
    ///
    /// Validates concurrency limits and builds the resource budget
    /// from the request (or defaults). The task starts in `Pending` status.
    pub fn submit(&mut self, request: AgentTaskRequest) -> Result<TaskId, AgentError> {
        // Enforce per-profile concurrency limit
        let active_count = self.active_count_for_profile(&request.profile_id);
        if active_count >= self.config.max_concurrent_per_profile {
            return Err(AgentError::TooManyConcurrentTasks {
                count: active_count,
                max: self.config.max_concurrent_per_profile,
            });
        }

        let task_id = Uuid::new_v4();
        let now_ms = epoch_ms();
        let budget = self.build_budget(&request.budget, now_ms);

        let task = ManagedTask {
            id: task_id,
            profile_id: request.profile_id,
            instruction: request.instruction,
            status: AgentTaskStatus::Pending,
            budget,
            available_tools: request.available_tools.unwrap_or_default(),
            progress_log: Vec::new(),
            tool_audit: Vec::new(),
            inference_count: 0,
            current_step: 0,
            total_steps: None,
            intermediate_result: None,
            result: None,
            started_at: 0,
            completed_at: 0,
        };

        self.tasks.insert(task_id, task);
        Ok(task_id)
    }

    /// Transition a task from Pending to Running.
    pub fn start_task(&mut self, task_id: TaskId) -> Result<(), AgentError> {
        let task = self.get_task_mut(task_id)?;
        match &task.status {
            AgentTaskStatus::Pending => {
                task.status = AgentTaskStatus::Running;
                task.started_at = epoch_ms();
                task.budget.started_at = task.started_at;
                Ok(())
            }
            other => Err(AgentError::Internal(format!(
                "Cannot start task in state: {other:?}"
            ))),
        }
    }

    /// Record that an inference call was made for a task.
    pub fn record_inference(
        &mut self,
        task_id: TaskId,
        tokens_used: u64,
    ) -> Result<(), AgentError> {
        let task = self.get_task_mut(task_id)?;
        Self::assert_active(&task.status)?;

        task.inference_count += 1;
        task.budget.tokens_used += tokens_used;

        // Check budget exhaustion
        if task.budget.is_exhausted() {
            let resource = if task.budget.tokens_used >= task.budget.max_tokens {
                "tokens".to_string()
            } else {
                "tool_calls".to_string()
            };
            task.status = AgentTaskStatus::BudgetExhausted {
                resource: resource.clone(),
            };
            task.completed_at = epoch_ms();
            return Err(AgentError::BudgetExhausted { resource });
        }

        Ok(())
    }

    /// Record a tool call made by a task.
    pub fn record_tool_call(
        &mut self,
        task_id: TaskId,
        entry: ToolAuditEntry,
    ) -> Result<(), AgentError> {
        let task = self.get_task_mut(task_id)?;
        Self::assert_active(&task.status)?;

        task.budget.tool_calls_used += 1;
        task.tool_audit.push(entry);

        // Check tool call budget
        if task.budget.tool_calls_used >= task.budget.max_tool_calls {
            task.status = AgentTaskStatus::BudgetExhausted {
                resource: "tool_calls".to_string(),
            };
            task.completed_at = epoch_ms();
            return Err(AgentError::BudgetExhausted {
                resource: "tool_calls".to_string(),
            });
        }

        Ok(())
    }

    /// Update progress on a running task.
    pub fn update_progress(
        &mut self,
        task_id: TaskId,
        message: String,
        percent: Option<u8>,
        step: u32,
        total_steps: Option<u32>,
        intermediate: Option<String>,
    ) -> Result<TaskProgress, AgentError> {
        let task = self.get_task_mut(task_id)?;
        Self::assert_active(&task.status)?;

        task.current_step = step;
        task.total_steps = total_steps;
        if intermediate.is_some() {
            task.intermediate_result = intermediate.clone();
        }

        let progress = TaskProgress {
            task_id: task_id.to_string(),
            status: task.status.clone(),
            message,
            percent_complete: percent,
            current_step: step,
            total_steps,
            intermediate_result: intermediate,
            budget: task.budget.clone(),
        };

        task.progress_log.push(progress.clone());
        Ok(progress)
    }

    /// Mark a task as waiting for tool execution results.
    pub fn await_tool_results(&mut self, task_id: TaskId) -> Result<(), AgentError> {
        let task = self.get_task_mut(task_id)?;
        Self::assert_active(&task.status)?;
        task.status = AgentTaskStatus::AwaitingToolResults;
        Ok(())
    }

    /// Resume a task from AwaitingToolResults back to Running.
    pub fn resume_from_tools(&mut self, task_id: TaskId) -> Result<(), AgentError> {
        let task = self.get_task_mut(task_id)?;
        match &task.status {
            AgentTaskStatus::AwaitingToolResults => {
                task.status = AgentTaskStatus::Running;
                Ok(())
            }
            other => Err(AgentError::Internal(format!(
                "Cannot resume from state: {other:?}"
            ))),
        }
    }

    /// Complete a task successfully with a final result.
    pub fn complete(
        &mut self,
        task_id: TaskId,
        result: String,
    ) -> Result<AgentTaskResponse, AgentError> {
        let task = self.get_task_mut(task_id)?;
        Self::assert_active(&task.status)?;

        task.status = AgentTaskStatus::Completed;
        task.result = Some(result);
        task.completed_at = epoch_ms();

        Ok(self.build_response(task_id))
    }

    /// Fail a task with an error reason.
    pub fn fail(
        &mut self,
        task_id: TaskId,
        reason: String,
    ) -> Result<AgentTaskResponse, AgentError> {
        let task = self.get_task_mut(task_id)?;
        Self::assert_active(&task.status)?;

        task.status = AgentTaskStatus::Failed {
            reason: reason.clone(),
        };
        task.completed_at = epoch_ms();

        Ok(self.build_response(task_id))
    }

    /// Cancel a task by user or system request.
    pub fn cancel(
        &mut self,
        task_id: TaskId,
        reason: String,
    ) -> Result<AgentTaskResponse, AgentError> {
        let task = self.get_task_mut(task_id)?;

        // Can cancel from any non-terminal state
        match &task.status {
            AgentTaskStatus::Completed
            | AgentTaskStatus::Failed { .. }
            | AgentTaskStatus::Cancelled { .. }
            | AgentTaskStatus::BudgetExhausted { .. } => {
                return Err(AgentError::Internal(format!(
                    "Cannot cancel task in terminal state: {:?}",
                    task.status
                )));
            }
            _ => {}
        }

        task.status = AgentTaskStatus::Cancelled { reason };
        task.completed_at = epoch_ms();

        Ok(self.build_response(task_id))
    }

    /// Get current progress for a task.
    pub fn get_progress(&self, task_id: TaskId) -> Result<TaskProgress, AgentError> {
        let task = self.get_task_ref(task_id)?;

        Ok(TaskProgress {
            task_id: task_id.to_string(),
            status: task.status.clone(),
            message: match &task.status {
                AgentTaskStatus::Pending => "Task queued".to_string(),
                AgentTaskStatus::Running => format!("Step {}", task.current_step),
                AgentTaskStatus::AwaitingToolResults => "Waiting for tool results".to_string(),
                AgentTaskStatus::Completed => "Task completed".to_string(),
                AgentTaskStatus::Failed { reason } => format!("Failed: {reason}"),
                AgentTaskStatus::Cancelled { reason } => format!("Cancelled: {reason}"),
                AgentTaskStatus::BudgetExhausted { resource } => {
                    format!("Budget exhausted: {resource}")
                }
            },
            percent_complete: task.progress_log.last().and_then(|p| p.percent_complete),
            current_step: task.current_step,
            total_steps: task.total_steps,
            intermediate_result: task.intermediate_result.clone(),
            budget: task.budget.clone(),
        })
    }

    /// Get the full response for a completed task.
    pub fn get_response(&self, task_id: TaskId) -> Result<AgentTaskResponse, AgentError> {
        let task = self.get_task_ref(task_id)?;
        match &task.status {
            AgentTaskStatus::Completed
            | AgentTaskStatus::Failed { .. }
            | AgentTaskStatus::Cancelled { .. }
            | AgentTaskStatus::BudgetExhausted { .. } => Ok(self.build_response(task_id)),
            _ => Err(AgentError::Internal(
                "Task is not in a terminal state".to_string(),
            )),
        }
    }

    /// Check if a task has exceeded its wall-clock timeout.
    pub fn check_timeout(&mut self, task_id: TaskId) -> Result<bool, AgentError> {
        let task = self.get_task_ref(task_id)?;

        if task.started_at == 0 {
            return Ok(false);
        }

        let elapsed_ms = epoch_ms().saturating_sub(task.started_at);
        let max_ms = task.budget.max_duration.as_millis() as u64;

        if elapsed_ms >= max_ms {
            let task = self.get_task_mut(task_id)?;
            task.status = AgentTaskStatus::BudgetExhausted {
                resource: "duration".to_string(),
            };
            task.completed_at = epoch_ms();
            return Ok(true);
        }

        Ok(false)
    }

    /// List tasks for a specific profile, optionally filtered by status.
    pub fn list_for_profile(
        &self,
        profile_id: &str,
        status_filter: Option<&AgentTaskStatus>,
    ) -> Vec<TaskProgress> {
        self.tasks
            .values()
            .filter(|t| t.profile_id == profile_id)
            .filter(|t| {
                status_filter
                    .map(|f| std::mem::discriminant(&t.status) == std::mem::discriminant(f))
                    .unwrap_or(true)
            })
            .map(|t| TaskProgress {
                task_id: t.id.to_string(),
                status: t.status.clone(),
                message: t.instruction.chars().take(100).collect(),
                percent_complete: t.progress_log.last().and_then(|p| p.percent_complete),
                current_step: t.current_step,
                total_steps: t.total_steps,
                intermediate_result: None,
                budget: t.budget.clone(),
            })
            .collect()
    }

    /// Number of active (non-terminal) tasks for a profile.
    pub fn active_count_for_profile(&self, profile_id: &str) -> usize {
        self.tasks
            .values()
            .filter(|t| t.profile_id == profile_id)
            .filter(|t| Self::is_active(&t.status))
            .count()
    }

    /// Total number of tasks tracked (including completed).
    pub fn total_count(&self) -> usize {
        self.tasks.len()
    }

    /// Number of currently active tasks across all profiles.
    pub fn active_count(&self) -> usize {
        self.tasks
            .values()
            .filter(|t| Self::is_active(&t.status))
            .count()
    }

    /// Get the tool audit trail for a specific task.
    pub fn task_tool_audit(&self, task_id: TaskId) -> Result<Vec<ToolAuditEntry>, AgentError> {
        let task = self.get_task_ref(task_id)?;
        Ok(task.tool_audit.clone())
    }

    /// Prune completed tasks beyond the retention limit.
    pub fn prune_completed(&mut self) -> usize {
        let mut completed: Vec<(TaskId, u64)> = self
            .tasks
            .iter()
            .filter(|(_, t)| !Self::is_active(&t.status))
            .map(|(id, t)| (*id, t.completed_at))
            .collect();

        if completed.len() <= self.max_retained {
            return 0;
        }

        completed.sort_by_key(|entry| std::cmp::Reverse(entry.1));

        let to_remove: Vec<TaskId> = completed
            .into_iter()
            .skip(self.max_retained)
            .map(|(id, _)| id)
            .collect();

        let count = to_remove.len();
        for id in to_remove {
            self.tasks.remove(&id);
        }
        count
    }

    /// Get available tools for a task.
    pub fn available_tools(&self, task_id: TaskId) -> Result<Vec<String>, AgentError> {
        let task = self.get_task_ref(task_id)?;
        Ok(task.available_tools.clone())
    }

    /// Get the profile that owns a task.
    pub fn task_profile(&self, task_id: TaskId) -> Result<String, AgentError> {
        let task = self.get_task_ref(task_id)?;
        Ok(task.profile_id.clone())
    }

    // ========================================================================
    // Private helpers
    // ========================================================================

    fn get_task_mut(&mut self, task_id: TaskId) -> Result<&mut ManagedTask, AgentError> {
        self.tasks
            .get_mut(&task_id)
            .ok_or_else(|| AgentError::TaskNotFound(task_id.to_string()))
    }

    fn get_task_ref(&self, task_id: TaskId) -> Result<&ManagedTask, AgentError> {
        self.tasks
            .get(&task_id)
            .ok_or_else(|| AgentError::TaskNotFound(task_id.to_string()))
    }

    fn build_budget(
        &self,
        overrides: &Option<ResourceBudgetRequest>,
        now_ms: u64,
    ) -> ResourceBudget {
        let defaults = &self.config;
        let max_dur = defaults.max_timeout;

        match overrides {
            Some(req) => ResourceBudget {
                max_tokens: req
                    .max_tokens
                    .unwrap_or(defaults.default_token_budget)
                    .min(defaults.default_token_budget * 10),
                tokens_used: 0,
                max_tool_calls: req
                    .max_tool_calls
                    .unwrap_or(defaults.default_max_tool_calls)
                    .min(defaults.default_max_tool_calls * 5),
                tool_calls_used: 0,
                max_duration: req
                    .max_duration_secs
                    .map(|s| Duration::from_secs(s).min(max_dur))
                    .unwrap_or(defaults.default_timeout),
                started_at: now_ms,
            },
            None => ResourceBudget {
                max_tokens: defaults.default_token_budget,
                tokens_used: 0,
                max_tool_calls: defaults.default_max_tool_calls,
                tool_calls_used: 0,
                max_duration: defaults.default_timeout,
                started_at: now_ms,
            },
        }
    }

    fn build_response(&self, task_id: TaskId) -> AgentTaskResponse {
        let task = self.tasks.get(&task_id).expect("task must exist");
        let duration_ms = if task.started_at > 0 && task.completed_at > 0 {
            task.completed_at.saturating_sub(task.started_at).max(1)
        } else {
            0
        };

        AgentTaskResponse {
            task_id: task_id.to_string(),
            status: task.status.clone(),
            result: task.result.clone(),
            tool_calls: task.tool_audit.clone(),
            budget: task.budget.clone(),
            duration_ms,
            inference_count: task.inference_count,
        }
    }

    fn is_active(status: &AgentTaskStatus) -> bool {
        matches!(
            status,
            AgentTaskStatus::Pending
                | AgentTaskStatus::Running
                | AgentTaskStatus::AwaitingToolResults
        )
    }

    fn assert_active(status: &AgentTaskStatus) -> Result<(), AgentError> {
        if Self::is_active(status) {
            Ok(())
        } else {
            Err(AgentError::Internal(format!(
                "Task is in terminal state: {status:?}"
            )))
        }
    }
}

/// Current epoch time in milliseconds.
fn epoch_ms() -> u64 {
    #[allow(clippy::cast_sign_loss)]
    {
        Utc::now().timestamp_millis() as u64
    }
}

// ============================================================================
// Unit Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config() -> TaskConfig {
        TaskConfig {
            max_concurrent_per_profile: 2,
            default_token_budget: 10_000,
            default_max_tool_calls: 10,
            default_timeout: Duration::from_secs(60),
            max_timeout: Duration::from_secs(300),
            progress_interval: Duration::from_secs(1),
        }
    }

    fn test_request(profile: &str) -> AgentTaskRequest {
        AgentTaskRequest {
            instruction: "Summarize the document".to_string(),
            profile_id: profile.to_string(),
            session_id: None,
            available_tools: Some(vec!["search".to_string(), "read_file".to_string()]),
            budget: None,
            model: None,
            stream_progress: false,
        }
    }

    fn test_tool_audit() -> ToolAuditEntry {
        ToolAuditEntry {
            timestamp: epoch_ms(),
            profile_id: "profile-1".to_string(),
            tool_id: "search".to_string(),
            call_id: Uuid::new_v4().to_string(),
            success: true,
            duration_ms: 42,
            chain_step: 1,
            error: None,
            session_id: "sess-1".to_string(),
        }
    }

    #[test]
    fn submit_and_start() {
        let mut mgr = TaskManager::new(test_config());
        let id = mgr.submit(test_request("profile-1")).unwrap();
        assert_eq!(mgr.total_count(), 1);
        assert_eq!(mgr.active_count(), 1);

        mgr.start_task(id).unwrap();
        let progress = mgr.get_progress(id).unwrap();
        assert_eq!(progress.status, AgentTaskStatus::Running);
    }

    #[test]
    fn concurrency_limit() {
        let mut mgr = TaskManager::new(test_config());
        mgr.submit(test_request("profile-1")).unwrap();
        mgr.submit(test_request("profile-1")).unwrap();
        let result = mgr.submit(test_request("profile-1"));
        assert!(result.is_err());

        let ok = mgr.submit(test_request("profile-2"));
        assert!(ok.is_ok());
    }

    #[test]
    fn complete_task() {
        let mut mgr = TaskManager::new(test_config());
        let id = mgr.submit(test_request("profile-1")).unwrap();
        mgr.start_task(id).unwrap();

        let resp = mgr.complete(id, "Summary: all good".to_string()).unwrap();
        assert_eq!(resp.status, AgentTaskStatus::Completed);
        assert_eq!(resp.result.as_deref(), Some("Summary: all good"));
    }

    #[test]
    fn fail_task() {
        let mut mgr = TaskManager::new(test_config());
        let id = mgr.submit(test_request("profile-1")).unwrap();
        mgr.start_task(id).unwrap();

        let resp = mgr.fail(id, "model unavailable".to_string()).unwrap();
        assert!(matches!(resp.status, AgentTaskStatus::Failed { .. }));
    }

    #[test]
    fn cancel_task() {
        let mut mgr = TaskManager::new(test_config());
        let id = mgr.submit(test_request("profile-1")).unwrap();
        mgr.start_task(id).unwrap();

        let resp = mgr.cancel(id, "user requested".to_string()).unwrap();
        assert!(matches!(resp.status, AgentTaskStatus::Cancelled { .. }));

        let err = mgr.cancel(id, "double cancel".to_string());
        assert!(err.is_err());
    }

    #[test]
    fn token_budget_exhaustion() {
        let mut mgr = TaskManager::new(test_config());
        let id = mgr.submit(test_request("profile-1")).unwrap();
        mgr.start_task(id).unwrap();

        mgr.record_inference(id, 5000).unwrap();
        mgr.record_inference(id, 4999).unwrap();
        let result = mgr.record_inference(id, 1001);
        assert!(result.is_err());

        let progress = mgr.get_progress(id).unwrap();
        assert!(matches!(
            progress.status,
            AgentTaskStatus::BudgetExhausted { .. }
        ));
    }

    #[test]
    fn tool_call_budget_exhaustion() {
        let mut mgr = TaskManager::new(test_config());
        let id = mgr.submit(test_request("profile-1")).unwrap();
        mgr.start_task(id).unwrap();

        for _ in 0..9 {
            mgr.record_tool_call(id, test_tool_audit()).unwrap();
        }

        let result = mgr.record_tool_call(id, test_tool_audit());
        assert!(result.is_err());
    }

    #[test]
    fn progress_tracking() {
        let mut mgr = TaskManager::new(test_config());
        let id = mgr.submit(test_request("profile-1")).unwrap();
        mgr.start_task(id).unwrap();

        let p = mgr
            .update_progress(id, "Reading doc".into(), Some(25), 1, Some(4), None)
            .unwrap();
        assert_eq!(p.percent_complete, Some(25));
        assert_eq!(p.current_step, 1);

        let p = mgr
            .update_progress(
                id,
                "Analyzing".into(),
                Some(75),
                3,
                Some(4),
                Some("partial result".into()),
            )
            .unwrap();
        assert_eq!(p.current_step, 3);
        assert_eq!(p.intermediate_result.as_deref(), Some("partial result"));
    }

    #[test]
    fn await_and_resume_tools() {
        let mut mgr = TaskManager::new(test_config());
        let id = mgr.submit(test_request("profile-1")).unwrap();
        mgr.start_task(id).unwrap();

        mgr.await_tool_results(id).unwrap();
        let p = mgr.get_progress(id).unwrap();
        assert_eq!(p.status, AgentTaskStatus::AwaitingToolResults);

        mgr.resume_from_tools(id).unwrap();
        let p = mgr.get_progress(id).unwrap();
        assert_eq!(p.status, AgentTaskStatus::Running);
    }

    #[test]
    fn list_for_profile() {
        let mut mgr = TaskManager::new(test_config());
        mgr.submit(test_request("alice")).unwrap();
        mgr.submit(test_request("alice")).unwrap();
        mgr.submit(test_request("bob")).unwrap();

        let alice_tasks = mgr.list_for_profile("alice", None);
        assert_eq!(alice_tasks.len(), 2);

        let bob_tasks = mgr.list_for_profile("bob", None);
        assert_eq!(bob_tasks.len(), 1);
    }

    #[test]
    fn prune_completed() {
        let mut mgr = TaskManager::new(test_config());
        mgr.max_retained = 2;

        for i in 0..5 {
            let id = mgr
                .submit(AgentTaskRequest {
                    instruction: format!("Task {i}"),
                    profile_id: "p1".to_string(),
                    session_id: None,
                    available_tools: None,
                    budget: None,
                    model: None,
                    stream_progress: false,
                })
                .unwrap();
            mgr.start_task(id).unwrap();
            mgr.complete(id, format!("Done {i}")).unwrap();
        }

        assert_eq!(mgr.total_count(), 5);
        let pruned = mgr.prune_completed();
        assert_eq!(pruned, 3);
        assert_eq!(mgr.total_count(), 2);
    }

    #[test]
    fn custom_budget_overrides() {
        let mut mgr = TaskManager::new(test_config());
        let id = mgr
            .submit(AgentTaskRequest {
                instruction: "Big task".to_string(),
                profile_id: "p1".to_string(),
                session_id: None,
                available_tools: None,
                budget: Some(ResourceBudgetRequest {
                    max_tokens: Some(50_000),
                    max_tool_calls: Some(25),
                    max_duration_secs: Some(120),
                }),
                model: Some("llama-70b".to_string()),
                stream_progress: true,
            })
            .unwrap();

        let progress = mgr.get_progress(id).unwrap();
        assert_eq!(progress.budget.max_tokens, 50_000);
        assert_eq!(progress.budget.max_tool_calls, 25);
        assert_eq!(progress.budget.max_duration, Duration::from_secs(120));
    }

    #[test]
    fn task_tool_audit_trail() {
        let mut mgr = TaskManager::new(test_config());
        let id = mgr.submit(test_request("profile-1")).unwrap();
        mgr.start_task(id).unwrap();

        mgr.record_tool_call(id, test_tool_audit()).unwrap();
        mgr.record_tool_call(id, test_tool_audit()).unwrap();

        let audit = mgr.task_tool_audit(id).unwrap();
        assert_eq!(audit.len(), 2);
        assert!(audit[0].success);
    }

    #[test]
    fn task_not_found() {
        let mgr = TaskManager::new(test_config());
        let fake_id = Uuid::new_v4();
        let result = mgr.get_progress(fake_id);
        assert!(result.is_err());
    }

    #[test]
    fn available_tools_and_profile() {
        let mut mgr = TaskManager::new(test_config());
        let id = mgr.submit(test_request("alice")).unwrap();

        let tools = mgr.available_tools(id).unwrap();
        assert_eq!(tools, vec!["search", "read_file"]);

        let profile = mgr.task_profile(id).unwrap();
        assert_eq!(profile, "alice");
    }

    #[test]
    fn get_response_for_terminal() {
        let mut mgr = TaskManager::new(test_config());
        let id = mgr.submit(test_request("p1")).unwrap();
        mgr.start_task(id).unwrap();

        assert!(mgr.get_response(id).is_err());

        mgr.complete(id, "done".to_string()).unwrap();
        let resp = mgr.get_response(id).unwrap();
        assert_eq!(resp.result.as_deref(), Some("done"));
        assert_eq!(resp.inference_count, 0);
    }
}
