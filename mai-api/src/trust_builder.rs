//! Trust builder.
//!
//! Construct the trust-side bootstrap components (`BundleVerifier` +
//! `TrustExchangeMode`) for the API server from a parsed `ShipProfile`.
//! This is the trust analog of `vault_builder` and `sealer_builder`.
//!
//! Scope:
//!
//! - [`TrustExchangeMode`] enum (LocalDevSynthetic / OpenBaoBridge /
//!   Disabled) — selects how `POST /v1/auth/exchange_token` mints
//!   tokens. Production must never use `LocalDevSynthetic`.
//! - [`build_trust_components`] — production rejects every demo
//!   shortcut: `AcceptAllBundleVerifier`, `allow_accept_all_verifier`,
//!   `allow_local_dev_exchange`, missing `require_trust_anchor`,
//!   missing `require_bundle_on_boot`, an empty / missing /
//!   unpopulated anchors directory, or a malformed anchor file.
//!   Local-dev tolerates each of those (and AcceptAll falls back to
//!   the test verifier).
//! - [`verify_boot_bundle`] — load the persisted policy bundle from
//!   `<trust.bundle_cache_dir>/bundle.json` and verify it against the
//!   loaded anchors. Production startup must call this and refuse to
//!   bind on any error; the wiring lives in `MaiServer::run()` at the
//!   convergence step.
//! - Conventional on-disk anchor file: `<anchors_dir>/<key_id>.pub`,
//!   exactly 2592 bytes of raw ML-DSA-87 public key.
//!
//! Out of scope:
//!
//! - Wiring into [`crate::server::MaiServer`]. Server bootstrap still
//!   constructs [`AppState`] with [`AcceptAllBundleVerifier`]; the
//!   convergence step swaps that to
//!   `build_trust_components(&profile)?.bundle_verifier` and flips
//!   `production_guard::PROD-TRUST-100` from `Deferred` to live.
//! - The live OpenBao bridge HTTP client. This builder ships the
//!   [`TrustExchangeMode`] flag and rejects `LocalDevSynthetic` in
//!   production; the bridge client lands in a follow-up session.
//! - Loading a bundle *into* the live [`LocalTrustCache`]. This builder
//!   verifies the bundle on disk; the cache hydration call from the
//!   verified bundle lands at convergence so it cannot collide
//!   on `state.rs`.
//!
//! [`AppState`]: crate::state::AppState
//! [`LocalTrustCache`]: mai_compliance::trust_cache::LocalTrustCache

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use mai_compliance::bundle::{
    AcceptAllBundleVerifier, BundleError, BundleVerifier, MlDsaBundleVerifier, SignedPolicyBundle,
};

use crate::ship_profile::{ProfileMode, ShipProfile, TrustVerifier};

/// ML-DSA-87 public-key length (FIPS 204). Matches the constant in
/// `mai_compliance::bundle`; duplicated here so the trust builder can
/// validate anchor file lengths without exposing that crate's
/// internals.
const MLDSA87_PK_LEN: usize = 2592;

/// Conventional anchor file extension. Anchor files are named
/// `<key_id>.pub` so the stem becomes the `public_key_id` registered
/// against [`MlDsaBundleVerifier`].
const ANCHOR_FILE_EXT: &str = "pub";

/// Conventional boot-bundle filename inside `trust.bundle_cache_dir`.
const BOOT_BUNDLE_FILENAME: &str = "bundle.json";

/// Conventional path for the persisted boot bundle.
///
/// Exposed so operator tooling and tests target the same file.
#[must_use]
pub fn boot_bundle_path(profile: &ShipProfile) -> PathBuf {
    profile.trust.bundle_cache_dir.join(BOOT_BUNDLE_FILENAME)
}

/// Selected behavior for `POST /v1/auth/exchange_token`.
///
/// The handler in `mai-api/src/handlers/trust.rs` switches on this at
/// the convergence step; right now the handler still always
/// mints the local-dev synthetic token regardless of profile.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrustExchangeMode {
    /// Local-dev synthetic token mint (the legacy behavior).
    /// Permitted only when the profile is `local-dev` and
    /// `trust.allow_local_dev_exchange = true`. Never used in
    /// production.
    LocalDevSynthetic,
    /// Real Trust Bridge exchange through the OpenBao client.
    /// Selected for every production profile; the live HTTP client
    /// lands in a follow-up session.
    OpenBaoBridge,
    /// Token exchange endpoint disabled. The handler returns 404 /
    /// 410. Used when the profile opts out of the synthetic path
    /// without wiring a bridge.
    Disabled,
}

impl TrustExchangeMode {
    /// Stable label for diagnostics / readiness reports.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::LocalDevSynthetic => "local-dev-synthetic",
            Self::OpenBaoBridge => "openbao-bridge",
            Self::Disabled => "disabled",
        }
    }
}

/// Outputs of [`build_trust_components`]. Each field is plumbed into
/// [`crate::state::AppState`] by the convergence step.
pub struct TrustComponents {
    /// Verifier installed on [`AppState::bundle_verifier`]. In
    /// production this is a fully-populated [`MlDsaBundleVerifier`];
    /// local-dev may fall back to [`AcceptAllBundleVerifier`] when
    /// `trust.allow_accept_all_verifier = true`.
    ///
    /// [`AppState::bundle_verifier`]: crate::state::AppState::bundle_verifier
    pub bundle_verifier: Arc<dyn BundleVerifier + Send + Sync>,
    /// Selected exchange-token behavior.
    pub exchange_mode: TrustExchangeMode,
    /// Identifiers of every trust anchor loaded into the verifier.
    /// Empty when the verifier is [`AcceptAllBundleVerifier`].
    pub anchor_ids: Vec<String>,
}

impl TrustComponents {
    /// Number of anchors loaded into the verifier.
    #[must_use]
    pub fn anchor_count(&self) -> usize {
        self.anchor_ids.len()
    }
}

/// Errors produced by [`build_trust_components`] and
/// [`verify_boot_bundle`].
#[derive(Debug, thiserror::Error)]
pub enum TrustBuildError {
    /// Production profile selected `trust.verifier = "accept-all"`.
    /// The parse-time validator already rejects this; the builder is
    /// the runtime second line of defense.
    #[error(
        "production profile selects trust.verifier = \"accept-all\"; \
         AcceptAllBundleVerifier is forbidden in production"
    )]
    AcceptAllInProduction,
    /// Production profile set `trust.allow_accept_all_verifier = true`.
    #[error(
        "production profile sets trust.allow_accept_all_verifier = true; \
         refused by the trust builder"
    )]
    AcceptAllAllowedInProduction,
    /// Production profile set `trust.allow_local_dev_exchange = true`.
    /// `LocalDevSynthetic` token exchange is never permitted in
    /// production.
    #[error(
        "production profile sets trust.allow_local_dev_exchange = true; \
         LocalDevSynthetic token exchange is forbidden in production"
    )]
    LocalDevExchangeInProduction,
    /// Production profile set `trust.require_trust_anchor = false`.
    #[error(
        "production profile sets trust.require_trust_anchor = false; \
         a trust anchor is required in production"
    )]
    TrustAnchorNotRequired,
    /// Production profile set `trust.require_bundle_on_boot = false`.
    #[error(
        "production profile sets trust.require_bundle_on_boot = false; \
         a verifiable boot bundle is required in production"
    )]
    BootBundleNotRequired,
    /// `trust.anchors_dir` is an empty path.
    #[error("trust.anchors_dir must be a non-empty path")]
    AnchorsDirEmpty,
    /// `trust.anchors_dir` is configured but the directory does not
    /// exist on disk.
    #[error(
        "trust.anchors_dir {path:?} does not exist; create it and \
         install at least one anchor before boot"
    )]
    AnchorsDirMissing {
        /// Configured directory.
        path: PathBuf,
    },
    /// `trust.anchors_dir` exists but is not a directory.
    #[error("trust.anchors_dir {path:?} is not a directory")]
    AnchorsDirNotDir {
        /// Configured path.
        path: PathBuf,
    },
    /// `trust.anchors_dir` could not be enumerated.
    #[error("trust.anchors_dir {path:?} could not be read: {source}")]
    AnchorsDirIo {
        /// Directory the builder tried to read.
        path: PathBuf,
        /// Underlying I/O error.
        #[source]
        source: std::io::Error,
    },
    /// No usable `*.pub` files were found inside `trust.anchors_dir`.
    /// In production this is always an error; local-dev tolerates an
    /// empty directory.
    #[error("trust.anchors_dir {path:?} contains no usable *.{ext} anchor files")]
    NoAnchorsFound {
        /// Directory the builder scanned.
        path: PathBuf,
        /// Expected file extension ([`ANCHOR_FILE_EXT`]).
        ext: &'static str,
    },
    /// A specific anchor file could not be read.
    #[error("trust anchor file {path:?} could not be read: {source}")]
    AnchorFileIo {
        /// Anchor file the builder tried to read.
        path: PathBuf,
        /// Underlying I/O error.
        #[source]
        source: std::io::Error,
    },
    /// An anchor file did not have exactly [`MLDSA87_PK_LEN`] bytes.
    #[error(
        "trust anchor file {path:?} has {actual} bytes; expected exactly \
         {expected} (raw ML-DSA-87 public key)"
    )]
    AnchorLengthInvalid {
        /// Anchor file.
        path: PathBuf,
        /// Required byte count.
        expected: usize,
        /// Byte count the file actually had.
        actual: usize,
    },
    /// Two anchor files claimed the same `<key_id>` stem.
    #[error("trust anchor id {id:?} appears twice (second copy at {path:?})")]
    DuplicateAnchorId {
        /// Conflicting anchor id.
        id: String,
        /// Second anchor file the builder found for that id.
        path: PathBuf,
    },
    /// `trust.bundle_cache_dir` is an empty path.
    #[error("trust.bundle_cache_dir must be a non-empty path")]
    BundleCacheDirEmpty,
    /// Production boot expected a persisted bundle and none was found.
    #[error("boot bundle {path:?} does not exist")]
    BootBundleMissing {
        /// Path the builder looked for.
        path: PathBuf,
    },
    /// The boot bundle file existed but could not be read.
    #[error("boot bundle {path:?} could not be read: {source}")]
    BootBundleIo {
        /// Path the builder tried to read.
        path: PathBuf,
        /// Underlying I/O error.
        #[source]
        source: std::io::Error,
    },
    /// The boot bundle file did not parse as a [`SignedPolicyBundle`].
    #[error("boot bundle {path:?} is not valid JSON: {source}")]
    BootBundleParse {
        /// Path the builder read.
        path: PathBuf,
        /// Underlying JSON parse error.
        #[source]
        source: serde_json::Error,
    },
    /// The boot bundle parsed but failed signature / window
    /// verification against the loaded anchors.
    #[error("boot bundle {path:?} failed verification: {source}")]
    BootBundleVerify {
        /// Path the builder verified.
        path: PathBuf,
        /// Underlying verifier error.
        #[source]
        source: BundleError,
    },
}

/// Construct the trust components selected by the profile.
///
/// Behavior matrix (selected rows; see acceptance tests for the rest):
///
/// | Mode        | verifier    | allow_accept_all | allow_local_dev_exchange | anchors_dir         | Outcome                          |
/// |-------------|-------------|------------------|--------------------------|---------------------|----------------------------------|
/// | production  | ml-dsa      | false            | false                    | exists w/ valid pub | `MlDsaBundleVerifier` + Bridge   |
/// | production  | accept-all  | any              | any                      | any                 | [`AcceptAllInProduction`]        |
/// | production  | ml-dsa      | true             | any                      | any                 | [`AcceptAllAllowedInProduction`] |
/// | production  | ml-dsa      | false            | true                     | any                 | [`LocalDevExchangeInProduction`] |
/// | production  | ml-dsa      | false            | false                    | missing             | [`AnchorsDirMissing`]            |
/// | production  | ml-dsa      | false            | false                    | empty               | [`NoAnchorsFound`]               |
/// | production  | ml-dsa      | false            | false                    | malformed anchor    | [`AnchorLengthInvalid`]          |
/// | local-dev   | accept-all  | true             | true                     | any                 | `AcceptAllBundleVerifier` + Synth|
/// | local-dev   | ml-dsa      | false            | false                    | missing             | empty `MlDsaBundleVerifier`      |
/// | local-dev   | ml-dsa      | false            | true                     | exists              | `MlDsaBundleVerifier` + Synth    |
///
/// [`AcceptAllInProduction`]: TrustBuildError::AcceptAllInProduction
/// [`AcceptAllAllowedInProduction`]: TrustBuildError::AcceptAllAllowedInProduction
/// [`LocalDevExchangeInProduction`]: TrustBuildError::LocalDevExchangeInProduction
/// [`AnchorsDirMissing`]: TrustBuildError::AnchorsDirMissing
/// [`NoAnchorsFound`]: TrustBuildError::NoAnchorsFound
/// [`AnchorLengthInvalid`]: TrustBuildError::AnchorLengthInvalid
/// Key id under which the compliance audit-chain signing key is registered as a
/// trust anchor, so `verify` checks the periodic ML-DSA audit signatures.
pub const AUDIT_CHAIN_KEY_ID: &str = "mai-audit-chain";

/// Build trust components without registering an audit-chain anchor (the common
/// path for tests and the ship validator).
pub fn build_trust_components(profile: &ShipProfile) -> Result<TrustComponents, TrustBuildError> {
    build_trust_components_with_audit_anchor(profile, None)
}

/// Build trust components, additionally registering `audit_chain_pubkey` (when
/// present) as the [`AUDIT_CHAIN_KEY_ID`] anchor so `verify` checks the
/// compliance audit chain's periodic signatures.
pub fn build_trust_components_with_audit_anchor(
    profile: &ShipProfile,
    audit_chain_pubkey: Option<&[u8]>,
) -> Result<TrustComponents, TrustBuildError> {
    let is_production = matches!(profile.profile.mode, ProfileMode::Production);

    if is_production {
        if matches!(profile.trust.verifier, TrustVerifier::AcceptAll) {
            return Err(TrustBuildError::AcceptAllInProduction);
        }
        if profile.trust.allow_accept_all_verifier {
            return Err(TrustBuildError::AcceptAllAllowedInProduction);
        }
        if profile.trust.allow_local_dev_exchange {
            return Err(TrustBuildError::LocalDevExchangeInProduction);
        }
        if !profile.trust.require_trust_anchor {
            return Err(TrustBuildError::TrustAnchorNotRequired);
        }
        if !profile.trust.require_bundle_on_boot {
            return Err(TrustBuildError::BootBundleNotRequired);
        }
        if profile.trust.bundle_cache_dir.as_os_str().is_empty() {
            return Err(TrustBuildError::BundleCacheDirEmpty);
        }
    }

    let (bundle_verifier, anchor_ids): (Arc<dyn BundleVerifier + Send + Sync>, Vec<String>) =
        match profile.trust.verifier {
            TrustVerifier::AcceptAll => {
                // Already rejected above for production; reachable
                // only in local-dev.
                (Arc::new(AcceptAllBundleVerifier), Vec::new())
            }
            TrustVerifier::MlDsa => {
                let (mut verifier, mut ids) =
                    load_anchors(&profile.trust.anchors_dir, is_production)?;
                // Register the compliance audit-chain public key so `verify`
                // checks the periodic signatures the vault-held key produces.
                if let Some(pubkey) = audit_chain_pubkey {
                    verifier =
                        verifier.with_anchor(AUDIT_CHAIN_KEY_ID.to_string(), pubkey.to_vec());
                    ids.push(AUDIT_CHAIN_KEY_ID.to_string());
                }
                (Arc::new(verifier), ids)
            }
        };

    let exchange_mode = if is_production {
        TrustExchangeMode::OpenBaoBridge
    } else if profile.trust.allow_local_dev_exchange {
        TrustExchangeMode::LocalDevSynthetic
    } else {
        TrustExchangeMode::Disabled
    };

    Ok(TrustComponents {
        bundle_verifier,
        exchange_mode,
        anchor_ids,
    })
}

/// Load every `*.pub` file in `dir` as an ML-DSA-87 public key and
/// register it on an [`MlDsaBundleVerifier`].
///
/// File-stem `<key_id>` becomes the `public_key_id` for the anchor.
/// Production requires at least one valid anchor; local-dev tolerates
/// an empty or absent directory so bring-up is not blocked on anchor
/// distribution.
fn load_anchors(
    dir: &Path,
    is_production: bool,
) -> Result<(MlDsaBundleVerifier, Vec<String>), TrustBuildError> {
    if dir.as_os_str().is_empty() {
        return Err(TrustBuildError::AnchorsDirEmpty);
    }
    if !dir.exists() {
        if is_production {
            return Err(TrustBuildError::AnchorsDirMissing {
                path: dir.to_path_buf(),
            });
        }
        return Ok((MlDsaBundleVerifier::new(), Vec::new()));
    }
    if !dir.is_dir() {
        return Err(TrustBuildError::AnchorsDirNotDir {
            path: dir.to_path_buf(),
        });
    }

    let entries = fs::read_dir(dir).map_err(|source| TrustBuildError::AnchorsDirIo {
        path: dir.to_path_buf(),
        source,
    })?;

    // Use a BTreeMap so anchor registration order is deterministic and
    // diagnostics are stable across platforms.
    let mut loaded: BTreeMap<String, Vec<u8>> = BTreeMap::new();
    for entry in entries {
        let entry = entry.map_err(|source| TrustBuildError::AnchorsDirIo {
            path: dir.to_path_buf(),
            source,
        })?;
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        if path.extension().and_then(|s| s.to_str()) != Some(ANCHOR_FILE_EXT) {
            continue;
        }
        let id = match path.file_stem().and_then(|s| s.to_str()) {
            Some(s) if !s.is_empty() => s.to_string(),
            _ => continue,
        };
        let bytes = fs::read(&path).map_err(|source| TrustBuildError::AnchorFileIo {
            path: path.clone(),
            source,
        })?;
        if bytes.len() != MLDSA87_PK_LEN {
            return Err(TrustBuildError::AnchorLengthInvalid {
                path,
                expected: MLDSA87_PK_LEN,
                actual: bytes.len(),
            });
        }
        if loaded.insert(id.clone(), bytes).is_some() {
            return Err(TrustBuildError::DuplicateAnchorId { id, path });
        }
    }

    if loaded.is_empty() {
        if is_production {
            return Err(TrustBuildError::NoAnchorsFound {
                path: dir.to_path_buf(),
                ext: ANCHOR_FILE_EXT,
            });
        }
        return Ok((MlDsaBundleVerifier::new(), Vec::new()));
    }

    let mut verifier = MlDsaBundleVerifier::new();
    let mut ids = Vec::with_capacity(loaded.len());
    for (id, bytes) in loaded {
        verifier = verifier.with_anchor(id.clone(), bytes);
        ids.push(id);
    }
    Ok((verifier, ids))
}

/// Load and verify the persisted boot bundle.
///
/// Reads `<trust.bundle_cache_dir>/bundle.json`, deserializes it as a
/// [`SignedPolicyBundle`], and verifies its signature + freshness
/// window against `verifier` at wall-clock `now_secs`. On success the
/// bundle's `metadata.version` is returned so the readiness report
/// can surface it.
///
/// Production startup must call this and refuse to bind on any
/// error. The call lives in `MaiServer::run()` at the
/// convergence step.
pub fn verify_boot_bundle(
    profile: &ShipProfile,
    verifier: &dyn BundleVerifier,
    now_secs: u64,
) -> Result<String, TrustBuildError> {
    if profile.trust.bundle_cache_dir.as_os_str().is_empty() {
        return Err(TrustBuildError::BundleCacheDirEmpty);
    }
    let path = boot_bundle_path(profile);
    if !path.exists() {
        return Err(TrustBuildError::BootBundleMissing { path });
    }
    let bytes = fs::read(&path).map_err(|source| TrustBuildError::BootBundleIo {
        path: path.clone(),
        source,
    })?;
    let bundle: SignedPolicyBundle =
        serde_json::from_slice(&bytes).map_err(|source| TrustBuildError::BootBundleParse {
            path: path.clone(),
            source,
        })?;
    // `SignedPolicyBundle::verified_payload<V>` takes a `Sized` generic
    // verifier; wrap the trait-object reference so the trait-object
    // version of the AppState verifier can still drive verification.
    let wrapper = DynVerifierWrapper(verifier);
    bundle
        .verified_payload(&wrapper, now_secs)
        .map_err(|source| TrustBuildError::BootBundleVerify {
            path: path.clone(),
            source,
        })?;
    Ok(bundle.metadata.version)
}

/// Sized adapter around `&dyn BundleVerifier`.
///
/// `SignedPolicyBundle::verified_payload<V>` requires `V: Sized`, which
/// excludes trait-object references. Wrapping in this tiny newtype
/// satisfies the bound without forcing the caller to either own a
/// concrete verifier or downcast through `Arc`.
struct DynVerifierWrapper<'a>(&'a dyn BundleVerifier);

impl BundleVerifier for DynVerifierWrapper<'_> {
    fn verify(
        &self,
        payload_hash: &[u8; 32],
        signature_bytes: &[u8],
        public_key_id: &str,
    ) -> Result<(), BundleError> {
        self.0.verify(payload_hash, signature_bytes, public_key_id)
    }
}

// ─── Unit tests ─────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exchange_mode_label() {
        assert_eq!(
            TrustExchangeMode::LocalDevSynthetic.label(),
            "local-dev-synthetic"
        );
        assert_eq!(TrustExchangeMode::OpenBaoBridge.label(), "openbao-bridge");
        assert_eq!(TrustExchangeMode::Disabled.label(), "disabled");
    }

    #[test]
    fn anchor_file_constants_match_compliance_crate() {
        // Sanity: the duplicated ML-DSA-87 PK length matches the wire
        // contract documented in TRUST-BUNDLE-SPEC.md.
        assert_eq!(MLDSA87_PK_LEN, 2592);
        assert_eq!(ANCHOR_FILE_EXT, "pub");
        assert_eq!(BOOT_BUNDLE_FILENAME, "bundle.json");
    }
}
