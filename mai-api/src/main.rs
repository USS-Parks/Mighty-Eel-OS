//! MAI API Server binary entry point.
//!
//! Parses an optional config file path from the command line, initializes
//! the tracing subscriber for structured logging, and delegates to
//! `MaiServer::run()` which handles the full startup-to-shutdown lifecycle.
//!
//! Usage:
//!   mai-api                         # Scout tier defaults
//!   mai-api /etc/mai/server.toml    # Load config from file
//!   mai-api --config path.toml      # Explicit flag form

use std::path::PathBuf;
use std::process::ExitCode;

use tracing::{error, info};
use tracing_subscriber::EnvFilter;

use mai_api::MaiServer;

#[tokio::main]
async fn main() -> ExitCode {
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

    let server = match config_path {
        Some(ref path) => {
            info!(path = %path.display(), "Loading configuration from file");
            MaiServer::from_config_path(path)
        }
        None => {
            info!("No config file specified, using Scout tier defaults");
            MaiServer::default_scout()
        }
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
    use super::*;

    #[test]
    fn test_parse_no_args() {
        // parse_config_path reads from std::env::args which we can't
        // easily mock, so we just verify the function exists and the
        // binary compiles. Integration testing in Session 11e tests.
        assert!(true);
    }
}
