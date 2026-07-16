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

use fabric_contracts::Budget;
use fabric_token::spend::{Reservation, ReservationLedger, Spent};
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

/// An active mission contract. Runtime call/spend state lives in the shared A4
/// reservation ledger rather than a check-then-record counter on this value.
#[derive(Debug, Clone)]
pub struct Mission {
    contract: MissionContract,
}

impl Mission {
    /// Start a mission from its contract, with a zeroed tally.
    #[must_use]
    pub fn new(contract: MissionContract) -> Self {
        Self { contract }
    }

    /// The mission id (for receipts).
    #[must_use]
    pub fn mission_id(&self) -> &str {
        &self.contract.mission_id
    }

    /// Evaluate immutable tool/system scope. Call and spend ceilings are reserved
    /// atomically by [`Self::reserve`] before asynchronous approval.
    #[must_use]
    pub fn check(&self, call: &ToolCall, ctx: &InvokeContext) -> Option<String> {
        let c = &self.contract;
        if !c.allowed_tools.contains(&call.tool_id) {
            return Some(format!(
                "tool '{}' is not in mission '{}'",
                call.tool_id, c.mission_id
            ));
        }
        if !c.allowed_systems.is_empty() && ctx.system().is_none() {
            return Some(format!(
                "canonical target system is required by mission '{}'",
                c.mission_id
            ));
        }
        if let Some(system) = ctx.system()
            && !c.allowed_systems.contains(system)
        {
            return Some(format!(
                "system '{system}' is not in mission '{}'",
                c.mission_id
            ));
        }
        None
    }

    /// Atomically reserve one call and its declared spend against the immutable
    /// tenant/root-lineage/mission namespace. The aggregate cap deliberately
    /// spans systems; system fan-out is reserved separately by guardrails.
    pub fn reserve(
        &self,
        ledger: &ReservationLedger,
        ctx: &InvokeContext,
    ) -> Result<Option<Reservation>, String> {
        if self.contract.max_tool_calls == u32::MAX && self.contract.spend_ceiling_cents == u64::MAX
        {
            return Ok(None);
        }
        let mut key = ctx
            .reservation_key(Some(self.contract.mission_id.clone()))
            .ok_or_else(|| {
                format!(
                    "mission '{}' requires authenticated tenant/root lineage",
                    self.contract.mission_id
                )
            })?;
        key.system = None;
        let cap = Budget {
            tool_call_cap: if self.contract.max_tool_calls == u32::MAX {
                0
            } else {
                self.contract.max_tool_calls
            },
            usd_cap_cents: if self.contract.spend_ceiling_cents == u64::MAX {
                0
            } else {
                self.contract.spend_ceiling_cents
            },
            ..Budget::default()
        };
        ledger
            .reserve(
                key,
                &cap,
                Spent {
                    tool_calls: 1,
                    usd_cents: ctx.estimated_cost_cents,
                    ..Spent::default()
                },
            )
            .map(Some)
            .map_err(|_| {
                format!(
                    "mission '{}' authority ceiling reached",
                    self.contract.mission_id
                )
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mai_agent::types::ToolAccessRole;

    fn ctx() -> InvokeContext {
        let mut ctx = InvokeContext::unverified("s1", "tok_1", ToolAccessRole::Guest);
        ctx.authority = Some(crate::AuthorityBinding {
            tenant_id: "tenant-a".to_string(),
            root_lineage: "root-a".to_string(),
        });
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
        assert!(
            m.check(&call("s3.get"), &c)
                .unwrap()
                .contains("canonical target")
        );
        c.canonical_system = Some("gcp".to_string());
        assert!(m.check(&call("s3.get"), &c).unwrap().contains("gcp"));
        c.canonical_system = Some("aws".to_string());
        assert!(
            m.check(&call("s3.get"), &c).is_none(),
            "allowed system passes"
        );
    }

    #[test]
    fn call_ceiling_blocks_once_reached() {
        let m = Mission::new(
            MissionContract::new("m1")
                .allow_tool("read.file")
                .with_max_calls(2),
        );
        let ledger = ReservationLedger::new();
        m.reserve(&ledger, &ctx())
            .unwrap()
            .unwrap()
            .commit()
            .unwrap();
        m.reserve(&ledger, &ctx())
            .unwrap()
            .unwrap()
            .commit()
            .unwrap();
        assert!(m.reserve(&ledger, &ctx()).unwrap_err().contains("ceiling"));
    }

    #[test]
    fn spend_ceiling_blocks_when_exceeded() {
        let m = Mission::new(
            MissionContract::new("m1")
                .allow_tool("api.call")
                .with_spend_ceiling_cents(100),
        );
        let ledger = ReservationLedger::new();
        m.reserve(&ledger, &ctx().with_estimated_cost_cents(100))
            .unwrap()
            .unwrap()
            .commit()
            .unwrap();
        assert!(
            m.reserve(&ledger, &ctx().with_estimated_cost_cents(1))
                .unwrap_err()
                .contains("ceiling")
        );
    }
}
