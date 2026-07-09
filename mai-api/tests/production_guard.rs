//! Integration test for the production guard.
//!
//! Confirms the on-disk `deployment/ship/profile.toml` evaluates clean
//! through the production guard (modulo deferred runtime checks), and
//! that the `mai-api validate` subcommand returns the expected exit
//! code and report shape.

use std::path::PathBuf;
use std::process::Command;

use mai_api::production_guard::{CheckStatus, ProductionReadinessReport};
use mai_api::ship_profile::load_ship_profile;

fn workspace_path(rel: &[&str]) -> PathBuf {
    let manifest_dir =
        std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR must be set by cargo");
    let mut p = PathBuf::from(manifest_dir)
        .parent()
        .expect("mai-api manifest dir must have a parent")
        .to_path_buf();
    for component in rel {
        p.push(component);
    }
    p
}

#[test]
fn production_guard_baseline_profile_is_ship_ready() {
    let path = workspace_path(&["deployment", "ship", "profile.toml"]);
    let profile = load_ship_profile(&path)
        .unwrap_or_else(|e| panic!("loading {} must succeed: {e}", path.display()));
    let report = ProductionReadinessReport::evaluate(&profile);
    report.assert_ids_unique();
    assert!(
        report.is_ship_ready(),
        "deployment/ship/profile.toml must be ship-ready. Report:\n{}",
        report.render_human()
    );

    // Every check is either Pass or Deferred (no Fail, no Skipped on a
    // production profile).
    for c in &report.checks {
        assert!(
            matches!(c.status, CheckStatus::Pass | CheckStatus::Deferred),
            "{} is {:?}, expected Pass or Deferred",
            c.id,
            c.status
        );
    }

    // Counts sanity.
    let counts = report.counts();
    assert_eq!(counts.fail, 0);
    assert_eq!(counts.skipped, 0);
    assert!(counts.pass > 0);
    assert!(counts.deferred > 0);
}

#[test]
fn production_guard_production_example_template_is_ship_ready() {
    let path = workspace_path(&["config", "production.example.toml"]);
    let profile = load_ship_profile(&path)
        .unwrap_or_else(|e| panic!("loading {} must succeed: {e}", path.display()));
    let report = ProductionReadinessReport::evaluate(&profile);
    assert!(
        report.is_ship_ready(),
        "config/production.example.toml must be ship-ready. Report:\n{}",
        report.render_human()
    );
}

/// Exercise the `mai-api validate` subcommand end-to-end against the
/// real CLI binary. Uses `CARGO_BIN_EXE_lamprey-mai-api` which cargo populates
/// for integration tests of bin targets.
#[test]
fn production_guard_cli_exits_zero_on_baseline_profile() {
    let bin = env!("CARGO_BIN_EXE_lamprey-mai-api");
    let profile = workspace_path(&["deployment", "ship", "profile.toml"]);
    let output = Command::new(bin)
        .arg("validate")
        .arg("--profile")
        .arg(&profile)
        .output()
        .expect("spawn mai-api validate");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "mai-api validate must exit 0 on baseline. stdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(stdout.contains("MAI Production Readiness: PASS"));
    assert!(stdout.contains("PROD-CONFIG-001"));
}

#[test]
fn production_guard_cli_json_form_emits_parsable_report() {
    let bin = env!("CARGO_BIN_EXE_lamprey-mai-api");
    let profile = workspace_path(&["deployment", "ship", "profile.toml"]);
    let output = Command::new(bin)
        .arg("validate")
        .arg("--profile")
        .arg(&profile)
        .arg("--json")
        .output()
        .expect("spawn mai-api validate --json");
    assert!(output.status.success(), "validate --json must exit 0");
    let stdout = String::from_utf8(output.stdout).expect("stdout must be utf-8");
    let report: ProductionReadinessReport =
        serde_json::from_str(&stdout).expect("--json output must deserialize");
    assert!(report.is_ship_ready());
    assert!(report.find("PROD-CONFIG-001").is_some());
}

#[test]
fn production_guard_cli_exit_code_2_on_missing_profile() {
    let bin = env!("CARGO_BIN_EXE_lamprey-mai-api");
    let output = Command::new(bin)
        .arg("validate")
        .arg("--profile")
        .arg("/nonexistent/path/to/profile.toml")
        .output()
        .expect("spawn mai-api validate");
    assert_eq!(
        output.status.code(),
        Some(2),
        "exit code must be 2 (state unreadable). stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn production_guard_cli_exit_code_2_when_profile_flag_missing() {
    let bin = env!("CARGO_BIN_EXE_lamprey-mai-api");
    let output = Command::new(bin)
        .arg("validate")
        .output()
        .expect("spawn mai-api validate");
    assert_eq!(
        output.status.code(),
        Some(2),
        "exit code must be 2 when --profile missing. stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}
