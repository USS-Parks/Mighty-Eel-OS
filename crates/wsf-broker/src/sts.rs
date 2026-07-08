//! Minimal STS `AssumeRole` client: build the SigV4-signed form POST, send it
//! with `reqwest`, and parse the XML response — no `aws-sdk` dependency.

use std::fmt;

use chrono::{DateTime, Utc};
use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, utf8_percent_encode};
use zeroize::{Zeroize, ZeroizeOnDrop};

use crate::error::BrokerError;
use crate::sigv4::SigV4Request;

/// AWS-style form encoding: everything except the RFC3986 unreserved set.
const AWS_FORM: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'_')
    .remove(b'.')
    .remove(b'~');

fn enc(s: &str) -> String {
    utf8_percent_encode(s, AWS_FORM).to_string()
}

/// Root credentials the broker holds (custodied in OpenBao), used to sign the
/// AssumeRole call. These are the broker's crown jewels (plan B5): the buffers
/// are zeroized on drop and `Debug` is fully redacted — nothing legitimate
/// needs to print any part of them.
#[derive(Zeroize, ZeroizeOnDrop)]
pub struct RootCredentials {
    pub access_key_id: String,
    pub secret_access_key: String,
    pub session_token: Option<String>,
}

impl fmt::Debug for RootCredentials {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RootCredentials")
            .field("access_key_id", &"<redacted>")
            .field("secret_access_key", &"<redacted>")
            .field("session_token", &"<redacted>")
            .finish()
    }
}

/// Temporary, scoped credentials minted by STS for a trust token.
///
/// Hygiene (plan B5): `Debug` redacts the secret access key and session token
/// (the access key id is CloudTrail-visible correlation data, not a secret).
/// The fields stay movable — they are handed to the caller in the exchange
/// response by design — so containment is redaction plus the short STS TTL,
/// not zeroize-on-drop.
#[derive(Clone)]
pub struct TemporaryCredentials {
    /// Temporary access key id.
    pub access_key_id: String,
    /// Temporary secret access key.
    pub secret_access_key: String,
    /// STS session token.
    pub session_token: String,
    /// Expiry (from STS) — tracks the requested duration / token TTL.
    pub expiration: DateTime<Utc>,
}

impl fmt::Debug for TemporaryCredentials {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TemporaryCredentials")
            .field("access_key_id", &self.access_key_id)
            .field("secret_access_key", &"<redacted>")
            .field("session_token", &"<redacted>")
            .field("expiration", &self.expiration)
            .finish()
    }
}

/// Parameters for one AssumeRole exchange.
pub struct AssumeRoleParams<'a> {
    pub endpoint: &'a str,
    pub region: &'a str,
    pub role_arn: &'a str,
    pub session_name: &'a str,
    pub session_policy: &'a str,
    pub duration_secs: i64,
    /// Optional `ExternalId` from the grant (confused-deputy defense, plan B3):
    /// the target role's trust policy can require it, so only this broker —
    /// via this specific grant — can assume the role.
    pub external_id: Option<&'a str>,
    /// `X-Amz-Date` timestamp, `YYYYMMDDTHHMMSSZ`.
    pub amz_date: &'a str,
    /// Date stamp, `YYYYMMDD`.
    pub datestamp: &'a str,
}

/// Build the AssumeRole form body. Pure, so the parameter binding (policy,
/// duration, external id) is unit-testable without an STS endpoint.
#[must_use]
pub fn assume_role_body(p: &AssumeRoleParams<'_>) -> String {
    let mut body = format!(
        "Action=AssumeRole&Version=2011-06-15&RoleArn={}&RoleSessionName={}&DurationSeconds={}&Policy={}",
        enc(p.role_arn),
        enc(p.session_name),
        p.duration_secs,
        enc(p.session_policy),
    );
    if let Some(ext) = p.external_id {
        body.push_str("&ExternalId=");
        body.push_str(&enc(ext));
    }
    body
}

/// Extract the text of the first `<tag>...</tag>` in `xml`.
fn extract_tag<'a>(xml: &'a str, tag: &str) -> Option<&'a str> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let start = xml.find(&open)? + open.len();
    let rest = &xml[start..];
    let end = rest.find(&close)?;
    Some(&rest[..end])
}

/// The host[:port] of `endpoint`, for the SigV4 `Host` header.
fn host_of(endpoint: &str) -> &str {
    endpoint
        .trim_end_matches('/')
        .split_once("://")
        .map_or(endpoint, |(_, rest)| rest)
}

/// Perform an STS `AssumeRole`, returning the scoped temporary credentials.
///
/// # Errors
/// [`BrokerError::Http`] on transport failure, [`BrokerError::Sts`] on an STS
/// error response, or [`BrokerError::Parse`] if the credentials are missing.
pub async fn assume_role(
    http: &reqwest::Client,
    root: &RootCredentials,
    p: &AssumeRoleParams<'_>,
) -> Result<TemporaryCredentials, BrokerError> {
    let body = assume_role_body(p);

    let sig = SigV4Request {
        access_key: &root.access_key_id,
        secret_key: &root.secret_access_key,
        session_token: root.session_token.as_deref(),
        region: p.region,
        service: "sts",
        host: host_of(p.endpoint),
        amz_date: p.amz_date,
        datestamp: p.datestamp,
        body: &body,
    };
    let auth = sig.authorization_header();

    let url = format!("{}/", p.endpoint.trim_end_matches('/'));
    let mut builder = http
        .post(&url)
        .header("X-Amz-Date", p.amz_date)
        .header("Content-Type", "application/x-www-form-urlencoded")
        .header("Authorization", auth);
    if let Some(tok) = &root.session_token {
        builder = builder.header("X-Amz-Security-Token", tok);
    }
    let resp = builder.body(body).send().await?;

    let status = resp.status();
    let text = resp.text().await?;
    if !status.is_success() {
        let code = extract_tag(&text, "Code").unwrap_or("Unknown");
        let msg = extract_tag(&text, "Message").unwrap_or(text.trim());
        return Err(BrokerError::Sts(format!("{status} {code}: {msg}")));
    }

    let access_key_id = extract_tag(&text, "AccessKeyId")
        .ok_or_else(|| BrokerError::Parse("missing AccessKeyId".into()))?
        .to_string();
    let secret_access_key = extract_tag(&text, "SecretAccessKey")
        .ok_or_else(|| BrokerError::Parse("missing SecretAccessKey".into()))?
        .to_string();
    let session_token = extract_tag(&text, "SessionToken")
        .ok_or_else(|| BrokerError::Parse("missing SessionToken".into()))?
        .to_string();
    let expiration = extract_tag(&text, "Expiration")
        .ok_or_else(|| BrokerError::Parse("missing Expiration".into()))?;
    let expiration = DateTime::parse_from_rfc3339(expiration)
        .map_err(|e| BrokerError::Parse(format!("bad Expiration: {e}")))?
        .with_timezone(&Utc);

    Ok(TemporaryCredentials {
        access_key_id,
        secret_access_key,
        session_token,
        expiration,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_tag_reads_first_occurrence() {
        let xml = "<Credentials><AccessKeyId>AKIA123</AccessKeyId><Expiration>2026-07-03T12:15:00Z</Expiration></Credentials>";
        assert_eq!(extract_tag(xml, "AccessKeyId"), Some("AKIA123"));
        assert_eq!(extract_tag(xml, "Expiration"), Some("2026-07-03T12:15:00Z"));
        assert_eq!(extract_tag(xml, "Missing"), None);
    }

    #[test]
    fn host_of_strips_scheme_and_trailing_slash() {
        assert_eq!(host_of("http://localhost:4566/"), "localhost:4566");
        assert_eq!(host_of("https://sts.amazonaws.com"), "sts.amazonaws.com");
    }

    #[test]
    fn enc_encodes_reserved_but_not_unreserved() {
        assert_eq!(enc("arn:aws:s3:::b/x*"), "arn%3Aaws%3As3%3A%3A%3Ab%2Fx%2A");
        assert_eq!(enc("Abc-1_2.3~"), "Abc-1_2.3~");
    }

    fn params<'a>(external_id: Option<&'a str>, policy: &'a str) -> AssumeRoleParams<'a> {
        AssumeRoleParams {
            endpoint: "http://localhost:5566",
            region: "us-east-1",
            role_arn: "arn:aws:iam::0:role/x",
            session_name: "tok_abc",
            session_policy: policy,
            duration_secs: 900,
            external_id,
            amz_date: "20260707T000000Z",
            datestamp: "20260707",
        }
    }

    #[test]
    fn body_carries_external_id_only_when_granted() {
        let with = assume_role_body(&params(Some("wsf-ext-1"), "{}"));
        assert!(with.contains("&ExternalId=wsf-ext-1"));
        let without = assume_role_body(&params(None, "{}"));
        assert!(!without.contains("ExternalId"));
    }

    #[test]
    fn debug_output_redacts_secret_material() {
        // B5: a stray `{:?}` in a log line must never leak credential secrets.
        let root = RootCredentials {
            access_key_id: "AKIAROOTREDACTME".to_string(),
            secret_access_key: "root-secret-material".to_string(),
            session_token: Some("root-session-material".to_string()),
        };
        let d = format!("{root:?}");
        assert!(!d.contains("AKIAROOTREDACTME"));
        assert!(!d.contains("root-secret-material"));
        assert!(!d.contains("root-session-material"));
        assert!(d.contains("<redacted>"));

        let tmp = TemporaryCredentials {
            access_key_id: "ASIATEMPKEY".to_string(),
            secret_access_key: "temp-secret-material".to_string(),
            session_token: "temp-session-material".to_string(),
            expiration: Utc::now(),
        };
        let d = format!("{tmp:?}");
        assert!(d.contains("ASIATEMPKEY"), "correlation id stays visible");
        assert!(!d.contains("temp-secret-material"));
        assert!(!d.contains("temp-session-material"));
    }

    #[test]
    fn root_credentials_zeroize_clears_buffers() {
        let mut root = RootCredentials {
            access_key_id: "AKIAROOT".to_string(),
            secret_access_key: "root-secret".to_string(),
            session_token: Some("root-session".to_string()),
        };
        root.zeroize();
        assert!(root.access_key_id.is_empty());
        assert!(root.secret_access_key.is_empty());
        assert!(
            root.session_token.as_deref().is_none_or(|t| t.is_empty()),
            "session token cleared"
        );
    }
}
