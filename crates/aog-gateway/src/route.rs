//! Classify + route (G5) — the destination decision.
//!
//! Two ways to reach a routing decision:
//!
//! * **Classified** — reuse `mai-router`'s `DefaultRouter` (its `RuleBasedClassifier`
//!   detects PHI / SSN / regulated content) to map request text → local / cloud /
//!   deny. PHI is forced **local**.
//! * **Envelope label (the flagship short-circuit)** — if the payload arrives as a
//!   sealed WSF envelope, read the **F5 label** (`fabric_envelope::read_label`) and
//!   use its `classification` + `permitted_destinations` **without re-classifying**.
//!   The upstream sealer already did the classification; re-doing it would be waste
//!   and could disagree with the sealed decision.
//!
//! G5 produces the decision; the surfaces attach it as `x-aog-*` headers (shadow
//! mode — decide + log, never block). Enforcement + shadow/report/enforce modes
//! are G6.

use axum::http::HeaderValue;
use axum::response::Response;
use fabric_contracts::{Envelope, Route};
use mai_router::{RouteRequest, Router, RoutingDecision};

use crate::provider::ChatMessage;

/// Where a [`GatewayRoute`] came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RouteSource {
    /// Classified from request text by `mai-router`.
    Classified,
    /// Read from a sealed envelope's F5 label (no re-classification).
    EnvelopeLabel,
}

/// The gateway's routing decision for a request.
#[derive(Debug, Clone)]
pub struct GatewayRoute {
    /// The destination ceiling.
    pub route: Route,
    /// A human-readable classification label (`"regulated"`, `"restricted"`, …).
    pub classification: String,
    /// Why this route was chosen.
    pub reason: String,
    /// How the decision was reached.
    pub source: RouteSource,
    /// The router explicitly denied the request (a policy concern G6 acts on).
    pub denied: bool,
}

/// Concatenate message contents into the text a classifier scans.
#[must_use]
pub fn query_text(messages: &[ChatMessage]) -> String {
    messages
        .iter()
        .map(|m| m.content.as_str())
        .collect::<Vec<_>>()
        .join("\n")
}

/// Classify request text and pick a destination via `mai-router`. Fails **closed**
/// to `LocalOnly` on any router error — an unclassifiable request never egresses.
#[must_use]
pub fn classify_and_route(
    router: &dyn Router,
    query: &str,
    estimated_tokens: u32,
    profile_id: &str,
    role: &str,
) -> GatewayRoute {
    let req = RouteRequest {
        query: query.to_string(),
        estimated_tokens,
        profile_id: profile_id.to_string(),
        role: role.to_string(),
        upstream_flags: vec![],
    };
    match router.route(&req) {
        Ok(RoutingDecision::Local {
            reason,
            classification,
        }) => GatewayRoute {
            route: Route::LocalOnly,
            classification: classification.as_str().to_string(),
            reason,
            source: RouteSource::Classified,
            denied: false,
        },
        Ok(RoutingDecision::Cloud {
            reason,
            classification,
            ..
        }) => GatewayRoute {
            route: Route::CloudAllowed,
            classification: classification.as_str().to_string(),
            reason,
            source: RouteSource::Classified,
            denied: false,
        },
        Ok(RoutingDecision::Denied {
            code,
            reason,
            classification,
        }) => GatewayRoute {
            route: Route::LocalOnly,
            classification: classification.as_str().to_string(),
            reason: format!("{code}: {reason}"),
            source: RouteSource::Classified,
            denied: true,
        },
        Err(e) => GatewayRoute {
            route: Route::LocalOnly,
            classification: "unknown".to_string(),
            reason: format!("router error, failing closed to local: {e}"),
            source: RouteSource::Classified,
            denied: false,
        },
    }
}

/// The most-restrictive destination among a label's permitted set (empty = local).
fn tighten(destinations: &[Route]) -> Route {
    if destinations.is_empty() || destinations.contains(&Route::LocalOnly) {
        Route::LocalOnly
    } else if destinations.contains(&Route::LocalPreferred) {
        Route::LocalPreferred
    } else {
        Route::CloudAllowed
    }
}

/// Route from a sealed envelope's F5 label — **no re-classification**.
#[must_use]
pub fn route_from_envelope(envelope: &Envelope) -> GatewayRoute {
    let label = fabric_envelope::read_label(envelope);
    GatewayRoute {
        route: tighten(&label.permitted_destinations),
        classification: format!("{:?}", label.classification).to_lowercase(),
        reason: "sealed envelope label (F5) — no re-classification".to_string(),
        source: RouteSource::EnvelopeLabel,
        denied: false,
    }
}

/// Header token for a [`Route`] (matches the `snake_case` wire form).
#[must_use]
pub fn route_header(route: Route) -> &'static str {
    match route {
        Route::LocalOnly => "local_only",
        Route::LocalPreferred => "local_preferred",
        Route::CloudAllowed => "cloud_allowed",
    }
}

/// Header token for a [`RouteSource`].
#[must_use]
pub fn source_header(source: RouteSource) -> &'static str {
    match source {
        RouteSource::Classified => "classified",
        RouteSource::EnvelopeLabel => "envelope_label",
    }
}

/// Attach a routing decision to a response as `x-aog-*` headers — the shadow-mode
/// "decide + log" surface. G6 turns these into report-only / enforce behavior.
#[must_use]
pub fn tag_route(mut resp: Response, decision: &GatewayRoute) -> Response {
    let h = resp.headers_mut();
    h.insert(
        "x-aog-route",
        HeaderValue::from_static(route_header(decision.route)),
    );
    h.insert(
        "x-aog-route-source",
        HeaderValue::from_static(source_header(decision.source)),
    );
    if let Ok(v) = HeaderValue::from_str(&decision.classification) {
        h.insert("x-aog-classification", v);
    }
    if decision.denied {
        h.insert("x-aog-policy", HeaderValue::from_static("router_denied"));
    }
    resp
}

#[cfg(test)]
mod tests {
    use super::*;
    use fabric_contracts::{Classification, ComplianceScope, Label, Seal, Thread};
    use mai_router::DefaultRouter;

    fn enveloped(dests: Vec<Route>) -> Envelope {
        Envelope {
            envelope_id: "e1".to_string(),
            seal: Seal {
                aead_alg: String::new(),
                data_key_wrapped: String::new(),
                nonce: String::new(),
                ciphertext: String::new(),
                aad_hash: String::new(),
            },
            label: Label {
                classification: Classification::Restricted,
                compliance_scopes: vec![ComplianceScope::Hipaa],
                origin: "ingest".to_string(),
                permitted_ops: vec![],
                permitted_destinations: dests,
                detected_entities: vec![],
            },
            thread: Thread {
                created_at: String::new(),
                authorizing_token_id: String::new(),
                previous_hash: String::new(),
                signatures: vec![],
            },
        }
    }

    #[test]
    fn phi_text_is_forced_local() {
        let router = DefaultRouter::with_defaults();
        let d = classify_and_route(
            &router,
            "Patient John Doe, SSN 123-45-6789, diagnosis and treatment plan",
            120,
            "tenant-a",
            "clinician",
        );
        assert_eq!(
            d.route,
            Route::LocalOnly,
            "PHI must be forced local — got {:?}: {}",
            d.route,
            d.reason
        );
        assert_eq!(d.source, RouteSource::Classified);
    }

    #[test]
    fn benign_text_is_not_forced_local() {
        let router = DefaultRouter::with_defaults();
        let d = classify_and_route(
            &router,
            "What is the capital of France?",
            20,
            "tenant-a",
            "user",
        );
        assert_ne!(
            d.route,
            Route::LocalOnly,
            "a benign query should not be forced local: {}",
            d.reason
        );
    }

    #[test]
    fn envelope_label_short_circuits_to_local() {
        // A label that permits only local → local, from the label, no re-classification.
        let d = route_from_envelope(&enveloped(vec![Route::LocalOnly]));
        assert_eq!(d.source, RouteSource::EnvelopeLabel);
        assert_eq!(d.route, Route::LocalOnly);
        assert!(d.reason.contains("no re-classification"));
    }

    #[test]
    fn envelope_label_can_permit_cloud() {
        let d = route_from_envelope(&enveloped(vec![Route::CloudAllowed]));
        assert_eq!(d.route, Route::CloudAllowed);
        // an empty permitted set fails closed to local.
        assert_eq!(
            route_from_envelope(&enveloped(vec![])).route,
            Route::LocalOnly
        );
    }

    #[test]
    fn tag_route_writes_headers() {
        use axum::http::StatusCode;
        use axum::response::IntoResponse;
        let d = GatewayRoute {
            route: Route::LocalOnly,
            classification: "regulated".to_string(),
            reason: "phi".to_string(),
            source: RouteSource::Classified,
            denied: false,
        };
        let resp = tag_route(StatusCode::OK.into_response(), &d);
        let h = resp.headers();
        assert_eq!(h.get("x-aog-route").unwrap(), "local_only");
        assert_eq!(h.get("x-aog-classification").unwrap(), "regulated");
        assert_eq!(h.get("x-aog-route-source").unwrap(), "classified");
    }
}
