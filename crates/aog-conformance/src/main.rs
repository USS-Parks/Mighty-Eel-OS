//! `aog-conformance` binary — run the Loom conformance suite and emit a JSON
//! report. Exit 0 when green (no asserted bar failed), 1 otherwise, so a CI lane
//! or a customer can gate on it directly.

use std::io::Write;
use std::process::ExitCode;

#[tokio::main]
async fn main() -> ExitCode {
    let report = aog_conformance::run().await;
    let json = serde_json::to_string_pretty(&report)
        .unwrap_or_else(|e| format!("{{\"error\":\"serialize report: {e}\"}}"));

    // Write through a stdout handle rather than the `println!` macro, which the
    // workspace lints deny (`print_stdout`).
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "{json}");

    if report.is_green() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}
