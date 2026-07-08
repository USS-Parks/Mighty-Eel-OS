//! WebSocket server for bidirectional multiplexed streaming.
//!
//! The WebSocket endpoint at `/v1/ws` supports:
//!
//! - **Multiplexed requests:** Each request carries a unique `request_id`.
//!   Responses are tagged with the same ID, allowing concurrent inference
//!   requests on a single connection.
//!
//! - **Client-to-server messages:**
//!   - `inference.request`: Start a new streaming inference request
//!   - `inference.cancel`: Cancel an in-progress request by ID
//!   - `audio.chunk`: Binary frame with PCM audio for speech-to-text
//!   - `tool.result`: Return value from a tool/function call
//!
//! - **Server-to-client messages:**
//!   - `inference.token`: Single token delta for a request
//!   - `inference.complete`: Final response for a request
//!   - `inference.error`: Error for a specific request (does NOT close connection)
//!   - `transcription.partial`: Interim speech-to-text result
//!   - `transcription.final`: Completed transcription
//!
//! - **Auth:** the connection is authenticated by the auth middleware from the
//!   API key on the `/v1/ws` upgrade request; that verified identity is the
//!   connection's authority. An optional `auth.handshake` message only *confirms*
//!   it — the client cannot self-declare a role in-band (audit P3).
//! - **Keepalive:** Server sends WebSocket ping every 30 seconds.
//! - **Graceful shutdown:** Close frame with reason on server shutdown.
//! - **Binary frames:** Accepted for `audio.chunk` (16kHz PCM, 16-bit).

use std::collections::HashMap;
use std::time::Duration;

use axum::extract::State;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::response::IntoResponse;
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};

use tokio::time::interval;
use tracing::{debug, info, warn};
use uuid::Uuid;

use crate::state::AppState;
use crate::types::{ApiChatMessage, ProfileInfo};

// ─── Constants ─────────────────────────────────────────────────────

/// Ping interval for WebSocket keepalive.
const WS_PING_INTERVAL: Duration = Duration::from_secs(30);

/// Maximum concurrent requests per WebSocket connection.
const MAX_CONCURRENT_REQUESTS: usize = 8;

/// Maximum text message size (64 KB).
const MAX_TEXT_MESSAGE_SIZE: usize = 65_536;

/// Maximum binary frame size for audio chunks (1 MB).
const MAX_BINARY_FRAME_SIZE: usize = 1_048_576;

// ─── Client Message Types ──────────────────────────────────────────

/// Envelope for all client-to-server WebSocket messages.
#[derive(Debug, Clone, Deserialize)]
pub struct ClientMessage {
    /// Message type discriminator.
    #[serde(rename = "type")]
    pub msg_type: String,
    /// Unique request identifier (required for inference messages).
    #[serde(default)]
    pub request_id: Option<String>,
    /// Message payload (type-dependent).
    #[serde(default)]
    pub payload: serde_json::Value,
}

/// Inference request payload (mirrors ChatCompletionRequest fields).
#[derive(Debug, Clone, Deserialize)]
pub struct WsInferenceRequest {
    /// Model identifier.
    #[serde(default)]
    pub model: Option<String>,
    /// Conversation messages.
    pub messages: Vec<ApiChatMessage>,
    /// Sampling temperature.
    #[serde(default)]
    pub temperature: Option<f32>,
    /// Maximum tokens.
    #[serde(default)]
    pub max_tokens: Option<u32>,
}

/// Tool result payload returned by the client.
#[derive(Debug, Clone, Deserialize)]
pub struct ToolResultPayload {
    /// The tool call ID this result corresponds to.
    pub tool_call_id: String,
    /// The function name that was called.
    pub function_name: String,
    /// The result value (JSON).
    pub result: serde_json::Value,
}

// ─── Server Message Types ──────────────────────────────────────────

/// Envelope for all server-to-client WebSocket messages.
#[derive(Debug, Clone, Serialize)]
pub struct ServerMessage {
    /// Message type discriminator.
    #[serde(rename = "type")]
    pub msg_type: String,
    /// Request identifier this message belongs to.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
    /// Message payload (type-dependent).
    pub payload: serde_json::Value,
}

impl ServerMessage {
    /// Create a new server message.
    fn new(msg_type: &str, request_id: Option<String>, payload: serde_json::Value) -> Self {
        Self {
            msg_type: msg_type.to_string(),
            request_id,
            payload,
        }
    }

    /// Create an auth.success response.
    fn auth_success(profile_id: &str) -> Self {
        Self::new(
            "auth.success",
            None,
            serde_json::json!({ "profile_id": profile_id }),
        )
    }

    /// Create an auth.error response.
    fn auth_error(reason: &str) -> Self {
        Self::new("auth.error", None, serde_json::json!({ "error": reason }))
    }

    /// Create an inference.token message.
    fn inference_token(request_id: &str, token: &str, sequence: u64) -> Self {
        Self::new(
            "inference.token",
            Some(request_id.to_string()),
            serde_json::json!({
                "token": token,
                "sequence": sequence,
            }),
        )
    }

    /// Create an inference.complete message.
    fn inference_complete(request_id: &str, finish_reason: &str, total_tokens: u32) -> Self {
        Self::new(
            "inference.complete",
            Some(request_id.to_string()),
            serde_json::json!({
                "finish_reason": finish_reason,
                "usage": { "total_tokens": total_tokens },
            }),
        )
    }

    /// Create an inference.error message (does NOT close connection).
    fn inference_error(request_id: &str, code: &str, message: &str) -> Self {
        Self::new(
            "inference.error",
            Some(request_id.to_string()),
            serde_json::json!({
                "code": code,
                "message": message,
            }),
        )
    }

    /// Create a transcription.partial message.
    fn transcription_partial(request_id: &str, text: &str, is_final: bool) -> Self {
        let msg_type = if is_final {
            "transcription.final"
        } else {
            "transcription.partial"
        };
        Self::new(
            msg_type,
            Some(request_id.to_string()),
            serde_json::json!({ "text": text }),
        )
    }

    /// Serialize to a WebSocket text message.
    fn to_ws_message(&self) -> Result<Message, serde_json::Error> {
        let json = serde_json::to_string(self)?;
        Ok(Message::Text(json.into()))
    }
}

// ─── Connection State ──────────────────────────────────────────────

/// Per-connection state tracking active requests and auth status.
struct ConnectionState {
    /// Whether the auth handshake has completed.
    authenticated: bool,
    /// Authenticated profile info (populated after handshake).
    profile: Option<ProfileInfo>,
    /// Active inference request IDs.
    active_requests: HashMap<String, ActiveRequest>,
}

/// Tracking info for an active streaming request.
struct ActiveRequest {
    /// When the request was started.
    started_at: std::time::Instant,
    /// Model being used.
    model_id: Option<String>,
    /// Whether cancellation was requested.
    cancelled: bool,
}

impl ConnectionState {
    fn new() -> Self {
        Self {
            authenticated: false,
            profile: None,
            active_requests: HashMap::new(),
        }
    }

    /// A connection already authenticated by the auth middleware (audit P3): the
    /// identity comes from the verified API key, never the in-band handshake.
    fn authenticated(profile: ProfileInfo) -> Self {
        Self {
            authenticated: true,
            profile: Some(profile),
            active_requests: HashMap::new(),
        }
    }
}

// ─── WebSocket Upgrade Handler ─────────────────────────────────────

/// GET /v1/ws - WebSocket upgrade handler.
///
/// This is registered in routes.rs. It upgrades the HTTP connection
/// to a WebSocket and spawns the connection handler task.
pub async fn ws_upgrade(
    State(state): State<AppState>,
    profile: ProfileInfo,
    ws: WebSocketUpgrade,
) -> impl IntoResponse {
    // `profile` is the identity the auth middleware verified from the API key on
    // the upgrade request (/v1/ws is not auth-exempt). It — not any in-band
    // handshake — is the connection's authority (audit P3).
    ws.max_message_size(MAX_BINARY_FRAME_SIZE)
        .on_upgrade(move |socket| handle_ws_connection(socket, state, profile))
}

// ─── Connection Handler ────────────────────────────────────────────

/// Handle a single WebSocket connection lifecycle.
///
/// Manages auth handshake, message routing, keepalive pings,
/// and graceful cleanup on disconnect.
#[allow(clippy::too_many_lines)]
async fn handle_ws_connection(socket: WebSocket, state: AppState, profile: ProfileInfo) {
    let (mut ws_tx, mut ws_rx) = socket.split();
    let mut conn = ConnectionState::authenticated(profile);
    let mut ping_interval = interval(WS_PING_INTERVAL);
    let conn_id = Uuid::new_v4();

    info!(conn_id = %conn_id, "WebSocket connection opened");

    loop {
        tokio::select! {
            // Incoming message from client
            maybe_msg = ws_rx.next() => {
                match maybe_msg {
                    Some(Ok(msg)) => {
                        match msg {
                            Message::Text(text) => {
                                if text.len() > MAX_TEXT_MESSAGE_SIZE {
                                    let err = ServerMessage::new(
                                        "error",
                                        None,
                                        serde_json::json!({
                                            "code": "MAI-1004",
                                            "message": "Message too large"
                                        }),
                                    );
                                    if let Ok(ws_msg) = err.to_ws_message() {
                                        let _ = ws_tx.send(ws_msg).await;
                                    }
                                    continue;
                                }
                                let response = handle_text_message(
                                    &text,
                                    &mut conn,
                                    &state,
                                ).await;
                                if let Some(resp) = response
                                    && let Ok(ws_msg) = resp.to_ws_message()
                                    && ws_tx.send(ws_msg).await.is_err()
                                {
                                    debug!(conn_id = %conn_id, "WebSocket send failed");
                                    break;
                                }
                            }
                            Message::Binary(data) => {
                                if !conn.authenticated {
                                    let err = ServerMessage::auth_error(
                                        "Authentication required before sending data"
                                    );
                                    if let Ok(ws_msg) = err.to_ws_message() {
                                        let _ = ws_tx.send(ws_msg).await;
                                    }
                                    continue;
                                }
                                if data.len() > MAX_BINARY_FRAME_SIZE {
                                    let err = ServerMessage::new(
                                        "error",
                                        None,
                                        serde_json::json!({
                                            "code": "MAI-1004",
                                            "message": "Binary frame too large"
                                        }),
                                    );
                                    if let Ok(ws_msg) = err.to_ws_message() {
                                        let _ = ws_tx.send(ws_msg).await;
                                    }
                                    continue;
                                }
                                // Binary frames are audio chunks for STT.
                                // Audio processing integration deferred.
                                debug!(
                                    conn_id = %conn_id,
                                    size = data.len(),
                                    "Received audio chunk (STT not yet implemented)"
                                );
                            }
                            Message::Pong(_) => {
                                // Client responded to our ping. Connection is alive.
                                debug!(conn_id = %conn_id, "Pong received");
                            }
                            Message::Ping(data) => {
                                // Respond with pong (axum usually handles this automatically).
                                let _ = ws_tx.send(Message::Pong(data)).await;
                            }
                            Message::Close(reason) => {
                                info!(
                                    conn_id = %conn_id,
                                    reason = ?reason,
                                    "WebSocket close frame received"
                                );
                                break;
                            }
                        }
                    }
                    Some(Err(e)) => {
                        warn!(conn_id = %conn_id, error = %e, "WebSocket error");
                        break;
                    }
                    None => {
                        debug!(conn_id = %conn_id, "WebSocket stream ended");
                        break;
                    }
                }
            }

            // Keepalive ping
            _ = ping_interval.tick() => {
                if ws_tx.send(Message::Ping(vec![0x4D, 0x41, 0x49].into())).await.is_err() {
                    debug!(conn_id = %conn_id, "Ping send failed, client disconnected");
                    break;
                }
            }
        }
    }

    // Cleanup: cancel all active requests
    let active_count = conn.active_requests.len();
    if active_count > 0 {
        info!(
            conn_id = %conn_id,
            active_requests = active_count,
            "Cleaning up active requests on disconnect"
        );
    }
    conn.active_requests.clear();

    info!(conn_id = %conn_id, "WebSocket connection closed");
}

// ─── Message Routing ───────────────────────────────────────────────

/// Route a text message to the appropriate handler.
///
/// Returns an optional response message. Some messages (like inference
/// tokens) generate responses asynchronously via the token channel
/// rather than returning a direct response.
#[allow(clippy::too_many_lines)]
async fn handle_text_message(
    text: &str,
    conn: &mut ConnectionState,
    state: &AppState,
) -> Option<ServerMessage> {
    let client_msg: ClientMessage = match serde_json::from_str(text) {
        Ok(msg) => msg,
        Err(e) => {
            return Some(ServerMessage::new(
                "error",
                None,
                serde_json::json!({
                    "code": "MAI-1001",
                    "message": format!("Invalid message format: {e}"),
                }),
            ));
        }
    };

    // Auth handshake must be first message
    if !conn.authenticated && client_msg.msg_type != "auth.handshake" {
        return Some(ServerMessage::auth_error(
            "First message must be auth.handshake",
        ));
    }

    match client_msg.msg_type.as_str() {
        "auth.handshake" => handle_auth_handshake(conn, &client_msg),
        "inference.request" => handle_inference_request(conn, state, &client_msg).await,
        "inference.cancel" => handle_inference_cancel(conn, &client_msg),
        "tool.result" => handle_tool_result(conn, &client_msg),
        other => Some(ServerMessage::new(
            "error",
            client_msg.request_id,
            serde_json::json!({
                "code": "MAI-1002",
                "message": format!("Unknown message type: '{other}'"),
            }),
        )),
    }
}

// ─── Auth Handshake ────────────────────────────────────────────────

/// Process the `auth.handshake` message.
///
/// The connection's identity is the [`ProfileInfo`] the auth middleware verified
/// from the API key at upgrade — captured in `conn.profile`. The in-band handshake
/// NEVER establishes or changes it: a client could otherwise self-declare `admin`
/// (audit P3). This handler only confirms the already-authenticated identity and
/// ignores the payload's `profile_id`/`role` entirely.
#[allow(clippy::unnecessary_wraps)]
fn handle_auth_handshake(
    conn: &mut ConnectionState,
    _msg: &ClientMessage,
) -> Option<ServerMessage> {
    match &conn.profile {
        Some(profile) => {
            info!(
                profile_id = %profile.profile_id,
                role = ?profile.role,
                "WebSocket handshake confirmed (identity from auth middleware)"
            );
            Some(ServerMessage::auth_success(&profile.profile_id))
        }
        // Unreachable in production: /v1/ws is behind the auth middleware, so an
        // upgraded connection always carries a verified profile. Fail closed.
        None => Some(ServerMessage::auth_error("connection is not authenticated")),
    }
}

// ─── Inference Request ─────────────────────────────────────────────

/// Handle an inference.request message.
///
/// Validates the request, checks concurrent request limits, and
/// registers the request. In full integration, this spawns a
/// streaming task that feeds tokens back via the WebSocket.
#[allow(clippy::unused_async)] // will await adapter calls in future sessions
async fn handle_inference_request(
    conn: &mut ConnectionState,
    state: &AppState,
    msg: &ClientMessage,
) -> Option<ServerMessage> {
    let Some(request_id) = msg.request_id.clone() else {
        return Some(ServerMessage::new(
            "error",
            None,
            serde_json::json!({
                "code": "MAI-1002",
                "message": "inference.request requires request_id",
            }),
        ));
    };

    // Check concurrent request limit
    if conn.active_requests.len() >= MAX_CONCURRENT_REQUESTS {
        return Some(ServerMessage::inference_error(
            &request_id,
            "MAI-3001",
            &format!("Maximum concurrent requests ({MAX_CONCURRENT_REQUESTS}) reached"),
        ));
    }

    // Check for duplicate request_id
    if conn.active_requests.contains_key(&request_id) {
        return Some(ServerMessage::inference_error(
            &request_id,
            "MAI-1002",
            "Duplicate request_id",
        ));
    }

    // Parse the inference request payload
    let ws_req: WsInferenceRequest = match serde_json::from_value(msg.payload.clone()) {
        Ok(r) => r,
        Err(e) => {
            return Some(ServerMessage::inference_error(
                &request_id,
                "MAI-1001",
                &format!("Invalid inference request: {e}"),
            ));
        }
    };

    if ws_req.messages.is_empty() {
        return Some(ServerMessage::inference_error(
            &request_id,
            "MAI-1002",
            "Messages array cannot be empty",
        ));
    }

    // Register the active request
    conn.active_requests.insert(
        request_id.clone(),
        ActiveRequest {
            started_at: std::time::Instant::now(),
            model_id: ws_req.model.clone(),
            cancelled: false,
        },
    );

    // Streaming inference is not yet wired to the scheduler + token pipeline.
    // Report an explicit not-implemented error instead of a fabricated
    // inference.complete(0 tokens), so a client cannot mistake the unwired
    // endpoint for a working one (audit P4). The request is registered above and
    // unregistered here so the connection's in-flight bookkeeping stays honest.
    // TODO(basho): build the streaming flow — route the InferenceRequest through
    // the scheduler, stream inference.token per token, then inference.complete on
    // finish.
    info!(
        request_id = %request_id,
        model = ?ws_req.model,
        messages = ws_req.messages.len(),
        "WebSocket inference rejected: streaming inference not implemented"
    );

    conn.active_requests.remove(&request_id);

    Some(ServerMessage::inference_error(
        &request_id,
        "MAI-5004",
        "WebSocket streaming inference is not yet implemented",
    ))
}

// ─── Inference Cancel ──────────────────────────────────────────────

/// Handle an inference.cancel message.
///
/// Marks the request as cancelled. The streaming task checks this
/// flag and stops producing tokens.
#[allow(clippy::unnecessary_wraps)]
fn handle_inference_cancel(
    conn: &mut ConnectionState,
    msg: &ClientMessage,
) -> Option<ServerMessage> {
    let Some(request_id) = msg.request_id.clone() else {
        return Some(ServerMessage::new(
            "error",
            None,
            serde_json::json!({
                "code": "MAI-1002",
                "message": "inference.cancel requires request_id",
            }),
        ));
    };

    match conn.active_requests.get_mut(&request_id) {
        Some(req) => {
            req.cancelled = true;
            info!(request_id = %request_id, "Inference request cancelled");
            Some(ServerMessage::new(
                "inference.cancelled",
                Some(request_id),
                serde_json::json!({ "status": "cancelled" }),
            ))
        }
        None => Some(ServerMessage::inference_error(
            &request_id,
            "MAI-2002",
            "No active request with this ID",
        )),
    }
}

// ─── Tool Result ───────────────────────────────────────────────────

/// Handle a tool.result message.
///
/// Tool calling integration is built. This handler
/// validates the message format and acknowledges receipt.
#[allow(clippy::unnecessary_wraps)]
fn handle_tool_result(conn: &mut ConnectionState, msg: &ClientMessage) -> Option<ServerMessage> {
    let Some(request_id) = msg.request_id.clone() else {
        return Some(ServerMessage::new(
            "error",
            None,
            serde_json::json!({
                "code": "MAI-1002",
                "message": "tool.result requires request_id",
            }),
        ));
    };

    let tool_result: ToolResultPayload = match serde_json::from_value(msg.payload.clone()) {
        Ok(r) => r,
        Err(e) => {
            return Some(ServerMessage::inference_error(
                &request_id,
                "MAI-1001",
                &format!("Invalid tool result: {e}"),
            ));
        }
    };

    debug!(
        request_id = %request_id,
        tool_call_id = %tool_result.tool_call_id,
        function = %tool_result.function_name,
        "Tool result received (processing not yet implemented)"
    );

    Some(ServerMessage::new(
        "tool.acknowledged",
        Some(request_id),
        serde_json::json!({
            "tool_call_id": tool_result.tool_call_id,
            "status": "received",
        }),
    ))
}

// ─── Tests ─────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::ProfileRole;

    fn profile(role: ProfileRole) -> ProfileInfo {
        let permissions = role.permissions();
        ProfileInfo {
            profile_id: "family-kid".to_string(),
            role,
            display_name: Some("Kid".to_string()),
            permissions,
        }
    }

    #[test]
    fn test_server_message_auth_success() {
        let msg = ServerMessage::auth_success("profile-1");
        assert_eq!(msg.msg_type, "auth.success");
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("profile-1"));
    }

    #[test]
    fn test_server_message_auth_error() {
        let msg = ServerMessage::auth_error("bad token");
        assert_eq!(msg.msg_type, "auth.error");
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("bad token"));
    }

    #[test]
    fn test_server_message_inference_token() {
        let msg = ServerMessage::inference_token("req-1", "hello", 5);
        assert_eq!(msg.msg_type, "inference.token");
        assert_eq!(msg.request_id.as_deref(), Some("req-1"));
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("hello"));
        assert!(json.contains("5"));
    }

    #[test]
    fn test_server_message_inference_complete() {
        let msg = ServerMessage::inference_complete("req-2", "stop", 42);
        assert_eq!(msg.msg_type, "inference.complete");
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("stop"));
        assert!(json.contains("42"));
    }

    #[test]
    fn test_server_message_inference_error() {
        let msg = ServerMessage::inference_error("req-3", "MAI-2001", "Model not found");
        assert_eq!(msg.msg_type, "inference.error");
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("MAI-2001"));
        assert!(json.contains("Model not found"));
    }

    #[test]
    fn test_client_message_deserialize() {
        let json = r#"{
            "type": "inference.request",
            "request_id": "req-1",
            "payload": { "messages": [{"role": "user", "content": "hi"}] }
        }"#;
        let msg: ClientMessage = serde_json::from_str(json).unwrap();
        assert_eq!(msg.msg_type, "inference.request");
        assert_eq!(msg.request_id.as_deref(), Some("req-1"));
    }

    #[test]
    fn test_ws_inference_request_deserialize() {
        let json = r#"{
            "model": "phi-4-mini",
            "messages": [
                {"role": "system", "content": "You are helpful."},
                {"role": "user", "content": "Hello"}
            ],
            "temperature": 0.7,
            "max_tokens": 256
        }"#;
        let req: WsInferenceRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.model.as_deref(), Some("phi-4-mini"));
        assert_eq!(req.messages.len(), 2);
        assert_eq!(req.temperature, Some(0.7));
    }

    #[test]
    fn test_tool_result_deserialize() {
        let json = r#"{
            "tool_call_id": "call-123",
            "function_name": "get_weather",
            "result": {"temp": 72, "unit": "F"}
        }"#;
        let tr: ToolResultPayload = serde_json::from_str(json).unwrap();
        assert_eq!(tr.tool_call_id, "call-123");
        assert_eq!(tr.function_name, "get_weather");
    }

    #[test]
    fn test_server_message_to_ws_message() {
        let msg = ServerMessage::auth_success("test");
        let ws_msg = msg.to_ws_message().unwrap();
        match ws_msg {
            Message::Text(text) => {
                assert!(text.contains("auth.success"));
                assert!(text.contains("test"));
            }
            _ => panic!("Expected text message"),
        }
    }

    #[test]
    fn test_connection_state_new() {
        let conn = ConnectionState::new();
        assert!(!conn.authenticated);
        assert!(conn.profile.is_none());
        assert!(conn.active_requests.is_empty());
    }

    #[test]
    fn test_handshake_ignores_declared_role_uses_middleware_identity() {
        // Audit P3: the connection is authenticated by the middleware as a Guest.
        // A handshake payload self-declaring `admin` must NOT change the role — the
        // in-band payload is ignored; the middleware identity stands.
        let mut conn = ConnectionState::authenticated(profile(ProfileRole::Guest));
        let forged = ClientMessage {
            msg_type: "auth.handshake".to_string(),
            request_id: None,
            payload: serde_json::json!({
                "profile_id": "attacker",
                "role": "admin",
            }),
        };
        let resp = handle_auth_handshake(&mut conn, &forged).unwrap();
        assert_eq!(resp.msg_type, "auth.success");
        // Role stays Guest; profile_id stays the authenticated one, not "attacker".
        let profile = conn.profile.as_ref().unwrap();
        assert_eq!(profile.role, ProfileRole::Guest);
        assert_eq!(profile.profile_id, "family-kid");
    }

    #[test]
    fn test_handshake_without_authenticated_profile_fails_closed() {
        // Defensive: a connection with no middleware-verified profile cannot be
        // authenticated by the handshake alone.
        let mut conn = ConnectionState::new();
        let msg = ClientMessage {
            msg_type: "auth.handshake".to_string(),
            request_id: None,
            payload: serde_json::json!({ "profile_id": "x", "role": "admin" }),
        };
        let resp = handle_auth_handshake(&mut conn, &msg).unwrap();
        assert_eq!(resp.msg_type, "auth.error");
    }

    #[test]
    fn test_handle_inference_cancel_not_found() {
        let mut conn = ConnectionState::new();
        conn.authenticated = true;
        let msg = ClientMessage {
            msg_type: "inference.cancel".to_string(),
            request_id: Some("nonexistent".to_string()),
            payload: serde_json::Value::Null,
        };
        let resp = handle_inference_cancel(&mut conn, &msg).unwrap();
        assert_eq!(resp.msg_type, "inference.error");
    }

    #[test]
    fn test_handle_inference_cancel_success() {
        let mut conn = ConnectionState::new();
        conn.authenticated = true;
        conn.active_requests.insert(
            "req-1".to_string(),
            ActiveRequest {
                started_at: std::time::Instant::now(),
                model_id: None,
                cancelled: false,
            },
        );
        let msg = ClientMessage {
            msg_type: "inference.cancel".to_string(),
            request_id: Some("req-1".to_string()),
            payload: serde_json::Value::Null,
        };
        let resp = handle_inference_cancel(&mut conn, &msg).unwrap();
        assert_eq!(resp.msg_type, "inference.cancelled");
        assert!(conn.active_requests["req-1"].cancelled);
    }

    #[test]
    fn test_transcription_messages() {
        let partial = ServerMessage::transcription_partial("req-5", "Hello wor", false);
        assert_eq!(partial.msg_type, "transcription.partial");

        let final_msg = ServerMessage::transcription_partial("req-5", "Hello world", true);
        assert_eq!(final_msg.msg_type, "transcription.final");
    }
}
