//! Shared gateway application state + the auth seam every inference surface
//! funnels through.
//!
//! The OpenAI (G3) and Anthropic (G4) surfaces are thin translators in front of
//! the same [`AppState`]: authorize the virtual key (G1), map the requested model
//! to a provider target, dispatch through the [`Registry`] (G2), and translate the
//! neutral response back to the caller's wire format. A [`ModelMap`] is the
//! model-alias → provider routing config; G5 layers classify-and-route on top of
//! it (e.g. PHI forces a local target regardless of the requested model).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use axum::Json;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use chrono::Utc;
use fabric_contracts::{Budget, Classification, Route};
use fabric_token::spend::{
    Reservation, ReservationError, ReservationKey, ReservationLedger, Spent,
};
use mai_compliance::PhiDetector;
use mai_router::{DefaultRouter, Router};
use serde_json::json;

use crate::meter::{PriceBook, ReceiptLedger};
use crate::policy::{ModeOutcome, PolicyDecision, PolicyEngine, PolicyMode, Profile};
use crate::provider::{
    ChunkStream, CompletionRequest, CompletionResponse, Provider, ProviderError, Registry, Usage,
};
use crate::route::GatewayRoute;
use crate::{Gateway, GatewayError, ResolvedContext};

/// Where a requested model is dispatched: which registered provider, under what
/// upstream model id.
#[derive(Debug, Clone)]
pub struct Target {
    /// The [`crate::provider::Provider::name`] to dispatch to.
    pub provider: String,
    /// The upstream model id the provider should be called with.
    pub model: String,
}

impl Target {
    pub fn new(provider: impl Into<String>, model: impl Into<String>) -> Self {
        Self {
            provider: provider.into(),
            model: model.into(),
        }
    }
}

/// Model-alias → [`Target`] routing, with an optional default for unmapped ids.
#[derive(Debug, Clone, Default)]
pub struct ModelMap {
    map: HashMap<String, Target>,
    default: Option<Target>,
}

impl ModelMap {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Route an inbound model id to a provider target.
    #[must_use]
    pub fn route(mut self, inbound: impl Into<String>, target: Target) -> Self {
        self.map.insert(inbound.into(), target);
        self
    }

    /// Set the fallback target for any unmapped model id.
    #[must_use]
    pub fn default_target(mut self, target: Target) -> Self {
        self.default = Some(target);
        self
    }

    /// Resolve an inbound model to its target (explicit mapping, else default).
    #[must_use]
    pub fn resolve(&self, model: &str) -> Option<&Target> {
        self.map.get(model).or(self.default.as_ref())
    }

    /// The explicitly-mapped model ids, sorted (for `/v1/models`).
    #[must_use]
    pub fn model_ids(&self) -> Vec<String> {
        let mut ids: Vec<String> = self.map.keys().cloned().collect();
        ids.sort();
        ids
    }
}

/// Everything an inference surface needs: auth (Gateway), dispatch (Registry),
/// model routing (ModelMap), and classify-and-route (`mai-router`). Cheap to
/// clone (all `Arc`).
#[derive(Clone)]
pub struct AppState {
    pub gateway: Arc<Gateway>,
    pub registry: Arc<Registry>,
    pub models: Arc<ModelMap>,
    /// The G5 classify-and-route engine. Defaults to `mai-router`'s `DefaultRouter`.
    pub router: Arc<dyn Router + Send + Sync>,
    /// The G6 deny-wins policy engine (mai-compliance).
    pub policy: Arc<PolicyEngine>,
    /// The G6 enforcement posture. Defaults to `Enforce` (fail-closed).
    pub mode: PolicyMode,
    /// The deployment profile, surfaced in readiness/audit. Defaults to `Production`.
    pub profile: Profile,
    /// The G7 append-only receipt ledger (BLAKE3 chain over metadata-only receipts).
    pub receipts: Arc<Mutex<ReceiptLedger>>,
    /// The G7 cost model.
    pub prices: Arc<PriceBook>,
    /// The G8 PHI/PII detector — finds the sensitive spans tokenized on cloud egress.
    pub detector: Arc<PhiDetector>,
    /// Atomic reserve/commit/release barrier shared by every inference surface.
    pub reservations: Arc<ReservationLedger>,
}

impl AppState {
    /// Assemble state with the default `mai-router` engine, the baseline policy
    /// engine, `Enforce` mode + `Production` profile (the fail-closed default —
    /// callers opt into a non-blocking mode explicitly via [`Self::with_mode`]),
    /// a fresh receipt ledger, and the baseline price book.
    #[must_use]
    pub fn new(gateway: Arc<Gateway>, registry: Arc<Registry>, models: Arc<ModelMap>) -> Self {
        Self {
            gateway,
            registry,
            models,
            router: Arc::new(DefaultRouter::with_defaults()),
            policy: Arc::new(PolicyEngine::baseline()),
            mode: PolicyMode::Enforce,
            profile: Profile::Production,
            receipts: Arc::new(Mutex::new(ReceiptLedger::new())),
            prices: Arc::new(PriceBook::baseline()),
            detector: Arc::new(PhiDetector::baseline()),
            reservations: Arc::new(ReservationLedger::new()),
        }
    }

    /// Override the classify-and-route engine (tests / custom policy).
    #[must_use]
    pub fn with_router(mut self, router: Arc<dyn Router + Send + Sync>) -> Self {
        self.router = router;
        self
    }

    /// Set the enforcement posture (shadow / report-only / enforce).
    #[must_use]
    pub fn with_mode(mut self, mode: PolicyMode) -> Self {
        self.mode = mode;
        self
    }

    /// Set the deployment profile (informational; surfaced in readiness/audit).
    #[must_use]
    pub fn with_profile(mut self, profile: Profile) -> Self {
        self.profile = profile;
        self
    }

    /// Inject a shared reservation ledger for tests or a process-wide owner.
    #[must_use]
    pub fn with_reservations(mut self, reservations: Arc<ReservationLedger>) -> Self {
        self.reservations = reservations;
        self
    }
}

/// Extract the virtual key from either client convention: Anthropic SDKs send
/// `x-api-key`, OpenAI SDKs send `Authorization: Bearer`. Accepting both lets a
/// single gateway front either surface with the caller's native header.
fn extract_key(headers: &HeaderMap) -> Result<&str, (StatusCode, String)> {
    if let Some(k) = headers.get("x-api-key").and_then(|v| v.to_str().ok()) {
        let k = k.trim();
        if !k.is_empty() {
            return Ok(k);
        }
    }
    crate::http::bearer_key(headers)
}

/// Authorize a request: extract the virtual key (Bearer or `x-api-key`), resolve
/// and verify its trust token, and run the pre-flight budget check (all of G1).
/// Returns the verified [`ResolvedContext`] or an HTTP error tuple.
pub(crate) async fn authorize(
    state: &AppState,
    headers: &HeaderMap,
) -> Result<ResolvedContext, (StatusCode, String)> {
    let key = extract_key(headers)?;
    state
        .gateway
        .resolve_and_check(key, Utc::now())
        .await
        .map_err(|e| crate::http::to_http(&e))
}

/// A final, immutable authorization decision carried to the provider sink.
///
/// All fields are private so a protocol surface cannot swap the verified token,
/// provider, upstream model, route, or policy result after authorization. The
/// only provider-call methods overwrite the request model with the frozen target.
pub(crate) struct AuthorizedDispatch {
    ctx: ResolvedContext,
    gateway: Arc<Gateway>,
    inbound_model: String,
    target: Target,
    provider: Arc<dyn Provider>,
    target_cloud: bool,
    route: GatewayRoute,
    policy: PolicyDecision,
    outcome: ModeOutcome,
    reservation: Option<DispatchReservation>,
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum DispatchError {
    #[error(transparent)]
    Revocation(#[from] GatewayError),
    #[error(transparent)]
    Provider(#[from] ProviderError),
}

/// Authority reserved before a provider side effect. Streaming requests move
/// this value into their SSE meter so cancellation settles exactly once.
pub(crate) struct DispatchReservation {
    reservation: Reservation,
    cap: Budget,
}

impl DispatchReservation {
    pub(crate) fn commit_usage(self, usage: Spent) -> Result<(), ReservationError> {
        self.reservation.commit_usage(&self.cap, usage)
    }
}

const DEFAULT_OUTPUT_RESERVATION: u32 = 4096;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
enum DecisionError {
    #[error("the router denied this request")]
    RouterDenied,
    #[error("model '{0}' is not authorized by the trust token")]
    ModelDenied(String),
    #[error("the resolved provider locality is not authorized by the trust token")]
    RouteDenied,
    #[error("request classification '{actual}' exceeds token ceiling '{ceiling}'")]
    ClassificationDenied { actual: String, ceiling: String },
    #[error("request classification '{0}' is not recognized")]
    UnknownClassification(String),
    #[error("resolved provider identity does not match its registered target")]
    ProviderMismatch,
}

impl DecisionError {
    fn code(&self) -> &'static str {
        match self {
            Self::RouterDenied => "aog_router_denied",
            Self::ModelDenied(_) => "aog_model_denied",
            Self::RouteDenied => "aog_route_denied",
            Self::ClassificationDenied { .. } => "aog_classification_denied",
            Self::UnknownClassification(_) => "aog_classification_unknown",
            Self::ProviderMismatch => "aog_provider_mismatch",
        }
    }

    fn into_response(self) -> Response {
        let body = json!({
            "error": {
                "message": self.to_string(),
                "type": "authorization_denied",
                "code": self.code(),
            }
        });
        (StatusCode::FORBIDDEN, Json(body)).into_response()
    }
}

pub(crate) fn reservation_http(error: &ReservationError) -> Response {
    let body = json!({
        "error": {
            "message": error.to_string(),
            "type": "budget_exhausted",
            "code": "aog_budget_reconciliation_denied",
        }
    });
    (StatusCode::PAYMENT_REQUIRED, Json(body)).into_response()
}

fn classification_level(name: &str) -> Option<Classification> {
    match name {
        "public" => Some(Classification::Public),
        "internal" => Some(Classification::Internal),
        // The router's sensitive and regulated tiers both fall within the WSF
        // Restricted capability used by current tenant issuance policy. ITAR /
        // national-security material is elevated to Controlled/Secret by its
        // dedicated policy controls or the router's terminal Critical decision.
        "sensitive" | "regulated" | "restricted" => Some(Classification::Restricted),
        "controlled" => Some(Classification::Controlled),
        "critical" | "secret" => Some(Classification::Secret),
        _ => None,
    }
}

fn route_authorizes_target(allowed: &[Route], target_cloud: bool) -> bool {
    if allowed.is_empty() {
        // Omitted deserializes to empty. Neither form is authority: production
        // issuance resolves caller omission to a tenant allowlist, so seeing an
        // empty signed result here is malformed/legacy authority and fails closed.
        return false;
    }
    if target_cloud {
        allowed.contains(&Route::CloudAllowed) || allowed.contains(&Route::LocalPreferred)
    } else {
        allowed.contains(&Route::LocalOnly) || allowed.contains(&Route::LocalPreferred)
    }
}

fn validate_final_decision(
    ctx: &ResolvedContext,
    inbound_model: &str,
    target: &Target,
    route: &GatewayRoute,
) -> Result<(), DecisionError> {
    if route.denied {
        return Err(DecisionError::RouterDenied);
    }

    if !ctx
        .token
        .allowed_models
        .iter()
        .any(|model| model == inbound_model)
    {
        return Err(DecisionError::ModelDenied(inbound_model.to_string()));
    }

    let target_cloud = crate::policy::target_is_cloud(target);
    if !route_authorizes_target(&ctx.token.allowed_routes, target_cloud) {
        return Err(DecisionError::RouteDenied);
    }

    let actual = classification_level(&route.classification)
        .ok_or_else(|| DecisionError::UnknownClassification(route.classification.clone()))?;
    if actual > ctx.token.max_data_classification {
        return Err(DecisionError::ClassificationDenied {
            actual: route.classification.clone(),
            ceiling: format!("{:?}", ctx.token.max_data_classification).to_lowercase(),
        });
    }
    Ok(())
}

impl AuthorizedDispatch {
    // Every argument is a frozen authorization input; grouping them into a
    // caller-constructible bag would weaken the boundary this type enforces.
    #[allow(clippy::too_many_arguments)]
    fn new(
        ctx: ResolvedContext,
        gateway: Arc<Gateway>,
        inbound_model: String,
        target: Target,
        provider: Arc<dyn Provider>,
        route: GatewayRoute,
        policy: PolicyDecision,
        outcome: ModeOutcome,
    ) -> Result<Self, DecisionError> {
        validate_final_decision(&ctx, &inbound_model, &target, &route)?;
        if provider.name() != target.provider {
            return Err(DecisionError::ProviderMismatch);
        }
        let target_cloud = crate::policy::target_is_cloud(&target);
        Ok(Self {
            ctx,
            gateway,
            inbound_model,
            target,
            provider,
            target_cloud,
            route,
            policy,
            outcome,
            reservation: None,
        })
    }

    fn reserve_budget(
        &mut self,
        ledger: &ReservationLedger,
        prices: &PriceBook,
        query: &str,
        max_output_tokens: Option<u32>,
    ) -> Result<(), ReservationError> {
        let Some(cap) = self.ctx.token.budget.clone() else {
            return Ok(());
        };
        let input = crate::route::estimate_tokens(None, query);
        let output = max_output_tokens.unwrap_or(DEFAULT_OUTPUT_RESERVATION);
        let usage = Spent {
            tokens: u64::from(input).saturating_add(u64::from(output)),
            usd_cents: prices.cost(&self.target.provider, &self.inbound_model, input, output),
            tool_calls: 1,
        };
        let key = ReservationKey::for_token(&self.ctx.token, None, Some("aog-gateway".to_string()));
        self.reservation = Some(DispatchReservation {
            reservation: ledger.reserve(key, &cap, usage)?,
            cap,
        });
        Ok(())
    }

    /// Reconcile a completed non-stream request to its final usage.
    pub(crate) fn commit_usage(
        &mut self,
        prices: &PriceBook,
        usage: Usage,
    ) -> Result<(), ReservationError> {
        let Some(reservation) = self.reservation.take() else {
            return Ok(());
        };
        reservation.commit_usage(Spent {
            tokens: u64::from(usage.input_tokens).saturating_add(u64::from(usage.output_tokens)),
            usd_cents: prices.cost(
                &self.target.provider,
                &self.inbound_model,
                usage.input_tokens,
                usage.output_tokens,
            ),
            tool_calls: 1,
        })
    }

    /// Move pending authority into the SSE meter. Provider setup failure before
    /// this call releases automatically when the dispatch is dropped.
    pub(crate) fn take_reservation(&mut self) -> Option<DispatchReservation> {
        self.reservation.take()
    }

    pub(crate) fn context(&self) -> &ResolvedContext {
        &self.ctx
    }

    pub(crate) fn inbound_model(&self) -> &str {
        &self.inbound_model
    }

    pub(crate) fn provider_name(&self) -> &str {
        &self.target.provider
    }

    pub(crate) fn target_model(&self) -> &str {
        &self.target.model
    }

    pub(crate) fn target_is_cloud(&self) -> bool {
        self.target_cloud
    }

    pub(crate) fn route(&self) -> &GatewayRoute {
        &self.route
    }

    pub(crate) fn policy(&self) -> &PolicyDecision {
        &self.policy
    }

    pub(crate) fn outcome(&self) -> ModeOutcome {
        self.outcome
    }

    pub(crate) async fn complete(
        &self,
        request: &CompletionRequest,
    ) -> Result<CompletionResponse, DispatchError> {
        self.gateway
            .authorize_current(&self.ctx.token, Utc::now())
            .await?;
        let mut frozen = request.clone();
        frozen.model.clone_from(&self.target.model);
        Ok(self.provider.complete(&frozen).await?)
    }

    pub(crate) async fn stream(
        &self,
        request: &CompletionRequest,
    ) -> Result<ChunkStream, DispatchError> {
        self.gateway
            .authorize_current(&self.ctx.token, Utc::now())
            .await?;
        let mut frozen = request.clone();
        frozen.model.clone_from(&self.target.model);
        Ok(self.provider.stream(&frozen).await?)
    }
}

/// Resolve every authorization input once and return the immutable decision
/// consumed by provider execution. Router denies and signed token caveats are
/// terminal in every mode, including development shadow/report-only modes.
#[allow(clippy::result_large_err)]
pub(crate) async fn authorize_dispatch(
    state: &AppState,
    headers: &HeaderMap,
    inbound_model: &str,
    query: &str,
    max_output_tokens: Option<u32>,
) -> Result<AuthorizedDispatch, Response> {
    let ctx = authorize(state, headers)
        .await
        .map_err(IntoResponse::into_response)?;
    let target = state
        .models
        .resolve(inbound_model)
        .cloned()
        .ok_or_else(|| {
            (
                StatusCode::NOT_FOUND,
                format!("unknown model: {inbound_model}"),
            )
                .into_response()
        })?;
    let provider = state.registry.get(&target.provider).ok_or_else(|| {
        (
            StatusCode::BAD_GATEWAY,
            format!("provider not registered: {}", target.provider),
        )
            .into_response()
    })?;
    let route = crate::route::classify_and_route(
        state.router.as_ref(),
        query,
        crate::route::estimate_tokens(max_output_tokens, query),
        &ctx.tenant_id,
        ctx.token.roles.first().map_or("user", String::as_str),
    );
    let target_cloud = crate::policy::target_is_cloud(&target);
    let policy = state.policy.evaluate(query, &route);
    let outcome = crate::policy::apply_mode(&policy, state.mode, target_cloud);

    let mut decision = AuthorizedDispatch::new(
        ctx,
        state.gateway.clone(),
        inbound_model.to_string(),
        target,
        provider,
        route,
        policy,
        outcome,
    )
    .map_err(DecisionError::into_response)?;
    if decision.outcome.block {
        return Err(crate::policy::blocked(&decision.policy, state.mode));
    }
    decision
        .reserve_budget(
            state.reservations.as_ref(),
            state.prices.as_ref(),
            query,
            max_output_tokens,
        )
        .map_err(|error| {
            let body = json!({
                "error": {
                    "message": error.to_string(),
                    "type": "budget_exhausted",
                    "code": "aog_budget_reservation_denied",
                }
            });
            (StatusCode::PAYMENT_REQUIRED, Json(body)).into_response()
        })?;
    Ok(decision)
}

#[cfg(test)]
mod tests {
    use std::sync::RwLock;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use async_trait::async_trait;
    use fabric_contracts::{Attenuation, RevocationStatus, Signature, TrustToken};
    use fabric_crypto::Signer;
    use fabric_crypto::providers::{MlDsa87Verifier, RustCryptoMlDsa87};
    use fabric_revocation::{MonotonicRevocationStore, RevocationSnapshot};

    use super::*;
    use crate::provider::{ChatMessage, Role, Usage};
    use crate::route::RouteSource;

    fn context(models: Vec<&str>, routes: Vec<Route>, ceiling: Classification) -> ResolvedContext {
        ResolvedContext {
            tenant_id: "tenant-a".into(),
            token: TrustToken {
                token_id: "token-a".into(),
                issued_at: "2026-07-16T00:00:00Z".into(),
                expires_at: "2099-01-01T00:00:00Z".into(),
                issuer: "wsf".into(),
                trust_bundle_version: "v1".into(),
                tenant_id: "tenant-a".into(),
                subject_id: Some("subject-a".into()),
                subject_hash: "hash-a".into(),
                service_identity: None,
                identity_id: None,
                roles: vec!["user".into()],
                compliance_scopes: vec![],
                allowed_routes: routes,
                allowed_models: models.into_iter().map(str::to_string).collect(),
                max_data_classification: ceiling,
                country: None,
                person_type: None,
                offline_mode: false,
                revocation_status: RevocationStatus::Unknown,
                budget: None,
                attenuation: Attenuation::default(),
                signature: Signature {
                    alg: "ML-DSA-87".into(),
                    key_id: "anchor".into(),
                    value: "sig".into(),
                },
            },
        }
    }

    fn gateway_route(route: Route, classification: &str, denied: bool) -> GatewayRoute {
        GatewayRoute {
            route,
            classification: classification.into(),
            reason: "test decision".into(),
            source: RouteSource::Classified,
            denied,
        }
    }

    fn offline_gateway() -> Arc<Gateway> {
        Arc::new(Gateway::new(
            wsf_bridge::OpenBaoAuth::new(wsf_bridge::OpenBaoConfig::new(
                "http://127.0.0.1:1",
                "role",
                "secret",
            ))
            .unwrap(),
            crate::GatewayConfig {
                token_public_key: vec![],
                virtual_key_kv_prefix: "kv/data/test".into(),
            },
        ))
    }

    fn required_test_gateway(
        signer: &RustCryptoMlDsa87,
        store: Arc<RwLock<MonotonicRevocationStore>>,
    ) -> Arc<Gateway> {
        Arc::new(
            Gateway::new(
                wsf_bridge::OpenBaoAuth::new(wsf_bridge::OpenBaoConfig::new(
                    "http://127.0.0.1:1",
                    "role",
                    "secret",
                ))
                .unwrap(),
                crate::GatewayConfig {
                    token_public_key: signer.public_key().to_vec(),
                    virtual_key_kv_prefix: "kv/data/test".into(),
                },
            )
            .with_test_revocation_store(store),
        )
    }

    #[test]
    fn terminal_router_deny_is_invariant_across_locality_and_route() {
        let ctx = context(
            vec!["alias"],
            vec![Route::LocalOnly, Route::CloudAllowed],
            Classification::Secret,
        );
        for provider in ["local", "openai"] {
            for route in [Route::LocalOnly, Route::LocalPreferred, Route::CloudAllowed] {
                let result = validate_final_decision(
                    &ctx,
                    "alias",
                    &Target::new(provider, "upstream"),
                    &gateway_route(route, "public", true),
                );
                assert_eq!(
                    result,
                    Err(DecisionError::RouterDenied),
                    "a target transform to {provider}/{route:?} must not revive a deny"
                );
            }
        }
    }

    #[test]
    fn excluded_model_cannot_be_revived_by_alias_target_or_locality() {
        let ctx = context(
            vec!["authorized-alias"],
            vec![Route::LocalOnly, Route::CloudAllowed],
            Classification::Restricted,
        );
        for target in [
            Target::new("local", "authorized-alias"),
            Target::new("openai", "authorized-alias"),
            Target::new("local", "blocked-alias"),
        ] {
            assert_eq!(
                validate_final_decision(
                    &ctx,
                    "blocked-alias",
                    &target,
                    &gateway_route(Route::CloudAllowed, "public", false),
                ),
                Err(DecisionError::ModelDenied("blocked-alias".into()))
            );
        }
    }

    #[test]
    fn signed_route_and_classification_caveats_are_terminal() {
        let cloud_only = context(
            vec!["alias"],
            vec![Route::CloudAllowed],
            Classification::Internal,
        );
        assert_eq!(
            validate_final_decision(
                &cloud_only,
                "alias",
                &Target::new("local", "upstream"),
                &gateway_route(Route::LocalOnly, "public", false),
            ),
            Err(DecisionError::RouteDenied)
        );
        assert_eq!(
            validate_final_decision(
                &cloud_only,
                "alias",
                &Target::new("openai", "upstream"),
                &gateway_route(Route::CloudAllowed, "regulated", false),
            ),
            Err(DecisionError::ClassificationDenied {
                actual: "regulated".into(),
                ceiling: "internal".into(),
            })
        );
    }

    #[test]
    fn generated_five_surface_model_route_caveat_matrix() {
        let surfaces = [
            "openai_chat",
            "openai_stream",
            "openai_legacy",
            "anthropic_message",
            "anthropic_stream",
        ];
        let cloud_target = Target::new("openai", "upstream-model");
        let public_cloud = gateway_route(Route::CloudAllowed, "public", false);
        let mut instance_regressions = 0usize;

        for surface in surfaces {
            for models in [vec![], vec!["different-model"]] {
                assert_eq!(
                    validate_final_decision(
                        &context(
                            models,
                            vec![Route::CloudAllowed],
                            Classification::Restricted,
                        ),
                        "authorized-alias",
                        &cloud_target,
                        &public_cloud,
                    ),
                    Err(DecisionError::ModelDenied("authorized-alias".into())),
                    "{surface}: omitted/empty or excluding model caveat must deny"
                );
            }
            instance_regressions += 1;

            for routes in [vec![], vec![Route::LocalOnly]] {
                assert_eq!(
                    validate_final_decision(
                        &context(
                            vec!["authorized-alias", "other-allowed-alias"],
                            routes,
                            Classification::Restricted,
                        ),
                        "authorized-alias",
                        &cloud_target,
                        &public_cloud,
                    ),
                    Err(DecisionError::RouteDenied),
                    "{surface}: omitted/empty or excluding route caveat must deny"
                );
            }
            instance_regressions += 1;

            assert!(
                validate_final_decision(
                    &context(
                        vec!["authorized-alias", "other-allowed-alias"],
                        vec![Route::LocalOnly, Route::CloudAllowed],
                        Classification::Restricted,
                    ),
                    "authorized-alias",
                    &cloud_target,
                    &public_cloud,
                )
                .is_ok(),
                "{surface}: a valid subset request must retain compatibility"
            );
        }
        assert_eq!(instance_regressions, 10, "five surfaces x two caveat axes");
    }

    #[test]
    fn omitted_caveats_deserialize_to_fail_closed_empty_authority() {
        let mut value = serde_json::to_value(
            context(
                vec!["authorized-alias"],
                vec![Route::CloudAllowed],
                Classification::Restricted,
            )
            .token,
        )
        .unwrap();
        let object = value.as_object_mut().unwrap();
        object.remove("allowed_models");
        object.remove("allowed_routes");
        let omitted: TrustToken = serde_json::from_value(value).unwrap();
        assert!(omitted.allowed_models.is_empty());
        assert!(omitted.allowed_routes.is_empty());
        assert_eq!(
            validate_final_decision(
                &ResolvedContext {
                    tenant_id: omitted.tenant_id.clone(),
                    token: omitted,
                },
                "authorized-alias",
                &Target::new("openai", "upstream-model"),
                &gateway_route(Route::CloudAllowed, "public", false),
            ),
            Err(DecisionError::ModelDenied("authorized-alias".into()))
        );
    }

    struct RecordingProvider {
        seen_models: Arc<Mutex<Vec<String>>>,
    }

    #[async_trait]
    impl Provider for RecordingProvider {
        fn name(&self) -> &str {
            "local"
        }

        async fn complete(
            &self,
            req: &CompletionRequest,
        ) -> Result<CompletionResponse, ProviderError> {
            self.seen_models
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(req.model.clone());
            Ok(CompletionResponse {
                model: req.model.clone(),
                content: "ok".into(),
                usage: Usage::default(),
                finish_reason: "stop".into(),
            })
        }

        async fn stream(&self, _req: &CompletionRequest) -> Result<ChunkStream, ProviderError> {
            Ok(Box::pin(futures::stream::empty()))
        }
    }

    struct NamedProvider(&'static str);

    #[async_trait]
    impl Provider for NamedProvider {
        fn name(&self) -> &str {
            self.0
        }

        async fn complete(
            &self,
            req: &CompletionRequest,
        ) -> Result<CompletionResponse, ProviderError> {
            Ok(CompletionResponse {
                model: req.model.clone(),
                content: "ok".into(),
                usage: Usage {
                    input_tokens: 1000,
                    output_tokens: 1000,
                },
                finish_reason: "stop".into(),
            })
        }

        async fn stream(&self, _req: &CompletionRequest) -> Result<ChunkStream, ProviderError> {
            Ok(Box::pin(futures::stream::empty()))
        }
    }

    struct CountingProvider {
        calls: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl Provider for CountingProvider {
        fn name(&self) -> &str {
            "openai"
        }

        async fn complete(
            &self,
            req: &CompletionRequest,
        ) -> Result<CompletionResponse, ProviderError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(CompletionResponse {
                model: req.model.clone(),
                content: "unexpected".into(),
                usage: Usage::default(),
                finish_reason: "stop".into(),
            })
        }

        async fn stream(&self, _req: &CompletionRequest) -> Result<ChunkStream, ProviderError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(Box::pin(futures::stream::empty()))
        }
    }

    #[tokio::test]
    async fn every_revocation_dimension_stops_all_five_surfaces_before_provider() {
        let signer = RustCryptoMlDsa87::generate("g4-revocation-anchor").unwrap();
        let now = Utc::now();
        let baseline = fabric_revocation::sign(
            RevocationSnapshot::new(
                "baseline",
                (now - chrono::Duration::minutes(1)).to_rfc3339(),
                (now + chrono::Duration::hours(1)).to_rfc3339(),
            )
            .with_sequence(1),
            &signer,
        )
        .unwrap();
        let surfaces = [
            ("openai_chat", false),
            ("openai_stream", true),
            ("openai_legacy", false),
            ("anthropic_message", false),
            ("anthropic_stream", true),
        ];
        let dimensions = [
            "token_id",
            "subject_hash",
            "signing_key",
            "issuer",
            "bundle_version",
            "tenant",
            "service_identity",
        ];
        let calls = Arc::new(AtomicUsize::new(0));
        let request = CompletionRequest {
            model: "gpt-4o-mini".into(),
            messages: vec![ChatMessage::user("hello")],
            max_tokens: Some(8),
            temperature: None,
        };
        let mut regressions = 0usize;

        for (surface, streaming) in surfaces {
            for dimension in dimensions {
                let store = Arc::new(RwLock::new(MonotonicRevocationStore::new()));
                store
                    .write()
                    .unwrap()
                    .advance(baseline.clone(), &MlDsa87Verifier, signer.public_key())
                    .unwrap();
                let gateway = required_test_gateway(&signer, store.clone());
                let mut ctx = context(
                    vec!["gpt-4o-mini"],
                    vec![Route::CloudAllowed],
                    Classification::Restricted,
                );
                ctx.token.service_identity = Some("service-a".into());
                let dispatch = AuthorizedDispatch::new(
                    ctx,
                    gateway,
                    "gpt-4o-mini".into(),
                    Target::new("openai", "gpt-4o-mini"),
                    Arc::new(CountingProvider {
                        calls: calls.clone(),
                    }),
                    gateway_route(Route::CloudAllowed, "public", false),
                    PolicyDecision {
                        allowed_cloud: true,
                        phi_detected: false,
                        effective_route: Route::CloudAllowed,
                        reasons: vec![],
                    },
                    ModeOutcome {
                        block: false,
                        report: false,
                    },
                )
                .unwrap();

                let mut revoked = RevocationSnapshot::new(
                    format!("revoked-{surface}-{dimension}"),
                    (now - chrono::Duration::minutes(1)).to_rfc3339(),
                    (now + chrono::Duration::hours(1)).to_rfc3339(),
                )
                .with_sequence(2);
                match dimension {
                    "token_id" => revoked.revoked_tokens.push("token-a".into()),
                    "subject_hash" => revoked.revoked_subjects.push("hash-a".into()),
                    "signing_key" => revoked.revoked_signing_keys.push("anchor".into()),
                    "issuer" => revoked.revoked_issuers.push("wsf".into()),
                    "bundle_version" => revoked.revoked_bundle_versions.push("v1".into()),
                    "tenant" => revoked.revoked_tenants.push("tenant-a".into()),
                    "service_identity" => {
                        revoked.revoked_service_identities.push("service-a".into());
                    }
                    _ => unreachable!(),
                }
                store
                    .write()
                    .unwrap()
                    .advance(
                        fabric_revocation::sign(revoked, &signer).unwrap(),
                        &MlDsa87Verifier,
                        signer.public_key(),
                    )
                    .unwrap();

                let denied = if streaming {
                    matches!(
                        dispatch.stream(&request).await,
                        Err(DispatchError::Revocation(GatewayError::Revoked))
                    )
                } else {
                    matches!(
                        dispatch.complete(&request).await,
                        Err(DispatchError::Revocation(GatewayError::Revoked))
                    )
                };
                assert!(denied, "{surface}/{dimension} was not denied");
                regressions += 1;
            }
        }

        assert_eq!(regressions, 35, "five surfaces x seven dimensions");
        assert_eq!(
            calls.load(Ordering::SeqCst),
            0,
            "provider was never reached"
        );
    }

    #[tokio::test]
    async fn stream_continuation_observes_a_new_revocation_sequence() {
        let signer = RustCryptoMlDsa87::generate("g4-stream-anchor").unwrap();
        let now = Utc::now();
        let store = Arc::new(RwLock::new(MonotonicRevocationStore::new()));
        let baseline = fabric_revocation::sign(
            RevocationSnapshot::new(
                "baseline",
                (now - chrono::Duration::minutes(1)).to_rfc3339(),
                (now + chrono::Duration::hours(1)).to_rfc3339(),
            )
            .with_sequence(1),
            &signer,
        )
        .unwrap();
        store
            .write()
            .unwrap()
            .advance(baseline, &MlDsa87Verifier, signer.public_key())
            .unwrap();
        let gateway = required_test_gateway(&signer, store.clone());
        let ctx = context(
            vec!["gpt-4o-mini"],
            vec![Route::CloudAllowed],
            Classification::Restricted,
        );
        let meter = crate::meter::StreamMeter {
            receipts: Arc::new(Mutex::new(crate::meter::ReceiptLedger::new())),
            prices: Arc::new(PriceBook::baseline()),
            gateway,
            ctx,
            provider: "openai".into(),
            model: "gpt-4o-mini".into(),
            route: gateway_route(Route::CloudAllowed, "public", false),
            allowed_cloud: true,
            workflow_id: None,
            input_estimate: 1,
            reported: Usage::default(),
            delta_chars: 0,
            reservation: None,
        };
        assert!(meter.authorize_continuation().await.is_ok());

        let mut revoked = RevocationSnapshot::new(
            "revoked",
            (now - chrono::Duration::minutes(1)).to_rfc3339(),
            (now + chrono::Duration::hours(1)).to_rfc3339(),
        )
        .with_sequence(2);
        revoked.revoked_tenants.push("tenant-a".into());
        store
            .write()
            .unwrap()
            .advance(
                fabric_revocation::sign(revoked, &signer).unwrap(),
                &MlDsa87Verifier,
                signer.public_key(),
            )
            .unwrap();
        assert!(matches!(
            meter.authorize_continuation().await,
            Err(GatewayError::Revoked)
        ));
    }

    fn budgeted_dispatch(
        budget: Budget,
        reservations: &ReservationLedger,
        prices: &PriceBook,
    ) -> Result<AuthorizedDispatch, ReservationError> {
        let mut ctx = context(
            vec!["gpt-4o-mini"],
            vec![Route::CloudAllowed],
            Classification::Restricted,
        );
        ctx.token.budget = Some(budget);
        let mut dispatch = AuthorizedDispatch::new(
            ctx,
            offline_gateway(),
            "gpt-4o-mini".into(),
            Target::new("openai", "gpt-4o-mini"),
            Arc::new(NamedProvider("openai")),
            gateway_route(Route::CloudAllowed, "public", false),
            PolicyDecision {
                allowed_cloud: true,
                phi_detected: false,
                effective_route: Route::CloudAllowed,
                reasons: vec![],
            },
            ModeOutcome {
                block: false,
                report: false,
            },
        )
        .unwrap();
        dispatch.reserve_budget(reservations, prices, &"x".repeat(4000), Some(1000))?;
        Ok(dispatch)
    }

    #[test]
    fn atomic_gateway_spend_caps_mixed_stream_and_non_stream_concurrency() {
        let reservations = Arc::new(ReservationLedger::new());
        let prices = Arc::new(PriceBook::baseline());
        let cap = Budget {
            token_cap: 10_000,
            usd_cap_cents: 375,
            tool_call_cap: 5,
            ..Budget::default()
        };
        let barrier = Arc::new(std::sync::Barrier::new(100));
        let mut workers = Vec::new();
        for worker_id in 0..100 {
            let reservations = reservations.clone();
            let prices = prices.clone();
            let cap = cap.clone();
            let barrier = barrier.clone();
            workers.push(std::thread::spawn(move || {
                barrier.wait();
                let Ok(mut dispatch) =
                    budgeted_dispatch(cap, reservations.as_ref(), prices.as_ref())
                else {
                    return false;
                };
                if worker_id % 2 == 0 {
                    // Non-stream settlement path.
                    dispatch
                        .commit_usage(
                            prices.as_ref(),
                            Usage {
                                input_tokens: 1000,
                                output_tokens: 1000,
                            },
                        )
                        .is_ok()
                } else {
                    // Stream path moves the reservation into the SSE guard.
                    dispatch
                        .take_reservation()
                        .unwrap()
                        .commit_usage(Spent {
                            tokens: 2000,
                            usd_cents: 75,
                            tool_calls: 1,
                        })
                        .is_ok()
                }
            }));
        }
        let admitted = workers
            .into_iter()
            .map(|worker| worker.join().unwrap())
            .filter(|admitted| *admitted)
            .count();
        assert_eq!(admitted, 5, "all three ceilings admit exactly five calls");

        let key = ReservationKey::for_token(
            &context(
                vec!["gpt-4o-mini"],
                vec![Route::CloudAllowed],
                Classification::Restricted,
            )
            .token,
            None,
            Some("aog-gateway".to_string()),
        );
        assert_eq!(
            reservations.committed(&key),
            Spent {
                tokens: 10_000,
                usd_cents: 375,
                tool_calls: 5,
            }
        );
    }

    #[test]
    fn failure_releases_and_stream_cancellation_settles_once() {
        let reservations = ReservationLedger::new();
        let prices = PriceBook::baseline();
        let cap = Budget {
            token_cap: 4000,
            usd_cap_cents: 150,
            tool_call_cap: 1,
            ..Budget::default()
        };

        // Provider setup failure: dropping the dispatch releases all capacity.
        drop(budgeted_dispatch(cap.clone(), &reservations, &prices).unwrap());
        let mut stream = budgeted_dispatch(cap, &reservations, &prices).unwrap();
        let reservation = stream.take_reservation().unwrap();
        drop(stream);
        // Client cancellation after stream start commits observed/fallback use.
        reservation
            .commit_usage(Spent {
                tokens: 500,
                usd_cents: 1,
                tool_calls: 1,
            })
            .unwrap();

        assert!(
            matches!(
                budgeted_dispatch(
                    Budget {
                        token_cap: 4000,
                        usd_cap_cents: 150,
                        tool_call_cap: 1,
                        ..Budget::default()
                    },
                    &reservations,
                    &prices,
                ),
                Err(ReservationError::Exhausted)
            ),
            "the cancelled stream charged one call exactly once"
        );
    }

    #[tokio::test]
    async fn authorized_dispatch_freezes_provider_model_at_the_sink() {
        let seen_models = Arc::new(Mutex::new(Vec::new()));
        let provider: Arc<dyn Provider> = Arc::new(RecordingProvider {
            seen_models: seen_models.clone(),
        });
        let route = gateway_route(Route::LocalOnly, "public", false);
        let policy = PolicyDecision {
            allowed_cloud: true,
            phi_detected: false,
            effective_route: Route::LocalOnly,
            reasons: vec![],
        };
        let decision = AuthorizedDispatch::new(
            context(
                vec!["public-alias"],
                vec![Route::LocalOnly],
                Classification::Restricted,
            ),
            offline_gateway(),
            "public-alias".into(),
            Target::new("local", "frozen-upstream"),
            provider,
            route,
            policy,
            ModeOutcome {
                block: false,
                report: false,
            },
        )
        .expect("authorized final decision");

        let caller_mutated = CompletionRequest {
            model: "attacker-change-after-authorization".into(),
            messages: vec![ChatMessage {
                role: Role::User,
                content: "hello".into(),
            }],
            max_tokens: Some(8),
            temperature: None,
        };
        decision.complete(&caller_mutated).await.unwrap();
        assert_eq!(
            *seen_models.lock().unwrap_or_else(|e| e.into_inner()),
            vec!["frozen-upstream".to_string()]
        );
    }

    #[test]
    fn model_map_routes_and_defaults() {
        let m = ModelMap::new()
            .route("gpt-4o", Target::new("openai", "gpt-4o"))
            .default_target(Target::new("local", "llama3"));
        assert_eq!(m.resolve("gpt-4o").unwrap().provider, "openai");
        // unmapped → default.
        assert_eq!(m.resolve("mystery").unwrap().provider, "local");
        assert_eq!(m.model_ids(), vec!["gpt-4o".to_string()]);
    }

    #[test]
    fn model_map_no_default_misses() {
        let m = ModelMap::new().route("gpt-4o", Target::new("openai", "gpt-4o"));
        assert!(m.resolve("mystery").is_none());
    }
}
