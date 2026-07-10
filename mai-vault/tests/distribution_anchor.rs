//! Supply-chain gate for model packages: verification binds to the pinned
//! distribution trust anchor, never the appliance's own boot key.
//!
//! Drives mai-core's `verify_package` through a real `ZfsVault` + `PqcEngine`
//! with real ML-DSA-87 keys: an anchor-signed package verifies and installs
//! clean; a package signed by the appliance self-key or by a foreign key is
//! refused; a manifest declaring the wrong signing-key fingerprint is refused
//! even when its signatures verify; and the fail-closed posture (anchor
//! required but absent) refuses every package.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use mai_core::models::ModelPackage;
use mai_core::models::verify::{compute_hash_tree_root, verify_package};
use mai_core::vault::{PqcProvider, VaultInterface};
use mai_vault::ZfsVault;
use mai_vault::audit::AuditWriter;
use mai_vault::config::VaultConfig;
use mai_vault::pqc::PqcEngine;
use tempfile::TempDir;

/// An ML-DSA-87 signing identity for the test: the "factory" distribution
/// key, a foreign key, or the appliance's own key.
struct SigningKey {
    public: Vec<u8>,
    secret: Vec<u8>,
}

async fn keypair(engine: &PqcEngine) -> SigningKey {
    let (public, secret) = engine.dsa_generate_keypair().await.expect("keypair");
    SigningKey { public, secret }
}

fn vault_config(tmp: &TempDir) -> VaultConfig {
    let mut cfg = VaultConfig::default();
    cfg.storage.mount_point = tmp.path().join("models");
    cfg.storage.staging_dir = tmp.path().join("staging");
    cfg.pqc.key_store_path = tmp.path().join("keys");
    cfg.profiles.db_path = tmp.path().join("profiles.db");
    cfg.audit.db_path = tmp.path().join("audit.json");
    cfg
}

/// A vault whose engine optionally pins a distribution anchor and optionally
/// requires one. Returns the engine too, so tests can sign with the
/// appliance's own key.
async fn vault_with(
    tmp: &TempDir,
    anchor: Option<&[u8]>,
    required: bool,
) -> (ZfsVault, Arc<PqcEngine>) {
    let cfg = vault_config(tmp);
    let pqc = Arc::new(PqcEngine::new(cfg.pqc.clone()));
    pqc.initialize().await.expect("engine init");
    if let Some(pk) = anchor {
        pqc.set_distribution_anchor(pk.to_vec())
            .await
            .expect("anchor pins");
    }
    if required {
        pqc.require_distribution_anchor();
    }
    let audit = Arc::new(AuditWriter::with_pqc(cfg.audit.clone(), pqc.clone()));
    let vault = ZfsVault::with_engines(cfg, pqc.clone(), audit);
    (vault, pqc)
}

fn manifest_toml(fingerprint: &str, integrity_root: &str) -> String {
    format!(
        r#"[model]
name = "anchored-model"
version = "1.0.0"
format = "GGUF"
quantization = "Q4_K_M"
size_bytes = 100
required_vram_bytes = 200

[compatibility]
min_mai_version = "0.1.0"
supported_backends = ["ollama"]
hardware_classes = ["cpu"]

[capabilities]
chat = true
completion = true
embedding = false
vision = false
structured_output = false
max_context_tokens = 4096
supported_languages = ["en"]

[security]
signature_algorithm = "ML-DSA-87"
public_key_fingerprint = "{fingerprint}"
integrity_hash_tree = "{integrity_root}"

[metadata]
license = "MIT"
changelog = "Initial"
"#
    )
}

/// Write a complete v2 (manifest-authenticated) package: weights + hash tree,
/// weights and manifest both signed by `signer` (via the engine's raw ML-DSA
/// ops), manifest declaring `fingerprint`.
async fn write_package(
    dir: &Path,
    engine: &PqcEngine,
    signer: &SigningKey,
    fingerprint: &str,
) -> ModelPackage {
    let pkg_dir = dir.join("anchored-model.mai-pkg");
    std::fs::create_dir_all(&pkg_dir).expect("pkg dir");

    let weights = vec![7u8; 256];
    let root = compute_hash_tree_root(&weights);
    let manifest = manifest_toml(fingerprint, &root);

    let weights_sig = engine
        .dsa_sign(&weights, &signer.secret)
        .await
        .expect("sign weights");
    let manifest_sig = engine
        .dsa_sign(manifest.as_bytes(), &signer.secret)
        .await
        .expect("sign manifest");

    std::fs::write(pkg_dir.join("manifest.toml"), &manifest).expect("manifest");
    std::fs::write(pkg_dir.join("weights.bin"), &weights).expect("weights");
    std::fs::write(pkg_dir.join("signature.mldsa"), &weights_sig).expect("weights sig");
    std::fs::write(pkg_dir.join("hash_tree.sha256"), format!("{root}\n")).expect("hash tree");
    std::fs::write(pkg_dir.join("manifest.mldsa"), &manifest_sig).expect("manifest sig");

    ModelPackage::open(&pkg_dir).expect("package opens")
}

fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("mai-dist-anchor-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("scratch dir");
    dir
}

#[tokio::test]
async fn anchor_signed_package_verifies_and_fingerprint_matches() {
    let tmp = TempDir::new().unwrap();
    let (vault, pqc) = vault_with(&tmp, None, false).await;
    let factory = keypair(&pqc).await;
    pqc.set_distribution_anchor(factory.public.clone())
        .await
        .expect("anchor pins");

    let anchor_fp = vault
        .distribution_fingerprint()
        .await
        .expect("anchor fingerprint exposed");
    assert!(
        anchor_fp.starts_with("sha256:") && anchor_fp.len() == 7 + 64,
        "fingerprint is sha256:<64 hex>, got {anchor_fp}"
    );

    let dir = scratch("ok");
    let pkg = write_package(&dir, &pqc, &factory, &anchor_fp).await;
    let result = verify_package(&pkg, &vault, "0.1.0").await;

    assert!(result.signature_valid, "anchor-signed weights verify");
    assert!(
        result.manifest_authenticated,
        "anchor-signed manifest verifies"
    );
    assert!(
        result.verified,
        "anchor-signed package installs: {:?}",
        result.messages
    );
    assert!(
        result
            .messages
            .iter()
            .any(|m| m.contains("matches the distribution anchor")),
        "fingerprint consult recorded: {:?}",
        result.messages
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn self_key_signed_package_is_refused() {
    // The appliance's own per-boot key must no longer authenticate a package
    // once a distribution anchor is pinned.
    let tmp = TempDir::new().unwrap();
    let (vault, pqc) = vault_with(&tmp, None, false).await;
    let factory = keypair(&pqc).await;
    pqc.set_distribution_anchor(factory.public.clone())
        .await
        .expect("anchor pins");
    let anchor_fp = vault.distribution_fingerprint().await.unwrap();

    // Sign with the ENGINE's own master key (what the old path trusted).
    let dir = scratch("selfkey");
    let pkg_dir = dir.join("anchored-model.mai-pkg");
    std::fs::create_dir_all(&pkg_dir).unwrap();
    let weights = vec![7u8; 256];
    let root = compute_hash_tree_root(&weights);
    let manifest = manifest_toml(&anchor_fp, &root);
    let weights_sig = pqc.sign_package(&weights).await.unwrap();
    let manifest_sig = pqc.sign_package(manifest.as_bytes()).await.unwrap();
    std::fs::write(pkg_dir.join("manifest.toml"), &manifest).unwrap();
    std::fs::write(pkg_dir.join("weights.bin"), &weights).unwrap();
    std::fs::write(pkg_dir.join("signature.mldsa"), &weights_sig).unwrap();
    std::fs::write(pkg_dir.join("hash_tree.sha256"), format!("{root}\n")).unwrap();
    std::fs::write(pkg_dir.join("manifest.mldsa"), &manifest_sig).unwrap();
    let pkg = ModelPackage::open(&pkg_dir).unwrap();

    let result = verify_package(&pkg, &vault, "0.1.0").await;
    assert!(
        !result.signature_valid,
        "self-key weights signature refused"
    );
    assert!(!result.verified, "self-key-signed package must not install");
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn foreign_key_signed_package_is_refused() {
    let tmp = TempDir::new().unwrap();
    let (vault, pqc) = vault_with(&tmp, None, false).await;
    let factory = keypair(&pqc).await;
    let foreign = keypair(&pqc).await;
    pqc.set_distribution_anchor(factory.public.clone())
        .await
        .expect("anchor pins");
    let anchor_fp = vault.distribution_fingerprint().await.unwrap();

    let dir = scratch("foreign");
    let pkg = write_package(&dir, &pqc, &foreign, &anchor_fp).await;
    let result = verify_package(&pkg, &vault, "0.1.0").await;

    assert!(!result.signature_valid, "foreign-key signature refused");
    assert!(
        !result.verified,
        "foreign-key-signed package must not install"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn fingerprint_mismatch_is_refused_even_with_valid_signatures() {
    let tmp = TempDir::new().unwrap();
    let (vault, pqc) = vault_with(&tmp, None, false).await;
    let factory = keypair(&pqc).await;
    pqc.set_distribution_anchor(factory.public.clone())
        .await
        .expect("anchor pins");

    // Anchor-signed package whose manifest names a DIFFERENT key.
    let dir = scratch("fp");
    let wrong_fp = format!("sha256:{}", "ab".repeat(32));
    let pkg = write_package(&dir, &pqc, &factory, &wrong_fp).await;
    let result = verify_package(&pkg, &vault, "0.1.0").await;

    assert!(
        result.signature_valid && result.manifest_authenticated,
        "signatures themselves verify — the fingerprint is what fails"
    );
    assert!(
        !result.verified,
        "fingerprint mismatch must refuse the package"
    );
    assert!(
        result
            .messages
            .iter()
            .any(|m| m.contains("does not match the pinned distribution anchor")),
        "mismatch named in messages: {:?}",
        result.messages
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn required_anchor_missing_fails_closed() {
    // The production posture with no anchor installed: every package is
    // refused — never a silent fallback to the self-key.
    let tmp = TempDir::new().unwrap();
    let (vault, pqc) = vault_with(&tmp, None, true).await;
    let signer = keypair(&pqc).await;

    let dir = scratch("required");
    let pkg = write_package(&dir, &pqc, &signer, "sha256:whatever").await;
    let result = verify_package(&pkg, &vault, "0.1.0").await;

    assert!(
        !result.signature_valid,
        "no anchor → signature cannot verify"
    );
    assert!(
        !result.verified,
        "no anchor → package refused (fail closed)"
    );
    assert!(
        result
            .messages
            .iter()
            .any(|m| m.contains("no model-distribution trust anchor pinned")),
        "refusal names the missing anchor: {:?}",
        result.messages
    );
    let _ = std::fs::remove_dir_all(&dir);
}
