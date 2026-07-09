//! Encrypted estate backup + cold restore. A backup is the estate's
//! committed key/value content, serialized and **envelope-sealed** (AES-256-GCM
//! under a 32-byte data key, `fabric-envelope`) so it is ciphertext at rest — safe
//! to write to removable media or off-site storage. Restore unseals it with the
//! same data key and re-applies each entry to a fresh estate.
//!
//! The data key is the DR key an operator escrows (wrapped via OpenBao Transit in
//! production; the `data_key_wrapped` field carries that opaque reference). The
//! runbook (`docs/LOOM-DR-RUNBOOK.md`) is the human procedure; this module is the
//! machinery it drives, and the DR drill (`tests/dr_drill.rs`) proves a cold
//! restore reproduces the estate content by the runbook alone.

use fabric_contracts::Seal;
use fabric_envelope::{seal, unseal};
use serde::{Deserialize, Serialize};

use aog_store::Versioned;

/// Additional authenticated data binding a backup to its purpose — a seal made
/// for something else will not unseal as an estate backup.
const BACKUP_AAD: &[u8] = b"loom-dr-estate-backup-v1";
/// The opaque wrapped-data-key reference recorded in the seal (in production the
/// OpenBao Transit ciphertext; here the escrow reference the runbook resolves).
const DATA_KEY_REF: &str = "openbao:transit/loom-dr-backup";

/// A backup failure — sealing, unsealing, or (de)serialization.
#[derive(Debug, thiserror::Error)]
pub enum BackupError {
    #[error("serialize: {0}")]
    Serialize(String),
    #[error("envelope: {0}")]
    Envelope(String),
}

/// One backed-up estate entry: a key and its committed value bytes. Revisions are
/// re-established on restore (a logical backup restores content), so only the
/// authoritative key→value mapping is carried.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackupEntry {
    pub key: String,
    pub value: Vec<u8>,
}

/// Seal an estate dump into an encrypted backup blob. `entries` is the estate's
/// committed content (e.g. from `RaftNode::range("")`); the returned bytes are
/// ciphertext — nothing readable without the data key.
///
/// # Errors
/// [`BackupError`] on serialization or sealing failure.
pub fn backup_estate(
    entries: &[(String, Versioned)],
    data_key: &[u8; 32],
) -> Result<Vec<u8>, BackupError> {
    let dump: Vec<BackupEntry> = entries
        .iter()
        .map(|(key, versioned)| BackupEntry {
            key: key.clone(),
            value: versioned.value.clone(),
        })
        .collect();
    let plaintext = serde_json::to_vec(&dump).map_err(|e| BackupError::Serialize(e.to_string()))?;
    let sealed = seal(&plaintext, data_key, DATA_KEY_REF, BACKUP_AAD)
        .map_err(|e| BackupError::Envelope(e.to_string()))?;
    serde_json::to_vec(&sealed).map_err(|e| BackupError::Serialize(e.to_string()))
}

/// Unseal an encrypted backup blob back into its estate entries, with the same
/// data key it was sealed under. The caller re-applies each `(key, value)` to a
/// fresh estate (a `Put` per entry).
///
/// # Errors
/// [`BackupError`] on deserialization or unsealing failure (a wrong key, tampered
/// ciphertext, or a seal made for another purpose all fail closed).
pub fn restore_estate(sealed: &[u8], data_key: &[u8; 32]) -> Result<Vec<BackupEntry>, BackupError> {
    let seal: Seal =
        serde_json::from_slice(sealed).map_err(|e| BackupError::Serialize(e.to_string()))?;
    let plaintext =
        unseal(&seal, data_key, BACKUP_AAD).map_err(|e| BackupError::Envelope(e.to_string()))?;
    serde_json::from_slice(&plaintext).map_err(|e| BackupError::Serialize(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn versioned(value: &[u8]) -> Versioned {
        Versioned {
            value: value.to_vec(),
            create_revision: 1,
            mod_revision: 1,
            version: 1,
        }
    }

    #[test]
    fn a_backup_round_trips_under_the_right_key() {
        let key = [7u8; 32];
        let estate = vec![
            ("Workload/gw".to_owned(), versioned(b"spec-a")),
            ("Capability/cap".to_owned(), versioned(b"spec-b")),
        ];
        let sealed = backup_estate(&estate, &key).unwrap();
        let restored = restore_estate(&sealed, &key).unwrap();
        assert_eq!(restored.len(), 2);
        assert_eq!(restored[0].key, "Workload/gw");
        assert_eq!(restored[0].value, b"spec-a");
    }

    #[test]
    fn a_backup_is_ciphertext_at_rest() {
        let key = [7u8; 32];
        let estate = vec![("Secret/s".to_owned(), versioned(b"top-secret-value"))];
        let sealed = backup_estate(&estate, &key).unwrap();
        // The plaintext value must not appear in the sealed bytes.
        assert!(
            !sealed
                .windows(b"top-secret-value".len())
                .any(|w| w == b"top-secret-value"),
            "the backup blob must be encrypted at rest"
        );
    }

    #[test]
    fn the_wrong_key_fails_closed() {
        let estate = vec![("Workload/gw".to_owned(), versioned(b"spec"))];
        let sealed = backup_estate(&estate, &[7u8; 32]).unwrap();
        assert!(
            restore_estate(&sealed, &[9u8; 32]).is_err(),
            "a wrong data key must not decrypt the backup"
        );
    }
}
