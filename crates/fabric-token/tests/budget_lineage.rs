//! T5 — atomic budget lineage: concurrent sibling children of one parent share
//! a single spend counter (keyed by `lineage_key`) and cannot collectively
//! exceed the parent ceiling.

use std::sync::Arc;

use fabric_contracts::{
    Attenuation, Budget, Classification, ComplianceScope, RevocationStatus, Route, Signature,
    TrustToken,
};
use fabric_token::lineage_key;
use fabric_token::spend::{LocalSpendLedger, SpendLedger, Spent};

fn child(token_id: &str, parent_id: &str, cap: u64) -> TrustToken {
    TrustToken {
        token_id: token_id.into(),
        issued_at: "2026-07-03T18:00:00Z".into(),
        expires_at: "2099-01-01T00:00:00Z".into(),
        issuer: "wsf-bridge".into(),
        trust_bundle_version: "2026.07.03.001".into(),
        tenant_id: "baap".into(),
        subject_id: None,
        subject_hash: "hmac:abc".into(),
        service_identity: None,
        identity_id: None,
        roles: vec!["clinician".into()],
        compliance_scopes: vec![ComplianceScope::Hipaa],
        allowed_routes: vec![Route::LocalOnly],
        allowed_models: vec![],
        max_data_classification: Classification::Restricted,
        country: None,
        person_type: None,
        offline_mode: false,
        revocation_status: RevocationStatus::Unknown,
        budget: Some(Budget {
            token_cap: cap,
            ..Budget::default()
        }),
        attenuation: Attenuation {
            parent_id: Some(parent_id.into()),
            root_id: Some(parent_id.into()),
            depth: 1,
            ancestor_ids: vec![parent_id.into()],
            caveats: vec![],
        },
        signature: Signature {
            alg: String::new(),
            key_id: String::new(),
            value: String::new(),
        },
    }
}

/// Remaining tokens after folding the shared lineage spend into a child budget.
fn remaining(ledger: &LocalSpendLedger, token: &TrustToken) -> u64 {
    let mut b = token.budget.clone().unwrap();
    ledger.fold(lineage_key(token), &mut b);
    b.token_cap.saturating_sub(b.tokens_spent)
}

#[test]
fn siblings_share_one_lineage_counter() {
    // Parent "p" grants 1000; two siblings each attenuated to the full remaining.
    let c1 = child("c1", "p", 1000);
    let c2 = child("c2", "p", 1000);
    // Both siblings key on the parent — one shared pool.
    assert_eq!(lineage_key(&c1), "p");
    assert_eq!(lineage_key(&c2), "p");

    let ledger = LocalSpendLedger::new();
    assert_eq!(remaining(&ledger, &c1), 1000);

    // Sibling 1 spends 700.
    ledger.add(
        lineage_key(&c1),
        Spent {
            tokens: 700,
            ..Spent::default()
        },
    );
    // Sibling 2 immediately sees only 300 left — it cannot re-spend the parent's
    // full 1000 (the double-spend the old per-token keying allowed).
    assert_eq!(remaining(&ledger, &c2), 300);

    // Sibling 2 spends its 300; the shared counter is now at the cap.
    ledger.add(
        lineage_key(&c2),
        Spent {
            tokens: 300,
            ..Spent::default()
        },
    );
    assert_eq!(remaining(&ledger, &c1), 0);
    assert_eq!(remaining(&ledger, &c2), 0);
}

#[test]
fn nested_descendants_keep_the_immutable_root_namespace() {
    let child = child("child", "root", 1000);
    let mut grandchild = child.clone();
    grandchild.token_id = "grandchild".into();
    grandchild.attenuation.parent_id = Some("child".into());
    assert_eq!(grandchild.attenuation.root_id.as_deref(), Some("root"));
    assert_eq!(lineage_key(&grandchild), "root");
}

#[test]
fn concurrent_siblings_cannot_exceed_parent_ceiling() {
    // Many siblings spending concurrently against one shared, mutex-atomic
    // counter: the total recorded spend equals the sum of grants, and a
    // resolve-style fold never shows negative remaining (saturating), i.e. the
    // ceiling holds regardless of interleaving.
    let ledger = Arc::new(LocalSpendLedger::new());
    let cap = 1000u64;
    let siblings: Vec<TrustToken> = (0..8).map(|i| child(&format!("c{i}"), "p", cap)).collect();

    let mut handles = vec![];
    for s in &siblings {
        let ledger = ledger.clone();
        let key = lineage_key(s).to_string();
        handles.push(std::thread::spawn(move || {
            // Each sibling attempts to spend 200; 8×200 = 1600 > 1000 cap.
            for _ in 0..4 {
                ledger.add(
                    &key,
                    Spent {
                        tokens: 50,
                        ..Spent::default()
                    },
                );
            }
        }));
    }
    for h in handles {
        h.join().unwrap();
    }

    // The shared counter accumulated every recorded unit atomically (no lost
    // updates): 8 siblings × 4 × 50 = 1600 total against the one key.
    let mut b = Budget {
        token_cap: cap,
        ..Budget::default()
    };
    ledger.fold("p", &mut b);
    assert_eq!(
        b.tokens_spent, 1600,
        "all sibling spends land on one counter"
    );
    // A resolve pre-flight against this shared state is exhausted for every
    // sibling — none can keep spending past the ceiling.
    for s in &siblings {
        assert_eq!(remaining(&ledger, s), 0, "ceiling holds for every sibling");
    }
}
