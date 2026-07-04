//! Policy decision bundle.
//!
//! [`PolicyBundle`] is the unified, serialisable input fed to the
//! [`composer`](super) when it asks each compliance module for a
//! decision. It carries three things:
//!
//! 1. [`RequestMetadata`] — provenance fields that identify *what* is
//!    being decided on (request id, tenant, timestamp, source).
//! 2. [`TrustContext`] — the verified claim projection produced earlier
//!    in the pipeline (see [`crate::trust`]).
//! 3. [`ClassificationResult`] — the sensitivity verdict from the
//!    router's classifier, reduced to a wire-format level
//!    plus the patterns that matched.
//!
//! The bundle is `Serialize + Deserialize` so it can be persisted for
//! replay / audit and reloaded from disk by tooling. No verification or
//! signing happens here — that lives in the audit subsystem

use std::fs;
use std::io;
use std::path::Path;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::trust::TrustContext;

/// Provenance fields describing the request being evaluated.
///
/// These are the fields the policy runtime needs that are *not* already
/// carried by [`TrustContext`] — origin, timing, and any caller-supplied
/// routing hints. Kept intentionally lean; richer telemetry lives in
/// the audit record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RequestMetadata {
    /// Globally unique id for this inference request. Used as the
    /// audit-log correlation key.
    pub request_id: String,
    /// Tenant the request was submitted under. Must match
    /// `TrustContext::tenant_id` when both are present; the composer
    /// rejects mismatches.
    pub tenant_id: String,
    /// Wall-clock submission time, milliseconds since the Unix epoch.
    pub timestamp_unix_ms: i64,
    /// Origin surface: `"api"`, `"sdk"`, `"hil"`, etc. Free-form so new
    /// surfaces can be added without a schema bump.
    pub source: String,
    /// Optional caller hint at the target model. The router may
    /// override this; the runtime records the hint for audit.
    #[serde(default)]
    pub model_hint: Option<String>,
}

/// Output of the router's sensitivity classifier, projected into a
/// serialisable shape so it can travel with the bundle without pulling
/// `mai-router` into the compliance crate's dependency graph.
///
/// The `level` field uses the same wire strings as
/// `mai_router::classifier::Classification::as_str` — `"public"`,
/// `"internal"`, `"sensitive"`, `"regulated"`, `"critical"`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClassificationResult {
    /// Highest sensitivity level the classifier matched.
    pub level: String,
    /// Pattern strings that matched, for explainability. May be empty.
    #[serde(default)]
    pub matched_patterns: Vec<String>,
    /// Number of named entities detected alongside the classification.
    /// Zero is valid (no entities found, or entity detection disabled).
    #[serde(default)]
    pub entity_count: u32,
}

/// Unified decision input for the policy runtime.
///
/// Each enabled compliance module (HIPAA, ITAR, OCAP, …) receives a
/// reference to this bundle and returns its own per-module decision.
/// The composer then folds those into an aggregate verdict.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PolicyBundle {
    /// Request provenance.
    pub request: RequestMetadata,
    /// Verified claim projection.
    pub trust: TrustContext,
    /// Router classifier output.
    pub classification: ClassificationResult,
}

/// Errors returned by [`PolicyBundle::load_from_file`].
#[derive(Debug, Error)]
pub enum PolicyBundleError {
    /// The file could not be opened or read.
    #[error("failed to read policy bundle from {path}: {source}")]
    Io {
        /// Path that was being read.
        path: String,
        /// Underlying I/O error.
        #[source]
        source: io::Error,
    },
    /// File contents were not valid JSON, or did not match the schema.
    #[error("failed to deserialize policy bundle from {path}: {source}")]
    Json {
        /// Path whose contents failed to parse.
        path: String,
        /// Underlying serde error.
        #[source]
        source: serde_json::Error,
    },
}

impl PolicyBundle {
    /// Read a JSON-encoded [`PolicyBundle`] from disk.
    ///
    /// No verification, signature checking, or schema-version
    /// negotiation happens here — those concerns belong to the audit
    /// and policy-version subsystems. This is purely a typed read.
    pub fn load_from_file<P: AsRef<Path>>(path: P) -> Result<Self, PolicyBundleError> {
        let path_ref = path.as_ref();
        let bytes = fs::read(path_ref).map_err(|source| PolicyBundleError::Io {
            path: path_ref.display().to_string(),
            source,
        })?;
        serde_json::from_slice(&bytes).map_err(|source| PolicyBundleError::Json {
            path: path_ref.display().to_string(),
            source,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn sample_bundle() -> PolicyBundle {
        PolicyBundle {
            request: RequestMetadata {
                request_id: "req-0001".to_string(),
                tenant_id: "local-dev".to_string(),
                timestamp_unix_ms: 1_700_000_000_000,
                source: "api".to_string(),
                model_hint: Some("llama-3-70b".to_string()),
            },
            trust: TrustContext::for_local_dev(),
            classification: ClassificationResult {
                level: "regulated".to_string(),
                matched_patterns: vec![r"\b\d{3}-\d{2}-\d{4}\b".to_string()],
                entity_count: 2,
            },
        }
    }

    #[test]
    fn roundtrip_serialize_deserialize() {
        let bundle = sample_bundle();
        let json = serde_json::to_string(&bundle).expect("serialize");
        let back: PolicyBundle = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(bundle, back);
    }

    #[test]
    fn load_from_file_reads_valid_bundle() {
        let bundle = sample_bundle();
        let dir = std::env::temp_dir();
        let path = dir.join("mai-policy-bundle-test-ok.json");
        let mut f = std::fs::File::create(&path).expect("create temp file");
        f.write_all(serde_json::to_vec_pretty(&bundle).unwrap().as_slice())
            .expect("write temp file");
        f.sync_all().ok();
        drop(f);

        let loaded = PolicyBundle::load_from_file(&path).expect("load");
        assert_eq!(loaded, bundle);

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn load_from_file_missing_path_returns_io_error() {
        let err = PolicyBundle::load_from_file("/definitely/not/a/real/path.json")
            .expect_err("missing file must error");
        assert!(matches!(err, PolicyBundleError::Io { .. }));
    }

    #[test]
    fn load_from_file_invalid_json_returns_json_error() {
        let dir = std::env::temp_dir();
        let path = dir.join("mai-policy-bundle-test-badjson.json");
        std::fs::write(&path, b"{ this is not json").expect("write");
        let err = PolicyBundle::load_from_file(&path).expect_err("bad json must error");
        assert!(matches!(err, PolicyBundleError::Json { .. }));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn optional_fields_default_when_missing() {
        let json = r#"{
            "request": {
                "request_id": "r1",
                "tenant_id": "t1",
                "timestamp_unix_ms": 1,
                "source": "api"
            },
            "trust": {
                "tenant_id": "local-dev",
                "subject_id": "s",
                "subject_hash": "h",
                "max_data_classification": "secret",
                "trust_bundle_version": "v0",
                "claim_id": "c",
                "revocation_status": "valid"
            },
            "classification": {
                "level": "public"
            }
        }"#;
        let bundle: PolicyBundle = serde_json::from_str(json).expect("defaults must fill in");
        assert!(bundle.request.model_hint.is_none());
        assert!(bundle.classification.matched_patterns.is_empty());
        assert_eq!(bundle.classification.entity_count, 0);
    }
}
