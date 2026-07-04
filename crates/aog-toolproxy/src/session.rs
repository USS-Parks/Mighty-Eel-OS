//! Session record + replay (T7).
//!
//! An agent loop is a sequence of steps — a prompt in, the model's turn, a tool
//! call, an approval, a tool result, the next turn — and governance needs the
//! *whole* sequence, not just the tool receipts, to answer "what did this agent
//! actually do?" A [`SessionRecord`] captures that full loop as an ordered,
//! **hash-chained** transcript (`fabric-proof`, the same primitive the tool-receipt
//! chain and the WSF ledger use), so it verifies off-host and **replays
//! deterministically**: the same recorded ledger always reconstructs the same
//! step sequence, and any silent edit to a recorded step is caught on replay.
//!
//! Like every receipt in this crate, events are **metadata-only** — a summary and
//! structured, non-sensitive detail, never raw prompts, tool arguments, or tool
//! output (those are redacted upstream by T5 before they are ever summarised here).
//! The console (C7) renders the replay step-by-step.

use fabric_proof::{ChainLink, GENESIS_HASH, canonical_hash, chain_link, verify_chain};
use serde::Serialize;

/// The kind of a captured agent-loop step.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionEventKind {
    /// A prompt entering the loop (user or system).
    Prompt,
    /// A model turn (assistant output / reasoning summary).
    ModelOutput,
    /// A tool invocation brokered through the proxy.
    ToolCall,
    /// A human approval decision on a gated call.
    Approval,
    /// A tool result re-entering the loop (post-redaction).
    ToolResult,
    /// A freeform governance note.
    Note,
}

/// One captured step of the agent loop. Metadata only.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SessionEvent {
    /// Monotonic position in the session (0-based).
    pub seq: u32,
    pub kind: SessionEventKind,
    /// RFC-3339 timestamp.
    pub at: String,
    /// Who/what produced the step (an actor, a tool id, a model name); optional.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub actor: Option<String>,
    /// A short, human-readable summary (no secrets — T5 has already redacted).
    pub summary: String,
    /// Structured, non-sensitive detail for the step (e.g. `{ "tool_id": ... }`).
    #[serde(skip_serializing_if = "serde_json::Value::is_null")]
    pub detail: serde_json::Value,
}

/// A deterministic replay step — the console-facing projection of a [`SessionEvent`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ReplayStep {
    pub seq: u32,
    pub kind: SessionEventKind,
    pub at: String,
    pub actor: Option<String>,
    pub summary: String,
}

impl From<&SessionEvent> for ReplayStep {
    fn from(e: &SessionEvent) -> Self {
        Self {
            seq: e.seq,
            kind: e.kind,
            at: e.at.clone(),
            actor: e.actor.clone(),
            summary: e.summary.clone(),
        }
    }
}

/// Why a replay failed — always a tamper signal, never a normal outcome.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ReplayError {
    /// The hash chain's links are not continuous from genesis.
    #[error("session chain is broken")]
    ChainBroken,
    /// A recorded event's content no longer hashes to its chained link — it was
    /// edited after the fact.
    #[error("session event seq {0} was tampered")]
    EventTampered(u32),
}

/// An append-only, hash-chained record of one agent session.
#[derive(Debug, Default)]
pub struct SessionRecord {
    session_id: String,
    events: Vec<SessionEvent>,
    links: Vec<ChainLink>,
    last_hash: [u8; 32],
}

impl SessionRecord {
    /// Start an empty record for `session_id`.
    #[must_use]
    pub fn new(session_id: impl Into<String>) -> Self {
        Self {
            session_id: session_id.into(),
            events: Vec::new(),
            links: Vec::new(),
            last_hash: GENESIS_HASH,
        }
    }

    /// The session id.
    #[must_use]
    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    /// Append a step to the session; returns the new chain head (hex). The `seq` is
    /// assigned automatically from the current length.
    pub fn record(
        &mut self,
        kind: SessionEventKind,
        actor: Option<String>,
        summary: impl Into<String>,
        detail: serde_json::Value,
        at: impl Into<String>,
    ) -> String {
        let event = SessionEvent {
            seq: u32::try_from(self.events.len()).unwrap_or(u32::MAX),
            kind,
            at: at.into(),
            actor,
            summary: summary.into(),
            detail,
        };
        let value = serde_json::to_value(&event).expect("session event serializes");
        let entry_hash = canonical_hash(&value).expect("canonical hash of session event");
        self.links.push(ChainLink {
            previous_hash: self.last_hash,
            entry_hash,
        });
        self.last_hash = chain_link(&self.last_hash, &entry_hash);
        self.events.push(event);
        hex::encode(self.last_hash)
    }

    /// The recorded events, in order.
    #[must_use]
    pub fn events(&self) -> &[SessionEvent] {
        &self.events
    }

    /// The chain head (hex).
    #[must_use]
    pub fn head_hex(&self) -> String {
        hex::encode(self.last_hash)
    }

    /// Number of recorded steps.
    #[must_use]
    pub fn len(&self) -> usize {
        self.events.len()
    }

    /// Whether nothing has been recorded.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }

    /// Deterministically reconstruct the session's ordered steps from the ledger.
    ///
    /// Replay is a pure function of the recorded chain, so it always yields the same
    /// step sequence. It also re-hashes every event against its chained link, so a
    /// silent edit to any recorded step is caught here rather than replayed as truth.
    ///
    /// # Errors
    /// [`ReplayError::ChainBroken`] if the links are discontinuous;
    /// [`ReplayError::EventTampered`] if a recorded event's content was edited.
    pub fn replay(&self) -> Result<Vec<ReplayStep>, ReplayError> {
        verify_chain(&self.links).map_err(|_| ReplayError::ChainBroken)?;
        for (event, link) in self.events.iter().zip(&self.links) {
            let value = serde_json::to_value(event).expect("session event serializes");
            let recomputed = canonical_hash(&value).expect("canonical hash of session event");
            if recomputed != link.entry_hash {
                return Err(ReplayError::EventTampered(event.seq));
            }
        }
        Ok(self.events.iter().map(ReplayStep::from).collect())
    }
}

/// A stable digest over a replayed transcript — two replays of the same record
/// produce the same digest, which is what "deterministic replay" means concretely.
#[must_use]
pub fn transcript_digest(steps: &[ReplayStep]) -> String {
    let value = serde_json::to_value(steps).expect("replay steps serialize");
    let hash = canonical_hash(&value).expect("canonical hash of transcript");
    hex::encode(hash)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Record a representative full loop: prompt → model turn → tool call →
    /// approval → tool result.
    fn recorded_session() -> SessionRecord {
        let mut s = SessionRecord::new("sess-1");
        s.record(
            SessionEventKind::Prompt,
            Some("user".to_string()),
            "summarise the incident and page on-call",
            serde_json::Value::Null,
            "2026-07-04T00:00:00Z",
        );
        s.record(
            SessionEventKind::ModelOutput,
            Some("local-model".to_string()),
            "plans to read the log then page on-call",
            serde_json::Value::Null,
            "2026-07-04T00:00:01Z",
        );
        s.record(
            SessionEventKind::ToolCall,
            Some("read.log".to_string()),
            "read.log call c1",
            serde_json::json!({ "tool_id": "read.log", "call_id": "c1" }),
            "2026-07-04T00:00:02Z",
        );
        s.record(
            SessionEventKind::Approval,
            Some("alice".to_string()),
            "approved pager.page",
            serde_json::json!({ "decision": "approved" }),
            "2026-07-04T00:00:03Z",
        );
        s.record(
            SessionEventKind::ToolResult,
            Some("pager.page".to_string()),
            "pager.page succeeded",
            serde_json::json!({ "success": true }),
            "2026-07-04T00:00:04Z",
        );
        s
    }

    #[test]
    fn full_loop_is_captured_in_order() {
        let s = recorded_session();
        assert_eq!(s.len(), 5);
        let kinds: Vec<_> = s.events().iter().map(|e| e.kind).collect();
        assert_eq!(
            kinds,
            vec![
                SessionEventKind::Prompt,
                SessionEventKind::ModelOutput,
                SessionEventKind::ToolCall,
                SessionEventKind::Approval,
                SessionEventKind::ToolResult,
            ]
        );
        // seq is assigned monotonically.
        assert_eq!(s.events()[4].seq, 4);
    }

    #[test]
    fn replay_is_deterministic() {
        let s = recorded_session();
        let a = s.replay().unwrap();
        let b = s.replay().unwrap();
        assert_eq!(a, b, "two replays of one record are identical");
        assert_eq!(
            transcript_digest(&a),
            transcript_digest(&b),
            "the transcript digest is stable — this is what deterministic means"
        );
        assert_eq!(a.len(), 5);
        assert_eq!(a[0].kind, SessionEventKind::Prompt);
        assert_eq!(a[3].actor.as_deref(), Some("alice"));
    }

    #[test]
    fn empty_session_replays_to_nothing() {
        let s = SessionRecord::new("empty");
        assert!(s.is_empty());
        assert_eq!(s.replay().unwrap(), Vec::new());
    }

    #[test]
    fn a_tampered_event_is_caught_on_replay() {
        let mut s = recorded_session();
        // Silently edit a recorded step's summary (without re-chaining).
        s.events[2].summary = "read.secrets call c1".to_string();
        let err = s.replay().unwrap_err();
        assert_eq!(err, ReplayError::EventTampered(2));
    }

    #[test]
    fn a_broken_link_is_caught_on_replay() {
        let mut s = recorded_session();
        // Corrupt a chain link directly.
        s.links[1].previous_hash = [9u8; 32];
        assert_eq!(s.replay().unwrap_err(), ReplayError::ChainBroken);
    }

    #[test]
    fn head_advances_with_each_step() {
        let mut s = SessionRecord::new("sess-2");
        let h0 = s.head_hex();
        s.record(
            SessionEventKind::Note,
            None,
            "start",
            serde_json::Value::Null,
            "2026-07-04T00:00:00Z",
        );
        let h1 = s.head_hex();
        assert_ne!(h0, h1, "the head advances past genesis");
    }
}
