//! Composable compliance rule engine.
//!
//! Provides three layers:
//!
//! - `engine`: low-level `Rule` / `Condition` / `Action` types and
//!   evaluation against a per-request `FactSet`.
//! - `modules`: named groups of rules (`PolicyModule`) with a registry
//!   that supports hot-reload and runtime enable/disable.
//! - The pipeline (see `crate::pipeline`) wires the registry into a full
//!   request → decision flow.

pub mod engine;
pub mod modules;

pub use engine::{
    Action, AuditLevel, Condition, FactSet, Operator, RerouteTarget, Rule, RuleError, RuleHit,
    Value, evaluate, resolve,
};
pub use modules::{ModuleError, PolicyModule, PolicyModuleRegistry};
