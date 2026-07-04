//! Production trace capture for the scheduler.
//!
//! Captures per-request telemetry at completion time and appends NDJSON to a
//! locally-rotated trace file. Privacy is structural: this module never sees
//! prompts, responses, or user identifiers. Session IDs are hashed at capture
//! time with a configurable salt so that even the raw trace cannot be reversed
//! back to the underlying conversation.
//!
//! The capture is opt-in via configuration and is a no-op when disabled. When
//! enabled, writes are synchronous and buffered; daily rotation keys files by
//! UTC date.

use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::types::{Priority, ScheduleRequest, SequenceId};

/// Configuration for trace capture.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TraceConfig {
    /// Whether trace capture is active.
    #[serde(default)]
    pub enabled: bool,
    /// Directory that holds rotated NDJSON trace files.
    #[serde(default = "default_output_dir")]
    pub output_dir: PathBuf,
    /// File name prefix; final name is `{prefix}-{YYYY-MM-DD}.ndjson`.
    #[serde(default = "default_prefix")]
    pub file_prefix: String,
    /// Salt mixed into session-id hashes. Operators should rotate this on a
    /// schedule (e.g., daily) to limit cross-trace correlation.
    #[serde(default)]
    pub session_id_salt: String,
}

fn default_output_dir() -> PathBuf {
    PathBuf::from("/var/lib/mai/traces")
}

fn default_prefix() -> String {
    "scheduler-trace".to_string()
}

impl Default for TraceConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            output_dir: default_output_dir(),
            file_prefix: default_prefix(),
            session_id_salt: String::new(),
        }
    }
}

/// One captured request, serialized to NDJSON.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TraceEvent {
    /// Capture time, RFC 3339 UTC.
    pub timestamp: String,
    /// Per-event UUID. Stable across re-anonymization passes.
    pub request_id: String,
    /// Hashed session id (blake3 truncated to 32 hex chars).
    pub session_id_hash: String,
    /// Model alias the request targeted (product information, not user info).
    pub model_alias: String,
    /// Prompt tokens observed.
    pub input_tokens: u32,
    /// Tokens generated.
    pub output_tokens: u32,
    /// End-to-end latency in milliseconds.
    pub latency_ms: u64,
    /// Time the request spent queued before dispatch.
    pub queue_wait_ms: u64,
    /// Request priority as a wire string.
    pub priority: String,
    /// Whether this was a continuation of an earlier sequence.
    pub was_continuation: bool,
}

/// Inputs required to capture one request completion.
#[derive(Debug, Clone)]
pub struct CaptureContext {
    pub session_id: SequenceId,
    pub model_alias: String,
    pub input_tokens: u32,
    pub output_tokens: u32,
    pub latency_ms: u64,
    pub queue_wait_ms: u64,
    pub priority: Priority,
    pub was_continuation: bool,
}

impl CaptureContext {
    /// Build a capture context from a [`ScheduleRequest`] and completion timings.
    pub fn from_request(
        request: &ScheduleRequest,
        output_tokens: u32,
        latency_ms: u64,
        queue_wait_ms: u64,
    ) -> Self {
        Self {
            session_id: request.session_id,
            model_alias: request.model_alias.clone(),
            input_tokens: request.prompt_tokens,
            output_tokens,
            latency_ms,
            queue_wait_ms,
            priority: request.priority,
            was_continuation: request.continuation_of.is_some(),
        }
    }
}

/// Append-only NDJSON trace writer with daily rotation.
pub struct TraceCapture {
    config: TraceConfig,
    writer: Mutex<Option<RotatingWriter>>,
}

struct RotatingWriter {
    date_key: String,
    inner: BufWriter<File>,
}

impl TraceCapture {
    /// Construct a capture. Creates the output directory when enabled.
    pub fn new(config: TraceConfig) -> std::io::Result<Self> {
        if config.enabled {
            std::fs::create_dir_all(&config.output_dir)?;
        }
        Ok(Self {
            config,
            writer: Mutex::new(None),
        })
    }

    /// Disabled capture useful for tests or air-gapped boot where the output
    /// directory may not exist.
    pub fn disabled() -> Self {
        Self {
            config: TraceConfig::default(),
            writer: Mutex::new(None),
        }
    }

    /// Returns whether tracing is currently active.
    pub fn enabled(&self) -> bool {
        self.config.enabled
    }

    /// Record one completed request. Builds the event and, when enabled,
    /// appends it to the current trace file. Returns the constructed event so
    /// tests and diagnostics can inspect what would be written.
    pub fn record(&self, ctx: CaptureContext) -> std::io::Result<TraceEvent> {
        let now = Utc::now();
        let event = self.build_event(&now, ctx);
        if self.config.enabled {
            self.append(&now, &event)?;
        }
        Ok(event)
    }

    fn build_event(&self, now: &DateTime<Utc>, ctx: CaptureContext) -> TraceEvent {
        TraceEvent {
            timestamp: now.to_rfc3339(),
            request_id: uuid::Uuid::new_v4().to_string(),
            session_id_hash: hash_session_id(&ctx.session_id, &self.config.session_id_salt),
            model_alias: ctx.model_alias,
            input_tokens: ctx.input_tokens,
            output_tokens: ctx.output_tokens,
            latency_ms: ctx.latency_ms,
            queue_wait_ms: ctx.queue_wait_ms,
            priority: ctx.priority.to_string(),
            was_continuation: ctx.was_continuation,
        }
    }

    fn append(&self, now: &DateTime<Utc>, event: &TraceEvent) -> std::io::Result<()> {
        let date_key = now.format("%Y-%m-%d").to_string();
        let mut guard = self.writer.lock().unwrap();
        let needs_rotation = match guard.as_ref() {
            None => true,
            Some(w) => w.date_key != date_key,
        };
        if needs_rotation {
            if let Some(old) = guard.as_mut() {
                let _ = old.inner.flush();
            }
            let path = trace_path(&self.config.output_dir, &self.config.file_prefix, &date_key);
            let file = OpenOptions::new().create(true).append(true).open(&path)?;
            *guard = Some(RotatingWriter {
                date_key,
                inner: BufWriter::new(file),
            });
        }
        if let Some(writer) = guard.as_mut() {
            let line = serde_json::to_string(event)
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
            writeln!(writer.inner, "{line}")?;
            writer.inner.flush()?;
        }
        Ok(())
    }

    /// Force flush the current writer, if any.
    pub fn flush(&self) -> std::io::Result<()> {
        let mut guard = self.writer.lock().unwrap();
        if let Some(w) = guard.as_mut() {
            w.inner.flush()?;
        }
        Ok(())
    }
}

/// Resolve the on-disk path for a given day's trace file.
pub fn trace_path(dir: &Path, prefix: &str, date_key: &str) -> PathBuf {
    dir.join(format!("{prefix}-{date_key}.ndjson"))
}

/// Hash a session id with a salt. Truncated blake3 (32 hex chars).
pub fn hash_session_id(session_id: &SequenceId, salt: &str) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(salt.as_bytes());
    hasher.update(session_id.0.as_bytes());
    let hex = hasher.finalize().to_hex().to_string();
    hex[..32].to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn sample_request() -> ScheduleRequest {
        ScheduleRequest {
            session_id: SequenceId::new(),
            model_alias: "qwen3-14b".to_string(),
            prompt_tokens: 128,
            max_tokens: 512,
            priority: Priority::Normal,
            continuation_of: None,
            request_metadata: HashMap::new(),
        }
    }

    #[test]
    fn test_disabled_capture_produces_event_but_no_file() {
        let dir = std::env::temp_dir().join("mai_trace_disabled");
        let _ = std::fs::remove_dir_all(&dir);
        let cfg = TraceConfig {
            enabled: false,
            output_dir: dir.clone(),
            file_prefix: "test".to_string(),
            session_id_salt: "salt".to_string(),
        };
        let capture = TraceCapture::new(cfg).unwrap();
        let req = sample_request();
        let ctx = CaptureContext::from_request(&req, 42, 100, 20);
        let event = capture.record(ctx).unwrap();
        assert_eq!(event.output_tokens, 42);
        assert!(!dir.exists());
    }

    #[test]
    fn test_enabled_capture_writes_ndjson() {
        let dir = std::env::temp_dir().join("mai_trace_enabled");
        let _ = std::fs::remove_dir_all(&dir);
        let cfg = TraceConfig {
            enabled: true,
            output_dir: dir.clone(),
            file_prefix: "test".to_string(),
            session_id_salt: "salt".to_string(),
        };
        let capture = TraceCapture::new(cfg).unwrap();
        let req = sample_request();
        let ctx = CaptureContext::from_request(&req, 7, 250, 30);
        let event = capture.record(ctx).unwrap();
        capture.flush().unwrap();

        let date_key = Utc::now().format("%Y-%m-%d").to_string();
        let path = trace_path(&dir, "test", &date_key);
        assert!(path.exists(), "trace file should exist at {:?}", path);
        let contents = std::fs::read_to_string(&path).unwrap();
        assert!(contents.contains(&event.request_id));
        assert!(contents.contains("qwen3-14b"));
        // Privacy: no prompts/responses fields are present in the schema.
        assert!(!contents.contains("prompt_text"));
        assert!(!contents.contains("response_text"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_session_id_hashing_is_stable_with_salt() {
        let id = SequenceId::new();
        let a = hash_session_id(&id, "salt-1");
        let b = hash_session_id(&id, "salt-1");
        let c = hash_session_id(&id, "salt-2");
        assert_eq!(a, b, "same id+salt produces same hash");
        assert_ne!(a, c, "different salt produces different hash");
        assert_eq!(a.len(), 32);
    }

    #[test]
    fn test_capture_context_marks_continuation() {
        let mut req = sample_request();
        let parent = SequenceId::new();
        req.continuation_of = Some(parent);
        let ctx = CaptureContext::from_request(&req, 0, 0, 0);
        assert!(ctx.was_continuation);
    }
}
