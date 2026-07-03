//! AWS Signature Version 4 signing for the STS `AssumeRole` call.
//!
//! Hand-rolled (over `hmac` + `sha2`, already in the trust-plane's dep set)
//! rather than pulling the full `aws-sdk-*` stack — which drags `aws-lc-rs`
//! (a C/assembler build, fragile on Windows) and a large surface for one POST.
//! The signing-key derivation is pinned to AWS's documented known-answer vector
//! (`signing_key_matches_aws_known_answer`), so correctness does not depend on
//! LocalStack (which does not verify signatures).

use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};

type HmacSha256 = Hmac<Sha256>;

fn hmac(key: &[u8], data: &[u8]) -> [u8; 32] {
    let mut mac = HmacSha256::new_from_slice(key).expect("HMAC accepts any key length");
    mac.update(data);
    mac.finalize().into_bytes().into()
}

fn sha256_hex(data: &[u8]) -> String {
    hex::encode(Sha256::digest(data))
}

/// Derive the SigV4 signing key: `HMAC(HMAC(HMAC(HMAC("AWS4"+secret, date),
/// region), service), "aws4_request")`.
#[must_use]
pub fn signing_key(secret: &str, datestamp: &str, region: &str, service: &str) -> [u8; 32] {
    let k_date = hmac(format!("AWS4{secret}").as_bytes(), datestamp.as_bytes());
    let k_region = hmac(&k_date, region.as_bytes());
    let k_service = hmac(&k_region, service.as_bytes());
    hmac(&k_service, b"aws4_request")
}

/// Compute the SigV4 hex signature over an already-formed canonical request.
fn signature_hex(
    secret: &str,
    datestamp: &str,
    region: &str,
    service: &str,
    amz_date: &str,
    canonical_request: &str,
) -> String {
    let scope = format!("{datestamp}/{region}/{service}/aws4_request");
    let string_to_sign = format!(
        "AWS4-HMAC-SHA256\n{amz_date}\n{scope}\n{}",
        sha256_hex(canonical_request.as_bytes())
    );
    let key = signing_key(secret, datestamp, region, service);
    hex::encode(hmac(&key, string_to_sign.as_bytes()))
}

/// Inputs to build a SigV4 `Authorization` header for a form-POST to a service
/// root path (`/`), as STS uses.
pub struct SigV4Request<'a> {
    /// Access key id.
    pub access_key: &'a str,
    /// Secret access key.
    pub secret_key: &'a str,
    /// Optional session token (for temporary root creds).
    pub session_token: Option<&'a str>,
    /// Region, e.g. `us-east-1`.
    pub region: &'a str,
    /// Service, e.g. `sts`.
    pub service: &'a str,
    /// `Host` header value (host[:port]).
    pub host: &'a str,
    /// `X-Amz-Date` timestamp, `YYYYMMDDTHHMMSSZ`.
    pub amz_date: &'a str,
    /// Date stamp, `YYYYMMDD`.
    pub datestamp: &'a str,
    /// The `application/x-www-form-urlencoded` request body (signed verbatim).
    pub body: &'a str,
}

impl SigV4Request<'_> {
    /// Compute the `Authorization` header value for this request.
    #[must_use]
    pub fn authorization_header(&self) -> String {
        let payload_hash = sha256_hex(self.body.as_bytes());

        // Canonical headers are sorted by lowercase name: content-type < host <
        // x-amz-date < x-amz-security-token.
        let mut canonical_headers = format!(
            "content-type:application/x-www-form-urlencoded\nhost:{}\nx-amz-date:{}\n",
            self.host, self.amz_date
        );
        let mut signed_headers = String::from("content-type;host;x-amz-date");
        if let Some(tok) = self.session_token {
            canonical_headers.push_str(&format!("x-amz-security-token:{tok}\n"));
            signed_headers.push_str(";x-amz-security-token");
        }

        let canonical_request =
            format!("POST\n/\n\n{canonical_headers}\n{signed_headers}\n{payload_hash}");
        let scope = format!(
            "{}/{}/{}/aws4_request",
            self.datestamp, self.region, self.service
        );
        let signature = signature_hex(
            self.secret_key,
            self.datestamp,
            self.region,
            self.service,
            self.amz_date,
            &canonical_request,
        );
        format!(
            "AWS4-HMAC-SHA256 Credential={}/{}, SignedHeaders={}, Signature={}",
            self.access_key, scope, signed_headers, signature
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn end_to_end_matches_aws_sigv4_test_suite_get_vanilla() {
        // AWS `aws4_testsuite` canonical case `get-vanilla`: GET / with only
        // host + x-amz-date signed, empty payload. This locks the whole signing
        // path (key derivation + string-to-sign + final HMAC) to AWS's own
        // published vector — correctness independent of any live service.
        let canonical_request = concat!(
            "GET\n",
            "/\n",
            "\n",
            "host:example.amazonaws.com\n",
            "x-amz-date:20150830T123600Z\n",
            "\n",
            "host;x-amz-date\n",
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
        );
        let signature = signature_hex(
            "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY",
            "20150830",
            "us-east-1",
            "service",
            "20150830T123600Z",
            canonical_request,
        );
        assert_eq!(
            signature,
            "5fa00fa31553b73ebf1942676e86291e8372ff2a2260956d9b8aae1d763fbf31"
        );
    }

    #[test]
    fn authorization_header_is_well_formed() {
        let req = SigV4Request {
            access_key: "AKIDEXAMPLE",
            secret_key: "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY",
            session_token: None,
            region: "us-east-1",
            service: "sts",
            host: "localhost:4566",
            amz_date: "20260703T120000Z",
            datestamp: "20260703",
            body: "Action=AssumeRole&Version=2011-06-15",
        };
        let auth = req.authorization_header();
        assert!(auth.starts_with(
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260703/us-east-1/sts/aws4_request"
        ));
        assert!(auth.contains("SignedHeaders=content-type;host;x-amz-date,"));
        assert!(auth.contains("Signature="));
        // Deterministic for fixed inputs.
        assert_eq!(auth, req.authorization_header());
    }

    #[test]
    fn session_token_extends_signed_headers() {
        let req = SigV4Request {
            access_key: "AKIDEXAMPLE",
            secret_key: "secret",
            session_token: Some("FwoGZXIvYXdzEID"),
            region: "us-east-1",
            service: "sts",
            host: "localhost:4566",
            amz_date: "20260703T120000Z",
            datestamp: "20260703",
            body: "Action=AssumeRole",
        };
        assert!(
            req.authorization_header()
                .contains("SignedHeaders=content-type;host;x-amz-date;x-amz-security-token,")
        );
    }
}
