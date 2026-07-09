//! A thin async client for the admin API — how the conformance harness (and
//! `aogd`'s own gate test) drive a daemon: form the cluster, write, read, and query
//! leadership over HTTP.

use aog_store::raft::types::{NodeId, RaftResponse};
use aog_store::{Op, Versioned};

use crate::api::{ChangeMembershipRequest, GetRequest, InitializeRequest, LeaderStatus, Member};

/// A failure talking to a daemon's admin API.
#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    #[error("transport: {0}")]
    Transport(#[from] reqwest::Error),
    #[error("status {status}: {message}")]
    Status { status: u16, message: String },
}

/// An HTTP client bound to one daemon's base URL (e.g. `http://127.0.0.1:4600`).
pub struct Client {
    http: reqwest::Client,
    base: String,
}

impl Client {
    #[must_use]
    pub fn new(base: impl Into<String>) -> Self {
        Self {
            http: reqwest::Client::new(),
            base: base.into(),
        }
    }

    /// Lift a non-2xx response into [`ClientError::Status`] (carrying the body).
    async fn ok(response: reqwest::Response) -> Result<reqwest::Response, ClientError> {
        if response.status().is_success() {
            return Ok(response);
        }
        let status = response.status().as_u16();
        let message = response.text().await.unwrap_or_default();
        Err(ClientError::Status { status, message })
    }

    /// Whether the daemon answers `/healthz`.
    ///
    /// # Errors
    /// [`ClientError::Transport`] if the request cannot be sent.
    pub async fn healthz(&self) -> Result<bool, ClientError> {
        let response = self
            .http
            .get(format!("{}/healthz", self.base))
            .send()
            .await?;
        Ok(response.status().is_success())
    }

    /// Form a fresh cluster with `members`.
    ///
    /// # Errors
    /// [`ClientError`] on transport failure or a non-success status.
    pub async fn initialize(&self, members: Vec<Member>) -> Result<(), ClientError> {
        let response = self
            .http
            .post(format!("{}/admin/initialize", self.base))
            .json(&InitializeRequest { members })
            .send()
            .await?;
        Self::ok(response).await.map(|_| ())
    }

    /// Add `member` as a learner.
    ///
    /// # Errors
    /// [`ClientError`] on transport failure or a non-success status.
    pub async fn add_learner(&self, member: Member) -> Result<(), ClientError> {
        let response = self
            .http
            .post(format!("{}/admin/add-learner", self.base))
            .json(&member)
            .send()
            .await?;
        Self::ok(response).await.map(|_| ())
    }

    /// Set the voter set to `voters`.
    ///
    /// # Errors
    /// [`ClientError`] on transport failure or a non-success status.
    pub async fn change_membership(&self, voters: Vec<NodeId>) -> Result<(), ClientError> {
        let response = self
            .http
            .post(format!("{}/admin/change-membership", self.base))
            .json(&ChangeMembershipRequest { voters })
            .send()
            .await?;
        Self::ok(response).await.map(|_| ())
    }

    /// Apply one desired-state mutation and return the store's response.
    ///
    /// # Errors
    /// [`ClientError`] on transport failure or a non-success status.
    pub async fn write(&self, op: Op) -> Result<RaftResponse, ClientError> {
        let response = self
            .http
            .post(format!("{}/admin/write", self.base))
            .json(&op)
            .send()
            .await?;
        Ok(Self::ok(response).await?.json().await?)
    }

    /// Read one key from the daemon's committed state.
    ///
    /// # Errors
    /// [`ClientError`] on transport failure or a non-success status.
    pub async fn get(&self, key: impl Into<String>) -> Result<Option<Versioned>, ClientError> {
        let response = self
            .http
            .post(format!("{}/admin/get", self.base))
            .json(&GetRequest { key: key.into() })
            .send()
            .await?;
        Ok(Self::ok(response).await?.json().await?)
    }

    /// The daemon's id and its current leader view.
    ///
    /// # Errors
    /// [`ClientError`] on transport failure or a non-success status.
    pub async fn leader(&self) -> Result<LeaderStatus, ClientError> {
        let response = self
            .http
            .get(format!("{}/admin/leader", self.base))
            .send()
            .await?;
        Ok(Self::ok(response).await?.json().await?)
    }
}
