//! H4 gate — a full DR drill from a cold, encrypted backup succeeds by the runbook
//! (`docs/LOOM-DR-RUNBOOK.md`) alone: back up a live estate envelope-sealed, lose
//! the control plane entirely, then cold-restore its content into a fresh estate
//! from the sealed blob on media plus the escrowed data key — no primary state.

use aog_apiserver::backup::{backup_estate, restore_estate};
use aog_store::raft::RaftNode;
use aog_store::{Op, Precondition};

fn base(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(name);
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

/// The escrowed DR data key — in production an OpenBao Transit-wrapped key the
/// operator unwraps at restore time; here a fixed test key (the runbook's step 1).
const DR_KEY: [u8; 32] = [0x5c; 32];

#[tokio::test]
async fn a_full_dr_drill_from_cold_backup_succeeds() {
    // Removable media / off-site store — outlives the control-plane hosts.
    let media = base("loom-h4-media");
    std::fs::create_dir_all(&media).unwrap();
    let backup_file = media.join("estate.sealed");

    // ── Take a backup of a live estate, then lose the control plane entirely.
    let manifest: Vec<(String, Vec<u8>)> = {
        let dir = base("loom-h4-primary");
        let node = RaftNode::bootstrap(1, &dir).await.unwrap();
        for i in 0..12 {
            node.write(Op::Put {
                key: format!("Workload/w{i:02}"),
                value: format!("replicas={i}").into_bytes(),
                expected: Precondition::Any,
            })
            .await
            .unwrap();
        }
        let entries = node.range("").await.unwrap();
        let sealed = backup_estate(&entries, &DR_KEY).unwrap();
        std::fs::write(&backup_file, &sealed).unwrap();
        node.shutdown().await.unwrap();

        // Disaster: the primary control-plane host is gone, stores and all.
        std::fs::remove_dir_all(&dir).unwrap();
        entries.into_iter().map(|(k, v)| (k, v.value)).collect()
    };

    // ── Cold restore on a clean host, following the runbook's restore steps.
    // (1-2) Recover the data key from escrow; read the sealed blob from media.
    let sealed = std::fs::read(&backup_file).unwrap();
    // (3) Unseal — a wrong key or tampered blob would fail closed here.
    let restored = restore_estate(&sealed, &DR_KEY).unwrap();
    assert_eq!(restored.len(), 12, "every backed-up entry is recovered");
    // (4) Bootstrap a fresh single-node control plane on the clean host.
    let dr_dir = base("loom-h4-dr");
    let node = RaftNode::bootstrap(2, &dr_dir).await.unwrap();
    // (5) Re-apply every entry.
    for entry in &restored {
        node.write(Op::Put {
            key: entry.key.clone(),
            value: entry.value.clone(),
            expected: Precondition::Any,
        })
        .await
        .unwrap();
    }
    // (6) Verify the restored content against the manifest.
    let recovered: Vec<(String, Vec<u8>)> = node
        .range("")
        .await
        .unwrap()
        .into_iter()
        .map(|(k, v)| (k, v.value))
        .collect();
    assert_eq!(
        recovered, manifest,
        "the cold restore reproduces the estate content by the runbook alone"
    );
    let spot = node
        .get("Workload/w05")
        .await
        .unwrap()
        .expect("key present");
    assert_eq!(
        spot.value, b"replicas=5",
        "a spot-checked value is restored verbatim"
    );

    // Prove the backup on media was genuinely encrypted (not the primary state).
    assert!(
        !sealed
            .windows(b"replicas=5".len())
            .any(|w| w == b"replicas=5"),
        "the backup blob on media is ciphertext at rest"
    );

    node.shutdown().await.unwrap();
    let _ = std::fs::remove_dir_all(&media);
}
