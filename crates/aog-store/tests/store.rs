//! K2 gate: deterministic apply from a fixed op log; CAS rejects stale writes;
//! the redb backend persists across reopen.

use aog_store::{Applied, MemBackend, Op, Precondition, RedbBackend, Store, StoreError};

fn put(key: &str, val: &str, expected: Precondition) -> Op {
    Op::Put {
        key: key.to_owned(),
        value: val.as_bytes().to_vec(),
        expected,
    }
}

fn del(key: &str, expected: Precondition) -> Op {
    Op::Delete {
        key: key.to_owned(),
        expected,
    }
}

#[test]
fn deterministic_apply_from_fixed_oplog() {
    let ops = vec![
        put("tenant/acme", "v1", Precondition::Absent),
        put("tenant/beta", "v1", Precondition::Absent),
        put("tenant/acme", "v2", Precondition::Any),
        del("tenant/beta", Precondition::Any),
    ];

    let mut a = Store::open(MemBackend::new()).unwrap();
    let mut b = Store::open(MemBackend::new()).unwrap();
    let results_a = a.apply_all(&ops).unwrap();
    let results_b = b.apply_all(&ops).unwrap();

    assert_eq!(results_a, results_b, "same op log -> same results");
    assert_eq!(a.revision(), b.revision(), "same op log -> same revision");
    assert_eq!(
        a.range("").unwrap(),
        b.range("").unwrap(),
        "same op log -> identical state"
    );

    // acme survives with v2, updated once; beta was deleted.
    let acme = a.get("tenant/acme").unwrap().unwrap();
    assert_eq!(acme.value, b"v2".to_vec());
    assert_eq!(acme.version, 2);
    assert_eq!(acme.create_revision, 1);
    assert_eq!(acme.mod_revision, 3);
    assert!(a.get("tenant/beta").unwrap().is_none());
    assert_eq!(a.revision(), 4, "four successful mutations");
}

#[test]
fn cas_rejects_stale_write() {
    let mut s = Store::open(MemBackend::new()).unwrap();

    let created = s.apply(&put("k", "a", Precondition::Absent)).unwrap();
    let Applied::Put { revision, created } = created else {
        panic!("expected a Put result");
    };
    assert!(created);

    // Correct expected revision advances the key.
    s.apply(&put("k", "b", Precondition::Revision(revision)))
        .unwrap();

    // The now-stale revision is rejected.
    let err = s
        .apply(&put("k", "c", Precondition::Revision(revision)))
        .unwrap_err();
    assert!(
        matches!(err, StoreError::StaleRevision { .. }),
        "stale revision must be rejected, got {err:?}"
    );

    // Absent precondition on an existing key is rejected too.
    let err = s.apply(&put("k", "d", Precondition::Absent)).unwrap_err();
    assert!(matches!(err, StoreError::Exists { .. }));

    // Deleting an absent key is a miss.
    let err = s.apply(&del("missing", Precondition::Any)).unwrap_err();
    assert!(matches!(err, StoreError::NotFound { .. }));
}

#[test]
fn redb_persists_across_reopen() {
    let path = std::env::temp_dir().join("aog-store-k2-durability.redb");
    let _ = std::fs::remove_file(&path);

    {
        let mut s = Store::open(RedbBackend::open(&path).unwrap()).unwrap();
        s.apply(&put("tenant/acme", "v1", Precondition::Absent))
            .unwrap();
        s.apply(&put("tenant/acme", "v2", Precondition::Any))
            .unwrap();
        assert_eq!(s.revision(), 2);
    }

    // Reopen: the revision and value must be recovered from disk.
    let s = Store::open(RedbBackend::open(&path).unwrap()).unwrap();
    assert_eq!(s.revision(), 2, "revision recovered from stored state");
    let v = s.get("tenant/acme").unwrap().unwrap();
    assert_eq!(v.value, b"v2".to_vec());
    assert_eq!(v.mod_revision, 2);
    assert_eq!(v.version, 2);

    let _ = std::fs::remove_file(&path);
}
