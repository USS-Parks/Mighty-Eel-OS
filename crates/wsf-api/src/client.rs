//! Typed Rust SDK for the WSF API — [`WsfClient`] round-trips every endpoint.

use base64::Engine;
use fabric_contracts::{Envelope, TrustToken};
use wsf_ledger::LedgerEntry;

use crate::{
    AttenuateReq, ExchangeReq, ExchangeResp, IssueReq, ReceiptsResp, SealReq, TokenResp, UnsealReq,
    UnsealResp, VerifyReq, VerifyResp,
};

/// SDK errors.
#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    /// Transport failure.
    #[error("transport: {0}")]
    Http(#[from] reqwest::Error),
    /// The API returned a non-2xx status.
    #[error("api error {status}: {body}")]
    Api {
        /// HTTP status code.
        status: u16,
        /// Response body.
        body: String,
    },
}

/// A typed client for the WSF API.
pub struct WsfClient {
    base: String,
    http: reqwest::Client,
}

impl WsfClient {
    /// A client pointed at `base` (e.g. `http://127.0.0.1:8300`).
    #[must_use]
    pub fn new(base: impl Into<String>) -> Self {
        Self {
            base: base.into(),
            http: reqwest::Client::new(),
        }
    }

    async fn post<Req: serde::Serialize, Resp: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        body: &Req,
    ) -> Result<Resp, ClientError> {
        let resp = self
            .http
            .post(format!("{}{path}", self.base))
            .json(body)
            .send()
            .await?;
        let status = resp.status();
        if !status.is_success() {
            return Err(ClientError::Api {
                status: status.as_u16(),
                body: resp.text().await.unwrap_or_default(),
            });
        }
        Ok(resp.json().await?)
    }

    /// Issue a trust token.
    ///
    /// # Errors
    /// [`ClientError`] on transport or a non-2xx response.
    pub async fn issue(&self, req: &IssueReq) -> Result<TrustToken, ClientError> {
        let r: TokenResp = self.post("/v1/tokens/issue", req).await?;
        Ok(r.token)
    }

    /// Verify a token.
    ///
    /// # Errors
    /// [`ClientError`] on transport failure.
    pub async fn verify(&self, token: &TrustToken) -> Result<VerifyResp, ClientError> {
        self.post(
            "/v1/tokens/verify",
            &VerifyReq {
                token: token.clone(),
            },
        )
        .await
    }

    /// Attenuate a parent into a narrower child under `restrictions`. The child
    /// identity is generated server-side from the authenticated parent — the
    /// caller supplies narrowing intent only (plan T2).
    ///
    /// # Errors
    /// [`ClientError`] on transport or a non-2xx response (widening → 422,
    /// unauthenticated/forged parent → 403).
    pub async fn attenuate(
        &self,
        parent: &TrustToken,
        restrictions: &fabric_token::TokenRestrictions,
    ) -> Result<TrustToken, ClientError> {
        let r: TokenResp = self
            .post(
                "/v1/tokens/attenuate",
                &AttenuateReq {
                    parent: parent.clone(),
                    restrictions: restrictions.clone(),
                },
            )
            .await?;
        Ok(r.token)
    }

    /// Seal a payload into an envelope.
    ///
    /// # Errors
    /// [`ClientError`] on transport or a non-2xx response (e.g. unauthorized → 403).
    pub async fn seal(&self, req: &SealReq) -> Result<Envelope, ClientError> {
        let r: crate::SealResp = self.post("/v1/envelopes/seal", req).await?;
        Ok(r.envelope)
    }

    /// Unseal an envelope, returning the recovered plaintext.
    ///
    /// # Errors
    /// [`ClientError`] on transport, a non-2xx response, or a malformed body.
    pub async fn unseal(&self, req: &UnsealReq) -> Result<Vec<u8>, ClientError> {
        let r: UnsealResp = self.post("/v1/envelopes/unseal", req).await?;
        base64::engine::general_purpose::STANDARD
            .decode(r.plaintext_b64)
            .map_err(|e| ClientError::Api {
                status: 0,
                body: format!("bad base64: {e}"),
            })
    }

    /// Exchange a token for scoped cloud credentials.
    ///
    /// # Errors
    /// [`ClientError`] on transport or a non-2xx response.
    pub async fn exchange(&self, req: &ExchangeReq) -> Result<ExchangeResp, ClientError> {
        self.post("/v1/credentials/exchange", req).await
    }

    /// Query the receipt ledger by correlation field/value (both or neither).
    ///
    /// # Errors
    /// [`ClientError`] on transport or a non-2xx response.
    pub async fn receipts(
        &self,
        field: Option<&str>,
        value: Option<&str>,
    ) -> Result<Vec<LedgerEntry>, ClientError> {
        let mut url = format!("{}/v1/receipts", self.base);
        if let (Some(f), Some(v)) = (field, value) {
            url.push_str(&format!("?field={f}&value={v}"));
        }
        let resp = self.http.get(url).send().await?;
        let status = resp.status();
        if !status.is_success() {
            return Err(ClientError::Api {
                status: status.as_u16(),
                body: resp.text().await.unwrap_or_default(),
            });
        }
        let r: ReceiptsResp = resp.json().await?;
        Ok(r.entries)
    }

    /// Fetch the raw OpenAPI document.
    ///
    /// # Errors
    /// [`ClientError`] on transport failure.
    pub async fn openapi(&self) -> Result<String, ClientError> {
        Ok(self
            .http
            .get(format!("{}/openapi.json", self.base))
            .send()
            .await?
            .text()
            .await?)
    }
}
