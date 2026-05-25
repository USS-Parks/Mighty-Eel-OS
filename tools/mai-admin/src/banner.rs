//! Boot banner for `mai-admin demo`.
//!
//! Renders the embedded lamprey art when the terminal is wide enough
//! and the destination is an actual TTY; otherwise falls back to the
//! plain `MINI_BANNER`. No new runtime dependencies — width detection
//! reads the `COLUMNS` env var, color detection uses the stdlib
//! `IsTerminal` trait plus the conventional `NO_COLOR` / `TERM=dumb`
//! escape hatches.

use std::env;
use std::io::{self, IsTerminal, Write};

use crate::banner_art::{LAMPREY_ART, MIN_WIDE_BANNER_COLS, MINI_BANNER};

/// Print the boot banner appropriate for the current terminal.
///
/// `freeze` is rendered next to the wordmark (e.g. the short SHA of
/// the freeze commit). The function never errors; an unwriteable
/// stdout is silently skipped because a banner is cosmetic.
pub fn print_boot_banner(freeze: &str) {
    let mut out = io::stdout().lock();
    let color = ColorMode::detect();
    let cols = terminal_columns();

    if cols >= MIN_WIDE_BANNER_COLS {
        // Wide-terminal path: render the full lamprey art, then the
        // wordmark + freeze metadata.
        let _ = out.write_all(LAMPREY_ART.as_bytes());
        let _ = writeln!(out);
        let _ = writeln!(
            out,
            "    {bold}Island Mountain MAI + Lamprey{reset}   freeze {dim}{freeze}{reset}",
            bold = color.bold(),
            reset = color.reset(),
            dim = color.dim(),
            freeze = freeze
        );
    } else {
        // Narrow path: text-only mini-banner avoids wrapping the art.
        let _ = out.write_all(MINI_BANNER.as_bytes());
        let _ = writeln!(
            out,
            "  Island Mountain MAI + Lamprey  freeze {freeze}",
            freeze = freeze
        );
    }
    let _ = writeln!(out);
}

/// Whether ANSI color escapes should be emitted on this run.
#[derive(Clone, Copy, Debug)]
pub struct ColorMode(bool);

impl ColorMode {
    /// Detect color support: stdout is a TTY, `NO_COLOR` is unset,
    /// `TERM` is not "dumb". Honors `CLICOLOR_FORCE=1` as an override
    /// that re-enables color even when stdout isn't a TTY (useful for
    /// piping into colored pagers).
    pub fn detect() -> Self {
        let force = env::var_os("CLICOLOR_FORCE").is_some_and(|v| v == "1");
        if force {
            return Self(true);
        }
        if env::var_os("NO_COLOR").is_some() {
            return Self(false);
        }
        if env::var("TERM").map(|t| t == "dumb").unwrap_or(false) {
            return Self(false);
        }
        Self(io::stdout().is_terminal())
    }

    pub const fn cyan(self) -> &'static str {
        if self.0 { "\x1b[36m" } else { "" }
    }
    pub const fn green(self) -> &'static str {
        if self.0 { "\x1b[32m" } else { "" }
    }
    pub const fn yellow(self) -> &'static str {
        if self.0 { "\x1b[33m" } else { "" }
    }
    pub const fn magenta(self) -> &'static str {
        if self.0 { "\x1b[35m" } else { "" }
    }
    pub const fn red(self) -> &'static str {
        if self.0 { "\x1b[31m" } else { "" }
    }
    pub const fn dim(self) -> &'static str {
        if self.0 { "\x1b[2m" } else { "" }
    }
    pub const fn bold(self) -> &'static str {
        if self.0 { "\x1b[1m" } else { "" }
    }
    pub const fn reset(self) -> &'static str {
        if self.0 { "\x1b[0m" } else { "" }
    }
}

/// Best-effort terminal width via the `COLUMNS` env var. Returns a
/// sane default of 120 cols when unset or unparseable. We avoid
/// pulling `crossterm` just for `terminal::size()` — the env-var
/// approach catches every modern shell (PowerShell, bash, zsh, fish,
/// cmd.exe via Windows Terminal) and degrades to "assume wide" only
/// in edge cases like cron jobs and CI runners, where the banner
/// being slightly wrong is not a blocker.
fn terminal_columns() -> u16 {
    env::var("COLUMNS")
        .ok()
        .and_then(|v| v.parse::<u16>().ok())
        .unwrap_or(120)
}
