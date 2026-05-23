//! Production readiness guard.
//!
//! Centralised, fail-closed contract scanner that runs over a parsed
//! [`ShipProfile`] and reports every violation against the SHIP-HARDENING-PLAN.md
//! §6 (Workstream 4) check list. One report → one ID per violation →
//! one remediation hint per violation.
//!
//! Scope (SHIP-02):
//! - Define [`ProductionReadinessReport`], [`ProductionCheck`],
//!   [`CheckSeverity`], [`CheckStatus`].
//! - Implement the config-only checks: anything decidable from the
//!   profile struct alone.
//! - Register the runtime-only checks (vault open, audit append round
//!   trip, trust bundle verify, etc.) with `CheckStatus::Deferred` and
//!   a note about which later SHIP session closes them. This keeps
//!   the report shape stable across the hardening lane — adding a
//!   runtime check later flips its status from Deferred to Pass / Fail
//!   without renumbering anything.
//!
//! Out of scope (SHIP-02):
//! - Filesystem existence checks (the guard does not stat paths).
//!   Path-exists / writable / chain-verifies / bundle-loads lands in
//!   SHIP-03 (vault), SHIP-04 (API audit WAL), SHIP-05 (compliance
//!   audit sealer), SHIP-06 (trust).
//! - Wiring into `MaiServer::run` startup. The guard is callable from
//!   tests and the SHIP-02 CLI subcommand today; the production
//!   startup hook lands in SHIP-07 alongside the validator binary.
//! - HTTP endpoint `GET /v1/system/production-readiness` — also SHIP-07.
//!
//! Check ID convention:
//! - `PROD-{AREA}-NNN` where AREA is CONFIG / PATHS / VAULT / AUDIT /
//!   TRUST / AUTH / DASH / NET / POLICY / OBS.
//! - IDs are stable. New checks append; existing IDs never get reused
//!   or renumbered. Operators wire alerts against the ID.
//! - IDs ≥ 100 are runtime checks (SHIP-03+); IDs < 100 are config-only
//!   (this session).

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::ship_profile::{AuditWriter, ProfileMode, ShipProfile, TrustVerifier, VaultBackend};

/// Severity tag attached to every [`ProductionCheck`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CheckSeverity {
    /// Non-blocking. Useful for surfaces like "deferred to SHIP-04".
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
    /// `mai-api validate --json` CLI form and the SHIP-07
    /// `/v1/system/production-readiness` endpoint.
    pub fn to_json(&self) -> serde_json::Result<String> {
        serde_json::to_string_pretty(self)
    }

    /// Sanity check used by SHIP-02 tests: every ID in `expected`
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
        "set [vault].backend to a real backend (e.g. \"zfs\")",
        |p| {
            if matches!(p.vault.backend, VaultBackend::Stub) {
                fail("vault.backend is \"stub\"; StubVault is rejected in production")
            } else {
                pass(&format!("vault.backend = {:?}", p.vault.backend))
            }
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
        "SHIP-03",
        "vault opens, sealed master key loads, root directory is writable",
        "ensure /var/lib/mai/vault is initialized; see docs/SECURITY-PRODUCTION.md",
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
        "SHIP-04",
        "API audit WAL writable, chain verifies, append round-trip succeeds",
        "ensure audit.wal_dir is writable by the mai user; see docs/AUDIT-RETENTION.md",
    );
    ctx.deferred(
        "PROD-AUDIT-101",
        CheckSeverity::Critical,
        "SHIP-05",
        "compliance audit sealer is vault-backed AEAD (not NullSealer) at runtime",
        "wire mai-compliance AuditLog to vault AEAD sealer per SHIP-05",
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
        "SHIP-06",
        "trust bundle present, signature verifies, revocation snapshot fresh",
        "load a signed bundle into trust.bundle_cache_dir; see docs/TRUST-BRIDGE-PRODUCTION.md",
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
        "SHIP-07",
        "auth keys file is loadable and contains at least one entry",
        "populate auth.auth_keys_path with at least one rotated key",
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
        "SHIP-05",
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

    /// Stable catalogue of every check ID SHIP-02 registers. Adding a
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
        "PROD-DASH-001",
        "PROD-NET-001",
        "PROD-NET-002",
        "PROD-POLICY-001",
    ];

    /// SHIP-01 parsing rejects most of these violations before the
    /// guard ever runs. For SHIP-02 we exercise the guard directly by
    /// mutating fields on a parsed ShipProfile.
    fn mutate<F>(f: F) -> ProductionReadinessReport
    where
        F: FnOnce(&mut ShipProfile),
    {
        // Parse local-dev so SHIP-01 doesn't reject the mutations
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
}
