//! "duplicate/dropped events converge identically (replay test)".
//!
//! Three controllers watch the same key prefix through three delivery
//! histories: clean, duplicated (every key force-enqueued repeatedly), and
//! dropped (the 64-slot watch buffer genuinely overflowed, so recovery runs
//! the production lag re-list path — not a simulation). All three must record
//! byte-identical end states, equal to the store's authoritative state.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::future::Future;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use aog_controller::{
    Action, AlwaysLeader, Backoff, Controller, ReconcileError, Reconciler, SharedGate,
};
use aog_store::raft::RaftNode;
use aog_store::{Op, Precondition};

fn fresh_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(name);
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

async fn estate(name: &str) -> Arc<RaftNode> {
    Arc::new(RaftNode::bootstrap(1, fresh_dir(name)).await.unwrap())
}

async fn put(node: &RaftNode, key: &str, value: &str) {
    node.write(Op::Put {
        key: key.to_owned(),
        value: value.as_bytes().to_vec(),
        expected: Precondition::Any,
    })
    .await
    .unwrap();
}

async fn del(node: &RaftNode, key: &str) {
    node.write(Op::Delete {
        key: key.to_owned(),
        expected: Precondition::Any,
    })
    .await
    .unwrap();
}

/// Run sync passes until one does nothing and the queue is empty (bounded).
async fn settle<R: Reconciler>(controller: &mut Controller<R>) {
    for _ in 0..50 {
        let stats = controller.sync(Instant::now()).await.unwrap();
        if stats.enqueued == 0
            && stats.drained == 0
            && stats.processed == 0
            && controller.queue_len() == 0
        {
            return;
        }
    }
    panic!("controller did not settle within 50 sync passes");
}

/// A level-triggered reconciler that records the *current* store state for
/// each key it is woken for — the observable end state the gate compares.
#[derive(Clone)]
struct Recorder {
    node: Arc<RaftNode>,
    state: Arc<Mutex<BTreeMap<String, Option<String>>>>,
    calls: Arc<Mutex<u32>>,
}

impl Recorder {
    fn new(node: Arc<RaftNode>) -> Self {
        Self {
            node,
            state: Arc::new(Mutex::new(BTreeMap::new())),
            calls: Arc::new(Mutex::new(0)),
        }
    }

    fn recorded(&self) -> BTreeMap<String, Option<String>> {
        self.state.lock().unwrap().clone()
    }

    fn calls(&self) -> u32 {
        *self.calls.lock().unwrap()
    }
}

impl Reconciler for Recorder {
    fn reconcile(&self, key: &str) -> impl Future<Output = Result<Action, ReconcileError>> + Send {
        let node = Arc::clone(&self.node);
        let state = Arc::clone(&self.state);
        let calls = Arc::clone(&self.calls);
        let key = key.to_owned();
        async move {
            let current = node
                .get(&key)
                .await
                .map_err(|e| ReconcileError(e.to_string()))?;
            let value = current.map(|v| String::from_utf8_lossy(&v.value).into_owned());
            state.lock().unwrap().insert(key, value);
            *calls.lock().unwrap() += 1;
            Ok(Action::Done)
        }
    }
}

enum Mode {
    Clean,
    Duplicated,
    Dropped,
}

/// One full scenario: initial estate observed, then the same late history
/// delivered under `mode`. Returns the recorder's end-state map.
async fn scenario(dir: &str, mode: Mode) -> BTreeMap<String, Option<String>> {
    let node = estate(dir).await;
    put(&node, "Tenant/a", "v1").await;
    put(&node, "Tenant/b", "v1").await;
    put(&node, "Tenant/c", "v1").await;

    let recorder = Recorder::new(Arc::clone(&node));
    let mut controller = Controller::new(
        "replay",
        node.informer("Tenant/"),
        recorder.clone(),
        Arc::new(AlwaysLeader),
    );
    settle(&mut controller).await;

    // The same late history in every scenario.
    put(&node, "Tenant/a", "v2").await;
    del(&node, "Tenant/b").await;
    put(&node, "Tenant/d", "v1").await;

    match mode {
        Mode::Clean => {}
        Mode::Duplicated => {
            // Deliver every key several extra times on top of the real events.
            for _ in 0..3 {
                for key in ["Tenant/a", "Tenant/b", "Tenant/c", "Tenant/d"] {
                    controller.enqueue(key);
                }
            }
        }
        Mode::Dropped => {
            // Overflow the 64-slot watch buffer so the tenant events above are
            // genuinely evicted before this controller polls again — its next
            // poll() hits Lagged and takes the production re-list path.
            for i in 0..80 {
                put(&node, &format!("Noise/{i:03}"), "x").await;
            }
        }
    }
    settle(&mut controller).await;

    assert!(recorder.calls() > 0);
    recorder.recorded()
}

#[tokio::test]
async fn duplicate_and_dropped_events_converge_identically() {
    let clean = scenario("loom-r1-clean", Mode::Clean).await;
    let duplicated = scenario("loom-r1-dup", Mode::Duplicated).await;
    let dropped = scenario("loom-r1-drop", Mode::Dropped).await;

    // The invariant: three delivery histories, one end state.
    assert_eq!(clean, duplicated, "duplicated events diverged");
    assert_eq!(clean, dropped, "dropped events diverged");

    // And that end state is the store's authoritative final state.
    let want: BTreeMap<String, Option<String>> = [
        ("Tenant/a".to_owned(), Some("v2".to_owned())),
        ("Tenant/b".to_owned(), None),
        ("Tenant/c".to_owned(), Some("v1".to_owned())),
        ("Tenant/d".to_owned(), Some("v1".to_owned())),
    ]
    .into_iter()
    .collect();
    assert_eq!(clean, want);
}

/// A reconciler that fails once per key, then records like [`Recorder`].
#[derive(Clone)]
struct FailOnce {
    inner: Recorder,
    pending_failures: Arc<Mutex<HashSet<String>>>,
}

impl Reconciler for FailOnce {
    fn reconcile(&self, key: &str) -> impl Future<Output = Result<Action, ReconcileError>> + Send {
        let inner = self.inner.clone();
        let pending = Arc::clone(&self.pending_failures);
        let key = key.to_owned();
        async move {
            if pending.lock().unwrap().remove(&key) {
                return Err(ReconcileError(format!("injected failure for {key}")));
            }
            inner.reconcile(&key).await
        }
    }
}

#[tokio::test]
async fn failed_reconciles_retry_with_backoff_and_converge() {
    let node = estate("loom-r1-flaky").await;
    put(&node, "Tenant/a", "v1").await;
    put(&node, "Tenant/b", "v1").await;

    let recorder = Recorder::new(Arc::clone(&node));
    let flaky = FailOnce {
        inner: recorder.clone(),
        pending_failures: Arc::new(Mutex::new(
            ["Tenant/a".to_owned(), "Tenant/b".to_owned()].into(),
        )),
    };
    let mut controller = Controller::new(
        "flaky",
        node.informer("Tenant/"),
        flaky,
        Arc::new(AlwaysLeader),
    )
    .with_backoff(Backoff {
        base: Duration::from_millis(1),
        max: Duration::from_millis(20),
    });

    let stats = controller.sync(Instant::now()).await.unwrap();
    assert_eq!(stats.failed, 2, "both keys fail their first reconcile");
    assert!(recorder.recorded().is_empty());

    // Past the backoff window the retries come due and converge.
    tokio::time::sleep(Duration::from_millis(20)).await;
    settle(&mut controller).await;

    let want: BTreeMap<String, Option<String>> = [
        ("Tenant/a".to_owned(), Some("v1".to_owned())),
        ("Tenant/b".to_owned(), Some("v1".to_owned())),
    ]
    .into_iter()
    .collect();
    assert_eq!(recorder.recorded(), want);
}

#[tokio::test]
async fn non_leader_observes_but_never_acts() {
    let node = estate("loom-r1-leader").await;
    put(&node, "Tenant/a", "v1").await;
    put(&node, "Tenant/b", "v1").await;

    let recorder = Recorder::new(Arc::clone(&node));
    let gate = SharedGate::new(false);
    let mut controller = Controller::new(
        "gated",
        node.informer("Tenant/"),
        recorder.clone(),
        Arc::clone(&gate) as Arc<dyn aog_controller::LeaderGate>,
    );

    let stats = controller.sync(Instant::now()).await.unwrap();
    assert!(!stats.leader);
    assert_eq!(stats.enqueued, 2, "a non-leader still observes");
    assert_eq!(stats.processed, 0, "a non-leader never reconciles");
    assert!(recorder.recorded().is_empty());
    assert_eq!(controller.queue_len(), 2, "work accumulates for takeover");

    // Takeover: the accumulated queue is reconciled.
    gate.set(true);
    settle(&mut controller).await;
    let want: BTreeMap<String, Option<String>> = [
        ("Tenant/a".to_owned(), Some("v1".to_owned())),
        ("Tenant/b".to_owned(), Some("v1".to_owned())),
    ]
    .into_iter()
    .collect();
    assert_eq!(recorder.recorded(), want);
}

/// A reconciler that asks to run again once per key — `Requeue` for one key,
/// `RequeueAfter` for the other — then completes.
#[derive(Clone)]
struct RunTwice {
    calls: Arc<Mutex<HashMap<String, u32>>>,
}

impl Reconciler for RunTwice {
    fn reconcile(&self, key: &str) -> impl Future<Output = Result<Action, ReconcileError>> + Send {
        let calls = Arc::clone(&self.calls);
        let key = key.to_owned();
        async move {
            let mut calls = calls.lock().unwrap();
            let n = calls.entry(key.clone()).or_insert(0);
            *n += 1;
            if *n > 1 {
                return Ok(Action::Done);
            }
            if key.ends_with("delayed") {
                Ok(Action::RequeueAfter(Duration::from_millis(1)))
            } else {
                Ok(Action::Requeue)
            }
        }
    }
}

#[tokio::test]
async fn requeue_actions_run_the_key_again() {
    let node = estate("loom-r1-requeue").await;
    put(&node, "Tenant/immediate", "v1").await;
    put(&node, "Tenant/delayed", "v1").await;

    let reconciler = RunTwice {
        calls: Arc::new(Mutex::new(HashMap::new())),
    };
    let calls = Arc::clone(&reconciler.calls);
    let mut controller = Controller::new(
        "requeue",
        node.informer("Tenant/"),
        reconciler,
        Arc::new(AlwaysLeader),
    );

    settle(&mut controller).await;
    tokio::time::sleep(Duration::from_millis(10)).await;
    settle(&mut controller).await;

    let calls = calls.lock().unwrap().clone();
    assert_eq!(calls.get("Tenant/immediate"), Some(&2));
    assert_eq!(calls.get("Tenant/delayed"), Some(&2));
}

#[tokio::test]
async fn resync_heartbeat_reconciles_without_a_change() {
    let node = estate("loom-r1-resync").await;
    put(&node, "Tenant/a", "v1").await;

    let recorder = Recorder::new(Arc::clone(&node));
    let mut controller = Controller::new(
        "resync",
        node.informer("Tenant/"),
        recorder.clone(),
        Arc::new(AlwaysLeader),
    )
    .with_resync(Duration::from_millis(1));

    let first = controller.sync(Instant::now()).await.unwrap();
    assert!(first.processed > 0);
    let after_first = recorder.calls();
    assert!(after_first > 0);

    // No store change at all — the heartbeat alone re-reconciles the key
    // (what drives time-based work like HMAC-rotation windows).
    tokio::time::sleep(Duration::from_millis(5)).await;
    let stats = controller.sync(Instant::now()).await.unwrap();
    assert!(stats.processed > 0, "heartbeat re-enqueued known keys");
    assert!(recorder.calls() > after_first);
}
