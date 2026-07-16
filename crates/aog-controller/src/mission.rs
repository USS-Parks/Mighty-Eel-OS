//! O5 — the MissionContract operator: it turns a Phase-T mission scope envelope
//! into the concrete authority an agent run may use, and enforces that the run
//! cannot exceed it. The contract declares the tools and systems a mission may
//! touch plus a hard call ceiling; the operator materializes one owned
//! `ToolGrant` per allowed tool (so the toolproxy — O6 — only ever mints
//! credentials the contract sanctioned) and the pure [`mission_allows`] gate
//! denies any action outside the declared tools/systems or past the call ceiling.
//!
//! Two halves, each independently testable:
//!   * **enforcement** — [`mission_allows`] is the fail-closed decision the agent
//!     runtime consults per action; scope and the call budget are checked with no
//!     estate read. The monetary `spend` budget rides the derived grant's
//!     credential (the existing SpendLedger), so it is enforced where spend is;
//!   * **materialization** — the [`MissionContractController`] reconciles the
//!     contract's `allowed_tools` into owned `ToolGrant`s (created for new tools,
//!     pruned for withdrawn ones), and marks the contract `Failed` once its call
//!     ceiling is spent, `Ready` otherwise.

use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::future::Future;

use aog_estate::{
    Kind, MissionContract, MissionContractSpec, MissionContractStatus, OwnerRef, Phase, Resource,
    ResourceObject, ToolGrant, ToolGrantSpec,
};

use crate::objects::{EstateClient, parse_key};
use crate::runtime::{Action, ReconcileError, Reconciler};

/// One action an agent proposes during a mission run.
#[derive(Debug, Clone, Copy)]
pub struct MissionRequest<'a> {
    /// The tool the agent wants to call.
    pub tool: &'a str,
    /// The target system, when the action names one.
    pub system: Option<&'a str>,
}

/// Whether the mission permits an action.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MissionVerdict {
    /// Within the mission's declared scope and call budget.
    Allow,
    /// Refused, with the reason (surfaced to the agent / audit).
    Deny(String),
}

impl MissionVerdict {
    /// Whether the action is permitted.
    #[must_use]
    pub fn is_allowed(&self) -> bool {
        matches!(self, MissionVerdict::Allow)
    }
}

/// The pure enforcement decision (the O5 gate): an agent bound to `spec` — with
/// `status` calls used so far — may take `req` only if the tool is in
/// `allowed_tools`, the system (when the contract restricts systems) is in
/// `allowed_systems`, and the call ceiling is not yet reached. Every failure is
/// fail-closed: an out-of-scope or over-budget action is denied, so a run can
/// never exceed its contract (doctrine I-4).
#[must_use]
pub fn mission_allows(
    spec: &MissionContractSpec,
    status: &MissionContractStatus,
    req: &MissionRequest,
) -> MissionVerdict {
    if status.calls_used >= spec.call_ceiling {
        return MissionVerdict::Deny(format!(
            "mission call ceiling {} reached",
            spec.call_ceiling
        ));
    }
    if !spec.allowed_tools.iter().any(|t| t == req.tool) {
        return MissionVerdict::Deny(format!(
            "tool {:?} is not in the mission's allowed tools",
            req.tool
        ));
    }
    // An empty allowed_systems means the mission does not restrict systems; a
    // non-empty list is a closed set the target must belong to.
    if !spec.allowed_systems.is_empty() {
        match req.system {
            Some(sys) if spec.allowed_systems.iter().any(|s| s == sys) => {}
            Some(sys) => {
                return MissionVerdict::Deny(format!(
                    "system {sys:?} is not in the mission's allowed systems"
                ));
            }
            None => {
                return MissionVerdict::Deny(
                    "the mission restricts systems; the action must name one".to_owned(),
                );
            }
        }
    }
    MissionVerdict::Allow
}

/// Sanitize an arbitrary tool string into a DNS-label segment (`[a-z0-9-]`),
/// so a derived `ToolGrant` name always validates.
fn label(s: &str) -> String {
    let mapped: String = s
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    let trimmed = mapped.trim_matches('-');
    if trimmed.is_empty() {
        "x".to_owned()
    } else {
        trimmed.to_owned()
    }
}

/// The `ToolGrant` name derived for `tool` under mission `mission`. Capped at the
/// 63-char name limit, never edge-quoting a hyphen.
fn grant_name(tenant: &str, mission_uid: &str, mission: &str, tool: &str) -> String {
    let digest = Sha256::digest(format!("{tenant}\0{mission_uid}\0{mission}\0{tool}").as_bytes());
    let suffix = &hex::encode(digest)[..10];
    let mut name = format!("{mission}-t-{}-{suffix}", label(tool));
    name.truncate(63);
    name.trim_end_matches('-').to_owned()
}

/// Reconciles `MissionContract`s into owned `ToolGrant`s. Run it on a
/// `"MissionContract/"` informer.
#[derive(Clone)]
pub struct MissionContractController {
    client: EstateClient,
}

impl MissionContractController {
    #[must_use]
    pub fn new(client: EstateClient) -> Self {
        Self { client }
    }

    /// Every live `ToolGrant` owned by mission `mission`.
    async fn owned_grants(
        &self,
        mission: &str,
        mission_uid: &str,
        tenant: Option<&str>,
    ) -> Result<Vec<ToolGrant>, ReconcileError> {
        let mut out = Vec::new();
        for object in self.client.list(Kind::ToolGrant).await? {
            if let ResourceObject::ToolGrant(grant) = object
                && grant.metadata.owner_refs.iter().any(|o| {
                    o.kind == Kind::MissionContract && o.name == mission && o.uid == mission_uid
                })
                && grant.metadata.tenant.as_deref() == tenant
            {
                out.push(grant);
            }
        }
        Ok(out)
    }

    async fn set_status(
        &self,
        contract: MissionContract,
        phase: Phase,
    ) -> Result<(), ReconcileError> {
        let calls_used = contract.status.as_ref().map_or(0, |s| s.calls_used);
        let desired = MissionContractStatus { phase, calls_used };
        if contract.status.as_ref() != Some(&desired) {
            let mut converged = contract;
            converged.status = Some(desired);
            self.client
                .update(ResourceObject::MissionContract(converged))
                .await?;
        }
        Ok(())
    }

    async fn reconcile_mission(&self, name: &str) -> Result<Action, ReconcileError> {
        let Some(ResourceObject::MissionContract(contract)) =
            self.client.get(Kind::MissionContract, name).await?
        else {
            return Ok(Action::Done);
        };
        if contract.metadata.deletion_timestamp.is_some() {
            return Ok(Action::Done); // owned grants are the GC's to reclaim
        }

        // Desired grants: one per allowed tool, keyed by its derived name.
        let desired: BTreeMap<String, String> = contract
            .spec
            .allowed_tools
            .iter()
            .map(|tool| {
                (
                    grant_name(
                        contract.metadata.tenant.as_deref().unwrap_or_default(),
                        &contract.metadata.uid,
                        name,
                        tool,
                    ),
                    tool.clone(),
                )
            })
            .collect();
        let existing = self
            .owned_grants(
                name,
                &contract.metadata.uid,
                contract.metadata.tenant.as_deref(),
            )
            .await?;
        let existing_names: BTreeSet<String> =
            existing.iter().map(|g| g.metadata.name.clone()).collect();

        // Create a grant for each newly-allowed tool, scoped to the mission's
        // systems and owned by the contract (so the GC cascades on delete).
        for (gname, tool) in &desired {
            if !existing_names.contains(gname) {
                let mut grant = Resource::new(
                    gname.clone(),
                    ToolGrantSpec {
                        tool: tool.clone(),
                        systems: contract.spec.allowed_systems.clone(),
                        requires_approval: false,
                        credential_ref: None,
                    },
                );
                grant.metadata.owner_refs.push(OwnerRef {
                    kind: Kind::MissionContract,
                    name: name.to_owned(),
                    uid: contract.metadata.uid.clone(),
                });
                grant.metadata.tenant.clone_from(&contract.metadata.tenant);
                self.client
                    .ensure_created(ResourceObject::ToolGrant(grant))
                    .await?;
            }
        }
        // Prune grants for tools the contract no longer allows (scope shrank).
        for grant in &existing {
            if !desired.contains_key(&grant.metadata.name) {
                self.client
                    .delete(Kind::ToolGrant, &grant.metadata.name)
                    .await?;
            } else if grant.spec.systems != contract.spec.allowed_systems {
                let mut narrowed = grant.clone();
                narrowed.spec.systems = contract.spec.allowed_systems.clone();
                self.client
                    .update(ResourceObject::ToolGrant(narrowed))
                    .await?;
            }
        }

        // A spent contract is Failed (its runs are done); otherwise Ready.
        let calls_used = contract.status.as_ref().map_or(0, |s| s.calls_used);
        let phase = if calls_used >= contract.spec.call_ceiling {
            Phase::Failed
        } else {
            Phase::Ready
        };
        self.set_status(contract, phase).await?;
        Ok(Action::Done)
    }
}

impl Reconciler for MissionContractController {
    fn reconcile(&self, key: &str) -> impl Future<Output = Result<Action, ReconcileError>> + Send {
        let controller = self.clone();
        let key = key.to_owned();
        async move {
            let Some((Kind::MissionContract, name)) = parse_key(&key) else {
                return Ok(Action::Done);
            };
            controller.reconcile_mission(name).await
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(tools: &[&str], systems: &[&str], ceiling: u32) -> MissionContractSpec {
        MissionContractSpec {
            allowed_tools: tools.iter().map(|s| (*s).to_owned()).collect(),
            allowed_systems: systems.iter().map(|s| (*s).to_owned()).collect(),
            call_ceiling: ceiling,
            spend: fabric_contracts::Budget::default(),
        }
    }

    fn used(calls: u32) -> MissionContractStatus {
        MissionContractStatus {
            phase: Phase::Ready,
            calls_used: calls,
        }
    }

    fn req<'a>(tool: &'a str, system: Option<&'a str>) -> MissionRequest<'a> {
        MissionRequest { tool, system }
    }

    #[test]
    fn allows_an_in_scope_action_within_budget() {
        let s = spec(&["search", "calc"], &["crm"], 25);
        assert!(mission_allows(&s, &used(3), &req("search", Some("crm"))).is_allowed());
    }

    #[test]
    fn denies_a_tool_outside_the_contract() {
        let s = spec(&["search"], &[], 25);
        assert!(!mission_allows(&s, &used(0), &req("delete_db", None)).is_allowed());
    }

    #[test]
    fn denies_a_system_outside_the_contract() {
        let s = spec(&["search"], &["crm"], 25);
        let v = mission_allows(&s, &used(0), &req("search", Some("prod-db")));
        assert!(matches!(v, MissionVerdict::Deny(_)));
    }

    #[test]
    fn denies_when_a_restricted_mission_names_no_system() {
        let s = spec(&["search"], &["crm"], 25);
        assert!(!mission_allows(&s, &used(0), &req("search", None)).is_allowed());
    }

    #[test]
    fn an_unrestricted_mission_allows_any_system() {
        // Empty allowed_systems = no system restriction.
        let s = spec(&["search"], &[], 25);
        assert!(mission_allows(&s, &used(0), &req("search", Some("anything"))).is_allowed());
        assert!(mission_allows(&s, &used(0), &req("search", None)).is_allowed());
    }

    #[test]
    fn denies_once_the_call_ceiling_is_spent() {
        let s = spec(&["search"], &[], 5);
        assert!(mission_allows(&s, &used(4), &req("search", None)).is_allowed());
        assert!(!mission_allows(&s, &used(5), &req("search", None)).is_allowed());
        assert!(!mission_allows(&s, &used(6), &req("search", None)).is_allowed());
    }

    #[test]
    fn derived_grant_names_are_valid_labels() {
        // Arbitrary tool strings sanitize to DNS labels.
        for (mission, tool) in [("m1", "search"), ("mission-a", "Delete_DB"), ("m", "a/b c")] {
            let name = grant_name("tenant-a", "uid-a", mission, tool);
            assert!(
                aog_estate::validate_name(&name).is_ok(),
                "grant name {name:?} must validate"
            );
        }
    }
}
