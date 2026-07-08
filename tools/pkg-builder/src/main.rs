//! MAI Package Builder — CLI tool to create signed `.mai-pkg` directories.
//!
//! Usage:
//!   mai-pkg-builder --manifest manifest.toml --weights model.gguf \
//!       --signature sig.mldsa --output my-model.mai-pkg
//!
//! The tool assembles a `.mai-pkg` directory with manifest, weights,
//! signature, and hash tree for air-gap installation.

use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::Parser;

/// MAI Package Builder: create signed model packages for air-gap installation.
#[derive(Parser, Debug)]
#[command(name = "mai-pkg-builder", version, about)]
struct Cli {
    /// Path to model manifest TOML file
    #[arg(short = 'm', long)]
    manifest: PathBuf,

    /// Path to model weights file (GGUF, safetensors, etc.)
    #[arg(short = 'w', long)]
    weights: PathBuf,

    /// Path to ML-DSA signature file over the weights
    #[arg(short = 's', long)]
    signature: PathBuf,

    /// Output directory name (will be created with .mai-pkg suffix if missing)
    #[arg(short = 'o', long)]
    output: PathBuf,

    /// Overwrite existing output directory
    #[arg(long, default_value_t = false)]
    force: bool,

    /// Optional ML-DSA signature over the canonical manifest.toml bytes. When
    /// provided, the output is a manifest-authenticated (v2) package and the
    /// manifest must declare the correct weights hash-tree root (finding DF-01A).
    #[arg(long)]
    manifest_signature: Option<PathBuf>,
}

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    let cli = Cli::parse();

    // Ensure output has .mai-pkg suffix
    let output_dir = if cli
        .output
        .extension()
        .map(|e| e == "mai-pkg")
        .unwrap_or(false)
    {
        cli.output.clone()
    } else {
        let mut with_suffix = cli.output.clone();
        if let Some(name) = with_suffix.file_name() {
            let name = name.to_string_lossy().to_string();
            with_suffix.set_file_name(format!("{name}.mai-pkg"));
        }
        with_suffix
    };

    // Check output doesn't exist
    if output_dir.exists() {
        if cli.force {
            fs::remove_dir_all(&output_dir)
                .with_context(|| format!("Removing existing output: {}", output_dir.display()))?;
            tracing::warn!("Removed existing output directory");
        } else {
            anyhow::bail!(
                "Output directory '{}' already exists. Use --force to overwrite.",
                output_dir.display()
            );
        }
    }

    // Read manifest
    tracing::info!("Reading manifest: {}", cli.manifest.display());
    let manifest_content = fs::read_to_string(&cli.manifest)
        .with_context(|| format!("Failed to read manifest: {}", cli.manifest.display()))?;

    // Validate manifest parses
    let _manifest: mai_core::registry::ModelManifest =
        toml::from_str(&manifest_content).with_context(|| "Invalid manifest TOML")?;

    // Read weights
    tracing::info!("Reading weights: {}", cli.weights.display());
    let weights_data = fs::read(&cli.weights)
        .with_context(|| format!("Failed to read weights: {}", cli.weights.display()))?;

    // Read signature
    tracing::info!("Reading signature: {}", cli.signature.display());
    let signature_data = fs::read(&cli.signature)
        .with_context(|| format!("Failed to read signature: {}", cli.signature.display()))?;

    // Compute hash tree root
    tracing::info!("Computing SHA-256 Merkle hash tree root");
    let hash_root = mai_core::models::verify::compute_hash_tree_root(&weights_data);

    // Create output directory
    fs::create_dir_all(&output_dir).with_context(|| {
        format!(
            "Failed to create output directory: {}",
            output_dir.display()
        )
    })?;

    // Write package files
    let files: [(&str, &[u8]); 4] = [
        ("manifest.toml", manifest_content.as_bytes()),
        ("weights.bin", &weights_data),
        ("signature.mldsa", &signature_data),
        ("hash_tree.sha256", hash_root.as_bytes()),
    ];

    for (file_name, data) in &files {
        let dest = output_dir.join(file_name);
        fs::write(&dest, data)
            .with_context(|| format!("Failed to write {file_name} to {}", dest.display()))?;
        tracing::info!("  Wrote {} ({} bytes)", dest.display(), data.len());
    }

    // DF-01A: a manifest-authenticated (v2) package carries a signature over the
    // manifest and must bind the manifest to the weights by declaring the
    // computed hash-tree root, so the consumer can prove the pair belongs
    // together.
    if let Some(msig_path) = &cli.manifest_signature {
        if _manifest.security.integrity_hash_tree.trim() != hash_root {
            anyhow::bail!(
                "manifest security.integrity_hash_tree ({}) does not equal the computed weights \
                 hash-tree root ({hash_root}); a manifest-signed package must bind them",
                _manifest.security.integrity_hash_tree
            );
        }
        let msig = fs::read(msig_path).with_context(|| {
            format!("Failed to read manifest signature: {}", msig_path.display())
        })?;
        let dest = output_dir.join("manifest.mldsa");
        fs::write(&dest, &msig)
            .with_context(|| format!("Failed to write manifest.mldsa to {}", dest.display()))?;
        tracing::info!(
            "  Wrote {} ({} bytes) [manifest-authenticated v2]",
            dest.display(),
            msig.len()
        );
    }

    tracing::info!("Package created successfully at {}", output_dir.display());
    tracing::info!(
        "  Model ID: {}:{}:{}",
        _manifest.model.name,
        _manifest.model.version,
        _manifest.model.quantization.as_deref().unwrap_or("native")
    );

    Ok(())
}
