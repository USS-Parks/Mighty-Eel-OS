//! Store-key scheme + the encode/decode shared by the read path
//! ([`crate::reader`]) and the write path ([`crate::admission`]), kept in one
//! place so the on-the-wire form can never drift between them.

use aog_estate::{Kind, ResourceObject};
use aog_store::Versioned;

use crate::error::ApiError;

/// The store key for one object: `"<Kind>/<name>"`. The trailing separator lets a
/// kind be listed by prefix (`"<Kind>/"`) without one kind's prefix matching
/// another's keys.
#[must_use]
pub fn store_key(kind: Kind, name: &str) -> String {
    format!("{kind}/{name}")
}

/// The list prefix for a kind.
#[must_use]
pub fn kind_prefix(kind: Kind) -> String {
    format!("{kind}/")
}

/// Map a URL path segment to a [`Kind`]. Accepts the canonical kind name
/// (e.g. `Tenant`) and reuses the `Kind` deserializer, so the mapping can never
/// drift from the enum. `aogctl` (K11) parses the same way.
#[must_use]
pub fn parse_kind(segment: &str) -> Option<Kind> {
    serde_json::from_value::<Kind>(serde_json::Value::String(segment.to_owned())).ok()
}

/// Serialize an object to its stored bytes.
///
/// `resource_version` is deliberately not authoritative in the stored body — it
/// is the store's `mod_revision`, overlaid on read (etcd/K8s convention) — so a
/// write never has to know its own revision in advance.
///
/// # Errors
/// [`ApiError::Invalid`] / [`ApiError::Store`] if the object cannot be encoded.
pub fn encode(object: &ResourceObject) -> Result<Vec<u8>, ApiError> {
    let value = object.to_value()?;
    serde_json::to_vec(&value).map_err(|e| ApiError::Store(e.to_string()))
}

/// Parse stored bytes back into a typed object, overlaying the authoritative
/// `resource_version` from the store's `mod_revision`.
///
/// # Errors
/// [`ApiError::Store`] / [`ApiError::Invalid`] if the bytes do not decode.
pub fn decode(versioned: &Versioned) -> Result<ResourceObject, ApiError> {
    let value: serde_json::Value =
        serde_json::from_slice(&versioned.value).map_err(|e| ApiError::Store(e.to_string()))?;
    let mut object = ResourceObject::from_value(value)?;
    object.metadata_mut().resource_version = versioned.mod_revision;
    Ok(object)
}
