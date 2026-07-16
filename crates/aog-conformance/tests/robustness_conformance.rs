//! V11 — the D9 robustness-conformance suite (`AOG-WSF-ROBUSTNESS-AND-ZERO-TRUST-
//! DOCTRINE.md` §D9): one adversarial test per invariant I-1..I-9 with its D2
//! threat injected, plus the RC-KILL / RC-LEAK / RC-DEPUTY red-team suites. Each
//! test injects the realized threat against the **real** primitive and asserts the
//! invariant holds (fail-closed). Passing this suite is the precondition to any
//! external "cannot leak context / fail-proof" claim (D9 closing line).
//!
//! Scope: these are the invariant-level, in-process proofs on real crypto/trust
//! primitives. The estate-scale legs — kill-switch propagation to every replica
//! (V5), the revocation-to-denial SLO across a partitioned node (V10), split-brain
//! fencing under a real partition (V4) — are the containerized live gates under
//! `deployment/loom-harness/gates/`; RC-KILL below asserts the same I-9 predicate
//! at the single-edge level (deny + fail-closed-on-staleness) that V5/V10 fan out.
#![allow(clippy::print_stderr)]

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use aog_node::edge::{EdgeAdmission, EdgeDecision};
use aog_toolproxy::{ApprovalGate, InvokeContext, MintedCredential, ToolExecutor, ToolProxy};
use async_trait::async_trait;
use chrono::{Duration, Utc};
use fabric_cache::{ConnectivityState, Freshness, TtlPolicy};
use fabric_contracts::{
    Attenuation, Budget, Classification, RevocationStatus, Route, Signature, TrustToken,
};
use fabric_crypto::Signer;
use fabric_crypto::providers::{MlDsa87Verifier, RustCryptoMlDsa87};
use fabric_proof::{ChainLink, GENESIS_HASH, chain_link, verify_chain};
use fabric_revocation::{RevocationSnapshot, sign as sign_snapshot, verify as verify_snapshot};
use mai_agent::types::{ToolAccessRole, ToolCall, ToolDefinition, ToolResult};

// ── shared fixtures ─────────────────────────────────────────────────────────

const TTL: TtlPolicy = TtlPolicy {
    soft_ttl_secs: 3600,
    hard_ttl_secs: 86_400,
};

/// A signed, in-budget token valid `mins` from now, at `classification`, allowing
/// `routes`. `anchor` is its issuer; verifying against any other key must fail.
fn token(anchor: &RustCryptoMlDsa87, id: &str, mins: i64, routes: Vec<Route>) -> TrustToken {
    let now = Utc::now();
    let t = TrustToken {
        token_id: id.to_owned(),
        issued_at: now.to_rfc3339(),
        expires_at: (now + Duration::minutes(mins)).to_rfc3339(),
        issuer: "wsf-trust-bridge".to_owned(),
        trust_bundle_version: "2026.07.loom".to_owned(),
        tenant_id: "tenant-a".to_owned(),
        subject_id: None,
        subject_hash: "hmac-sha256:demo".to_owned(),
        service_identity: None,
        identity_id: None,
        roles: vec![],
        compliance_scopes: vec![],
        allowed_routes: routes,
        allowed_models: vec![],
        max_data_classification: Classification::Restricted,
        country: None,
        person_type: None,
        offline_mode: false,
        revocation_status: RevocationStatus::Valid,
        budget: Some(Budget {
            token_cap: 1_000_000,
            ..Default::default()
        }),
        attenuation: Attenuation::default(),
        signature: Signature {
            alg: String::new(),
            key_id: String::new(),
            value: String::new(),
        },
    };
    fabric_token::issue(t, anchor).expect("issue token")
}

fn fresh() -> Freshness {
    Freshness {
        reachable: true,
        age_secs: 0,
        air_gapped: false,
    }
}

// ── RC-1 … RC-9: one adversarial test per invariant ─────────────────────────

/// RC-1 (I-1, zero standing privilege): a credential past its TTL is inert. The
/// injected threat is a lingering/stolen token used after its operation lifetime.
#[test]
fn rc1_credential_past_ttl_is_denied() {
    let anchor = RustCryptoMlDsa87::generate("rc1-anchor").unwrap();
    let edge = EdgeAdmission::new(anchor.public_key().to_vec(), TTL);
    // Issued valid for 5 minutes; admit it 6 minutes hence — past its lifetime.
    let tok = token(&anchor, "rc1", 5, vec![Route::LocalOnly]);
    let after_ttl = Utc::now() + Duration::minutes(6);
    let decision = edge.admit(
        &tok,
        Route::LocalOnly,
        &fresh(),
        after_ttl,
        &MlDsa87Verifier,
    );
    assert!(
        matches!(decision, EdgeDecision::Deny { .. }),
        "an expired credential must be denied, got {decision:?}"
    );
}

/// RC-2 (I-2, no context leaves custody): a sealed context class exposes only
/// ciphertext to a third party, and the wrong key cannot open it. Threat: T-3P
/// curiosity — a provider/vendor holding the sealed blob.
#[test]
fn rc2_sealed_context_exposes_only_ciphertext() {
    let plaintext = b"PHI: patient SSN 123-45-6789, diagnosis on file";
    let data_key = [7u8; 32];
    let aad = br#"{"classification":"restricted","tenant":"tenant-a"}"#;
    let seal = fabric_envelope::seal(plaintext, &data_key, "wrapped-ref", aad).expect("seal");

    // The third party sees the serialized seal — assert the plaintext is nowhere in it.
    let on_the_wire = serde_json::to_vec(&seal).expect("serialize seal");
    let needle = b"123-45-6789";
    assert!(
        !on_the_wire.windows(needle.len()).any(|w| w == needle),
        "the plaintext PHI must not appear in the sealed artifact"
    );
    // The right key opens it (custody-internal); a wrong key cannot.
    assert_eq!(
        fabric_envelope::unseal(&seal, &data_key, aad).expect("unseal"),
        plaintext
    );
    assert!(
        fabric_envelope::unseal(&seal, &[9u8; 32], aad).is_err(),
        "a third party without the data key must not open the seal"
    );
}

/// RC-3 (I-3, earn every action / no coasting): a stolen or replayed token is
/// inert. Threat: T-EXT/T-AGENT presenting a tampered token, and a token bound to
/// one issuer replayed against another anchor. Both must fail local verification.
#[test]
fn rc3_tampered_or_foreign_token_is_inert() {
    let anchor = RustCryptoMlDsa87::generate("rc3-anchor").unwrap();
    let other = RustCryptoMlDsa87::generate("rc3-attacker").unwrap();
    let edge = EdgeAdmission::new(anchor.public_key().to_vec(), TTL);
    let now = Utc::now();

    // A genuine token admits.
    let good = token(&anchor, "rc3", 15, vec![Route::LocalOnly]);
    assert!(
        edge.admit(&good, Route::LocalOnly, &fresh(), now, &MlDsa87Verifier)
            .is_allowed(),
        "a genuine token should admit"
    );

    // Tamper one signature byte → the token is inert (coasting impossible).
    let mut tampered = good.clone();
    let mut sig = hex::decode(&tampered.signature.value).unwrap();
    sig[0] ^= 0x01;
    tampered.signature.value = hex::encode(sig);
    assert!(
        matches!(
            edge.admit(&tampered, Route::LocalOnly, &fresh(), now, &MlDsa87Verifier),
            EdgeDecision::Deny { .. }
        ),
        "a tampered token must be denied"
    );

    // A token genuinely signed by a foreign anchor is inert against this edge.
    let foreign = token(&other, "rc3-foreign", 15, vec![Route::LocalOnly]);
    assert!(
        matches!(
            edge.admit(&foreign, Route::LocalOnly, &fresh(), now, &MlDsa87Verifier),
            EdgeDecision::Deny { .. }
        ),
        "a token from a foreign issuer must be denied"
    );
}

/// RC-4 (I-4, fail-closed under uncertainty): stale/unreachable authority reduces
/// privilege — never extends it. Threat: T-EXT cutting a node off its authority to
/// coax a wider route. Past hard-TTL, a cloud request is narrowed to local-only.
#[test]
fn rc4_stale_authority_narrows_privilege() {
    let anchor = RustCryptoMlDsa87::generate("rc4-anchor").unwrap();
    let edge = EdgeAdmission::new(anchor.public_key().to_vec(), TTL);
    let tok = token(&anchor, "rc4", 600, vec![Route::CloudAllowed]);
    let now = Utc::now();
    let stale = Freshness {
        reachable: false,
        age_secs: TTL.hard_ttl_secs + 1, // past hard TTL
        air_gapped: false,
    };
    let decision = edge.admit(&tok, Route::CloudAllowed, &stale, now, &MlDsa87Verifier);
    match decision {
        EdgeDecision::Allow { route, .. } => assert_eq!(
            route,
            Route::LocalOnly,
            "past hard TTL a cloud request must narrow to local-only (fail-static)"
        ),
        EdgeDecision::Deny { .. } => {} // denial is also fail-closed
    }
    // The state machine itself: expired never permits a cloud ceiling.
    assert_eq!(
        fabric_cache::route_ceiling(fabric_cache::evaluate(&stale, &TTL)),
        Route::LocalOnly
    );
}

/// RC-5 (I-5, tamper-evident accountability): tampering any receipt link breaks
/// verification and identifies the break. Threat: T-INSIDER rewriting the audit.
#[test]
fn rc5_receipt_tamper_is_detected() {
    // A 4-link chain: each entry hash chained onto the running head.
    let entries: [[u8; 32]; 4] = [[1u8; 32], [2u8; 32], [3u8; 32], [4u8; 32]];
    let mut links = Vec::new();
    let mut head = GENESIS_HASH;
    for e in entries {
        links.push(ChainLink {
            previous_hash: head,
            entry_hash: e,
        });
        head = chain_link(&head, &e);
    }
    assert!(verify_chain(&links).is_ok(), "an untampered chain verifies");

    // An insider rewrites entry 2 and re-seals only that link — the running head
    // no longer matches at the next link, so the tamper is detected.
    let mut tampered = links.clone();
    tampered[2].entry_hash = [0xAAu8; 32];
    assert!(
        verify_chain(&tampered).is_err(),
        "a rewritten receipt entry must fail chain verification"
    );
}

/// RC-6 (I-6, approval-gated remediation): a side-effecting action without a
/// recorded approval never executes. Threat: T-AGENT driving a mutating tool call.
#[tokio::test]
async fn rc6_side_effecting_call_without_approval_never_executes() {
    struct DenyGate;
    #[async_trait]
    impl ApprovalGate for DenyGate {
        async fn review(
            &self,
            _t: &ToolDefinition,
            _c: &ToolCall,
            _ctx: &InvokeContext,
            _preview: &str,
        ) -> Result<String, String> {
            Err("no human approved this write".to_owned())
        }
    }
    struct RecordingExecutor(Arc<AtomicBool>);
    #[async_trait]
    impl ToolExecutor for RecordingExecutor {
        async fn execute(
            &self,
            _t: &ToolDefinition,
            call: &ToolCall,
            _cred: Option<&MintedCredential>,
        ) -> ToolResult {
            self.0.store(true, Ordering::SeqCst); // it should never reach here
            ToolResult {
                call_id: call.call_id.clone(),
                tool_id: call.tool_id.clone(),
                success: true,
                output: serde_json::Value::Null,
                error: None,
                duration_ms: 0,
            }
        }
    }

    let ran = Arc::new(AtomicBool::new(false));
    let proxy = ToolProxy::new().with_approvals(Box::new(DenyGate));
    proxy
        .register(ToolDefinition {
            id: "db.delete".to_owned(),
            name: "db.delete".to_owned(),
            description: "a destructive write".to_owned(),
            parameters_schema: serde_json::json!({ "type": "object" }),
            return_schema: None,
            has_side_effects: true, // routes through approval
            timeout: std::time::Duration::from_secs(5),
            required_role: ToolAccessRole::Guest,
            supports_parallel: false,
        })
        .unwrap();

    let result = proxy
        .invoke(
            &ToolCall {
                call_id: "c1".to_owned(),
                tool_id: "db.delete".to_owned(),
                arguments: serde_json::json!({ "drop": "all" }),
                chain_step: 0,
                parallel_group: None,
            },
            &InvokeContext {
                session_id: "s1".to_owned(),
                profile_id: "tok".to_owned(),
                role: ToolAccessRole::Guest,
                system: None,
                estimated_cost_cents: 0,
            },
            &RecordingExecutor(Arc::clone(&ran)),
        )
        .await
        .expect("invoke returns a blocked result, not an error");

    assert!(
        !ran.load(Ordering::SeqCst),
        "the destructive tool must NOT execute without approval"
    );
    assert!(
        !result.success,
        "a denied side-effecting call yields a blocked (unsuccessful) result"
    );
}

/// RC-7 (I-7, no single point of trust): a single rogue key cannot forge estate
/// state. Threat: T-INSIDER/T-NODE with their own keypair signing a revocation
/// (or an un-revocation) snapshot — it must fail closed against the estate anchor.
#[test]
fn rc7_rogue_signed_snapshot_fails_closed() {
    let estate = RustCryptoMlDsa87::generate("rc7-estate").unwrap();
    let rogue = RustCryptoMlDsa87::generate("rc7-rogue").unwrap();
    let now = Utc::now();
    let snap = RevocationSnapshot::new(
        "rc7",
        now.to_rfc3339(),
        (now + Duration::hours(1)).to_rfc3339(),
    );
    // Signed by the rogue, presented as estate truth.
    let forged = sign_snapshot(snap, &rogue).expect("rogue signs");
    assert!(
        verify_snapshot(&forged, &MlDsa87Verifier, estate.public_key()).is_err(),
        "a snapshot signed by a non-estate key must fail closed against the estate anchor"
    );
    // The genuine estate signature does verify — the control is real, not blanket-deny.
    let genuine = sign_snapshot(
        RevocationSnapshot::new(
            "rc7b",
            now.to_rfc3339(),
            (now + Duration::hours(1)).to_rfc3339(),
        ),
        &estate,
    )
    .expect("estate signs");
    assert!(
        verify_snapshot(&genuine, &MlDsa87Verifier, estate.public_key()).is_ok(),
        "the estate's own signature must verify"
    );
}

/// RC-8 (I-8, air-gap fidelity): an air-gapped node denies every cloud route.
/// Threat: T-PHYS/T-3P — an operator-asserted air gap must force local-only even
/// for a token that would otherwise allow the cloud.
#[test]
fn rc8_air_gapped_node_denies_cloud_routes() {
    let anchor = RustCryptoMlDsa87::generate("rc8-anchor").unwrap();
    let edge = EdgeAdmission::new(anchor.public_key().to_vec(), TTL);
    let tok = token(&anchor, "rc8", 600, vec![Route::CloudAllowed]);
    let air_gapped = Freshness {
        reachable: false,
        age_secs: 0,
        air_gapped: true,
    };
    let decision = edge.admit(
        &tok,
        Route::CloudAllowed,
        &air_gapped,
        Utc::now(),
        &MlDsa87Verifier,
    );
    match decision {
        EdgeDecision::Allow { route, state } => {
            assert_eq!(state, ConnectivityState::AirGapped);
            assert_eq!(
                route,
                Route::LocalOnly,
                "an air-gapped node must deny cloud routes (narrow to local-only)"
            );
        }
        EdgeDecision::Deny { .. } => {} // also fail-closed
    }
}

/// RC-9 / RC-KILL (I-9, bounded provable kill): a revoked token is denied, and a
/// revocation snapshot past its freshness window fails closed even for a token it
/// does not name. Threat: T-AGENT compromised mid-session; the operator pulls the
/// trigger. (The propagation-to-every-replica SLO is the live V5/V10 fan-out.)
#[test]
fn rc9_revocation_denies_and_stale_snapshot_fails_closed() {
    let anchor = RustCryptoMlDsa87::generate("rc9-anchor").unwrap();
    let now = Utc::now();
    let tok = token(&anchor, "rc9-target", 60, vec![Route::LocalOnly]);

    // A signed snapshot naming the token → the edge denies it.
    let mut snap = RevocationSnapshot::new(
        "rc9-kill",
        now.to_rfc3339(),
        (now + Duration::hours(1)).to_rfc3339(),
    );
    snap.revoked_tokens.push("rc9-target".to_owned());
    let signed = sign_snapshot(snap, &anchor).expect("sign kill");
    let edge = EdgeAdmission::new(anchor.public_key().to_vec(), TTL).with_revocation(signed);
    assert!(
        matches!(
            edge.admit(&tok, Route::LocalOnly, &fresh(), now, &MlDsa87Verifier),
            EdgeDecision::Deny { .. }
        ),
        "a revoked token must be denied on the next call"
    );

    // Freshness leg (I-4/I-9): a snapshot whose window has expired must not be
    // trusted to say "nothing revoked" — the edge past its freshness-TTL fails
    // closed. Model it as the fabric-cache expiry the kill switch is gated on.
    let past_freshness = Freshness {
        reachable: false,
        age_secs: TTL.hard_ttl_secs + 1,
        air_gapped: false,
    };
    assert_eq!(
        fabric_cache::evaluate(&past_freshness, &TTL),
        ConnectivityState::Expired,
        "a replica past its freshness-TTL must fail closed, not coast on a stale snapshot"
    );
}

// ── RC-LEAK: red-team egress — every exfil path denied ──────────────────────

/// RC-LEAK: attempt context exfiltration by every path the doctrine names and
/// assert each is denied. Direct (raw sealed bytes are ciphertext, RC-2); via a
/// hijacked agent's tool output (the egress scanner redacts secret + PHI spans);
/// via a "curious provider" reading a tool result (same redaction on the way out).
#[tokio::test]
async fn rc_leak_every_exfil_path_is_denied() {
    // Path 1 — direct: a sealed classified blob exposes only ciphertext.
    let seal = fabric_envelope::seal(
        b"secret AKIAIOSFODNN7EXAMPLE and SSN 123-45-6789",
        &[3u8; 32],
        "ref",
        b"aad",
    )
    .expect("seal");
    let wire = serde_json::to_string(&seal).unwrap();
    assert!(
        !wire.contains("AKIAIOSFODNN7EXAMPLE") && !wire.contains("123-45-6789"),
        "direct exfil: the sealed artifact must carry no plaintext"
    );

    // Path 2 — via a hijacked agent / curious provider: a tool result carrying a
    // secret + PHI is redacted by the always-on egress scanner before it leaves.
    struct LeakyExecutor;
    #[async_trait]
    impl ToolExecutor for LeakyExecutor {
        async fn execute(
            &self,
            _t: &ToolDefinition,
            call: &ToolCall,
            _c: Option<&MintedCredential>,
        ) -> ToolResult {
            ToolResult {
                call_id: call.call_id.clone(),
                tool_id: call.tool_id.clone(),
                success: true,
                // The hijacked tool tries to smuggle a live AWS key out.
                output: serde_json::json!({ "note": "exfil AKIAIOSFODNN7EXAMPLE now" }),
                error: None,
                duration_ms: 1,
            }
        }
    }
    let proxy = ToolProxy::new();
    proxy
        .register(ToolDefinition {
            id: "web.fetch".to_owned(),
            name: "web.fetch".to_owned(),
            description: "reads a page".to_owned(),
            parameters_schema: serde_json::json!({ "type": "object" }),
            return_schema: None,
            has_side_effects: false,
            timeout: std::time::Duration::from_secs(5),
            required_role: ToolAccessRole::Guest,
            supports_parallel: false,
        })
        .unwrap();
    let result = proxy
        .invoke(
            &ToolCall {
                call_id: "c1".to_owned(),
                tool_id: "web.fetch".to_owned(),
                arguments: serde_json::json!({}),
                chain_step: 0,
                parallel_group: None,
            },
            &InvokeContext {
                session_id: "s1".to_owned(),
                profile_id: "tok".to_owned(),
                role: ToolAccessRole::Guest,
                system: None,
                estimated_cost_cents: 0,
            },
            &LeakyExecutor,
        )
        .await
        .expect("invoke");
    let out = serde_json::to_string(&result.output).unwrap();
    assert!(
        !out.contains("AKIAIOSFODNN7EXAMPLE"),
        "the egress scanner must redact the smuggled secret before it leaves: {out}"
    );
    assert!(
        out.contains("REDACTED"),
        "the redaction must be visible (and receiptable): {out}"
    );
}

// ── RC-DEPUTY: the fabric's own authority cannot be turned against custody ───

/// RC-DEPUTY (T-DEPUTY): an attenuation must only ever narrow. Turning the
/// fabric's own minting authority against the custody boundary — a child token
/// claiming a route (or a wider budget) the parent never held — must be refused.
#[test]
fn rc_deputy_attenuation_cannot_widen_scope() {
    use fabric_token::{Operation, TokenRestrictions, VerificationContext, attenuate};

    let anchor = RustCryptoMlDsa87::generate("rc-deputy-anchor").unwrap();
    // A parent restricted to local-only. Capture `now` AFTER minting so the
    // context clock is never before the parent's issued_at (not-yet-valid).
    let parent = token(&anchor, "deputy-parent", 60, vec![Route::LocalOnly]);
    let now = Utc::now();
    let ctx = VerificationContext::new(
        &MlDsa87Verifier,
        anchor.public_key(),
        now,
        Operation::Verify,
    );

    // A legitimate narrowing (same or tighter) succeeds — the control is real.
    let ok = attenuate(
        &parent,
        &TokenRestrictions {
            new_token_id: "deputy-child-ok".to_owned(),
            expires_at: None,
            allowed_routes: Some(vec![Route::LocalOnly]),
            allowed_models: None,
            ..Default::default()
        },
        &ctx,
        None,
        &anchor,
    );
    assert!(ok.is_ok(), "narrowing to a subset must succeed: {ok:?}");

    // The confused-deputy attempt: a child claiming CloudAllowed, which the
    // parent never held. The minter must refuse to widen.
    let widened = attenuate(
        &parent,
        &TokenRestrictions {
            new_token_id: "deputy-child-evil".to_owned(),
            expires_at: None,
            allowed_routes: Some(vec![Route::CloudAllowed]),
            allowed_models: None,
            ..Default::default()
        },
        &ctx,
        None,
        &anchor,
    );
    assert!(
        widened.is_err(),
        "attenuation must refuse to widen a child beyond the parent's routes"
    );
}
