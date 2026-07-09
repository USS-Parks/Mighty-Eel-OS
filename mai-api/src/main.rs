//! MAI API Server binary entry point.
//!
//! Parses an optional config file path from the command line, initializes
//! the tracing subscriber for structured logging, and delegates to
//! `MaiServer::run()` which handles the full startup-to-shutdown lifecycle.
//!
//! Usage:
//!   mai-api                                       # Scout tier defaults
//!   mai-api /etc/mai/server.toml                  # Load config from file
//!   mai-api --config path.toml                    # Explicit flag form
//!   mai-api validate --profile deployment/ship/profile.toml [--json]
//!                                                 # readiness check

#![allow(clippy::print_stdout, clippy::print_stderr)]

use std::path::PathBuf;
use std::process::ExitCode;

use tracing::{error, info};
use tracing_subscriber::EnvFilter;

use mai_api::MaiServer;
use mai_api::production_guard::ProductionReadinessReport;
use mai_api::ship_profile::load_ship_profile;

#[tokio::main]
async fn main() -> ExitCode {
    // The `validate` subcommand bypasses the tracing subscriber
    // so its stdout is the report and nothing else. Other paths set
    // tracing up first.
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(|s| s.as_str()) == Some("validate") {
        return run_validate_subcommand(&args[2..]);
    }

    // Initialize structured logging with RUST_LOG env filter support.
    // Default level: info for mai crates, warn for everything else.
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("mai_api=info,mai_core=info,mai_hil=info,warn")),
        )
        .with_target(true)
        .with_thread_ids(false)
        .with_file(false)
        .init();

    info!(
        version = env!("CARGO_PKG_VERSION"),
        "Island Mountain MAI API Server"
    );

    // Parse config path from command line arguments.
    let config_path = parse_config_path();

    let server = if let Some(ref path) = config_path {
        info!(path = %path.display(), "Loading configuration from file");
        MaiServer::from_config_path(path)
    } else {
        info!("No config file specified, using Scout tier defaults");
        MaiServer::default_scout()
    };

    // Run the server (blocks until shutdown signal).
    match server.run().await {
        Ok(()) => {
            info!("MAI server exited cleanly");
            ExitCode::SUCCESS
        }
        Err(e) => {
            error!(error = %e, "MAI server exited with error");
            ExitCode::FAILURE
        }
    }
}

/// Stop-gap CLI. The full `mai-ship-validate` binary lands later;
/// this subcommand exists so operators can dry-run the
/// production guard against a profile today.
///
/// Exit codes:
/// - 0: every Critical check passes (Deferred checks are not failures)
/// - 1: at least one Critical check failed
/// - 2: profile could not be loaded
//
// SCAN-1 (Code Quality QUA-005): the `println!`/`eprintln!` calls
// below are deliberate CLI output (--help text, --json report on
// stdout, error lines on stderr — `tracing` is not initialized at
// this point in the lifecycle). Annotated so future scans treat
// this function as PASS instead of flagging it as debug spam. See
// `docs/SCAN-1-SECURITY-FALSE-POSITIVES.md` §QUA-005.
#[allow(clippy::print_stdout, clippy::print_stderr)]
fn run_validate_subcommand(args: &[String]) -> ExitCode {
    let mut profile_path: Option<PathBuf> = None;
    let mut json = false;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--profile" | "-p" => {
                if i + 1 >= args.len() {
                    eprintln!("error: --profile requires a path argument");
                    return ExitCode::from(2);
                }
                profile_path = Some(PathBuf::from(&args[i + 1]));
                i += 2;
            }
            "--json" => {
                json = true;
                i += 1;
            }
            "-h" | "--help" => {
                println!("Usage: mai-api validate --profile <PATH> [--json]");
                println!();
                println!("Run the production readiness guard against a");
                println!("ship profile. Config-only checks today; runtime checks");
                println!("(vault open, audit append, trust bundle verify) are not");
                println!("yet wired and currently report DEFERRED.");
                return ExitCode::SUCCESS;
            }
            other => {
                eprintln!("error: unknown argument {other:?}");
                return ExitCode::from(2);
            }
        }
    }

    let Some(path) = profile_path else {
        eprintln!("error: --profile <PATH> is required");
        return ExitCode::from(2);
    };

    let profile = match load_ship_profile(&path) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("error: failed to load {}: {e}", path.display());
            return ExitCode::from(2);
        }
    };

    let report = ProductionReadinessReport::evaluate(&profile);

    if json {
        match report.to_json() {
            Ok(s) => println!("{s}"),
            Err(e) => {
                eprintln!("error: failed to serialize report: {e}");
                return ExitCode::from(2);
            }
        }
    } else {
        print!("{}", report.render_human());
    }

    if report.is_ship_ready() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

/// Extract config file path from CLI arguments.
///
/// Accepts:
///   mai-api /path/to/config.toml
///   mai-api --config /path/to/config.toml
///   mai-api -c /path/to/config.toml
fn parse_config_path() -> Option<PathBuf> {
    let args: Vec<String> = std::env::args().collect();

    if args.len() < 2 {
        return None;
    }

    // --config <path> or -c <path>
    if (args[1] == "--config" || args[1] == "-c") && args.len() >= 3 {
        return Some(PathBuf::from(&args[2]));
    }

    // Bare path argument (no flag)
    if !args[1].starts_with('-') {
        return Some(PathBuf::from(&args[1]));
    }

    // --help: print usage and exit (not a real arg parser, just enough)
    if args[1] == "--help" || args[1] == "-h" {
        eprintln!("Usage: mai-api [OPTIONS] [CONFIG_PATH]");
        eprintln!();
        eprintln!("Options:");
        eprintln!("  -c, --config <PATH>  Path to server.toml configuration file");
        eprintln!("  -h, --help           Print this help message");
        eprintln!();
        eprintln!("If no config file is specified, Scout tier defaults are used.");
        eprintln!("The server binds to 127.0.0.1:8420 (REST) and 127.0.0.1:8421 (gRPC).");
        std::process::exit(0);
    }

    None
}

#[cfg(test)]
mod tests {
    use super::parse_config_path;

    #[test]
    fn test_parse_no_args() {
        // parse_config_path reads from std::env::args which we can't
        // easily mock; the assertion just pins the function signature
        // so future refactors that drop or rename it fail this test.
        let _: fn() -> Option<std::path::PathBuf> = parse_config_path;
    }
}
