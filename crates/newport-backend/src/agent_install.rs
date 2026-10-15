//! Session-owned deployment progress exposed through the ordinary snapshot.
use serde::Serialize;
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Progress {
    pub operation_id: Option<uuid::Uuid>,
    pub stage: &'static str,
    pub sent: usize,
    pub total: usize,
    pub error: Option<String>,
    pub failed_stage: Option<String>,
    pub bytes_per_second: Option<f64>,
    #[serde(skip)]
    rate: UploadRate,
}
/// A recent rate, measured from completed SSH writes; idle periods reduce it to zero.
#[derive(Clone, Default)]
struct UploadRate(VecDeque<(Instant, usize)>);
impl UploadRate {
    fn trim(&mut self, now: Instant) {
        while self.0.len() > 1 && now.duration_since(self.0[1].0) >= Duration::from_secs(5) {
            self.0.pop_front();
        }
    }
    fn observe(&mut self, now: Instant, sent: usize) {
        self.0.push_back((now, sent));
        self.trim(now);
    }
    fn speed(&mut self, now: Instant) -> Option<f64> {
        self.trim(now);
        let &(start, before) = self.0.front()?;
        let &(_, sent) = self.0.back()?;
        let seconds = now.duration_since(start).as_secs_f64();
        (seconds >= 0.2).then(|| sent.saturating_sub(before) as f64 / seconds)
    }
}

/// Owned by the authoritative ServerSession. Retired attempts can only mutate
/// their retired owner, never a replacement session's progress.
#[derive(Default)]
pub struct State(Mutex<Option<Progress>>);
impl State {
    pub fn snapshot(&self) -> Option<Progress> {
        self.0.lock().unwrap().clone().map(|mut progress| {
            progress.bytes_per_second = if progress.stage == "uploading" {
                progress.rate.speed(Instant::now())
            } else {
                None
            };
            progress
        })
    }
    pub fn attempt(self: &Arc<Self>) -> Attempt {
        self.attempt_for(None)
    }
    pub fn attempt_for(self: &Arc<Self>, operation_id: Option<uuid::Uuid>) -> Attempt {
        let attempt = Attempt {
            state: self.clone(),
            finished: false,
            operation_id,
        };
        attempt.update("preparing", 0, 0, None);
        attempt
    }
}
pub struct Attempt {
    operation_id: Option<uuid::Uuid>,
    state: Arc<State>,
    finished: bool,
}
impl Attempt {
    pub fn update(&self, stage: &'static str, sent: usize, total: usize, error: Option<String>) {
        let mut current = self.state.0.lock().unwrap();
        let mut rate = current
            .as_ref()
            .filter(|progress| progress.stage == "uploading")
            .map(|progress| progress.rate.clone())
            .unwrap_or_default();
        if stage == "uploading" {
            rate.observe(Instant::now(), sent);
        }
        *current = Some(Progress {
            operation_id: self.operation_id,
            stage,
            sent,
            total,
            error,
            failed_stage: None,
            bytes_per_second: None,
            rate,
        });
    }
    fn fail(&self, error: &str) {
        if let Some(progress) = self.state.0.lock().unwrap().as_mut() {
            progress.failed_stage = Some(progress.stage.to_owned());
            progress.error = Some(format!("{}: {error}", progress.stage));
            progress.stage = "error";
        }
    }
    pub fn finish(mut self, result: &Result<String, String>) {
        match result {
            Ok(_) => self.update("ready", 0, 0, None),
            Err(error) => self.fail(error),
        }
        self.finished = true;
    }
}
impl Drop for Attempt {
    fn drop(&mut self) {
        if !self.finished {
            self.fail("Installation interrupted. Retry when the connection is available.");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn progress_is_owned_and_interrupted_attempts_are_visible() {
        let state = Arc::new(State::default());
        let attempt = state.attempt();
        attempt.update("uploading", 32768, 100000, None);
        assert_eq!(state.snapshot().unwrap().sent, 32768);
        attempt.finish(&Err("transfer stalled".into()));
        let progress = state.snapshot().unwrap();
        assert_eq!(progress.failed_stage.as_deref(), Some("uploading"));
        assert!(
            progress
                .error
                .unwrap()
                .contains("uploading: transfer stalled")
        );
        state.attempt().finish(&Ok(String::new()));
        assert_eq!(state.snapshot().unwrap().stage, "ready");
        drop(state.attempt());
        assert!(
            state
                .snapshot()
                .unwrap()
                .error
                .unwrap()
                .contains("interrupted")
        );
        assert!(State::default().snapshot().is_none());
    }
    #[test]
    fn upload_speed_tracks_recent_bytes_and_decays_when_stalled() {
        let start = Instant::now();
        let mut rate = UploadRate::default();
        rate.observe(start, 0);
        assert_eq!(rate.speed(start), None);
        rate.observe(start + Duration::from_secs(1), 1024);
        assert_eq!(rate.speed(start + Duration::from_secs(1)), Some(1024.));
        rate.observe(start + Duration::from_secs(2), 4096);
        assert_eq!(rate.speed(start + Duration::from_secs(2)), Some(2048.));
        assert_eq!(rate.speed(start + Duration::from_secs(7)), Some(0.));
        rate.observe(start + Duration::from_secs(8), 5120);
        assert!(rate.speed(start + Duration::from_secs(8)).unwrap() > 0.);
    }
}
