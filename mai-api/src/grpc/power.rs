//! MaiPower gRPC service implementation.
//!
//! Provides power state queries and transition requests.
//! Power control is restricted to Admin profiles.
//! All transitions are audit-logged.

use tonic::{Request, Response, Status};
use tracing::{debug, info, warn};

use super::proto;
use super::{extract_grpc_profile, role_has_permission};
use crate::state::AppState;

use mai_core::power::{TransitionResult, TransitionTrigger};

/// MaiPower service implementation.
pub struct MaiPowerService {
    state: AppState,
}

impl MaiPowerService {
    pub fn new(state: AppState) -> Self {
        Self { state }
    }
}

/// Map a client action string to a TransitionTrigger.
#[allow(clippy::result_large_err)]
fn parse_transition_action(action: &str) -> Result<TransitionTrigger, Status> {
    match action.to_lowercase().as_str() {
        "boot" | "system_boot" => Ok(TransitionTrigger::SystemBoot),
        "wake" | "wake_trigger" => Ok(TransitionTrigger::WakeTrigger(
            mai_core::power::WakeSource::ApiRequest,
        )),
        "urgent_wake" => Ok(TransitionTrigger::UrgentWake(
            mai_core::power::WakeSource::ApiRequest,
        )),
        "promote" | "sentinel_promotion" => Ok(TransitionTrigger::SentinelPromotion),
        "demote" | "inactivity_timeout" => Ok(TransitionTrigger::InactivityTimeout),
        "deep_sleep" | "extended_inactivity" => Ok(TransitionTrigger::ExtendedInactivity),
        "override" | "manual_override" => Ok(TransitionTrigger::ManualOverride),
        "shutdown" | "system_shutdown" => Ok(TransitionTrigger::SystemShutdown),
        _ => Err(Status::invalid_argument(format!(
            "unknown power action '{action}'; valid: boot, wake, urgent_wake, promote, \
             demote, deep_sleep, override, shutdown"
        ))),
    }
}

#[tonic::async_trait]
impl proto::mai_power_server::MaiPower for MaiPowerService {
    /// Get current power state.
    async fn get_power_state(
        &self,
        _request: Request<proto::GetPowerStateRequest>,
    ) -> Result<Response<proto::PowerStateResponse>, Status> {
        debug!("gRPC GetPowerState");

        let power = self.state.power.read().await;
        let current = power.current_state();

        // Check if auto-demotion is pending
        let demotion_pending = power.check_auto_demotion().is_some();

        Ok(Response::new(proto::PowerStateResponse {
            state: current.as_str().to_string(),
            estimated_watts: current.estimated_watts_gpu_era(),
            state_duration_secs: 0, // Duration tracking
            demotion_pending,
        }))
    }

    /// Request a power state transition. Admin only.
    async fn transition_power(
        &self,
        request: Request<proto::PowerTransitionRequest>,
    ) -> Result<Response<proto::PowerTransitionResponse>, Status> {
        let (profile_id, role) = extract_grpc_profile(&request)?;
        if !role_has_permission(&role, "power_control") {
            return Err(Status::permission_denied(
                "admin role required for power control",
            ));
        }

        let req = request.into_inner();
        info!(
            profile_id = %profile_id,
            action = %req.action,
            reason = %req.reason,
            "gRPC TransitionPower"
        );

        let trigger = parse_transition_action(&req.action)?;

        let mut power = self.state.power.write().await;
        let previous = power.current_state().as_str().to_string();

        match power.request_transition(trigger) {
            Ok(result) => match result {
                TransitionResult::Completed { from, to, .. } => {
                    Ok(Response::new(proto::PowerTransitionResponse {
                        previous_state: from.as_str().to_string(),
                        current_state: to.as_str().to_string(),
                        accepted: true,
                        message: "transition completed".to_string(),
                    }))
                }
                TransitionResult::InProgress { from, to, .. } => {
                    Ok(Response::new(proto::PowerTransitionResponse {
                        previous_state: from.as_str().to_string(),
                        current_state: from.as_str().to_string(), // Still in progress
                        accepted: true,
                        message: format!("transition to {} in progress", to.as_str()),
                    }))
                }
                TransitionResult::Rejected { from, to, reason } => {
                    warn!(
                        profile_id = %profile_id,
                        from = %from.as_str(),
                        to = %to.as_str(),
                        reason = %reason,
                        "power transition rejected"
                    );
                    Ok(Response::new(proto::PowerTransitionResponse {
                        previous_state: from.as_str().to_string(),
                        current_state: from.as_str().to_string(),
                        accepted: false,
                        message: format!("transition rejected: {reason}"),
                    }))
                }
            },
            Err(e) => {
                warn!(
                    profile_id = %profile_id,
                    action = %req.action,
                    error = %e,
                    "power transition error"
                );
                Ok(Response::new(proto::PowerTransitionResponse {
                    previous_state: previous.clone(),
                    current_state: previous,
                    accepted: false,
                    message: format!("transition error: {e}"),
                }))
            }
        }
    }
}

// ── Tests ──────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_power_service_constructable() {
        fn _assert_send_sync<T: Send + Sync>() {}
        _assert_send_sync::<MaiPowerService>();
    }

    #[test]
    fn test_parse_transition_actions() {
        assert!(parse_transition_action("boot").is_ok());
        assert!(parse_transition_action("shutdown").is_ok());
        assert!(parse_transition_action("wake").is_ok());
        assert!(parse_transition_action("promote").is_ok());
        assert!(parse_transition_action("demote").is_ok());
        assert!(parse_transition_action("override").is_ok());
        assert!(parse_transition_action("BOOT").is_ok()); // Case insensitive
        assert!(parse_transition_action("invalid_action").is_err());
    }
}
