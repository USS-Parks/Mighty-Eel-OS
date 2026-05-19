//! Integration test: RAG pipeline with semantic cache and vector store interface.
//!
//! Tests the full flow: embedding batch preparation -> packaging -> retrieval
//! -> augmented prompt construction -> semantic cache hit/miss.
//! No external services required (pure in-memory).

use mai_agent::{
    DocumentChunk, RagConfig, RagPipeline, RetrievalResult,
};

fn test_config() -> RagConfig {
    RagConfig {
        max_batch_size: 32,
        default_top_k: 5,
        min_similarity_threshold: 0.3,
        semantic_cache_size: 100,
        semantic_cache_ttl_secs: 300,
        cache_similarity_threshold: 0.92,
        chunk_overlap_tokens: 50,
        max_context_chunks: 10,
    }
}

#[test]
fn full_rag_pipeline_flow() {
    let mut pipeline = RagPipeline::new(test_config(), 384);

    // Step 1: Prepare an embedding batch
    let texts = vec![
        "Island Mountain provides air-gapped AI inference servers.".to_string(),
        "Post-quantum cryptography protects data sovereignty.".to_string(),
        "The Sentinel model runs in sleep mode with minimal VRAM.".to_string(),
    ];
    let batch = pipeline.prepare_embedding_batch(texts.clone());
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
        .package_embeddings(&batch, &fake_vectors)
        .expect("packaging should succeed");
    assert_eq!(points.len(), 3);
    assert_eq!(points[0].vector.len(), 384);

    // Step 3: Process retrieval results into augmented prompt
    let results = vec![
        RetrievalResult {
            chunk: DocumentChunk {
                id: "chunk-1".to_string(),
                content: "Air-gapped servers ensure no data leaves the device.".to_string(),
                source: "docs/architecture.md".to_string(),
                chunk_index: 0,
                total_chunks: 1,
                metadata: Default::default(),
            },
            score: 0.95,
            collection: "profile-admin".to_string(),
        },
        RetrievalResult {
            chunk: DocumentChunk {
                id: "chunk-2".to_string(),
                content: "ML-KEM-1024 provides post-quantum key encapsulation.".to_string(),
                source: "docs/pqc.md".to_string(),
                chunk_index: 0,
                total_chunks: 1,
                metadata: Default::default(),
            },
            score: 0.85,
            collection: "profile-admin".to_string(),
        },
        RetrievalResult {
            chunk: DocumentChunk {
                id: "chunk-3".to_string(),
                content: "Low relevance filler content.".to_string(),
                source: "docs/misc.md".to_string(),
                chunk_index: 0,
                total_chunks: 1,
                metadata: Default::default(),
            },
            score: 0.2, // Below threshold
            collection: "profile-admin".to_string(),
        },
    ];

    let query_vec = {
        let mut v = vec![0.0f32; 384];
        v[0] = 0.9;
        v[1] = 0.1;
        v
    };

    let response = pipeline.process_retrieval(
        "admin-profile-id",
        &query_vec,
        &results,
    );

    // Should filter out the low-score chunk
    assert_eq!(response.chunks_used, 2);
    assert!(response.augmented_prompt.contains("Air-gapped servers"));
    assert!(response.augmented_prompt.contains("ML-KEM-1024"));
    assert!(!response.augmented_prompt.contains("Low relevance filler"));

    // Step 4: Semantic cache should have stored this result
    let cache_hit = pipeline.check_cache("admin-profile-id", &query_vec);
    assert!(cache_hit.is_some(), "Should hit semantic cache on exact same vector");

    // Step 5: Slightly different vector should miss cache
    let different_vec = {
        let mut v = vec![0.0f32; 384];
        v[200] = 1.0;
        v
    };
    let cache_miss = pipeline.check_cache("admin-profile-id", &different_vec);
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
    let pipeline = RagPipeline::new(test_config(), 384);

    let admin_col = pipeline.collection_for_profile("admin-id");
    let teen_col = pipeline.collection_for_profile("teen-id");

    assert_ne!(admin_col, teen_col);
    assert!(admin_col.contains("admin-id"));
    assert!(teen_col.contains("teen-id"));
}

#[test]
fn dimension_mismatch_rejected() {
    let pipeline = RagPipeline::new(test_config(), 384);

    let texts = vec!["test".to_string()];
    let batch = pipeline.prepare_embedding_batch(texts);
    let wrong_dim = vec![vec![1.0f32; 128]]; // 128 != 384

    let result = pipeline.package_embeddings(&batch, &wrong_dim);
    assert!(result.is_err(), "Should reject dimension mismatch");
}

#[test]
fn cache_profile_isolation() {
    let mut pipeline = RagPipeline::new(test_config(), 384);

    let vec_a = {
        let mut v = vec![0.0f32; 384];
        v[0] = 1.0;
        v
    };

    let results = vec![RetrievalResult {
        chunk: DocumentChunk {
            id: "c1".to_string(),
            content: "Admin-only data".to_string(),
            source: "admin.md".to_string(),
            chunk_index: 0,
            total_chunks: 1,
            metadata: Default::default(),
        },
        score: 0.9,
        collection: "admin".to_string(),
    }];

    // Store for admin profile
    pipeline.process_retrieval("admin", &vec_a, &results);

    // Teen profile with same vector should NOT hit admin's cache
    let teen_hit = pipeline.check_cache("teen", &vec_a);
    assert!(teen_hit.is_none(), "Teen must not see admin's cached results");
}
