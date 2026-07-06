//! In-process bar checks that run against the real `aog-store` Raft state machine
//! and the real `aog-controller` reconcile runtime. Each returns `Ok(detail)` on
//! pass or `Err(detail)` on fail — never a panic, so the suite always produces a
//! full report.

use std::collections::{BTreeMap, BTreeSet};
use std::future::Future;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use aog_controller::{Action, AlwaysLeader, Controller, ReconcileError, Reconciler};
use aog_store::raft::types::RaftResponse;
use aog_store::raft::{Cluster, RaftNode};
use aog_store::{Op, Precondition};
use chrono::{DateTime, Duration as ChronoDuration, Utc};
use fabric_crypto::Signer;
use fabric_crypto::providers::{MlDsa87Verifier, RustCryptoMlDsa87};
use fabric_revocation::{RevocationSnapshot, sign as sign_snapshot, verify as verify_snapshot};

/// A fresh, unique scratch dir for a single check's Raft state — unique per call
/// (process id + a monotonic counter) so concurrently-running tests never share a
/// redb file, which would fail to acquire its lock.
fn scratch(name: &str) -> std::path::PathBuf {
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("{name}-{}-{seq}", std::process::id()));
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

/// The index (into `nodes`) of a node that currently confirms quorum leadership,
/// or `None` if none can right now (e.g. mid-election). A confirmed leader is
/// authoritative — a fenced minority never confirms — so writing only here makes
/// a stale allow impossible by construction.
async fn confirmed_leader(nodes: &[Arc<RaftNode>]) -> Option<usize> {
    for (i, node) in nodes.iter().enumerate() {
        if node.confirm_leadership(Duration::from_millis(200)).await {
            return Some(i);
        }
    }
    None
}

fn parse_counter(bytes: &[u8]) -> Option<u64> {
    std::str::from_utf8(bytes).ok().and_then(|s| s.parse().ok())
}

/// Bar 2 deepened (V3) — linearizability under concurrent clients + fault
/// injection (Jepsen-style). `clients` tasks race compare-and-set *increments* of
/// one counter through a real 3-node Raft cluster while a fault task repeatedly
/// isolates then heals a single node (never a majority), forcing real leader
/// failovers. Each committed increment is acknowledged. The register invariant:
/// **acknowledged increments must be ≤ the final counter** — a lost update or a
/// stale allow would push acks above the counter; only benign ambiguous in-flight
/// commits raise the counter above acks. Clients write solely to a
/// quorum-confirmed leader, so a fenced minority never serves an allow.
pub async fn linearizable_under_faults(
    clients: usize,
    attempts: usize,
    seed: u64,
) -> Result<String, String> {
    const KEY: &str = "Counter/linearizability";

    let dir = scratch("loom-conformance-linearizability");
    let cluster = Arc::new(Cluster::new());
    let mut nodes: Vec<Arc<RaftNode>> = Vec::with_capacity(3);
    for id in 1..=3u64 {
        nodes.push(Arc::new(
            RaftNode::join(id, dir.join(format!("n{id}")), &cluster)
                .await
                .map_err(|e| format!("node {id} join failed: {e:?}"))?,
        ));
    }
    nodes[0]
        .initialize(BTreeSet::from([1]))
        .await
        .map_err(|e| format!("initialize failed: {e:?}"))?;
    nodes[0]
        .wait_for_leader(Duration::from_secs(10))
        .await
        .map_err(|e| format!("no leader: {e:?}"))?;
    nodes[0]
        .add_learner(2)
        .await
        .map_err(|e| format!("add learner 2: {e:?}"))?;
    nodes[0]
        .add_learner(3)
        .await
        .map_err(|e| format!("add learner 3: {e:?}"))?;
    nodes[0]
        .change_membership(BTreeSet::from([1, 2, 3]))
        .await
        .map_err(|e| format!("change membership: {e:?}"))?;

    // Seed the counter at 0 on the authoritative leader.
    {
        let li = confirmed_leader(&nodes)
            .await
            .ok_or("no confirmed leader to seed the counter")?;
        nodes[li]
            .write(Op::Put {
                key: KEY.to_owned(),
                value: b"0".to_vec(),
                expected: Precondition::Absent,
            })
            .await
            .map_err(|e| format!("seed write failed: {e:?}"))?;
    }

    // Concurrent CAS-increment clients.
    let mut client_handles = Vec::with_capacity(clients);
    for _ in 0..clients {
        let nodes_c: Vec<Arc<RaftNode>> = nodes.clone();
        client_handles.push(tokio::spawn(async move {
            let mut acks = 0u64;
            for _ in 0..attempts {
                let Some(li) = confirmed_leader(&nodes_c).await else {
                    tokio::time::sleep(Duration::from_millis(5)).await;
                    continue;
                };
                let leader = &nodes_c[li];
                let Ok(Some(cur)) = leader.get(KEY).await else {
                    continue;
                };
                let Some(n) = parse_counter(&cur.value) else {
                    continue;
                };
                let resp = leader
                    .write(Op::Put {
                        key: KEY.to_owned(),
                        value: (n + 1).to_string().into_bytes(),
                        expected: Precondition::Revision(cur.mod_revision),
                    })
                    .await;
                if matches!(resp, Ok(RaftResponse::Applied { .. })) {
                    acks += 1;
                }
            }
            acks
        }));
    }

    // A fault task isolates then heals a single node repeatedly (never a majority),
    // forcing real leader failovers while the clients race.
    let fault_cluster = Arc::clone(&cluster);
    let fault = tokio::spawn(async move {
        let mut prng = SplitMix64::new(seed);
        for _ in 0..10 {
            let victim = prng.below(3) + 1;
            fault_cluster.isolate(victim);
            tokio::time::sleep(Duration::from_millis(40)).await;
            fault_cluster.heal(victim);
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    });

    // Collect acknowledged increments and let the faults finish.
    let mut total_acks = 0u64;
    for h in client_handles {
        total_acks += h
            .await
            .map_err(|e| format!("client task panicked: {e:?}"))?;
    }
    fault
        .await
        .map_err(|e| format!("fault task panicked: {e:?}"))?;

    // Heal, let a stable leader form, and read the final committed counter.
    cluster.heal_all();
    let start = Instant::now();
    let final_n = loop {
        if let Some(i) = confirmed_leader(&nodes).await {
            match nodes[i].get(KEY).await {
                Ok(Some(v)) => {
                    break parse_counter(&v.value).ok_or("counter is not a number")?;
                }
                Ok(None) => return Err("counter key vanished".to_owned()),
                Err(e) => return Err(format!("final read failed: {e:?}")),
            }
        }
        if start.elapsed() >= Duration::from_secs(10) {
            return Err("no confirmed leader after healing".to_owned());
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    };

    if total_acks > final_n {
        return Err(format!(
            "linearizability violation: {total_acks} acknowledged increments exceed the final counter {final_n} — a lost update or a stale allow occurred"
        ));
    }
    let ambiguous = final_n - total_acks;
    Ok(format!(
        "{clients} concurrent CAS-increment clients under injected partitions and real leader failovers: {total_acks} acknowledged increments ≤ final counter {final_n} — no lost update, no stale allow ({ambiguous} benign ambiguous in-flight commits)"
    ))
}

// ---------------------------------------------------------------------------
// Live-estate substrate shared by V5 / V7 / V8 / V10: a real, in-process,
// multi-voter openraft cluster (the analog of the containerized 5-CP estate)
// plus the signed-revocation kill switch every control-plane replica polls.
// ---------------------------------------------------------------------------

/// Spawn an `n`-voter in-process Raft cluster over one shared [`Cluster`]
/// transport: each node runs real openraft (election, replication, commit), so a
/// gate exercises the same consensus the live 5-CP estate does, in one process.
/// Returns the transport handle (for fault injection) and the nodes, index 0 ==
/// node 1. The caller drives writes through a [`confirmed_leader`].
async fn spawn_cluster(n: u64, label: &str) -> Result<(Arc<Cluster>, Vec<Arc<RaftNode>>), String> {
    let dir = scratch(label);
    let cluster = Arc::new(Cluster::new());
    let mut nodes: Vec<Arc<RaftNode>> = Vec::with_capacity(usize::try_from(n).unwrap_or(0));
    for id in 1..=n {
        nodes.push(Arc::new(
            RaftNode::join(id, dir.join(format!("n{id}")), &cluster)
                .await
                .map_err(|e| format!("node {id} join failed: {e:?}"))?,
        ));
    }
    nodes[0]
        .initialize(BTreeSet::from([1]))
        .await
        .map_err(|e| format!("initialize failed: {e:?}"))?;
    nodes[0]
        .wait_for_leader(Duration::from_secs(10))
        .await
        .map_err(|e| format!("no leader: {e:?}"))?;
    for id in 2..=n {
        nodes[0]
            .add_learner(id)
            .await
            .map_err(|e| format!("add learner {id}: {e:?}"))?;
    }
    nodes[0]
        .change_membership((1..=n).collect())
        .await
        .map_err(|e| format!("change membership: {e:?}"))?;
    Ok((cluster, nodes))
}

/// The KV key every gateway/control-plane replica's kill switch polls for the
/// signed estate revocation snapshot (the R9 online path, on the Raft store).
const REV_PATH: &str = "wsf/revocation/estate";

/// A replica's kill-switch verdict on a presented token. Fail-closed (doctrine
/// I-9): a missing, unverifiable, or stale snapshot denies rather than guesses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum KillDecision {
    Allow,
    DenyRevoked,
    DenyFailClosed,
}

/// Publish a signed revocation snapshot (revoking `tokens`) through the confirmed
/// leader, so Raft replicates it to every replica's kill switch. `issued`/
/// `expires` set the freshness window the replicas honor (I-9).
async fn publish_revocation(
    nodes: &[Arc<RaftNode>],
    signer: &dyn Signer,
    tokens: &[&str],
    issued: DateTime<Utc>,
    expires: DateTime<Utc>,
) -> Result<(), String> {
    let mut snapshot = RevocationSnapshot::new(
        "loom-estate-revocation",
        issued.to_rfc3339(),
        expires.to_rfc3339(),
    );
    snapshot.revoked_tokens = tokens.iter().map(|t| (*t).to_owned()).collect();
    let sealed = sign_snapshot(snapshot, signer).map_err(|e| format!("sign snapshot: {e}"))?;
    let bytes = serde_json::to_vec(&sealed).map_err(|e| format!("encode snapshot: {e}"))?;
    let li = confirmed_leader(nodes)
        .await
        .ok_or("no confirmed leader to publish the revocation")?;
    nodes[li]
        .write(Op::Put {
            key: REV_PATH.to_owned(),
            value: bytes,
            expected: Precondition::Any,
        })
        .await
        .map_err(|e| format!("publish write failed: {e:?}"))?;
    Ok(())
}

/// Model one replica's kill switch reading its OWN committed state: pull the
/// snapshot from `REV_PATH`, verify it under the estate `anchor_pk`, honor its
/// freshness window at `now`, then check `token`. Every failure path denies.
async fn kill_switch(
    node: &RaftNode,
    anchor_pk: &[u8],
    token: &str,
    now: DateTime<Utc>,
) -> Result<KillDecision, String> {
    let Some(v) = node
        .get(REV_PATH)
        .await
        .map_err(|e| format!("kill-switch read failed: {e:?}"))?
    else {
        // No snapshot on this replica: it cannot prove the token is *not* revoked.
        return Ok(KillDecision::DenyFailClosed);
    };
    let snapshot: RevocationSnapshot =
        serde_json::from_slice(&v.value).map_err(|e| format!("snapshot decode failed: {e}"))?;
    if verify_snapshot(&snapshot, &MlDsa87Verifier, anchor_pk).is_err() {
        return Ok(KillDecision::DenyFailClosed); // tampered / wrong anchor
    }
    match DateTime::parse_from_rfc3339(&snapshot.expires_at) {
        Ok(exp) if now <= exp.with_timezone(&Utc) => {}
        _ => return Ok(KillDecision::DenyFailClosed), // stale (I-9)
    }
    if snapshot.is_token_revoked(token) {
        Ok(KillDecision::DenyRevoked)
    } else {
        Ok(KillDecision::Allow)
    }
}

/// Wait (bounded) until every replica's committed state carries a snapshot at
/// `REV_PATH` — the revocation has fanned out to the whole estate.
async fn await_replicated(nodes: &[Arc<RaftNode>], timeout: Duration) -> Result<(), String> {
    let deadline = Instant::now() + timeout;
    loop {
        let mut present = 0usize;
        for node in nodes {
            let got = node
                .get(REV_PATH)
                .await
                .map_err(|e| format!("replication read failed: {e:?}"))?;
            present += usize::from(got.is_some());
        }
        if present == nodes.len() {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err("revocation snapshot did not replicate to every replica".to_owned());
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// Bar 7 (V5) — kill-switch-under-scale. Under estate scale (a store populated
/// with `workloads` objects), a published revocation halts the next call on
/// **every** replica: each of the `replicas` control-plane nodes, reading its own
/// Raft-replicated committed state, denies the revoked token and still admits a
/// live one — no replica misses the kill, and a snapshot that does not verify
/// under the estate anchor fails closed (doctrine I-9). Real openraft replication
/// + a real signed `fabric-revocation` snapshot under a real ML-DSA-87 anchor.
pub async fn kill_switch_under_scale(replicas: u64, workloads: usize) -> Result<String, String> {
    const REVOKED: &str = "tok-compromised";
    const LIVE: &str = "tok-healthy";

    let (_cluster, nodes) = spawn_cluster(replicas, "loom-conformance-killswitch").await?;
    let anchor = RustCryptoMlDsa87::generate("loom-estate-anchor")
        .map_err(|e| format!("anchor keygen failed: {e}"))?;
    let anchor_pk = anchor.public_key().to_vec();

    // Scale: populate the estate with `workloads` objects — the load the kill
    // switch must stay correct under (bar 7 is kill-switch-*under-scale*).
    {
        let li = confirmed_leader(&nodes)
            .await
            .ok_or("no confirmed leader to load the estate")?;
        for i in 0..workloads {
            put(&nodes[li], &format!("Workload/scale-{i:05}"), "spec").await?;
        }
    }

    // Publish the kill through the leader; wait for it to reach every replica.
    let now = Utc::now();
    let expires = now + ChronoDuration::days(3650);
    publish_revocation(&nodes, &anchor, &[REVOKED], now, expires).await?;
    await_replicated(&nodes, Duration::from_secs(10)).await?;

    // Every replica must land the kill: revoked denied, live still admitted.
    for (i, node) in nodes.iter().enumerate() {
        let id = i as u64 + 1;
        match kill_switch(node, &anchor_pk, REVOKED, now).await? {
            KillDecision::DenyRevoked => {}
            other => {
                return Err(format!(
                    "replica {id} missed the kill: the revoked token resolved to {other:?} (want DenyRevoked) — a replica served an authoritative allow"
                ));
            }
        }
        match kill_switch(node, &anchor_pk, LIVE, now).await? {
            KillDecision::Allow => {}
            other => {
                return Err(format!(
                    "replica {id} wrongly denied a live token → {other:?} (want Allow) — the kill switch is not a precise filter"
                ));
            }
        }
    }

    // Adversarial: a snapshot signed by a rogue anchor must fail closed on every
    // replica — the kill switch never trusts an unverified snapshot.
    let rogue = RustCryptoMlDsa87::generate("rogue-anchor").map_err(|e| format!("{e}"))?;
    publish_revocation(&nodes, &rogue, &[REVOKED], now, expires).await?;
    tokio::time::sleep(Duration::from_millis(100)).await;
    for (i, node) in nodes.iter().enumerate() {
        // Re-read may briefly lag; poll this replica until it reflects the rogue
        // snapshot, then assert it fails closed under the real anchor.
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if kill_switch(node, &anchor_pk, LIVE, now).await? == KillDecision::DenyFailClosed {
                break;
            }
            if Instant::now() >= deadline {
                return Err(format!(
                    "replica {} trusted a rogue-signed snapshot (did not fail closed)",
                    i as u64 + 1
                ));
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    Ok(format!(
        "under {workloads} estate objects, a signed revocation replicated to all {replicas} replicas; every replica denied the revoked token and admitted a live one, and a rogue-signed snapshot failed closed on all — no replica missed the kill"
    ))
}
