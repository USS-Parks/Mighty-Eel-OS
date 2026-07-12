//! Streaming protocol implementations for the MAI API server.
//!
//! - **SSE (Server-Sent Events):** Token-by-token delivery for
//!   `stream=true` on `/v1/chat/completions`. OpenAI-compatible format
//!   with sequence numbering, heartbeat, backpressure, and resume.
//!
//! The `TokenSender`/`TokenReceiver` channel abstraction and
//! `BackpressureMonitor` provide flow control.

pub mod sse;

use std::time::Instant;
use tokio::sync::mpsc;

// ─── Shared Token Channel ──────────────────────────────────────────

/// A single token event produced by an adapter during streaming inference.
#[derive(Debug, Clone)]
pub struct TokenEvent {
    /// Monotonic sequence number within this stream (starts at 1).
    pub sequence: u64,
    /// The generated token text fragment (None on final/error events).
    pub token: Option<String>,
    /// Whether this is the final event in the stream.
    pub is_final: bool,
    /// Finish reason if this is the final event.
    pub finish_reason: Option<String>,
    /// Timestamp when this token was produced by the adapter.
    pub produced_at: Instant,
}

/// Channel capacity for token streams. 64 events matches the SSE
/// backpressure buffer size so the channel itself provides natural
/// backpressure signaling.
pub const TOKEN_CHANNEL_CAPACITY: usize = 64;

/// Create a paired token sender and receiver for a single inference stream.
///
/// The sender is handed to the adapter/scheduler side. The receiver is
/// consumed by the SSE or WebSocket handler to emit events to the client.
pub fn token_channel() -> (TokenSender, TokenReceiver) {
    let (tx, rx) = mpsc::channel(TOKEN_CHANNEL_CAPACITY);
    (TokenSender { inner: tx }, TokenReceiver { inner: rx })
}

/// Sending half of the token channel. Held by the adapter bridge.
#[derive(Debug, Clone)]
pub struct TokenSender {
    inner: mpsc::Sender<TokenEvent>,
}

impl TokenSender {
    /// Send a token event. Returns Err if the receiver has been dropped
    /// (client disconnected).
    pub async fn send(&self, event: TokenEvent) -> Result<(), TokenSendError> {
        self.inner
            .send(event)
            .await
            .map_err(|_| TokenSendError::ReceiverDropped)
    }

    /// Try to send without waiting. Returns Err if the channel is full
    /// or the receiver is dropped.
    pub fn try_send(&self, event: TokenEvent) -> Result<(), TokenSendError> {
        self.inner.try_send(event).map_err(|e| match e {
            mpsc::error::TrySendError::Full(_) => TokenSendError::ChannelFull,
            mpsc::error::TrySendError::Closed(_) => TokenSendError::ReceiverDropped,
        })
    }
}

/// Receiving half of the token channel. Held by the streaming handler.
#[derive(Debug)]
pub struct TokenReceiver {
    inner: mpsc::Receiver<TokenEvent>,
}

impl TokenReceiver {
    /// Receive the next token event, or None if the sender is dropped.
    pub async fn recv(&mut self) -> Option<TokenEvent> {
        self.inner.recv().await
    }
}

/// Errors that can occur when sending tokens.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TokenSendError {
    /// The receiving end (client handler) has been dropped.
    ReceiverDropped,
    /// The channel buffer is full (backpressure).
    ChannelFull,
}

impl std::fmt::Display for TokenSendError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ReceiverDropped => write!(f, "Token receiver dropped (client disconnected)"),
            Self::ChannelFull => write!(f, "Token channel full (backpressure)"),
        }
    }
}

impl std::error::Error for TokenSendError {}

// ─── Backpressure Monitor ──────────────────────────────────────────

/// Tracks backpressure state for a single streaming connection.
///
/// When the event buffer fills beyond the threshold, the monitor
/// signals that oldest events should be dropped and a gap marker
/// inserted into the stream.
#[derive(Debug)]
pub struct BackpressureMonitor {
    /// Maximum events to buffer before triggering drops.
    capacity: usize,
    /// Current buffer occupancy.
    current: usize,
    /// Total events dropped due to backpressure in this stream.
    dropped_count: u64,
    /// Sequence number of the last dropped event.
    last_dropped_seq: Option<u64>,
}

impl BackpressureMonitor {
    /// Create a new monitor with the given buffer capacity.
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity,
            current: 0,
            dropped_count: 0,
            last_dropped_seq: None,
        }
    }

    /// Record that an event was buffered.
    pub fn event_buffered(&mut self) {
        self.current = self.current.saturating_add(1);
    }

    /// Record that an event was consumed by the client.
    pub fn event_consumed(&mut self) {
        self.current = self.current.saturating_sub(1);
    }

    /// Record that an event was dropped due to backpressure.
    pub fn event_dropped(&mut self, sequence: u64) {
        self.dropped_count += 1;
        self.last_dropped_seq = Some(sequence);
        self.current = self.current.saturating_sub(1);
    }

    /// Whether the buffer is at capacity and events should be dropped.
    pub fn should_drop(&self) -> bool {
        self.current >= self.capacity
    }

    /// Total events dropped in this stream's lifetime.
    pub fn total_dropped(&self) -> u64 {
        self.dropped_count
    }

    /// Current buffer occupancy.
    pub fn current_occupancy(&self) -> usize {
        self.current
    }
}

// ─── Stream Identifier ─────────────────────────────────────────────

/// Unique identifier for a streaming session. Used to correlate
/// SSE resume requests.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct StreamId(pub String);

impl StreamId {
    /// Generate a new random stream identifier.
    pub fn new() -> Self {
        Self(uuid::Uuid::new_v4().to_string())
    }
}

impl Default for StreamId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for StreamId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

// ─── Tests ─────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_token_channel_send_recv() {
        let (tx, mut rx) = token_channel();
        let event = TokenEvent {
            sequence: 1,
            token: Some("hello".to_string()),
            is_final: false,
            finish_reason: None,
            produced_at: Instant::now(),
        };
        tx.send(event.clone()).await.unwrap();
        let received = rx.recv().await.unwrap();
        assert_eq!(received.sequence, 1);
        assert_eq!(received.token.as_deref(), Some("hello"));
    }

    #[tokio::test]
    async fn test_token_channel_drop_sender() {
        let (tx, mut rx) = token_channel();
        drop(tx);
        assert!(rx.recv().await.is_none());
    }

    #[tokio::test]
    async fn test_token_channel_drop_receiver() {
        let (tx, rx) = token_channel();
        drop(rx);
        let event = TokenEvent {
            sequence: 1,
            token: None,
            is_final: true,
            finish_reason: Some("stop".to_string()),
            produced_at: Instant::now(),
        };
        let result = tx.send(event).await;
        assert_eq!(result.unwrap_err(), TokenSendError::ReceiverDropped);
    }

    #[test]
    fn test_backpressure_monitor() {
        let mut mon = BackpressureMonitor::new(3);
        assert!(!mon.should_drop());
        mon.event_buffered();
        mon.event_buffered();
        mon.event_buffered();
        assert!(mon.should_drop());
        assert_eq!(mon.current_occupancy(), 3);

        mon.event_dropped(3);
        assert_eq!(mon.total_dropped(), 1);
        assert!(!mon.should_drop());
    }

    #[test]
    fn test_stream_id_uniqueness() {
        let a = StreamId::new();
        let b = StreamId::new();
        assert_ne!(a, b);
    }

    #[test]
    fn test_backpressure_consumed() {
        let mut mon = BackpressureMonitor::new(2);
        mon.event_buffered();
        mon.event_buffered();
        assert!(mon.should_drop());
        mon.event_consumed();
        assert!(!mon.should_drop());
        assert_eq!(mon.current_occupancy(), 1);
    }
}
