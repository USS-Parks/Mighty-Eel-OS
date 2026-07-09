//! Request-path observability middleware.
//!
//! Two thin axum middleware layers that wire every request into the
//! [`crate::metrics::MetricsRegistry`] and into the structured-log
//! correlation chain:
//!
//! * [`metrics_middleware`] — increments
//!   [`crate::metrics::REQUESTS_TOTAL`] sliced by `route` and
//!   `status_class`, observes the request duration into
//!   [`crate::metrics::REQUEST_DURATION_MS`], and bumps
//!   [`crate::metrics::AUTH_FAILURES_TOTAL`] /
//!   [`crate::metrics::RATE_LIMITED_TOTAL`] when the response carries
//!   the corresponding status code. This is *passive*: it observes
//!   what handlers already do; it never short-circuits a request.
//!
//! * [`correlation_middleware`] — guarantees every request has an
//!   `X-Request-Id` header. If the caller sent one (validated against
//!   a strict opaque-token alphabet so a hostile caller cannot inject
//!   log entries via the header), it is used; otherwise a fresh UUID
//!   v4 is minted. The ID is attached to the request extensions
//!   (handlers can read it via `Extension<RequestId>`), echoed back in
//!   the response, and recorded in a per-request `tracing::info_span!`
//!   so every log line emitted while handling the request carries the
//!   ID.
//!
//! Both layers are pure axum middleware functions and integrate via
//! `axum::middleware::from_fn_with_state`. They run outside the auth
//! middleware (so health/metrics scrapes are still observed) and
//! outside any handler — the goal is "every request gets counted, no
//! exceptions, no opt-in."

use std::time::Instant;

use axum::extract::{Request, State};
use axum::http::{HeaderName, HeaderValue, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use tracing::Instrument;
use uuid::Uuid;

use crate::metrics::{
    AUTH_FAILURES_TOTAL, Labels, RATE_LIMITED_TOTAL, REQUEST_DURATION_MS, REQUESTS_TOTAL,
};
use crate::state::AppState;

// ─── Correlation ID middleware ─────────────────────────────────────

/// Header name used for request correlation. Lowercase form because
/// HTTP/2 mandates lowercase header names; axum normalizes inbound
/// headers anyway, but we use the canonical form when emitting.
pub const REQUEST_ID_HEADER: &str = "x-request-id";

/// Wrapper around the per-request correlation ID. Handlers that want
/// to log or propagate the ID extract it via
/// `axum::Extension<RequestId>`.
#[derive(Debug, Clone)]
pub struct RequestId(pub String);

/// Inject (or generate) the request ID, propagate it into the
/// `tracing` span for the rest of the request lifetime, and echo it
/// back to the caller as `X-Request-Id`.
pub async fn correlation_middleware(mut request: Request, next: Next) -> Response {
    let incoming = request
        .headers()
        .get(REQUEST_ID_HEADER)
        .and_then(|v| v.to_str().ok())
        .map(sanitize_request_id)
        .filter(|s| !s.is_empty());

    let request_id = incoming.unwrap_or_else(|| Uuid::new_v4().to_string());

    request
        .extensions_mut()
        .insert(RequestId(request_id.clone()));

    // A short, structured span so every log line emitted by downstream
    // handlers carries the request_id field automatically.
    let span = tracing::info_span!(
        "http_request",
        request_id = %request_id,
        method = %request.method(),
        path = %request.uri().path(),
    );

    let mut response = next.run(request).instrument(span).await;

    // Echo the ID back. `HeaderValue::from_str` only fails on
    // non-visible bytes; `sanitize_request_id` already excluded those,
    // and a fresh UUID is always valid. Fall through silently if it
    // somehow does — the caller gets the response, just without the
    // header echo.
    if let Ok(value) = HeaderValue::from_str(&request_id) {
        response
            .headers_mut()
            .insert(HeaderName::from_static(REQUEST_ID_HEADER), value);
    }

    response
}

/// Constrain an incoming `X-Request-Id` to a safe opaque-token
/// alphabet: `[A-Za-z0-9_-]{1,128}`. A hostile caller cannot inject
/// `\r\n` (header smuggling) or arbitrary log noise through the
/// correlation ID — anything outside the alphabet is dropped, and an
/// empty result triggers fresh-UUID generation in the caller.
fn sanitize_request_id(s: &str) -> String {
    s.chars()
        .filter(|c| matches!(c, 'a'..='z' | 'A'..='Z' | '0'..='9' | '_' | '-'))
        .take(128)
        .collect()
}

// ─── Metrics middleware ────────────────────────────────────────────

/// Observe the request: increment `mai_requests_total` and record the
/// duration into `mai_request_duration_ms`. Increment
/// `mai_auth_failures_total` on 401 and `mai_rate_limited_total` on
/// 429. Runs around every request including auth-exempt ones, so the
/// scraper sees `/v1/health` and `/v1/metrics` traffic too.
pub async fn metrics_middleware(
    State(state): State<AppState>,
    request: Request,
    next: Next,
) -> Response {
    let route = normalize_route(request.uri().path());
    let started = Instant::now();

    let response = next.run(request).await;

    let elapsed_ms = started.elapsed().as_secs_f64() * 1_000.0;
    let status = response.status();
    let status_class = match status.as_u16() {
        100..=199 => "1xx",
        200..=299 => "2xx",
        300..=399 => "3xx",
        400..=499 => "4xx",
        _ => "5xx",
    };

    let req_labels = Labels::new()
        .with("route", &route)
        .with("status_class", status_class);
    state.metrics_registry.inc(REQUESTS_TOTAL, req_labels);

    state.metrics_registry.observe(
        REQUEST_DURATION_MS,
        Labels::new().with("route", &route),
        elapsed_ms,
    );

    if status == StatusCode::UNAUTHORIZED {
        state
            .metrics_registry
            .inc(AUTH_FAILURES_TOTAL, Labels::new().with("route", &route));
    }
    if status == StatusCode::TOO_MANY_REQUESTS {
        state
            .metrics_registry
            .inc(RATE_LIMITED_TOTAL, Labels::new().with("route", &route));
    }

    response
}

/// Collapse high-cardinality path segments into route templates so
/// `/v1/models/{id}` doesn't explode the metrics surface with one
/// series per model name. The rules are conservative — only the
/// segments we know are dynamic in `routes.rs` are templated.
fn normalize_route(path: &str) -> String {
    // Strip query string defensively (axum's `uri().path()` already
    // does, but a future change could shift the boundary).
    let path = path.split('?').next().unwrap_or(path);

    let segments: Vec<&str> = path.split('/').collect();
    let mut out = String::with_capacity(path.len());
    for (i, seg) in segments.iter().enumerate() {
        if i > 0 {
            out.push('/');
        }
        // Heuristic: a path segment is "dynamic" if the previous
        // segment is one of the known parent collections in routes.rs.
        // Anything matching that pattern is replaced with `{id}`.
        let prev = if i > 0 { segments[i - 1] } else { "" };
        let is_dynamic = matches!(
            prev,
            "models" | "instances" | "profiles" | "policies" | "modules" | "reports" | "audit"
        ) && !seg.is_empty()
            && !is_known_subaction(seg);
        if is_dynamic {
            out.push_str("{id}");
        } else {
            out.push_str(seg);
        }
    }
    if out.is_empty() { "/".to_string() } else { out }
}

/// Sub-actions that follow a collection name and should NOT be
/// templated (otherwise `/v1/audit/verify` becomes `/v1/audit/{id}`).
fn is_known_subaction(seg: &str) -> bool {
    matches!(
        seg,
        "discover"
            | "install"
            | "remove"
            | "scan"
            | "reload"
            | "template"
            | "generate"
            | "verify"
            | "integrity"
    )
}

// ─── Rate-limit middleware ─────────────────────────────────────────

/// SEC-95 (closes SEC-011-MAI): consult the per-route token-bucket
/// limiter installed on [`AppState`] and short-circuit the request
/// with `429 Too Many Requests` + `Retry-After` if the matching
/// bucket is empty. When no limiter is installed (legacy bring-up /
/// tests / no-profile path), the middleware is a no-op.
///
/// The 429 response is observed by [`metrics_middleware`] via the
/// existing `mai_rate_limited_total` counter — no new metric needed.
/// Placement note: this layer runs OUTSIDE auth so a flood of
/// unauthenticated requests cannot exhaust the auth check itself;
/// rate-limit decisions are by path prefix, not by caller identity.
pub async fn rate_limit_middleware(
    State(state): State<AppState>,
    request: Request,
    next: Next,
) -> Response {
    if let Some(limiter) = state.rate_limiter.as_ref() {
        let path = request.uri().path();
        if let Err(retry_after) = limiter.check(path) {
            let secs = retry_after.as_secs_f64().ceil() as u64;
            let mut resp = (StatusCode::TOO_MANY_REQUESTS, "rate limit exceeded\n").into_response();
            if let Ok(value) = HeaderValue::from_str(&secs.to_string()) {
                resp.headers_mut()
                    .insert(HeaderName::from_static("retry-after"), value);
            }
            return resp;
        }
    }
    next.run(request).await
}

// ─── Tests ─────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sanitize_request_id_strips_header_smuggling() {
        let raw = "abc123\r\nX-Injected: evil";
        let cleaned = sanitize_request_id(raw);
        // \r, \n, ':', ' ', '<', '-' (wait '-' is allowed) ...
        assert!(!cleaned.contains('\r'));
        assert!(!cleaned.contains('\n'));
        assert!(!cleaned.contains(' '));
        assert!(!cleaned.contains(':'));
    }

    #[test]
    fn test_sanitize_request_id_caps_length() {
        let long = "a".repeat(1024);
        let cleaned = sanitize_request_id(&long);
        assert_eq!(cleaned.len(), 128);
    }

    #[test]
    fn test_sanitize_request_id_passes_uuid() {
        let uuid = "550e8400-e29b-41d4-a716-446655440000";
        assert_eq!(sanitize_request_id(uuid), uuid);
    }

    #[test]
    fn test_normalize_route_templates_dynamic_ids() {
        assert_eq!(
            normalize_route("/v1/models/llama-3-70b-instruct"),
            "/v1/models/{id}"
        );
        assert_eq!(
            normalize_route("/v1/profiles/admin-12345"),
            "/v1/profiles/{id}"
        );
    }

    #[test]
    fn test_normalize_route_preserves_subactions() {
        assert_eq!(
            normalize_route("/v1/models/discover"),
            "/v1/models/discover"
        );
        assert_eq!(
            normalize_route("/v1/compliance/policies/reload"),
            "/v1/compliance/policies/reload"
        );
        assert_eq!(
            normalize_route("/v1/compliance/audit/verify"),
            "/v1/compliance/audit/verify"
        );
    }

    #[test]
    fn test_normalize_route_handles_root_and_static() {
        assert_eq!(normalize_route("/"), "/");
        assert_eq!(normalize_route("/v1/health/live"), "/v1/health/live");
        assert_eq!(normalize_route("/v1/metrics"), "/v1/metrics");
    }
}
