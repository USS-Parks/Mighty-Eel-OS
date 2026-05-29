# Acquirer Integration Guide

**Project:** Island Mountain Model Abstraction Interface (MAI) +
Lamprey
**Audience:** Acquirer engineering teams evaluating embed
feasibility, platform-architecture leads
**Status:** Session 45 acquisition documentation
**Last Updated:** 2026-05-23

This guide is for the acquirer's engineering team. It assumes the
diligence team has already read
[`../ACQUISITION-PACKAGE.md`](../product/ACQUISITION-PACKAGE.md) (positioning),
[`ARCHITECTURE.md`](ARCHITECTURE.md) (system shape), and
[`../BUYER-INTEGRATION-GUIDE.md`](../product/BUYER-INTEGRATION-GUIDE.md) (the
seven-step buyer onboarding). What follows is the deeper "how do we
embed this into our own product" reference — custom policy module
development, audit-export shapes, configuration semantics, and the
boundary contract.

For per-domain demos see [`demos/`](demos/).

---

## Three acquisition shapes

The integration approach depends on what you bought:

### Shape A — Lamprey only

You take `mai-compliance/` and `mai-sdk-python/` and run them above
your existing inference stack. MAI scheduler and HIL are not in
scope.

- **What you write:** A bridge that calls
  `mai_compliance::policy::PolicyManager::compose(...)` before your
  existing placement, and writes the resulting `AggregateDecision`
  into your existing logging.
- **What you keep:** Your inference runtime, your model registry,
  your placement engine.
- **What you drop:** Whatever ad-hoc compliance code you had.

### Shape B — Lamprey + MAI scheduler

You take the compliance + scheduler crates and rebuild the
inference adapter layer to call your existing model serving.

- **What you write:** Adapters that implement the `Adapter` trait
  in `mai-adapters/` and bridge to your model serving.
- **What you keep:** Your model registry, model storage, hardware
  fleet.
- **What you drop:** Your placement logic; MAI scheduler owns
  placement.

### Shape C — Full MAI + Lamprey

You take the entire repository and deploy as-is, possibly with
custom policy templates and your own SDK wrappers.

- **What you write:** Custom policy templates, deployment
  profile(s), perhaps acquirer-branded dashboard themes.
- **What you keep:** Trust-bridge OpenBao deployment, IdP, SIEM,
  hardware.
- **What you drop:** Building any of the above from scratch.

The rest of this guide assumes shape A or B (the more invasive
shapes). For shape C, the buyer integration guide is more
directly applicable.

---

## Embedding Lamprey above an existing inference stack

The Lamprey compose call looks like this:

```rust
use mai_compliance::policy::{PolicyManager, ComposeInput};
use mai_compliance::trust::TrustContext;

let manager: PolicyManager = /* initialised once at startup */;
let trust: TrustContext = /* built from your IdP claim */;

let decision = manager.compose(ComposeInput {
    request_metadata: req.into(),
    trust_context: trust,
    connectivity: my_connectivity_reader.state(),
    classification: my_classifier.classify(&req),
})?;

match decision.aggregate {
    Aggregate::Allow => proceed_to_placement(req),
    Aggregate::LocalOnly { reasons } => proceed_local_only(req, reasons),
    Aggregate::Quarantine { reasons } => quarantine(req, reasons),
    Aggregate::Deny { reasons } => refuse(req, reasons),
}
```

The Lamprey audit log writes asynchronously; the call returns the
decision before the audit row is persisted. The audit entry's
`mai_request_id` field is filled in by the caller post-decision so
that the audit row joins on whatever request-ID convention the
acquirer already uses.

### Classification supply

`ClassificationResult` is the bridge between your existing
classifier (or Lamprey's default classifier) and the policy modules.
Shape:

```rust
pub struct ClassificationResult {
    pub phi_detected: bool,
    pub phi_entities: Vec<PhiEntity>,
    pub controlled_tech_data: bool,
    pub controlled_tech_indicators: Vec<TechDataIndicator>,
    pub ocap_metadata: Option<OcapMetadata>,
    pub sensitivity: SensitivityClass,
    pub raw_evidence: serde_json::Value,
}
```

You can:

- Use Lamprey's default classifier (`mai-compliance` ships pattern
  dictionaries — see [`IP.md`](IP.md) trade-secret section).
- Wire your own classifier (NeMo, Guardrails AI, a fine-tuned BERT
  model, a third-party PHI detector) and feed its output into
  `ClassificationResult`.
- Combine — your classifier for general PII / toxicity, Lamprey's
  pattern dictionaries for HIPAA / ITAR / OCAP specifics.

The composer doesn't care where the classification came from; it
treats `ClassificationResult` as evidence and lets each module
decide what the evidence means in its domain.

---

## Custom policy module development

The four shipping modules (HIPAA, ITAR, EAR, OCAP) cover the bulk
of regulated-AI demand. For verticals with unique rules (PCI-DSS,
GDPR, SOX, FERPA, CJIS), an acquirer can add a custom module.

### Module trait

```rust
pub trait ComplianceModule: Send + Sync {
    fn name(&self) -> &'static str;
    fn version(&self) -> ModuleVersion;
    fn evaluate(
        &self,
        request: &RequestMetadata,
        trust: &TrustContext,
        classification: &ClassificationResult,
    ) -> Result<ModuleDecision, ModuleError>;
}
```

A module returns a `ModuleDecision`:

```rust
pub enum ModuleDecision {
    Allow { reasons: Vec<ComplianceReason>, flags: Vec<ComplianceFlag> },
    LocalOnly { reasons: Vec<ComplianceReason>, flags: Vec<ComplianceFlag> },
    Deny { reasons: Vec<ComplianceReason>, flags: Vec<ComplianceFlag> },
    Quarantine { reasons: Vec<ComplianceReason>, flags: Vec<ComplianceFlag> },
}
```

The composer normalises these per the deny-wins / most-restrictive-
route fold described in [`../LAMPREY-BRIEF.md`](../product/LAMPREY-BRIEF.md).

### Registering a custom module

```rust
let mut manager = PolicyManager::new(config)?;
manager.register_module(Box::new(MyPciDssModule::new(pci_config)))?;
manager.apply_template(Template::Custom("pci-dss-financial"));
```

The custom module participates in conflict resolution per the
default precedence chain; if the acquirer needs a different
precedence, they call:

```rust
manager.set_precedence_chain(&[
    ModuleId::Ocap,
    ModuleId::Itar,
    ModuleId::Custom("pci-dss-financial"),
    ModuleId::Hipaa,
]);
```

---

## Audit log export

The audit chain stays local. What ships to a SIEM is the metadata
side of `CorrelationFields`, which matches §A.9 of the build plan:

```json
{
  "credential_event_id": "cred_evt_123",
  "lamprey_decision_id": "dec_456",
  "mai_request_id": "req_789",
  "tenant": "your-tenant",
  "subject_hash": "hmac:...",
  "service_identity": "lamprey-router",
  "policy_version": "2026.05.22.001",
  "trust_bundle_version": "2026.05.22.001",
  "decision": "local_only_allowed",
  "decided_at": "2026-05-22T23:14:51Z",
  "reasons": ["hipaa.phi_detected", "trust.scope_check_passed"],
  "module_versions": {"hipaa": "1.4", "ocap": "1.2", "itar": "1.0"}
}
```

The offline correlation queue (`mai-compliance/src/audit/store.rs`,
BF-5) holds up to 4096 events when the SIEM endpoint is
unreachable, with a drop counter and an alert trigger at warn /
critical thresholds.

### SIEM bridge implementations

Lamprey ships no built-in SIEM exporters. The integration points
are:

| Surface | What you implement |
|---|---|
| `CorrelationSink` trait | `record(events: &[CorrelationFields])` — push to your destination |
| `/v1/compliance/audit` GET | Pull-based polling against the API |
| `/v1/compliance/feed` SSE | Live event stream for low-latency forwarders |

Most acquirers wire a `CorrelationSink` implementation that calls
their Splunk HEC / Datadog logs / Sumo Logic ingest. Sample shapes
land in `mai-compliance/src/audit/api.rs` and the relevant pages of
the dashboard's audit view.

---

## Configuration semantics

The four shipped deployment profiles
(`mai/deployment/{local-dev,cloud-trust-core,local-mai-node,airgap-demo}/`)
each carry a `profile.toml` selecting the major axes:

| Key | Type | Values |
|---|---|---|
| `trust.mode` | enum | `local-dev` / `live-openbao` / `local-cache` / `airgap` |
| `trust.bridge_url` | URL | https URL of acquirer's Lamprey Trust Bridge |
| `trust.cache_path` | path | local trust cache state directory |
| `trust.refresh_interval_seconds` | int | bundle refresh cadence |
| `compliance.template` | enum + custom | `Standard` / `Healthcare` / `Defense` / `TribalGovernment` / `custom:<name>` |
| `compliance.modules` | list | override which modules are active |
| `airgap.enabled` | bool | air-gap mode hard-on |
| `airgap.switch_path` | path | hardware switch reader path |
| `routes.cloud_allowed` | bool | global cloud-route permission |
| `audit.retention_days` | int | per-type retention overrides |
| `audit.wal_path` | path | JSON-lines WAL directory |
| `audit.sink.url` | URL | SIEM correlation endpoint |

Profile values can be overridden by env (`MAI_COMPLIANCE_TEMPLATE`,
`MAI_TRUST_BRIDGE_URL`, etc.) and by CLI flag on `mai-api` startup.

The `profile.toml` is the single source of truth at deployment time;
do not edit code defaults to express deployment differences.

---

## Boundary contract checklist

The buyer integration guide ships a
[`../BUYER-INTEGRATION-GUIDE.md`](../product/BUYER-INTEGRATION-GUIDE.md)
"Boundary contract review checklist." For an acquirer's embed
review, add these acquirer-specific items:

- [ ] **Vendoring strategy.** Cargo workspace requires Rust
      stable. If you vendor, vendor the entire workspace —
      `mai-compliance` has compile-time dependencies on `mai-core`,
      `mai-vault`, and tokio runtime traits.
- [ ] **Crypto provider replacement.** ML-DSA-87 ships via a known
      pluggable backend (see `mai-compliance::bundle::Signer` and
      `mai-vault::pqc`). Document any FIPS-mode swap in your build.
- [ ] **Pattern dictionary updates.** Plan a quarterly cadence for
      reviewing `medical_entities.rs`, `phi.rs`, `tech_data.rs`,
      and `ocap/cultural.rs`. Regulator interpretations drift; the
      modules don't.
- [ ] **Trade-secret protection regime.** Pattern dictionaries are
      proprietary IP. Access to the `mai-compliance/src/` tree
      should be NDA-gated for non-engineering staff.
- [ ] **Trust bridge ownership.** The bridge runs on your infra and
      under your IAM. Document who has Transit-key sign privileges.
- [ ] **SIEM data classification.** Even the metadata-only audit
      stream contains tenant + subject_hash; document data-protection
      classification for the SIEM sink.
- [ ] **Audit retention vs litigation hold.** The retention engine
      (`mai-compliance/src/reports/prune.rs`) honours
      `protected = true` records, but the prune workflow needs an
      operational tie-in to your legal-hold process.

---

## Build / test surface

Standing-up integration tests on the acquirer's side:

```powershell
# Workspace lib + unit tests (1196+ green)
cargo test --workspace --lib

# Compliance-specific (326+ green)
cargo test -p mai-compliance --lib

# mai-api integration (17 green covering BF-6 + S44 surface)
cargo test -p mai-api --test compliance_integration

# Python SDK (94 green)
cd mai-sdk-python; pytest

# Reference scaffolds (61 green; one app at a time per
# CLAUDE.md disk + collision constraints)
PYTHONPATH=mai-sdk-python/src python -m pytest apps/openbao-trust-demo/tests/
PYTHONPATH=mai-sdk-python/src python -m pytest apps/compliance-routed/tests/
PYTHONPATH=mai-sdk-python/src python -m pytest apps/tribal-sovereignty/tests/
PYTHONPATH=mai-sdk-python/src python -m pytest apps/operator/tests/
PYTHONPATH=mai-sdk-python/src python -m pytest apps/local-secure-inference/tests/
PYTHONPATH=mai-sdk-python/src python -m pytest apps/rag-reference/tests/

# Dashboard
cd compliance-dashboard; pytest

# Integrity check before merging acquirer changes
mai/.integrity/scripts/verify-tree.sh
```

A clean run of the above is the "green light" gate for an
acquirer's embed PR.

---

## What to ask the source maintainer (us)

When an acquirer's engineering team starts the embed work, useful
clarifications:

1. Custom policy module: do you need help wiring your domain
   (PCI / GDPR / SOX / FERPA / CJIS)? We can provide a starter
   template and pattern dictionary.
2. SIEM bridge: do you need a Splunk HEC / Datadog logs / Sumo
   Logic / Elastic SIEM helper? We have reference shapes.
3. Custom dashboard theming: the FastAPI app is straightforward to
   reskin; do you need a packaging guide?
4. Trust bridge implementation: if your OpenBao deployment differs
   from the assumed shape, we can pair on the bridge handler.
5. Hardware air-gap switch reader: if your hardware doesn't expose
   the switch state at the path the default reader expects, we can
   wire your reader.
6. Trade-secret rotation: cadence and contents of the next pattern
   dictionary updates.

These are the conversations that turn the diligence verification
into a smooth production embed.
