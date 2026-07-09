//! Qdrant vector database interface.
//!
//! Implements `VectorStore` for local RAG embedding storage and retrieval.
//! The Qdrant instance runs locally at 127.0.0.1:6334 and is never exposed
//! to the network (air-gap safe).
//!
//! # Collection Scoping
//!
//! Collections are scoped to family profiles. Each profile's RAG context
//! is isolated: a Child profile cannot search an Adult profile's embeddings.
//!
//! # Backend Status
//!
//! This implementation uses in-memory storage that simulates Qdrant behavior.
//! Real Qdrant operations require the `qdrant-client` crate and a running
//! Qdrant instance.

use std::collections::HashMap;

use async_trait::async_trait;
use tokio::sync::RwLock;
use tracing::{debug, info};

use mai_core::vault::{
    CollectionConfig, DistanceMetric, EmbeddingPoint, SearchResult, VaultError, VectorStore,
};

use crate::config::VectorConfig;

/// In-memory vector store simulating Qdrant.
pub struct VectorManager {
    config: VectorConfig,
    /// Collections: name -> (config, points)
    collections: RwLock<HashMap<String, CollectionData>>,
    /// Backup counter for generating backup IDs.
    backup_counter: RwLock<u64>,
    /// In-memory snapshots keyed by backup id — this backend's stand-in for the
    /// durable Qdrant->ZFS backup a real deployment performs. A backup actually
    /// preserves the collection state so a later restore reloads it.
    snapshots: RwLock<HashMap<String, HashMap<String, CollectionData>>>,
}

/// Data for a single collection.
#[derive(Clone)]
struct CollectionData {
    config: CollectionConfig,
    points: Vec<EmbeddingPoint>,
}

impl VectorManager {
    /// Create a new vector manager.
    pub fn new(config: VectorConfig) -> Self {
        Self {
            config,
            collections: RwLock::new(HashMap::new()),
            backup_counter: RwLock::new(0),
            snapshots: RwLock::new(HashMap::new()),
        }
    }

    /// Initialize: connect to Qdrant (or verify in-memory mode).
    pub fn initialize(&self) -> Result<(), VaultError> {
        info!(
            endpoint = %self.config.endpoint,
            "Initializing vector store (in-memory mode)"
        );
        // In production: connect to Qdrant gRPC endpoint, verify health.
        Ok(())
    }

    /// Compute similarity score between two vectors.
    fn compute_score(a: &[f32], b: &[f32], metric: DistanceMetric) -> f32 {
        match metric {
            DistanceMetric::Cosine => {
                let dot: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
                let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
                let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
                if norm_a == 0.0 || norm_b == 0.0 {
                    0.0
                } else {
                    dot / (norm_a * norm_b)
                }
            }
            DistanceMetric::Euclidean => {
                let sum_sq: f32 = a.iter().zip(b.iter()).map(|(x, y)| (x - y).powi(2)).sum();
                // Convert distance to similarity (closer = higher score)
                1.0 / (1.0 + sum_sq.sqrt())
            }
            DistanceMetric::DotProduct => a.iter().zip(b.iter()).map(|(x, y)| x * y).sum(),
        }
    }
}

#[async_trait]
impl VectorStore for VectorManager {
    async fn create_collection(&self, config: &CollectionConfig) -> Result<(), VaultError> {
        let mut collections = self.collections.write().await;
        if collections.contains_key(&config.name) {
            return Err(VaultError::CollectionAlreadyExists(config.name.clone()));
        }

        info!(
            collection = %config.name,
            dimension = config.dimension,
            distance = ?config.distance,
            profile = %config.profile_id,
            "Creating vector collection"
        );

        collections.insert(
            config.name.clone(),
            CollectionData {
                config: config.clone(),
                points: Vec::new(),
            },
        );
        Ok(())
    }

    async fn delete_collection(&self, collection_name: &str) -> Result<(), VaultError> {
        let mut collections = self.collections.write().await;
        if collections.remove(collection_name).is_none() {
            return Err(VaultError::CollectionNotFound(collection_name.to_string()));
        }
        info!(collection = %collection_name, "Vector collection deleted");
        Ok(())
    }

    async fn list_collections(
        &self,
        profile_filter: Option<&str>,
    ) -> Result<Vec<CollectionConfig>, VaultError> {
        let collections = self.collections.read().await;
        let result: Vec<CollectionConfig> = collections
            .values()
            .filter(|c| profile_filter.is_none_or(|pid| c.config.profile_id == pid))
            .map(|c| c.config.clone())
            .collect();
        Ok(result)
    }

    async fn store_embeddings(
        &self,
        collection_name: &str,
        points: &[EmbeddingPoint],
    ) -> Result<(), VaultError> {
        let mut collections = self.collections.write().await;
        let collection = collections
            .get_mut(collection_name)
            .ok_or_else(|| VaultError::CollectionNotFound(collection_name.to_string()))?;

        // Validate dimensions
        for point in points {
            if point.vector.len() != collection.config.dimension {
                return Err(VaultError::DimensionMismatch {
                    expected: collection.config.dimension,
                    actual: point.vector.len(),
                });
            }
        }

        debug!(
            collection = %collection_name,
            count = points.len(),
            "Storing embeddings"
        );

        // Upsert: replace existing points with same ID, append new ones
        for point in points {
            if let Some(existing) = collection.points.iter_mut().find(|p| p.id == point.id) {
                *existing = point.clone();
            } else {
                collection.points.push(point.clone());
            }
        }

        Ok(())
    }

    async fn search_similar(
        &self,
        collection_name: &str,
        query_vector: &[f32],
        top_k: usize,
        score_threshold: Option<f32>,
    ) -> Result<Vec<SearchResult>, VaultError> {
        let collections = self.collections.read().await;
        let collection = collections
            .get(collection_name)
            .ok_or_else(|| VaultError::CollectionNotFound(collection_name.to_string()))?;

        if query_vector.len() != collection.config.dimension {
            return Err(VaultError::DimensionMismatch {
                expected: collection.config.dimension,
                actual: query_vector.len(),
            });
        }

        let mut scored: Vec<SearchResult> = collection
            .points
            .iter()
            .map(|point| {
                let score =
                    Self::compute_score(query_vector, &point.vector, collection.config.distance);
                SearchResult {
                    id: point.id.clone(),
                    score,
                    payload: point.payload.clone(),
                }
            })
            .filter(|r| score_threshold.is_none_or(|t| r.score >= t))
            .collect();

        // Sort by score descending
        scored.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        scored.truncate(top_k);

        debug!(
            collection = %collection_name,
            results = scored.len(),
            "Similarity search complete"
        );
        Ok(scored)
    }

    async fn delete_points(
        &self,
        collection_name: &str,
        point_ids: &[String],
    ) -> Result<(), VaultError> {
        let mut collections = self.collections.write().await;
        let collection = collections
            .get_mut(collection_name)
            .ok_or_else(|| VaultError::CollectionNotFound(collection_name.to_string()))?;

        let before = collection.points.len();
        collection.points.retain(|p| !point_ids.contains(&p.id));
        let removed = before - collection.points.len();

        debug!(
            collection = %collection_name,
            removed,
            "Points deleted"
        );
        Ok(())
    }

    async fn point_count(&self, collection_name: &str) -> Result<u64, VaultError> {
        let collections = self.collections.read().await;
        let collection = collections
            .get(collection_name)
            .ok_or_else(|| VaultError::CollectionNotFound(collection_name.to_string()))?;
        Ok(collection.points.len() as u64)
    }

    async fn backup_to_vault(&self) -> Result<String, VaultError> {
        // Snapshot the current collection state so a later restore can reload it.
        // A production deployment additionally copies this to the ZFS vault; here
        // the snapshot lives in-process alongside the in-memory store.
        let snapshot = self.collections.read().await.clone();
        let mut counter = self.backup_counter.write().await;
        *counter += 1;
        let backup_id = format!("qdrant-backup-{}", *counter);
        let collections = snapshot.len();
        self.snapshots
            .write()
            .await
            .insert(backup_id.clone(), snapshot);
        info!(backup_id = %backup_id, collections, "Vector store snapshotted");
        Ok(backup_id)
    }

    async fn restore_from_vault(&self, backup_id: &str) -> Result<(), VaultError> {
        // Reload the collection state captured by the matching backup. An unknown
        // backup id is an error, not a silent success.
        let snapshot = self
            .snapshots
            .read()
            .await
            .get(backup_id)
            .cloned()
            .ok_or_else(|| VaultError::SnapshotNotFound(backup_id.to_string()))?;
        *self.collections.write().await = snapshot;
        info!(backup_id = %backup_id, "Vector store restored from snapshot");
        Ok(())
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn test_vector_config() -> VectorConfig {
        VectorConfig {
            endpoint: "http://127.0.0.1:6334".into(),
            connect_timeout_secs: 5,
            request_timeout_secs: 30,
        }
    }

    fn test_collection_config(name: &str, dim: usize) -> CollectionConfig {
        CollectionConfig {
            name: name.to_string(),
            dimension: dim,
            distance: DistanceMetric::Cosine,
            profile_id: "test-profile".to_string(),
        }
    }

    fn make_point(id: &str, vector: Vec<f32>) -> EmbeddingPoint {
        EmbeddingPoint {
            id: id.to_string(),
            vector,
            payload: HashMap::new(),
        }
    }

    #[tokio::test]
    async fn test_create_and_list_collections() {
        let mgr = VectorManager::new(test_vector_config());
        mgr.initialize().unwrap();

        mgr.create_collection(&test_collection_config("docs", 384))
            .await
            .unwrap();
        mgr.create_collection(&test_collection_config("chat", 768))
            .await
            .unwrap();

        let all = mgr.list_collections(None).await.unwrap();
        assert_eq!(all.len(), 2);
    }

    #[tokio::test]
    async fn test_duplicate_collection_fails() {
        let mgr = VectorManager::new(test_vector_config());
        mgr.create_collection(&test_collection_config("dup", 384))
            .await
            .unwrap();

        let result = mgr
            .create_collection(&test_collection_config("dup", 384))
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_store_and_search() {
        let mgr = VectorManager::new(test_vector_config());
        mgr.create_collection(&test_collection_config("test", 3))
            .await
            .unwrap();

        let points = vec![
            make_point("p1", vec![1.0, 0.0, 0.0]),
            make_point("p2", vec![0.0, 1.0, 0.0]),
            make_point("p3", vec![0.9, 0.1, 0.0]),
        ];
        mgr.store_embeddings("test", &points).await.unwrap();

        // Search for vector similar to p1
        let results = mgr
            .search_similar("test", &[1.0, 0.0, 0.0], 2, None)
            .await
            .unwrap();
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].id, "p1"); // exact match first
        assert_eq!(results[1].id, "p3"); // close second
    }

    #[tokio::test]
    async fn test_dimension_mismatch_rejected() {
        let mgr = VectorManager::new(test_vector_config());
        mgr.create_collection(&test_collection_config("dim-test", 3))
            .await
            .unwrap();

        // Wrong dimension
        let result = mgr
            .store_embeddings("dim-test", &[make_point("bad", vec![1.0, 0.0])])
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_delete_points() {
        let mgr = VectorManager::new(test_vector_config());
        mgr.create_collection(&test_collection_config("del", 2))
            .await
            .unwrap();

        let points = vec![
            make_point("a", vec![1.0, 0.0]),
            make_point("b", vec![0.0, 1.0]),
            make_point("c", vec![0.5, 0.5]),
        ];
        mgr.store_embeddings("del", &points).await.unwrap();
        assert_eq!(mgr.point_count("del").await.unwrap(), 3);

        mgr.delete_points("del", &["a".to_string(), "c".to_string()])
            .await
            .unwrap();
        assert_eq!(mgr.point_count("del").await.unwrap(), 1);
    }

    #[tokio::test]
    async fn test_score_threshold() {
        let mgr = VectorManager::new(test_vector_config());
        mgr.create_collection(&test_collection_config("thresh", 2))
            .await
            .unwrap();

        let points = vec![
            make_point("close", vec![1.0, 0.0]),
            make_point("far", vec![-1.0, 0.0]),
        ];
        mgr.store_embeddings("thresh", &points).await.unwrap();

        // High threshold should filter out the distant point
        let results = mgr
            .search_similar("thresh", &[1.0, 0.0], 10, Some(0.5))
            .await
            .unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].id, "close");
    }

    #[tokio::test]
    async fn test_backup_and_restore_round_trips_state() {
        let mgr = VectorManager::new(test_vector_config());
        mgr.create_collection(&test_collection_config("rag", 2))
            .await
            .unwrap();
        mgr.store_embeddings("rag", &[make_point("p", vec![1.0, 0.0])])
            .await
            .unwrap();

        let backup_id = mgr.backup_to_vault().await.unwrap();
        assert!(backup_id.starts_with("qdrant-backup-"));

        // Mutate after the backup, then restore: the mutation is undone.
        mgr.delete_collection("rag").await.unwrap();
        assert!(mgr.point_count("rag").await.is_err());
        mgr.restore_from_vault(&backup_id).await.unwrap();
        assert_eq!(mgr.point_count("rag").await.unwrap(), 1);

        // An unknown backup id is an error, not a silent success.
        assert!(mgr.restore_from_vault("nope").await.is_err());
    }

    #[tokio::test]
    async fn test_delete_collection() {
        let mgr = VectorManager::new(test_vector_config());
        mgr.create_collection(&test_collection_config("temp", 2))
            .await
            .unwrap();
        mgr.delete_collection("temp").await.unwrap();

        let result = mgr.point_count("temp").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_upsert_existing_points() {
        let mgr = VectorManager::new(test_vector_config());
        mgr.create_collection(&test_collection_config("upsert", 2))
            .await
            .unwrap();

        mgr.store_embeddings("upsert", &[make_point("x", vec![1.0, 0.0])])
            .await
            .unwrap();
        assert_eq!(mgr.point_count("upsert").await.unwrap(), 1);

        // Update existing point
        mgr.store_embeddings("upsert", &[make_point("x", vec![0.0, 1.0])])
            .await
            .unwrap();
        assert_eq!(mgr.point_count("upsert").await.unwrap(), 1); // still 1, not 2
    }
}
