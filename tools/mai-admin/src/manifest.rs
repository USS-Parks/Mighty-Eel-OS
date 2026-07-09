//! Backup manifest schema.
//!
//! A backup is a directory tree: `manifest.json` at the root, plus one
//! file (or sub-tree) per component listed inside the manifest. The
//! manifest is the audit-grade record of what was backed up, with one
//! SHA3-256 digest per component file and an optional ML-DSA-87
//! signature over the canonical manifest body. Verification reloads
//! the manifest, recomputes every digest, and (when an anchor is
//! configured) checks the signature against a public key file.
//!
//! Out of scope:
//! - Restore. `restore plan/apply` and the recovery boot are separate.
//! - Tarball / cpio packaging. The tool emits a directory tree; the
//!   operator wraps it in whatever transport their site policy requires.
//! - Per-component cipher choice. The tool backs up data that is already
//!   sealed at rest (vault encryption, audit AeadSealer) or that is
//!   not sensitive on its own (trust anchor public keys, manifest JSON).
//!   Raw API keys never enter the backup.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha3::{Digest, Sha3_256};
use thiserror::Error;

/// ML-DSA-87 key sizes. Mirror the constants in
/// `mai-compliance/src/bundle.rs` so we don't take a workspace-wide
/// dependency on a private symbol.
pub const MLDSA87_PK_LEN: usize = 2592;
pub const MLDSA87_SK_LEN: usize = 4896;
pub const MLDSA87_SIG_LEN: usize = 4627;

/// Top-level manifest. Serialised to `manifest.json` at the root of a
/// backup directory.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BackupManifest {
    /// Stable identifier: `mai-backup-{rfc3339-no-colons}`. Operators
    /// use this to refer to a backup in support tickets.
    pub backup_id: String,
    /// RFC 3339 timestamp the backup started.
    pub created_at: String,
    /// `CARGO_PKG_VERSION` of the mai-admin binary that produced the
    /// backup. Restore refuses to apply when the live
    /// `mai-api` is older than this.
    pub mai_version: String,
    /// Git commit (short) of the producing build. Best-effort: empty
    /// when not running inside a git checkout.
    pub git_commit: String,
    /// Profile name as recorded in `[profile].name` of the source
    /// ship profile (`ship` for production, `local-dev` otherwise).
    pub profile: String,
    /// Hostname the backup was produced on. Audit only.
    pub host: String,
    /// Migration version of the producing build. Restore will compare
    /// this against the target node's migration version to plan
    /// upgrade / refuse downgrade.
    pub migration_version: String,
    /// One entry per file (or directory tree) included in the backup.
    /// Order is deterministic by `name`.
    pub components: Vec<ManifestComponent>,
    /// Signature material. Absent until `BackupManifest::sign` runs.
    #[serde(default)]
    pub signatures: ManifestSignatures,
}

/// One file or sub-tree of the backup.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ManifestComponent {
    /// Stable identifier: `api_audit_wal`, `compliance_audit_wal`,
    /// `trust_bundle_cache`, `trust_anchors`, `config_checksums`,
    /// `auth_key_hashes`, `model_registry`, `reports`,
    /// `vault_snapshot_ref`. Restore keys component handlers off this.
    pub name: String,
    /// Path inside the backup directory (relative to the backup root).
    pub path: String,
    /// SHA3-256 of the on-disk component, hex-encoded lowercase.
    pub sha3_256: String,
    /// Byte size of the component file or — for tree components — the
    /// sum of every file in the tree.
    pub bytes: u64,
    /// For WAL components: total entries replayed at backup time. The backup
    /// uses this as a fast sanity-check before recomputing the SHA3.
    /// `None` for non-WAL components.
    pub entry_count: Option<u64>,
    /// For WAL components: the chain `entry_hash` of the last entry,
    /// or the genesis hash when the WAL is empty. `None` for non-WAL.
    pub last_entry_hash: Option<String>,
    /// For tree components: number of files. `None` for single-file
    /// components.
    pub file_count: Option<u64>,
}

/// Signature material for the manifest. Optional in local-dev mode and
/// when no signing key is supplied; required in ship profile.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ManifestSignatures {
    /// ML-DSA-87 signature, hex-encoded. `None` when the manifest is
    /// unsigned (verify will report this and exit non-zero in
    /// `--require-signed` mode).
    #[serde(default)]
    pub manifest_mldsa: Option<String>,
    /// Stable identifier of the public key that verifies
    /// `manifest_mldsa`. Operators put the matching `<anchor_id>.pub`
    /// into the verifier's anchor directory.
    #[serde(default)]
    pub anchor_id: Option<String>,
    /// SHA3-256 of the canonical (unsigned) manifest body. Cheap
    /// integrity check that does not require the public key.
    #[serde(default)]
    pub body_sha3_256: Option<String>,
}

/// What can go wrong while sealing or opening a manifest.
#[derive(Debug, Error)]
pub enum ManifestError {
    #[error("manifest io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("manifest serialization failed: {0}")]
    Serde(#[from] serde_json::Error),
    #[error("signing key length {actual} != expected {MLDSA87_SK_LEN}")]
    SigningKeyLength { actual: usize },
    #[error("verifying key length {actual} != expected {MLDSA87_PK_LEN}")]
    VerifyingKeyLength { actual: usize },
    #[error("signature length {actual} != expected {MLDSA87_SIG_LEN}")]
    SignatureLength { actual: usize },
    #[error("manifest is unsigned but --require-signed was set")]
    Unsigned,
    #[error("signature does not verify against anchor {0}")]
    BadSignature(String),
    #[error(
        "manifest body sha3 mismatch: stored {stored} computed {computed}; \
         the manifest has been tampered with"
    )]
    BodyDigestMismatch { stored: String, computed: String },
    #[error("manifest references anchor {0:?} but no public key supplied")]
    MissingAnchor(String),
}

impl BackupManifest {
    fn anchor_id_for_error(&self) -> String {
        self.signatures
            .anchor_id
            .clone()
            .unwrap_or_else(|| "unknown-anchor".to_string())
    }

    /// Canonical bytes the signature covers. Equivalent to the
    /// serialized manifest with `signatures` cleared, then encoded as
    /// pretty JSON with sorted keys. Stability matters here — any
    /// future serializer change must keep this byte-for-byte stable
    /// or the entire fleet of pre-existing signed backups becomes
    /// unverifiable.
    pub fn canonical_body(&self) -> Result<Vec<u8>, ManifestError> {
        let mut copy = self.clone();
        copy.signatures = ManifestSignatures::default();
        // Sort components by name so two producers that built the
        // same logical set of components emit identical bytes.
        copy.components.sort_by(|a, b| a.name.cmp(&b.name));
        let value = serde_json::to_value(&copy)?;
        let sorted = canonical_json(&value);
        Ok(sorted.into_bytes())
    }

    /// SHA3-256 of `canonical_body`. Stored in
    /// `signatures.body_sha3_256` for quick tamper detection without
    /// public key material.
    pub fn body_sha3_hex(&self) -> Result<String, ManifestError> {
        let body = self.canonical_body()?;
        Ok(sha3_hex(&body))
    }

    /// Sign the canonical body with an ML-DSA-87 secret key.
    /// `secret_key` is the 4896-byte raw key. `anchor_id` is the
    /// stable string operators look up in their anchor directory.
    pub fn sign(
        &mut self,
        secret_key: &[u8],
        anchor_id: impl Into<String>,
    ) -> Result<(), ManifestError> {
        if secret_key.len() != MLDSA87_SK_LEN {
            return Err(ManifestError::SigningKeyLength {
                actual: secret_key.len(),
            });
        }
        let body = self.canonical_body()?;
        let signature_bytes = sign_with_mldsa87(secret_key, &body)?;
        self.signatures = ManifestSignatures {
            manifest_mldsa: Some(hex::encode(&signature_bytes)),
            anchor_id: Some(anchor_id.into()),
            body_sha3_256: Some(sha3_hex(&body)),
        };
        Ok(())
    }

    /// Verify the signature against a supplied public key.
    ///
    /// Returns `Ok(VerifyOutcome::Signed { anchor_id })` when the
    /// signature checks out, `Ok(VerifyOutcome::Unsigned)` when no
    /// signature is present, and `Err(_)` for any other failure mode
    /// (anchor mismatch, wrong key, length error, body sha mismatch).
    pub fn verify(&self, public_key: &[u8]) -> Result<VerifyOutcome, ManifestError> {
        let Some(sig_hex) = self.signatures.manifest_mldsa.as_deref() else {
            return Ok(VerifyOutcome::Unsigned);
        };
        let signature_bytes = hex::decode(sig_hex)
            .map_err(|_| ManifestError::BadSignature(self.anchor_id_for_error()))?;
        if signature_bytes.len() != MLDSA87_SIG_LEN {
            return Err(ManifestError::SignatureLength {
                actual: signature_bytes.len(),
            });
        }
        if public_key.len() != MLDSA87_PK_LEN {
            return Err(ManifestError::VerifyingKeyLength {
                actual: public_key.len(),
            });
        }
        let body = self.canonical_body()?;
        if let Some(stored) = self.signatures.body_sha3_256.as_deref() {
            let computed = sha3_hex(&body);
            if stored != computed {
                return Err(ManifestError::BodyDigestMismatch {
                    stored: stored.to_string(),
                    computed,
                });
            }
        }
        verify_with_mldsa87(public_key, &body, &signature_bytes)
            .map_err(|()| ManifestError::BadSignature(self.anchor_id_for_error()))?;
        Ok(VerifyOutcome::Signed {
            anchor_id: self.anchor_id_for_error(),
        })
    }

    pub fn write_to(&self, path: &Path) -> Result<(), ManifestError> {
        let pretty = serde_json::to_vec_pretty(self)?;
        std::fs::write(path, pretty)?;
        Ok(())
    }

    pub fn load_from(path: &Path) -> Result<Self, ManifestError> {
        let bytes = std::fs::read(path)?;
        let manifest: BackupManifest = serde_json::from_slice(&bytes)?;
        Ok(manifest)
    }

    /// Insertion-order-preserving lookup. There are never more than
    /// ~12 components per backup so a linear scan is fine.
    pub fn component(&self, name: &str) -> Option<&ManifestComponent> {
        self.components.iter().find(|c| c.name == name)
    }
}

/// What a manifest verification produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VerifyOutcome {
    /// Manifest was signed and the signature verified.
    Signed { anchor_id: String },
    /// Manifest had no signature. The caller decides whether to treat
    /// this as a failure (it should, in ship mode).
    Unsigned,
}

/// SHA3-256 hex-encoded, lowercase. Stable formatting for the manifest.
pub fn sha3_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha3_256::new();
    hasher.update(bytes);
    hex::encode(hasher.finalize())
}

/// 64 KiB streaming buffer. Lives on the heap because clippy flags any
/// stack array over 16 KiB.
const STREAM_BUF: usize = 64 * 1024;

/// SHA3-256 a file by streaming, never loading the whole thing into RAM.
pub fn sha3_file(path: &Path) -> Result<String, ManifestError> {
    use std::io::Read;
    let mut hasher = Sha3_256::new();
    let mut buf = vec![0u8; STREAM_BUF];
    let mut file = std::fs::File::open(path)?;
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hex::encode(hasher.finalize()))
}

/// SHA3-256 a directory tree by hashing each file's path + contents.
/// Returns `(digest, file_count, total_bytes)`.
pub fn sha3_tree(root: &Path) -> Result<(String, u64, u64), ManifestError> {
    use std::io::Read;
    let mut entries = Vec::new();
    walk_tree(root, root, &mut entries)?;
    entries.sort_by(|a, b| a.0.cmp(&b.0));

    let mut hasher = Sha3_256::new();
    let mut total_files: u64 = 0;
    let mut total_bytes: u64 = 0;
    let mut buf = vec![0u8; STREAM_BUF];
    for (relative, abs) in entries {
        hasher.update(relative.as_bytes());
        hasher.update([0u8]);
        let mut file = std::fs::File::open(&abs)?;
        loop {
            let n = file.read(&mut buf)?;
            if n == 0 {
                break;
            }
            hasher.update(&buf[..n]);
            total_bytes += n as u64;
        }
        hasher.update([0u8]);
        total_files += 1;
    }
    Ok((hex::encode(hasher.finalize()), total_files, total_bytes))
}

fn walk_tree(root: &Path, dir: &Path, out: &mut Vec<(String, PathBuf)>) -> std::io::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            walk_tree(root, &path, out)?;
        } else {
            let rel = path.strip_prefix(root).map_err(std::io::Error::other)?;
            out.push((rel.to_string_lossy().replace('\\', "/"), path));
        }
    }
    Ok(())
}

fn sign_with_mldsa87(secret_key: &[u8], payload: &[u8]) -> Result<Vec<u8>, ManifestError> {
    use ml_dsa::signature::Signer;
    use ml_dsa::{EncodedSigningKey, MlDsa87, Signature, SigningKey};
    let sk_arr: &[u8; MLDSA87_SK_LEN] =
        secret_key
            .try_into()
            .map_err(|_| ManifestError::SigningKeyLength {
                actual: secret_key.len(),
            })?;
    let sk_encoded = EncodedSigningKey::<MlDsa87>::from(*sk_arr);
    let sk = SigningKey::<MlDsa87>::decode(&sk_encoded);
    let sig: Signature<MlDsa87> = sk.sign(payload);
    Ok(sig.encode().to_vec())
}

fn verify_with_mldsa87(public_key: &[u8], payload: &[u8], signature: &[u8]) -> Result<(), ()> {
    use ml_dsa::signature::Verifier;
    use ml_dsa::{EncodedSignature, EncodedVerifyingKey, MlDsa87, Signature, VerifyingKey};
    let pk_arr: &[u8; MLDSA87_PK_LEN] = public_key.try_into().map_err(|_| ())?;
    let sig_arr: &[u8; MLDSA87_SIG_LEN] = signature.try_into().map_err(|_| ())?;
    let pk_encoded = EncodedVerifyingKey::<MlDsa87>::from(*pk_arr);
    let pk = VerifyingKey::<MlDsa87>::decode(&pk_encoded);
    let sig_encoded = EncodedSignature::<MlDsa87>::from(*sig_arr);
    let sig = Signature::<MlDsa87>::decode(&sig_encoded).ok_or(())?;
    pk.verify(payload, &sig).map_err(|_| ())
}

/// Stable JSON serialiser. Sorts object keys lexically at every level.
/// Used for `canonical_body` so signatures stay valid across map
/// iteration order changes in future serde_json releases.
fn canonical_json(value: &serde_json::Value) -> String {
    let mut out = String::new();
    write_canonical(value, &mut out);
    out
}

fn write_canonical(value: &serde_json::Value, out: &mut String) {
    use serde_json::Value;
    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Number(n) => out.push_str(&n.to_string()),
        Value::String(s) => {
            out.push_str(&serde_json::to_string(s).expect("string encodes"));
        }
        Value::Array(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_canonical(item, out);
            }
            out.push(']');
        }
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            out.push('{');
            for (i, k) in keys.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                out.push_str(&serde_json::to_string(k).expect("key encodes"));
                out.push(':');
                write_canonical(&map[*k], out);
            }
            out.push('}');
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fresh_keypair() -> (Vec<u8>, Vec<u8>) {
        use ml_dsa::{B32, KeyGen, MlDsa87};
        use rand::RngCore;
        let mut seed_bytes = [0u8; 32];
        rand::thread_rng().fill_bytes(&mut seed_bytes);
        let seed = B32::from(seed_bytes);
        let kp = MlDsa87::key_gen_internal(&seed);
        let pk = kp.verifying_key().encode().to_vec();
        let sk = kp.signing_key().encode().to_vec();
        (pk, sk)
    }

    fn sample_manifest() -> BackupManifest {
        BackupManifest {
            backup_id: "mai-backup-2026-05-23T12-00-00Z".into(),
            created_at: "2026-05-23T12:00:00Z".into(),
            mai_version: "0.1.0".into(),
            git_commit: "abc1234".into(),
            profile: "ship".into(),
            host: "test-host".into(),
            migration_version: "2026-05-22.001".into(),
            components: vec![
                ManifestComponent {
                    name: "api_audit_wal".into(),
                    path: "audit/api/current.jsonl".into(),
                    sha3_256: "00".repeat(32),
                    bytes: 0,
                    entry_count: Some(0),
                    last_entry_hash: Some("genesis".into()),
                    file_count: None,
                },
                ManifestComponent {
                    name: "auth_key_hashes".into(),
                    path: "auth/key-hashes.json".into(),
                    sha3_256: "11".repeat(32),
                    bytes: 42,
                    entry_count: None,
                    last_entry_hash: None,
                    file_count: None,
                },
            ],
            signatures: ManifestSignatures::default(),
        }
    }

    #[test]
    fn canonical_body_is_deterministic_across_component_order() {
        let mut a = sample_manifest();
        let mut b = sample_manifest();
        b.components.reverse();
        assert_eq!(a.canonical_body().unwrap(), b.canonical_body().unwrap());
        // Sanity: the unsorted manifest itself differs.
        assert_ne!(
            serde_json::to_vec(&a).unwrap(),
            serde_json::to_vec(&b).unwrap()
        );
        // Re-sort `a` so the assertion that order doesn't matter is honest.
        a.components.sort_by(|x, y| x.name.cmp(&y.name));
    }

    #[test]
    fn canonical_body_excludes_signatures() {
        let mut m = sample_manifest();
        let before = m.canonical_body().unwrap();
        m.signatures = ManifestSignatures {
            manifest_mldsa: Some("ff".repeat(MLDSA87_SIG_LEN)),
            anchor_id: Some("test".into()),
            body_sha3_256: Some("00".repeat(32)),
        };
        let after = m.canonical_body().unwrap();
        assert_eq!(
            before, after,
            "signatures must be stripped from canonical body"
        );
    }

    #[test]
    fn sign_then_verify_round_trips() {
        let (pk, sk) = fresh_keypair();
        let mut m = sample_manifest();
        m.sign(&sk, "anchor-test").unwrap();
        assert!(m.signatures.manifest_mldsa.is_some());
        assert_eq!(m.signatures.anchor_id.as_deref(), Some("anchor-test"));
        assert!(m.signatures.body_sha3_256.is_some());

        let outcome = m.verify(&pk).unwrap();
        assert_eq!(
            outcome,
            VerifyOutcome::Signed {
                anchor_id: "anchor-test".into()
            }
        );
    }

    #[test]
    fn verify_rejects_tampered_body() {
        let (pk, sk) = fresh_keypair();
        let mut m = sample_manifest();
        m.sign(&sk, "a").unwrap();
        // Tamper: change a component digest.
        m.components[0].sha3_256 = "22".repeat(32);
        // The stored body_sha3 catches this first.
        let err = m.verify(&pk).unwrap_err();
        assert!(
            matches!(err, ManifestError::BodyDigestMismatch { .. }),
            "expected body digest mismatch, got {err:?}"
        );
    }

    #[test]
    fn verify_rejects_wrong_public_key() {
        let (_pk, sk) = fresh_keypair();
        let (other_pk, _) = fresh_keypair();
        let mut m = sample_manifest();
        m.sign(&sk, "a").unwrap();
        let err = m.verify(&other_pk).unwrap_err();
        assert!(matches!(err, ManifestError::BadSignature(_)));
    }

    #[test]
    fn unsigned_manifest_returns_unsigned_outcome() {
        let (pk, _) = fresh_keypair();
        let m = sample_manifest();
        let outcome = m.verify(&pk).unwrap();
        assert_eq!(outcome, VerifyOutcome::Unsigned);
    }

    #[test]
    fn sign_with_wrong_length_key_errors() {
        let mut m = sample_manifest();
        let bad_sk = vec![0u8; 100];
        let err = m.sign(&bad_sk, "a").unwrap_err();
        assert!(matches!(
            err,
            ManifestError::SigningKeyLength { actual: 100 }
        ));
    }

    #[test]
    fn write_then_load_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("manifest.json");
        let m = sample_manifest();
        m.write_to(&path).unwrap();
        let loaded = BackupManifest::load_from(&path).unwrap();
        assert_eq!(m, loaded);
    }

    #[test]
    fn sha3_file_matches_sha3_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("data");
        let payload = b"hello mai-admin".to_vec();
        std::fs::write(&path, &payload).unwrap();
        assert_eq!(sha3_file(&path).unwrap(), sha3_hex(&payload));
    }

    #[test]
    fn sha3_tree_is_stable_across_walks() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("nested")).unwrap();
        std::fs::write(dir.path().join("a.txt"), b"alpha").unwrap();
        std::fs::write(dir.path().join("nested/b.txt"), b"bravo").unwrap();
        let (digest_1, files_1, bytes_1) = sha3_tree(dir.path()).unwrap();
        let (digest_2, files_2, bytes_2) = sha3_tree(dir.path()).unwrap();
        assert_eq!(digest_1, digest_2);
        assert_eq!(files_1, 2);
        assert_eq!(files_2, 2);
        assert_eq!(bytes_1, 10);
        assert_eq!(bytes_2, 10);
    }

    #[test]
    fn component_lookup_finds_by_name() {
        let m = sample_manifest();
        assert!(m.component("api_audit_wal").is_some());
        assert!(m.component("nonexistent").is_none());
    }
}
