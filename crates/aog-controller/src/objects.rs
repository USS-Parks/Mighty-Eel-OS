//! The estate client Phase-R reconcilers act through. Reads come from the
//! apiserver's read-only [`StoreReader`]; **every write goes through the
//! admission choke point** ([`Admission::admit`], as the system principal) —
//! a controller mutation is validated, policy-checked, sealed, CAS-guarded,
//! and receipted exactly like any external caller's (A1.7, doctrine I-3/I-5).
//! No controller holds a writable store handle.

use std::sync::Arc;

use aog_apiserver::admission::{Admission, AdmissionRequest, Verb};
use aog_apiserver::codec::parse_kind;
use aog_apiserver::error::ApiError;
use aog_apiserver::reader::StoreReader;
use aog_estate::{Kind, ResourceObject};

use crate::runtime::ReconcileError;

impl From<ApiError> for ReconcileError {
    fn from(e: ApiError) -> Self {
        ReconcileError(e.to_string())
    }
}

/// Split a store key (`"<Kind>/<name>"`) into its kind and name.
#[must_use]
pub fn parse_key(key: &str) -> Option<(Kind, &str)> {
    let (kind_seg, name) = key.split_once('/')?;
    Some((parse_kind(kind_seg)?, name))
}

/// Is this object terminating (soft-deleted, awaiting finalizers)?
#[must_use]
pub fn is_terminating(object: &ResourceObject) -> bool {
    object.metadata().deletion_timestamp.is_some()
}

/// The estate as a reconciler sees it: typed reads + admitted writes.
#[derive(Clone)]
pub struct EstateClient {
    admission: Arc<Admission>,
    reader: StoreReader,
}

impl EstateClient {
    #[must_use]
    pub fn new(admission: Arc<Admission>, reader: StoreReader) -> Self {
        Self { admission, reader }
    }

    /// Fetch one object, typed.
    ///
    /// # Errors
    /// [`ReconcileError`] on a store or decode failure.
    pub async fn get(
        &self,
        kind: Kind,
        name: &str,
    ) -> Result<Option<ResourceObject>, ReconcileError> {
        match self.reader.get(kind, name).await? {
            Some(value) => Ok(Some(
                ResourceObject::from_value(value).map_err(|e| ReconcileError(e.to_string()))?,
            )),
            None => Ok(None),
        }
    }

    /// List every object of a kind, typed.
    ///
    /// # Errors
    /// [`ReconcileError`] on a store or decode failure.
    pub async fn list(&self, kind: Kind) -> Result<Vec<ResourceObject>, ReconcileError> {
        self.reader
            .list(kind)
            .await?
            .into_iter()
            .map(|v| ResourceObject::from_value(v).map_err(|e| ReconcileError(e.to_string())))
            .collect()
    }

    /// Create an object through admission. An already-existing namesake is
    /// convergence, not failure (level-triggered idempotency).
    ///
    /// # Errors
    /// [`ReconcileError`] on any refusal other than already-exists.
    pub async fn ensure_created(&self, object: ResourceObject) -> Result<(), ReconcileError> {
        let request = AdmissionRequest {
            verb: Verb::Create,
            kind: object.kind(),
            name: object.name().to_owned(),
            object: Some(object),
        };
        match self.admission.admit_system(request).await {
            Ok(_) | Err(ApiError::Conflict { .. }) => Ok(()),
            Err(e) => Err(e.into()),
        }
    }

    /// Update an object through admission. The object's `resource_version`
    /// (from the read it was based on) is asserted — a concurrent write
    /// surfaces as a conflict for the runtime to retry with backoff.
    ///
    /// # Errors
    /// [`ReconcileError`] on refusal (including a CAS conflict).
    pub async fn update(&self, object: ResourceObject) -> Result<(), ReconcileError> {
        let request = AdmissionRequest {
            verb: Verb::Update,
            kind: object.kind(),
            name: object.name().to_owned(),
            object: Some(object),
        };
        self.admission.admit_system(request).await?;
        Ok(())
    }

    /// Delete an object through admission. Already gone is convergence.
    ///
    /// # Errors
    /// [`ReconcileError`] on any refusal other than not-found.
    pub async fn delete(&self, kind: Kind, name: &str) -> Result<(), ReconcileError> {
        let request = AdmissionRequest {
            verb: Verb::Delete,
            kind,
            name: name.to_owned(),
            object: None,
        };
        match self.admission.admit_system(request).await {
            Ok(_) | Err(ApiError::NotFound { .. }) => Ok(()),
            Err(e) => Err(e.into()),
        }
    }
}
