//! Production trace capture.
//!
//! Captures per-request telemetry at completion time for offline replay
//! through the simulator. Privacy guarantees: no prompts, no responses, no
//! user identifiers. Session IDs are hashed at capture time.

pub mod capture;

pub use capture::{
    CaptureContext, TraceCapture, TraceConfig, TraceEvent, hash_session_id, trace_path,
};
