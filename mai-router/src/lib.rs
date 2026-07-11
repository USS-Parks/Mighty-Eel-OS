//! Lamprey Query Router.
//!
//! The router is the entry point for every inference request that enters the
//! MAI compliance stack. It decides whether a query is processed locally by
//! the MAI inference engine or routed to a cloud frontier model, then emits
//! an audit-grade reason for the decision.
//!
//! Routing decisions are based on:
//!
//! - **Sensitivity classification** (regex-based, TOML-configurable)
//! - **Entity detection** (medical, tribal, export-controlled dictionaries)
//! - **Per-profile cost budget** (monthly token caps with soft/hard limits)
//! - **Fallback chain** (cloud-unreachable → local retry → denied)
//!
//! The router does **not** pick GPU instances — that is the scheduler's job
//! (see `mai-scheduler`). The router decides local vs cloud BEFORE the
//! scheduler is consulted.

#![forbid(unsafe_code)]

pub mod classifier;
pub mod cost;
pub mod entities;
pub mod pipeline;
pub mod router;
pub mod rules;

pub use classifier::{
    Classification, ClassifierConfig, RuleBasedClassifier, SensitivityClassifier,
};
pub use cost::{BudgetCheck, BudgetConfig, BudgetError, BudgetTracker};
pub use entities::{EntityDictionary, EntityKind, EntityMatch, EntityScanner};
pub use pipeline::{Pipeline, PipelineError, PipelineResult, PipelineRuleHit, StageMetrics};
pub use router::{
    CloudProvider, DefaultRouter, RouteRequest, Router, RouterConfig, RouterError, RoutingDecision,
};
pub use rules::{
    Action, AuditLevel, Condition, FactSet, ModuleError, Operator, PolicyModule,
    PolicyModuleRegistry, RerouteTarget, Rule, RuleError, RuleHit, Value, evaluate, resolve,
};
