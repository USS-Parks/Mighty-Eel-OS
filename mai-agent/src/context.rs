//! Context Management for the MAI Agent Interface.
//!
//! Manages multi-turn conversation sessions with:
//! - Per-model context window tracking and token accounting
//! - Priority-based truncation when context exceeds the window
//! - System prompt, RAG context, and tool result injection
//! - Profile-scoped session isolation
//! - Automatic session cleanup on TTL expiry
//!
//! # Architecture
//!
//! The ContextManager does NOT own inference. It assembles the context
//! window that gets passed to the scheduler via InferenceRequest. L4
//! components call inject_rag_context() and inject_tool_result() to
//! add segments. The manager handles truncation and token accounting.
//!
//! # Air-Gap Safety
//!
//! All session data stays in local memory. No persistence layer.
//! Sessions are ephemeral: lost on restart (by design, not a bug).

use std::collections::HashMap;
use std::time::Instant;

use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};
use uuid::Uuid;

use mai_core::types::ProfileId;

use crate::types::{
    AgentError, ContextConfig, ContextPriority, ContextSegment, ConversationSession, SegmentSource,
    SessionId, TokenAccounting,
};

// ============================================================================
// Token Estimation
// ============================================================================

/// Rough token estimate: ~4 characters per token for English text.
/// This is a heuristic. Real tokenization happens at the adapter level.
/// The MAI uses this for budget planning, not for billing.
const CHARS_PER_TOKEN: u32 = 4;

/// Estimate token count from text length.
pub fn estimate_tokens(text: &str) -> u32 {
    let chars = u32::try_from(text.len()).unwrap_or(u32::MAX);
    chars.div_ceil(CHARS_PER_TOKEN)
}

// ============================================================================
// Context Manager
// ============================================================================

/// Manages conversation sessions and context windows.
///
/// Thread safety: NOT internally synchronized. The API layer must wrap
/// this in Arc<RwLock<ContextManager>> (same pattern as ResponseCache).
pub struct ContextManager {
    /// Active sessions indexed by session ID
    sessions: HashMap<SessionId, ConversationSession>,
    /// Default configuration for new sessions
    default_config: ContextConfig,
    /// Maximum total sessions across all profiles
    max_sessions: usize,
}

impl ContextManager {
    /// Create a new context manager with default configuration.
    pub fn new(default_config: ContextConfig) -> Self {
        Self {
            sessions: HashMap::new(),
            default_config,
            max_sessions: 1000,
        }
    }

    /// Create a new conversation session for a profile.
    pub fn create_session(
        &mut self,
        profile_id: ProfileId,
        config: Option<ContextConfig>,
        model_id: Option<String>,
    ) -> Result<SessionId, AgentError> {
        // Enforce session limit (prevent memory exhaustion)
        self.cleanup_expired();
        if self.sessions.len() >= self.max_sessions {
            return Err(AgentError::Internal(format!(
                "Session limit reached: {}/{}",
                self.sessions.len(),
                self.max_sessions
            )));
        }

        let session_id = Uuid::new_v4();
        let cfg = config.unwrap_or_else(|| self.default_config.clone());
        let now = Instant::now();

        let session = ConversationSession {
            id: session_id,
            profile_id,
            config: cfg,
            segments: Vec::new(),
            token_accounting: TokenAccounting::default(),
            turn_count: 0,
            created_at: now,
            last_active: now,
            model_id,
        };

        self.sessions.insert(session_id, session);
        info!(%session_id, %profile_id, "Created conversation session");
        Ok(session_id)
    }

    /// Get session state (read-only).
    pub fn get_session(&self, session_id: &SessionId) -> Result<&ConversationSession, AgentError> {
        self.sessions
            .get(session_id)
            .ok_or_else(|| AgentError::SessionNotFound(session_id.to_string()))
    }

    /// Get mutable session state.
    fn get_session_mut(
        &mut self,
        session_id: &SessionId,
    ) -> Result<&mut ConversationSession, AgentError> {
        // Check expiry first
        if let Some(session) = self.sessions.get(session_id) {
            if session.last_active.elapsed() > session.config.session_ttl {
                self.sessions.remove(session_id);
                return Err(AgentError::SessionExpired(session_id.to_string()));
            }
        }
        self.sessions
            .get_mut(session_id)
            .ok_or_else(|| AgentError::SessionNotFound(session_id.to_string()))
    }

    /// Set the system prompt for a session. Replaces any existing system prompt.
    pub fn set_system_prompt(
        &mut self,
        session_id: &SessionId,
        prompt: String,
    ) -> Result<(), AgentError> {
        let session = self.get_session_mut(session_id)?;
        let token_count = estimate_tokens(&prompt);

        // Remove existing system prompt if any
        session
            .segments
            .retain(|s| s.source != SegmentSource::SystemPrompt);

        let segment = ContextSegment {
            id: format!("{session_id}:system"),
            priority: ContextPriority::System,
            content: prompt,
            token_count,
            source: SegmentSource::SystemPrompt,
            added_at: now_epoch_secs(),
        };

        // System prompt always goes first
        session.segments.insert(0, segment);
        session.last_active = Instant::now();
        self.recompute_accounting(session_id)?;
        Ok(())
    }

    /// Add a user message to the conversation.
    pub fn add_user_message(
        &mut self,
        session_id: &SessionId,
        message: String,
    ) -> Result<(), AgentError> {
        let session = self.get_session_mut(session_id)?;
        let token_count = estimate_tokens(&message);

        let segment = ContextSegment {
            id: format!("{}:user:{}", session_id, session.turn_count),
            priority: ContextPriority::RecentTurn,
            content: message,
            token_count,
            source: SegmentSource::UserMessage,
            added_at: now_epoch_secs(),
        };

        session.segments.push(segment);
        session.last_active = Instant::now();
        self.recompute_accounting(session_id)?;
        self.apply_truncation(session_id)?;
        Ok(())
    }

    /// Add an assistant response to the conversation.
    pub fn add_assistant_message(
        &mut self,
        session_id: &SessionId,
        message: String,
    ) -> Result<(), AgentError> {
        let session = self.get_session_mut(session_id)?;
        let token_count = estimate_tokens(&message);

        let segment = ContextSegment {
            id: format!("{}:assistant:{}", session_id, session.turn_count),
            priority: ContextPriority::RecentTurn,
            content: message,
            token_count,
            source: SegmentSource::AssistantMessage,
            added_at: now_epoch_secs(),
        };

        session.segments.push(segment);
        session.turn_count += 1;
        session.last_active = Instant::now();
        self.recompute_accounting(session_id)?;
        self.apply_truncation(session_id)?;
        Ok(())
    }

    /// Inject RAG retrieval results into the context window.
    ///
    /// RAG context is injected between the system prompt and conversation
    /// history, at RagContext priority (truncated before system prompt but
    /// after older conversation turns).
    pub fn inject_rag_context(
        &mut self,
        session_id: &SessionId,
        chunks: Vec<String>,
    ) -> Result<u32, AgentError> {
        let session = self.get_session_mut(session_id)?;

        // Remove any existing RAG context (fresh retrieval replaces stale)
        session
            .segments
            .retain(|s| s.source != SegmentSource::RagRetrieval);

        let mut total_tokens = 0u32;
        for (i, chunk) in chunks.into_iter().enumerate() {
            let token_count = estimate_tokens(&chunk);
            total_tokens = total_tokens.saturating_add(token_count);

            let segment = ContextSegment {
                id: format!("{session_id}:rag:{i}"),
                priority: ContextPriority::RagContext,
                content: chunk,
                token_count,
                source: SegmentSource::RagRetrieval,
                added_at: now_epoch_secs(),
            };

            // Insert after system prompt, before conversation history
            let insert_pos = session
                .segments
                .iter()
                .position(|s| {
                    s.source != SegmentSource::SystemPrompt
                        && s.source != SegmentSource::RagRetrieval
                })
                .unwrap_or(session.segments.len());
            session.segments.insert(insert_pos, segment);
        }

        session.last_active = Instant::now();
        self.recompute_accounting(session_id)?;
        self.apply_truncation(session_id)?;

        debug!(%session_id, total_tokens, "Injected RAG context");
        Ok(total_tokens)
    }

    /// Inject a tool/function call result into the context.
    pub fn inject_tool_result(
        &mut self,
        session_id: &SessionId,
        call_id: &str,
        tool_id: &str,
        result_text: String,
    ) -> Result<(), AgentError> {
        let session = self.get_session_mut(session_id)?;
        let token_count = estimate_tokens(&result_text);

        let segment = ContextSegment {
            id: format!("{session_id}:tool:{call_id}"),
            priority: ContextPriority::ToolResult,
            content: format!("[Tool: {tool_id}] {result_text}"),
            token_count,
            source: SegmentSource::ToolResult,
            added_at: now_epoch_secs(),
        };

        session.segments.push(segment);
        session.last_active = Instant::now();
        self.recompute_accounting(session_id)?;
        self.apply_truncation(session_id)?;
        Ok(())
    }

    /// Inject family memory context (from FamilyVault).
    pub fn inject_family_memory(
        &mut self,
        session_id: &SessionId,
        memory_text: String,
    ) -> Result<(), AgentError> {
        let session = self.get_session_mut(session_id)?;
        let token_count = estimate_tokens(&memory_text);

        let segment = ContextSegment {
            id: format!("{session_id}:family_memory"),
            priority: ContextPriority::RagContext,
            content: memory_text,
            token_count,
            source: SegmentSource::FamilyMemory,
            added_at: now_epoch_secs(),
        };

        // Insert after system prompt
        let insert_pos = session
            .segments
            .iter()
            .position(|s| s.source != SegmentSource::SystemPrompt)
            .unwrap_or(session.segments.len());
        session.segments.insert(insert_pos, segment);

        session.last_active = Instant::now();
        self.recompute_accounting(session_id)?;
        self.apply_truncation(session_id)?;
        Ok(())
    }

    /// Assemble the full context window as a flat list of messages
    /// suitable for passing to the scheduler as RequestPayload::Chat.
    pub fn assemble_context(
        &self,
        session_id: &SessionId,
    ) -> Result<Vec<(String, String)>, AgentError> {
        let session = self.get_session(session_id)?;
        let mut messages = Vec::new();

        for segment in &session.segments {
            let role = match segment.source {
                SegmentSource::SystemPrompt => "system",
                SegmentSource::UserMessage => "user",
                SegmentSource::AssistantMessage => "assistant",
                SegmentSource::RagRetrieval | SegmentSource::FamilyMemory => "system",
                SegmentSource::ToolResult => "tool",
            };
            messages.push((role.to_string(), segment.content.clone()));
        }

        Ok(messages)
    }

    /// Get token accounting for a session.
    pub fn token_accounting(&self, session_id: &SessionId) -> Result<&TokenAccounting, AgentError> {
        let session = self.get_session(session_id)?;
        Ok(&session.token_accounting)
    }

    /// Report context window utilization for a session.
    pub fn context_report(&self, session_id: &SessionId) -> Result<ContextReport, AgentError> {
        let session = self.get_session(session_id)?;
        let total_used: u32 = session.segments.iter().map(|s| s.token_count).sum();
        let max = session.config.max_context_tokens;

        Ok(ContextReport {
            session_id: session.id,
            max_context_tokens: max,
            used_tokens: total_used,
            available_tokens: max.saturating_sub(total_used),
            segment_count: session.segments.len(),
            turn_count: session.turn_count,
            accounting: session.token_accounting.clone(),
        })
    }

    /// Update the model for a session (e.g., after Sentinel promotion).
    pub fn update_model(
        &mut self,
        session_id: &SessionId,
        model_id: String,
        new_max_tokens: Option<u32>,
    ) -> Result<(), AgentError> {
        let session = self.get_session_mut(session_id)?;
        session.model_id = Some(model_id);
        if let Some(max) = new_max_tokens {
            session.config.max_context_tokens = max;
        }
        session.last_active = Instant::now();
        self.recompute_accounting(session_id)?;
        self.apply_truncation(session_id)?;
        Ok(())
    }

    /// Remove a session.
    pub fn destroy_session(&mut self, session_id: &SessionId) -> bool {
        let removed = self.sessions.remove(session_id).is_some();
        if removed {
            info!(%session_id, "Destroyed conversation session");
        }
        removed
    }

    /// List active sessions for a profile.
    pub fn list_sessions(&self, profile_id: &ProfileId) -> Vec<SessionId> {
        self.sessions
            .iter()
            .filter(|(_, s)| s.profile_id == *profile_id)
            .map(|(id, _)| *id)
            .collect()
    }

    /// Count active sessions.
    pub fn session_count(&self) -> usize {
        self.sessions.len()
    }

    // ── Internal helpers ──────────────────────────────────────────────

    /// Recompute token accounting for a session.
    fn recompute_accounting(&mut self, session_id: &SessionId) -> Result<(), AgentError> {
        let session = self.get_session_mut(session_id)?;
        let mut accounting = TokenAccounting::default();

        for segment in &session.segments {
            match segment.source {
                SegmentSource::SystemPrompt => {
                    accounting.system_tokens += segment.token_count;
                }
                SegmentSource::RagRetrieval | SegmentSource::FamilyMemory => {
                    accounting.rag_tokens += segment.token_count;
                }
                SegmentSource::UserMessage | SegmentSource::AssistantMessage => {
                    accounting.history_tokens += segment.token_count;
                }
                SegmentSource::ToolResult => {
                    accounting.tool_tokens += segment.token_count;
                }
            }
        }

        let total_used = accounting.system_tokens
            + accounting.rag_tokens
            + accounting.history_tokens
            + accounting.tool_tokens;

        accounting.available_tokens = session
            .config
            .max_context_tokens
            .saturating_sub(total_used)
            .saturating_sub(session.config.generation_reserve);

        accounting.cumulative_tokens =
            session.token_accounting.cumulative_tokens + u64::from(total_used);

        session.token_accounting = accounting;
        Ok(())
    }

    /// Apply truncation strategy when context exceeds the window.
    fn apply_truncation(&mut self, session_id: &SessionId) -> Result<(), AgentError> {
        let session = self.get_session_mut(session_id)?;
        let max_tokens = session
            .config
            .max_context_tokens
            .saturating_sub(session.config.generation_reserve);
        let total: u32 = session.segments.iter().map(|s| s.token_count).sum();

        if total <= max_tokens {
            return Ok(());
        }

        let overflow = total - max_tokens;
        debug!(
            %session_id,
            total,
            max_tokens,
            overflow,
            "Context overflow, applying truncation"
        );

        match session.config.truncation_strategy {
            crate::types::TruncationStrategy::OldestFirst => {
                self.truncate_oldest_first(session_id, overflow)?;
            }
            crate::types::TruncationStrategy::MiddleOut => {
                self.truncate_middle_out(session_id, overflow)?;
            }
            crate::types::TruncationStrategy::RelevanceScored => {
                // Falls back to OldestFirst (relevance scoring requires
                // embedding which is an L4 concern, not available here)
                warn!(%session_id, "RelevanceScored not implemented, falling back to OldestFirst");
                self.truncate_oldest_first(session_id, overflow)?;
            }
            crate::types::TruncationStrategy::HardCutoff => {
                self.truncate_hard_cutoff(session_id, overflow)?;
            }
        }

        // Recompute after truncation
        self.recompute_accounting(session_id)?;
        Ok(())
    }

    /// Remove oldest non-system segments until overflow is resolved.
    fn truncate_oldest_first(
        &mut self,
        session_id: &SessionId,
        mut overflow: u32,
    ) -> Result<(), AgentError> {
        let session = self
            .sessions
            .get_mut(session_id)
            .ok_or_else(|| AgentError::SessionNotFound(session_id.to_string()))?;

        // Sort removable segments by priority (lowest first), then by age
        let mut removable_indices: Vec<usize> = session
            .segments
            .iter()
            .enumerate()
            .filter(|(_, s)| s.priority != ContextPriority::System)
            .map(|(i, _)| i)
            .collect();

        // Sort by priority ascending (OlderTurn first), then by added_at ascending
        removable_indices.sort_by(|&a, &b| {
            let sa = &session.segments[a];
            let sb = &session.segments[b];
            sa.priority
                .cmp(&sb.priority)
                .then(sa.added_at.cmp(&sb.added_at))
        });

        let mut to_remove = Vec::new();
        for idx in removable_indices {
            if overflow == 0 {
                break;
            }
            let tokens = session.segments[idx].token_count;
            overflow = overflow.saturating_sub(tokens);
            to_remove.push(idx);
        }

        // Remove in reverse order to preserve indices
        to_remove.sort_unstable();
        to_remove.reverse();
        for idx in to_remove {
            let removed = session.segments.remove(idx);
            debug!(segment_id = %removed.id, tokens = removed.token_count, "Truncated segment");
        }

        Ok(())
    }

    /// Remove middle segments, preserving first (system) and most recent turns.
    fn truncate_middle_out(
        &mut self,
        session_id: &SessionId,
        mut overflow: u32,
    ) -> Result<(), AgentError> {
        let session = self
            .sessions
            .get_mut(session_id)
            .ok_or_else(|| AgentError::SessionNotFound(session_id.to_string()))?;

        let len = session.segments.len();
        if len <= 2 {
            return Ok(());
        }

        // Protect first segment (system) and last 4 segments (recent context)
        let protected_tail = 4.min(len - 1);
        let middle_start = 1; // After system prompt
        let middle_end = len.saturating_sub(protected_tail);

        let mut to_remove = Vec::new();
        for idx in middle_start..middle_end {
            if overflow == 0 {
                break;
            }
            if session.segments[idx].priority == ContextPriority::System {
                continue;
            }
            overflow = overflow.saturating_sub(session.segments[idx].token_count);
            to_remove.push(idx);
        }

        to_remove.reverse();
        for idx in to_remove {
            let removed = session.segments.remove(idx);
            debug!(segment_id = %removed.id, "Truncated middle segment");
        }

        Ok(())
    }

    /// Hard cutoff: remove segments from the end until under budget.
    fn truncate_hard_cutoff(
        &mut self,
        session_id: &SessionId,
        mut overflow: u32,
    ) -> Result<(), AgentError> {
        let session = self
            .sessions
            .get_mut(session_id)
            .ok_or_else(|| AgentError::SessionNotFound(session_id.to_string()))?;

        while overflow > 0 && session.segments.len() > 1 {
            // Find the lowest-priority segment
            let min_idx = session
                .segments
                .iter()
                .enumerate()
                .filter(|(_, s)| s.priority != ContextPriority::System)
                .min_by_key(|(_, s)| s.priority)
                .map(|(i, _)| i);

            match min_idx {
                Some(idx) => {
                    let tokens = session.segments[idx].token_count;
                    overflow = overflow.saturating_sub(tokens);
                    session.segments.remove(idx);
                }
                None => break, // Only system segments remain
            }
        }

        Ok(())
    }

    /// Remove expired sessions.
    fn cleanup_expired(&mut self) {
        let expired: Vec<SessionId> = self
            .sessions
            .iter()
            .filter(|(_, s)| s.last_active.elapsed() > s.config.session_ttl)
            .map(|(id, _)| *id)
            .collect();

        for id in &expired {
            self.sessions.remove(id);
        }
        if !expired.is_empty() {
            info!(count = expired.len(), "Cleaned up expired sessions");
        }
    }
}

/// Summary report of context window utilization.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextReport {
    pub session_id: SessionId,
    pub max_context_tokens: u32,
    pub used_tokens: u32,
    pub available_tokens: u32,
    pub segment_count: usize,
    pub turn_count: u32,
    pub accounting: TokenAccounting,
}

/// Current epoch time in seconds.
fn now_epoch_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config() -> ContextConfig {
        ContextConfig {
            max_context_tokens: 1000,
            system_prompt_reserve: 100,
            rag_context_reserve: 200,
            generation_reserve: 100,
            truncation_strategy: crate::types::TruncationStrategy::OldestFirst,
            max_turns: 10,
            session_ttl: Duration::from_secs(60),
        }
    }

    #[test]
    fn test_create_session() {
        let mut mgr = ContextManager::new(test_config());
        let profile = Uuid::new_v4();
        let sid = mgr.create_session(profile, None, None).unwrap();
        assert_eq!(mgr.session_count(), 1);
        let session = mgr.get_session(&sid).unwrap();
        assert_eq!(session.profile_id, profile);
        assert_eq!(session.turn_count, 0);
    }

    #[test]
    fn test_system_prompt() {
        let mut mgr = ContextManager::new(test_config());
        let sid = mgr.create_session(Uuid::new_v4(), None, None).unwrap();

        mgr.set_system_prompt(&sid, "You are a helpful assistant.".to_string())
            .unwrap();

        let session = mgr.get_session(&sid).unwrap();
        assert_eq!(session.segments.len(), 1);
        assert_eq!(session.segments[0].source, SegmentSource::SystemPrompt);
    }

    #[test]
    fn test_conversation_flow() {
        let mut mgr = ContextManager::new(test_config());
        let sid = mgr.create_session(Uuid::new_v4(), None, None).unwrap();

        mgr.set_system_prompt(&sid, "System.".to_string()).unwrap();
        mgr.add_user_message(&sid, "Hello".to_string()).unwrap();
        mgr.add_assistant_message(&sid, "Hi there!".to_string())
            .unwrap();

        let session = mgr.get_session(&sid).unwrap();
        assert_eq!(session.segments.len(), 3);
        assert_eq!(session.turn_count, 1);

        let messages = mgr.assemble_context(&sid).unwrap();
        assert_eq!(messages.len(), 3);
        assert_eq!(messages[0].0, "system");
        assert_eq!(messages[1].0, "user");
        assert_eq!(messages[2].0, "assistant");
    }

    #[test]
    fn test_rag_injection() {
        let mut mgr = ContextManager::new(test_config());
        let sid = mgr.create_session(Uuid::new_v4(), None, None).unwrap();

        mgr.set_system_prompt(&sid, "System.".to_string()).unwrap();
        mgr.add_user_message(&sid, "What is X?".to_string())
            .unwrap();

        let tokens = mgr
            .inject_rag_context(&sid, vec!["Doc chunk 1".into(), "Doc chunk 2".into()])
            .unwrap();
        assert!(tokens > 0);

        let messages = mgr.assemble_context(&sid).unwrap();
        // System, RAG1, RAG2, User
        assert_eq!(messages.len(), 4);
        assert_eq!(messages[0].0, "system"); // System prompt
        assert_eq!(messages[1].0, "system"); // RAG (injected as system)
        assert_eq!(messages[2].0, "system"); // RAG
        assert_eq!(messages[3].0, "user"); // User message
    }

    #[test]
    fn test_tool_result_injection() {
        let mut mgr = ContextManager::new(test_config());
        let sid = mgr.create_session(Uuid::new_v4(), None, None).unwrap();

        mgr.add_user_message(&sid, "Turn on lights".to_string())
            .unwrap();
        mgr.inject_tool_result(
            &sid,
            "call-1",
            "homebase.lights",
            "Lights turned on".to_string(),
        )
        .unwrap();

        let messages = mgr.assemble_context(&sid).unwrap();
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[1].0, "tool");
    }

    #[test]
    fn test_truncation_oldest_first() {
        let cfg = ContextConfig {
            max_context_tokens: 50,
            generation_reserve: 10,
            ..test_config()
        };
        let mut mgr = ContextManager::new(cfg);
        let sid = mgr.create_session(Uuid::new_v4(), None, None).unwrap();

        // Each message ~5-10 tokens. Fill beyond 40 usable tokens.
        mgr.set_system_prompt(&sid, "Sys".to_string()).unwrap();
        for i in 0..20 {
            mgr.add_user_message(&sid, format!("Message number {i} with some content"))
                .unwrap();
        }

        let session = mgr.get_session(&sid).unwrap();
        let total: u32 = session.segments.iter().map(|s| s.token_count).sum();
        // Should be truncated to fit within 40 tokens
        assert!(
            total <= 40,
            "Total {total} should be <= 40 after truncation"
        );
        // System prompt should survive
        assert_eq!(session.segments[0].source, SegmentSource::SystemPrompt);
    }

    #[test]
    fn test_context_report() {
        let mut mgr = ContextManager::new(test_config());
        let sid = mgr.create_session(Uuid::new_v4(), None, None).unwrap();
        mgr.set_system_prompt(&sid, "System prompt text.".to_string())
            .unwrap();
        mgr.add_user_message(&sid, "Hello world".to_string())
            .unwrap();

        let report = mgr.context_report(&sid).unwrap();
        assert!(report.used_tokens > 0);
        assert!(report.available_tokens > 0);
        assert_eq!(report.segment_count, 2);
    }

    #[test]
    fn test_destroy_session() {
        let mut mgr = ContextManager::new(test_config());
        let sid = mgr.create_session(Uuid::new_v4(), None, None).unwrap();
        assert_eq!(mgr.session_count(), 1);
        assert!(mgr.destroy_session(&sid));
        assert_eq!(mgr.session_count(), 0);
    }

    #[test]
    fn test_list_sessions_by_profile() {
        let mut mgr = ContextManager::new(test_config());
        let profile1 = Uuid::new_v4();
        let profile2 = Uuid::new_v4();
        mgr.create_session(profile1, None, None).unwrap();
        mgr.create_session(profile1, None, None).unwrap();
        mgr.create_session(profile2, None, None).unwrap();

        assert_eq!(mgr.list_sessions(&profile1).len(), 2);
        assert_eq!(mgr.list_sessions(&profile2).len(), 1);
    }

    #[test]
    fn test_estimate_tokens() {
        assert_eq!(estimate_tokens(""), 0);
        assert_eq!(estimate_tokens("hi"), 1);
        assert_eq!(estimate_tokens("hello world, this is a test"), 7);
    }

    #[test]
    fn test_update_model_expands_context() {
        let mut mgr = ContextManager::new(test_config());
        let sid = mgr
            .create_session(Uuid::new_v4(), None, Some("phi-4-mini".into()))
            .unwrap();

        mgr.update_model(&sid, "llama-3.1-70b".into(), Some(32768))
            .unwrap();

        let session = mgr.get_session(&sid).unwrap();
        assert_eq!(session.model_id.as_deref(), Some("llama-3.1-70b"));
        assert_eq!(session.config.max_context_tokens, 32768);
    }
}
