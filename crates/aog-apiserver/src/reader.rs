//! The read side of the control plane. [`StoreReader`] holds the shared
//! `RaftNode` but exposes **only** reads — it has no method that mutates the
//! store. Every write goes through [`crate::admission::Admission`]. Reads and
//! writes are separate capabilities by construction (part of the K5 gate).

use std::sync::Arc;

use aog_estate::{Kind, ResourceObject};
use aog_store::raft::RaftNode;

use crate::codec::{decode, kind_prefix, store_key};
use crate::error::ApiError;

/// Read-only view over the committed estate.
#[derive(Clone)]
pub struct StoreReader {
    raft: Arc<RaftNode>,
}

impl StoreReader {
    #[must_use]
    pub fn new(raft: Arc<RaftNode>) -> Self {
        Self { raft }
    }

    /// Fetch one object by kind + name.
    ///
    /// # Errors
    /// [`ApiError::Store`] on backend failure; [`ApiError::Invalid`] if the
    /// stored bytes fail to decode.
    pub async fn get(&self, kind: Kind, name: &str) -> Result<Option<ResourceObject>, ApiError> {
        let key = store_key(kind, name);
        match self
            .raft
            .get(&key)
            .await
            .map_err(|e| ApiError::Store(e.to_string()))?
        {
            Some(versioned) => Ok(Some(decode(&versioned)?)),
            None => Ok(None),
        }
    }

    /// List every object of a kind (ascending by name).
    ///
    /// # Errors
    /// [`ApiError::Store`] on backend failure; [`ApiError::Invalid`] if any
    /// stored object fails to decode.
    pub async fn list(&self, kind: Kind) -> Result<Vec<ResourceObject>, ApiError> {
        let entries = self
            .raft
            .range(&kind_prefix(kind))
            .await
            .map_err(|e| ApiError::Store(e.to_string()))?;
        entries.iter().map(|(_, v)| decode(v)).collect()
    }
}
