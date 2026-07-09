//! Node heartbeat + liveness. The agent periodically reports the node's
//! reconciled free (`allocatable`) capacity and a fresh timestamp into
//! `NodeStatus` (ready). The control-plane node controller treats a node whose
//! heartbeat has aged past its freshness window as not-live and reschedules its
//! workloads. Fail-closed: a node that has never beaten, or whose timestamp is
//! unparseable, is stale — a silent node loses candidacy (I-4).

use chrono::{DateTime, Utc};

use aog_estate::{Capacity, NodeStatus, Phase};

/// The status a live node reports: ready, timestamped `now`, advertising the
/// capacity it currently has free.
#[must_use]
pub fn heartbeat(allocatable: Capacity, now: DateTime<Utc>) -> NodeStatus {
    NodeStatus {
        phase: Phase::Ready,
        ready: true,
        allocatable,
        last_heartbeat: Some(now.to_rfc3339()),
    }
}

/// Whether a node's last heartbeat has aged past `ttl_secs` as of `now`. A node
/// with no heartbeat, or an unparseable one, is stale (fail-closed).
#[must_use]
pub fn is_stale(status: &NodeStatus, now: DateTime<Utc>, ttl_secs: i64) -> bool {
    let Some(beat) = status.last_heartbeat.as_deref() else {
        return true;
    };
    let Ok(beat) = DateTime::parse_from_rfc3339(beat) else {
        return true;
    };
    (now - beat.with_timezone(&Utc)).num_seconds() > ttl_secs
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fresh_heartbeat_is_ready_and_live() {
        let now = Utc::now();
        let status = heartbeat(Capacity::default(), now);
        assert!(status.ready);
        assert!(status.last_heartbeat.is_some());
        assert!(!is_stale(&status, now, 30));
    }

    #[test]
    fn a_missing_heartbeat_is_stale() {
        assert!(is_stale(&NodeStatus::default(), Utc::now(), 30));
    }

    #[test]
    fn an_old_heartbeat_is_stale() {
        let now = Utc::now();
        let status = heartbeat(Capacity::default(), now - chrono::Duration::seconds(120));
        assert!(is_stale(&status, now, 30));
    }

    #[test]
    fn an_unparseable_heartbeat_is_stale() {
        let status = NodeStatus {
            last_heartbeat: Some("not-a-timestamp".to_owned()),
            ready: true,
            ..NodeStatus::default()
        };
        assert!(is_stale(&status, Utc::now(), 30));
    }
}
