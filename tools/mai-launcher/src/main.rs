//! `mai.exe` — the front-door launcher (WELCOME-01).
//!
//! What a user sees when they double-click the program:
//!   1. A console window opens (we are a console-subsystem app, on
//!      purpose, so the terminal UI has somewhere to live).
//!   2. The Win32 splash window floats above the console for 2 sec,
//!      showing the gold Lamprey MAI badge.
//!   3. The splash dismisses (timer or click); the console clears
//!      and the ASCII lamprey banner is printed at the top, stapled
//!      above an interactive prompt.
//!   4. Commands: `demo`, `start`, `status`, `help`, `quit`. Both
//!      `demo` (narrated compliance walk) and `start` (supervise
//!      the headless `mai-api.exe` daemon) shell out to sibling
//!      binaries discovered next to the launcher.
//!
//! The launcher intentionally stays small: it owns the *experience*,
//! not the implementation. Compliance lives in `mai-admin demo`,
//! inference lives in `mai-api`. `mai.exe` is the front door.

#![allow(unsafe_code)]

mod splash;

use std::env;
use std::io::{self, IsTerminal, Write};
use std::path::PathBuf;
use std::process::{Command, ExitStatus};

/// Embedded ASCII lamprey banner. Reused from the canonical asset
/// committed in `docs/assets/lamprey-banner.txt`.
const LAMPREY_ASCII: &str = include_str!("../../../docs/assets/lamprey-banner.txt");

/// Default duration the gold-badge splash floats before auto-dismiss.
/// Override via `MAI_SPLASH_MS=<ms>`; set to `0` to skip the splash
/// entirely (developer / CI flow).
const SPLASH_DEFAULT_MS: u32 = 2000;

fn main() {
    let splash_ms = env::var("MAI_SPLASH_MS")
        .ok()
        .and_then(|v| v.parse::<u32>().ok())
        .unwrap_or(SPLASH_DEFAULT_MS);

    if splash_ms > 0
        && let Err(e) = splash::show_splash(splash_ms)
    {
        // Splash is cosmetic — if it fails (no Win32 access, no
        // display, weird remote shell, etc.) we degrade to a
        // text-only welcome rather than abort the launcher.
        eprintln!("splash skipped: {e:#}");
    }

    if let Err(e) = run_terminal_ui() {
        eprintln!("launcher error: {e:#}");
        std::process::exit(1);
    }
}

fn run_terminal_ui() -> io::Result<()> {
    clear_screen();
    print_banner();
    print_intro();

    let stdin = io::stdin();
    let mut line = String::new();
    loop {
        prompt();
        line.clear();
        if stdin.read_line(&mut line)? == 0 {
            // EOF (piped input exhausted, or terminal closed).
            writeln!(io::stdout())?;
            return Ok(());
        }
        let cmd = line.trim();
        if cmd.is_empty() {
            continue;
        }
        match cmd {
            "q" | "quit" | "exit" => {
                writeln!(io::stdout(), "  goodbye.")?;
                return Ok(());
            }
            "h" | "help" | "?" => print_help(),
            "demo" => run_sibling("mai-admin", &["demo", "all"])?,
            "demo --no-pacing" | "demo nopace" => {
                // Convenience: faster playback for re-runs.
                unsafe {
                    env::set_var("MAI_DEMO_PACING_MS", "0");
                }
                run_sibling("mai-admin", &["demo", "all"])?;
            }
            cmd if cmd.starts_with("demo ") => {
                let scenario = cmd.trim_start_matches("demo ").trim();
                run_sibling("mai-admin", &["demo", "run", scenario])?;
            }
            "start" => run_sibling("mai-api", &[])?,
            "status" => print_status(),
            other => {
                writeln!(
                    io::stdout(),
                    "  unknown command `{other}` — type `help` for the list."
                )?;
            }
        }
    }
}

fn prompt() {
    let mut out = io::stdout().lock();
    let _ = write!(out, "\nmai> ");
    let _ = out.flush();
}

fn print_banner() {
    let mut out = io::stdout().lock();
    let _ = out.write_all(LAMPREY_ASCII.as_bytes());
    let _ = writeln!(out);
}

fn print_intro() {
    let mut out = io::stdout().lock();
    let _ = writeln!(
        out,
        "  Island Mountain MAI + Lamprey   build {}",
        env!("CARGO_PKG_VERSION")
    );
    let _ = writeln!(
        out,
        "  air-gapped local AI inference + compliance governance"
    );
    let _ = writeln!(out);
    let _ = writeln!(out, "  Commands:  demo  start  status  help  quit");
    let _ = writeln!(
        out,
        "             demo <hipaa|itar|ocap|multi|tamper|trust>  for a single scenario"
    );
}

fn print_help() {
    let mut out = io::stdout().lock();
    let _ = writeln!(out);
    let _ = writeln!(out, "  Available commands:");
    let _ = writeln!(
        out,
        "    demo                run all 6 narrated compliance demos (~12 sec)"
    );
    let _ = writeln!(
        out,
        "    demo <name>         one scenario: hipaa, itar, ocap, multi, tamper, trust"
    );
    let _ = writeln!(
        out,
        "    start               launch the mai-api headless daemon (Ctrl-C to stop)"
    );
    let _ = writeln!(
        out,
        "    status              print version + sibling-binary locations"
    );
    let _ = writeln!(out, "    help, h, ?          show this help");
    let _ = writeln!(out, "    quit, exit, q       leave the launcher");
    let _ = writeln!(out);
    let _ = writeln!(out, "  Environment:");
    let _ = writeln!(
        out,
        "    MAI_SPLASH_MS=<n>   override or disable (0) the boot splash"
    );
    let _ = writeln!(
        out,
        "    MAI_DEMO_PACING_MS=<n>  override demo phase pacing (default 150)"
    );
    let _ = writeln!(out, "    NO_COLOR=1          force monochrome output");
}

fn print_status() {
    let mut out = io::stdout().lock();
    let _ = writeln!(out);
    let _ = writeln!(out, "  launcher version    {}", env!("CARGO_PKG_VERSION"));
    let _ = writeln!(out, "  launcher path       {}", current_exe_display());
    let _ = writeln!(out, "  bin directory       {}", bin_dir_display());
    let _ = writeln!(out);
    for sibling in ["mai-api.exe", "mai-admin.exe"] {
        let path = sibling_binary_path(sibling);
        let present = path.as_ref().is_some_and(|p| p.exists());
        let _ = writeln!(
            out,
            "  {sibling:18} {}",
            if present {
                "found"
            } else {
                "MISSING (sibling-binary not next to launcher)"
            }
        );
    }
    let _ = writeln!(out);
    let _ = writeln!(out, "  stdout is_tty       {}", io::stdout().is_terminal());
}

fn current_exe_display() -> String {
    env::current_exe()
        .ok()
        .and_then(|p| p.to_str().map(str::to_owned))
        .unwrap_or_else(|| "<unknown>".to_owned())
}

fn bin_dir_display() -> String {
    bin_dir()
        .and_then(|p| p.to_str().map(str::to_owned))
        .unwrap_or_else(|| "<unknown>".to_owned())
}

fn bin_dir() -> Option<PathBuf> {
    env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(PathBuf::from))
}

fn sibling_binary_path(name: &str) -> Option<PathBuf> {
    bin_dir().map(|d| d.join(name))
}

/// Spawn a sibling binary that lives next to `mai.exe`, inherit
/// stdio so its output streams to this terminal in real time, and
/// wait for it to exit.
fn run_sibling(stem: &str, args: &[&str]) -> io::Result<()> {
    let exe_name = format!("{stem}.exe");
    let path = sibling_binary_path(&exe_name);
    let Some(path) = path else {
        writeln!(
            io::stdout(),
            "  cannot locate launcher exe; sibling lookup failed."
        )?;
        return Ok(());
    };
    if !path.exists() {
        // Fall back to PATH so cargo-run + workspace target/release
        // works in development.
        let status = run_command(stem, args);
        return report_status(stem, status);
    }
    let status = run_command(path.to_string_lossy().as_ref(), args);
    report_status(stem, status)
}

fn run_command(program: &str, args: &[&str]) -> io::Result<ExitStatus> {
    Command::new(program).args(args).status()
}

fn report_status(label: &str, status: io::Result<ExitStatus>) -> io::Result<()> {
    let mut out = io::stdout().lock();
    match status {
        Ok(s) if s.success() => writeln!(out, "  ({label} exited 0)")?,
        Ok(s) => {
            let code = s.code().unwrap_or(-1);
            writeln!(out, "  ({label} exited {code})")?;
        }
        Err(e) => writeln!(out, "  ({label} failed to launch: {e:#})")?,
    }
    Ok(())
}

/// ANSI clear-screen + cursor-home. Modern Windows Terminal,
/// PowerShell 7, and any POSIX terminal handle this; legacy
/// `cmd.exe` ignores it but the banner reprints on top so the
/// scrollback shows the same thing twice — not broken, just less
/// pretty.
fn clear_screen() {
    let mut out = io::stdout().lock();
    let _ = out.write_all(b"\x1b[2J\x1b[H");
    let _ = out.flush();
}
