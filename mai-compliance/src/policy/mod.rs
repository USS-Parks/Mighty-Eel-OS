//! Policy runtime.
//!
//! The runtime coordinates the per-domain compliance modules (HIPAA,
//! ITAR/EAR, OCAP) and folds their independent verdicts into a single
//! aggregate. Submodules:
//!
//! - [`bundle`] — unified decision input ([`PolicyBundle`]:
//!   [`RequestMetadata`] + [`crate::trust::TrustContext`] +
//!   [`ClassificationResult`]).
//! - [`composer`] — conflict-resolution engine. Any deny wins; the
//!   most-restrictive route wins; flags accumulate. Default priority
//!   chain `OCAP > ITAR > HIPAA` is configurable from
//!   `config/compliance/policy.toml`.
//! - [`cache`] — content-addressed TTL cache for repeated decisions.
//! - [`templates`] — pre-built vertical profiles (`standard`,
//!   `healthcare`, `defense`, `tribal_government`).
//! - [`audit_feed`] — in-process broadcast channel for compliance
//!   events. The eventual SSE endpoint in `mai-api` wraps a subscriber.
//! - [`api`] — typed runtime-management surface ([`PolicyManager`])
//!   that backs the compliance HTTP endpoints in `mai-api`.

pub mod api;
pub mod audit_feed;
pub mod bundle;
pub mod cache;
pub mod composer;
pub mod templates;

pub use api::{ModuleStatus, OverallStatus, PolicyManager, PolicySource};
pub use audit_feed::{AuditFeed, DEFAULT_BUFFER_CAPACITY, FeedEvent, FeedSubscriber};
pub use bundle::{ClassificationResult, PolicyBundle, PolicyBundleError, RequestMetadata};
pub use cache::{DEFAULT_TTL_SECS, DecisionCache, DecisionCacheConfig, DecisionKey};
pub use composer::{
    AggregateDecision, ComplianceFlag, ComplianceReason, ComposerConfig, Destination,
    ModuleDecision, ModuleId, PolicyComposer,
};
pub use templates::{PolicyTemplate, TemplateVersion};
