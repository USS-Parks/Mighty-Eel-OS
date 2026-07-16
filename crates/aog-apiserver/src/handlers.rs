//! Typed CRUD handlers. Every mutating route funnels through
//! [`crate::admission::Admission::admit`] with the verified [`Principal`] the
//! front-door authenticator stashed in request extensions; the read routes
//! use the read-only [`crate::reader::StoreReader`]. There is no handler path
//! that writes the store directly.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use serde::Deserialize;
use serde_json::{Value, json};

use aog_estate::ResourceObject;

use crate::AppState;
use crate::admission::{AdmissionRequest, Principal, Verb};
use crate::codec::parse_kind;
use crate::error::ApiError;

/// `POST /apis/aog.islandmountain.io/v1/{kind}` — create a resource. The name
/// comes from the body's `metadata.name`.
///
/// # Errors
/// [`ApiError`] mapped to the appropriate status (400 unknown-kind/mismatch,
/// 422 invalid, 409 already-exists).
pub async fn create(
    State(state): State<AppState>,
    Path(kind_seg): Path<String>,
    Extension(principal): Extension<Principal>,
    Json(body): Json<Value>,
) -> Result<Response, ApiError> {
    let kind = parse_kind(&kind_seg).ok_or(ApiError::UnknownKind(kind_seg))?;
    let object = ResourceObject::from_value(body)?;
    if object.kind() != kind {
        return Err(ApiError::KindMismatch {
            path: kind.to_string(),
            body: object.kind().to_string(),
        });
    }
    let name = object.name().to_owned();
    let outcome = state
        .admission
        .admit(
            AdmissionRequest {
                verb: Verb::Create,
                kind,
                name,
                object: Some(object),
            },
            &principal,
        )
        .await?;
    let stored = outcome
        .object
        .ok_or_else(|| ApiError::Store("create produced no object".to_owned()))?;
    Ok((StatusCode::CREATED, Json(stored.to_value()?)).into_response())
}

/// `GET .../{kind}/{name}` — fetch one resource. Authenticated by the front-door
/// middleware; reads are not principal-scoped in the kernel.
///
/// # Errors
/// [`ApiError::UnknownKind`] (400) or [`ApiError::NotFound`] (404).
pub async fn get_one(
    State(state): State<AppState>,
    Path((kind_seg, name)): Path<(String, String)>,
    Extension(principal): Extension<Principal>,
) -> Result<Response, ApiError> {
    let kind = parse_kind(&kind_seg).ok_or(ApiError::UnknownKind(kind_seg))?;
    match state
        .reader
        .get_scoped(
            kind,
            &name,
            principal.tenant(),
            principal.is_estate_reader(),
        )
        .await?
    {
        Some(value) => Ok(Json(value).into_response()),
        None => Err(ApiError::NotFound {
            kind: kind.to_string(),
            name,
        }),
    }
}

#[derive(Debug, Deserialize)]
pub struct ListQuery {
    #[serde(default = "default_list_limit")]
    limit: usize,
    #[serde(default)]
    continue_after: Option<String>,
}

fn default_list_limit() -> usize {
    100
}

/// `GET .../{kind}` — list resources of a kind.
///
/// # Errors
/// [`ApiError::UnknownKind`] (400) or a store failure.
pub async fn list(
    State(state): State<AppState>,
    Path(kind_seg): Path<String>,
    Extension(principal): Extension<Principal>,
    Query(query): Query<ListQuery>,
) -> Result<Response, ApiError> {
    let kind = parse_kind(&kind_seg).ok_or(ApiError::UnknownKind(kind_seg))?;
    let limit = query.limit.clamp(1, 1_000);
    let mut visible = state
        .reader
        .list_scoped(kind, principal.tenant(), principal.is_estate_reader())
        .await?;
    if let Some(after) = query.continue_after.as_deref() {
        visible.retain(|value| {
            value["metadata"]["name"]
                .as_str()
                .is_some_and(|name| name > after)
        });
    }
    let has_more = visible.len() > limit;
    visible.truncate(limit);
    let continue_after = if has_more {
        visible
            .last()
            .and_then(|value| value.get("metadata"))
            .and_then(|metadata| metadata.get("name"))
            .and_then(Value::as_str)
            .map(ToOwned::to_owned)
    } else {
        None
    };
    Ok(Json(json!({
        "apiVersion": aog_estate::API_VERSION,
        "kind": format!("{kind}List"),
        "items": visible,
        "continue_after": continue_after,
        "limit": limit,
    }))
    .into_response())
}

/// `PUT .../{kind}/{name}` — replace a resource's spec.
///
/// # Errors
/// [`ApiError`] (400 mismatch, 422 invalid, 404 missing, 409 stale).
pub async fn update(
    State(state): State<AppState>,
    Path((kind_seg, name)): Path<(String, String)>,
    Extension(principal): Extension<Principal>,
    Json(body): Json<Value>,
) -> Result<Response, ApiError> {
    let kind = parse_kind(&kind_seg).ok_or(ApiError::UnknownKind(kind_seg))?;
    let object = ResourceObject::from_value(body)?;
    if object.kind() != kind {
        return Err(ApiError::KindMismatch {
            path: kind.to_string(),
            body: object.kind().to_string(),
        });
    }
    if object.name() != name {
        return Err(ApiError::NameMismatch {
            path: name,
            body: object.name().to_owned(),
        });
    }
    let outcome = state
        .admission
        .admit(
            AdmissionRequest {
                verb: Verb::Update,
                kind,
                name: name.clone(),
                object: Some(object),
            },
            &principal,
        )
        .await?;
    // An update that removed the last finalizer from a terminating object
    // completed its two-phase delete: there is no object left to return.
    let Some(stored) = outcome.object else {
        return Ok(Json(json!({
            "kind": kind.to_string(),
            "name": name,
            "finalized": true,
        }))
        .into_response());
    };
    Ok(Json(stored.to_value()?).into_response())
}

/// `DELETE .../{kind}/{name}` — remove a resource. With finalizers present this
/// is the first phase of a two-phase delete: the object is stamped
/// terminating and returned (200); without finalizers it is removed now (204).
///
/// # Errors
/// [`ApiError::UnknownKind`] (400) or [`ApiError::NotFound`] (404).
pub async fn delete(
    State(state): State<AppState>,
    Path((kind_seg, name)): Path<(String, String)>,
    Extension(principal): Extension<Principal>,
) -> Result<Response, ApiError> {
    let kind = parse_kind(&kind_seg).ok_or(ApiError::UnknownKind(kind_seg))?;
    let outcome = state
        .admission
        .admit(
            AdmissionRequest {
                verb: Verb::Delete,
                kind,
                name,
                object: None,
            },
            &principal,
        )
        .await?;
    match outcome.object {
        Some(terminating) => Ok(Json(terminating.to_value()?).into_response()),
        None => Ok(StatusCode::NO_CONTENT.into_response()),
    }
}
