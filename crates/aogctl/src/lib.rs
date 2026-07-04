//! `aogctl` — the Loom control-plane CLI client (K11 kernel subset).
//!
//! A thin HTTP client over `aog-apiserver`'s typed CRUD surface. Every request
//! carries a WSF trust token in the `x-wsf-token` header (the K6 front door), so
//! the CLI earns each action exactly as any other caller does. The binary
//! (`main.rs`) is a formatting shell over this client; the client is what the
//! K11 gate exercises.

use serde_json::Value;

/// Header carrying the base64-encoded JSON trust token (matches the apiserver).
const TOKEN_HEADER: &str = "x-wsf-token";
/// The estate's API group + version path segment.
const API_BASE: &str = "apis/aog.islandmountain.io/v1";

/// A failure talking to the control-plane API.
#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    /// Transport-level failure (connection, timeout).
    #[error("request failed: {0}")]
    Http(String),
    /// The server refused the request; the status + message are surfaced so a
    /// refusal (401 unauth, 402 over-budget, 403 policy, 409 conflict, …) is
    /// visible to the operator, not swallowed.
    #[error("server returned {status}: {message}")]
    Status {
        /// HTTP status code.
        status: u16,
        /// The server's error message (or raw body).
        message: String,
    },
    /// The response body was not the expected JSON.
    #[error("decode failed: {0}")]
    Decode(String),
}

/// An `aog-apiserver` HTTP client.
pub struct Client {
    base: String,
    token: String,
    http: reqwest::Client,
}

impl Client {
    /// Build a client against `base` (e.g. `http://127.0.0.1:8080`) authenticating
    /// with `token` (the base64 trust-token header value).
    #[must_use]
    pub fn new(base: impl Into<String>, token: impl Into<String>) -> Self {
        Self {
            base: base.into(),
            token: token.into(),
            http: reqwest::Client::new(),
        }
    }

    fn collection_url(&self, kind: &str) -> String {
        format!("{}/{API_BASE}/{kind}", self.base)
    }

    fn object_url(&self, kind: &str, name: &str) -> String {
        format!("{}/{API_BASE}/{kind}/{name}", self.base)
    }

    async fn send(&self, request: reqwest::RequestBuilder) -> Result<Value, ClientError> {
        let response = request
            .header(TOKEN_HEADER, &self.token)
            .send()
            .await
            .map_err(|e| ClientError::Http(e.to_string()))?;
        let status = response.status();
        let body = response
            .text()
            .await
            .map_err(|e| ClientError::Http(e.to_string()))?;
        if status.is_success() {
            if body.is_empty() {
                Ok(Value::Null)
            } else {
                serde_json::from_str(&body).map_err(|e| ClientError::Decode(e.to_string()))
            }
        } else {
            let message = serde_json::from_str::<Value>(&body)
                .ok()
                .and_then(|v| {
                    v.get("error")
                        .and_then(Value::as_str)
                        .map(std::borrow::ToOwned::to_owned)
                })
                .unwrap_or(body);
            Err(ClientError::Status {
                status: status.as_u16(),
                message,
            })
        }
    }

    /// Create a resource (`POST`).
    ///
    /// # Errors
    /// [`ClientError`] on transport failure or a non-2xx response.
    pub async fn create(&self, kind: &str, body: &Value) -> Result<Value, ClientError> {
        self.send(self.http.post(self.collection_url(kind)).json(body))
            .await
    }

    /// Replace a resource (`PUT`).
    ///
    /// # Errors
    /// [`ClientError`] on transport failure or a non-2xx response.
    pub async fn replace(
        &self,
        kind: &str,
        name: &str,
        body: &Value,
    ) -> Result<Value, ClientError> {
        self.send(self.http.put(self.object_url(kind, name)).json(body))
            .await
    }

    /// Apply a resource (create, or replace if it already exists) — the name is
    /// taken from `body.metadata.name`.
    ///
    /// # Errors
    /// [`ClientError`] on transport failure or a non-2xx response other than the
    /// create-time conflict that triggers the replace.
    pub async fn apply(&self, kind: &str, body: &Value) -> Result<Value, ClientError> {
        match self.create(kind, body).await {
            Err(ClientError::Status { status: 409, .. }) => {
                let name = body
                    .get("metadata")
                    .and_then(|m| m.get("name"))
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                self.replace(kind, name, body).await
            }
            other => other,
        }
    }

    /// Fetch one resource (`GET`).
    ///
    /// # Errors
    /// [`ClientError`] on transport failure or a non-2xx response.
    pub async fn get(&self, kind: &str, name: &str) -> Result<Value, ClientError> {
        self.send(self.http.get(self.object_url(kind, name))).await
    }

    /// List a kind (`GET`).
    ///
    /// # Errors
    /// [`ClientError`] on transport failure or a non-2xx response.
    pub async fn list(&self, kind: &str) -> Result<Value, ClientError> {
        self.send(self.http.get(self.collection_url(kind))).await
    }

    /// Delete a resource (`DELETE`).
    ///
    /// # Errors
    /// [`ClientError`] on transport failure or a non-2xx response.
    pub async fn delete(&self, kind: &str, name: &str) -> Result<Value, ClientError> {
        self.send(self.http.delete(self.object_url(kind, name)))
            .await
    }
}
