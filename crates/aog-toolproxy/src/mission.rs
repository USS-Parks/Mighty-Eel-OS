//! Mission contracts (T6).
//!
//! A mission contract is the scope an agent workload **declares up front**: the
//! tools it may call, the systems it may touch, and the ceilings (call count,
//! spend) it commits to stay within. The proxy holds the running mission to that
//! declared envelope — a call outside it is a *deviation*: blocked when no
//! approval inbox is configured, or escalated for a human decision when one is.
//! This is the runtime half of the Lamprey "change contract" idea — intent
//! declared, then enforced, so an agent cannot quietly widen its own scope.
//!
//! The contract is **fail-closed on the tool axis**: an empty `allowed_tools`
//! set admits nothing. Ceilings default open (set them to restrict). The system
//! axis only constrains calls that declare a target system.

use std::collections::BTreeSet;

use mai_agent::types::ToolCall;

use crate::InvokeContext;

/// The declared scope of a mission. Build with the `allow_*` / `with_*` builders.
#[derive(Debug, Clone)]
pub struct MissionContract {
    /// A stable id for the mission (receipted; ties the trace together).
    pub mission_id: String,
    /// Tool ids the mission may call. Empty admits nothing (fail-closed).
    pub allowed_tools: BTreeSet<String>,
    /// Target systems the mission may touch, matched against
    /// [`InvokeContext::system`]. A call declaring a system not in this set is a
    /// deviation; a call declaring no system is unconstrained on this axis.
    pub allowed_systems: BTreeSet<String>,
    /// Hard ceiling on the mission's total tool calls (blast radius). Defaults to
    /// no cap; set to restrict.
    pub max_tool_calls: u32,
    /// Hard ceiling on the mission's cumulative declared spend, in cents. Defaults
    /// to no cap; set to restrict.
    pub spend_ceiling_cents: u64,
}

impl MissionContract {
    /// A new, empty contract for `mission_id` — admits **no** tools until one is
    /// allowed (fail-closed), with no call/spend ceiling until set.
    #[must_use]
    pub fn new(mission_id: impl Into<String>) -> Self {
        Self {
            mission_id: mission_id.into(),
            allowed_tools: BTreeSet::new(),
            allowed_systems: BTreeSet::new(),
            max_tool_calls: u32::MAX,
            spend_ceiling_cents: u64::MAX,
        }
    }

    /// Permit a tool id.
    #[must_use]
    pub fn allow_tool(mut self, tool_id: impl Into<String>) -> Self {
        self.allowed_tools.insert(tool_id.into());
        self
    }

    /// Permit a target system.
    #[must_use]
    pub fn allow_system(mut self, system: impl Into<String>) -> Self {
        self.allowed_systems.insert(system.into());
        self
    }

    /// Set the total-tool-call ceiling.
    #[must_use]
    pub fn with_max_calls(mut self, max: u32) -> Self {
        self.max_tool_calls = max;
        self
    }

    /// Set the cumulative-spend ceiling (cents).
    #[must_use]
    pub fn with_spend_ceiling_cents(mut self, cents: u64) -> Self {
        self.spend_ceiling_cents = cents;
        self
    }
}

/// A running mission: a contract plus the tally of what it has consumed so far.
#[derive(Debug, Clone)]
pub struct Mission {
    contract: MissionContract,
    calls: u32,
    spend_cents: u64,
}

impl Mission {
    /// Start a mission from its contract, with a zeroed tally.
    #[must_use]
    pub fn new(contract: MissionContract) -> Self {
        Self {
            contract,
            calls: 0,
            spend_cents: 0,
        }
    }

    /// The mission id (for receipts).
    #[must_use]
    pub fn mission_id(&self) -> &str {
        &self.contract.mission_id
    }

    /// Calls made so far.
    #[must_use]
    pub fn calls(&self) -> u32 {
        self.calls
    }

    /// Cumulative spend so far (cents).
    #[must_use]
    pub fn spend_cents(&self) -> u64 {
        self.spend_cents
    }

    /// Evaluate a call against the contract **without** mutating the tally. Returns
    /// `Some(reason)` when the call is out of contract (an un-listed tool, an
    /// un-listed target system, or one that would breach the call / spend ceiling),
    /// `None` when it is within scope. Read-only, so it never holds a lock across an
    /// approval `await`.
    #[must_use]
    pub fn check(&self, call: &ToolCall, ctx: &InvokeContext) -> Option<String> {
        let c = &self.contract;
        if !c.allowed_tools.contains(&call.tool_id) {
            return Some(format!(
                "tool '{}' is not in mission '{}'",
                call.tool_id, c.mission_id
            ));
        }
        if let Some(system) = &ctx.system
            && !c.allowed_systems.contains(system)
        {
            return Some(format!(
                "system '{system}' is not in mission '{}'",
                c.mission_id
            ));
        }
        if self.calls >= c.max_tool_calls {
            return Some(format!(
                "mission '{}' call ceiling reached ({} of {})",
                c.mission_id, self.calls, c.max_tool_calls
            ));
        }
        if self.spend_cents.saturating_add(ctx.estimated_cost_cents) > c.spend_ceiling_cents {
            return Some(format!(
                "mission '{}' spend ceiling {}¢ would be exceeded",
                c.mission_id, c.spend_ceiling_cents
            ));
        }
        None
    }

    /// Record an admitted call's consumption against the tally. Called once a call
    /// is cleared to proceed (in-contract, or a deviation a human approved).
    pub fn record(&mut self, ctx: &InvokeContext) {
        self.calls = self.calls.saturating_add(1);
        self.spend_cents = self.spend_cents.saturating_add(ctx.estimated_cost_cents);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mai_agent::types::ToolAccessRole;

    fn ctx() -> InvokeContext {
        InvokeContext {
            session_id: "s1".to_string(),
            profile_id: "tok_1".to_string(),
            role: ToolAccessRole::Guest,
            system: None,
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
    fn in_contract_tool_passes() {
        let m = Mission::new(MissionContract::new("m1").allow_tool("read.file"));
        assert!(m.check(&call("read.file"), &ctx()).is_none());
    }

    #[test]
    fn tool_not_in_contract_is_a_deviation() {
        let m = Mission::new(MissionContract::new("m1").allow_tool("read.file"));
        let reason = m.check(&call("delete.all"), &ctx()).unwrap();
        assert!(reason.contains("delete.all"));
        assert!(reason.contains("not in mission"));
    }

    #[test]
    fn empty_contract_admits_nothing() {
        let m = Mission::new(MissionContract::new("m1"));
        assert!(m.check(&call("read.file"), &ctx()).is_some(), "fail-closed");
    }

    #[test]
    fn system_outside_contract_is_a_deviation() {
        let m = Mission::new(
            MissionContract::new("m1")
                .allow_tool("s3.get")
                .allow_system("aws"),
        );
        let mut c = ctx();
        c.system = Some("gcp".to_string());
        assert!(m.check(&call("s3.get"), &c).unwrap().contains("gcp"));
        c.system = Some("aws".to_string());
        assert!(
            m.check(&call("s3.get"), &c).is_none(),
            "allowed system passes"
        );
    }

    #[test]
    fn call_ceiling_blocks_once_reached() {
        let mut m = Mission::new(
            MissionContract::new("m1")
                .allow_tool("read.file")
                .with_max_calls(2),
        );
        assert!(m.check(&call("read.file"), &ctx()).is_none());
        m.record(&ctx());
        assert!(m.check(&call("read.file"), &ctx()).is_none());
        m.record(&ctx());
        // Two calls made; the third is over the ceiling.
        assert!(
            m.check(&call("read.file"), &ctx())
                .unwrap()
                .contains("ceiling")
        );
        assert_eq!(m.calls(), 2);
    }

    #[test]
    fn spend_ceiling_blocks_when_exceeded() {
        let m = Mission::new(
            MissionContract::new("m1")
                .allow_tool("api.call")
                .with_spend_ceiling_cents(100),
        );
        let mut c = ctx();
        c.estimated_cost_cents = 150;
        assert!(m.check(&call("api.call"), &c).unwrap().contains("spend"));
        c.estimated_cost_cents = 100;
        assert!(
            m.check(&call("api.call"), &c).is_none(),
            "exactly at ceiling passes"
        );
    }
}
