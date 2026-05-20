//! RAG Pipeline Interface for the MAI.
//!
//! Defines the retrieval-augmented generation protocol:
//! 1. L4 sends text to embed -> MAI returns vectors via scheduler
//! 2. L4 queries vector store with vectors -> gets relevant documents
//! 3. L4 sends augmented prompt (original + retrieved) -> MAI generates
//!
//! Also implements semantic caching: if a semantically similar query was
//! recently asked for the same profile, return the cached result without
//! re-running the full RAG pipeline.
//!
//! # Architecture
//!
//! The RagPipeline does NOT own the vector store or the embedding model.
//! It orchestrates the protocol between L4 and the MAI's existing
//! components (scheduler for embedding, vault's VectorStore for search).
//!
//! # Air-Gap Safety
//!
//! All embeddings and documents stay local. Qdrant runs at 127.0.0.1.
//! No external retrieval sources.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use tracing::{debug, info};
use mai_core::vault::{CollectionConfig, DistanceMetric, EmbeddingPoint, SearchResult};

use crate::types::{
    AgentError, DocumentChunk, RagConfig, RagResponse,
    RetrievalResult,
};

// ============================================================================
// Semantic Cache
// ============================================================================

/// A cached RAG result keyed by embedding similarity.
#[derive(Debug, Clone)]
struct SemanticCacheEntry {
    /// Query embedding that produced this result
    query_embedding: Vec<f32>,
    /// Cached response
    response: RagResponse,
    /// When this entry was created
    created_at: Instant,
    /// Profile that owns this entry
    profile_id: String,
    /// Number of cache hits
    hit_count: u64,
}

/// Semantic cache for RAG results.
///
/// Unlike the exact-match ResponseCache in mai-core, this cache uses
/// embedding similarity to detect semantically equivalent queries.
/// A query is a cache hit if its embedding is within the configured
/// similarity threshold of a cached query's embedding.
struct SemanticCache {
    /// Cached entries
    entries: Vec<SemanticCacheEntry>,
    /// Maximum entries
    max_entries: usize,
    /// TTL for cache entries
    ttl: Duration,
    /// Similarity threshold for cache hit (e.g., 0.95 = 95% similar)
    similarity_threshold: f32,
}

impl SemanticCache {
    fn new(ttl: Duration, similarity_threshold: f32, max_entries: usize) -> Self {
        Self {
            entries: Vec::new(),
            max_entries,
            ttl,
            similarity_threshold,
        }
    }

    /// Look up a query embedding in the cache.
    /// Returns the cached response if a sufficiently similar query exists.
    fn lookup(
        &mut self,
        query_embedding: &[f32],
        profile_id: &str,
    ) -> Option<RagResponse> {
        // Evict expired entries first
        let now = Instant::now();
        self.entries.retain(|e| now.duration_since(e.created_at) < self.ttl);

        let mut best_score = 0.0f32;
        let mut best_idx = None;

        for (i, entry) in self.entries.iter().enumerate() {
            // Profile isolation: only match same profile
            if entry.profile_id != profile_id {
                continue;
            }
            let score = cosine_similarity(query_embedding, &entry.query_embedding);
            if score > best_score {
                best_score = score;
                best_idx = Some(i);
            }
        }

        if best_score >= self.similarity_threshold {
            if let Some(idx) = best_idx {
                self.entries[idx].hit_count += 1;
                let mut response = self.entries[idx].response.clone();
                response.cache_hit = true;
                debug!(
                    score = best_score,
                    profile_id,
                    "Semantic cache hit"
                );
                return Some(response);
            }
        }

        None
    }

    /// Store a query/response pair in the cache.
    fn store(
        &mut self,
        query_embedding: Vec<f32>,
        response: RagResponse,
        profile_id: &str,
    ) {
        if self.entries.len() >= self.max_entries {
            // Evict least recently used (lowest hit count, oldest)
            if let Some(evict_idx) = self
                .entries
                .iter()
                .enumerate()
                .min_by_key(|(_, e)| (e.hit_count, std::cmp::Reverse(e.created_at)))
                .map(|(i, _)| i)
            {
                self.entries.remove(evict_idx);
            }
        }

        self.entries.push(SemanticCacheEntry {
            query_embedding,
            response,
            created_at: Instant::now(),
            profile_id: profile_id.to_string(),
            hit_count: 0,
        });
    }

    /// Invalidate all entries for a profile.
    fn invalidate_profile(&mut self, profile_id: &str) {
        self.entries.retain(|e| e.profile_id != profile_id);
    }

    /// Clear the entire cache.
    fn clear(&mut self) {
        self.entries.clear();
    }

    /// Current entry count.
    fn len(&self) -> usize {
        self.entries.len()
    }
}

/// Cosine similarity between two vectors.
/// Returns 0.0 if either vector is zero-length or dimensions mismatch.
fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }

    let mut dot = 0.0f32;
    let mut norm_a = 0.0f32;
    let mut norm_b = 0.0f32;

    for (x, y) in a.iter().zip(b.iter()) {
        dot += x * y;
        norm_a += x * x;
        norm_b += y * y;
    }

    let denom = norm_a.sqrt() * norm_b.sqrt();
    if denom < f32::EPSILON {
        0.0
    } else {
        dot / denom
    }
}

// ============================================================================
// RAG Pipeline
// ============================================================================

/// RAG pipeline interface for the MAI.
///
/// Thread safety: NOT internally synchronized. Wrap in Arc<RwLock<_>>.
pub struct RagPipeline {
    /// Pipeline configuration
    config: RagConfig,
    /// Semantic cache (profile-isolated)
    cache: SemanticCache,
    /// Embedding dimension (set on first embedding response)
    embedding_dim: Option<usize>,
    /// Metrics
    metrics: RagMetrics,
}

/// RAG pipeline performance metrics.
#[derive(Debug, Clone, Default)]
pub struct RagMetrics {
    /// Total embedding requests
    pub embed_requests: u64,
    /// Total retrieval requests
    pub retrieval_requests: u64,
    /// Total augmented generation requests
    pub augmented_requests: u64,
    /// Semantic cache hits
    pub cache_hits: u64,
    /// Semantic cache misses
    pub cache_misses: u64,
    /// Total chunks embedded
    pub chunks_embedded: u64,
    /// Total documents retrieved
    pub documents_retrieved: u64,
}

impl RagPipeline {
    /// Create a new RAG pipeline with the given configuration.
    pub fn new(config: RagConfig) -> Self {
        let cache = SemanticCache::new(
            config.semantic_cache_ttl,
            config.semantic_cache_threshold,
            1000, // Max cache entries
        );
        Self {
            config,
            cache,
            embedding_dim: None,
            metrics: RagMetrics::default(),
        }
    }

    /// Get the collection name for a profile (profile isolation).
    pub fn collection_for_profile(&self, profile_id: &str) -> String {
        format!("{}_{}", self.config.collection_prefix, profile_id)
    }

    /// Prepare a batch of document chunks for embedding.
    ///
    /// Validates chunk count against max_batch_size and returns the
    /// texts ready for the embedding endpoint. The actual embedding
    /// call goes through the scheduler (RequestType::Embedding).
    pub fn prepare_embedding_batch(
        &self,
        chunks: &[DocumentChunk],
    ) -> Result<Vec<String>, AgentError> {
        if chunks.is_empty() {
            return Ok(Vec::new());
        }
        if chunks.len() > self.config.max_batch_size {
            return Err(AgentError::Internal(format!(
                "Batch size {} exceeds max {}",
                chunks.len(),
                self.config.max_batch_size
            )));
        }

        Ok(chunks.iter().map(|c| c.text.clone()).collect())
    }

    /// Store embedding results in the vector store.
    ///
    /// Takes the raw embedding vectors returned by the scheduler and
    /// packages them as EmbeddingPoints for the VectorStore trait.
    /// Returns the number of points stored.
    pub fn package_embeddings(
        &mut self,
        chunks: &[DocumentChunk],
        embeddings: &[Vec<f32>],
    ) -> Result<Vec<EmbeddingPoint>, AgentError> {
        if chunks.len() != embeddings.len() {
            return Err(AgentError::Internal(format!(
                "Chunk count {} != embedding count {}",
                chunks.len(),
                embeddings.len()
            )));
        }

        // Record embedding dimension on first call
        if let Some(first) = embeddings.first() {
            if let Some(expected) = self.embedding_dim {
                if first.len() != expected {
                    return Err(AgentError::DimensionMismatch {
                        expected,
                        actual: first.len(),
                    });
                }
            } else {
                self.embedding_dim = Some(first.len());
                info!(dim = first.len(), "Set embedding dimension");
            }
        }

        let mut points = Vec::with_capacity(chunks.len());
        for (chunk, embedding) in chunks.iter().zip(embeddings.iter()) {
            let mut payload: HashMap<String, serde_json::Value> = chunk
                .metadata
                .iter()
                .map(|(k, v)| (k.clone(), serde_json::Value::String(v.clone())))
                .collect();
            payload.insert("document_id".into(), serde_json::Value::String(chunk.document_id.clone()));
            payload.insert("chunk_index".into(), serde_json::Value::String(chunk.chunk_index.to_string()));

            // Use hash of document_id + chunk_index as point ID
            let point_id = compute_point_id(&chunk.document_id, chunk.chunk_index);

            points.push(EmbeddingPoint {
                id: point_id.to_string(),
                vector: embedding.clone(),
                payload,
            });
        }

        self.metrics.chunks_embedded += chunks.len() as u64;
        self.metrics.embed_requests += 1;

        Ok(points)
    }

    /// Build a retrieval query: check semantic cache first, then
    /// prepare the query for vector search.
    ///
    /// Returns Some(RagResponse) on cache hit, None on miss.
    pub fn check_cache(
        &mut self,
        query_embedding: &[f32],
        profile_id: &str,
    ) -> Option<RagResponse> {
        if !self.config.semantic_cache_enabled {
            return None;
        }

        match self.cache.lookup(query_embedding, profile_id) {
            Some(response) => {
                self.metrics.cache_hits += 1;
                Some(response)
            }
            None => {
                self.metrics.cache_misses += 1;
                None
            }
        }
    }

    /// Process retrieval results from the vector store into a RagResponse.
    ///
    /// Filters by similarity threshold, formats the augmented prompt,
    /// and optionally stores in semantic cache.
    pub fn process_retrieval(
        &mut self,
        query: &str,
        query_embedding: Vec<f32>,
        search_results: Vec<SearchResult>,
        profile_id: &str,
    ) -> RagResponse {
        self.metrics.retrieval_requests += 1;

        // Filter by similarity threshold
        let threshold = self.config.similarity_threshold;
        let filtered: Vec<RetrievalResult> = search_results
            .into_iter()
            .filter(|r| r.score >= threshold)
            .map(|r| {
                // Convert serde_json::Value payload back to String metadata
                let string_meta: HashMap<String, String> = r
                    .payload
                    .iter()
                    .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                    .collect();
                RetrievalResult {
                    chunk: DocumentChunk {
                        document_id: r
                            .payload
                            .get("document_id")
                            .and_then(|v| v.as_str())
                            .unwrap_or_default()
                            .to_string(),
                        chunk_index: r
                            .payload
                            .get("chunk_index")
                            .and_then(|v| v.as_str())
                            .and_then(|s| s.parse().ok())
                            .unwrap_or(0),
                        text: r
                            .payload
                            .get("text")
                            .and_then(|v| v.as_str())
                            .unwrap_or_default()
                            .to_string(),
                        metadata: string_meta,
                    },
                    score: r.score,
                    embedding: None,
                }
            })
            .collect();

        self.metrics.documents_retrieved += filtered.len() as u64;

        // Build augmented prompt
        let context_parts: Vec<String> = filtered
            .iter()
            .enumerate()
            .map(|(i, r)| {
                format!(
                    "[Retrieved Document {}] (score: {:.3})\n{}",
                    i + 1,
                    r.score,
                    r.chunk.text
                )
            })
            .collect();

        let augmented_prompt = if context_parts.is_empty() {
            query.to_string()
        } else {
            format!(
                "Context from relevant documents:\n\n{}\n\nUser query: {}",
                context_parts.join("\n\n"),
                query
            )
        };

        let token_count = crate::context::estimate_tokens(&augmented_prompt);

        let response = RagResponse {
            retrieved: filtered,
            augmented_prompt,
            cache_hit: false,
            augmented_token_count: token_count,
        };

        // Store in semantic cache for future queries
        if self.config.semantic_cache_enabled && !query_embedding.is_empty() {
            self.cache
                .store(query_embedding, response.clone(), profile_id);
        }

        self.metrics.augmented_requests += 1;
        response
    }

    /// Get the collection config for creating a new profile collection.
    pub fn collection_config(&self, profile_id: &str) -> Option<CollectionConfig> {
        self.embedding_dim.map(|dim| CollectionConfig {
            name: self.collection_for_profile(profile_id),
            dimension: dim,
            distance: DistanceMetric::Cosine,
            profile_id: profile_id.to_string(),
        })
    }

    /// Invalidate semantic cache for a profile.
    pub fn invalidate_profile_cache(&mut self, profile_id: &str) {
        self.cache.invalidate_profile(profile_id);
    }

    /// Clear the entire semantic cache.
    pub fn clear_cache(&mut self) {
        self.cache.clear();
    }

    /// Get pipeline metrics.
    pub fn metrics(&self) -> &RagMetrics {
        &self.metrics
    }

    /// Get current semantic cache size.
    pub fn cache_size(&self) -> usize {
        self.cache.len()
    }

    /// Get the detected embedding dimension (None if no embeddings processed yet).
    pub fn embedding_dimension(&self) -> Option<usize> {
        self.embedding_dim
    }

    /// Get configuration (read-only).
    pub fn config(&self) -> &RagConfig {
        &self.config
    }
}

/// Compute a deterministic point ID from document_id and chunk_index.
fn compute_point_id(document_id: &str, chunk_index: u32) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    document_id.hash(&mut hasher);
    chunk_index.hash(&mut hasher);
    hasher.finish()
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config() -> RagConfig {
        RagConfig {
            max_batch_size: 10,
            default_top_k: 3,
            similarity_threshold: 0.5,
            max_chunk_tokens: 256,
            semantic_cache_ttl: Duration::from_secs(60),
            semantic_cache_threshold: 0.95,
            semantic_cache_enabled: true,
            collection_prefix: "test_rag".to_string(),
        }
    }

    fn sample_chunks() -> Vec<DocumentChunk> {
        vec![
            DocumentChunk {
                document_id: "doc-1".into(),
                chunk_index: 0,
                text: "The sky is blue.".into(),
                metadata: HashMap::new(),
            },
            DocumentChunk {
                document_id: "doc-1".into(),
                chunk_index: 1,
                text: "Water is wet.".into(),
                metadata: HashMap::new(),
            },
        ]
    }

    fn sample_embeddings() -> Vec<Vec<f32>> {
        vec![vec![0.1, 0.2, 0.3, 0.4], vec![0.5, 0.6, 0.7, 0.8]]
    }

    #[test]
    fn test_cosine_similarity_identical() {
        let a = vec![1.0, 0.0, 0.0];
        assert!((cosine_similarity(&a, &a) - 1.0).abs() < 0.001);
    }

    #[test]
    fn test_cosine_similarity_orthogonal() {
        let a = vec![1.0, 0.0];
        let b = vec![0.0, 1.0];
        assert!(cosine_similarity(&a, &b).abs() < 0.001);
    }

    #[test]
    fn test_cosine_similarity_dimension_mismatch() {
        let a = vec![1.0, 0.0];
        let b = vec![1.0, 0.0, 0.0];
        assert_eq!(cosine_similarity(&a, &b), 0.0);
    }

    #[test]
    fn test_prepare_embedding_batch() {
        let pipeline = RagPipeline::new(test_config());
        let chunks = sample_chunks();
        let texts = pipeline.prepare_embedding_batch(&chunks).unwrap();
        assert_eq!(texts.len(), 2);
        assert_eq!(texts[0], "The sky is blue.");
    }

    #[test]
    fn test_prepare_embedding_batch_exceeds_max() {
        let config = RagConfig {
            max_batch_size: 1,
            ..test_config()
        };
        let pipeline = RagPipeline::new(config);
        let chunks = sample_chunks(); // 2 chunks > max 1
        assert!(pipeline.prepare_embedding_batch(&chunks).is_err());
    }

    #[test]
    fn test_package_embeddings() {
        let mut pipeline = RagPipeline::new(test_config());
        let chunks = sample_chunks();
        let embeddings = sample_embeddings();

        let points = pipeline.package_embeddings(&chunks, &embeddings).unwrap();
        assert_eq!(points.len(), 2);
        assert_eq!(points[0].vector.len(), 4);
        assert_eq!(pipeline.embedding_dimension(), Some(4));
    }

    #[test]
    fn test_package_embeddings_dimension_mismatch() {
        let mut pipeline = RagPipeline::new(test_config());
        let chunks = sample_chunks();
        let embeddings = sample_embeddings();

        // First call sets dimension to 4
        pipeline.package_embeddings(&chunks, &embeddings).unwrap();

        // Second call with different dimension
        let bad_embeddings = vec![vec![0.1, 0.2], vec![0.3, 0.4]];
        let result = pipeline.package_embeddings(&chunks, &bad_embeddings);
        assert!(matches!(result, Err(AgentError::DimensionMismatch { .. })));
    }

    #[test]
    fn test_collection_for_profile() {
        let pipeline = RagPipeline::new(test_config());
        let name = pipeline.collection_for_profile("dad-profile");
        assert_eq!(name, "test_rag_dad-profile");
    }

    #[test]
    fn test_process_retrieval() {
        let mut pipeline = RagPipeline::new(test_config());

        let mut meta = HashMap::new();
        meta.insert("document_id".into(), serde_json::Value::String("doc-1".into()));
        meta.insert("chunk_index".into(), serde_json::Value::String("0".into()));
        meta.insert("text".into(), serde_json::Value::String("The answer is 42.".into()));

        let results = vec![SearchResult {
            id: "1".to_string(),
            score: 0.85,
            payload: meta,
        }];

        let response = pipeline.process_retrieval(
            "What is the answer?",
            vec![0.1, 0.2, 0.3],
            results,
            "profile-1",
        );

        assert!(!response.cache_hit);
        assert_eq!(response.retrieved.len(), 1);
        assert!(response.augmented_prompt.contains("The answer is 42."));
        assert!(response.augmented_prompt.contains("What is the answer?"));
    }

    #[test]
    fn test_process_retrieval_filters_low_score() {
        let mut pipeline = RagPipeline::new(test_config());

        let results = vec![SearchResult {
            id: "1".to_string(),
            score: 0.3, // Below threshold of 0.5
            payload: HashMap::new(),
        }];

        let response = pipeline.process_retrieval(
            "query",
            vec![0.1, 0.2],
            results,
            "profile-1",
        );

        assert_eq!(response.retrieved.len(), 0);
    }

    #[test]
    fn test_semantic_cache_hit() {
        let mut pipeline = RagPipeline::new(test_config());

        let query_embedding = vec![0.1, 0.2, 0.3];
        let profile = "profile-1";

        // First call: miss
        assert!(pipeline.check_cache(&query_embedding, profile).is_none());

        // Store a result
        let response = RagResponse {
            retrieved: Vec::new(),
            augmented_prompt: "cached result".into(),
            cache_hit: false,
            augmented_token_count: 5,
        };
        pipeline.cache.store(query_embedding.clone(), response, profile);

        // Same query: hit (identical embedding = similarity 1.0 > 0.95)
        let cached = pipeline.check_cache(&query_embedding, profile);
        assert!(cached.is_some());
        assert!(cached.unwrap().cache_hit);
    }

    #[test]
    fn test_semantic_cache_profile_isolation() {
        let mut pipeline = RagPipeline::new(test_config());

        let embedding = vec![0.1, 0.2, 0.3];
        let response = RagResponse {
            retrieved: Vec::new(),
            augmented_prompt: "cached".into(),
            cache_hit: false,
            augmented_token_count: 2,
        };

        // Store for profile-1
        pipeline.cache.store(embedding.clone(), response, "profile-1");

        // profile-2 should NOT get the hit
        assert!(pipeline.check_cache(&embedding, "profile-2").is_none());
        // profile-1 should get the hit
        assert!(pipeline.check_cache(&embedding, "profile-1").is_some());
    }

    #[test]
    fn test_semantic_cache_invalidation() {
        let mut pipeline = RagPipeline::new(test_config());

        let embedding = vec![0.1, 0.2];
        let response = RagResponse {
            retrieved: Vec::new(),
            augmented_prompt: "cached".into(),
            cache_hit: false,
            augmented_token_count: 2,
        };

        pipeline.cache.store(embedding.clone(), response, "profile-1");
        assert_eq!(pipeline.cache_size(), 1);

        pipeline.invalidate_profile_cache("profile-1");
        assert_eq!(pipeline.cache_size(), 0);
    }

    #[test]
    fn test_metrics_tracking() {
        let mut pipeline = RagPipeline::new(test_config());
        let chunks = sample_chunks();
        let embeddings = sample_embeddings();

        pipeline.package_embeddings(&chunks, &embeddings).unwrap();

        let metrics = pipeline.metrics();
        assert_eq!(metrics.embed_requests, 1);
        assert_eq!(metrics.chunks_embedded, 2);
    }

    #[test]
    fn test_compute_point_id_deterministic() {
        let id1 = compute_point_id("doc-1", 0);
        let id2 = compute_point_id("doc-1", 0);
        assert_eq!(id1, id2);

        let id3 = compute_point_id("doc-1", 1);
        assert_ne!(id1, id3);
    }
}
