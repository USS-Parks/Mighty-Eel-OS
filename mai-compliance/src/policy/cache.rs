//! Decision cache.
//!
//! [`DecisionCache`] memoises recent [`AggregateDecision`] outputs so
//! that repeated identical requests bypass the per-module evaluators.
//! It is keyed on a `blake3` hash of the decision-relevant fields of
//! the [`PolicyBundle`] (request `tenant_id`, `source`, `model_hint`;
//! classification `level` + matched pattern set; `TrustContext`
//! `claim_id`, `revocation_status`, `offline_mode`, `subject_hash`,
//! and the lexicographically sorted projection of `compliance_scopes`
//! / `allowed_routes`).
//!
//! Entries expire after a configurable TTL (default 60s). Any policy
//! change MUST call [`DecisionCache::invalidate_all`] so stale
//! decisions cannot leak across a policy reload — the policy manager
//! (see [`super::api`]) does this on every `reload` / `enable` /
//! `disable` / `update` call.
//!
//! The cache is thread-safe via an internal `Mutex`; cloning the
//! cache shares state. It does **not** spawn a background reaper —
//! eviction happens lazily on `get` / `put`. This keeps the cache
//! free of any async runtime and trivially embeddable.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use blake3::Hasher;
use serde::{Deserialize, Serialize};

use super::bundle::PolicyBundle;
use super::composer::AggregateDecision;
use crate::jurisdiction::{ActorContext, CountryCode, PersonType};

/// Default TTL for cache entries (60s).
pub const DEFAULT_TTL_SECS: u64 = 60;

/// Configuration for the decision cache.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DecisionCacheConfig {
    /// Time-to-live for cached decisions, in seconds.
    #[serde(default = "DecisionCacheConfig::default_ttl_secs")]
    pub ttl_secs: u64,
    /// Soft maximum number of entries. When exceeded, the cache
    /// evicts expired entries first; if still above the cap, the
    /// oldest live entries are dropped until under the cap.
    #[serde(default = "DecisionCacheConfig::default_max_entries")]
    pub max_entries: usize,
}

impl Default for DecisionCacheConfig {
    fn default() -> Self {
        Self {
            ttl_secs: Self::default_ttl_secs(),
            max_entries: Self::default_max_entries(),
        }
    }
}

impl DecisionCacheConfig {
    /// Default TTL (60 seconds).
    pub fn default_ttl_secs() -> u64 {
        DEFAULT_TTL_SECS
    }

    /// Default soft cap (1024 entries).
    pub fn default_max_entries() -> usize {
        1024
    }
}

/// 32-byte content-addressed key derived from a [`PolicyBundle`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct DecisionKey([u8; 32]);

impl DecisionKey {
    /// Key for a bundle with no distinct actor context (the actor defaults to
    /// unknown). Two bundles that differ only in request id, timestamp, or other
    /// non-decision-relevant fields hash to the same key. Prefer
    /// [`Self::from_bundle_and_actor`] on any path where the jurisdiction actor
    /// (country / person type / deployment profile) influences the decision, so
    /// two identical bundles from different actors do not collide (audit G6).
    pub fn from_bundle(bundle: &PolicyBundle) -> Self {
        Self::from_bundle_and_actor(bundle, &ActorContext::default())
    }

    /// Key for a bundle **plus the requesting actor**. The actor's country,
    /// person type, and deployment profile are folded in, so a decision that
    /// varies by jurisdiction is cached per actor and never shared across them
    /// (audit G6). This is the constructor to use on any actor-aware path.
    pub fn from_bundle_and_actor(bundle: &PolicyBundle, actor: &ActorContext) -> Self {
        let mut h = Hasher::new();
        Self::hash_bundle(&mut h, bundle);
        h.update(b"|actor.country=");
        h.update(
            actor
                .country
                .as_ref()
                .map_or("", CountryCode::as_str)
                .as_bytes(),
        );
        h.update(b"|actor.person=");
        h.update(Self::person_type_tag(actor.person_type).as_bytes());
        h.update(b"|actor.profile=");
        h.update(actor.deployment_profile.as_deref().unwrap_or("").as_bytes());
        Self(*h.finalize().as_bytes())
    }

    /// Stable tag for a person type (matches the serde snake_case wire form).
    fn person_type_tag(p: PersonType) -> &'static str {
        match p {
            PersonType::UsPerson => "us_person",
            PersonType::NonUsPerson => "non_us_person",
            PersonType::Unknown => "unknown",
        }
    }

    /// Fold the decision-relevant projection of a bundle into `h`.
    fn hash_bundle(h: &mut Hasher, bundle: &PolicyBundle) {
        let r = &bundle.request;
        h.update(b"req.tenant=");
        h.update(r.tenant_id.as_bytes());
        h.update(b"|req.source=");
        h.update(r.source.as_bytes());
        h.update(b"|req.model=");
        h.update(r.model_hint.as_deref().unwrap_or("").as_bytes());

        let c = &bundle.classification;
        h.update(b"|cls.level=");
        h.update(c.level.as_bytes());
        let mut patterns = c.matched_patterns.clone();
        patterns.sort();
        h.update(b"|cls.patterns=");
        for p in &patterns {
            h.update(b"\x1f");
            h.update(p.as_bytes());
        }
        h.update(b"|cls.entities=");
        h.update(&c.entity_count.to_le_bytes());

        let t = &bundle.trust;
        h.update(b"|trust.claim=");
        h.update(t.claim_id.as_bytes());
        h.update(b"|trust.bundle=");
        h.update(t.trust_bundle_version.as_bytes());
        h.update(b"|trust.subject=");
        h.update(t.subject_hash.as_str().as_bytes());
        h.update(b"|trust.revocation=");
        h.update(t.revocation_status.as_str().as_bytes());
        h.update(b"|trust.offline=");
        h.update(&[u8::from(t.offline_mode())]);
        let mut scopes: Vec<&str> = t.compliance_scopes.iter().map(|s| s.as_str()).collect();
        scopes.sort_unstable();
        h.update(b"|trust.scopes=");
        for s in scopes {
            h.update(b"\x1f");
            h.update(s.as_bytes());
        }
        let mut routes: Vec<&str> = t.allowed_routes.iter().map(|r| r.as_str()).collect();
        routes.sort_unstable();
        h.update(b"|trust.routes=");
        for r in routes {
            h.update(b"\x1f");
            h.update(r.as_bytes());
        }
    }

    /// Return the raw 32-byte digest (hex-encoded by callers that
    /// need a string form).
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

#[derive(Debug, Clone)]
struct Entry {
    decision: AggregateDecision,
    expires_at: Instant,
    inserted_at: Instant,
}

#[derive(Debug)]
struct Inner {
    config: DecisionCacheConfig,
    entries: HashMap<DecisionKey, Entry>,
    hits: u64,
    misses: u64,
}

/// Lazy TTL cache for [`AggregateDecision`] values, keyed on the
/// decision-relevant projection of a [`PolicyBundle`].
#[derive(Debug, Clone)]
pub struct DecisionCache {
    inner: Arc<Mutex<Inner>>,
}

impl Default for DecisionCache {
    fn default() -> Self {
        Self::new(DecisionCacheConfig::default())
    }
}

impl DecisionCache {
    /// Build an empty cache with the given configuration.
    pub fn new(config: DecisionCacheConfig) -> Self {
        Self {
            inner: Arc::new(Mutex::new(Inner {
                config,
                entries: HashMap::new(),
                hits: 0,
                misses: 0,
            })),
        }
    }

    /// Active configuration (cloned so callers don't hold the lock).
    pub fn config(&self) -> DecisionCacheConfig {
        self.inner
            .lock()
            .expect("decision cache poisoned")
            .config
            .clone()
    }

    /// Replace the active configuration. Existing entries keep their
    /// original expiry; only future inserts use the new TTL. Callers
    /// that want a clean slate should call
    /// [`DecisionCache::invalidate_all`] afterwards.
    pub fn set_config(&self, config: DecisionCacheConfig) {
        self.inner.lock().expect("decision cache poisoned").config = config;
    }

    /// Look up a decision. Returns `None` on miss or expiry, updating
    /// the hit/miss counters either way.
    pub fn get(&self, key: &DecisionKey) -> Option<AggregateDecision> {
        let mut guard = self.inner.lock().expect("decision cache poisoned");
        let now = Instant::now();
        let live_decision = match guard.entries.get(key) {
            Some(entry) if entry.expires_at > now => Some(entry.decision.clone()),
            _ => None,
        };
        if live_decision.is_some() {
            guard.hits += 1;
            return live_decision;
        }
        // Either missing or expired; drop any expired entry and bump misses.
        guard.entries.remove(key);
        guard.misses += 1;
        None
    }

    /// Insert (or replace) a decision under `key`. The new entry's
    /// expiry is computed from `Instant::now() + ttl`.
    pub fn put(&self, key: DecisionKey, decision: AggregateDecision) {
        let mut guard = self.inner.lock().expect("decision cache poisoned");
        let now = Instant::now();
        let ttl = Duration::from_secs(guard.config.ttl_secs);
        let entry = Entry {
            decision,
            expires_at: now + ttl,
            inserted_at: now,
        };
        guard.entries.insert(key, entry);
        if guard.entries.len() > guard.config.max_entries {
            Self::evict_locked(&mut guard, now);
        }
    }

    /// Drop every cached entry. MUST be called by the policy manager
    /// on any rule / config change that can affect future decisions.
    pub fn invalidate_all(&self) {
        let mut guard = self.inner.lock().expect("decision cache poisoned");
        guard.entries.clear();
    }

    /// Number of live (non-expired) entries.
    pub fn len(&self) -> usize {
        let guard = self.inner.lock().expect("decision cache poisoned");
        let now = Instant::now();
        guard
            .entries
            .values()
            .filter(|e| e.expires_at > now)
            .count()
    }

    /// `true` when there are no live entries.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// `(hits, misses)` since cache construction.
    pub fn stats(&self) -> (u64, u64) {
        let guard = self.inner.lock().expect("decision cache poisoned");
        (guard.hits, guard.misses)
    }

    fn evict_locked(inner: &mut Inner, now: Instant) {
        // Drop expired entries first.
        inner.entries.retain(|_, e| e.expires_at > now);
        if inner.entries.len() <= inner.config.max_entries {
            return;
        }
        // Still over cap — drop oldest by `inserted_at` until under.
        let mut by_age: Vec<(DecisionKey, Instant)> = inner
            .entries
            .iter()
            .map(|(k, e)| (*k, e.inserted_at))
            .collect();
        by_age.sort_by_key(|(_, t)| *t);
        let to_drop = inner.entries.len() - inner.config.max_entries;
        for (k, _) in by_age.into_iter().take(to_drop) {
            inner.entries.remove(&k);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::jurisdiction::{ActorContext, CountryCode, PersonType};
    use crate::policy::bundle::{ClassificationResult, PolicyBundle, RequestMetadata};
    use crate::trust::TrustContext;

    fn sample_bundle(request_id: &str) -> PolicyBundle {
        PolicyBundle {
            request: RequestMetadata {
                request_id: request_id.to_string(),
                tenant_id: "local-dev".to_string(),
                timestamp_unix_ms: 1_700_000_000_000,
                source: "api".to_string(),
                model_hint: Some("llama-3-70b".to_string()),
            },
            trust: TrustContext::for_local_dev(),
            classification: ClassificationResult {
                level: "regulated".to_string(),
                matched_patterns: vec!["ssn".to_string()],
                entity_count: 1,
            },
        }
    }

    fn empty_decision() -> AggregateDecision {
        AggregateDecision {
            allowed: true,
            route: Some(super::super::composer::Destination::Cloud),
            flags: Vec::new(),
            reasons: Vec::new(),
            modules_applied: Vec::new(),
        }
    }

    #[test]
    fn key_is_stable_across_clones() {
        let a = sample_bundle("r1");
        let b = sample_bundle("r1");
        assert_eq!(DecisionKey::from_bundle(&a), DecisionKey::from_bundle(&b));
    }

    #[test]
    fn key_differs_by_actor_country_person_and_profile() {
        // Audit G6: an identical bundle from a different actor must not collide.
        let bundle = sample_bundle("r1");
        let us = ActorContext {
            country: Some(CountryCode::us()),
            person_type: PersonType::UsPerson,
            deployment_profile: None,
        };
        let foreign = ActorContext {
            country: Some(CountryCode::new("FR").unwrap()),
            person_type: PersonType::NonUsPerson,
            deployment_profile: None,
        };
        let k_us = DecisionKey::from_bundle_and_actor(&bundle, &us);
        assert_ne!(
            k_us,
            DecisionKey::from_bundle_and_actor(&bundle, &foreign),
            "different country + person must not collide"
        );
        // Stable for the same (bundle, actor).
        assert_eq!(k_us, DecisionKey::from_bundle_and_actor(&bundle, &us));
        // person_type alone distinguishes.
        let us_unknown = ActorContext {
            person_type: PersonType::Unknown,
            ..us.clone()
        };
        assert_ne!(
            k_us,
            DecisionKey::from_bundle_and_actor(&bundle, &us_unknown)
        );
        // deployment_profile alone distinguishes.
        let us_defense = ActorContext {
            deployment_profile: Some("defense".to_string()),
            ..us.clone()
        };
        assert_ne!(
            k_us,
            DecisionKey::from_bundle_and_actor(&bundle, &us_defense)
        );
    }

    #[test]
    fn key_ignores_request_id_and_timestamp() {
        let mut a = sample_bundle("r1");
        let b = {
            let mut x = a.clone();
            x.request.request_id = "different".into();
            x.request.timestamp_unix_ms += 12345;
            x
        };
        // Touch unrelated trust fields too — should not change the key.
        a.request.request_id = "r1".into();
        assert_eq!(DecisionKey::from_bundle(&a), DecisionKey::from_bundle(&b));
    }

    #[test]
    fn key_changes_on_classification_level() {
        let a = sample_bundle("r1");
        let mut b = a.clone();
        b.classification.level = "critical".into();
        assert_ne!(DecisionKey::from_bundle(&a), DecisionKey::from_bundle(&b));
    }

    #[test]
    fn key_is_pattern_order_independent() {
        let mut a = sample_bundle("r1");
        a.classification.matched_patterns = vec!["x".into(), "y".into()];
        let mut b = a.clone();
        b.classification.matched_patterns = vec!["y".into(), "x".into()];
        assert_eq!(DecisionKey::from_bundle(&a), DecisionKey::from_bundle(&b));
    }

    #[test]
    fn put_and_get_hits_within_ttl() {
        let cache = DecisionCache::default();
        let bundle = sample_bundle("r1");
        let key = DecisionKey::from_bundle(&bundle);
        assert!(cache.get(&key).is_none());
        cache.put(key, empty_decision());
        assert!(cache.get(&key).is_some());
        let (hits, misses) = cache.stats();
        assert_eq!(hits, 1);
        assert_eq!(misses, 1);
    }

    #[test]
    fn entries_expire_after_ttl() {
        let cache = DecisionCache::new(DecisionCacheConfig {
            ttl_secs: 0,
            max_entries: 16,
        });
        let bundle = sample_bundle("r1");
        let key = DecisionKey::from_bundle(&bundle);
        cache.put(key, empty_decision());
        // 0-second TTL → already expired when we look it up.
        std::thread::sleep(Duration::from_millis(5));
        assert!(cache.get(&key).is_none());
        assert!(cache.is_empty());
    }

    #[test]
    fn invalidate_all_clears_cache() {
        let cache = DecisionCache::default();
        // Vary model_hint so each entry hashes to a distinct key
        // (the cache key intentionally ignores request_id).
        for i in 0..4 {
            let mut b = sample_bundle("r");
            b.request.model_hint = Some(format!("model-{i}"));
            cache.put(DecisionKey::from_bundle(&b), empty_decision());
        }
        assert_eq!(cache.len(), 4);
        cache.invalidate_all();
        assert!(cache.is_empty());
    }

    #[test]
    fn max_entries_caps_size() {
        let cache = DecisionCache::new(DecisionCacheConfig {
            ttl_secs: 60,
            max_entries: 2,
        });
        // Insert 4 distinct entries (different model_hint values).
        for i in 0..4 {
            let mut b = sample_bundle("r1");
            b.request.model_hint = Some(format!("model-{i}"));
            cache.put(DecisionKey::from_bundle(&b), empty_decision());
        }
        assert!(cache.len() <= 2);
    }
}
