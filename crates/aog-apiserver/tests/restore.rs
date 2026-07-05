//! H3 gate — restore + receipt-chain continuity. A snapshot/restore of the estate
//! (aog-store) reproduces it **exactly**, and the receipt chain (`wsf-ledger`) — a
//! physically separate, hash-chained store (A1.4: intent and proof never share a
//! store) — **remains chained** across that restore: the estate restore neither
//! reads nor writes it, the chain still verifies off-host with the public key
//! alone, and a post-restore receipt links unbroken onto the pre-restore head.

use std::sync::Arc;
use std::time::Duration;

use aog_store::raft::RaftNode;
use aog_store::{Op, Precondition};
use fabric_crypto::Signer;
use fabric_crypto::providers::{MlDsa87Verifier, RustCryptoMlDsa87};
use wsf_ledger::{Ledger, verify_pack};

fn base(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(name);
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

#[tokio::test]
async fn a_restore_reproduces_the_estate_and_keeps_the_receipt_chain_intact() {
    // A hash-chained receipt ledger with a verifiable head + signed pack.
    let signer: Arc<dyn Signer> = Arc::new(RustCryptoMlDsa87::generate("loom-h3-ledger").unwrap());
    let mut ledger = Ledger::new(Arc::clone(&signer));
    for i in 0..5 {
        ledger
            .ingest(
                "admission",
                serde_json::json!({ "decision": "admit", "seq": i }),
            )
            .unwrap();
    }
    let head_before = ledger.verify().expect("receipt chain valid before restore");
    let pack = ledger.export_pack("2026-07-05T00:00:00Z").unwrap();
    assert!(
        verify_pack(&pack, &MlDsa87Verifier, ledger.public_key()),
        "the receipt pack verifies off-host before the restore"
    );

    // An estate: write, snapshot, and capture the exact committed state.
    let dir = base("loom-h3-restore");
    let expected = {
        let node = RaftNode::bootstrap(1, &dir).await.unwrap();
        for i in 0..10 {
            node.write(Op::Put {
                key: format!("Capability/c{i:02}"),
                value: format!("cap-{i}").into_bytes(),
                expected: Precondition::Any,
            })
            .await
            .unwrap();
        }
        node.snapshot(Duration::from_secs(10)).await.unwrap();
        let estate = node.range("").await.unwrap();
        node.shutdown().await.unwrap();
        estate
    };

    // Restore the estate from the snapshot + durable stores — reproduced exactly.
    let node = RaftNode::bootstrap(1, &dir).await.unwrap();
    assert_eq!(
        node.range("").await.unwrap(),
        expected,
        "restore reproduces the exact estate"
    );
    node.shutdown().await.unwrap();

    // The receipt chain — a physically separate store — was untouched by the
    // estate restore: same head, still off-host verifiable.
    assert_eq!(
        ledger.verify().expect("receipt chain valid after restore"),
        head_before,
        "the receipt chain head is unchanged across the estate restore"
    );
    assert!(
        verify_pack(&pack, &MlDsa87Verifier, ledger.public_key()),
        "the exported receipt pack still verifies after the estate restore"
    );

    // And the chain continues unbroken: a post-restore receipt links onto the
    // pre-restore head, and the whole chain still verifies.
    let head_after = ledger
        .ingest(
            "admission",
            serde_json::json!({ "decision": "admit", "seq": 5, "phase": "post-restore" }),
        )
        .unwrap();
    assert_ne!(
        head_after, head_before,
        "the chain advanced with the new receipt"
    );
    assert_eq!(
        ledger.verify().expect("chain valid after extending"),
        head_after,
        "the post-restore receipt chains unbroken onto the pre-restore head"
    );
    assert_eq!(ledger.len(), 6);
}
