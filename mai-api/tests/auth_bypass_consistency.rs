//! Regression coverage (closes KNOWN-ISSUES Issue 13) for the
//! `PROD-AUTH-101` runtime check that detects divergence between the
//! ship profile's `auth.allow_internal_profile_header` field and the
//! runtime auth store's actual flag.
//!
//! The static `PROD-AUTH-002` check reads the profile field; before
//! this fix, an operator could silently end up with the runtime bypass
//! enabled if `load_auth_state` fell through to first-boot under a
//! misconfigured production profile. `PROD-AUTH-101` is the runtime
//! cross-check that fails closed in that case.

use mai_api::{
    CheckStatus, ProductionReadinessReport, RuntimeChecks, RuntimeOutcome, parse_ship_profile,
};

fn baseline_production_toml() -> &'static str {
    r#"
[profile]
name = "ship17-integration"
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
fn prod_auth_101_deferred_without_runtime() {
    // Without a runtime introspection result, PROD-AUTH-101 stays
    // Deferred — the report names the gap rather than silently
    // passing it.
    let profile = parse_ship_profile(baseline_production_toml()).expect("baseline parses");
    let report = ProductionReadinessReport::evaluate(&profile);
    let check = report
        .find("PROD-AUTH-101")
        .expect("PROD-AUTH-101 must be registered");
    assert_eq!(check.status, CheckStatus::Deferred);
    assert!(
        check.message.contains("SHIP-17"), // slop-ok: asserts the readiness message cites the deferred ship-plan item
        "deferred message must name the closing session: {}",
        check.message
    );
}

#[test]
fn prod_auth_101_passes_when_consistent() {
    // Runtime flag matches profile field: report flips Deferred -> Pass.
    let profile = parse_ship_profile(baseline_production_toml()).expect("baseline parses");
    let runtime = RuntimeChecks {
        auth_internal_bypass_consistent: Some(RuntimeOutcome::pass(
            "runtime bypass = false, profile field = false: consistent",
        )),
        ..RuntimeChecks::default()
    };
    let report = ProductionReadinessReport::evaluate_with_runtime(&profile, &runtime);
    let check = report
        .find("PROD-AUTH-101")
        .expect("PROD-AUTH-101 must be registered");
    assert_eq!(check.status, CheckStatus::Pass);
    assert!(check.message.contains("consistent"));
}

#[test]
fn prod_auth_101_fail_blocks_ship_ready() {
    // Issue 13 scenario: profile says bypass is off, runtime store
    // says bypass is on. The runtime check fails and the overall
    // report is no longer ship-ready. This is the regression test
    // for the bug this check closes.
    let profile = parse_ship_profile(baseline_production_toml()).expect("baseline parses");
    let runtime = RuntimeChecks {
        auth_internal_bypass_consistent: Some(RuntimeOutcome::fail(
            "runtime bypass = true but profile field = false: \
             X-IM-Internal-Profile bypass diverges from profile contract",
        )),
        ..RuntimeChecks::default()
    };
    let report = ProductionReadinessReport::evaluate_with_runtime(&profile, &runtime);
    let check = report
        .find("PROD-AUTH-101")
        .expect("PROD-AUTH-101 must be registered");
    assert_eq!(check.status, CheckStatus::Fail);
    assert!(
        check.message.contains("diverges"),
        "fail message must explain the divergence: {}",
        check.message
    );
    assert!(
        !report.is_ship_ready(),
        "PROD-AUTH-101 Fail must block ship-readiness"
    );
}
