//! N8 — attestation-liveness (the differentiator, A1.3.6). Liveness here is not
//! "is it responding" but "is it still the code we trust." The node periodically
//! re-measures each running workload; on **drift** from its sealed measurement it
//! **evicts** the workload and **revokes its runtime token**, so the token is
//! denied estate-wide — the revocation reuses R9's fan-out, and the node's own
//! edge admission (N6) applies the same snapshot. A tampered replica does not
//! merely restart; it is removed and cut off.

use chrono::{DateTime, Utc};

use fabric_crypto::Signer;
use fabric_revocation::{RevocationError, RevocationSnapshot};

use crate::driver::{DriverError, WorkloadDriver, WorkloadHandle, WorkloadRun};

/// A workload the node is attesting: its handle, its runtime token id, and the
/// sealed measurement it must keep matching.
#[derive(Debug, Clone)]
pub struct AttestedWorkload {
    /// How to restart it (unused on eviction, carried for the caller).
    pub run: WorkloadRun,
    /// The running instance.
    pub handle: WorkloadHandle,
    /// The runtime token minted for this replica (S7) — revoked on drift.
    pub token_id: String,
    /// The measurement sealed at placement; the workload must keep matching it.
    pub expected_measurement: String,
}

/// Re-measures a running workload. Real impls read a TPM / Nitro PCR or hash the
/// on-disk image; a test impl returns a controllable digest.
pub trait Measurer: Send + Sync {
    /// The current measurement of `handle`'s instance.
    fn measure(&self, handle: &WorkloadHandle) -> String;
}

/// The outcome of an attestation-liveness sweep for one workload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AttestationVerdict {
    /// Measurement matches — still the code we trust.
    Intact,
    /// Drift: the workload was evicted; its token must be denied estate-wide.
    Evicted {
        /// The runtime token to revoke.
        token_id: String,
        /// The measurement observed (which did not match).
        observed: String,
    },
}

/// Re-measure `workload`; on drift, evict it (stop via the driver) and return an
/// `Evicted` verdict carrying the token to revoke.
///
/// # Errors
/// Propagates a [`DriverError`] from the eviction.
pub fn check(
    driver: &dyn WorkloadDriver,
    measurer: &dyn Measurer,
    workload: &AttestedWorkload,
) -> Result<AttestationVerdict, DriverError> {
    let observed = measurer.measure(&workload.handle);
    if observed == workload.expected_measurement {
        return Ok(AttestationVerdict::Intact);
    }
    driver.stop(&workload.handle)?;
    Ok(AttestationVerdict::Evicted {
        token_id: workload.token_id.clone(),
        observed,
    })
}

/// Build a signed, emergency revocation snapshot denying every drifted token —
/// the artifact R9 fans out estate-wide and the edge (N6) applies.
///
/// # Errors
/// Propagates a signing error.
pub fn revocation_for(
    verdicts: &[AttestationVerdict],
    snapshot_id: &str,
    now: DateTime<Utc>,
    valid_for: chrono::Duration,
    signer: &dyn Signer,
) -> Result<RevocationSnapshot, RevocationError> {
    let mut snapshot = RevocationSnapshot::new(
        snapshot_id,
        now.to_rfc3339(),
        (now + valid_for).to_rfc3339(),
    )
    .emergency();
    for verdict in verdicts {
        if let AttestationVerdict::Evicted { token_id, .. } = verdict {
            snapshot.revoked_tokens.push(token_id.clone());
        }
    }
    fabric_revocation::sign(snapshot, signer)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::driver::{NoopDriver, WorkloadDriver, WorkloadState};
    use fabric_crypto::providers::{MlDsa87Verifier, RustCryptoMlDsa87};

    struct FixedMeasure(&'static str);
    impl Measurer for FixedMeasure {
        fn measure(&self, _handle: &WorkloadHandle) -> String {
            self.0.to_owned()
        }
    }

    fn attested(driver: &NoopDriver, name: &str, expected: &str) -> AttestedWorkload {
        let run = WorkloadRun {
            name: name.to_owned(),
            image: None,
            command: vec!["gateway".to_owned()],
        };
        let handle = driver.start(&run).unwrap();
        AttestedWorkload {
            run,
            handle,
            token_id: format!("rt:{name}"),
            expected_measurement: expected.to_owned(),
        }
    }

    #[test]
    fn an_intact_workload_is_left_running() {
        let driver = NoopDriver::default();
        let workload = attested(&driver, "gw-a", "sha256:trusted");
        let verdict = check(&driver, &FixedMeasure("sha256:trusted"), &workload).unwrap();
        assert_eq!(verdict, AttestationVerdict::Intact);
        assert_eq!(
            driver.inspect(&workload.handle).unwrap(),
            WorkloadState::Running
        );
    }

    #[test]
    fn a_drifted_workload_is_evicted_and_revoked() {
        let driver = NoopDriver::default();
        let workload = attested(&driver, "gw-a", "sha256:trusted");

        // The measurement has drifted (tampered code / firmware).
        let verdict = check(&driver, &FixedMeasure("sha256:tampered"), &workload).unwrap();
        assert!(matches!(verdict, AttestationVerdict::Evicted { .. }));

        // Evicted: no longer running.
        assert_ne!(
            driver.inspect(&workload.handle).unwrap(),
            WorkloadState::Running
        );

        // Its token is in a signed revocation snapshot → denied estate-wide.
        let anchor = RustCryptoMlDsa87::generate("anchor").unwrap();
        let snapshot = revocation_for(
            std::slice::from_ref(&verdict),
            "drift-1",
            Utc::now(),
            chrono::Duration::hours(1),
            &anchor,
        )
        .unwrap();
        assert!(snapshot.is_token_revoked("rt:gw-a"));
        assert!(snapshot.emergency);
        assert!(
            fabric_revocation::verify(&snapshot, &MlDsa87Verifier, anchor.public_key()).is_ok(),
            "the revocation snapshot verifies against the anchor"
        );
    }
}
