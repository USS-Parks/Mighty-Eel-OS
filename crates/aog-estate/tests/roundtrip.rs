//! Every kind round-trips through JSON and rejects malformed input.

use aog_estate::{
    API_VERSION, AttestationProfile, Capability, CapabilitySpec, Capacity, EstateError, Kind,
    MissionContract, MissionContractSpec, Node, NodeSpec, Placement, PlacementSpec, PolicyBundle,
    PolicyBundleSpec, PolicyMode, PolicyRule, ProviderPool, ProviderPoolSpec, Resource,
    ResourceObject, RevocationIntent, RevocationIntentSpec, RevocationTarget, RolloutPlan,
    RolloutPlanSpec, RolloutStrategy, Tenant, TenantSpec, ToolGrant, ToolGrantSpec, TrustRing,
    TrustRingSpec, VirtualKey, VirtualKeySpec, Workload, WorkloadKind, WorkloadSpec,
};
use fabric_contracts::{Budget, Classification, ComplianceScope, Route, RoutingDecision};

// ---- sample builders (one valid instance per kind) --------------------------

fn tenant() -> Tenant {
    Resource::new(
        "acme",
        TenantSpec {
            display_name: "Acme Health".to_owned(),
            ring: 3,
            classification_ceiling: Classification::Controlled,
            compliance_scopes: vec![ComplianceScope::Hipaa],
            subject_hmac_rotation_days: 30,
        },
    )
}

fn trust_ring() -> TrustRing {
    Resource::new(
        "ring-3",
        TrustRingSpec {
            ring: 3,
            transit_key: "transit/ring-3".to_owned(),
            attestation: AttestationProfile::default(),
        },
    )
}

fn virtual_key() -> VirtualKey {
    Resource::new(
        "vk-demo",
        VirtualKeySpec {
            tenant: "acme".to_owned(),
            capability: "cap-chat".to_owned(),
            display_name: "Demo key".to_owned(),
        },
    )
}

fn capability() -> Capability {
    Resource::new(
        "cap-chat",
        CapabilitySpec {
            budget: Budget::default(),
            caveats: vec![],
            allowed_routes: vec![Route::LocalPreferred],
            allowed_models: vec!["demo".to_owned()],
            max_classification: Classification::Internal,
            ttl_seconds: 900,
        },
    )
}

fn policy_bundle() -> PolicyBundle {
    Resource::new(
        "hipaa-v1",
        PolicyBundleSpec {
            version: 1,
            mode: PolicyMode::default(),
            rules: vec![PolicyRule {
                name: "phi-local-only".to_owned(),
                effect: RoutingDecision::LocalOnly,
                when: "classification>=controlled".to_owned(),
            }],
        },
    )
}

fn provider_pool() -> ProviderPool {
    Resource::new(
        "openai",
        ProviderPoolSpec {
            provider: "openai".to_owned(),
            endpoints: vec![],
        },
    )
}

fn workload() -> Workload {
    Resource::new(
        "gateway",
        WorkloadSpec {
            workload_kind: WorkloadKind::Gateway,
            replicas: 3,
            ring: 2,
            classification_ceiling: Classification::Restricted,
            image: Some("aog-gateway:latest".to_owned()),
            command: vec![],
            capability: Some("cap-chat".to_owned()),
        },
    )
}

fn placement() -> Placement {
    Resource::new(
        "gateway-node1",
        PlacementSpec {
            workload: "gateway".to_owned(),
            node: "node1".to_owned(),
            token_id: "tok-1".to_owned(),
        },
    )
}

fn node() -> Node {
    Resource::new(
        "node1",
        NodeSpec {
            ring: 3,
            attestation_floor: Classification::Controlled,
            attestation: AttestationProfile::default(),
            capacity: Capacity::default(),
        },
    )
}

fn mission_contract() -> MissionContract {
    Resource::new(
        "mission-1",
        MissionContractSpec {
            allowed_tools: vec!["search".to_owned()],
            allowed_systems: vec![],
            call_ceiling: 25,
            spend: Budget::default(),
        },
    )
}

fn tool_grant() -> ToolGrant {
    Resource::new(
        "grant-search",
        ToolGrantSpec {
            tool: "search".to_owned(),
            systems: vec![],
            requires_approval: true,
            credential_ref: None,
        },
    )
}

fn rollout_plan() -> RolloutPlan {
    Resource::new(
        "gateway-rollout",
        RolloutPlanSpec {
            target: "gateway".to_owned(),
            strategy: RolloutStrategy::default(),
            max_surge: 1,
            max_unavailable: 0,
            error_budget: 3,
        },
    )
}

fn revocation_intent() -> RevocationIntent {
    Resource::new(
        "revoke-tok-1",
        RevocationIntentSpec {
            target: RevocationTarget::Token("tok-1".to_owned()),
            reason: "compromised".to_owned(),
        },
    )
}

/// Round-trip a typed resource through JSON, validate it, and confirm the
/// type-erased [`ResourceObject`] path reproduces it identically.
macro_rules! roundtrip {
    ($name:ident, $ctor:expr, $kind:expr) => {
        #[test]
        fn $name() {
            let obj = $ctor;
            obj.validate().expect("sample must be valid");

            let json = serde_json::to_string(&obj).expect("serialize");
            let back = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(obj, back, "typed round-trip must be lossless");

            let value = serde_json::to_value(&obj).expect("to_value");
            let erased = ResourceObject::from_value(value).expect("from_value");
            assert_eq!(erased.kind(), $kind);
            assert_eq!(erased.name(), obj.metadata.name.as_str());
            erased.validate().expect("erased validate");
        }
    };
}

roundtrip!(rt_tenant, tenant(), Kind::Tenant);
roundtrip!(rt_trust_ring, trust_ring(), Kind::TrustRing);
roundtrip!(rt_virtual_key, virtual_key(), Kind::VirtualKey);
roundtrip!(rt_capability, capability(), Kind::Capability);
roundtrip!(rt_policy_bundle, policy_bundle(), Kind::PolicyBundle);
roundtrip!(rt_provider_pool, provider_pool(), Kind::ProviderPool);
roundtrip!(rt_workload, workload(), Kind::Workload);
roundtrip!(rt_placement, placement(), Kind::Placement);
roundtrip!(rt_node, node(), Kind::Node);
roundtrip!(
    rt_mission_contract,
    mission_contract(),
    Kind::MissionContract
);
roundtrip!(rt_tool_grant, tool_grant(), Kind::ToolGrant);
roundtrip!(rt_rollout_plan, rollout_plan(), Kind::RolloutPlan);
roundtrip!(
    rt_revocation_intent,
    revocation_intent(),
    Kind::RevocationIntent
);

// ---- schema-reject cases ----------------------------------------------------

#[test]
fn rejects_bad_name() {
    let mut t = tenant();
    t.metadata.name = "Acme_Corp".to_owned(); // uppercase + underscore
    assert!(matches!(t.validate(), Err(EstateError::InvalidName(_))));
}

#[test]
fn rejects_empty_name() {
    let mut t = tenant();
    t.metadata.name = String::new();
    assert!(matches!(t.validate(), Err(EstateError::EmptyName)));
}

#[test]
fn rejects_bad_ring() {
    let mut t = tenant();
    t.spec.ring = 9;
    assert!(matches!(t.validate(), Err(EstateError::Invalid { .. })));
}

#[test]
fn rejects_kind_mismatch() {
    let mut t = tenant();
    t.type_meta.kind = Kind::Node;
    assert!(matches!(
        t.validate(),
        Err(EstateError::KindMismatch {
            expected: Kind::Tenant,
            found: Kind::Node
        })
    ));
}

#[test]
fn rejects_unknown_api_version() {
    let mut t = tenant();
    t.type_meta.api_version = "aog.islandmountain.io/v0".to_owned();
    assert!(matches!(t.validate(), Err(EstateError::ApiVersion(_))));
}

#[test]
fn capability_requires_nonzero_ttl() {
    let mut c = capability();
    c.spec.ttl_seconds = 0;
    assert!(
        c.validate().is_err(),
        "ttl_seconds 0 violates zero standing privilege (I-1)"
    );
}

#[test]
fn from_value_unknown_kind_errs() {
    let value = serde_json::json!({
        "kind": "Bogus",
        "api_version": API_VERSION,
        "metadata": { "name": "x" },
        "spec": {}
    });
    assert!(matches!(
        ResourceObject::from_value(value),
        Err(EstateError::UnknownKind(_))
    ));
}

#[test]
fn from_value_body_mismatch_errs() {
    // kind says Node but body is a Tenant spec — deserialize must fail.
    let value = serde_json::json!({
        "kind": "Node",
        "api_version": API_VERSION,
        "metadata": { "name": "x" },
        "spec": { "display_name": "oops", "ring": 3, "classification_ceiling": "internal" }
    });
    assert!(matches!(
        ResourceObject::from_value(value),
        Err(EstateError::Deserialize(_))
    ));
}
