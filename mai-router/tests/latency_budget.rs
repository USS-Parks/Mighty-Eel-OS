//! acceptance: router decision must complete in under 5ms on the
//! 99th percentile. We sample 1_000 mixed-classification queries and assert
//! the p99 stays under budget.
//!
//! This test is intentionally hardware-loose — `Instant::now` plus a small
//! sample size protects against flake on CI shared runners while still
//! catching a real regression.

#![allow(clippy::print_stdout, clippy::print_stderr)]

use std::time::{Duration, Instant};

use mai_router::{DefaultRouter, RouteRequest, Router};

fn req(query: &str) -> RouteRequest {
    RouteRequest {
        query: query.to_string(),
        estimated_tokens: 200,
        profile_id: "bench-user".to_string(),
        role: "adult".to_string(),
        upstream_flags: vec![],
    }
}

#[test]
fn router_p99_decision_under_5ms() {
    let router = DefaultRouter::with_defaults();
    let corpus: &[&str] = &[
        "What is the capital of France?",
        "Please summarize the meeting notes",
        "Contact alice@example.com about the deal",
        "The patient was given a prescription for amoxicillin",
        "Per the treaty, sacred site access is restricted",
        "ITAR controlled drawings of widget assembly",
        "The SSN is 123-45-6789",
        "Brief on Q3 internal use only",
    ];

    let samples = 1_000;
    let mut durations: Vec<Duration> = Vec::with_capacity(samples);

    for i in 0..samples {
        let query = corpus[i % corpus.len()];
        let request = req(query);
        let start = Instant::now();
        let _ = router.route(&request);
        durations.push(start.elapsed());
    }

    durations.sort();
    let p99_index = ((samples as f64) * 0.99) as usize;
    let p99 = durations[p99_index.min(samples - 1)];
    let max = *durations.last().unwrap();

    println!(
        "router p50={:?} p99={:?} max={:?}",
        durations[samples / 2],
        p99,
        max
    );

    assert!(
        p99 < Duration::from_millis(5),
        "router p99 {:?} exceeded 5ms budget (max {:?})",
        p99,
        max,
    );
}
