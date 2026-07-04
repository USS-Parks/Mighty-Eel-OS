//! Canonical-JSON encoding + BLAKE3 hashing.
//!
//! Byte-identical to mai-compliance's `write_canonical`: object keys are
//! emitted in lexicographic order, arrays preserve their order, and scalars use
//! the default `serde_json` encoding. This guarantees a bundle hashes the same
//! in `fabric-proof` and `mai-compliance`.

use std::collections::BTreeMap;

use serde::Serialize;

use crate::error::ProofError;

/// Canonicalize a `serde_json::Value` into a deterministic byte sequence.
pub fn write_canonical(out: &mut Vec<u8>, value: &serde_json::Value) {
    match value {
        serde_json::Value::Null => out.extend_from_slice(b"null"),
        serde_json::Value::Bool(true) => out.extend_from_slice(b"true"),
        serde_json::Value::Bool(false) => out.extend_from_slice(b"false"),
        serde_json::Value::Number(n) => out.extend_from_slice(n.to_string().as_bytes()),
        serde_json::Value::String(s) => {
            let encoded =
                serde_json::to_string(s).expect("serde_json never fails to encode a string");
            out.extend_from_slice(encoded.as_bytes());
        }
        serde_json::Value::Array(arr) => {
            out.push(b'[');
            for (i, v) in arr.iter().enumerate() {
                if i > 0 {
                    out.push(b',');
                }
                write_canonical(out, v);
            }
            out.push(b']');
        }
        serde_json::Value::Object(map) => {
            out.push(b'{');
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            for (i, k) in keys.iter().enumerate() {
                if i > 0 {
                    out.push(b',');
                }
                let key_encoded =
                    serde_json::to_string(k).expect("serde_json never fails to encode a string");
                out.extend_from_slice(key_encoded.as_bytes());
                out.push(b':');
                write_canonical(out, &map[*k]);
            }
            out.push(b'}');
        }
    }
}

/// Canonical-JSON bytes of any serializable value.
///
/// # Errors
/// Returns [`ProofError::Serialize`] if `value` cannot be represented as JSON.
pub fn canonical_bytes<T: Serialize>(value: &T) -> Result<Vec<u8>, ProofError> {
    let v = serde_json::to_value(value).map_err(|e| ProofError::Serialize(e.to_string()))?;
    let mut buf = Vec::new();
    write_canonical(&mut buf, &v);
    Ok(buf)
}

/// BLAKE3-32 of the canonical-JSON encoding of `value`.
///
/// # Errors
/// Returns [`ProofError::Serialize`] if `value` cannot be represented as JSON.
pub fn canonical_hash<T: Serialize>(value: &T) -> Result<[u8; 32], ProofError> {
    Ok(*blake3::hash(&canonical_bytes(value)?).as_bytes())
}

/// BLAKE3-32 of the canonical-JSON encoding of `{"metadata": M, "payload": P}`.
///
/// Matches mai-compliance `payload_hash` exactly, so signed bundles verify
/// across both crates.
///
/// # Errors
/// Returns [`ProofError::Serialize`] if either value cannot be represented as JSON.
pub fn combined_hash<M: Serialize, P: Serialize>(
    metadata: &M,
    payload: &P,
) -> Result<[u8; 32], ProofError> {
    let metadata_value =
        serde_json::to_value(metadata).map_err(|e| ProofError::Serialize(e.to_string()))?;
    let payload_value =
        serde_json::to_value(payload).map_err(|e| ProofError::Serialize(e.to_string()))?;
    let mut combined = BTreeMap::new();
    combined.insert("metadata".to_string(), metadata_value);
    combined.insert("payload".to_string(), payload_value);
    let combined_value =
        serde_json::to_value(combined).map_err(|e| ProofError::Serialize(e.to_string()))?;
    let mut buf = Vec::new();
    write_canonical(&mut buf, &combined_value);
    Ok(*blake3::hash(&buf).as_bytes())
}
