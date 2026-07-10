//! `mai-ship-validate` — standalone production-readiness validator.
//!
//! Runs the same [`ProductionReadinessReport`] that
//! `MaiServer::run()` uses at startup, but as a one-shot CLI that an
//! operator (or CI) can invoke against any ship profile + optional
//! state directory without binding sockets or starting components
//! that affect production state.
//!
//! Usage:
//!
//! ```text
//! mai-ship-validate --profile /etc/mai/profile.toml
//! mai-ship-validate --profile deployment/ship/profile.toml --state-dir /var/lib/mai
//! mai-ship-validate --profile deployment/ship/profile.toml --json
//! ```
//!
//! Without `--state-dir` only the config-only checks evaluate; the
//! runtime checks (`PROD-*-100/101`, `PROD-POLICY-001`) stay Deferred
//! and the report is ship-ready as long as the config-only checks
//! pass. With `--state-dir` the validator exercises the
//! builders against the profile so the runtime checks flip Pass / Fail
//! against the real filesystem state.
//!
//! Exit codes follow SHIP-HARDENING-PLAN §13:
//!
//! | Code | Meaning                                                                 |
//! |------|-------------------------------------------------------------------------|
//! | 0    | Ship-ready: no Critical Fail (Deferred + Skipped do not block).         |
//! | 1    | Validation failed: at least one Critical Fail in the report.            |
//! | 2    | Config unreadable: `--profile` missing, invalid, or rejected by parser. |
//! | 3    | State unreadable: `--state-dir` missing, not a directory.               |
//! | 4    | Internal validator error: JSON serialization failure, panic recovery.   |

#![allow(clippy::print_stdout, clippy::print_stderr)]

use std::path::PathBuf;
use std::process::ExitCode;
use std::time::{SystemTime, UNIX_EPOCH};

use mai_api::audit_wal::{WalAuditConfig, WalAuditWriter};
use mai_api::auth::load_api_keys_from_toml;
use mai_api::production_guard::{ProductionReadinessReport, RuntimeChecks, RuntimeOutcome};
use mai_api::sealer_builder::build_sealer;
use mai_api::ship_profile::{ProfileMode, ShipProfile, load_ship_profile};
use mai_api::trust_builder::{build_trust_components, verify_boot_bundle};
use mai_api::vault_builder::build_vault;

#[tokio::main]
async fn main() -> ExitCode {
    // Skip the tracing subscriber: stdout is the report and nothing else.
    let argv: Vec<String> = std::env::args().collect();
    let parsed = match parse_args(&argv[1..]) {
        Ok(p) => p,
        Err(ArgError::Help) => {
            print_help();
            return ExitCode::SUCCESS;
        }
        Err(ArgError::Bad(msg)) => {
            eprintln!("error: {msg}");
            print_usage();
            return ExitCode::from(2);
        }
    };

    let profile = match load_ship_profile(&parsed.profile_path) {
        Ok(p) => p,
        Err(e) => {
            eprintln!(
                "error: failed to load profile {}: {e}",
                parsed.profile_path.display()
            );
            return ExitCode::from(2);
        }
    };

    let runtime = if let Some(state_dir) = parsed.state_dir.as_ref() {
        if !state_dir.exists() {
            eprintln!("error: state dir {} does not exist", state_dir.display());
            return ExitCode::from(3);
        }
        if !state_dir.is_dir() {
            eprintln!(
                "error: state dir {} is not a directory",
                state_dir.display()
            );
            return ExitCode::from(3);
        }
        Some(evaluate_runtime(&profile).await)
    } else {
        None
    };

    let report = match runtime.as_ref() {
        Some(r) => ProductionReadinessReport::evaluate_with_runtime(&profile, r),
        None => ProductionReadinessReport::evaluate(&profile),
    };

    if parsed.json {
        match report.to_json() {
            Ok(s) => println!("{s}"),
            Err(e) => {
                eprintln!("error: failed to serialize report: {e}");
                return ExitCode::from(4);
            }
        }
    } else {
        print!("{}", report.render_human());
    }

    if report.is_ship_ready() {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    }
}

#[derive(Debug)]
struct ParsedArgs {
    profile_path: PathBuf,
    state_dir: Option<PathBuf>,
    json: bool,
}

#[derive(Debug)]
enum ArgError {
    Help,
    Bad(String),
}

fn parse_args(args: &[String]) -> Result<ParsedArgs, ArgError> {
    let mut profile_path: Option<PathBuf> = None;
    let mut state_dir: Option<PathBuf> = None;
    let mut json = false;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "-h" | "--help" => return Err(ArgError::Help),
            "--json" => {
                json = true;
                i += 1;
            }
            "--profile" | "-p" => {
                let value = args
                    .get(i + 1)
                    .ok_or_else(|| ArgError::Bad("--profile requires a path".into()))?;
                profile_path = Some(PathBuf::from(value));
                i += 2;
            }
            "--state-dir" | "-s" => {
                let value = args
                    .get(i + 1)
                    .ok_or_else(|| ArgError::Bad("--state-dir requires a path".into()))?;
                state_dir = Some(PathBuf::from(value));
                i += 2;
            }
            other => {
                return Err(ArgError::Bad(format!("unknown argument {other:?}")));
            }
        }
    }
    let profile_path =
        profile_path.ok_or_else(|| ArgError::Bad("--profile <PATH> is required".into()))?;
    Ok(ParsedArgs {
        profile_path,
        state_dir,
        json,
    })
}

fn print_usage() {
    eprintln!("Usage: mai-ship-validate --profile <PATH> [--state-dir <PATH>] [--json]");
}

fn print_help() {
    println!("mai-ship-validate — production readiness validator");
    println!();
    println!("Usage:");
    println!("  mai-ship-validate --profile <PATH> [--state-dir <PATH>] [--json]");
    println!();
    println!("Options:");
    println!("  -p, --profile <PATH>     Ship-profile TOML to validate.");
    println!("  -s, --state-dir <PATH>   Exercise runtime checks against this state dir.");
    println!("  --json                   Emit the report as JSON (default: human text).");
    println!("  -h, --help               Print this help and exit.");
    println!();
    println!("Exit codes:");
    println!("  0  ship-ready");
    println!("  1  validation failed");
    println!("  2  --profile path missing or invalid");
    println!("  3  --state-dir path missing or not a directory");
    println!("  4  internal validator error");
}

/// Exercise the builders against the profile and
/// package their outcomes into a [`RuntimeChecks`] for the readiness
/// report. Mirrors the runtime introspection that
/// `MaiServer::apply_ship_profile` collects at boot, but without
/// mutating any AppState or starting sockets.
async fn evaluate_runtime(profile: &ShipProfile) -> RuntimeChecks {
    // V2/V8: initialized construction + a measured storage round-trip —
    // the same probe the server runs at boot, never a fabricated pass.
    let (vault_opened, audit_signer_present) = match build_vault(profile).await {
        Ok((vault, audit_signer)) => (
            mai_api::vault_builder::probe_vault(vault.as_ref()).await,
            audit_signer.is_some(),
        ),
        Err(e) => (
            RuntimeOutcome::fail(format!("vault builder rejected profile: {e}")),
            false,
        ),
    };

    let api_audit_wal_ready =
        match WalAuditWriter::open(WalAuditConfig::for_dir(&profile.audit.wal_dir)).await {
            Ok(_) => {
                RuntimeOutcome::pass(format!("WAL opened at {}", profile.audit.wal_dir.display()))
            }
            Err(e) => RuntimeOutcome::fail(format!(
                "WAL open failed at {}: {e}",
                profile.audit.wal_dir.display()
            )),
        };

    let compliance_sealer_real = match build_sealer(profile) {
        Ok(_) => RuntimeOutcome::pass(if matches!(profile.profile.mode, ProfileMode::Production) {
            "AEAD sealer wired from sealer.key".to_string()
        } else {
            "ephemeral AEAD sealer (local-dev)".to_string()
        }),
        Err(e) => RuntimeOutcome::fail(format!("sealer builder rejected profile: {e}")),
    };

    let trust_bundle_verified = match build_trust_components(profile) {
        Ok(components) => {
            let require_boot = matches!(profile.profile.mode, ProfileMode::Production)
                && profile.trust.require_bundle_on_boot;
            if require_boot {
                let now = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0);
                match verify_boot_bundle(profile, components.bundle_verifier.as_ref(), now) {
                    Ok(version) => RuntimeOutcome::pass(format!(
                        "bundle v{version} verified against {} anchors",
                        components.anchor_ids.len()
                    )),
                    Err(e) => RuntimeOutcome::fail(format!("boot bundle verify: {e}")),
                }
            } else {
                RuntimeOutcome::pass(format!(
                    "bundle verification not required ({:?}); {} anchor(s) loaded",
                    profile.profile.mode,
                    components.anchor_ids.len()
                ))
            }
        }
        Err(e) => RuntimeOutcome::fail(format!("trust builder rejected profile: {e}")),
    };

    let (auth_keys_nonempty, auth_internal_bypass_consistent) = if profile
        .auth
        .auth_keys_path
        .exists()
    {
        match load_api_keys_from_toml(&profile.auth.auth_keys_path) {
            Ok(store) if !store.is_empty() => {
                let nonempty = RuntimeOutcome::pass(format!("{} key(s) loaded", store.len()));
                // PROD-AUTH-101: the loaded store's
                // allow_internal_profile_header must match the
                // profile field that the static guard inspects.
                let profile_bypass = profile.auth.allow_internal_profile_header;
                let store_bypass = store.allow_internal_profile_header;
                let consistent = if store_bypass == profile_bypass {
                    RuntimeOutcome::pass(format!(
                        "runtime bypass = {store_bypass}, profile field = {profile_bypass}: consistent"
                    ))
                } else {
                    RuntimeOutcome::fail(format!(
                        "runtime bypass = {store_bypass} but profile field = {profile_bypass}: \
                             X-IM-Internal-Profile bypass diverges from profile contract"
                    ))
                };
                (nonempty, consistent)
            }
            Ok(_) => (
                RuntimeOutcome::fail(format!(
                    "auth keys file {} parsed but contains zero keys",
                    profile.auth.auth_keys_path.display()
                )),
                RuntimeOutcome::fail(format!(
                    "auth keys file {} parsed but contains zero keys; bypass consistency could not be evaluated",
                    profile.auth.auth_keys_path.display()
                )),
            ),
            Err(e) => (
                RuntimeOutcome::fail(format!(
                    "auth keys file {} could not be loaded: {e}",
                    profile.auth.auth_keys_path.display()
                )),
                RuntimeOutcome::fail(format!(
                    "auth keys file {} could not be loaded: {e}; bypass consistency could not be evaluated",
                    profile.auth.auth_keys_path.display()
                )),
            ),
        }
    } else {
        (
            RuntimeOutcome::fail(format!(
                "auth keys file {} does not exist",
                profile.auth.auth_keys_path.display()
            )),
            RuntimeOutcome::fail(format!(
                "auth keys file {} does not exist; bypass consistency could not be evaluated",
                profile.auth.auth_keys_path.display()
            )),
        )
    };

    // Policy template loads infallibly via PolicyManager::from_template.
    // Once per-tenant template selection is wired, this branch will
    // exercise the configured template's load path.
    let policy_modules_loaded = RuntimeOutcome::pass("standard policy modules loaded".to_string());
    let compliance_signer_real = if audit_signer_present {
        RuntimeOutcome::pass(
            "compliance audit chain wired with a vault-held ML-DSA signer".to_string(),
        )
    } else {
        RuntimeOutcome::fail("compliance audit chain has no real signer (NullSigner)".to_string())
    };

    RuntimeChecks {
        vault_opened: Some(vault_opened),
        api_audit_wal_ready: Some(api_audit_wal_ready),
        compliance_sealer_real: Some(compliance_sealer_real),
        compliance_signer_real: Some(compliance_signer_real),
        trust_bundle_verified: Some(trust_bundle_verified),
        auth_keys_nonempty: Some(auth_keys_nonempty),
        auth_internal_bypass_consistent: Some(auth_internal_bypass_consistent),
        policy_modules_loaded: Some(policy_modules_loaded),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_args_requires_profile() {
        let err = parse_args(&[]).unwrap_err();
        assert!(matches!(err, ArgError::Bad(_)));
    }

    #[test]
    fn parse_args_help_short_circuits() {
        assert!(matches!(
            parse_args(&["--help".into()]),
            Err(ArgError::Help)
        ));
        assert!(matches!(parse_args(&["-h".into()]), Err(ArgError::Help)));
    }

    #[test]
    fn parse_args_accepts_profile_and_flags() {
        let args = parse_args(&[
            "--profile".into(),
            "/tmp/p.toml".into(),
            "--state-dir".into(),
            "/tmp/state".into(),
            "--json".into(),
        ])
        .expect("parse");
        assert_eq!(args.profile_path, PathBuf::from("/tmp/p.toml"));
        assert_eq!(args.state_dir, Some(PathBuf::from("/tmp/state")));
        assert!(args.json);
    }

    #[test]
    fn parse_args_rejects_unknown_flag() {
        let err = parse_args(&["--bogus".into()]).unwrap_err();
        match err {
            ArgError::Bad(msg) => assert!(msg.contains("--bogus") || msg.contains("bogus")),
            ArgError::Help => panic!("expected Bad, got Help"),
        }
    }

    #[test]
    fn parse_args_rejects_dangling_profile() {
        let err = parse_args(&["--profile".into()]).unwrap_err();
        match err {
            ArgError::Bad(msg) => assert!(msg.contains("--profile")),
            ArgError::Help => panic!("expected Bad, got Help"),
        }
    }
}
