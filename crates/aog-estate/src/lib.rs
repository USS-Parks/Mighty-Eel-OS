//! `aog-estate` — the typed resource model for the Loom orchestration control
//! plane (M3, addendum A1.5). These are Loom's "CRDs": versioned,
//! schema-validated, watchable resource kinds. Each is an envelope of
//! `spec`/`status`/`metadata`, and every object carries a `token_ref` (the
//! authorizing capability) and a `receipt_ref` (its provenance).
//!
//! Every trust-bearing field reuses a **frozen `fabric-contracts` type** —
//! there is no ad-hoc re-declaration of `Classification`, `Budget`, `Caveat`,
//! `Route`, or `ComplianceScope` here (addendum: "extends `fabric-contracts`,
//! no ad-hoc structs"). The store (`aog-store`) persists these as desired
//! state; the apiserver admission chain (`aog-apiserver`) validates, seals,
//! and receipts them.
//!
//! Doctrine: this crate holds **intent only**. Receipts (proof) live in
//! `wsf-ledger`, never here (A1.4).

pub mod kinds;

use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Serialize};

pub use kinds::*;

/// The API group + version every Loom resource is served under. Schema
/// evolution is handled by conversion (K10); this is the current stored form.
pub const API_VERSION: &str = "aog.islandmountain.io/v1";

/// The resource kinds Loom orchestrates (addendum A1.5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Kind {
    Tenant,
    TrustRing,
    VirtualKey,
    Capability,
    PolicyBundle,
    ProviderPool,
    Workload,
    Placement,
    Node,
    MissionContract,
    ToolGrant,
    RolloutPlan,
    RevocationIntent,
}

impl fmt::Display for Kind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Kind::Tenant => "Tenant",
            Kind::TrustRing => "TrustRing",
            Kind::VirtualKey => "VirtualKey",
            Kind::Capability => "Capability",
            Kind::PolicyBundle => "PolicyBundle",
            Kind::ProviderPool => "ProviderPool",
            Kind::Workload => "Workload",
            Kind::Placement => "Placement",
            Kind::Node => "Node",
            Kind::MissionContract => "MissionContract",
            Kind::ToolGrant => "ToolGrant",
            Kind::RolloutPlan => "RolloutPlan",
            Kind::RevocationIntent => "RevocationIntent",
        };
        f.write_str(s)
    }
}

/// A resource failed its schema or structural invariants. Policy denial
/// (HIPAA/ITAR/OCAP, deny-wins) is a separate concern handled by admission.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum EstateError {
    #[error("resource name is empty")]
    EmptyName,
    #[error(
        "invalid resource name {0:?}: expected 1..=63 chars of [a-z0-9-], no leading/trailing '-'"
    )]
    InvalidName(String),
    #[error("{kind} invalid: {reason}")]
    Invalid { kind: Kind, reason: String },
    #[error("type_meta kind {found} does not match resource type {expected}")]
    KindMismatch { expected: Kind, found: Kind },
    #[error("unsupported api_version {0:?}")]
    ApiVersion(String),
    #[error("unknown kind {0:?}")]
    UnknownKind(String),
    #[error("deserialize: {0}")]
    Deserialize(String),
}

/// Structural validation. Fail-closed: an ambiguous or malformed resource is an
/// error, never a silent default (doctrine D7).
pub trait Validate {
    /// # Errors
    /// Returns [`EstateError`] when a structural invariant is violated.
    fn validate(&self) -> Result<(), EstateError>;
}

/// Associates a spec type with the single [`Kind`] it materialises. Enforces the
/// "no ad-hoc structs" rule: a `Resource` can only be built for a spec that
/// declares its kind here.
pub trait EstateKind {
    /// The resource kind this spec is the body of.
    const KIND: Kind;
}

/// Validates a DNS-label-style resource name.
///
/// # Errors
/// [`EstateError::EmptyName`] when empty; [`EstateError::InvalidName`] when it
/// contains anything but `[a-z0-9-]`, exceeds 63 chars, or edge-quotes a `-`.
pub fn validate_name(name: &str) -> Result<(), EstateError> {
    if name.is_empty() {
        return Err(EstateError::EmptyName);
    }
    let well_formed = name.len() <= 63
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        && !name.starts_with('-')
        && !name.ends_with('-');
    if well_formed {
        Ok(())
    } else {
        Err(EstateError::InvalidName(name.to_owned()))
    }
}

/// Group/version/kind discriminator, stored flattened at the object's top level.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TypeMeta {
    pub api_version: String,
    pub kind: Kind,
}

impl TypeMeta {
    #[must_use]
    pub fn new(kind: Kind) -> Self {
        Self {
            api_version: API_VERSION.to_owned(),
            kind,
        }
    }
}

/// Reference to the WSF capability token that authorized this object (A1.5).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenRef {
    pub token_id: String,
}

/// Reference to the provenance receipt for this object's last mutation (A1.5).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReceiptRef {
    pub receipt_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chain: Option<String>,
}

/// Identity + bookkeeping common to every resource.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObjectMeta {
    pub name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub uid: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tenant: Option<String>,
    /// Bumped by the apiserver on every accepted `spec` change.
    #[serde(default)]
    pub generation: u64,
    /// Store revision (set by `aog-store`; `0` until first admitted).
    #[serde(default)]
    pub resource_version: u64,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub labels: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub annotations: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token_ref: Option<TokenRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub receipt_ref: Option<ReceiptRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deletion_timestamp: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub finalizers: Vec<String>,
}

impl ObjectMeta {
    #[must_use]
    pub fn named(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            ..Self::default()
        }
    }

    /// # Errors
    /// Propagates [`validate_name`].
    pub fn validate(&self) -> Result<(), EstateError> {
        validate_name(&self.name)
    }
}

/// A typed Loom resource: `type_meta` + `metadata` + `spec` + optional `status`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Resource<S, T> {
    #[serde(flatten)]
    pub type_meta: TypeMeta,
    pub metadata: ObjectMeta,
    pub spec: S,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<T>,
}

impl<S: EstateKind + Validate, T> Resource<S, T> {
    /// Build a new desired-state resource with its kind fixed by the spec.
    pub fn new(name: impl Into<String>, spec: S) -> Self {
        Self {
            type_meta: TypeMeta::new(S::KIND),
            metadata: ObjectMeta::named(name),
            spec,
            status: None,
        }
    }

    /// The kind this resource materialises.
    #[must_use]
    pub fn kind(&self) -> Kind {
        S::KIND
    }

    /// Full structural validation: name, api-version, kind agreement, spec.
    ///
    /// # Errors
    /// Returns the first invariant violated (fail-closed).
    pub fn validate(&self) -> Result<(), EstateError> {
        self.metadata.validate()?;
        if self.type_meta.api_version != API_VERSION {
            return Err(EstateError::ApiVersion(self.type_meta.api_version.clone()));
        }
        if self.type_meta.kind != S::KIND {
            return Err(EstateError::KindMismatch {
                expected: S::KIND,
                found: self.type_meta.kind,
            });
        }
        self.spec.validate()
    }
}

/// Generates the type-erased [`ResourceObject`] enum plus its dispatch. One
/// variant per kind; the variant name, the [`Kind`] discriminant, and the type
/// alias (from `kinds`) all share the identifier, so the store and apiserver can
/// round-trip any object through JSON without a hand-written match per method.
macro_rules! resource_objects {
    ($($kind:ident),+ $(,)?) => {
        /// Any Loom resource, tagged by its kind — the unit `aog-store` persists
        /// and `aog-apiserver` admits.
        #[derive(Debug, Clone, PartialEq)]
        pub enum ResourceObject {
            $($kind($kind),)+
        }

        impl ResourceObject {
            /// This object's kind.
            #[must_use]
            pub fn kind(&self) -> Kind {
                match self { $(ResourceObject::$kind(_) => Kind::$kind,)+ }
            }

            /// This object's metadata name.
            #[must_use]
            pub fn name(&self) -> &str {
                match self { $(ResourceObject::$kind(r) => r.metadata.name.as_str(),)+ }
            }

            /// Validate the wrapped resource.
            ///
            /// # Errors
            /// Propagates the inner [`Resource::validate`].
            pub fn validate(&self) -> Result<(), EstateError> {
                match self { $(ResourceObject::$kind(r) => r.validate(),)+ }
            }

            /// Serialize to a JSON value.
            ///
            /// # Errors
            /// [`EstateError::Deserialize`] if serialization fails.
            pub fn to_value(&self) -> Result<serde_json::Value, EstateError> {
                let out = match self { $(ResourceObject::$kind(r) => serde_json::to_value(r),)+ };
                out.map_err(|e| EstateError::Deserialize(e.to_string()))
            }

            /// Parse a JSON value into the concrete kind named by its `kind` field.
            ///
            /// # Errors
            /// [`EstateError::UnknownKind`] if `kind` is missing/unrecognized;
            /// [`EstateError::Deserialize`] if the body does not match the kind.
            pub fn from_value(value: serde_json::Value) -> Result<Self, EstateError> {
                let kind = value
                    .get("kind")
                    .cloned()
                    .and_then(|k| serde_json::from_value::<Kind>(k).ok())
                    .ok_or_else(|| EstateError::UnknownKind(
                        value.get("kind").and_then(serde_json::Value::as_str).unwrap_or("<missing>").to_owned(),
                    ))?;
                match kind {
                    $(Kind::$kind => Ok(ResourceObject::$kind(
                        serde_json::from_value(value).map_err(|e| EstateError::Deserialize(e.to_string()))?,
                    )),)+
                }
            }
        }
    };
}

resource_objects!(
    Tenant,
    TrustRing,
    VirtualKey,
    Capability,
    PolicyBundle,
    ProviderPool,
    Workload,
    Placement,
    Node,
    MissionContract,
    ToolGrant,
    RolloutPlan,
    RevocationIntent,
);
