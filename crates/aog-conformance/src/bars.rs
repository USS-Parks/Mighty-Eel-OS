//! In-process bar checks that run against the real `aog-store` Raft state
//! machine. Each returns `Ok(detail)` on pass or `Err(detail)` on fail — never a
//! panic, so the suite always produces a full report.

use aog_store::raft::RaftNode;
use aog_store::raft::types::RaftResponse;
use aog_store::{Op, Precondition};

/// A fresh, empty scratch dir for a single check's Raft state.
fn scratch(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(name);
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

/// Bar 2 — linearizable writes / no lost update. Two compare-and-set writes pin
/// the same base revision through the Raft log; the first commits and the second
/// is rejected stale, so the committed value is always the winner's and the
/// global revision advances by exactly one — a lost update cannot occur. (A
/// rejected precondition is a `RaftResponse::Rejected` value, not a Raft error;
/// V3 deepens this Jepsen-style with concurrent clients under fault injection.)
pub async fn linearizable_writes() -> Result<String, String> {
    const KEY: &str = "Workload/conformance-cas";

    let dir = scratch("loom-conformance-linearizable");
    let node = RaftNode::bootstrap(1, &dir)
        .await
        .map_err(|e| format!("bootstrap failed: {e:?}"))?;

    // Seed the key, then capture the revision the compare-and-sets will pin to.
    let seed = node
        .write(Op::Put {
            key: KEY.to_owned(),
            value: b"v0".to_vec(),
            expected: Precondition::Absent,
        })
        .await
        .map_err(|e| format!("seed write failed: {e:?}"))?;
    if !matches!(seed, RaftResponse::Applied { created: true, .. }) {
        node.shutdown().await.ok();
        return Err(format!("seed did not create the key: {seed:?}"));
    }
    let base_rev = match node.get(KEY).await {
        Ok(Some(v)) => v.mod_revision,
        Ok(None) => {
            node.shutdown().await.ok();
            return Err("seeded key is missing".to_owned());
        }
        Err(e) => {
            node.shutdown().await.ok();
            return Err(format!("read-back failed: {e:?}"));
        }
    };
    let rev_before = node.revision().await;

    // Two compare-and-sets pinned to the same base revision.
    let first = node
        .write(Op::Put {
            key: KEY.to_owned(),
            value: b"first".to_vec(),
            expected: Precondition::Revision(base_rev),
        })
        .await
        .map_err(|e| format!("first CAS raft error: {e:?}"))?;
    let second = node
        .write(Op::Put {
            key: KEY.to_owned(),
            value: b"second".to_vec(),
            expected: Precondition::Revision(base_rev),
        })
        .await
        .map_err(|e| format!("second CAS raft error: {e:?}"))?;

    let final_value = node
        .get(KEY)
        .await
        .map_err(|e| format!("final read failed: {e:?}"));
    let rev_after = node.revision().await;
    node.shutdown().await.ok();

    // The first CAS commits; the second, pinned to a now-stale revision, is rejected.
    if !matches!(first, RaftResponse::Applied { .. }) {
        return Err(format!(
            "the first CAS at revision {base_rev} was not applied: {first:?}"
        ));
    }
    if !matches!(second, RaftResponse::Rejected { .. }) {
        return Err(format!(
            "the second CAS at the same revision {base_rev} was not rejected ({second:?}) — a lost update"
        ));
    }

    // The committed value is the winner's, and exactly one mutation advanced the
    // revision — a rejected write must not touch state.
    let winner: &[u8] = b"first";
    match final_value {
        Ok(Some(v)) if v.value == winner => {}
        Ok(Some(v)) => {
            return Err(format!(
                "committed value {:?} is not the CAS winner {winner:?} — lost update",
                v.value
            ));
        }
        Ok(None) => return Err("key vanished after a committed write".to_owned()),
        Err(e) => return Err(e),
    }
    if rev_after != rev_before + 1 {
        return Err(format!(
            "revision advanced by {} across two CAS writes (expected exactly 1) — a rejected write mutated state",
            rev_after - rev_before
        ));
    }

    Ok(format!(
        "at revision {base_rev}, the first CAS committed and the second was rejected stale; the global revision advanced by exactly one — no lost update"
    ))
}
