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
    /// Maximum calls one immutable root-lineage/mission may make. `None` = no cap.
    pub max_calls_per_task: Option<u32>,
    /// Maximum distinct canonical systems one root-lineage/mission may touch.
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

    /// Cap calls per immutable root-lineage/mission accounting namespace.
    #[must_use]
    pub fn with_max_calls_per_task(mut self, max: u32) -> Self {
        self.max_calls_per_task = Some(max);
        self
    }

    /// Cap distinct canonical systems per root-lineage/mission.
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
    pub fn check(&self, call: &ToolCall, ctx: &InvokeContext) -> Option<String> {
        if let Err(reason) = self.tool_allowed_for_token(&ctx.profile_id, &call.tool_id) {
            return Some(reason);
        }
        if (self.max_calls_per_task.is_some() || self.max_systems_per_task.is_some())
            && ctx.reservation_key(None).is_none()
        {
            return Some("blast-radius: authenticated tenant/root lineage is required".to_string());
        }
        if self.max_systems_per_task.is_some() && ctx.system().is_none() {
            return Some("blast-radius: canonical target system is required".to_string());
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mai_agent::types::ToolAccessRole;

    fn ctx(profile: &str, system: Option<&str>) -> InvokeContext {
        let mut ctx = InvokeContext::unverified("s1", profile, ToolAccessRole::Guest);
        ctx.authority = Some(crate::AuthorityBinding {
            tenant_id: "tenant-a".to_string(),
            root_lineage: "root-a".to_string(),
        });
        ctx.canonical_system = system.map(str::to_string);
        ctx
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
        assert!(g.check(&call("anything"), &ctx("tok", None)).is_none());
    }

    #[test]
    fn per_token_allowlist_denies_an_unlisted_tool() {
        let g = Guardrails::new().allow_token_tool("tok_1", "read.file");
        assert!(
            g.check(&call("read.file"), &ctx("tok_1", None)).is_none(),
            "the listed tool passes"
        );
        let reason = g.check(&call("write.db"), &ctx("tok_1", None)).unwrap();
        assert!(reason.contains("not allowed tool 'write.db'"));
    }

    #[test]
    fn deny_unlisted_tokens_fails_closed() {
        let g = Guardrails::new()
            .allow_token_tool("tok_1", "read.file")
            .deny_unlisted_tokens();
        // A different token with no allowlist entry is denied everything.
        assert!(
            g.check(&call("read.file"), &ctx("tok_other", None))
                .unwrap()
                .contains("deny-unlisted")
        );
    }

    #[test]
    fn call_cap_requires_authenticated_lineage() {
        let g = Guardrails::new().with_max_calls_per_task(2);
        let unverified = InvokeContext::unverified("s1", "tok", ToolAccessRole::Guest);
        assert!(
            g.check(&call("t"), &unverified)
                .unwrap()
                .contains("root lineage")
        );
    }

    #[test]
    fn system_cap_requires_canonical_target() {
        let g = Guardrails::new().with_max_systems_per_task(1);
        assert!(
            g.check(&call("t"), &ctx("tok", None))
                .unwrap()
                .contains("canonical target")
        );
        assert!(g.check(&call("t"), &ctx("tok", Some("aws"))).is_none());
    }
}
