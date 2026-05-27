//! One-shot tool: generate ML-DSA-87 keypair + signed policy bundle
//! for the OpenBao staging environment. Writes to deployment/openbao-staging/.
//!
//! This is NOT part of the production code path. It is a staging bring-up
//! utility. Run once, commit the resulting artifacts, and never build this
//! binary again.

use std::fs;

use mai_compliance::bundle::{
    BundleMetadata, MlDsaBundleVerifier, PolicyBundlePayload, SignedPolicyBundle, payload_hash,
};
use mai_compliance::trust_cache::{RevocationSnapshot, SnapshotStatus};
use ml_dsa::signature::Signer;
use ml_dsa::{B32, EncodedSigningKey, KeyGen, MlDsa87, Signature, SigningKey};
use rand::RngCore;

const ML_DSA87_SK_LEN: usize = 4896;

fn sign_with(secret_key: &[u8], msg: &[u8; 32]) -> Vec<u8> {
    let sk_arr: &[u8; ML_DSA87_SK_LEN] = secret_key.try_into().expect("SK must be 4896 bytes");
    let sk_encoded = EncodedSigningKey::<MlDsa87>::from(*sk_arr);
    let sk = SigningKey::<MlDsa87>::decode(&sk_encoded);
    let sig: Signature<MlDsa87> = sk.sign(msg);
    sig.encode().to_vec()
}

type DynError = Box<dyn std::error::Error + Send + Sync>;

fn main_inner() -> Result<(), DynError> {
    let staging = {
        let cwd = std::env::current_dir()?;
        // Walk up from tools/gen-trust-staging to workspace root
        let mut root = cwd.clone();
        while !root.join("mai-compliance").join("Cargo.toml").exists() {
            if !root.pop() {
                panic!("Cannot find workspace root from {:?}", cwd);
            }
        }
        root.join("deployment").join("openbao-staging")
    };
    fs::create_dir_all(&staging)?;

    // 1. Generate ML-DSA-87 keypair
    let mut seed = [0u8; 32];
    rand::rng().fill_bytes(&mut seed);
    let kp = MlDsa87::key_gen_internal(&B32::from(seed));
    let pk_bytes = kp.verifying_key().encode().to_vec();
    let sk_bytes = kp.signing_key().encode().to_vec();

    eprintln!("PK length: {} (expect 2592)", pk_bytes.len());
    assert_eq!(pk_bytes.len(), 2592);

    // 2. Write anchor .pub file
    let anchor_path = staging.join("bundle-signer-staging.pub");
    fs::write(&anchor_path, &pk_bytes)?;
    eprintln!("Wrote anchor: {}", anchor_path.display());

    // 3. Build signed policy bundle
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_secs();
    let metadata = BundleMetadata {
        version: "2026.05.26.001".into(),
        issuer: "trust-bridge-staging".into(),
        issued_at_secs: now,
        expires_at_secs: now + 86400 * 90,
        tenant_id: "tribal-health-demo".into(),
    };
    let payload = PolicyBundlePayload {
        revocations: vec![RevocationSnapshot {
            claim_id: "clm_2026-05-26T06-00-00Z_demo".into(),
            status: SnapshotStatus::Valid,
            recorded_at_secs: now,
        }],
    };

    let hash = payload_hash(&metadata, &payload).expect("payload hash must succeed");
    let sig = sign_with(&sk_bytes, &hash);

    let bundle = SignedPolicyBundle {
        metadata,
        payload,
        signature: mai_compliance::bundle::SignatureEnvelope {
            algorithm: "ml-dsa-87".into(),
            public_key_id: "bundle-signer-staging".into(),
            bytes_hex: hex::encode(sig),
        },
    };

    // 4. Write bundle.json
    let bundle_path = staging.join("bundle.json");
    let bundle_json = serde_json::to_vec_pretty(&bundle)?;
    fs::write(&bundle_path, &bundle_json)?;
    eprintln!("Wrote bundle: {}", bundle_path.display());

    // 5. Self-verify round trip
    let verifier = MlDsaBundleVerifier::new().with_anchor("bundle-signer-staging", pk_bytes);
    let loaded: SignedPolicyBundle = serde_json::from_slice(&fs::read(&bundle_path)?)?;
    loaded.verified_payload(&verifier, now + 60)?;
    eprintln!("Bundle self-verify: PASS");

    eprintln!("\nStaging trust material ready at: {}", staging.display());
    Ok(())
}

#[allow(clippy::print_stderr)]
fn main() -> Result<(), DynError> {
    let handle = std::thread::Builder::new()
        .stack_size(16 * 1024 * 1024)
        .spawn(main_inner)?;
    handle.join().unwrap()
}
