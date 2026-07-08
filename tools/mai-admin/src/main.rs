//! `mai-admin` CLI entry point.
//!
//! SHIP-09 wires `backup create` and `backup verify`. SHIP-10 adds
//! `restore plan` and `restore apply`. The remaining `audit`, `trust`,
//! and `vault` subcommands ship in later sessions and stub here with a
//! clear exit-with-message so the operator UX of `mai-admin --help`
//! reflects the whole roadmap.
//!
//! Exit codes (stable, mirror SHIP-HARDENING-PLAN.md §13):
//!   0  ok
//!   1  backup / restore / verification failed
//!   2  config / inputs unreadable
//!   3  state unreadable (manifest missing, paths gone)
//!   4  internal error

#![allow(clippy::print_stdout, clippy::print_stderr)]

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use mai_admin::manifest::MLDSA87_PK_LEN;
use mai_admin::profile::load_backup_source_profile;
use mai_admin::restore::{
    RestoreError, RestorePlan, RestoreReport, RestoreSignatureRecord, apply_restore, plan_restore,
};
use mai_admin::{
    BackupOptions, BackupReport, VerifyOutcome, VerifyReport, create_backup, verify_backup,
};

// WELCOME-01: narrated demo runner + boot banner. Binary-only modules
// (not exported from the lib crate) — they pull in mai-compliance + a
// large terminal-rendering surface that operator tooling consumers
// shouldn't transitively depend on.
mod banner;
mod banner_art;
mod demo;

#[derive(Parser, Debug)]
#[command(name = "mai-admin", version, about = "MAI operator tooling")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Backup management. SHIP-09.
    #[command(subcommand)]
    Backup(BackupCmd),
    /// Restore management. SHIP-10.
    #[command(subcommand)]
    Restore(RestoreCmd),
    /// Audit chain verification. Pending session.
    Audit,
    /// Trust bundle verification. Pending session.
    Trust,
    /// Vault status report. Pending session.
    Vault,
    /// Run narrated end-to-end compliance demos (WELCOME-01).
    #[command(subcommand)]
    Demo(DemoCmd),
}

#[derive(Subcommand, Debug)]
enum DemoCmd {
    /// Run every demo in sequence (HIPAA, ITAR, OCAP, Multi-Domain,
    /// Audit-Tamper, Trust-Manifold). Prints the boot banner first.
    All {
        /// Skip the lamprey boot banner (useful for CI or piping).
        #[arg(long, default_value_t = false)]
        no_banner: bool,
    },
    /// Run a single named scenario. Names: `hipaa`, `itar`, `ocap`,
    /// `multi`, `tamper`, `trust`.
    Run {
        /// Scenario name. See `--help` for the list.
        scenario: String,
        /// Skip the lamprey boot banner.
        #[arg(long, default_value_t = false)]
        no_banner: bool,
    },
}

#[derive(Subcommand, Debug)]
enum RestoreCmd {
    /// Plan a restore: load the manifest, verify every backup-side
    /// digest + audit chain, scan the target for conflicts, and emit
    /// the plan without touching the target.
    Plan {
        /// Backup directory (the one containing `manifest.json`).
        #[arg(long)]
        backup_dir: PathBuf,
        /// Target directory the restore would write into. Need not
        /// exist yet — restore creates it.
        #[arg(long)]
        target: PathBuf,
        /// Path to a 2592-byte ML-DSA-87 public key file matching the
        /// manifest's `anchor_id`. Required to verify a signed manifest unless
        /// `--allow-unsigned` is passed.
        #[arg(long)]
        verifying_key: Option<PathBuf>,
        /// Skip the signed-manifest requirement and plan a restore of an
        /// unsigned or unverified backup (AF-19: verification is on by default).
        #[arg(long, default_value_t = false)]
        allow_unsigned: bool,
        /// Emit machine-readable JSON instead of the human report.
        #[arg(long, default_value_t = false)]
        json: bool,
    },
    /// Execute a restore plan: write every component into the target,
    /// re-verify per-component sha3 after each write, replay the audit
    /// chain in the restored tree, drop `restore-report.json` and a
    /// copy of the source manifest at the target root.
    Apply {
        #[arg(long)]
        backup_dir: PathBuf,
        #[arg(long)]
        target: PathBuf,
        #[arg(long)]
        verifying_key: Option<PathBuf>,
        /// Skip the signed-manifest requirement (AF-19: verification is on by
        /// default).
        #[arg(long, default_value_t = false)]
        allow_unsigned: bool,
        /// Overwrite existing files / populated directory trees inside
        /// the target. Refuse to operate on a non-empty target without
        /// this flag (per SHIP-HARDENING-PLAN §9.5: restore must
        /// "refuse to overwrite live state unless --force and service
        /// is stopped").
        #[arg(long, default_value_t = false)]
        force: bool,
        #[arg(long, default_value_t = false)]
        json: bool,
    },
}

#[derive(Subcommand, Debug)]
enum BackupCmd {
    /// Take a new backup against a loaded ship profile.
    Create {
        /// Path to the ship profile TOML.
        #[arg(long)]
        profile: PathBuf,
        /// Parent directory; backup will be created at
        /// `<output>/<backup_id>/`.
        #[arg(long)]
        output: PathBuf,
        /// Optional override for the backup id; default
        /// `mai-backup-<rfc3339-stamp>`.
        #[arg(long)]
        backup_id: Option<String>,
        /// Path to a 4896-byte ML-DSA-87 secret key. When present the
        /// manifest is signed; ship profile requires it.
        #[arg(long)]
        signing_key: Option<PathBuf>,
        /// Stable identifier the verifier looks up the matching
        /// public key under. Required when `--signing-key` is set.
        #[arg(long)]
        anchor_id: Option<String>,
    },
    /// Verify a backup directory: manifest signature + per-component
    /// digests + audit chain replay.
    Verify {
        /// Backup directory (the one containing `manifest.json`).
        #[arg(long)]
        backup_dir: PathBuf,
        /// Path to a 2592-byte ML-DSA-87 public key file matching the
        /// `anchor_id` recorded in the manifest. Required to verify a signed
        /// manifest unless `--allow-unsigned` is passed.
        #[arg(long)]
        verifying_key: Option<PathBuf>,
        /// Skip the signed-manifest requirement and verify an unsigned or
        /// unverified backup (AF-19: verification is on by default).
        #[arg(long, default_value_t = false)]
        allow_unsigned: bool,
        /// Emit machine-readable JSON instead of the human report.
        #[arg(long, default_value_t = false)]
        json: bool,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match cli.command {
        Command::Backup(BackupCmd::Create {
            profile,
            output,
            backup_id,
            signing_key,
            anchor_id,
        }) => run_backup_create(&profile, &output, backup_id, signing_key, anchor_id),
        Command::Backup(BackupCmd::Verify {
            backup_dir,
            verifying_key,
            allow_unsigned,
            json,
        }) => run_backup_verify(&backup_dir, verifying_key, !allow_unsigned, json),
        Command::Restore(RestoreCmd::Plan {
            backup_dir,
            target,
            verifying_key,
            allow_unsigned,
            json,
        }) => run_restore_plan(&backup_dir, &target, verifying_key, !allow_unsigned, json),
        Command::Restore(RestoreCmd::Apply {
            backup_dir,
            target,
            verifying_key,
            allow_unsigned,
            force,
            json,
        }) => run_restore_apply(
            &backup_dir,
            &target,
            verifying_key,
            !allow_unsigned,
            force,
            json,
        ),
        Command::Audit => {
            eprintln!("`mai-admin audit verify` lands in a later session. Pending.");
            ExitCode::from(2)
        }
        Command::Trust => {
            eprintln!("`mai-admin trust verify` lands in a later session. Pending.");
            ExitCode::from(2)
        }
        Command::Vault => {
            eprintln!("`mai-admin vault status` lands in a later session. Pending.");
            ExitCode::from(2)
        }
        Command::Demo(DemoCmd::All { no_banner }) => run_demo(None, no_banner),
        Command::Demo(DemoCmd::Run {
            scenario,
            no_banner,
        }) => run_demo(Some(scenario.as_str()), no_banner),
    }
}

fn run_demo(scenario: Option<&str>, no_banner: bool) -> ExitCode {
    if !no_banner {
        banner::print_boot_banner(env!("CARGO_PKG_VERSION"));
    }
    let result = match scenario {
        Some(name) => demo::run_one(name),
        None => demo::run_all(),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("demo failed: {e:#}");
            ExitCode::from(1)
        }
    }
}

fn run_backup_create(
    profile_path: &Path,
    output_root: &Path,
    backup_id: Option<String>,
    signing_key_path: Option<PathBuf>,
    anchor_id: Option<String>,
) -> ExitCode {
    let profile = match load_backup_source_profile(profile_path) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("error: {e}");
            return ExitCode::from(2);
        }
    };

    let mut options = BackupOptions::from_env(output_root);
    options.backup_id = backup_id;
    if let Some(sk_path) = signing_key_path {
        match std::fs::read(&sk_path) {
            Ok(bytes) => {
                if anchor_id.is_none() {
                    eprintln!("error: --anchor-id is required with --signing-key");
                    return ExitCode::from(2);
                }
                options.signing_key = Some(bytes);
                options.anchor_id = anchor_id;
            }
            Err(e) => {
                eprintln!(
                    "error: could not read signing key {}: {e}",
                    sk_path.display()
                );
                return ExitCode::from(2);
            }
        }
    }

    match create_backup(&profile, options) {
        Ok(report) => {
            print_create_report(&report);
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("backup create failed: {e}");
            ExitCode::from(1)
        }
    }
}

fn run_backup_verify(
    backup_dir: &Path,
    verifying_key_path: Option<PathBuf>,
    require_signed: bool,
    json: bool,
) -> ExitCode {
    let verifying_key = match verifying_key_path {
        Some(path) => match std::fs::read(&path) {
            Ok(bytes) => {
                if bytes.len() != MLDSA87_PK_LEN {
                    eprintln!(
                        "error: verifying key {} has length {} != {MLDSA87_PK_LEN}",
                        path.display(),
                        bytes.len()
                    );
                    return ExitCode::from(2);
                }
                Some(bytes)
            }
            Err(e) => {
                eprintln!(
                    "error: could not read verifying key {}: {e}",
                    path.display()
                );
                return ExitCode::from(2);
            }
        },
        None => None,
    };

    let report = match verify_backup(backup_dir, verifying_key.as_deref(), require_signed) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("verify failed: {e}");
            return ExitCode::from(3);
        }
    };

    if json {
        match serde_json::to_string_pretty(&VerifyJson::from(&report)) {
            Ok(s) => println!("{s}"),
            Err(e) => {
                eprintln!("error: could not serialize verify report: {e}");
                return ExitCode::from(4);
            }
        }
    } else {
        print_verify_report(&report);
    }

    if report.is_clean() {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    }
}

fn print_create_report(report: &BackupReport) {
    println!("backup created: {}", report.backup_id);
    println!("  dir       : {}", report.backup_dir.display());
    println!("  manifest  : {}", report.manifest_path.display());
    println!("  components: {}", report.component_count);
    println!("  signed    : {}", report.signed);
    if !report.warnings.is_empty() {
        println!("  warnings  :");
        for w in &report.warnings {
            println!("    - {w}");
        }
    }
}

fn print_verify_report(report: &VerifyReport) {
    println!("verify backup: {}", report.backup_id);
    println!("  dir       : {}", report.backup_dir.display());
    println!("  signature : {}", outcome_str(&report.signature_outcome));
    println!("  components: {}", report.component_count);
    if !report.warnings.is_empty() {
        println!("  warnings  :");
        for w in &report.warnings {
            println!("    - {w}");
        }
    }
    if report.failures.is_empty() {
        println!("  result    : OK");
    } else {
        println!("  result    : FAIL");
        for f in &report.failures {
            println!("    - {f}");
        }
    }
}

fn outcome_str(o: &VerifyOutcome) -> String {
    match o {
        VerifyOutcome::Signed { anchor_id } => format!("signed by anchor {anchor_id}"),
        VerifyOutcome::Unsigned => "unsigned".to_string(),
    }
}

#[derive(serde::Serialize)]
struct VerifyJson<'a> {
    backup_id: &'a str,
    backup_dir: String,
    signature_outcome: &'static str,
    anchor_id: Option<&'a str>,
    component_count: usize,
    failures: &'a [String],
    warnings: &'a [String],
    ok: bool,
}

impl<'a> From<&'a VerifyReport> for VerifyJson<'a> {
    fn from(r: &'a VerifyReport) -> Self {
        let (outcome, anchor) = match &r.signature_outcome {
            VerifyOutcome::Signed { anchor_id } => ("signed", Some(anchor_id.as_str())),
            VerifyOutcome::Unsigned => ("unsigned", None),
        };
        VerifyJson {
            backup_id: &r.backup_id,
            backup_dir: r.backup_dir.display().to_string(),
            signature_outcome: outcome,
            anchor_id: anchor,
            component_count: r.component_count,
            failures: &r.failures,
            warnings: &r.warnings,
            ok: r.is_clean(),
        }
    }
}

// ─── restore ──────────────────────────────────────────────────────────

fn run_restore_plan(
    backup_dir: &Path,
    target: &Path,
    verifying_key_path: Option<PathBuf>,
    require_signed: bool,
    json: bool,
) -> ExitCode {
    let verifying_key = match load_verifying_key(verifying_key_path) {
        Ok(k) => k,
        Err(code) => return code,
    };
    let plan = match plan_restore(backup_dir, target, verifying_key.as_deref(), require_signed) {
        Ok(p) => p,
        Err(e) => return restore_error_exit(&e),
    };
    if json {
        match serde_json::to_string_pretty(&PlanJson::from(&plan)) {
            Ok(s) => println!("{s}"),
            Err(e) => {
                eprintln!("error: could not serialize restore plan: {e}");
                return ExitCode::from(4);
            }
        }
    } else {
        print_restore_plan(&plan);
    }
    // Plan never modifies state — exit 0 even with obstacles, since
    // the operator may follow up with `restore apply --force`. Obstacles
    // are surfaced in the printed report.
    ExitCode::SUCCESS
}

fn run_restore_apply(
    backup_dir: &Path,
    target: &Path,
    verifying_key_path: Option<PathBuf>,
    require_signed: bool,
    force: bool,
    json: bool,
) -> ExitCode {
    let verifying_key = match load_verifying_key(verifying_key_path) {
        Ok(k) => k,
        Err(code) => return code,
    };
    let plan = match plan_restore(backup_dir, target, verifying_key.as_deref(), require_signed) {
        Ok(p) => p,
        Err(e) => return restore_error_exit(&e),
    };
    let report = match apply_restore(&plan, force) {
        Ok(r) => r,
        Err(e) => return restore_error_exit(&e),
    };
    if json {
        match serde_json::to_string_pretty(&report) {
            Ok(s) => println!("{s}"),
            Err(e) => {
                eprintln!("error: could not serialize restore report: {e}");
                return ExitCode::from(4);
            }
        }
    } else {
        print_restore_report(&report);
    }
    ExitCode::SUCCESS
}

fn load_verifying_key(path: Option<PathBuf>) -> Result<Option<Vec<u8>>, ExitCode> {
    let Some(path) = path else { return Ok(None) };
    match std::fs::read(&path) {
        Ok(bytes) => {
            if bytes.len() != MLDSA87_PK_LEN {
                eprintln!(
                    "error: verifying key {} has length {} != {MLDSA87_PK_LEN}",
                    path.display(),
                    bytes.len()
                );
                return Err(ExitCode::from(2));
            }
            Ok(Some(bytes))
        }
        Err(e) => {
            eprintln!(
                "error: could not read verifying key {}: {e}",
                path.display()
            );
            Err(ExitCode::from(2))
        }
    }
}

fn restore_error_exit(err: &RestoreError) -> ExitCode {
    eprintln!("restore failed: {err}");
    match err {
        RestoreError::ManifestMissing(_) | RestoreError::SourceMissing(_) => ExitCode::from(3),
        RestoreError::Io(_) | RestoreError::Serde(_) | RestoreError::Manifest(_) => {
            ExitCode::from(4)
        }
        _ => ExitCode::from(1),
    }
}

fn print_restore_plan(plan: &RestorePlan) {
    println!("restore plan: {}", plan.backup_id);
    println!("  backup     : {}", plan.backup_dir.display());
    println!("  target     : {}", plan.target_dir.display());
    println!("  signature  : {}", outcome_str(&plan.signature_outcome));
    println!("  actions    : {}", plan.actions.len());
    for a in &plan.actions {
        println!("    - {:<22} -> {}", a.name, a.target_relative.display());
    }
    if plan.warnings.is_empty() {
        println!("  warnings   : (none)");
    } else {
        println!("  warnings   :");
        for w in &plan.warnings {
            println!("    - {w}");
        }
    }
    if plan.obstacles.is_empty() {
        println!("  obstacles  : (none) — apply runs without --force");
    } else {
        println!(
            "  obstacles  : {} (apply requires --force)",
            plan.obstacles.len()
        );
        for o in &plan.obstacles {
            println!("    - {} ({})", o.path.display(), o.reason);
        }
    }
}

fn print_restore_report(report: &RestoreReport) {
    println!("restore complete: {}", report.backup_id);
    println!("  backup     : {}", report.backup_dir);
    println!("  target     : {}", report.target_dir);
    println!(
        "  signature  : {}",
        match &report.signature_outcome {
            RestoreSignatureRecord::Signed { anchor_id } => format!("signed by anchor {anchor_id}"),
            RestoreSignatureRecord::Unsigned => "unsigned".to_string(),
        }
    );
    println!("  forced     : {}", report.forced_overwrite);
    println!(
        "  audit chain: {}",
        if report.audit_chain_verified {
            "verified"
        } else {
            "NOT VERIFIED"
        }
    );
    println!("  components : {}", report.restored_components.len());
    for c in &report.restored_components {
        match &c.last_entry_hash {
            Some(h) => println!(
                "    - {:<22} {:>5} bytes  last_entry={}",
                c.name,
                c.bytes,
                truncate_hash(h)
            ),
            None => println!("    - {:<22} {:>5} bytes", c.name, c.bytes),
        }
    }
    if !report.warnings.is_empty() {
        println!("  warnings   :");
        for w in &report.warnings {
            println!("    - {w}");
        }
    }
    println!("  completed  : {}", report.completed_at);
}

fn truncate_hash(h: &str) -> String {
    if h.len() <= 16 {
        h.to_string()
    } else {
        format!("{}…", &h[..16])
    }
}

#[derive(serde::Serialize)]
struct PlanJson<'a> {
    backup_id: &'a str,
    backup_dir: String,
    target_dir: String,
    signature_outcome: &'static str,
    anchor_id: Option<&'a str>,
    action_count: usize,
    obstacle_count: usize,
    warnings: &'a [String],
    actions: Vec<PlanActionJson<'a>>,
    obstacles: Vec<PlanObstacleJson<'a>>,
}

#[derive(serde::Serialize)]
struct PlanActionJson<'a> {
    name: &'a str,
    target_relative: String,
    kind: &'static str,
    expected_sha3: &'a str,
}

#[derive(serde::Serialize)]
struct PlanObstacleJson<'a> {
    path: String,
    reason: &'a str,
}

impl<'a> From<&'a RestorePlan> for PlanJson<'a> {
    fn from(p: &'a RestorePlan) -> Self {
        let (outcome, anchor) = match &p.signature_outcome {
            VerifyOutcome::Signed { anchor_id } => ("signed", Some(anchor_id.as_str())),
            VerifyOutcome::Unsigned => ("unsigned", None),
        };
        Self {
            backup_id: &p.backup_id,
            backup_dir: p.backup_dir.display().to_string(),
            target_dir: p.target_dir.display().to_string(),
            signature_outcome: outcome,
            anchor_id: anchor,
            action_count: p.actions.len(),
            obstacle_count: p.obstacles.len(),
            warnings: &p.warnings,
            actions: p
                .actions
                .iter()
                .map(|a| PlanActionJson {
                    name: a.name.as_str(),
                    target_relative: a.target_relative.display().to_string(),
                    kind: match a.kind {
                        mai_admin::ActionKind::File => "file",
                        mai_admin::ActionKind::Tree => "tree",
                    },
                    expected_sha3: a.expected_sha3.as_str(),
                })
                .collect(),
            obstacles: p
                .obstacles
                .iter()
                .map(|o| PlanObstacleJson {
                    path: o.path.display().to_string(),
                    reason: o.reason.as_str(),
                })
                .collect(),
        }
    }
}
