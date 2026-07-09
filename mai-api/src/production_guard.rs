//! Production readiness guard.
//!
//! Centralised, fail-closed contract scanner that runs over a parsed
//! [`ShipProfile`] and reports every violation against the SHIP-HARDENING-PLAN.md
//! §6 (Workstream 4) check list. One report → one ID per violation →
//! one remediation hint per violation.
//!
//! Scope:
//! - Define [`ProductionReadinessReport`], [`ProductionCheck`],
//!   [`CheckSeverity`], [`CheckStatus`].
//! - Implement the config-only checks: anything decidable from the
//!   profile struct alone.
//! - Register the runtime-only checks (vault open, audit append round
//!   trip, trust bundle verify, etc.) with `CheckStatus::Deferred`.
//!   Convergence adds [`RuntimeChecks`] and
//!   [`ProductionReadinessReport::evaluate_with_runtime`] so each
//!   deferred ID flips from Deferred to Pass / Fail when the server
//!   bootstrap supplies an introspection result.
//!
//! Out of scope (still):
//! - HTTP endpoint `GET /v1/system/production-readiness`. The
//!   standalone `mai-ship-validate` binary and the admin endpoint land
//!   alongside the packaging workstream.
//!
//! Check ID convention:
//! - `PROD-{AREA}-NNN` where AREA is CONFIG / PATHS / VAULT / AUDIT /
//!   TRUST / AUTH / DASH / NET / POLICY / OBS.
//! - IDs are stable. New checks append; existing IDs never get reused
//!   or renumbered. Operators wire alerts against the ID.
//! - IDs ≥ 100 are runtime checks; IDs < 100 are config-only.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::ship_profile::{AuditWriter, ProfileMode, ShipProfile, TrustVerifier, VaultBackend};

/// Severity tag attached to every [`ProductionCheck`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CheckSeverity {
    /// Non-blocking. Useful for surfaces like "deferred to a later step".
    Info,
    /// Should be addressed but does not by itself block ship-readiness.
    Warning,
    /// Blocks ship-readiness. Any [`CheckStatus::Fail`] at this
    /// severity flips [`ProductionReadinessReport::is_ship_ready`].
    Critical,
}

/// Outcome of a single [`ProductionCheck`] evaluation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CheckStatus {
    /// The check ran and the contract holds.
    Pass,
    /// The check ran and the contract is violated.
    Fail,
    /// The check is defined but cannot be evaluated yet — typically
    /// because it requires runtime introspection that lands in a
    /// later SHIP session. The `message` should name the session
    /// that will close it.
    Deferred,
    /// The check is not applicable to this profile mode (e.g. a
    /// production-only check evaluated against `local-dev`).
    Skipped,
}

/// One check in the readiness report.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProductionCheck {
    pub id: String,
    pub severity: CheckSeverity,
    pub status: CheckStatus,
    pub message: String,
    pub remediation: String,
}

/// Complete production-readiness report. Produced by
/// [`ProductionReadinessReport::evaluate`] over a [`ShipProfile`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProductionReadinessReport {
    pub profile: String,
    pub mode: ProfileMode,
    pub checks: Vec<ProductionCheck>,
}

impl ProductionReadinessReport {
    /// Evaluate every registered check against `profile` and return
    /// the full report. Never panics; never short-circuits — the
    /// caller decides what to do with the failures.
    pub fn evaluate(profile: &ShipProfile) -> Self {
        let mut ctx = CheckContext::new(profile);
        register_all_checks(&mut ctx);
        ProductionReadinessReport {
            profile: profile.profile.name.clone(),
            mode: profile.profile.mode,
            checks: ctx.checks,
        }
    }

    /// True when no Critical check is in [`CheckStatus::Fail`].
    /// Deferred and Skipped checks do not block ship-readiness — they
    /// surface known gaps for the operator without being false
    /// negatives.
    pub fn is_ship_ready(&self) -> bool {
        !self
            .checks
            .iter()
            .any(|c| c.severity == CheckSeverity::Critical && c.status == CheckStatus::Fail)
    }

    /// Number of checks in each status (PASS / FAIL / DEFERRED / SKIPPED).
    pub fn counts(&self) -> ReadinessCounts {
        let mut counts = ReadinessCounts::default();
        for c in &self.checks {
            match c.status {
                CheckStatus::Pass => counts.pass += 1,
                CheckStatus::Fail => counts.fail += 1,
                CheckStatus::Deferred => counts.deferred += 1,
                CheckStatus::Skipped => counts.skipped += 1,
            }
        }
        counts
    }

    /// Iterator over failing checks, in registration order.
    pub fn failures(&self) -> impl Iterator<Item = &ProductionCheck> {
        self.checks.iter().filter(|c| c.status == CheckStatus::Fail)
    }

    /// Iterator over deferred checks (runtime checks waiting on later
    /// SHIP sessions).
    pub fn deferred(&self) -> impl Iterator<Item = &ProductionCheck> {
        self.checks
            .iter()
            .filter(|c| c.status == CheckStatus::Deferred)
    }

    /// Find a check by ID. Used heavily in tests.
    pub fn find(&self, id: &str) -> Option<&ProductionCheck> {
        self.checks.iter().find(|c| c.id.as_str() == id)
    }

    /// Evaluate every registered check against `profile`, then upgrade
    /// each deferred runtime check using `runtime`. When a runtime
    /// field is `Some`, the corresponding `PROD-*-100/101` check flips
    /// from Deferred to Pass or Fail and its message is replaced with
    /// the supplied detail. Fields left `None` stay Deferred so the
    /// report still names the gap.
    ///
    /// The convergence wires this into `MaiServer::run()` so
    /// production startup fails closed on any flipped Critical Fail.
    pub fn evaluate_with_runtime(profile: &ShipProfile, runtime: &RuntimeChecks) -> Self {
        let mut report = Self::evaluate(profile);
        report.apply_runtime(runtime);
        report
    }

    /// Mutate an existing report in place by applying `runtime`. Used
    /// by [`Self::evaluate_with_runtime`] and the readiness
    /// endpoint where the config-only pass already ran.
    pub fn apply_runtime(&mut self, runtime: &RuntimeChecks) {
        let mut apply = |id: &str, outcome: Option<&RuntimeOutcome>| {
            let Some(outcome) = outcome else { return };
            for check in &mut self.checks {
                if check.id.as_str() != id {
                    continue;
                }
                if check.status != CheckStatus::Deferred {
                    return;
                }
                check.status = if outcome.passed {
                    CheckStatus::Pass
                } else {
                    CheckStatus::Fail
                };
                check.message = outcome.detail.clone();
                return;
            }
        };
        apply("PROD-VAULT-100", runtime.vault_opened.as_ref());
        apply("PROD-AUDIT-100", runtime.api_audit_wal_ready.as_ref());
        apply("PROD-AUDIT-101", runtime.compliance_sealer_real.as_ref());
        apply("PROD-TRUST-100", runtime.trust_bundle_verified.as_ref());
        apply("PROD-AUTH-100", runtime.auth_keys_nonempty.as_ref());
        apply(
            "PROD-AUTH-101",
            runtime.auth_internal_bypass_consistent.as_ref(),
        );
        apply("PROD-POLICY-001", runtime.policy_modules_loaded.as_ref());
    }

    /// Render the report in a human-readable form suitable for the
    /// `mai-api validate` CLI default output.
    pub fn render_human(&self) -> String {
        let counts = self.counts();
        let header = if self.is_ship_ready() {
            "MAI Production Readiness: PASS"
        } else {
            "MAI Production Readiness: FAIL"
        };
        let mut out = String::new();
        out.push_str(header);
        out.push('\n');
        out.push_str(&format!(
            "Profile: {} (mode={:?})\n",
            self.profile, self.mode
        ));
        out.push_str(&format!(
            "Checks: {} pass / {} fail / {} deferred / {} skipped\n\n",
            counts.pass, counts.fail, counts.deferred, counts.skipped
        ));
        for c in &self.checks {
            let tag = match c.status {
                CheckStatus::Pass => "[PASS]    ",
                CheckStatus::Fail => "[FAIL]    ",
                CheckStatus::Deferred => "[DEFERRED]",
                CheckStatus::Skipped => "[SKIPPED] ",
            };
            out.push_str(&format!("{tag} {}: {}\n", c.id, c.message));
            if c.status == CheckStatus::Fail {
                out.push_str(&format!("           Remediation: {}\n", c.remediation));
            }
        }
        out
    }

    /// Render the report as JSON suitable for the
    /// `mai-api validate --json` CLI form and the
    /// `/v1/system/production-readiness` endpoint.
    pub fn to_json(&self) -> serde_json::Result<String> {
        serde_json::to_string_pretty(self)
    }

    /// Sanity check used by tests: every ID in `expected`
    /// appears exactly once in the report. Catches typos and
    /// duplicate registrations.
    #[doc(hidden)]
    pub fn assert_ids_unique(&self) {
        let mut seen = BTreeSet::new();
        for c in &self.checks {
            assert!(
                seen.insert(c.id.as_str()),
                "duplicate production check ID registered: {}",
                c.id
            );
        }
    }
}

/// Tally of statuses across a report.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct ReadinessCounts {
    pub pass: usize,
    pub fail: usize,
    pub deferred: usize,
    pub skipped: usize,
}

// ----- Runtime introspection -----------------

/// Runtime introspection results gathered during `MaiServer::run()`
/// startup, after the builders have run. Each field
/// is `None` when the check was not performed; `Some` flips the
/// corresponding deferred ID to Pass or Fail in the readiness report.
///
/// One field per `PROD-*-100/101` deferred check. The mapping is
/// stable so operators can wire alerts against the IDs without
/// tracking which SHIP session closed each gap.
#[derive(Debug, Default, Clone)]
pub struct RuntimeChecks {
    /// `PROD-VAULT-100` — vault built without error and the configured
    /// `vault.root` is reachable.
    pub vault_opened: Option<RuntimeOutcome>,
    /// `PROD-AUDIT-100` — [`crate::audit_wal::WalAuditWriter::open`]
    /// returned and the chain replay verified.
    pub api_audit_wal_ready: Option<RuntimeOutcome>,
    /// `PROD-AUDIT-101` — compliance audit log was built with an AEAD
    /// sealer rather than `NullSealer`.
    pub compliance_sealer_real: Option<RuntimeOutcome>,
    /// `PROD-TRUST-100` — trust components built and (in production)
    /// [`crate::trust_builder::verify_boot_bundle`] succeeded.
    pub trust_bundle_verified: Option<RuntimeOutcome>,
    /// `PROD-AUTH-100` — auth key store contains at least one entry.
    pub auth_keys_nonempty: Option<RuntimeOutcome>,
    /// `PROD-AUTH-101` — runtime auth store's
    /// `allow_internal_profile_header` flag matches the profile field
    /// the static `PROD-AUTH-002` check verified. This check (closes
    /// KNOWN-ISSUES Issue 13) catches the case where the loader
    /// silently enables the X-IM-Internal-Profile bypass even though
    /// the profile declared it disabled.
    pub auth_internal_bypass_consistent: Option<RuntimeOutcome>,
    /// `PROD-POLICY-001` — compliance policy modules loaded and the
    /// composer template built successfully.
    pub policy_modules_loaded: Option<RuntimeOutcome>,
}

/// Outcome of a single runtime check. `passed=true` lifts the matching
/// deferred ID to Pass; `passed=false` lifts it to Fail. The `detail`
/// string replaces the deferred message so operators see the runtime
/// reality rather than the registration-time placeholder.
#[derive(Debug, Clone)]
pub struct RuntimeOutcome {
    pub passed: bool,
    pub detail: String,
}

impl RuntimeOutcome {
    /// Convenience constructor for a passing runtime outcome.
    pub fn pass(detail: impl Into<String>) -> Self {
        Self {
            passed: true,
            detail: detail.into(),
        }
    }
    /// Convenience constructor for a failing runtime outcome.
    pub fn fail(detail: impl Into<String>) -> Self {
        Self {
            passed: false,
            detail: detail.into(),
        }
    }
}

// ----- Internal: check registration plumbing ------------------------

struct CheckContext<'a> {
    profile: &'a ShipProfile,
    checks: Vec<ProductionCheck>,
    is_production: bool,
}

impl<'a> CheckContext<'a> {
    fn new(profile: &'a ShipProfile) -> Self {
        Self {
            profile,
            checks: Vec::new(),
            is_production: profile.profile.mode == ProfileMode::Production,
        }
    }

    /// Add a check with explicit status. The `cond` closure produces
    /// (status, message). `remediation` is only used when status=Fail
    /// but is stored unconditionally so the catalogue is queryable.
    fn check<F>(&mut self, id: &'static str, severity: CheckSeverity, remediation: &str, cond: F)
    where
        F: FnOnce(&ShipProfile) -> (CheckStatus, String),
    {
        let (status, message) = if !self.is_production {
            (CheckStatus::Skipped, "non-production profile".to_string())
        } else {
            cond(self.profile)
        };
        self.checks.push(ProductionCheck {
            id: id.to_string(),
            severity,
            status,
            message,
            remediation: remediation.to_string(),
        });
    }

    /// Add a deferred runtime-check placeholder.
    fn deferred(
        &mut self,
        id: &'static str,
        severity: CheckSeverity,
        which_session: &str,
        what: &str,
        remediation: &str,
    ) {
        let status = if self.is_production {
            CheckStatus::Deferred
        } else {
            CheckStatus::Skipped
        };
        let msg = if self.is_production {
            format!("{what} (lands in {which_session})")
        } else {
            "non-production profile".to_string()
        };
        self.checks.push(ProductionCheck {
            id: id.to_string(),
            severity,
            status,
            message: msg,
            remediation: remediation.to_string(),
        });
    }
}

fn pass(msg: &str) -> (CheckStatus, String) {
    (CheckStatus::Pass, msg.to_string())
}

fn fail(msg: &str) -> (CheckStatus, String) {
    (CheckStatus::Fail, msg.to_string())
}

// ----- Check registry -----------------------------------------------

fn register_all_checks(ctx: &mut CheckContext) {
    register_config_checks(ctx);
    register_paths_checks(ctx);
    register_vault_checks(ctx);
    register_audit_checks(ctx);
    register_trust_checks(ctx);
    register_auth_checks(ctx);
    register_dashboard_checks(ctx);
    register_network_checks(ctx);
    register_policy_checks(ctx);
}

fn register_config_checks(ctx: &mut CheckContext) {
    ctx.check(
        "PROD-CONFIG-001",
        CheckSeverity::Critical,
        "set [profile].mode = \"production\" in the ship profile",
        |p| {
            if p.profile.mode == ProfileMode::Production {
                pass("profile.mode = production")
            } else {
                fail(&format!(
                    "profile.mode is {:?}, not production",
                    p.profile.mode
                ))
            }
        },
    );
    ctx.check(
        "PROD-CONFIG-002",
        CheckSeverity::Critical,
        "set [profile].fail_closed = true",
        |p| {
            if p.profile.fail_closed {
                pass("profile.fail_closed = true")
            } else {
                fail("profile.fail_closed must be true in production")
            }
        },
    );
    ctx.check(
        "PROD-CONFIG-003",
        CheckSeverity::Critical,
        "set [profile].allow_demo_defaults = false",
        |p| {
            if p.profile.allow_demo_defaults {
                fail("profile.allow_demo_defaults = true is forbidden in production")
            } else {
                pass("profile.allow_demo_defaults = false")
            }
        },
    );
}

fn register_paths_checks(ctx: &mut CheckContext) {
    fn path_check(
        ctx: &mut CheckContext,
        id: &'static str,
        field: &'static str,
        get: fn(&ShipProfile) -> &std::path::Path,
    ) {
        let remediation = format!("set [paths].{field} to a persistent directory");
        ctx.check(id, CheckSeverity::Critical, &remediation, move |p| {
            let path = get(p);
            if path.as_os_str().is_empty() {
                fail(&format!("paths.{field} is empty"))
            } else {
                pass(&format!("paths.{field} = {}", path.display()))
            }
        });
    }
    path_check(ctx, "PROD-PATHS-001", "state_dir", |p| &p.paths.state_dir);
    path_check(ctx, "PROD-PATHS-002", "config_dir", |p| &p.paths.config_dir);
    path_check(ctx, "PROD-PATHS-003", "log_dir", |p| &p.paths.log_dir);
    path_check(ctx, "PROD-PATHS-004", "run_dir", |p| &p.paths.run_dir);
    path_check(ctx, "PROD-PATHS-005", "backup_dir", |p| &p.paths.backup_dir);
}

fn register_vault_checks(ctx: &mut CheckContext) {
    ctx.check(
        "PROD-VAULT-001",
        CheckSeverity::Critical,
        "set [vault].backend to the reviewed encrypted backend (\"zfs\")",
        |p| match p.vault.backend {
            VaultBackend::Stub => {
                fail("vault.backend is \"stub\"; StubVault is rejected in production")
            }
            // V1: file-dev stores plaintext — never a production vault.
            VaultBackend::FileDev => fail(
                "vault.backend is \"file-dev\" (plaintext); production requires the encrypted zfs backend",
            ),
            _ => pass(&format!("vault.backend = {:?}", p.vault.backend)),
        },
    );
    ctx.check(
        "PROD-VAULT-002",
        CheckSeverity::Critical,
        "set [vault].allow_stub = false",
        |p| {
            if p.vault.allow_stub {
                fail("vault.allow_stub = true is forbidden in production")
            } else {
                pass("vault.allow_stub = false")
            }
        },
    );
    ctx.check(
        "PROD-VAULT-003",
        CheckSeverity::Critical,
        "set [vault].root to the on-disk vault directory",
        |p| {
            if p.vault.root.as_os_str().is_empty() {
                fail("vault.root is empty")
            } else {
                pass(&format!("vault.root = {}", p.vault.root.display()))
            }
        },
    );
    ctx.check(
        "PROD-VAULT-004",
        CheckSeverity::Critical,
        "set [vault].require_sealed_master_key = true",
        |p| {
            if p.vault.require_sealed_master_key {
                pass("vault.require_sealed_master_key = true")
            } else {
                fail("vault.require_sealed_master_key must be true in production")
            }
        },
    );
    ctx.check(
        "PROD-VAULT-005",
        CheckSeverity::Critical,
        "set [vault].require_pqc = true",
        |p| {
            if p.vault.require_pqc {
                pass("vault.require_pqc = true")
            } else {
                fail("vault.require_pqc must be true in production")
            }
        },
    );
    ctx.deferred(
        "PROD-VAULT-100",
        CheckSeverity::Critical,
        "SHIP-03", // slop-ok: names the deferred check's ship-plan step (operator data)
        "vault opens, sealed master key loads, root directory is writable",
        "ensure /var/lib/mai/vault is initialized; see docs/compliance/SECURITY-PRODUCTION.md",
    );
}

fn register_audit_checks(ctx: &mut CheckContext) {
    ctx.check(
        "PROD-AUDIT-001",
        CheckSeverity::Critical,
        "set [audit].api_writer = \"wal\"",
        |p| {
            if matches!(p.audit.api_writer, AuditWriter::Memory) {
                fail("audit.api_writer = \"memory\"; MemoryAuditWriter is rejected in production")
            } else {
                pass("audit.api_writer = wal")
            }
        },
    );
    ctx.check(
        "PROD-AUDIT-002",
        CheckSeverity::Critical,
        "set [audit].compliance_writer = \"wal\"",
        |p| {
            if matches!(p.audit.compliance_writer, AuditWriter::Memory) {
                fail("audit.compliance_writer = \"memory\"; MemoryAuditWriter is rejected in production")
            } else {
                pass("audit.compliance_writer = wal")
            }
        },
    );
    ctx.check(
        "PROD-AUDIT-003",
        CheckSeverity::Critical,
        "set [audit].allow_memory_writer = false",
        |p| {
            if p.audit.allow_memory_writer {
                fail("audit.allow_memory_writer = true is forbidden in production")
            } else {
                pass("audit.allow_memory_writer = false")
            }
        },
    );
    ctx.check(
        "PROD-AUDIT-004",
        CheckSeverity::Critical,
        "set [audit].wal_dir to a persistent directory",
        |p| {
            if p.audit.wal_dir.as_os_str().is_empty() {
                fail("audit.wal_dir is empty")
            } else {
                pass(&format!("audit.wal_dir = {}", p.audit.wal_dir.display()))
            }
        },
    );
    ctx.check(
        "PROD-AUDIT-005",
        CheckSeverity::Critical,
        "set [audit].allow_null_sealer = false",
        |p| {
            if p.audit.allow_null_sealer {
                fail("audit.allow_null_sealer = true is forbidden in production")
            } else {
                pass("audit.allow_null_sealer = false")
            }
        },
    );
    ctx.check(
        "PROD-AUDIT-006",
        CheckSeverity::Critical,
        "set [audit].require_hash_chain = true",
        |p| {
            if p.audit.require_hash_chain {
                pass("audit.require_hash_chain = true")
            } else {
                fail("audit.require_hash_chain must be true in production")
            }
        },
    );
    ctx.check(
        "PROD-AUDIT-007",
        CheckSeverity::Critical,
        "set [audit].require_pqc_checkpoints = true",
        |p| {
            if p.audit.require_pqc_checkpoints {
                pass("audit.require_pqc_checkpoints = true")
            } else {
                fail("audit.require_pqc_checkpoints must be true in production")
            }
        },
    );
    ctx.check(
        "PROD-AUDIT-008",
        CheckSeverity::Critical,
        "set [audit].require_encryption_at_rest = true",
        |p| {
            if p.audit.require_encryption_at_rest {
                pass("audit.require_encryption_at_rest = true")
            } else {
                fail("audit.require_encryption_at_rest must be true in production")
            }
        },
    );
    ctx.deferred(
        "PROD-AUDIT-100",
        CheckSeverity::Critical,
        "SHIP-04", // slop-ok: names the deferred check's ship-plan step (operator data)
        "API audit WAL writable, chain verifies, append round-trip succeeds",
        "ensure audit.wal_dir is writable by the mai user; see docs/compliance/AUDIT-RETENTION.md",
    );
    ctx.deferred(
        "PROD-AUDIT-101",
        CheckSeverity::Critical,
        "SHIP-05", // slop-ok: names the deferred check's ship-plan step (operator data)
        "compliance audit sealer is vault-backed AEAD (not NullSealer) at runtime",
        "wire mai-compliance AuditLog to vault AEAD sealer",
    );
}

fn register_trust_checks(ctx: &mut CheckContext) {
    ctx.check(
        "PROD-TRUST-001",
        CheckSeverity::Critical,
        "set [trust].verifier = \"ml-dsa\"",
        |p| {
            if matches!(p.trust.verifier, TrustVerifier::AcceptAll) {
                fail("trust.verifier = \"accept-all\"; AcceptAllBundleVerifier is rejected in production")
            } else {
                pass("trust.verifier = ml-dsa")
            }
        },
    );
    ctx.check(
        "PROD-TRUST-002",
        CheckSeverity::Critical,
        "set [trust].allow_accept_all_verifier = false",
        |p| {
            if p.trust.allow_accept_all_verifier {
                fail("trust.allow_accept_all_verifier = true is forbidden in production")
            } else {
                pass("trust.allow_accept_all_verifier = false")
            }
        },
    );
    ctx.check(
        "PROD-TRUST-003",
        CheckSeverity::Critical,
        "set [trust].allow_local_dev_exchange = false",
        |p| {
            if p.trust.allow_local_dev_exchange {
                fail("trust.allow_local_dev_exchange = true is forbidden; LocalDevSynthetic token exchange is rejected in production")
            } else {
                pass("trust.allow_local_dev_exchange = false")
            }
        },
    );
    ctx.check(
        "PROD-TRUST-004",
        CheckSeverity::Critical,
        "set [trust].require_trust_anchor = true",
        |p| {
            if p.trust.require_trust_anchor {
                pass("trust.require_trust_anchor = true")
            } else {
                fail("trust.require_trust_anchor must be true in production")
            }
        },
    );
    ctx.check(
        "PROD-TRUST-005",
        CheckSeverity::Critical,
        "set [trust].anchors_dir to /etc/mai/trust-anchors or your chosen path",
        |p| {
            if p.trust.anchors_dir.as_os_str().is_empty() {
                fail("trust.anchors_dir is empty")
            } else {
                pass(&format!(
                    "trust.anchors_dir = {}",
                    p.trust.anchors_dir.display()
                ))
            }
        },
    );
    ctx.check(
        "PROD-TRUST-006",
        CheckSeverity::Critical,
        "set [trust].bundle_cache_dir to a persistent directory",
        |p| {
            if p.trust.bundle_cache_dir.as_os_str().is_empty() {
                fail("trust.bundle_cache_dir is empty")
            } else {
                pass(&format!(
                    "trust.bundle_cache_dir = {}",
                    p.trust.bundle_cache_dir.display()
                ))
            }
        },
    );
    ctx.check(
        "PROD-TRUST-007",
        CheckSeverity::Critical,
        "set [trust].require_bundle_on_boot = true",
        |p| {
            if p.trust.require_bundle_on_boot {
                pass("trust.require_bundle_on_boot = true")
            } else {
                fail("trust.require_bundle_on_boot must be true in production")
            }
        },
    );
    ctx.deferred(
        "PROD-TRUST-100",
        CheckSeverity::Critical,
        "SHIP-06", // slop-ok: names the deferred check's ship-plan step (operator data)
        "trust bundle present, signature verifies, revocation snapshot fresh",
        "load a signed bundle into trust.bundle_cache_dir; see docs/compliance/TRUST-BRIDGE-PRODUCTION.md",
    );
}

fn register_auth_checks(ctx: &mut CheckContext) {
    ctx.check(
        "PROD-AUTH-001",
        CheckSeverity::Critical,
        "set [auth].auth_keys_path to /etc/mai/auth_keys.toml or your chosen path",
        |p| {
            if p.auth.auth_keys_path.as_os_str().is_empty() {
                fail("auth.auth_keys_path is empty")
            } else {
                pass(&format!(
                    "auth.auth_keys_path = {}",
                    p.auth.auth_keys_path.display()
                ))
            }
        },
    );
    ctx.check(
        "PROD-AUTH-002",
        CheckSeverity::Critical,
        "set [auth].allow_internal_profile_header = false",
        |p| {
            if p.auth.allow_internal_profile_header {
                fail("auth.allow_internal_profile_header = true is forbidden; the X-IM-Internal-Profile bypass is rejected in production")
            } else {
                pass("auth.allow_internal_profile_header = false")
            }
        },
    );
    ctx.check(
        "PROD-AUTH-003",
        CheckSeverity::Critical,
        "set [auth].require_nonempty_key_store = true",
        |p| {
            if p.auth.require_nonempty_key_store {
                pass("auth.require_nonempty_key_store = true")
            } else {
                fail("auth.require_nonempty_key_store must be true in production")
            }
        },
    );
    ctx.deferred(
        "PROD-AUTH-100",
        CheckSeverity::Critical,
        "SHIP-07", // slop-ok: names the deferred check's ship-plan step (operator data)
        "auth keys file is loadable and contains at least one entry",
        "populate auth.auth_keys_path with at least one rotated key",
    );
    ctx.deferred(
        "PROD-AUTH-101",
        CheckSeverity::Critical,
        "SHIP-17", // slop-ok: names the deferred check's ship-plan step (operator data)
        "runtime auth store's allow_internal_profile_header matches the profile field",
        "ensure the auth bootstrap honors auth.auth_keys_path; never let the first-boot fallback enable the X-IM-Internal-Profile bypass under a production profile",
    );
}

fn register_dashboard_checks(ctx: &mut CheckContext) {
    ctx.check(
        "PROD-DASH-001",
        CheckSeverity::Critical,
        "set [dashboard].allow_default_admin_token = false",
        |p| {
            if p.dashboard.allow_default_admin_token {
                fail("dashboard.allow_default_admin_token = true is forbidden; the dashboard-dev token is rejected in production")
            } else {
                pass("dashboard.allow_default_admin_token = false")
            }
        },
    );
}

fn register_network_checks(ctx: &mut CheckContext) {
    ctx.check(
        "PROD-NET-001",
        CheckSeverity::Critical,
        "set [network].bind_address (e.g. \"127.0.0.1\")",
        |p| {
            if p.network.bind_address.trim().is_empty() {
                fail("network.bind_address is empty")
            } else {
                pass(&format!(
                    "network.bind_address = {}",
                    p.network.bind_address
                ))
            }
        },
    );
    ctx.check(
        "PROD-NET-002",
        CheckSeverity::Critical,
        "use a loopback bind (127.0.0.1 or ::1); terminate TLS at the reverse proxy",
        |p| {
            let bind = p.network.bind_address.trim();
            if bind == "0.0.0.0" || bind == "::" || bind == "[::]" {
                fail(&format!("network.bind_address {bind:?} is a wildcard"))
            } else {
                pass(&format!("network.bind_address {bind} is not a wildcard"))
            }
        },
    );
}

fn register_policy_checks(ctx: &mut CheckContext) {
    ctx.deferred(
        "PROD-POLICY-001",
        CheckSeverity::Critical,
        "SHIP-05", // slop-ok: names the deferred check's ship-plan step (operator data)
        "compliance policy modules load and template composes",
        "ensure config/compliance/*.toml is present in [paths].config_dir",
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ship_profile::parse_ship_profile;

    fn baseline_toml() -> &'static str {
        r#"
[profile]
name = "ship"
mode = "production"
allow_demo_defaults = false
fail_closed = true

[paths]
state_dir = "/var/lib/mai"
config_dir = "/etc/mai"
log_dir = "/var/log/mai"
run_dir = "/run/mai"
backup_dir = "/var/backups/mai"

[vault]
backend = "zfs"
root = "/var/lib/mai/vault"
require_sealed_master_key = true
require_pqc = true
allow_stub = false

[audit]
api_writer = "wal"
compliance_writer = "wal"
wal_dir = "/var/lib/mai/audit"
require_hash_chain = true
require_pqc_checkpoints = true
require_encryption_at_rest = true
allow_memory_writer = false
allow_null_sealer = false

[trust]
anchors_dir = "/etc/mai/trust-anchors"
bundle_cache_dir = "/var/lib/mai/trust"
verifier = "ml-dsa"
allow_accept_all_verifier = false
allow_local_dev_exchange = false
require_trust_anchor = true
require_bundle_on_boot = true

[auth]
auth_keys_path = "/etc/mai/auth_keys.toml"
allow_internal_profile_header = false
require_nonempty_key_store = true

[dashboard]
enabled = true
allow_default_admin_token = false

[network]
bind_address = "127.0.0.1"
tls_mode = "reverse-proxy-required"
require_forwarded_proto_header = false

[observability]
log_format = "json"
log_rotation = true
metrics_exporter = "prometheus"
alerts_enabled = true
"#
    }

    #[test]
    fn baseline_is_ship_ready_modulo_deferred() {
        let profile = parse_ship_profile(baseline_toml()).expect("baseline parses");
        let report = ProductionReadinessReport::evaluate(&profile);
        report.assert_ids_unique();
        assert!(
            report.is_ship_ready(),
            "baseline must be ship-ready (no Critical Fail). Report:\n{}",
            report.render_human()
        );
        let counts = report.counts();
        assert_eq!(counts.fail, 0);
        // The deferred runtime checks must be present so operators see
        // them in the report rather than silently passing.
        assert!(
            counts.deferred >= 5,
            "expected ≥5 deferred runtime checks, got {}",
            counts.deferred
        );
    }

    #[test]
    fn report_contains_every_documented_check_id() {
        let profile = parse_ship_profile(baseline_toml()).expect("baseline parses");
        let report = ProductionReadinessReport::evaluate(&profile);
        for id in EXPECTED_CHECK_IDS {
            assert!(
                report.find(id).is_some(),
                "expected report to contain {id}, got IDs: {:?}",
                report
                    .checks
                    .iter()
                    .map(|c| c.id.as_str())
                    .collect::<Vec<_>>()
            );
        }
    }

    /// Stable catalogue of every check ID the guard registers. Adding a
    /// new check means adding it here and to the registry above —
    /// both in one PR, never one without the other.
    const EXPECTED_CHECK_IDS: &[&str] = &[
        "PROD-CONFIG-001",
        "PROD-CONFIG-002",
        "PROD-CONFIG-003",
        "PROD-PATHS-001",
        "PROD-PATHS-002",
        "PROD-PATHS-003",
        "PROD-PATHS-004",
        "PROD-PATHS-005",
        "PROD-VAULT-001",
        "PROD-VAULT-002",
        "PROD-VAULT-003",
        "PROD-VAULT-004",
        "PROD-VAULT-005",
        "PROD-VAULT-100",
        "PROD-AUDIT-001",
        "PROD-AUDIT-002",
        "PROD-AUDIT-003",
        "PROD-AUDIT-004",
        "PROD-AUDIT-005",
        "PROD-AUDIT-006",
        "PROD-AUDIT-007",
        "PROD-AUDIT-008",
        "PROD-AUDIT-100",
        "PROD-AUDIT-101",
        "PROD-TRUST-001",
        "PROD-TRUST-002",
        "PROD-TRUST-003",
        "PROD-TRUST-004",
        "PROD-TRUST-005",
        "PROD-TRUST-006",
        "PROD-TRUST-007",
        "PROD-TRUST-100",
        "PROD-AUTH-001",
        "PROD-AUTH-002",
        "PROD-AUTH-003",
        "PROD-AUTH-100",
        "PROD-AUTH-101",
        "PROD-DASH-001",
        "PROD-NET-001",
        "PROD-NET-002",
        "PROD-POLICY-001",
    ];

    /// Profile parsing rejects most of these violations before the
    /// guard ever runs. We exercise the guard directly by
    /// mutating fields on a parsed ShipProfile.
    fn mutate<F>(f: F) -> ProductionReadinessReport
    where
        F: FnOnce(&mut ShipProfile),
    {
        // Parse local-dev so the parser doesn't reject the mutations
        // before the guard runs. Then flip mode to production so the
        // guard's production-only checks engage.
        let toml = baseline_toml().replace("mode = \"production\"", "mode = \"local-dev\"");
        let mut profile = parse_ship_profile(&toml).expect("local-dev baseline parses");
        profile.profile.mode = ProfileMode::Production;
        f(&mut profile);
        ProductionReadinessReport::evaluate(&profile)
    }

    fn assert_only_fail(report: &ProductionReadinessReport, id: &str) {
        let check = report.find(id).unwrap_or_else(|| {
            panic!(
                "report missing expected check {id}; ids: {:?}",
                report
                    .checks
                    .iter()
                    .map(|c| c.id.as_str())
                    .collect::<Vec<_>>()
            )
        });
        assert_eq!(
            check.status,
            CheckStatus::Fail,
            "expected {id} to FAIL, report:\n{}",
            report.render_human()
        );
        let extra: Vec<&str> = report
            .failures()
            .filter(|c| c.id.as_str() != id)
            .map(|c| c.id.as_str())
            .collect();
        assert!(
            extra.is_empty(),
            "expected only {id} to fail, also got: {extra:?}\n{}",
            report.render_human()
        );
        assert!(!report.is_ship_ready());
    }

    #[test]
    fn fail_config_003_allow_demo_defaults() {
        let r = mutate(|p| p.profile.allow_demo_defaults = true);
        assert_only_fail(&r, "PROD-CONFIG-003");
    }

    #[test]
    fn fail_config_002_fail_closed_false() {
        let r = mutate(|p| p.profile.fail_closed = false);
        assert_only_fail(&r, "PROD-CONFIG-002");
    }

    #[test]
    fn fail_paths_001_empty_state_dir() {
        let r = mutate(|p| p.paths.state_dir = std::path::PathBuf::new());
        assert_only_fail(&r, "PROD-PATHS-001");
    }

    #[test]
    fn fail_vault_001_stub_backend() {
        let r = mutate(|p| p.vault.backend = VaultBackend::Stub);
        assert_only_fail(&r, "PROD-VAULT-001");
    }

    #[test]
    fn fail_vault_002_allow_stub_true() {
        let r = mutate(|p| p.vault.allow_stub = true);
        assert_only_fail(&r, "PROD-VAULT-002");
    }

    #[test]
    fn fail_audit_001_memory_api_writer() {
        let r = mutate(|p| p.audit.api_writer = AuditWriter::Memory);
        assert_only_fail(&r, "PROD-AUDIT-001");
    }

    #[test]
    fn fail_audit_003_allow_memory_writer() {
        let r = mutate(|p| p.audit.allow_memory_writer = true);
        assert_only_fail(&r, "PROD-AUDIT-003");
    }

    #[test]
    fn fail_audit_005_allow_null_sealer() {
        let r = mutate(|p| p.audit.allow_null_sealer = true);
        assert_only_fail(&r, "PROD-AUDIT-005");
    }

    #[test]
    fn fail_trust_001_accept_all_verifier() {
        let r = mutate(|p| p.trust.verifier = TrustVerifier::AcceptAll);
        assert_only_fail(&r, "PROD-TRUST-001");
    }

    #[test]
    fn fail_trust_003_local_dev_exchange() {
        let r = mutate(|p| p.trust.allow_local_dev_exchange = true);
        assert_only_fail(&r, "PROD-TRUST-003");
    }

    #[test]
    fn fail_auth_002_internal_profile_header() {
        let r = mutate(|p| p.auth.allow_internal_profile_header = true);
        assert_only_fail(&r, "PROD-AUTH-002");
    }

    #[test]
    fn fail_dash_001_default_admin_token() {
        let r = mutate(|p| p.dashboard.allow_default_admin_token = true);
        assert_only_fail(&r, "PROD-DASH-001");
    }

    #[test]
    fn fail_net_002_wildcard_bind() {
        let r = mutate(|p| p.network.bind_address = "0.0.0.0".to_string());
        assert_only_fail(&r, "PROD-NET-002");
    }

    #[test]
    fn non_production_profile_skips_all_checks() {
        let mut profile = parse_ship_profile(baseline_toml()).expect("baseline parses");
        profile.profile.mode = ProfileMode::LocalDev;
        let report = ProductionReadinessReport::evaluate(&profile);
        assert!(report.is_ship_ready());
        let counts = report.counts();
        assert_eq!(counts.fail, 0);
        assert_eq!(counts.pass, 0);
        assert_eq!(counts.deferred, 0);
        assert!(counts.skipped > 0);
        // Every check is Skipped under local-dev.
        for c in &report.checks {
            assert_eq!(c.status, CheckStatus::Skipped, "{} not skipped", c.id);
        }
    }

    #[test]
    fn deferred_runtime_checks_surface_session_id() {
        let profile = parse_ship_profile(baseline_toml()).expect("baseline parses");
        let report = ProductionReadinessReport::evaluate(&profile);
        let deferred: Vec<&ProductionCheck> = report.deferred().collect();
        assert!(deferred.len() >= 5);
        // Each deferred check must name the SHIP session that closes it.
        for c in &deferred {
            assert!(
                c.message.contains("SHIP-"),
                "deferred check {} does not name a SHIP session: {}",
                c.id,
                c.message
            );
        }
    }

    #[test]
    fn json_roundtrip() {
        let profile = parse_ship_profile(baseline_toml()).expect("baseline parses");
        let report = ProductionReadinessReport::evaluate(&profile);
        let json = report.to_json().expect("to_json");
        assert!(json.contains("PROD-CONFIG-001"));
        let decoded: ProductionReadinessReport = serde_json::from_str(&json).expect("roundtrip");
        assert_eq!(decoded.checks.len(), report.checks.len());
    }

    #[test]
    fn human_render_marks_pass_and_fail() {
        let r = mutate(|p| p.profile.fail_closed = false);
        let text = r.render_human();
        assert!(text.contains("FAIL"));
        assert!(text.contains("PROD-CONFIG-002"));
        assert!(text.contains("Remediation:"));
    }

    // ----- Convergence: runtime checks -------------------------

    fn all_passing_runtime() -> RuntimeChecks {
        RuntimeChecks {
            vault_opened: Some(RuntimeOutcome::pass("ZfsVault opened at /tmp/vault")),
            api_audit_wal_ready: Some(RuntimeOutcome::pass("WAL opened (0 entries)")),
            compliance_sealer_real: Some(RuntimeOutcome::pass("AeadSealer wired")),
            trust_bundle_verified: Some(RuntimeOutcome::pass("bundle v1 verified")),
            auth_keys_nonempty: Some(RuntimeOutcome::pass("1 key loaded")),
            auth_internal_bypass_consistent: Some(RuntimeOutcome::pass(
                "runtime bypass = false, profile field = false: consistent",
            )),
            policy_modules_loaded: Some(RuntimeOutcome::pass("Standard template loaded")),
        }
    }

    #[test]
    fn runtime_flips_deferred_to_pass() {
        let profile = parse_ship_profile(baseline_toml()).expect("baseline parses");
        let runtime = all_passing_runtime();
        let report = ProductionReadinessReport::evaluate_with_runtime(&profile, &runtime);
        for id in [
            "PROD-VAULT-100",
            "PROD-AUDIT-100",
            "PROD-AUDIT-101",
            "PROD-TRUST-100",
            "PROD-AUTH-100",
            "PROD-AUTH-101",
            "PROD-POLICY-001",
        ] {
            let c = report.find(id).expect("check present");
            assert_eq!(
                c.status,
                CheckStatus::Pass,
                "{id} should flip Deferred -> Pass, got {:?} ({})",
                c.status,
                c.message
            );
        }
        assert!(report.is_ship_ready());
        // After flipping every runtime ID, no deferred remain.
        assert_eq!(report.counts().deferred, 0);
    }

    #[test]
    fn runtime_flip_to_fail_blocks_ship_ready() {
        let profile = parse_ship_profile(baseline_toml()).expect("baseline parses");
        let runtime = RuntimeChecks {
            vault_opened: Some(RuntimeOutcome::fail("vault root not writable: EACCES")),
            ..all_passing_runtime()
        };
        let report = ProductionReadinessReport::evaluate_with_runtime(&profile, &runtime);
        let c = report.find("PROD-VAULT-100").expect("check present");
        assert_eq!(c.status, CheckStatus::Fail);
        assert!(c.message.contains("EACCES"));
        assert!(!report.is_ship_ready());
    }

    #[test]
    fn runtime_partial_results_keep_others_deferred() {
        let profile = parse_ship_profile(baseline_toml()).expect("baseline parses");
        let runtime = RuntimeChecks {
            vault_opened: Some(RuntimeOutcome::pass("vault ok")),
            ..RuntimeChecks::default()
        };
        let report = ProductionReadinessReport::evaluate_with_runtime(&profile, &runtime);
        assert_eq!(
            report.find("PROD-VAULT-100").unwrap().status,
            CheckStatus::Pass
        );
        for id in [
            "PROD-AUDIT-100",
            "PROD-AUDIT-101",
            "PROD-TRUST-100",
            "PROD-AUTH-100",
            "PROD-AUTH-101",
            "PROD-POLICY-001",
        ] {
            assert_eq!(
                report.find(id).unwrap().status,
                CheckStatus::Deferred,
                "{id} should stay Deferred when runtime field is None"
            );
        }
    }

    #[test]
    fn runtime_does_not_override_config_only_pass() {
        // PROD-VAULT-001 is a config-only check (already Pass under
        // baseline). A runtime field for an unrelated ID must not
        // touch it.
        let profile = parse_ship_profile(baseline_toml()).expect("baseline parses");
        let runtime = all_passing_runtime();
        let report = ProductionReadinessReport::evaluate_with_runtime(&profile, &runtime);
        assert_eq!(
            report.find("PROD-VAULT-001").unwrap().status,
            CheckStatus::Pass
        );
    }

    #[test]
    fn runtime_skipped_under_local_dev_stays_skipped() {
        // Under local-dev every check is Skipped. Runtime introspection
        // must never resurrect a Skipped check into Pass/Fail.
        let mut profile = parse_ship_profile(baseline_toml()).expect("baseline parses");
        profile.profile.mode = ProfileMode::LocalDev;
        let runtime = all_passing_runtime();
        let report = ProductionReadinessReport::evaluate_with_runtime(&profile, &runtime);
        for id in [
            "PROD-VAULT-100",
            "PROD-AUDIT-100",
            "PROD-AUDIT-101",
            "PROD-TRUST-100",
            "PROD-AUTH-100",
            "PROD-AUTH-101",
            "PROD-POLICY-001",
        ] {
            assert_eq!(
                report.find(id).unwrap().status,
                CheckStatus::Skipped,
                "{id} must stay Skipped under local-dev"
            );
        }
    }
}
