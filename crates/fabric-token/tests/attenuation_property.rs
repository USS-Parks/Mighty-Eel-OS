//! K4 (audit H1): attenuation-monotonicity property suite. A deterministic,
//! seeded generator produces a genuine *widening* on every narrowable axis
//! (expiry, routes, models incl. the empty-set case, roles, compliance scopes,
//! classification, budget) and asserts each is refused; a companion control
//! proves a valid narrowing on each axis still succeeds, so the rejection is not
//! vacuous. Seeded (`StdRng::seed_from_u64`) for reproducibility — no flakes.

use chrono::{TimeZone, Utc};
use fabric_contracts::{
    Attenuation, Budget, Classification, ComplianceScope, RevocationStatus, Route, Signature,
    TrustToken,
};
use fabric_crypto::providers::RustCryptoMlDsa87;
use fabric_token::{TokenError, TokenRestrictions, attenuate_preverified};
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};

/// A parent restricted on every axis, so each has both a wider and a narrower
/// child. Budget remaining: tokens 900, usd 450, tool-calls 8.
fn restricted_parent() -> TrustToken {
    TrustToken {
        token_id: "parent".into(),
        issued_at: "2026-07-03T00:00:00Z".into(),
        expires_at: "2030-01-01T00:00:00Z".into(),
        issuer: "wsf-bridge".into(),
        trust_bundle_version: "2026.07.v2".into(),
        tenant_id: "baap".into(),
        subject_id: None,
        subject_hash: "hmac:abc".into(),
        service_identity: None,
        identity_id: None,
        roles: vec!["r1".into(), "r2".into()],
        compliance_scopes: vec![ComplianceScope::Hipaa],
        allowed_routes: vec![Route::LocalOnly],
        allowed_models: vec!["m1".into(), "m2".into()],
        max_data_classification: Classification::Restricted,
        country: None,
        person_type: None,
        offline_mode: false,
        revocation_status: RevocationStatus::Unknown,
        budget: Some(Budget {
            token_cap: 1000,
            tokens_spent: 100,
            usd_cap_cents: 500,
            usd_spent_cents: 50,
            tool_call_cap: 10,
            tool_calls_spent: 2,
        }),
        attenuation: Attenuation::default(),
        signature: Signature {
            alg: String::new(),
            key_id: String::new(),
            value: String::new(),
        },
    }
}

const AXES: usize = 7;

/// A named narrowing case for the control test: axis label + a mutation applying
/// a valid (narrowing) restriction on that axis.
type Narrowing = (&'static str, fn(&mut TokenRestrictions));

#[test]
fn randomized_widening_on_every_axis_is_rejected() {
    let signer = RustCryptoMlDsa87::generate("prop").unwrap();
    let parent = restricted_parent();
    let now = Utc.with_ymd_and_hms(2026, 7, 4, 0, 0, 0).unwrap();
    let mut rng = StdRng::seed_from_u64(0x5EED_A11E);

    let mut hit = [0u32; AXES];
    for i in 0..1000 {
        let axis = i % AXES;
        let mut r = TokenRestrictions::new(format!("child-{i}"));
        let expected: &str = match axis {
            0 => {
                // Later than the parent's 2030 expiry.
                r.expires_at = Some(format!("203{}-01-01T00:00:00Z", rng.gen_range(1..9)));
                "expires_at"
            }
            1 => {
                // A route the parent does not grant (superset of [LocalOnly]).
                let extra = if rng.gen_bool(0.5) {
                    Route::LocalPreferred
                } else {
                    Route::CloudAllowed
                };
                r.allowed_routes = Some(vec![Route::LocalOnly, extra]);
                "allowed_routes"
            }
            2 => {
                // Empty child (the H1 "all models" sentinel) or a model not in the parent.
                r.allowed_models = if rng.gen_bool(0.5) {
                    Some(vec![])
                } else {
                    Some(vec![format!("m{}", rng.gen_range(3..99))])
                };
                "allowed_models"
            }
            3 => {
                r.roles = Some(vec![format!("r{}", rng.gen_range(3..99))]);
                "roles"
            }
            4 => {
                r.compliance_scopes = Some(vec![if rng.gen_bool(0.5) {
                    ComplianceScope::ItarEar
                } else {
                    ComplianceScope::Ocap
                }]);
                "compliance_scopes"
            }
            5 => {
                // Above the parent's Restricted ceiling.
                r.max_data_classification = Some(if rng.gen_bool(0.5) {
                    Classification::Controlled
                } else {
                    Classification::Secret
                });
                "max_data_classification"
            }
            _ => {
                // Exactly one cap over the parent's remaining (900 / 450 / 8).
                r.budget = Some(match rng.gen_range(0..3) {
                    0 => Budget {
                        token_cap: 900 + rng.gen_range(1..1000),
                        ..Budget::default()
                    },
                    1 => Budget {
                        usd_cap_cents: 450 + rng.gen_range(1..1000),
                        ..Budget::default()
                    },
                    _ => Budget {
                        tool_call_cap: 8 + rng.gen_range(1..100),
                        ..Budget::default()
                    },
                });
                "budget"
            }
        };
        let err = attenuate_preverified(&parent, &r, now, None, &signer).unwrap_err();
        assert_eq!(
            err,
            TokenError::AttenuationWidens { axis: expected },
            "iteration {i}: widening on {expected} was not rejected as such"
        );
        hit[axis] += 1;
    }
    // Every axis was exercised (no dead generator arm).
    assert!(
        hit.iter().all(|&c| c > 100),
        "under-exercised axis: {hit:?}"
    );
}

#[test]
fn valid_narrowing_on_every_axis_succeeds() {
    // The control: a genuine narrowing on each axis is accepted, so the rejection
    // suite above is not passing vacuously.
    let signer = RustCryptoMlDsa87::generate("prop").unwrap();
    let parent = restricted_parent();
    let now = Utc.with_ymd_and_hms(2026, 7, 4, 0, 0, 0).unwrap();

    let narrowings: Vec<Narrowing> = vec![
        ("expires_at", |r| {
            r.expires_at = Some("2029-01-01T00:00:00Z".into());
        }),
        ("allowed_routes", |r| r.allowed_routes = Some(vec![])),
        ("allowed_models", |r| {
            r.allowed_models = Some(vec!["m1".into()]);
        }),
        ("roles", |r| r.roles = Some(vec!["r1".into()])),
        ("compliance_scopes", |r| r.compliance_scopes = Some(vec![])),
        ("max_data_classification", |r| {
            r.max_data_classification = Some(Classification::Internal);
        }),
        ("budget", |r| {
            r.budget = Some(Budget {
                token_cap: 10,
                usd_cap_cents: 10,
                tool_call_cap: 1,
                ..Budget::default()
            });
        }),
    ];

    for (axis, apply) in narrowings {
        let mut r = TokenRestrictions::new(format!("child-{axis}"));
        apply(&mut r);
        let child = attenuate_preverified(&parent, &r, now, None, &signer)
            .unwrap_or_else(|e| panic!("valid narrowing on {axis} was rejected: {e:?}"));
        assert_eq!(child.attenuation.parent_id.as_deref(), Some("parent"));
    }
}
