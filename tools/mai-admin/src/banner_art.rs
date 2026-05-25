//! Embedded banner assets for `mai-admin demo`.
//!
//! The full lamprey art (~146 columns wide) is the WELCOME-01 boot
//! banner; the mini-banner fires when the terminal is narrower than
//! `MIN_WIDE_BANNER_COLS` so the launch never spills wrapped art into
//! the user's scrollback.

/// Outline-style ASCII rendering of the Lamprey compliance-governance
/// logo. Source asset at `docs/assets/lamprey-banner.txt`, baked in
/// at compile time so the binary stays self-contained.
pub const LAMPREY_ART: &str = include_str!("../../../docs/assets/lamprey-banner.txt");

/// 5-line text-only banner used when the terminal is too narrow for
/// the full art. Mirrors the wordmark in the source PNG.
pub const MINI_BANNER: &str = "\
   _                                 \n\
  | |    __ _ _ __ ___  _ __ _ __ ___  _   _ \n\
  | |   / _` | '_ ` _ \\| '__| '_ ` _ \\| | | |\n\
  | |__| (_| | | | | | | |  | | | | | | |_| |\n\
  |_____\\__,_|_| |_| |_|_|  |_| |_| |_|\\__, |\n\
                                       |___/ \n";

/// Minimum terminal width (in columns) at which the full lamprey art
/// renders without wrapping. The source asset is 146 cols at its
/// widest line, so we want at least that plus a small right margin.
pub const MIN_WIDE_BANNER_COLS: u16 = 150;
