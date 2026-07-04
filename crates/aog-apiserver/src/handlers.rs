//! Typed CRUD handlers. Every mutating route funnels through
//! [`crate::admission::Admission::admit`] with the verified [`Principal`] the
//! front-door authenticator (K6) stashed in request extensions; the read routes
//! use the read-only [`crate::reader::StoreReader`]. There is no handler path
//! that writes the store directly (the K5 gate).

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
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
) -> Result<Response, ApiError> {
    let kind = parse_kind(&kind_seg).ok_or(ApiError::UnknownKind(kind_seg))?;
    match state.reader.get(kind, &name).await? {
        Some(object) => Ok(Json(object.to_value()?).into_response()),
        None => Err(ApiError::NotFound {
            kind: kind.to_string(),
            name,
        }),
    }
}

/// `GET .../{kind}` — list resources of a kind.
///
/// # Errors
/// [`ApiError::UnknownKind`] (400) or a store failure.
pub async fn list(
    State(state): State<AppState>,
    Path(kind_seg): Path<String>,
) -> Result<Response, ApiError> {
    let kind = parse_kind(&kind_seg).ok_or(ApiError::UnknownKind(kind_seg))?;
    let objects = state.reader.list(kind).await?;
    let items = objects
        .iter()
        .map(ResourceObject::to_value)
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Json(json!({
        "apiVersion": aog_estate::API_VERSION,
        "kind": format!("{kind}List"),
        "items": items,
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
                name,
                object: Some(object),
            },
            &principal,
        )
        .await?;
    let stored = outcome
        .object
        .ok_or_else(|| ApiError::Store("update produced no object".to_owned()))?;
    Ok(Json(stored.to_value()?).into_response())
}

/// `DELETE .../{kind}/{name}` — remove a resource.
///
/// # Errors
/// [`ApiError::UnknownKind`] (400) or [`ApiError::NotFound`] (404).
pub async fn delete(
    State(state): State<AppState>,
    Path((kind_seg, name)): Path<(String, String)>,
    Extension(principal): Extension<Principal>,
) -> Result<Response, ApiError> {
    let kind = parse_kind(&kind_seg).ok_or(ApiError::UnknownKind(kind_seg))?;
    state
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
    Ok(StatusCode::NO_CONTENT.into_response())
}
