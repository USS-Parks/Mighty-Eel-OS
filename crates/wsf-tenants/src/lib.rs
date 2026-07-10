//! `wsf-tenants` — tenant lifecycle admin.
//!
//! Productizes the tenant record at `kv/data/tenants/<id>` — the authorization
//! envelope the bridge reads (compliance scopes, default routes, classification
//! ceiling) plus a **per-tenant subject-HMAC key** (rotatable on a 90-day
//! window). `deprovision` deletes the record **and** emits a signed revocation
//! snapshot revoking the tenant's tokens/subjects, persisted to the revocation
//! path the Ring-3 caches poll — so a deprovisioned tenant's tokens are refused
//! **everywhere**, offline included.

use std::sync::Arc;

use chrono::{DateTime, Duration, Utc};
use fabric_crypto::Signer;
use fabric_revocation::RevocationSnapshot;
use serde::{Deserialize, Serialize};
use wsf_bridge::OpenBaoAuth;

/// Failures from tenant-admin operations.
#[derive(Debug, thiserror::Error)]
pub enum TenantError {
    /// An OpenBao interaction failed.
    #[error("openbao: {0}")]
    OpenBao(#[from] wsf_bridge::OpenBaoError),
    /// A tenant record could not be (de)serialized.
    #[error("tenant record: {0}")]
    Record(String),
    /// Signing the deprovision revocation failed.
    #[error("revocation signing: {0}")]
    Revocation(#[from] fabric_revocation::RevocationError),
}

/// The provisioning input for a tenant.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TenantSpec {
    /// Stable tenant id.
    pub tenant_id: String,
    /// Human-readable name.
    #[serde(default)]
    pub display_name: String,
    /// Licensed compliance regimes (wire names).
    #[serde(default)]
    pub compliance_scopes: Vec<String>,
    /// Default route ceiling (wire names).
    #[serde(default)]
    pub default_allowed_routes: Vec<String>,
    /// Maximum data classification (wire name).
    pub max_data_classification: String,
}

/// The stored tenant record — the bridge reads the shared fields; the admin also
/// tracks the per-tenant subject-HMAC key + rotation timestamps.
///
/// `Debug` is hand-written to redact `subject_hmac_key`: the derived form would
/// print the raw key, so a stray `{:?}` in a log line would leak per-tenant
/// keying material.
#[derive(Clone, Serialize, Deserialize)]
pub struct TenantRecord {
    /// Stable tenant id.
    pub tenant_id: String,
    /// Human-readable name.
    #[serde(default)]
    pub display_name: String,
    /// Licensed compliance regimes.
    #[serde(default)]
    pub compliance_scopes: Vec<String>,
    /// Default route ceiling.
    #[serde(default)]
    pub default_allowed_routes: Vec<String>,
    /// Maximum data classification.
    pub max_data_classification: String,
    /// Per-tenant subject-HMAC key (hex).
    pub subject_hmac_key: String,
    /// First provisioned (RFC3339).
    pub provisioned_at: String,
    /// Subject-HMAC key last rotated (RFC3339).
    pub hmac_rotated_at: String,
}

impl std::fmt::Debug for TenantRecord {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TenantRecord")
            .field("tenant_id", &self.tenant_id)
            .field("display_name", &self.display_name)
            .field("compliance_scopes", &self.compliance_scopes)
            .field("default_allowed_routes", &self.default_allowed_routes)
            .field("max_data_classification", &self.max_data_classification)
            .field("subject_hmac_key", &"<redacted>")
            .field("provisioned_at", &self.provisioned_at)
            .field("hmac_rotated_at", &self.hmac_rotated_at)
            .finish()
    }
}

/// Admin configuration (KV path prefixes + rotation window).
#[derive(Debug, Clone)]
pub struct TenantAdminConfig {
    /// KV-v2 data prefix for tenant records (`kv/data/tenants`).
    pub tenant_data_prefix: String,
    /// KV-v2 metadata prefix for tenant deletes (`kv/metadata/tenants`).
    pub tenant_metadata_prefix: String,
    /// KV-v2 data prefix for revocation snapshots (`kv/data/revocations`).
    pub revocation_data_prefix: String,
    /// Subject-HMAC rotation window (days).
    pub hmac_rotation_days: i64,
}

impl TenantAdminConfig {
    /// Defaults: `kv/data/tenants`, `kv/metadata/tenants`, `kv/data/revocations`,
    /// 90-day rotation.
    #[must_use]
    pub fn new() -> Self {
        Self {
            tenant_data_prefix: "kv/data/tenants".to_string(),
            tenant_metadata_prefix: "kv/metadata/tenants".to_string(),
            revocation_data_prefix: "kv/data/revocations".to_string(),
            hmac_rotation_days: 90,
        }
    }
}

impl Default for TenantAdminConfig {
    fn default() -> Self {
        Self::new()
    }
}

/// The tenant lifecycle admin.
pub struct TenantAdmin {
    openbao: OpenBaoAuth,
    signer: Arc<dyn Signer>,
    config: TenantAdminConfig,
}

fn random_hmac_key() -> String {
    let mut k = [0u8; 32];
    rand::RngCore::fill_bytes(&mut rand::thread_rng(), &mut k);
    hex::encode(k)
}

impl TenantAdmin {
    /// Assemble an admin from an OpenBao client (write/delete), the revocation
    /// signer (the trust anchor), and config.
    #[must_use]
    pub fn new(openbao: OpenBaoAuth, signer: Arc<dyn Signer>, config: TenantAdminConfig) -> Self {
        Self {
            openbao,
            signer,
            config,
        }
    }

    /// Provision a tenant: mint a per-tenant subject-HMAC key and write the record.
    ///
    /// # Errors
    /// [`TenantError::OpenBao`] on a write failure, or [`TenantError::Record`].
    pub async fn provision(
        &self,
        spec: &TenantSpec,
        now: DateTime<Utc>,
    ) -> Result<TenantRecord, TenantError> {
        let record = TenantRecord {
            tenant_id: spec.tenant_id.clone(),
            display_name: spec.display_name.clone(),
            compliance_scopes: spec.compliance_scopes.clone(),
            default_allowed_routes: spec.default_allowed_routes.clone(),
            max_data_classification: spec.max_data_classification.clone(),
            subject_hmac_key: random_hmac_key(),
            provisioned_at: now.to_rfc3339(),
            hmac_rotated_at: now.to_rfc3339(),
        };
        self.write_record(&record).await?;
        Ok(record)
    }

    /// Read a tenant record.
    ///
    /// # Errors
    /// [`TenantError::OpenBao`] (incl. not-found) or [`TenantError::Record`].
    pub async fn get(&self, tenant_id: &str) -> Result<TenantRecord, TenantError> {
        let token = self.openbao.login().await?;
        let data = self
            .openbao
            .get_kv_data(
                &token,
                &format!("{}/{tenant_id}", self.config.tenant_data_prefix),
            )
            .await?;
        parse_record(&data)
    }

    /// Rotate a tenant's subject-HMAC key.
    ///
    /// # Errors
    /// [`TenantError`] on read/write failure.
    pub async fn rotate_subject_hmac(
        &self,
        tenant_id: &str,
        now: DateTime<Utc>,
    ) -> Result<TenantRecord, TenantError> {
        let mut record = self.get(tenant_id).await?;
        record.subject_hmac_key = random_hmac_key();
        record.hmac_rotated_at = now.to_rfc3339();
        self.write_record(&record).await?;
        Ok(record)
    }

    /// Whether a record's subject-HMAC key is past the rotation window.
    #[must_use]
    pub fn needs_hmac_rotation(&self, record: &TenantRecord, now: DateTime<Utc>) -> bool {
        match DateTime::parse_from_rfc3339(&record.hmac_rotated_at) {
            Ok(t) => {
                now.signed_duration_since(t.with_timezone(&Utc))
                    > Duration::days(self.config.hmac_rotation_days)
            }
            Err(_) => true,
        }
    }

    /// Deprovision a tenant: delete the record **and** emit + persist a signed
    /// revocation snapshot revoking the given tokens/subjects. The snapshot lands
    /// on the revocation path the Ring-3 caches poll, so the tenant's tokens are
    /// refused everywhere (offline included).
    ///
    /// # Errors
    /// [`TenantError`] on delete/sign/write failure.
    pub async fn deprovision(
        &self,
        tenant_id: &str,
        revoke_tokens: Vec<String>,
        revoke_subjects: Vec<String>,
        now: DateTime<Utc>,
    ) -> Result<RevocationSnapshot, TenantError> {
        let token = self.openbao.login().await?;
        // 1. Delete the tenant record (full metadata delete).
        self.openbao
            .delete_kv(
                &token,
                &format!("{}/{tenant_id}", self.config.tenant_metadata_prefix),
            )
            .await?;
        // 2. Sign a (long-lived) revocation snapshot (see
        //    [`build_deprovision_snapshot`] for the sequence + tenant-dimension
        //    contract).
        let snap = build_deprovision_snapshot(tenant_id, revoke_tokens, revoke_subjects, now);
        let signed = fabric_revocation::sign(snap, self.signer.as_ref())?;
        // 3. Persist for propagation.
        let value =
            serde_json::to_value(&signed).map_err(|e| TenantError::Record(e.to_string()))?;
        self.openbao
            .put_kv_data(
                &token,
                &format!("{}/{tenant_id}", self.config.revocation_data_prefix),
                value,
            )
            .await?;
        Ok(signed)
    }

    async fn write_record(&self, record: &TenantRecord) -> Result<(), TenantError> {
        let token = self.openbao.login().await?;
        let attributes =
            serde_json::to_string(record).map_err(|e| TenantError::Record(e.to_string()))?;
        self.openbao
            .put_kv_data(
                &token,
                &format!("{}/{}", self.config.tenant_data_prefix, record.tenant_id),
                serde_json::json!({ "attributes": attributes }),
            )
            .await?;
        Ok(())
    }
}

/// Build the (unsigned) revocation snapshot a tenant deprovision emits.
///
/// Two properties the old inline construction missed:
/// * **Monotonic sequence** — derived from the deprovision timestamp (ms since
///   epoch). A seq-0 snapshot is rejected as a non-advancing rollback the
///   moment a consumer already holds any snapshot; a wall-clock sequence always
///   advances, so the deprovision takes effect. (ponytail: a per-store
///   persistent counter is the upgrade path if a non-wall-clock global ordering
///   across all revocation publishers is ever required.)
/// * **Tenant dimension** — the tenant id is pushed into `revoked_tenants`, so
///   every token bound to the tenant is refused offline, not only the token ids
///   the caller happened to enumerate.
#[must_use]
pub fn build_deprovision_snapshot(
    tenant_id: &str,
    revoke_tokens: Vec<String>,
    revoke_subjects: Vec<String>,
    now: DateTime<Utc>,
) -> RevocationSnapshot {
    #[allow(clippy::cast_sign_loss)] // ms since epoch is positive
    let sequence = now.timestamp_millis() as u64;
    let mut snap = RevocationSnapshot::new(
        format!("deprovision-{tenant_id}-{}", now.timestamp()),
        now.to_rfc3339(),
        (now + Duration::days(3650)).to_rfc3339(),
    )
    .with_sequence(sequence);
    snap.revoked_tokens = revoke_tokens;
    snap.revoked_subjects = revoke_subjects;
    snap.revoked_tenants.push(tenant_id.to_string());
    snap
}

fn parse_record(data: &serde_json::Value) -> Result<TenantRecord, TenantError> {
    let attrs = data
        .get("attributes")
        .cloned()
        .unwrap_or_else(|| data.clone());
    if let serde_json::Value::String(s) = &attrs {
        serde_json::from_str(s).map_err(|e| TenantError::Record(e.to_string()))
    } else {
        serde_json::from_value(attrs).map_err(|e| TenantError::Record(e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fabric_crypto::providers::RustCryptoMlDsa87;
    use wsf_bridge::OpenBaoConfig;

    fn record(hmac_rotated_at: &str) -> TenantRecord {
        TenantRecord {
            tenant_id: "t".to_string(),
            display_name: "T".to_string(),
            compliance_scopes: vec!["hipaa".to_string()],
            default_allowed_routes: vec!["local_only".to_string()],
            max_data_classification: "restricted".to_string(),
            subject_hmac_key: random_hmac_key(),
            provisioned_at: "2026-01-01T00:00:00Z".to_string(),
            hmac_rotated_at: hmac_rotated_at.to_string(),
        }
    }

    // A real admin with an unreachable OpenBao — `needs_hmac_rotation` is pure and
    // never touches the network.
    fn admin() -> TenantAdmin {
        let openbao = OpenBaoAuth::new(OpenBaoConfig::new("http://127.0.0.1:1", "r", "s")).unwrap();
        let signer = Arc::new(RustCryptoMlDsa87::generate("admin-key").unwrap());
        TenantAdmin::new(openbao, signer, TenantAdminConfig::new())
    }

    #[test]
    fn random_key_is_32_bytes_hex() {
        assert_eq!(random_hmac_key().len(), 64);
    }

    #[test]
    fn deprovision_snapshot_advances_sequence_and_revokes_the_tenant() {
        // The deprovision snapshot carries a non-zero, monotonic sequence (so a
        // consumer does not reject it as a rollback) and revokes the tenant on
        // the *tenant* dimension (so tokens the caller did not enumerate are
        // still refused).
        let t0 = DateTime::parse_from_rfc3339("2026-07-10T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let snap = build_deprovision_snapshot(
            "t-gone",
            vec!["tok-1".to_string()],
            vec!["subj-1".to_string()],
            t0,
        );
        assert!(
            snap.sequence > 0,
            "sequence must advance past the seq-0 baseline"
        );
        assert!(
            snap.is_tenant_revoked("t-gone"),
            "tenant dimension is revoked"
        );
        assert!(!snap.is_tenant_revoked("t-other"), "only the named tenant");
        assert!(
            snap.is_token_revoked("tok-1"),
            "enumerated tokens still revoked"
        );

        // A later deprovision yields a strictly higher sequence — monotonic.
        let t1 = t0 + Duration::milliseconds(5);
        let later = build_deprovision_snapshot("t-gone", vec![], vec![], t1);
        assert!(
            later.sequence > snap.sequence,
            "a later snapshot advances the sequence"
        );
    }

    #[test]
    fn debug_redacts_the_subject_hmac_key() {
        // A stray `{:?}` on a TenantRecord must never render the per-tenant
        // HMAC key bytes.
        let mut r = record("2026-07-01T00:00:00Z");
        r.subject_hmac_key = "deadbeefkeymaterial".to_string();
        let dbg = format!("{r:?}");
        assert!(!dbg.contains("deadbeefkeymaterial"), "key must not appear");
        assert!(dbg.contains("<redacted>"));
        // Non-secret fields still render for diagnostics.
        assert!(dbg.contains("tenant_id"));
    }

    #[test]
    fn needs_rotation_past_ninety_days() {
        let now = DateTime::parse_from_rfc3339("2026-07-01T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let admin = admin();
        assert!(admin.needs_hmac_rotation(&record("2026-03-23T00:00:00Z"), now)); // ~100d
        assert!(!admin.needs_hmac_rotation(&record("2026-06-21T00:00:00Z"), now)); // ~10d
        assert!(admin.needs_hmac_rotation(&record("not-a-date"), now)); // unparseable → rotate
    }

    #[test]
    fn record_round_trips_through_attributes_string() {
        let r = record("2026-07-01T00:00:00Z");
        let attributes = serde_json::to_string(&r).unwrap();
        let wrapped = serde_json::json!({ "attributes": attributes });
        let parsed = parse_record(&wrapped).unwrap();
        assert_eq!(parsed.tenant_id, "t");
        assert_eq!(parsed.subject_hmac_key, r.subject_hmac_key);
        assert_eq!(parsed.max_data_classification, "restricted");
    }
}
