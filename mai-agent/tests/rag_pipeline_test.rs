//! Integration test: RAG pipeline with semantic cache and vector store interface.
//!
//! Tests the full flow: embedding batch preparation -> packaging -> retrieval
//! -> augmented prompt construction -> semantic cache hit/miss.
//! No external services required (pure in-memory).

use std::collections::HashMap;
use std::time::Duration;

use mai_agent::{DocumentChunk, RagConfig, RagPipeline};
use mai_core::vault::SearchResult;

fn test_config() -> RagConfig {
    RagConfig {
        max_batch_size: 32,
        default_top_k: 5,
        similarity_threshold: 0.3,
        max_chunk_tokens: 512,
        semantic_cache_ttl: Duration::from_secs(300),
        semantic_cache_threshold: 0.92,
        semantic_cache_enabled: true,
        collection_prefix: "profile".to_string(),
    }
}

fn chunk(document_id: &str, chunk_index: u32, text: &str, source: &str) -> DocumentChunk {
    DocumentChunk {
        document_id: document_id.to_string(),
        chunk_index,
        text: text.to_string(),
        metadata: HashMap::from([("source".to_string(), source.to_string())]),
    }
}

fn search_result(document_id: &str, chunk_index: u32, text: &str, score: f32) -> SearchResult {
    SearchResult {
        id: format!("{document_id}-{chunk_index}"),
        score,
        payload: HashMap::from([
            (
                "document_id".to_string(),
                serde_json::Value::String(document_id.to_string()),
            ),
            (
                "chunk_index".to_string(),
                serde_json::Value::String(chunk_index.to_string()),
            ),
            (
                "text".to_string(),
                serde_json::Value::String(text.to_string()),
            ),
        ]),
    }
}

#[test]
fn full_rag_pipeline_flow() {
    let mut pipeline = RagPipeline::new(test_config());

    // Step 1: Prepare an embedding batch
    let chunks = vec![
        chunk(
            "architecture",
            0,
            "Island Mountain provides air-gapped AI inference servers.",
            "test-corpus/architecture.md",
        ),
        chunk(
            "pqc",
            0,
            "Post-quantum cryptography protects data sovereignty.",
            "test-corpus/pqc.md",
        ),
        chunk(
            "sentinel",
            0,
            "The Sentinel model runs in sleep mode with minimal VRAM.",
            "test-corpus/sentinel.md",
        ),
    ];
    let batch = pipeline
        .prepare_embedding_batch(&chunks)
        .expect("embedding batch should be valid");
    assert_eq!(batch.len(), 3);

    // Step 2: Package embeddings (simulated vectors)
    let fake_vectors: Vec<Vec<f32>> = (0..3)
        .map(|i| {
            let mut v = vec![0.0f32; 384];
            v[i] = 1.0;
            v
        })
        .collect();

    let points = pipeline
        .package_embeddings(&chunks, &fake_vectors)
        .expect("packaging should succeed");
    assert_eq!(points.len(), 3);
    assert_eq!(points[0].vector.len(), 384);

    // Step 3: Process retrieval results into augmented prompt
    let results = vec![
        search_result(
            "architecture",
            0,
            "Air-gapped servers ensure no data leaves the device.",
            0.95,
        ),
        search_result(
            "pqc",
            0,
            "ML-KEM-1024 provides post-quantum key encapsulation.",
            0.85,
        ),
        search_result("misc", 0, "Low relevance filler content.", 0.2),
    ];

    let query_vec = {
        let mut v = vec![0.0f32; 384];
        v[0] = 0.9;
        v[1] = 0.1;
        v
    };

    let response = pipeline.process_retrieval(
        "admin-profile-id",
        query_vec.clone(),
        results,
        "admin-profile-id",
    );

    // Should filter out the low-score chunk
    assert_eq!(response.retrieved.len(), 2);
    assert!(response.augmented_prompt.contains("Air-gapped servers"));
    assert!(response.augmented_prompt.contains("ML-KEM-1024"));
    assert!(!response.augmented_prompt.contains("Low relevance filler"));

    // Step 4: Semantic cache should have stored this result
    let cache_hit = pipeline.check_cache(&query_vec, "admin-profile-id");
    assert!(
        cache_hit.is_some(),
        "Should hit semantic cache on exact same vector"
    );

    // Step 5: Slightly different vector should miss cache
    let different_vec = {
        let mut v = vec![0.0f32; 384];
        v[200] = 1.0;
        v
    };
    let cache_miss = pipeline.check_cache(&different_vec, "admin-profile-id");
    assert!(cache_miss.is_none(), "Different vector should miss cache");

    // Step 6: Verify metrics
    let metrics = pipeline.metrics();
    assert_eq!(metrics.retrieval_requests, 1);
    assert_eq!(metrics.cache_hits, 1);
    assert_eq!(metrics.cache_misses, 1);
    assert_eq!(metrics.documents_retrieved, 2);
}

#[test]
fn collection_per_profile_isolation() {
    let pipeline = RagPipeline::new(test_config());

    let admin_col = pipeline.collection_for_profile("admin-id");
    let teen_col = pipeline.collection_for_profile("teen-id");

    assert_ne!(admin_col, teen_col);
    assert!(admin_col.contains("admin-id"));
    assert!(teen_col.contains("teen-id"));
}

#[test]
fn dimension_mismatch_rejected() {
    let mut pipeline = RagPipeline::new(test_config());

    let chunks = vec![chunk("doc", 0, "test", "test.md")];
    let correct_dim = vec![vec![1.0f32; 384]];
    pipeline
        .package_embeddings(&chunks, &correct_dim)
        .expect("first embedding batch establishes dimension");

    let wrong_dim = vec![vec![1.0f32; 128]];
    let result = pipeline.package_embeddings(&chunks, &wrong_dim);
    assert!(result.is_err(), "Should reject dimension mismatch");
}

#[test]
fn cache_profile_isolation() {
    let mut pipeline = RagPipeline::new(test_config());

    let vec_a = {
        let mut v = vec![0.0f32; 384];
        v[0] = 1.0;
        v
    };

    let results = vec![search_result("admin", 0, "Admin-only data", 0.9)];

    // Store for admin profile
    pipeline.process_retrieval("admin", vec_a.clone(), results, "admin");

    // Teen profile with same vector should NOT hit admin's cache
    let teen_hit = pipeline.check_cache(&vec_a, "teen");
    assert!(
        teen_hit.is_none(),
        "Teen must not see admin's cached results"
    );
}
