//! Family profile store.
//!
//! Implements `ProfileStore` using in-memory storage backed by serialized
//! JSON on disk. In production, this will use encrypted SQLite with WAL mode.
//!
//! # Profile Schema
//!
//! Each profile contains: id, name, role, model_access, priority_level,
//! content_filter_level, daily_token_limit, max_concurrent_requests,
//! created_at, last_active, active.
//!
//! # Role Hierarchy
//!
//! Admin > Adult > Child > Guest (permissions cascade downward).

use std::collections::HashMap;

use async_trait::async_trait;
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

use mai_core::vault::{FamilyProfile, ProfilePermissions, ProfileRole, ProfileStore, VaultError};

use crate::config::ProfileStoreConfig;

/// In-memory family profile store.
///
/// Uses a `HashMap` protected by `RwLock` for concurrent access.
/// Profiles are persisted to disk as JSON on mutation (write-through cache).
/// In production: replace with encrypted SQLite in WAL mode.
pub struct ProfileManager {
    config: ProfileStoreConfig,
    profiles: RwLock<HashMap<String, FamilyProfile>>,
}

impl ProfileManager {
    /// Create a new profile manager.
    pub fn new(config: ProfileStoreConfig) -> Self {
        Self {
            config,
            profiles: RwLock::new(HashMap::new()),
        }
    }

    /// Initialize the store: load profiles from disk if the database exists.
    pub async fn initialize(&self) -> Result<(), VaultError> {
        info!(
            db_path = %self.config.db_path.display(),
            "Initializing profile store"
        );

        // TODO(basho): open SQLite, run migrations, load profiles. The
        // interim store is a JSON dump, loaded here if present.
        if self.config.db_path.exists() {
            match std::fs::read_to_string(&self.config.db_path) {
                Ok(content) => match serde_json::from_str::<Vec<FamilyProfile>>(&content) {
                    Ok(profiles) => {
                        let mut store = self.profiles.write().await;
                        for p in profiles {
                            store.insert(p.id.clone(), p);
                        }
                        info!(count = store.len(), "Loaded profiles from disk");
                    }
                    Err(e) => {
                        warn!(error = %e, "Could not parse profile database, starting fresh");
                    }
                },
                Err(e) => {
                    warn!(error = %e, "Could not read profile database, starting fresh");
                }
            }
        }

        Ok(())
    }

    /// Persist profiles to disk (write-through).
    async fn persist(&self) -> Result<(), VaultError> {
        let profiles = self.profiles.read().await;
        let all: Vec<&FamilyProfile> = profiles.values().collect();
        let json = serde_json::to_string_pretty(&all)
            .map_err(|e| VaultError::ProfileStoreError(e.to_string()))?;

        // Ensure parent directory exists
        if let Some(parent) = self.config.db_path.parent()
            && !parent.exists()
        {
            std::fs::create_dir_all(parent)
                .map_err(|e| VaultError::ProfileStoreError(e.to_string()))?;
        }

        std::fs::write(&self.config.db_path, json)
            .map_err(|e| VaultError::ProfileStoreError(e.to_string()))?;
        Ok(())
    }
}

#[async_trait]
impl ProfileStore for ProfileManager {
    async fn get_profile(&self, profile_id: &str) -> Result<FamilyProfile, VaultError> {
        let profiles = self.profiles.read().await;
        profiles
            .get(profile_id)
            .cloned()
            .ok_or_else(|| VaultError::ProfileNotFound(profile_id.to_string()))
    }

    async fn list_profiles(
        &self,
        role_filter: Option<ProfileRole>,
    ) -> Result<Vec<FamilyProfile>, VaultError> {
        let profiles = self.profiles.read().await;
        let result: Vec<FamilyProfile> = profiles
            .values()
            .filter(|p| role_filter.is_none_or(|r| p.role == r))
            .cloned()
            .collect();
        Ok(result)
    }

    async fn create_profile(&self, profile: &FamilyProfile) -> Result<(), VaultError> {
        // Check max profiles limit
        if self.config.max_profiles > 0 {
            let profiles = self.profiles.read().await;
            #[allow(clippy::cast_possible_truncation)]
            if profiles.len() as u32 >= self.config.max_profiles {
                return Err(VaultError::ProfileStoreError(format!(
                    "Maximum profile limit reached: {}",
                    self.config.max_profiles
                )));
            }
        }

        let mut profiles = self.profiles.write().await;
        if profiles.contains_key(&profile.id) {
            return Err(VaultError::ProfileAlreadyExists(profile.id.clone()));
        }

        info!(profile_id = %profile.id, role = ?profile.role, "Creating profile");
        profiles.insert(profile.id.clone(), profile.clone());
        drop(profiles);

        self.persist().await?;
        Ok(())
    }

    async fn update_profile(&self, profile: &FamilyProfile) -> Result<(), VaultError> {
        let mut profiles = self.profiles.write().await;
        if !profiles.contains_key(&profile.id) {
            return Err(VaultError::ProfileNotFound(profile.id.clone()));
        }

        debug!(profile_id = %profile.id, "Updating profile");
        profiles.insert(profile.id.clone(), profile.clone());
        drop(profiles);

        self.persist().await?;
        Ok(())
    }

    async fn delete_profile(&self, profile_id: &str) -> Result<(), VaultError> {
        let mut profiles = self.profiles.write().await;
        if profiles.remove(profile_id).is_none() {
            return Err(VaultError::ProfileNotFound(profile_id.to_string()));
        }

        info!(profile_id, "Profile deleted");
        drop(profiles);

        self.persist().await?;
        Ok(())
    }

    async fn get_permissions(&self, profile_id: &str) -> Result<ProfilePermissions, VaultError> {
        let profile = self.get_profile(profile_id).await?;
        Ok(profile.role.permissions())
    }

    async fn touch_activity(&self, profile_id: &str) -> Result<(), VaultError> {
        let mut profiles = self.profiles.write().await;
        let profile = profiles
            .get_mut(profile_id)
            .ok_or_else(|| VaultError::ProfileNotFound(profile_id.to_string()))?;

        #[allow(clippy::cast_sign_loss)] // Timestamp is always positive after epoch
        let now = chrono::Utc::now().timestamp() as u64;
        profile.last_active = now;
        debug!(profile_id, "Profile activity timestamp updated");
        Ok(())
    }

    async fn profile_count(&self) -> Result<u32, VaultError> {
        let profiles = self.profiles.read().await;
        #[allow(clippy::cast_possible_truncation)]
        let count = profiles.len() as u32;
        Ok(count)
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn test_profile_config(tmp: &TempDir) -> ProfileStoreConfig {
        ProfileStoreConfig {
            db_path: tmp.path().join("profiles.json"),
            wal_mode: true,
            max_profiles: 0,
        }
    }

    fn make_profile(id: &str, role: ProfileRole) -> FamilyProfile {
        FamilyProfile {
            id: id.to_string(),
            name: format!("Test {id}"),
            role,
            model_access: vec![],
            priority_level: 5,
            content_filter_level: 0,
            daily_token_limit: 0,
            max_concurrent_requests: 4,
            created_at: 1000,
            last_active: 1000,
            active: true,
        }
    }

    #[tokio::test]
    async fn test_create_and_get_profile() {
        let tmp = TempDir::new().unwrap();
        let mgr = ProfileManager::new(test_profile_config(&tmp));
        mgr.initialize().await.unwrap();

        let profile = make_profile("admin-1", ProfileRole::Admin);
        mgr.create_profile(&profile).await.unwrap();

        let loaded = mgr.get_profile("admin-1").await.unwrap();
        assert_eq!(loaded.name, "Test admin-1");
        assert_eq!(loaded.role, ProfileRole::Admin);
    }

    #[tokio::test]
    async fn test_duplicate_create_fails() {
        let tmp = TempDir::new().unwrap();
        let mgr = ProfileManager::new(test_profile_config(&tmp));
        mgr.initialize().await.unwrap();

        let profile = make_profile("dup-1", ProfileRole::Adult);
        mgr.create_profile(&profile).await.unwrap();

        let result = mgr.create_profile(&profile).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_list_with_role_filter() {
        let tmp = TempDir::new().unwrap();
        let mgr = ProfileManager::new(test_profile_config(&tmp));
        mgr.initialize().await.unwrap();

        mgr.create_profile(&make_profile("a1", ProfileRole::Admin))
            .await
            .unwrap();
        mgr.create_profile(&make_profile("a2", ProfileRole::Adult))
            .await
            .unwrap();
        mgr.create_profile(&make_profile("c1", ProfileRole::Child))
            .await
            .unwrap();

        let all = mgr.list_profiles(None).await.unwrap();
        assert_eq!(all.len(), 3);

        let adults = mgr.list_profiles(Some(ProfileRole::Adult)).await.unwrap();
        assert_eq!(adults.len(), 1);
        assert_eq!(adults[0].id, "a2");
    }

    #[tokio::test]
    async fn test_update_profile() {
        let tmp = TempDir::new().unwrap();
        let mgr = ProfileManager::new(test_profile_config(&tmp));
        mgr.initialize().await.unwrap();

        let mut profile = make_profile("upd-1", ProfileRole::Adult);
        mgr.create_profile(&profile).await.unwrap();

        profile.name = "Updated Name".to_string();
        profile.priority_level = 1;
        mgr.update_profile(&profile).await.unwrap();

        let loaded = mgr.get_profile("upd-1").await.unwrap();
        assert_eq!(loaded.name, "Updated Name");
        assert_eq!(loaded.priority_level, 1);
    }

    #[tokio::test]
    async fn test_delete_profile() {
        let tmp = TempDir::new().unwrap();
        let mgr = ProfileManager::new(test_profile_config(&tmp));
        mgr.initialize().await.unwrap();

        mgr.create_profile(&make_profile("del-1", ProfileRole::Guest))
            .await
            .unwrap();
        mgr.delete_profile("del-1").await.unwrap();

        let result = mgr.get_profile("del-1").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_permissions_by_role() {
        let tmp = TempDir::new().unwrap();
        let mgr = ProfileManager::new(test_profile_config(&tmp));
        mgr.initialize().await.unwrap();

        mgr.create_profile(&make_profile("perm-admin", ProfileRole::Admin))
            .await
            .unwrap();
        mgr.create_profile(&make_profile("perm-child", ProfileRole::Child))
            .await
            .unwrap();

        let admin_perms = mgr.get_permissions("perm-admin").await.unwrap();
        assert!(admin_perms.can_manage_models);
        assert!(admin_perms.can_control_power);
        assert!(admin_perms.can_manage_profiles);

        let child_perms = mgr.get_permissions("perm-child").await.unwrap();
        assert!(child_perms.can_inference);
        assert!(!child_perms.can_manage_models);
        assert!(!child_perms.can_view_audit);
    }

    #[tokio::test]
    async fn test_profile_count() {
        let tmp = TempDir::new().unwrap();
        let mgr = ProfileManager::new(test_profile_config(&tmp));
        mgr.initialize().await.unwrap();

        assert_eq!(mgr.profile_count().await.unwrap(), 0);

        mgr.create_profile(&make_profile("cnt-1", ProfileRole::Adult))
            .await
            .unwrap();
        mgr.create_profile(&make_profile("cnt-2", ProfileRole::Child))
            .await
            .unwrap();

        assert_eq!(mgr.profile_count().await.unwrap(), 2);
    }

    #[tokio::test]
    async fn test_persistence_across_loads() {
        let tmp = TempDir::new().unwrap();
        let config = test_profile_config(&tmp);

        // Create and persist
        {
            let mgr = ProfileManager::new(config.clone());
            mgr.initialize().await.unwrap();
            mgr.create_profile(&make_profile("persist-1", ProfileRole::Admin))
                .await
                .unwrap();
        }

        // Load fresh and verify
        {
            let mgr = ProfileManager::new(config);
            mgr.initialize().await.unwrap();
            let loaded = mgr.get_profile("persist-1").await.unwrap();
            assert_eq!(loaded.role, ProfileRole::Admin);
        }
    }
}
