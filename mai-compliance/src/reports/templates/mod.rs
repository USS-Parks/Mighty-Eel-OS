//! Pre-built compliance report templates.
//!
//! Five canonical templates plus a [`TemplateRegistry`] that resolves
//! [`super::engine::ReportType`] values to template instances.
//!
//! Each template implements [`ReportTemplate`]: given a
//! [`super::engine::TemplateContext`] it produces a short narrative
//! summary and a list of [`super::engine::ReportSection`] entries.
//! The engine layers the §A.13 trust + credential section on top
//! before rendering, so templates never need to assemble the trust
//! section themselves.

mod activity;
mod digest;
mod hipaa;
mod itar;
mod ocap;

use std::collections::HashMap;

use crate::policy::composer::ModuleId;

use super::engine::{ReportSection, ReportType, TemplateContext};

pub use activity::SystemActivitySummary;
pub use digest::MonthlyComplianceDigest;
pub use hipaa::HipaaAuditTrail;
pub use itar::ItarComplianceSummary;
pub use ocap::OcapGovernanceReport;

/// Behaviour every report template must implement.
pub trait ReportTemplate: Send + Sync + std::fmt::Debug {
    /// Wire identifier the template is bound to. The
    /// [`TemplateRegistry`] uses this for `ReportType::Custom`
    /// lookups; the engine records it on the metadata.
    fn report_type(&self) -> ReportType;

    /// Optional module filter the engine should apply when querying
    /// the audit log. Returning `Some(m)` narrows the row set to
    /// entries whose `modules_applied` contains `m`. Returning
    /// `None` keeps all rows in the date range.
    fn scope_module(&self) -> Option<ModuleId> {
        None
    }

    /// Build the template's narrative output: a short summary
    /// paragraph plus zero or more named sections.
    fn build(&self, ctx: TemplateContext<'_>) -> (String, Vec<ReportSection>);
}

/// Registry of templates by string id.
///
/// The five built-in templates are registered by
/// [`TemplateRegistry::with_builtin`]; downstream code can register
/// extra templates against `ReportType::Custom` ids.
#[derive(Default)]
pub struct TemplateRegistry {
    by_id: HashMap<String, Box<dyn ReportTemplate>>,
}

impl std::fmt::Debug for TemplateRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TemplateRegistry")
            .field("count", &self.by_id.len())
            .finish()
    }
}

impl TemplateRegistry {
    /// Empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Registry pre-populated with the five built-in templates.
    pub fn with_builtin() -> Self {
        let mut reg = Self::new();
        reg.register(Box::new(HipaaAuditTrail));
        reg.register(Box::new(ItarComplianceSummary));
        reg.register(Box::new(OcapGovernanceReport));
        reg.register(Box::new(SystemActivitySummary));
        reg.register(Box::new(MonthlyComplianceDigest));
        reg
    }

    /// Register a template, replacing any existing one bound to the
    /// same id.
    pub fn register(&mut self, template: Box<dyn ReportTemplate>) {
        let id = template.report_type().as_str().to_string();
        self.by_id.insert(id, template);
    }

    /// Look up a template by [`ReportType`].
    pub fn get(&self, report_type: &ReportType) -> Option<&dyn ReportTemplate> {
        self.by_id
            .get(report_type.as_str())
            .map(std::convert::AsRef::as_ref)
    }

    /// Number of registered templates.
    pub fn len(&self) -> usize {
        self.by_id.len()
    }

    /// True when no templates are registered.
    pub fn is_empty(&self) -> bool {
        self.by_id.is_empty()
    }

    /// Iterate over registered report types in arbitrary order.
    pub fn report_types(&self) -> impl Iterator<Item = ReportType> + '_ {
        self.by_id.values().map(|t| t.report_type())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_registry_has_five_templates() {
        let reg = TemplateRegistry::with_builtin();
        assert_eq!(reg.len(), 5);
        assert!(reg.get(&ReportType::HipaaAuditTrail).is_some());
        assert!(reg.get(&ReportType::ItarComplianceSummary).is_some());
        assert!(reg.get(&ReportType::OcapGovernance).is_some());
        assert!(reg.get(&ReportType::SystemActivity).is_some());
        assert!(reg.get(&ReportType::MonthlyDigest).is_some());
    }

    #[test]
    fn custom_template_resolves() {
        #[derive(Debug)]
        struct DummyTemplate;
        impl ReportTemplate for DummyTemplate {
            fn report_type(&self) -> ReportType {
                ReportType::Custom("dummy".into())
            }
            fn build(&self, _ctx: TemplateContext<'_>) -> (String, Vec<ReportSection>) {
                ("dummy".into(), vec![])
            }
        }
        let mut reg = TemplateRegistry::new();
        reg.register(Box::new(DummyTemplate));
        assert!(reg.get(&ReportType::Custom("dummy".into())).is_some());
    }
}
