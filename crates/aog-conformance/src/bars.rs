//! In-process bar checks that run against the real `aog-store` Raft state machine
//! and the real `aog-controller` reconcile runtime. Each returns `Ok(detail)` on
//! pass or `Err(detail)` on fail — never a panic, so the suite always produces a
//! full report.

use std::collections::BTreeMap;
use std::future::Future;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use aog_controller::{Action, AlwaysLeader, Controller, ReconcileError, Reconciler};
use aog_store::raft::RaftNode;
use aog_store::raft::types::RaftResponse;
use aog_store::{Op, Precondition};

/// A fresh, empty scratch dir for a single check's Raft state.
fn scratch(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(name);
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

/// A small deterministic PRNG (SplitMix64) — the fuzz is reproducible, so a
/// divergence is reported with the exact seed and history index.
struct SplitMix64(u64);

impl SplitMix64 {
    fn new(seed: u64) -> Self {
        Self(seed)
    }

    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// A value in `0..n` (n >= 1).
    fn below(&mut self, n: u64) -> u64 {
        self.next_u64() % n.max(1)
    }
}

async fn put(node: &RaftNode, key: &str, value: &str) -> Result<(), String> {
    node.write(Op::Put {
        key: key.to_owned(),
        value: value.as_bytes().to_vec(),
        expected: Precondition::Any,
    })
    .await
    .map(|_| ())
    .map_err(|e| format!("put {key} failed: {e:?}"))
}

async fn delete(node: &RaftNode, key: &str) -> Result<(), String> {
    node.write(Op::Delete {
        key: key.to_owned(),
        expected: Precondition::Any,
    })
    .await
    .map(|_| ())
    .map_err(|e| format!("delete {key} failed: {e:?}"))
}

/// A level-triggered reconciler that records the *current* store value for each
/// key it is woken for — the observable end state the fuzz compares against the
/// store's authoritative state.
#[derive(Clone)]
struct Recorder {
    node: Arc<RaftNode>,
    state: Arc<Mutex<BTreeMap<String, Option<String>>>>,
}

impl Recorder {
    fn new(node: Arc<RaftNode>) -> Self {
        Self {
            node,
            state: Arc::new(Mutex::new(BTreeMap::new())),
        }
    }

    fn recorded(&self) -> BTreeMap<String, Option<String>> {
        self.state.lock().unwrap().clone()
    }
}

impl Reconciler for Recorder {
    fn reconcile(&self, key: &str) -> impl Future<Output = Result<Action, ReconcileError>> + Send {
        let node = Arc::clone(&self.node);
        let state = Arc::clone(&self.state);
        let key = key.to_owned();
        async move {
            let current = node
                .get(&key)
                .await
                .map_err(|e| ReconcileError(e.to_string()))?;
            let value = current.map(|v| String::from_utf8_lossy(&v.value).into_owned());
            state.lock().unwrap().insert(key, value);
            Ok(Action::Done)
        }
    }
}

/// Drive sync passes until one does nothing and the queue is empty (bounded).
async fn settle<R: Reconciler>(controller: &mut Controller<R>) -> Result<(), String> {
    for _ in 0..256 {
        let stats = controller
            .sync(Instant::now())
            .await
            .map_err(|e| format!("sync pass failed: {e:?}"))?;
        if stats.enqueued == 0
            && stats.drained == 0
            && stats.processed == 0
            && controller.queue_len() == 0
        {
            return Ok(());
        }
    }
    Err("controller did not settle within 256 sync passes".to_owned())
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

/// Bar 1 — level-triggered, idempotent reconciliation. A fixed base estate is
/// reconciled to convergence, then a fixed late history (an update, a delete, a
/// create) is delivered under `histories` randomized modes — the events reordered
/// and duplicated, and (occasionally) genuinely dropped by overflowing the watch
/// buffer so the controller must re-list. Every history must converge to the
/// store's one authoritative end state. Extends the R1 replay gate (three fixed
/// histories) to a reproducible fuzz.
pub async fn idempotent_reconcile(histories: usize, seed: u64) -> Result<String, String> {
    const PREFIX: &str = "Loom/";
    let base_keys = [
        "Loom/k0", "Loom/k1", "Loom/k2", "Loom/k3", "Loom/k4", "Loom/k5",
    ];
    let late_key = "Loom/k6";
    // The one authoritative end state after base + late (put k0=v2, del k1, put k6).
    let expected: BTreeMap<String, Option<String>> = [
        ("Loom/k0".to_owned(), Some("v2".to_owned())),
        ("Loom/k1".to_owned(), None),
        ("Loom/k2".to_owned(), Some("v1".to_owned())),
        ("Loom/k3".to_owned(), Some("v1".to_owned())),
        ("Loom/k4".to_owned(), Some("v1".to_owned())),
        ("Loom/k5".to_owned(), Some("v1".to_owned())),
        ("Loom/k6".to_owned(), Some("v1".to_owned())),
    ]
    .into_iter()
    .collect();

    let dir = scratch("loom-conformance-idempotency");
    let node = Arc::new(
        RaftNode::bootstrap(1, &dir)
            .await
            .map_err(|e| format!("bootstrap failed: {e:?}"))?,
    );
    let mut prng = SplitMix64::new(seed);
    let mut overflow_runs = 0u32;

    for i in 0..histories {
        // 1. Reset the estate to the base state.
        for k in base_keys {
            put(&node, k, "v1").await?;
        }
        let _ = delete(&node, late_key).await;

        // 2. A fresh controller observes the base and converges.
        let recorder = Recorder::new(Arc::clone(&node));
        let mut controller = Controller::new(
            "idempotency",
            node.informer(PREFIX),
            recorder.clone(),
            Arc::new(AlwaysLeader),
        );
        settle(&mut controller).await?;

        // 3. The fixed late history.
        put(&node, "Loom/k0", "v2").await?;
        delete(&node, "Loom/k1").await?;
        put(&node, late_key, "v1").await?;

        // 4. Deliver the affected keys under a randomized mode: reorder + duplicate.
        let mut order: Vec<&str> = vec!["Loom/k0", "Loom/k1", late_key];
        for j in (1..order.len()).rev() {
            let k = usize::try_from(prng.below(j as u64 + 1)).unwrap_or(0);
            order.swap(j, k);
        }
        for &k in &order {
            for _ in 0..prng.below(3) {
                controller.enqueue(k);
            }
        }
        // Occasionally drop for real: overflow the watch buffer (a foreign prefix,
        // so the Loom end state is untouched) — the next poll lags and re-lists.
        if prng.below(50) == 0 {
            overflow_runs += 1;
            for n in 0..80u32 {
                put(&node, &format!("Noise/{n:03}"), "x").await?;
            }
        }

        // 5. Converge, then assert the one authoritative end state.
        settle(&mut controller).await?;
        let got = recorder.recorded();
        if got != expected {
            return Err(format!(
                "history {i} (seed {seed}) diverged: got {got:?}, want {expected:?}"
            ));
        }
    }

    Ok(format!(
        "{histories} randomized delivery histories (reorder + duplicate + {overflow_runs} overflow-drop) all converged to the one authoritative end state; zero divergence"
    ))
}
