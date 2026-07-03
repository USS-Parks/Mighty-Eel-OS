//! Minimal STS `AssumeRole` client: build the SigV4-signed form POST, send it
//! with `reqwest`, and parse the XML response — no `aws-sdk` dependency.

use chrono::{DateTime, Utc};
use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, utf8_percent_encode};

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
/// AssumeRole call.
pub struct RootCredentials {
    pub access_key_id: String,
    pub secret_access_key: String,
    pub session_token: Option<String>,
}

/// Temporary, scoped credentials minted by STS for a trust token.
#[derive(Debug, Clone)]
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

/// Parameters for one AssumeRole exchange.
pub struct AssumeRoleParams<'a> {
    pub endpoint: &'a str,
    pub region: &'a str,
    pub role_arn: &'a str,
    pub session_name: &'a str,
    pub session_policy: &'a str,
    pub duration_secs: i64,
    /// `X-Amz-Date` timestamp, `YYYYMMDDTHHMMSSZ`.
    pub amz_date: &'a str,
    /// Date stamp, `YYYYMMDD`.
    pub datestamp: &'a str,
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
    let body = format!(
        "Action=AssumeRole&Version=2011-06-15&RoleArn={}&RoleSessionName={}&DurationSeconds={}&Policy={}",
        enc(p.role_arn),
        enc(p.session_name),
        p.duration_secs,
        enc(p.session_policy),
    );

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
}
