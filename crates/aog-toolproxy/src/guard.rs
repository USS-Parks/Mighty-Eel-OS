//! Tool-governance hardening (T8).
//!
//! The operator's guardrails — the layer beneath a workload's own mission
//! contract (T6). Where a mission is the scope the *workload* declares, guardrails
//! are the hard limits the *operator* sets, applied to every call regardless of any
//! mission, as defence in depth:
//!
//! - **Fail-closed on unknown tools.** A call to an unregistered tool is denied by
//!   the registry before anything else (T1) — guardrails keep that the default.
//! - **Per-token tool allowlists.** A token may be pinned to a set of tools; with
//!   `deny_unlisted_tokens`, a token with no allowlist is denied everything.
//! - **Blast-radius caps.** A task (session) may be capped at a maximum number of
//!   tool calls and a maximum number of *distinct systems* touched — so a single
//!   hijacked run cannot fan out across the estate.
//!
//! A tripped guardrail is a **hard** block (no approval escalation) and is
//! receipted, so the attempt is on the record.

use std::collections::{BTreeMap, BTreeSet};

use mai_agent::types::ToolCall;

use crate::InvokeContext;

/// Operator-set hard limits, applied to every brokered call. All limits default
/// off — an empty `Guardrails` changes nothing (the registry still denies unknown
/// tools). Build with the `with_*` / `allow_*` / `deny_*` builders.
#[derive(Debug, Clone, Default)]
pub struct Guardrails {
    /// Maximum tool calls a single task (session) may make. `None` = no cap.
    pub max_calls_per_task: Option<u32>,
    /// Maximum distinct systems a single task may touch. `None` = no cap.
    pub max_systems_per_task: Option<u32>,
    /// Per-token tool allowlists: token id → the tools it may call. A token with an
    /// entry may call only those tools.
    pub token_allowlists: BTreeMap<String, BTreeSet<String>>,
    /// Fail-closed: a token that has no allowlist entry is denied every tool.
    pub deny_unlisted_tokens: bool,
}

impl Guardrails {
    /// Permissive guardrails (everything off). The registry still denies unknown
    /// tools; this adds no further restriction until a limit is set.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Cap the number of tool calls per task (session).
    #[must_use]
    pub fn with_max_calls_per_task(mut self, max: u32) -> Self {
        self.max_calls_per_task = Some(max);
        self
    }

    /// Cap the number of distinct systems a task may touch.
    #[must_use]
    pub fn with_max_systems_per_task(mut self, max: u32) -> Self {
        self.max_systems_per_task = Some(max);
        self
    }

    /// Allow `token_id` to call `tool_id` (adds an allowlist entry for the token).
    #[must_use]
    pub fn allow_token_tool(
        mut self,
        token_id: impl Into<String>,
        tool_id: impl Into<String>,
    ) -> Self {
        self.token_allowlists
            .entry(token_id.into())
            .or_default()
            .insert(tool_id.into());
        self
    }

    /// Deny any token that has no allowlist entry (fail-closed).
    #[must_use]
    pub fn deny_unlisted_tokens(mut self) -> Self {
        self.deny_unlisted_tokens = true;
        self
    }

    /// Whether any limit is set — used to avoid tracking task usage when guardrails
    /// are entirely off.
    #[must_use]
    pub fn is_active(&self) -> bool {
        self.max_calls_per_task.is_some()
            || self.max_systems_per_task.is_some()
            || !self.token_allowlists.is_empty()
            || self.deny_unlisted_tokens
    }

    /// Is this tool allowed for this token by the per-token allowlist?
    fn tool_allowed_for_token(&self, profile_id: &str, tool_id: &str) -> Result<(), String> {
        match self.token_allowlists.get(profile_id) {
            Some(allowed) if allowed.contains(tool_id) => Ok(()),
            Some(_) => Err(format!(
                "token '{profile_id}' is not allowed tool '{tool_id}'"
            )),
            None if self.deny_unlisted_tokens => Err(format!(
                "token '{profile_id}' has no tool allowlist (deny-unlisted)"
            )),
            None => Ok(()),
        }
    }

    /// Check a call against the guardrails given the task's current usage. Returns
    /// `Some(reason)` when a hard limit trips (a block, never an escalation).
    #[must_use]
    pub fn check(&self, call: &ToolCall, ctx: &InvokeContext, usage: &TaskUsage) -> Option<String> {
        if let Err(reason) = self.tool_allowed_for_token(&ctx.profile_id, &call.tool_id) {
            return Some(reason);
        }
        if let Some(max) = self.max_calls_per_task
            && usage.calls >= max
        {
            return Some(format!("blast-radius: task call cap {max} reached"));
        }
        if let Some(max) = self.max_systems_per_task
            && let Some(system) = &ctx.system
            && !usage.systems.contains(system)
            && u32::try_from(usage.systems.len()).unwrap_or(u32::MAX) >= max
        {
            return Some(format!("blast-radius: task system cap {max} reached"));
        }
        None
    }
}

/// Per-task (per-session) running usage the blast-radius caps are measured against.
#[derive(Debug, Default)]
pub struct TaskUsage {
    calls: u32,
    systems: BTreeSet<String>,
}

impl TaskUsage {
    /// Record an admitted call: one more call, and its system (if any) added to the
    /// touched set.
    pub fn record(&mut self, ctx: &InvokeContext) {
        self.calls = self.calls.saturating_add(1);
        if let Some(system) = &ctx.system {
            self.systems.insert(system.clone());
        }
    }

    /// Calls made so far by this task.
    #[must_use]
    pub fn calls(&self) -> u32 {
        self.calls
    }

    /// Distinct systems touched so far by this task.
    #[must_use]
    pub fn systems_touched(&self) -> usize {
        self.systems.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mai_agent::types::ToolAccessRole;

    fn ctx(profile: &str, system: Option<&str>) -> InvokeContext {
        InvokeContext {
            session_id: "s1".to_string(),
            profile_id: profile.to_string(),
            role: ToolAccessRole::Guest,
            system: system.map(str::to_string),
            estimated_cost_cents: 0,
        }
    }

    fn call(tool_id: &str) -> ToolCall {
        ToolCall {
            call_id: "c1".to_string(),
            tool_id: tool_id.to_string(),
            arguments: serde_json::json!({}),
            chain_step: 0,
            parallel_group: None,
        }
    }

    #[test]
    fn default_guardrails_are_inactive_and_permissive() {
        let g = Guardrails::new();
        assert!(!g.is_active());
        assert!(
            g.check(&call("anything"), &ctx("tok", None), &TaskUsage::default())
                .is_none()
        );
    }

    #[test]
    fn per_token_allowlist_denies_an_unlisted_tool() {
        let g = Guardrails::new().allow_token_tool("tok_1", "read.file");
        assert!(
            g.check(
                &call("read.file"),
                &ctx("tok_1", None),
                &TaskUsage::default()
            )
            .is_none(),
            "the listed tool passes"
        );
        let reason = g
            .check(
                &call("write.db"),
                &ctx("tok_1", None),
                &TaskUsage::default(),
            )
            .unwrap();
        assert!(reason.contains("not allowed tool 'write.db'"));
    }

    #[test]
    fn deny_unlisted_tokens_fails_closed() {
        let g = Guardrails::new()
            .allow_token_tool("tok_1", "read.file")
            .deny_unlisted_tokens();
        // A different token with no allowlist entry is denied everything.
        assert!(
            g.check(
                &call("read.file"),
                &ctx("tok_other", None),
                &TaskUsage::default()
            )
            .unwrap()
            .contains("deny-unlisted")
        );
    }

    #[test]
    fn call_cap_trips_when_reached() {
        let g = Guardrails::new().with_max_calls_per_task(2);
        let mut usage = TaskUsage::default();
        assert!(g.check(&call("t"), &ctx("tok", None), &usage).is_none());
        usage.record(&ctx("tok", None));
        usage.record(&ctx("tok", None));
        // Two calls made; the cap now trips.
        assert!(
            g.check(&call("t"), &ctx("tok", None), &usage)
                .unwrap()
                .contains("call cap 2")
        );
    }

    #[test]
    fn system_cap_trips_on_a_new_system_beyond_the_limit() {
        let g = Guardrails::new().with_max_systems_per_task(1);
        let mut usage = TaskUsage::default();
        // First system is fine and gets recorded.
        assert!(
            g.check(&call("t"), &ctx("tok", Some("aws")), &usage)
                .is_none()
        );
        usage.record(&ctx("tok", Some("aws")));
        // Same system again is fine (not a new one).
        assert!(
            g.check(&call("t"), &ctx("tok", Some("aws")), &usage)
                .is_none()
        );
        // A second, different system trips the cap.
        assert!(
            g.check(&call("t"), &ctx("tok", Some("gcp")), &usage)
                .unwrap()
                .contains("system cap 1")
        );
        assert_eq!(usage.systems_touched(), 1);
    }
}
