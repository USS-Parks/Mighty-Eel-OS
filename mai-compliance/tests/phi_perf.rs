//! acceptance: PHI detection must complete in under 10ms p99
//! on a representative mixed corpus.

#![allow(clippy::print_stdout, clippy::print_stderr)]

use std::time::{Duration, Instant};

use mai_compliance::PhiDetector;

#[test]
fn phi_detection_p99_under_10ms() {
    let detector = PhiDetector::baseline();
    let corpus: &[&str] = &[
        "Tell me about the history of the Roman Empire.",
        "Patient Jane Doe DOB 03/14/1972 presented with chest pain.",
        "Contact alice@example.com or 415-555-1212 for follow-up.",
        "SSN 123-45-6789, MRN: 8472901, admitted to ICU.",
        "Lab results: Glucose 102 mg/dL, HbA1c 5.8%, BP 130/85 mmHg.",
        "Discharge summary mentions ICD-10 codes E11.9 and I10.",
        "DEA License #BS1234567 prescribed amoxicillin 500mg.",
        "URL https://patient-portal.example/charts and IP 10.0.0.42",
    ];

    let samples = 500;
    let mut durations: Vec<Duration> = Vec::with_capacity(samples);

    for i in 0..samples {
        let text = corpus[i % corpus.len()];
        let start = Instant::now();
        let _ = detector.scan(text);
        durations.push(start.elapsed());
    }

    durations.sort();
    let p99 = durations[(samples * 99 / 100).min(samples - 1)];
    let max = *durations.last().unwrap();
    println!(
        "phi.scan p50={:?} p99={:?} max={:?}",
        durations[samples / 2],
        p99,
        max,
    );

    assert!(
        p99 < Duration::from_millis(10),
        "PHI detection p99 {:?} exceeded 10ms budget (max {:?})",
        p99,
        max,
    );
}
