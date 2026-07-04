//! The read side of the control plane. [`StoreReader`] holds the shared
//! `RaftNode` but exposes **only** reads — it has no method that mutates the
//! store. Every write goes through [`crate::admission::Admission`] (part of the
//! K5 gate). Reads serve objects at the estate's **hub version**: a stored object
//! at an older schema version is converted transparently on read (K10), so old
//! objects keep being served across a kind bump.

use std::sync::Arc;

use aog_estate::Kind;
use aog_store::raft::RaftNode;
use serde_json::Value;

use crate::codec::{decode_value, kind_prefix, store_key};
use crate::convert::ConversionRegistry;
use crate::error::ApiError;

/// Read-only view over the committed estate.
#[derive(Clone)]
pub struct StoreReader {
    raft: Arc<RaftNode>,
    conversions: Arc<ConversionRegistry>,
}

impl StoreReader {
    #[must_use]
    pub fn new(raft: Arc<RaftNode>) -> Self {
        Self {
            raft,
            conversions: Arc::new(ConversionRegistry::identity()),
        }
    }

    /// Replace the read-path conversion registry (K10). Default is the identity.
    #[must_use]
    pub fn with_conversions(mut self, conversions: ConversionRegistry) -> Self {
        self.conversions = Arc::new(conversions);
        self
    }

    /// Fetch one object by kind + name, served at the hub version.
    ///
    /// # Errors
    /// [`ApiError::Store`] on backend failure; [`ApiError::Invalid`] if the stored
    /// bytes are not valid JSON.
    pub async fn get(&self, kind: Kind, name: &str) -> Result<Option<Value>, ApiError> {
        let key = store_key(kind, name);
        match self
            .raft
            .get(&key)
            .await
            .map_err(|e| ApiError::Store(e.to_string()))?
        {
            Some(versioned) => Ok(Some(
                self.conversions.convert(kind, decode_value(&versioned)?),
            )),
            None => Ok(None),
        }
    }

    /// List every object of a kind (ascending by name), each at the hub version.
    ///
    /// # Errors
    /// [`ApiError::Store`] on backend failure; [`ApiError::Invalid`] if any stored
    /// object is not valid JSON.
    pub async fn list(&self, kind: Kind) -> Result<Vec<Value>, ApiError> {
        let entries = self
            .raft
            .range(&kind_prefix(kind))
            .await
            .map_err(|e| ApiError::Store(e.to_string()))?;
        entries
            .iter()
            .map(|(_, v)| decode_value(v).map(|value| self.conversions.convert(kind, value)))
            .collect()
    }
}
