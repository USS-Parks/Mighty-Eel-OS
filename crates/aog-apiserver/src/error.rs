//! Typed API errors for the Loom control-plane apiserver, each mapped to an HTTP
//! status. The authN (`Unauthenticated`, K6) and policy (`Forbidden`, K7)
//! variants are declared now so the error surface is stable when those admission
//! stages land — a handler never has to grow a new arm for them.

use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::json;

use aog_estate::EstateError;

/// A request-handling failure on the control-plane API.
#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    /// Path names a kind that is not in the estate model.
    #[error("unknown kind {0:?}")]
    UnknownKind(String),
    /// Request body was not a well-formed resource of the expected shape.
    #[error("malformed body: {0}")]
    BadBody(String),
    /// Path kind and body kind disagree.
    #[error("kind mismatch: path {path}, body {body}")]
    KindMismatch { path: String, body: String },
    /// Path name and body `metadata.name` disagree.
    #[error("name mismatch: path {path:?}, body {body:?}")]
    NameMismatch { path: String, body: String },
    /// No such object.
    #[error("{kind} {name:?} not found")]
    NotFound { kind: String, name: String },
    /// Optimistic-concurrency / uniqueness conflict at the store (a failed CAS).
    #[error("conflict on {kind} {name:?}: {reason}")]
    Conflict {
        kind: String,
        name: String,
        reason: String,
    },
    /// Structural (schema) validation failure — fail-closed (doctrine D7).
    #[error("invalid resource: {0}")]
    Invalid(#[from] EstateError),
    /// Backend / store failure.
    #[error("store: {0}")]
    Store(String),
    /// Front-door authentication failure (K6 seam).
    #[error("unauthenticated")]
    Unauthenticated,
    /// Policy denied the mutation (K7 deny-wins seam).
    #[error("forbidden: {0}")]
    Forbidden(String),
}

impl ApiError {
    fn status(&self) -> StatusCode {
        match self {
            ApiError::UnknownKind(_)
            | ApiError::BadBody(_)
            | ApiError::KindMismatch { .. }
            | ApiError::NameMismatch { .. } => StatusCode::BAD_REQUEST,
            ApiError::Invalid(_) => StatusCode::UNPROCESSABLE_ENTITY,
            ApiError::NotFound { .. } => StatusCode::NOT_FOUND,
            ApiError::Conflict { .. } => StatusCode::CONFLICT,
            ApiError::Unauthenticated => StatusCode::UNAUTHORIZED,
            ApiError::Forbidden(_) => StatusCode::FORBIDDEN,
            ApiError::Store(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let status = self.status();
        let body = Json(json!({ "error": self.to_string(), "code": status.as_u16() }));
        (status, body).into_response()
    }
}
