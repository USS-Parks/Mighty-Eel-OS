//! Resource versioning + conversion.
//!
//! The estate is served at a single **hub** api-version. A stored object at an
//! older version is upgraded to the hub transparently **on read** by a chain of
//! per-`(Kind, from_version)` converters — so a schema bump serves old objects
//! without a migration or estate downtime. Writes are unchanged:
//! admission still validates the estate's current stored schema.
//!
//! The default registry is the identity (hub = the estate `API_VERSION`, no
//! converters): every object is served exactly as stored.

use std::collections::HashMap;

use aog_estate::{API_VERSION, Kind};
use serde_json::Value;

/// A single-step upgrade of an object's JSON from one api-version to the next.
type Converter = Box<dyn Fn(Value) -> Value + Send + Sync>;

/// A registry of per-kind version converters plus the hub version to serve at.
pub struct ConversionRegistry {
    hub: String,
    converters: HashMap<(Kind, String), Converter>,
}

impl ConversionRegistry {
    /// The identity registry: hub = the estate `API_VERSION`, no converters.
    #[must_use]
    pub fn identity() -> Self {
        Self {
            hub: API_VERSION.to_owned(),
            converters: HashMap::new(),
        }
    }

    /// A registry serving at `hub`.
    #[must_use]
    pub fn new(hub: impl Into<String>) -> Self {
        Self {
            hub: hub.into(),
            converters: HashMap::new(),
        }
    }

    /// Register a single-step converter for `kind` from api-version `from` (one
    /// step toward the hub).
    #[must_use]
    pub fn with_converter(
        mut self,
        kind: Kind,
        from: impl Into<String>,
        convert: impl Fn(Value) -> Value + Send + Sync + 'static,
    ) -> Self {
        self.converters
            .insert((kind, from.into()), Box::new(convert));
        self
    }

    /// The hub api-version served.
    #[must_use]
    pub fn hub(&self) -> &str {
        &self.hub
    }

    /// Convert `value` up to the hub version, applying registered converters step
    /// by step. If no converter advances toward the hub, the value is served as
    /// stored — an unknown-but-valid older version is never silently dropped.
    #[must_use]
    pub fn convert(&self, kind: Kind, mut value: Value) -> Value {
        // Bounded to guard against a non-advancing converter cycle.
        for _ in 0..16 {
            let current = value
                .get("api_version")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned();
            if current == self.hub {
                break;
            }
            match self.converters.get(&(kind, current)) {
                Some(convert) => value = convert(value),
                None => break,
            }
        }
        value
    }
}

impl Default for ConversionRegistry {
    fn default() -> Self {
        Self::identity()
    }
}
