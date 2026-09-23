//! Ordered connection evidence. Operation errors never erase a newer success.
use serde::Serialize;
use std::time::{Duration, Instant, SystemTime};

#[derive(Clone)]
pub struct Observation {
    pub server: crate::model::Server,
    pub started: Instant,
    pub finished: Instant,
    pub finished_wall: SystemTime,
    pub result: Result<(), String>,
}
fn observations() -> &'static tokio::sync::broadcast::Sender<Observation> {
    static CHANNEL: std::sync::OnceLock<tokio::sync::broadcast::Sender<Observation>> =
        std::sync::OnceLock::new();
    CHANNEL.get_or_init(|| tokio::sync::broadcast::channel(256).0)
}
pub fn subscribe() -> tokio::sync::broadcast::Receiver<Observation> {
    observations().subscribe()
}
pub fn report(server: &crate::model::Server, started: Instant, result: Result<(), String>) {
    let _ = observations().send(Observation {
        server: server.clone(),
        started,
        finished: Instant::now(),
        finished_wall: SystemTime::now(),
        result,
    });
}

const FRESH_FOR: Duration = Duration::from_secs(60);

#[derive(Clone, Default)]
pub struct Connectivity {
    observed: Option<Instant>,
    observed_wall: Option<SystemTime>,
    succeeded: Option<Instant>,
    error: Option<String>,
}
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Health {
    pub status: &'static str,
    pub error: Option<String>,
}
impl Connectivity {
    pub fn observe(&mut self, started: Instant, result: Result<(), String>) {
        self.observe_at(started, Instant::now(), SystemTime::now(), result);
    }
    pub fn observe_at(
        &mut self,
        started: Instant,
        finished: Instant,
        wall: SystemTime,
        result: Result<(), String>,
    ) {
        // Success proves liveness at completion. Failure cannot erase evidence
        // obtained while that failed attempt was still running.
        let cutoff = if result.is_ok() { finished } else { started };
        if self.observed.is_some_and(|at| at > cutoff) {
            return;
        }
        self.observed = Some(finished);
        self.observed_wall = Some(wall);
        match result {
            Ok(()) => {
                self.succeeded = Some(finished);
                self.error = None;
            }
            Err(error) => self.error = Some(error),
        }
    }
    pub fn health(&self, now: Instant) -> Health {
        let fresh = self
            .observed
            .is_some_and(|at| now.duration_since(at) < FRESH_FOR)
            && self
                .observed_wall
                .is_some_and(|at| at.elapsed().is_ok_and(|age| age < FRESH_FOR));
        Health {
            status: if !fresh {
                "unknown"
            } else if self.error.is_some() {
                "error"
            } else if self.succeeded.is_some() {
                "reachable"
            } else {
                "unknown"
            },
            error: self.error.clone(),
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn late_failure_cannot_replace_success() {
        let mut state = Connectivity::default();
        let old_probe = Instant::now();
        state.observe(Instant::now(), Ok(()));
        state.observe(old_probe, Err("timeout".into()));
        assert_eq!(state.health(Instant::now()).status, "reachable");
    }
    #[test]
    fn success_is_ordered_by_completion_and_delayed_events_cannot_rewind_state() {
        let mut state = Connectivity::default();
        let start = Instant::now();
        let wall = SystemTime::now();
        state.observe_at(
            start,
            start + Duration::from_secs(1),
            wall,
            Err("offline".into()),
        );
        state.observe_at(start, start + Duration::from_secs(2), wall, Ok(()));
        assert_eq!(
            state.health(start + Duration::from_secs(3)).status,
            "reachable"
        );
        state.observe_at(
            start + Duration::from_secs(3),
            start + Duration::from_secs(4),
            wall,
            Err("offline".into()),
        );
        state.observe_at(start, start + Duration::from_secs(2), wall, Ok(()));
        assert_eq!(state.health(start + Duration::from_secs(5)).status, "error");
        state.observed_wall = Some(wall - FRESH_FOR);
        assert_eq!(
            state.health(start + Duration::from_secs(5)).status,
            "unknown"
        );
    }
    #[test]
    fn evidence_expires_and_recovers() {
        let mut state = Connectivity::default();
        state.observe(Instant::now(), Err("authentication failed".into()));
        assert_eq!(
            state.health(Instant::now()).error.as_deref(),
            Some("authentication failed")
        );
        state.observe(Instant::now(), Ok(()));
        assert_eq!(state.health(Instant::now()).status, "reachable");
        assert_eq!(state.health(Instant::now() + FRESH_FOR).status, "unknown");
    }
}
