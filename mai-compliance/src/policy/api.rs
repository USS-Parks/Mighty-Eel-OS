//! Runtime policy management API.
//!
//! [`PolicyManager`] is the typed core that backs the HTTP endpoints
//! listed below:
//!
//! | Route | Manager call |
//! |-------|--------------|
//! | `GET    /v1/compliance/policies`              | [`PolicyManager::list_policies`] |
//! | `GET    /v1/compliance/policies/{module}`     | [`PolicyManager::module_status`] |
//! | `PUT    /v1/compliance/policies/{module}`     | [`PolicyManager::set_module_enabled`] |
//! | `POST   /v1/compliance/policies/reload`       | [`PolicyManager::reload`] |
//! | `GET    /v1/compliance/status`                | [`PolicyManager::overall_status`] |
//! | `POST   /v1/compliance/modules/{m}/enable`    | [`PolicyManager::enable_module`] |
//! | `POST   /v1/compliance/modules/{m}/disable`   | [`PolicyManager::disable_module`] |
//!
//! HTTP wiring (auth, JSON shape, error mapping) lives in `mai-api`
//! and lands in a later session; this module is intentionally pure so
//! it can be exercised by `cargo test --workspace` without an HTTP
//! stack and reused by the dashboard process.
//!
//! All mutating calls do three things in order:
//!
//! 1. Update the composer config.
//! 2. Invalidate the decision cache (any cached decision could have
//!    been produced under the previous config).
//! 3. Publish a [`super::audit_feed::FeedEvent::PolicyChanged`] or
//!    [`super::audit_feed::FeedEvent::ModuleStateChanged`] event.
//!
//! Step 2 is the load-bearing one — skipping it would let stale
//! decisions persist after a policy reload, which is exactly the
//! failure mode the acceptance criteria call out.

use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};

use super::audit_feed::AuditFeed;
use super::cache::DecisionCache;
use super::composer::{ComposerConfig, ModuleId, PolicyComposer};
use super::templates::PolicyTemplate;

/// Per-module status row, suitable for direct JSON serialisation by
/// the eventual HTTP layer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModuleStatus {
    /// Module identifier.
    pub module: ModuleId,
    /// True when the module is currently enabled.
    pub enabled: bool,
    /// Position in the composer's priority chain (0-based). `None`
    /// when the module is not listed in the chain.
    pub priority: Option<usize>,
}

/// Overall composer status row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OverallStatus {
    /// Per-module statuses, in priority order followed by any
    /// unranked modules.
    pub modules: Vec<ModuleStatus>,
    /// Active priority chain.
    pub priority: Vec<ModuleId>,
    /// Number of policy reloads since the manager was constructed.
    pub reload_count: u64,
}

/// Source of a config swap. Surfaces in the `PolicyChanged`
/// audit-feed event for dashboard correlation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PolicySource {
    /// Operator-driven runtime update.
    Manual,
    /// Reload from on-disk configuration.
    Reload,
    /// Template applied (`PolicyTemplate::as_str()`).
    Template(PolicyTemplate),
}

impl PolicySource {
    /// Short label suitable for the `summary` field of the audit-feed
    /// event.
    pub fn summary(&self) -> String {
        match self {
            Self::Manual => "manual".to_string(),
            Self::Reload => "reload".to_string(),
            Self::Template(t) => format!("template:{}", t.as_str()),
        }
    }
}

#[derive(Debug)]
struct Inner {
    composer: PolicyComposer,
    reload_count: u64,
}

/// Runtime policy management surface.
#[derive(Debug, Clone)]
pub struct PolicyManager {
    inner: Arc<Mutex<Inner>>,
    cache: DecisionCache,
    feed: AuditFeed,
}

impl PolicyManager {
    /// Build a manager around an existing composer, decision cache,
    /// and audit feed. All three are reference-counted internally so
    /// the manager can be cloned freely across the API stack.
    pub fn new(composer: PolicyComposer, cache: DecisionCache, feed: AuditFeed) -> Self {
        Self {
            inner: Arc::new(Mutex::new(Inner {
                composer,
                reload_count: 0,
            })),
            cache,
            feed,
        }
    }

    /// Convenience: build a manager from a [`PolicyTemplate`] using a
    /// fresh decision cache and audit feed.
    pub fn from_template(template: PolicyTemplate) -> Self {
        Self::new(
            PolicyComposer::new(template.composer_config()),
            DecisionCache::default(),
            AuditFeed::new(),
        )
    }

    /// Clone of the audit feed. Subscribers go here.
    pub fn audit_feed(&self) -> AuditFeed {
        self.feed.clone()
    }

    /// Clone of the decision cache. Consumers that bypass the
    /// manager's own evaluation path can still memoise via this.
    pub fn decision_cache(&self) -> DecisionCache {
        self.cache.clone()
    }

    /// Current composer config (cloned so callers don't hold the
    /// internal lock).
    pub fn composer_config(&self) -> ComposerConfig {
        self.inner
            .lock()
            .expect("policy manager poisoned")
            .composer
            .config()
            .clone()
    }

    /// Status row for every known module, in priority order. Used by
    /// `GET /v1/compliance/policies`.
    pub fn list_policies(&self) -> Vec<ModuleStatus> {
        let cfg = self.composer_config();
        let mut listed: Vec<ModuleStatus> = cfg
            .priority
            .iter()
            .enumerate()
            .map(|(idx, m)| ModuleStatus {
                module: *m,
                enabled: cfg.enabled.contains(m),
                priority: Some(idx),
            })
            .collect();
        // Append any modules present in `enabled` but missing from
        // the priority chain.
        for m in ModuleId::all() {
            if !cfg.priority.contains(&m) {
                let enabled = cfg.enabled.contains(&m);
                if enabled {
                    listed.push(ModuleStatus {
                        module: m,
                        enabled,
                        priority: None,
                    });
                }
            }
        }
        listed
    }

    /// Status for a single module. Used by
    /// `GET /v1/compliance/policies/{module}`.
    pub fn module_status(&self, module: ModuleId) -> ModuleStatus {
        let cfg = self.composer_config();
        ModuleStatus {
            module,
            enabled: cfg.enabled.contains(&module),
            priority: cfg.priority.iter().position(|m| *m == module),
        }
    }

    /// Overall composer status. Used by `GET /v1/compliance/status`.
    pub fn overall_status(&self) -> OverallStatus {
        let cfg = self.composer_config();
        let reload_count = self
            .inner
            .lock()
            .expect("policy manager poisoned")
            .reload_count;
        OverallStatus {
            modules: self.list_policies(),
            priority: cfg.priority,
            reload_count,
        }
    }

    /// Replace the active composer config. Invalidates the decision
    /// cache and publishes a `PolicyChanged` audit-feed event.
    pub fn replace_config(&self, config: ComposerConfig, source: PolicySource) {
        let summary = source.summary();
        {
            let mut guard = self.inner.lock().expect("policy manager poisoned");
            guard.composer.set_config(config);
        }
        self.cache.invalidate_all();
        self.feed.publish_policy_change(None, summary);
    }

    /// Apply a [`PolicyTemplate`]. Convenience wrapper around
    /// [`Self::replace_config`].
    pub fn apply_template(&self, template: PolicyTemplate) {
        self.replace_config(template.composer_config(), PolicySource::Template(template));
    }

    /// "Reload from disk." The manager has no disk knowledge
    /// — operators pass the freshly loaded config in. This still
    /// bumps the reload counter, invalidates the cache, and publishes
    /// the `policy_changed` event so the dashboard surfaces the
    /// reload event uniformly.
    pub fn reload(&self, config: ComposerConfig) {
        {
            let mut guard = self.inner.lock().expect("policy manager poisoned");
            guard.composer.set_config(config);
            guard.reload_count = guard.reload_count.saturating_add(1);
        }
        self.cache.invalidate_all();
        self.feed
            .publish_policy_change(None, PolicySource::Reload.summary());
    }

    /// Toggle the enabled state of a single module. Returns the new
    /// state. Idempotent.
    pub fn set_module_enabled(&self, module: ModuleId, enabled: bool) -> bool {
        let changed = {
            let mut guard = self.inner.lock().expect("policy manager poisoned");
            let mut cfg = guard.composer.config().clone();
            let was = cfg.enabled.contains(&module);
            if enabled == was {
                return enabled;
            }
            if enabled {
                cfg.enabled.insert(module);
            } else {
                cfg.enabled.remove(&module);
            }
            guard.composer.set_config(cfg);
            true
        };
        if changed {
            self.cache.invalidate_all();
            self.feed.publish_module_state(module, enabled);
        }
        enabled
    }

    /// Enable a module. Equivalent to
    /// `set_module_enabled(module, true)`.
    pub fn enable_module(&self, module: ModuleId) {
        self.set_module_enabled(module, true);
    }

    /// Disable a module. Equivalent to
    /// `set_module_enabled(module, false)`.
    pub fn disable_module(&self, module: ModuleId) {
        self.set_module_enabled(module, false);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::policy::audit_feed::FeedEvent;
    use crate::policy::bundle::{ClassificationResult, PolicyBundle, RequestMetadata};
    use crate::policy::cache::DecisionKey;
    use crate::policy::composer::{AggregateDecision, Destination};
    use crate::trust::TrustContext;

    fn fresh_manager() -> PolicyManager {
        PolicyManager::new(
            PolicyComposer::default(),
            DecisionCache::default(),
            AuditFeed::new(),
        )
    }

    #[test]
    fn list_policies_returns_default_priority_order() {
        let mgr = fresh_manager();
        let listed = mgr.list_policies();
        assert_eq!(listed.len(), 3);
        assert_eq!(listed[0].module, ModuleId::Ocap);
        assert_eq!(listed[1].module, ModuleId::Itar);
        assert_eq!(listed[2].module, ModuleId::Hipaa);
        for row in &listed {
            assert!(row.enabled);
            assert!(row.priority.is_some());
        }
    }

    #[test]
    fn module_status_reflects_disabled_state() {
        let mgr = fresh_manager();
        mgr.disable_module(ModuleId::Hipaa);
        let status = mgr.module_status(ModuleId::Hipaa);
        assert!(!status.enabled);
        assert_eq!(status.priority, Some(2));
    }

    #[test]
    fn enable_module_emits_audit_event() {
        let mgr = fresh_manager();
        let sub = mgr.audit_feed().subscribe();
        mgr.disable_module(ModuleId::Itar);
        mgr.enable_module(ModuleId::Itar);
        let events = sub.drain();
        let kinds: Vec<&str> = events.iter().map(FeedEvent::kind).collect();
        assert!(kinds.contains(&"module_state_changed"));
        // Two state flips means at least two module-state events.
        let module_events = events
            .iter()
            .filter(|e| e.kind() == "module_state_changed")
            .count();
        assert_eq!(module_events, 2);
    }

    #[test]
    fn enable_is_idempotent() {
        let mgr = fresh_manager();
        let sub = mgr.audit_feed().subscribe();
        // HIPAA is enabled by default; enabling again must not emit.
        mgr.enable_module(ModuleId::Hipaa);
        assert!(sub.is_empty());
    }

    #[test]
    fn reload_increments_counter_and_clears_cache() {
        let mgr = fresh_manager();
        // Seed the cache with a fake entry to prove invalidation runs.
        let bundle = PolicyBundle {
            request: RequestMetadata {
                request_id: "r".into(),
                tenant_id: "local-dev".into(),
                timestamp_unix_ms: 0,
                source: "api".into(),
                model_hint: None,
            },
            trust: TrustContext::for_local_dev(),
            classification: ClassificationResult {
                level: "public".into(),
                matched_patterns: vec![],
                entity_count: 0,
            },
        };
        mgr.decision_cache().put(
            DecisionKey::from_bundle(&bundle),
            AggregateDecision {
                allowed: true,
                route: Some(Destination::Cloud),
                flags: Vec::new(),
                reasons: Vec::new(),
                modules_applied: Vec::new(),
            },
        );
        assert_eq!(mgr.decision_cache().len(), 1);

        mgr.reload(ComposerConfig::default());

        assert_eq!(mgr.decision_cache().len(), 0);
        assert_eq!(mgr.overall_status().reload_count, 1);
    }

    #[test]
    fn apply_template_swaps_enabled_set() {
        let mgr = fresh_manager();
        mgr.apply_template(PolicyTemplate::Defense);
        let cfg = mgr.composer_config();
        assert!(cfg.enabled.contains(&ModuleId::Itar));
        assert!(!cfg.enabled.contains(&ModuleId::Hipaa));
        assert!(!cfg.enabled.contains(&ModuleId::Ocap));
    }

    #[test]
    fn replace_config_publishes_policy_change() {
        let mgr = fresh_manager();
        let sub = mgr.audit_feed().subscribe();
        mgr.replace_config(ComposerConfig::default(), PolicySource::Manual);
        let events = sub.drain();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].kind(), "policy_changed");
    }
}
