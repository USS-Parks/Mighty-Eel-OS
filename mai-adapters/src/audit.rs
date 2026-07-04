//! Audit logging for adapter framework operations.
//!
//! Every call through the adapter framework is logged with timing data.
//! Audit entries are written to the local-only audit trail (never transmitted
//! off-device per Rule 7).

use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use tracing::info;

/// A single audit log entry for an adapter operation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditEntry {
    /// Unix timestamp (milliseconds) when the operation started.
    pub timestamp_ms: u64,
    /// Name of the adapter that was called.
    pub adapter_name: String,
    /// RPC method that was invoked.
    pub method: String,
    /// Duration of the operation in milliseconds.
    pub duration_ms: u64,
    /// Whether the operation succeeded.
    pub success: bool,
    /// Error code if failed (None on success).
    pub error_code: Option<String>,
    /// Request ID for correlation.
    pub request_id: u64,
}

/// Tracks timing for a single operation.
pub struct AuditTimer {
    adapter_name: String,
    method: String,
    request_id: u64,
    start: Instant,
}

impl AuditTimer {
    /// Start timing an operation.
    pub fn start(adapter_name: String, method: String, request_id: u64) -> Self {
        Self {
            adapter_name,
            method,
            request_id,
            start: Instant::now(),
        }
    }

    /// Complete the operation successfully and emit the audit entry.
    pub fn success(self) -> AuditEntry {
        let entry = self.build_entry(true, None);
        emit_audit_entry(&entry);
        entry
    }

    /// Complete the operation with an error and emit the audit entry.
    pub fn failure(self, error_code: String) -> AuditEntry {
        let entry = self.build_entry(false, Some(error_code));
        emit_audit_entry(&entry);
        entry
    }

    #[allow(clippy::cast_possible_truncation)]
    fn build_entry(&self, success: bool, error_code: Option<String>) -> AuditEntry {
        let duration = self.start.elapsed();
        // Safe: Unix timestamps are positive post-epoch, and u128 millis
        // won't exceed u64 for ~584 million years
        #[allow(clippy::cast_sign_loss)]
        let timestamp_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or(Duration::ZERO)
            .as_millis() as u64;

        AuditEntry {
            timestamp_ms,
            adapter_name: self.adapter_name.clone(),
            method: self.method.clone(),
            duration_ms: duration.as_millis() as u64,
            success,
            error_code,
            request_id: self.request_id,
        }
    }
}

/// Emit an audit entry to the tracing system.
/// In production, these are consumed by the mai-core Health Monitor
/// and written to the append-only audit trail on the vault.
fn emit_audit_entry(entry: &AuditEntry) {
    if entry.success {
        info!(
            adapter = %entry.adapter_name,
            method = %entry.method,
            duration_ms = entry.duration_ms,
            request_id = entry.request_id,
            "adapter_audit: OK"
        );
    } else {
        info!(
            adapter = %entry.adapter_name,
            method = %entry.method,
            duration_ms = entry.duration_ms,
            request_id = entry.request_id,
            error_code = entry.error_code.as_deref().unwrap_or("unknown"),
            "adapter_audit: FAILED"
        );
    }
}

/// In-memory audit buffer for batch writes to vault.
/// The vault interface will consume this.
#[derive(Debug, Default)]
pub struct AuditBuffer {
    entries: Vec<AuditEntry>,
    max_buffer_size: usize,
}

impl AuditBuffer {
    /// Create a new audit buffer with a max size.
    pub fn new(max_buffer_size: usize) -> Self {
        Self {
            entries: Vec::with_capacity(max_buffer_size.min(1024)),
            max_buffer_size,
        }
    }

    /// Append an entry to the buffer.
    pub fn push(&mut self, entry: AuditEntry) {
        if self.entries.len() >= self.max_buffer_size {
            // Drop oldest entries (ring buffer behavior)
            self.entries.remove(0);
        }
        self.entries.push(entry);
    }

    /// Drain all entries for writing to vault.
    pub fn drain(&mut self) -> Vec<AuditEntry> {
        std::mem::take(&mut self.entries)
    }

    /// Number of buffered entries.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the buffer is empty.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_audit_timer_success() {
        let timer = AuditTimer::start("ollama".to_string(), "generate".to_string(), 42);
        let entry = timer.success();
        assert_eq!(entry.adapter_name, "ollama");
        assert_eq!(entry.method, "generate");
        assert_eq!(entry.request_id, 42);
        assert!(entry.success);
        assert!(entry.error_code.is_none());
        assert!(entry.timestamp_ms > 0);
    }

    #[test]
    fn test_audit_timer_failure() {
        let timer = AuditTimer::start("vllm".to_string(), "embed".to_string(), 7);
        let entry = timer.failure("Timeout".to_string());
        assert!(!entry.success);
        assert_eq!(entry.error_code, Some("Timeout".to_string()));
    }

    #[test]
    fn test_audit_buffer_push_drain() {
        let mut buf = AuditBuffer::new(100);
        assert!(buf.is_empty());

        let entry = AuditEntry {
            timestamp_ms: 1000,
            adapter_name: "test".to_string(),
            method: "health_check".to_string(),
            duration_ms: 5,
            success: true,
            error_code: None,
            request_id: 1,
        };

        buf.push(entry.clone());
        assert_eq!(buf.len(), 1);

        let drained = buf.drain();
        assert_eq!(drained.len(), 1);
        assert!(buf.is_empty());
    }

    #[test]
    fn test_audit_buffer_overflow() {
        let mut buf = AuditBuffer::new(3);
        for i in 0..5 {
            buf.push(AuditEntry {
                timestamp_ms: i * 1000,
                adapter_name: "test".to_string(),
                method: "call".to_string(),
                duration_ms: 1,
                success: true,
                error_code: None,
                request_id: i,
            });
        }
        // Should only have 3 entries (oldest dropped)
        assert_eq!(buf.len(), 3);
        let entries = buf.drain();
        assert_eq!(entries[0].request_id, 2); // First two were evicted
    }
}
