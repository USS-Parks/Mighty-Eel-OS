//! Routing fallback chain.
//!
//! After the router produces a decision, the caller asks an `Engine` to
//! actually execute it. Cloud calls can fail (timeout, rate limit, network);
//! local calls can fail (no instance loaded). `FallbackChain` composes the
//! engine with the router decision and applies a deterministic policy:
//!
//! 1. Try the primary decision.
//! 2. If a cloud call fails, retry locally (unless the router explicitly
//!    denied the request).
//! 3. If a local call fails because no instance is available, surface that
//!    as a structured error.
//! 4. `Denied` decisions short-circuit straight to a denied outcome — no
//!    fallback attempt is made.

use serde::Serialize;
use thiserror::Error;

use crate::router::{CloudProvider, RoutingDecision};

/// What actually happened when the engine ran the decision.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub enum FallbackOutcome {
    /// Primary decision executed successfully.
    PrimarySucceeded {
        /// What we tried.
        target: TargetKind,
    },
    /// Primary failed; local fallback succeeded.
    FellBackToLocal {
        /// Reason the primary failed.
        reason: FallbackReason,
    },
    /// Primary failed and no fallback was possible.
    Failed {
        /// Reason the primary failed.
        reason: FallbackReason,
    },
    /// Router denied the request; no execution attempted.
    DeniedByRouter {
        /// Stable code from the deny decision.
        code: String,
    },
}

/// Where the primary decision pointed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TargetKind {
    /// Local MAI inference engine.
    Local,
    /// Cloud frontier model.
    Cloud,
}

/// Why a primary attempt failed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FallbackReason {
    /// Cloud provider was unreachable (network, DNS, timeout).
    CloudUnreachable,
    /// Cloud provider rejected the request (rate limit, auth, content).
    CloudRejected,
    /// No local instance can serve the requested model.
    NoLocalInstance,
    /// Local engine is not initialized.
    LocalEngineUnavailable,
}

/// Error returned when neither the primary nor any fallback can serve.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum FallbackError {
    /// Both cloud and local were tried and both failed.
    #[error("all fallbacks exhausted: primary={primary:?}, local={local:?}")]
    Exhausted {
        /// Primary failure reason.
        primary: FallbackReason,
        /// Local fallback failure reason.
        local: FallbackReason,
    },
    /// Primary failed and the policy does not allow local fallback (e.g.,
    /// router decision was Denied).
    #[error("primary {0:?} failed and no fallback permitted")]
    NoFallbackPermitted(FallbackReason),
}

/// Trait the caller implements to wire actual execution.
///
/// The fallback chain calls into this for both primary and fallback paths.
/// Implementations should be cheap to retry — the chain may invoke
/// `run_local` immediately after a cloud failure.
pub trait Engine: Send + Sync {
    /// Run the request against a cloud provider.
    fn run_cloud(&self, provider: CloudProvider, model: &str) -> Result<(), FallbackReason>;
    /// Run the request against the local MAI inference engine.
    fn run_local(&self) -> Result<(), FallbackReason>;
}

/// Stateless chain that applies the fallback policy.
#[derive(Debug, Clone, Copy, Default)]
pub struct FallbackChain;

impl FallbackChain {
    /// Apply the fallback policy.
    pub fn execute<E: Engine + ?Sized>(
        &self,
        engine: &E,
        decision: &RoutingDecision,
    ) -> Result<FallbackOutcome, FallbackError> {
        match decision {
            RoutingDecision::Denied { code, .. } => {
                Ok(FallbackOutcome::DeniedByRouter { code: code.clone() })
            }
            RoutingDecision::Local { .. } => match engine.run_local() {
                Ok(()) => Ok(FallbackOutcome::PrimarySucceeded {
                    target: TargetKind::Local,
                }),
                Err(reason) => Ok(FallbackOutcome::Failed { reason }),
            },
            RoutingDecision::Cloud {
                provider, model, ..
            } => match engine.run_cloud(*provider, model) {
                Ok(()) => Ok(FallbackOutcome::PrimarySucceeded {
                    target: TargetKind::Cloud,
                }),
                Err(primary) => match engine.run_local() {
                    Ok(()) => Ok(FallbackOutcome::FellBackToLocal { reason: primary }),
                    Err(local) => Err(FallbackError::Exhausted { primary, local }),
                },
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::classifier::Classification;
    use std::sync::Mutex;

    struct ScriptedEngine {
        cloud_result: Result<(), FallbackReason>,
        local_result: Result<(), FallbackReason>,
        calls: Mutex<Vec<&'static str>>,
    }

    impl ScriptedEngine {
        fn new(cloud: Result<(), FallbackReason>, local: Result<(), FallbackReason>) -> Self {
            Self {
                cloud_result: cloud,
                local_result: local,
                calls: Mutex::new(Vec::new()),
            }
        }
        fn calls(&self) -> Vec<&'static str> {
            self.calls.lock().unwrap().clone()
        }
    }

    impl Engine for ScriptedEngine {
        fn run_cloud(&self, _provider: CloudProvider, _model: &str) -> Result<(), FallbackReason> {
            self.calls.lock().unwrap().push("cloud");
            self.cloud_result.clone()
        }
        fn run_local(&self) -> Result<(), FallbackReason> {
            self.calls.lock().unwrap().push("local");
            self.local_result.clone()
        }
    }

    fn cloud_decision() -> RoutingDecision {
        RoutingDecision::Cloud {
            provider: CloudProvider::Anthropic,
            model: "claude-sonnet-4-6".to_string(),
            reason: "test".to_string(),
            classification: Classification::Public,
        }
    }

    fn local_decision() -> RoutingDecision {
        RoutingDecision::Local {
            reason: "test".to_string(),
            classification: Classification::Regulated,
        }
    }

    fn denied_decision() -> RoutingDecision {
        RoutingDecision::Denied {
            code: "ROUTER-DENY-TEST".to_string(),
            reason: "test".to_string(),
            classification: Classification::Critical,
        }
    }

    #[test]
    fn test_cloud_success_returns_primary_succeeded() {
        let engine = ScriptedEngine::new(Ok(()), Err(FallbackReason::NoLocalInstance));
        let out = FallbackChain.execute(&engine, &cloud_decision()).unwrap();
        assert_eq!(
            out,
            FallbackOutcome::PrimarySucceeded {
                target: TargetKind::Cloud,
            },
        );
        assert_eq!(engine.calls(), vec!["cloud"]);
    }

    #[test]
    fn test_cloud_failure_falls_back_to_local() {
        let engine = ScriptedEngine::new(Err(FallbackReason::CloudUnreachable), Ok(()));
        let out = FallbackChain.execute(&engine, &cloud_decision()).unwrap();
        assert_eq!(
            out,
            FallbackOutcome::FellBackToLocal {
                reason: FallbackReason::CloudUnreachable,
            },
        );
        assert_eq!(engine.calls(), vec!["cloud", "local"]);
    }

    #[test]
    fn test_cloud_and_local_failure_returns_exhausted() {
        let engine = ScriptedEngine::new(
            Err(FallbackReason::CloudRejected),
            Err(FallbackReason::NoLocalInstance),
        );
        let err = FallbackChain
            .execute(&engine, &cloud_decision())
            .unwrap_err();
        assert!(matches!(err, FallbackError::Exhausted { .. }));
    }

    #[test]
    fn test_local_decision_only_calls_local() {
        let engine = ScriptedEngine::new(Err(FallbackReason::CloudUnreachable), Ok(()));
        FallbackChain.execute(&engine, &local_decision()).unwrap();
        assert_eq!(engine.calls(), vec!["local"]);
    }

    #[test]
    fn test_denied_decision_short_circuits_with_code() {
        let engine = ScriptedEngine::new(Ok(()), Ok(()));
        let out = FallbackChain.execute(&engine, &denied_decision()).unwrap();
        match out {
            FallbackOutcome::DeniedByRouter { code } => {
                assert_eq!(code, "ROUTER-DENY-TEST");
            }
            other => panic!("expected DeniedByRouter, got {other:?}"),
        }
        // Engine must not be called at all.
        assert!(engine.calls().is_empty());
    }

    #[test]
    fn test_local_decision_failure_surfaces_as_failed() {
        let engine = ScriptedEngine::new(Ok(()), Err(FallbackReason::LocalEngineUnavailable));
        let out = FallbackChain.execute(&engine, &local_decision()).unwrap();
        assert_eq!(
            out,
            FallbackOutcome::Failed {
                reason: FallbackReason::LocalEngineUnavailable,
            },
        );
    }
}
