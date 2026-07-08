//! The Loom resource kinds (addendum A1.5). Each is a `spec`/`status` pair
//! wrapped by [`crate::Resource`]. Trust-bearing fields reuse `fabric-contracts`
//! types verbatim; validation checks structural invariants only.

use serde::{Deserialize, Serialize};

use fabric_contracts::{Budget, Caveat, Classification, ComplianceScope, Route, RoutingDecision};

use crate::{EstateError, EstateKind, Kind, Resource, Validate};

// ---- shared sub-types -------------------------------------------------------

/// Lifecycle phase a controller writes into `status`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    #[default]
    Pending,
    Provisioning,
    Ready,
    Degraded,
    Terminating,
    Failed,
}

/// What a [`Workload`] runs as (A1.5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkloadKind {
    Gateway,
    Agent,
    Toolproxy,
    Inference,
}

/// Enforcement posture of a policy bundle (the G6 mode ladder).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PolicyMode {
    #[default]
    Shadow,
    ReportOnly,
    Enforce,
}

/// Rollout advancement strategy (Phase O).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RolloutStrategy {
    #[default]
    Progressive,
    Canary,
    BlueGreen,
}

/// Hardware root backing a node's attestation floor (S4).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AttestationPlatform {
    #[default]
    None,
    Tpm,
    NitroEnclave,
    SevSnp,
}

/// How a node proves its trust floor. `air_gapped` gates cloud routes (I-8).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttestationProfile {
    #[serde(default)]
    pub platform: AttestationPlatform,
    #[serde(default)]
    pub air_gapped: bool,
    /// Expected PCR / measurement digest, when `platform` provides one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pcr: Option<String>,
}

/// Schedulable capacity / utilisation for a node.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Capacity {
    #[serde(default)]
    pub cpu_millis: u64,
    #[serde(default)]
    pub memory_mb: u64,
    #[serde(default)]
    pub gpu: u32,
    #[serde(default)]
    pub max_workloads: u32,
}

/// One rule in a policy bundle. `when` is an opaque predicate the policy engine
/// (mai-compliance composer) interprets; the estate stores it verbatim.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PolicyRule {
    pub name: String,
    pub effect: RoutingDecision,
    #[serde(default)]
    pub when: String,
}

/// A model served by a provider pool.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelEndpoint {
    pub model: String,
    pub route: Route,
    #[serde(default)]
    pub cost_cents_per_ktoken: u64,
    #[serde(default)]
    pub healthy: bool,
}

/// The subject of a revocation (R9). A ring or tenant fans out to many tokens.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "target", content = "id")]
pub enum RevocationTarget {
    Token(String),
    Subject(String),
    Ring(u8),
    Tenant(String),
}

// ---- helpers ----------------------------------------------------------------

fn ring_ok(ring: u8) -> bool {
    (1..=3).contains(&ring)
}

fn invalid(kind: Kind, reason: impl Into<String>) -> EstateError {
    EstateError::Invalid {
        kind,
        reason: reason.into(),
    }
}

fn one() -> u32 {
    1
}

// ---- Tenant -----------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TenantSpec {
    pub display_name: String,
    pub ring: u8,
    pub classification_ceiling: Classification,
    #[serde(default)]
    pub compliance_scopes: Vec<ComplianceScope>,
    #[serde(default)]
    pub subject_hmac_rotation_days: u32,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TenantStatus {
    #[serde(default)]
    pub phase: Phase,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub openbao_path: Option<String>,
    #[serde(default)]
    pub issued_tokens: u64,
}

impl EstateKind for TenantSpec {
    const KIND: Kind = Kind::Tenant;
}

impl Validate for TenantSpec {
    fn validate(&self) -> Result<(), EstateError> {
        if self.display_name.trim().is_empty() {
            return Err(invalid(Kind::Tenant, "display_name is empty"));
        }
        if !ring_ok(self.ring) {
            return Err(invalid(
                Kind::Tenant,
                format!("ring {} not in 1..=3", self.ring),
            ));
        }
        Ok(())
    }
}

pub type Tenant = Resource<TenantSpec, TenantStatus>;

// ---- TrustRing --------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrustRingSpec {
    pub ring: u8,
    pub transit_key: String,
    #[serde(default)]
    pub attestation: AttestationProfile,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrustRingStatus {
    #[serde(default)]
    pub phase: Phase,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key_version: Option<u32>,
    /// A disabled ring key darkens the ring (R4): its envelopes stop unsealing.
    #[serde(default)]
    pub dark: bool,
}

impl EstateKind for TrustRingSpec {
    const KIND: Kind = Kind::TrustRing;
}

impl Validate for TrustRingSpec {
    fn validate(&self) -> Result<(), EstateError> {
        if !ring_ok(self.ring) {
            return Err(invalid(
                Kind::TrustRing,
                format!("ring {} not in 1..=3", self.ring),
            ));
        }
        if self.transit_key.trim().is_empty() {
            return Err(invalid(Kind::TrustRing, "transit_key is empty"));
        }
        Ok(())
    }
}

pub type TrustRing = Resource<TrustRingSpec, TrustRingStatus>;

// ---- VirtualKey -------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VirtualKeySpec {
    pub tenant: String,
    /// Name of the [`Capability`] this key resolves to.
    pub capability: String,
    #[serde(default)]
    pub display_name: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct VirtualKeyStatus {
    #[serde(default)]
    pub phase: Phase,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolved_token: Option<String>,
}

impl EstateKind for VirtualKeySpec {
    const KIND: Kind = Kind::VirtualKey;
}

impl Validate for VirtualKeySpec {
    fn validate(&self) -> Result<(), EstateError> {
        if self.tenant.trim().is_empty() {
            return Err(invalid(Kind::VirtualKey, "tenant is empty"));
        }
        if self.capability.trim().is_empty() {
            return Err(invalid(Kind::VirtualKey, "capability is empty"));
        }
        Ok(())
    }
}

pub type VirtualKey = Resource<VirtualKeySpec, VirtualKeyStatus>;

// ---- Capability -------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapabilitySpec {
    #[serde(default)]
    pub budget: Budget,
    #[serde(default)]
    pub caveats: Vec<Caveat>,
    #[serde(default)]
    pub allowed_routes: Vec<Route>,
    #[serde(default)]
    pub allowed_models: Vec<String>,
    pub max_classification: Classification,
    /// Token lifetime. Must be non-zero: zero standing privilege (I-1).
    pub ttl_seconds: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapabilityStatus {
    #[serde(default)]
    pub phase: Phase,
    #[serde(default)]
    pub issued: u64,
}

impl EstateKind for CapabilitySpec {
    const KIND: Kind = Kind::Capability;
}

impl Validate for CapabilitySpec {
    fn validate(&self) -> Result<(), EstateError> {
        if self.ttl_seconds == 0 {
            return Err(invalid(
                Kind::Capability,
                "ttl_seconds must be non-zero (zero standing privilege, I-1)",
            ));
        }
        Ok(())
    }
}

pub type Capability = Resource<CapabilitySpec, CapabilityStatus>;

// ---- PolicyBundle -----------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PolicyBundleSpec {
    pub version: u32,
    #[serde(default)]
    pub mode: PolicyMode,
    #[serde(default)]
    pub rules: Vec<PolicyRule>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PolicyBundleStatus {
    #[serde(default)]
    pub phase: Phase,
    #[serde(default)]
    pub distributed_to: Vec<String>,
}

impl EstateKind for PolicyBundleSpec {
    const KIND: Kind = Kind::PolicyBundle;
}

impl Validate for PolicyBundleSpec {
    fn validate(&self) -> Result<(), EstateError> {
        if self.version == 0 {
            return Err(invalid(Kind::PolicyBundle, "version must be >= 1"));
        }
        for rule in &self.rules {
            if rule.name.trim().is_empty() {
                return Err(invalid(Kind::PolicyBundle, "a rule has an empty name"));
            }
        }
        Ok(())
    }
}

pub type PolicyBundle = Resource<PolicyBundleSpec, PolicyBundleStatus>;

// ---- ProviderPool -----------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderPoolSpec {
    pub provider: String,
    #[serde(default)]
    pub endpoints: Vec<ModelEndpoint>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderPoolStatus {
    #[serde(default)]
    pub phase: Phase,
    #[serde(default)]
    pub healthy: Vec<String>,
}

impl EstateKind for ProviderPoolSpec {
    const KIND: Kind = Kind::ProviderPool;
}

impl Validate for ProviderPoolSpec {
    fn validate(&self) -> Result<(), EstateError> {
        if self.provider.trim().is_empty() {
            return Err(invalid(Kind::ProviderPool, "provider is empty"));
        }
        for ep in &self.endpoints {
            if ep.model.trim().is_empty() {
                return Err(invalid(
                    Kind::ProviderPool,
                    "a model endpoint has an empty model",
                ));
            }
        }
        Ok(())
    }
}

pub type ProviderPool = Resource<ProviderPoolSpec, ProviderPoolStatus>;

// ---- Workload ---------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkloadSpec {
    pub workload_kind: WorkloadKind,
    #[serde(default = "one")]
    pub replicas: u32,
    pub ring: u8,
    /// The workload's data-classification ceiling — must be `<=` the target
    /// node's attestation floor to be placed (S4).
    pub classification_ceiling: Classification,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image: Option<String>,
    #[serde(default)]
    pub command: Vec<String>,
    /// Name of the [`Capability`] whose scope the runtime token is minted from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capability: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkloadStatus {
    #[serde(default)]
    pub phase: Phase,
    #[serde(default)]
    pub ready_replicas: u32,
    #[serde(default)]
    pub placements: Vec<String>,
}

impl EstateKind for WorkloadSpec {
    const KIND: Kind = Kind::Workload;
}

impl Validate for WorkloadSpec {
    fn validate(&self) -> Result<(), EstateError> {
        if !ring_ok(self.ring) {
            return Err(invalid(
                Kind::Workload,
                format!("ring {} not in 1..=3", self.ring),
            ));
        }
        Ok(())
    }
}

pub type Workload = Resource<WorkloadSpec, WorkloadStatus>;

// ---- Placement --------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlacementSpec {
    pub workload: String,
    pub node: String,
    /// The runtime token minted for this binding (S7).
    #[serde(default)]
    pub token_id: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlacementStatus {
    #[serde(default)]
    pub phase: Phase,
    #[serde(default)]
    pub bound: bool,
}

impl EstateKind for PlacementSpec {
    const KIND: Kind = Kind::Placement;
}

impl Validate for PlacementSpec {
    fn validate(&self) -> Result<(), EstateError> {
        if self.workload.trim().is_empty() {
            return Err(invalid(Kind::Placement, "workload is empty"));
        }
        if self.node.trim().is_empty() {
            return Err(invalid(Kind::Placement, "node is empty"));
        }
        Ok(())
    }
}

pub type Placement = Resource<PlacementSpec, PlacementStatus>;

// ---- Node -------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeSpec {
    pub ring: u8,
    /// Highest classification this node is attested to hold (the S4 floor).
    pub attestation_floor: Classification,
    #[serde(default)]
    pub attestation: AttestationProfile,
    #[serde(default)]
    pub capacity: Capacity,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeStatus {
    #[serde(default)]
    pub phase: Phase,
    #[serde(default)]
    pub ready: bool,
    #[serde(default)]
    pub allocatable: Capacity,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_heartbeat: Option<String>,
}

impl EstateKind for NodeSpec {
    const KIND: Kind = Kind::Node;
}

impl Validate for NodeSpec {
    fn validate(&self) -> Result<(), EstateError> {
        if !ring_ok(self.ring) {
            return Err(invalid(
                Kind::Node,
                format!("ring {} not in 1..=3", self.ring),
            ));
        }
        Ok(())
    }
}

pub type Node = Resource<NodeSpec, NodeStatus>;

// ---- MissionContract --------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MissionContractSpec {
    #[serde(default)]
    pub allowed_tools: Vec<String>,
    #[serde(default)]
    pub allowed_systems: Vec<String>,
    /// Hard ceiling on tool calls for the mission (fail-closed at exhaustion).
    pub call_ceiling: u32,
    #[serde(default)]
    pub spend: Budget,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MissionContractStatus {
    #[serde(default)]
    pub phase: Phase,
    #[serde(default)]
    pub calls_used: u32,
}

impl EstateKind for MissionContractSpec {
    const KIND: Kind = Kind::MissionContract;
}

impl Validate for MissionContractSpec {
    fn validate(&self) -> Result<(), EstateError> {
        if self.call_ceiling == 0 {
            return Err(invalid(Kind::MissionContract, "call_ceiling must be >= 1"));
        }
        Ok(())
    }
}

pub type MissionContract = Resource<MissionContractSpec, MissionContractStatus>;

// ---- ToolGrant --------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolGrantSpec {
    pub tool: String,
    #[serde(default)]
    pub systems: Vec<String>,
    #[serde(default)]
    pub requires_approval: bool,
    /// OpenBao path the per-call credential is minted from (T2), never a secret.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credential_ref: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolGrantStatus {
    #[serde(default)]
    pub phase: Phase,
}

impl EstateKind for ToolGrantSpec {
    const KIND: Kind = Kind::ToolGrant;
}

impl Validate for ToolGrantSpec {
    fn validate(&self) -> Result<(), EstateError> {
        if self.tool.trim().is_empty() {
            return Err(invalid(Kind::ToolGrant, "tool is empty"));
        }
        Ok(())
    }
}

pub type ToolGrant = Resource<ToolGrantSpec, ToolGrantStatus>;

// ---- RolloutPlan ------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RolloutPlanSpec {
    /// Name of the `Workload` / `PolicyBundle` / `ProviderPool` being rolled.
    pub target: String,
    #[serde(default)]
    pub strategy: RolloutStrategy,
    #[serde(default = "one")]
    pub max_surge: u32,
    #[serde(default)]
    pub max_unavailable: u32,
    /// Max target errors tolerated mid-rollout before auto-rollback (O3). `0`
    /// disables auto-rollback; `>0` is the error budget the rollback controller
    /// enforces from receipts/meter telemetry.
    #[serde(default)]
    pub error_budget: u32,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RolloutPlanStatus {
    #[serde(default)]
    pub phase: Phase,
    #[serde(default)]
    pub step: u32,
    /// Set once the error budget was breached and the rollout reversed to its
    /// prior state (O3). A rolled-back rollout ends `Failed`, not `Ready`.
    #[serde(default)]
    pub rolled_back: bool,
}

impl EstateKind for RolloutPlanSpec {
    const KIND: Kind = Kind::RolloutPlan;
}

impl Validate for RolloutPlanSpec {
    fn validate(&self) -> Result<(), EstateError> {
        if self.target.trim().is_empty() {
            return Err(invalid(Kind::RolloutPlan, "target is empty"));
        }
        if self.max_surge == 0 && self.max_unavailable == 0 {
            return Err(invalid(
                Kind::RolloutPlan,
                "max_surge and max_unavailable cannot both be 0 (rollout would stall)",
            ));
        }
        Ok(())
    }
}

pub type RolloutPlan = Resource<RolloutPlanSpec, RolloutPlanStatus>;

// ---- RevocationIntent -------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RevocationIntentSpec {
    pub target: RevocationTarget,
    #[serde(default)]
    pub reason: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RevocationIntentStatus {
    #[serde(default)]
    pub phase: Phase,
    #[serde(default)]
    pub propagated: bool,
    #[serde(default)]
    pub replicas_denied: u32,
}

impl EstateKind for RevocationIntentSpec {
    const KIND: Kind = Kind::RevocationIntent;
}

impl Validate for RevocationIntentSpec {
    fn validate(&self) -> Result<(), EstateError> {
        match &self.target {
            RevocationTarget::Token(id)
            | RevocationTarget::Subject(id)
            | RevocationTarget::Tenant(id) => {
                if id.trim().is_empty() {
                    return Err(invalid(Kind::RevocationIntent, "target id is empty"));
                }
            }
            RevocationTarget::Ring(ring) => {
                if !ring_ok(*ring) {
                    return Err(invalid(
                        Kind::RevocationIntent,
                        format!("ring {ring} not in 1..=3"),
                    ));
                }
            }
        }
        Ok(())
    }
}

pub type RevocationIntent = Resource<RevocationIntentSpec, RevocationIntentStatus>;
