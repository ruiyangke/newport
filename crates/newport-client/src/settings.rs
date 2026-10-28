//! Session-owned Settings requests. Navigation never releases write ownership.
//!
//! Reads are independently scheduled and retain their last accepted value.
//! Mutations invalidate older reads without pretending to cancel backend work.
use crate::{backend::Backend, resources::Resource};
use serde_json::{Value, json};
use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, mpsc},
    thread,
    time::{Duration, Instant},
};

const STARTUP: &str = "startup";
const CACHE: &str = "cache";
const UPDATE: &str = "update";
const STARTUP_WRITE: &str = "startup-write";
const CACHE_CLEAR: &str = "cache-clear";
const UPDATE_CHECK: &str = "update-check";
const UPDATE_INSTALL: &str = "update-install";
const UPDATE_INTERVAL: Duration = Duration::from_secs(3);
const CACHE_INTERVAL: Duration = Duration::from_secs(10);

type Outcome = Result<Value, String>;
type Dispatch = dyn Fn(&'static str, Value) -> Result<mpsc::Receiver<Outcome>, String>;

struct Pending {
    receiver: mpsc::Receiver<Outcome>,
    revision: u64,
    valid: bool,
}

#[derive(Default, Debug, PartialEq, Eq)]
pub struct Effects {
    pub changed: bool,
    pub metrics_cleared: bool,
    pub update_ready: Option<String>,
}

/// Keep one controller for the desktop session, including while Settings is hidden.
pub struct Settings {
    resources: HashMap<&'static str, Resource>,
    // One physical worker per key, including logically invalidated reads.
    // Check and install are mutually exclusive: at most six workers exist.
    pending: HashMap<&'static str, Pending>,
    wanted_reads: HashSet<&'static str>,
    notified_versions: HashSet<String>,
    dispatch: Box<Dispatch>,
    now: Box<dyn Fn() -> Instant>,
    next_revision: u64,
    changed: bool,
}

impl Settings {
    pub fn new(backend: Backend) -> Self {
        let backend = Arc::new(backend);
        Self::with_dispatch(
            Box::new(move |method, args| {
                let (sender, receiver) = mpsc::sync_channel(1);
                let backend = backend.clone();
                thread::Builder::new()
                    .name(format!("newport-settings-{method}"))
                    .spawn(move || {
                        let outcome = guarded_call(|| {
                            backend.call_with_timeout(method, args, request_timeout(method))
                        });
                        let _ = sender.send(outcome);
                    })
                    .map_err(|error| format!("Could not start Settings request: {error}"))?;
                Ok(receiver)
            }),
            Box::new(Instant::now),
        )
    }

    fn with_dispatch(dispatch: Box<Dispatch>, now: Box<dyn Fn() -> Instant>) -> Self {
        Self {
            resources: HashMap::new(),
            pending: HashMap::new(),
            wanted_reads: HashSet::new(),
            notified_versions: HashSet::new(),
            dispatch,
            now,
            next_revision: 0,
            changed: false,
        }
    }

    /// Keys: startup, cache, update, startup-write, cache-clear, update-check,
    /// update-install. Mutation errors remain available until an explicit retry.
    pub fn resource(&self, key: &str) -> Option<&Resource> {
        self.resources.get(key)
    }

    pub fn refresh(&mut self) {
        for key in [STARTUP, CACHE, UPDATE] {
            self.read(key, true);
        }
    }

    pub fn retry_startup(&mut self) {
        self.read(STARTUP, true);
    }

    pub fn retry_cache(&mut self) {
        self.read(CACHE, true);
    }

    pub fn set_launch_at_login(&mut self, enabled: bool) -> bool {
        let Some(value) = self.resource(STARTUP).map(|row| &row.value) else {
            return false;
        };
        if value["available"] != true
            || value["enabled"] == enabled
            || self.pending.contains_key(STARTUP_WRITE)
        {
            return false;
        }
        self.invalidate_read(STARTUP);
        self.launch(
            STARTUP_WRITE,
            "set_launch_at_login",
            json!({"enabled":enabled}),
            true,
        )
    }

    pub fn clear_cache(&mut self) -> bool {
        let samples = self
            .resource(CACHE)
            .and_then(|row| row.value["samples"].as_u64());
        let failed = self
            .resource(CACHE_CLEAR)
            .is_some_and(|row| row.error.is_some());
        if self.pending.contains_key(CACHE_CLEAR)
            || samples.is_none()
            || (samples == Some(0) && !failed)
        {
            return false;
        }
        self.invalidate_read(CACHE);
        self.launch(CACHE_CLEAR, "clear_metrics_cache", json!({}), true)
    }

    pub fn check_updates(&mut self) -> bool {
        if !self.updates_available()
            || !matches!(self.update_phase(), Some("idle" | "current" | "error"))
        {
            return false;
        }
        self.launch(UPDATE_CHECK, "check_for_updates", json!({}), true)
    }

    /// The UI owns restart confirmation; call only after the user accepts it.
    pub fn install_update(&mut self) -> bool {
        if !self.updates_available() || self.update_phase() != Some("ready") {
            return false;
        }
        let Some(version) = self
            .resource(UPDATE)
            .and_then(|row| row.value["version"].as_str())
            .filter(|version| !version.is_empty())
            .map(str::to_owned)
        else {
            return false;
        };
        self.launch(
            UPDATE_INSTALL,
            "install_update",
            json!({"version":version}),
            true,
        )
    }

    fn updates_available(&self) -> bool {
        !self.pending.contains_key(UPDATE_CHECK)
            && !self.pending.contains_key(UPDATE_INSTALL)
            && self
                .resource(UPDATE)
                .is_some_and(|row| row.value["enabled"] == true)
    }

    fn update_phase(&self) -> Option<&str> {
        self.resource(UPDATE)?.value["phase"].as_str()
    }

    /// Drains completions even when hidden. Status polling is global; cache
    /// polling runs only on the active Settings page. Mutations never auto-retry.
    pub fn poll(&mut self, active: bool) -> Effects {
        let now = (self.now)();
        let mut effects = Effects::default();
        // Process writes before reads so a settled operation invalidates an
        // older status response even if both completed before this poll.
        for key in [
            STARTUP_WRITE,
            CACHE_CLEAR,
            UPDATE_CHECK,
            UPDATE_INSTALL,
            STARTUP,
            CACHE,
            UPDATE,
        ] {
            let result = self.pending.get(key).and_then(|pending| match pending.receiver.try_recv() {
                Ok(result) => Some(result),
                Err(mpsc::TryRecvError::Empty) => None,
                Err(mpsc::TryRecvError::Disconnected) => Some(Err(
                    "Settings worker stopped before confirming the result. Refresh before trying again.".into(),
                )),
            });
            if let Some(result) = result {
                let pending = self.pending.remove(key).expect("completion has an owner");
                if pending.valid {
                    self.accept(key, pending.revision, result, now, &mut effects);
                }
            }
        }

        for key in [STARTUP, CACHE, UPDATE] {
            if self.wanted_reads.contains(key) {
                self.read(key, false);
            }
        }
        if self.due(UPDATE, now, UPDATE_INTERVAL) {
            self.read(UPDATE, false);
        }
        if active {
            if self.resource(STARTUP).is_none() {
                self.read(STARTUP, false);
            }
            if self.due(CACHE, now, CACHE_INTERVAL) {
                self.read(CACHE, false);
            }
        }
        effects.changed = std::mem::take(&mut self.changed);
        effects
    }

    fn due(&self, key: &str, now: Instant, interval: Duration) -> bool {
        self.resource(key).is_none_or(|row| {
            row.finished_at
                .is_some_and(|at| now.saturating_duration_since(at) >= interval)
        })
    }

    fn read(&mut self, key: &'static str, manual: bool) {
        let (method, mutation) = match key {
            STARTUP => ("get_startup_settings", Some(STARTUP_WRITE)),
            CACHE => ("get_metrics_cache", Some(CACHE_CLEAR)),
            UPDATE => ("update_status", None),
            _ => unreachable!("fixed Settings read key"),
        };
        if mutation.is_some_and(|key| self.pending.contains_key(key)) {
            self.wanted_reads.insert(key);
            return;
        }
        if let Some(pending) = self.pending.get(key) {
            if !pending.valid {
                self.wanted_reads.insert(key);
            } else if manual {
                let row = self
                    .resources
                    .get_mut(key)
                    .expect("pending read has a resource");
                self.changed |= !row.manual_pending;
                row.manual_pending = true;
            }
            return;
        }
        self.wanted_reads.remove(key);
        self.launch(key, method, json!({}), manual);
    }

    fn launch(
        &mut self,
        key: &'static str,
        method: &'static str,
        args: Value,
        manual: bool,
    ) -> bool {
        if self.pending.contains_key(key) {
            return false;
        }
        self.next_revision += 1;
        let revision = self.next_revision;
        let row = self.resources.entry(key).or_default();
        let previous = (read_activity(row), row.error.clone());
        row.pending = true;
        row.manual_pending = manual;
        if is_write(key) || row.updated_at.is_none() {
            row.error = None;
        }
        self.changed |= is_write(key) || previous != (read_activity(row), row.error.clone());
        match (self.dispatch)(method, args) {
            Ok(receiver) => {
                self.pending.insert(
                    key,
                    Pending {
                        receiver,
                        revision,
                        valid: true,
                    },
                );
                true
            }
            Err(error) => {
                self.accept(
                    key,
                    revision,
                    Err(error),
                    (self.now)(),
                    &mut Effects::default(),
                );
                false
            }
        }
    }

    fn invalidate_read(&mut self, key: &'static str) {
        if let Some(pending) = self.pending.get_mut(key) {
            pending.valid = false;
        }
        if let Some(row) = self.resources.get_mut(key) {
            row.pending = false;
            row.manual_pending = false;
        }
    }

    fn finish(&mut self, key: &'static str, revision: u64, result: Outcome, now: Instant) {
        let row = self.resources.entry(key).or_default();
        let previous = (row.presentation_revision, read_activity(row));
        row.finish_read(revision, result, now, None);
        self.changed |=
            is_write(key) || previous != (row.presentation_revision, read_activity(row));
    }

    fn accept(
        &mut self,
        key: &'static str,
        revision: u64,
        result: Outcome,
        now: Instant,
        effects: &mut Effects,
    ) {
        match key {
            STARTUP_WRITE => {
                self.invalidate_read(STARTUP);
                if let Ok(value) = &result {
                    self.finish(STARTUP, revision, Ok(value.clone()), now);
                } else {
                    self.wanted_reads.insert(STARTUP);
                }
            }
            CACHE_CLEAR => {
                self.invalidate_read(CACHE);
                if let Ok(value) = &result {
                    self.finish(CACHE, revision, Ok(value.clone()), now);
                    effects.metrics_cleared = true;
                }
                self.wanted_reads.insert(CACHE);
            }
            UPDATE_CHECK | UPDATE_INSTALL => {
                self.invalidate_read(UPDATE);
                self.wanted_reads.insert(UPDATE);
            }
            UPDATE => {
                if let Ok(value) = &result
                    && value["enabled"] == true
                    && value["phase"] == "ready"
                    && let Some(version) = value["version"]
                        .as_str()
                        .filter(|version| !version.is_empty())
                    && self.notified_versions.insert(version.to_owned())
                {
                    effects.update_ready = Some(version.to_owned());
                }
            }
            _ => {}
        }
        self.finish(key, revision, result, now);
    }
}

fn read_activity(row: &Resource) -> bool {
    // Cached success stays visible during a background read. A retained error
    // exposes a Retry button whose enabled state follows the pending request.
    row.display_pending() || row.error.is_some() && row.pending
}

fn is_write(key: &str) -> bool {
    matches!(
        key,
        STARTUP_WRITE | CACHE_CLEAR | UPDATE_CHECK | UPDATE_INSTALL
    )
}

fn request_timeout(method: &str) -> Duration {
    Duration::from_secs(match method {
        "clear_metrics_cache" => 60,
        // Covers the backend's manifest and download deadlines.
        "check_for_updates" => 660,
        "install_update" => 600,
        _ => 30,
    })
}

fn guarded_call(call: impl FnOnce() -> Outcome) -> Outcome {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(call)).unwrap_or_else(|_| {
        Err(
            "Settings worker stopped before confirming the result. Refresh before trying again."
                .into(),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        cell::{Cell, RefCell},
        rc::Rc,
    };

    struct Request {
        method: &'static str,
        args: Value,
        sender: Option<mpsc::SyncSender<Outcome>>,
    }

    struct Harness {
        settings: Settings,
        requests: Rc<RefCell<Vec<Request>>>,
        now: Rc<Cell<Instant>>,
    }

    impl Harness {
        fn new() -> Self {
            let requests = Rc::new(RefCell::new(Vec::new()));
            let outgoing = requests.clone();
            let now = Rc::new(Cell::new(Instant::now()));
            let clock = now.clone();
            let settings = Settings::with_dispatch(
                Box::new(move |method, args| {
                    let (sender, receiver) = mpsc::sync_channel(1);
                    outgoing.borrow_mut().push(Request {
                        method,
                        args,
                        sender: Some(sender),
                    });
                    Ok(receiver)
                }),
                Box::new(move || clock.get()),
            );
            Self {
                settings,
                requests,
                now,
            }
        }

        fn ready() -> Self {
            let mut h = Self::new();
            h.settings.refresh();
            h.reply("get_startup_settings", Ok(startup(false)));
            h.reply("get_metrics_cache", Ok(cache(12)));
            h.reply("update_status", Ok(update("idle", None)));
            h.settings.poll(true);
            h
        }

        fn advance(&self, seconds: u64) {
            self.now.set(self.now.get() + Duration::from_secs(seconds));
        }

        fn count(&self, method: &str) -> usize {
            self.requests
                .borrow()
                .iter()
                .filter(|request| request.method == method)
                .count()
        }

        fn reply(&self, method: &str, outcome: Outcome) {
            self.requests
                .borrow_mut()
                .iter_mut()
                .find(|request| request.method == method && request.sender.is_some())
                .expect("expected pending request")
                .sender
                .take()
                .unwrap()
                .send(outcome)
                .unwrap();
        }

        fn disconnect(&self, method: &str) {
            self.requests
                .borrow_mut()
                .iter_mut()
                .find(|request| request.method == method && request.sender.is_some())
                .expect("expected pending request")
                .sender
                .take();
        }

        fn resource(&self, key: &str) -> &Resource {
            self.settings.resource(key).unwrap()
        }
    }

    fn startup(enabled: bool) -> Value {
        json!({"available":true,"enabled":enabled})
    }

    fn cache(samples: u64) -> Value {
        json!({"samples":samples,"bytes":samples * 80})
    }

    fn update(phase: &str, version: Option<&str>) -> Value {
        json!({"enabled":true,"phase":phase,"version":version,"currentVersion":"0.3.0"})
    }

    #[test]
    fn identical_background_readings_advance_freshness_without_presentation_changes() {
        let mut h = Harness::ready();
        let data_revision = h.resource(CACHE).data_revision;
        let presentation_revision = h.resource(CACHE).presentation_revision;
        let mut revision = h.resource(CACHE).revision;
        let mut observed = h.resource(CACHE).updated_at;
        for _ in 0..3 {
            h.advance(10);
            assert!(
                !h.settings.poll(true).changed,
                "cached background admission is quiet"
            );
            h.reply("get_metrics_cache", Ok(cache(12)));
            h.reply("update_status", Ok(update("idle", None)));
            assert!(
                !h.settings.poll(true).changed,
                "identical completion is quiet"
            );
            assert!(h.resource(CACHE).revision > revision);
            assert!(h.resource(CACHE).updated_at > observed);
            assert_eq!(h.resource(CACHE).data_revision, data_revision);
            assert_eq!(
                h.resource(CACHE).presentation_revision,
                presentation_revision
            );
            revision = h.resource(CACHE).revision;
            observed = h.resource(CACHE).updated_at;
        }
        assert_eq!(h.count("get_metrics_cache"), 4);
        assert_eq!(h.count("update_status"), 4);
    }

    #[test]
    fn retry_activity_and_same_value_recovery_remain_visible() {
        let mut h = Harness::ready();
        h.advance(10);
        assert!(!h.settings.poll(true).changed);
        h.reply("get_metrics_cache", Err("temporary failure".into()));
        h.reply("update_status", Ok(update("idle", None)));
        assert!(h.settings.poll(true).changed);
        let observed = h.resource(CACHE).updated_at;
        let data_revision = h.resource(CACHE).data_revision;
        h.advance(10);
        assert!(
            h.settings.poll(true).changed,
            "retry disables the error action"
        );
        assert_eq!(
            h.resource(CACHE).error.as_deref(),
            Some("temporary failure")
        );
        h.reply("get_metrics_cache", Err("temporary failure".into()));
        h.reply("update_status", Ok(update("idle", None)));
        assert!(
            h.settings.poll(true).changed,
            "failed retry re-enables the action"
        );
        assert_eq!(h.resource(CACHE).updated_at, observed);
        h.settings.retry_cache();
        assert!(h.settings.poll(true).changed);
        h.reply("get_metrics_cache", Ok(cache(12)));
        assert!(
            h.settings.poll(true).changed,
            "same data clears the retained error"
        );
        assert!(h.resource(CACHE).error.is_none());
        assert_eq!(h.resource(CACHE).data_revision, data_revision);
    }

    #[test]
    fn explicit_refresh_and_repeated_write_receipts_are_not_suppressed() {
        let mut h = Harness::ready();
        h.settings.refresh();
        assert!(h.settings.poll(true).changed);
        h.reply("get_startup_settings", Ok(startup(false)));
        h.reply("get_metrics_cache", Ok(cache(12)));
        h.reply("update_status", Ok(update("idle", None)));
        assert!(
            h.settings.poll(true).changed,
            "manual refresh settles visibly"
        );
        for _ in 0..2 {
            assert!(h.settings.check_updates());
            assert!(h.settings.poll(false).changed);
            h.reply("check_for_updates", Ok(Value::Null));
            assert!(
                h.settings.poll(false).changed,
                "each acknowledged action settles"
            );
            h.reply("update_status", Ok(update("idle", None)));
            assert!(!h.settings.poll(false).changed);
        }
        assert_eq!(h.count("check_for_updates"), 2);
    }

    #[test]
    fn independent_reads_coalesce_without_discarding_freshness() {
        let mut h = Harness::ready();
        let revision = h.resource(CACHE).revision;
        let updated_at = h.resource(CACHE).updated_at;
        h.advance(10);
        h.settings.poll(true);
        assert!(!h.resource(CACHE).manual_pending);
        h.settings.refresh();
        h.settings.refresh();
        assert_eq!(h.count("get_metrics_cache"), 2);
        assert_eq!(h.count("update_status"), 2);
        assert_eq!(h.count("get_startup_settings"), 2);
        assert!(h.resource(CACHE).manual_pending);
        assert_eq!(h.resource(CACHE).revision, revision);
        assert_eq!(h.resource(CACHE).updated_at, updated_at);

        h.reply("get_metrics_cache", Err("connection closed".into()));
        h.settings.poll(true);
        assert_eq!(h.resource(CACHE).value, cache(12));
        assert_eq!(h.resource(CACHE).updated_at, updated_at);
        assert_eq!(
            h.resource(CACHE).error.as_deref(),
            Some("connection closed")
        );
        h.settings.retry_cache();
        assert_eq!(h.count("get_metrics_cache"), 3);
        assert_eq!(
            h.resource(CACHE).error.as_deref(),
            Some("connection closed")
        );
        h.reply("get_metrics_cache", Ok(cache(13)));
        h.settings.poll(true);
        assert_eq!(h.resource(CACHE).value, cache(13));
        assert!(h.resource(CACHE).error.is_none());
    }

    #[test]
    fn writes_are_deduplicated_across_navigation_and_independent_sections() {
        let mut h = Harness::ready();
        assert!(!h.settings.set_launch_at_login(false));
        assert!(h.settings.set_launch_at_login(true));
        assert!(!h.settings.set_launch_at_login(true));
        assert!(h.settings.clear_cache());
        assert!(!h.settings.clear_cache());
        assert!(h.settings.check_updates());
        assert!(!h.settings.check_updates());
        h.advance(10);
        h.settings.poll(false);
        h.settings.refresh();
        h.settings.poll(true);
        assert!(!h.settings.set_launch_at_login(true));
        assert!(!h.settings.clear_cache());
        assert!(!h.settings.check_updates());
        assert_eq!(h.count("set_launch_at_login"), 1);
        assert_eq!(h.count("clear_metrics_cache"), 1);
        assert_eq!(h.count("check_for_updates"), 1);
        assert_eq!(h.count("get_startup_settings"), 1);
        assert_eq!(h.count("get_metrics_cache"), 1);
        assert_eq!(h.count("update_status"), 2);
        assert!(h.resource(STARTUP_WRITE).pending);
        let requests = h.requests.borrow();
        assert_eq!(
            requests
                .iter()
                .find(|r| r.method == "set_launch_at_login")
                .unwrap()
                .args,
            json!({"enabled":true})
        );
    }

    #[test]
    fn startup_write_wins_over_old_reads_in_both_completion_orders() {
        for read_first in [false, true] {
            let mut h = Harness::ready();
            h.settings.retry_startup();
            assert!(h.settings.set_launch_at_login(true));
            h.reply("get_startup_settings", Ok(startup(false)));
            if read_first {
                h.settings.poll(false);
                assert_eq!(h.resource(STARTUP).value, startup(false));
                assert!(h.resource(STARTUP_WRITE).pending);
            }
            h.reply("set_launch_at_login", Ok(startup(true)));
            h.settings.poll(false);
            assert_eq!(h.resource(STARTUP).value, startup(true));
            assert!(!h.resource(STARTUP_WRITE).pending);
            assert_eq!(h.count("get_startup_settings"), 2);
        }
    }

    #[test]
    fn invalidated_read_stays_physically_owned_until_it_returns() {
        let mut h = Harness::ready();
        h.settings.retry_startup();
        assert!(h.settings.set_launch_at_login(true));
        h.reply("set_launch_at_login", Ok(startup(true)));
        h.settings.poll(false);
        h.settings.retry_startup();
        for _ in 0..20 {
            h.advance(10);
            h.settings.refresh();
            h.settings.poll(true);
        }
        assert_eq!(h.count("get_startup_settings"), 2);
        assert_eq!(h.resource(STARTUP).value, startup(true));
        assert!(h.settings.pending.len() <= 6);
        h.reply("get_startup_settings", Ok(startup(false)));
        h.settings.poll(false);
        assert_eq!(h.resource(STARTUP).value, startup(true));
        assert_eq!(h.count("get_startup_settings"), 3);
        h.reply("get_startup_settings", Ok(startup(true)));
        h.settings.poll(false);
        assert!(!h.resource(STARTUP).pending);
    }

    #[test]
    fn startup_failure_refetches_without_retrying_mutation() {
        let mut h = Harness::ready();
        assert!(h.settings.set_launch_at_login(true));
        h.reply("set_launch_at_login", Err("connection closed".into()));
        h.settings.poll(false);
        assert_eq!(h.count("get_startup_settings"), 2);
        assert_eq!(h.count("set_launch_at_login"), 1);
        assert_eq!(
            h.resource(STARTUP_WRITE).error.as_deref(),
            Some("connection closed")
        );
        // The server may have applied the write before transport failure.
        h.reply("get_startup_settings", Ok(startup(true)));
        h.advance(10);
        h.settings.poll(true);
        assert_eq!(h.resource(STARTUP).value, startup(true));
        assert_eq!(h.count("set_launch_at_login"), 1);
        assert!(!h.settings.set_launch_at_login(true));
    }

    #[test]
    fn cache_clear_emits_once_and_refetches_without_accepting_stale_cache() {
        let mut h = Harness::ready();
        h.settings.retry_cache();
        assert!(h.settings.clear_cache());
        h.reply("clear_metrics_cache", Ok(cache(0)));
        let effects = h.settings.poll(false);
        assert!(effects.metrics_cleared);
        assert_eq!(h.resource(CACHE).value, cache(0));
        assert_eq!(h.count("get_metrics_cache"), 2);
        assert!(!h.settings.poll(false).metrics_cleared);
        h.reply("get_metrics_cache", Ok(cache(12)));
        assert!(!h.settings.poll(false).metrics_cleared);
        assert_eq!(h.resource(CACHE).value, cache(0));
        assert_eq!(h.count("get_metrics_cache"), 3);
        assert!(!h.settings.clear_cache());
        h.reply("get_metrics_cache", Ok(cache(1)));
        h.settings.poll(false);
        assert_eq!(h.resource(CACHE).value, cache(1));
    }

    #[test]
    fn failed_cache_clear_refetches_and_allows_explicit_retry_even_if_empty() {
        let mut h = Harness::ready();
        assert!(h.settings.clear_cache());
        h.reply("clear_metrics_cache", Err("clear interrupted".into()));
        assert!(!h.settings.poll(false).metrics_cleared);
        h.reply("get_metrics_cache", Ok(cache(0)));
        h.settings.poll(false);
        assert_eq!(h.count("clear_metrics_cache"), 1);
        assert!(h.settings.clear_cache());
        assert_eq!(h.count("clear_metrics_cache"), 2);
    }

    #[test]
    fn long_update_check_does_not_block_global_status_or_active_cache_polling() {
        let mut h = Harness::ready();
        assert!(h.settings.check_updates());
        h.advance(3);
        h.settings.poll(false);
        assert_eq!(h.count("update_status"), 2);
        h.reply("update_status", Ok(update("downloading", Some("0.4.0"))));
        h.settings.poll(false);
        h.advance(10);
        h.settings.poll(false);
        assert_eq!(h.count("update_status"), 3);
        assert_eq!(h.count("get_metrics_cache"), 1);
        h.settings.poll(true);
        assert_eq!(h.count("get_metrics_cache"), 2);
        assert!(h.resource(UPDATE_CHECK).pending);
        assert!(!h.settings.install_update());
        // Both completions arrive before the next poll; writes must win.
        h.reply("check_for_updates", Ok(Value::Null));
        h.reply("update_status", Ok(update("downloading", Some("0.4.0"))));
        h.settings.poll(false);
        assert_eq!(h.count("update_status"), 4);
        assert_eq!(h.resource(UPDATE).value["phase"], "downloading");
        h.reply("update_status", Ok(update("ready", Some("0.4.0"))));
        assert_eq!(
            h.settings.poll(false).update_ready.as_deref(),
            Some("0.4.0")
        );
        assert!(h.settings.install_update());
        assert_eq!(
            h.requests
                .borrow()
                .iter()
                .find(|request| request.method == "install_update")
                .unwrap()
                .args,
            json!({"version":"0.4.0"})
        );
        assert!(!h.settings.install_update());
        assert!(!h.settings.check_updates());
        h.reply("install_update", Err("installer failed".into()));
        h.settings.poll(false);
        assert_eq!(h.count("update_status"), 5);
        assert_eq!(h.count("install_update"), 1);
        assert_eq!(
            h.resource(UPDATE_INSTALL).error.as_deref(),
            Some("installer failed")
        );
    }

    #[test]
    fn ready_notification_is_once_per_version_and_independent_of_navigation() {
        let mut h = Harness::new();
        assert!(!h.settings.poll(false).update_ready.is_some());
        assert_eq!(h.count("update_status"), 1);
        assert_eq!(h.count("get_metrics_cache"), 0);
        for (phase, version, expected) in [
            ("ready", Some("0.4.0"), Some("0.4.0")),
            ("ready", Some("0.4.0"), None),
            ("error", Some("0.4.0"), None),
            ("ready", Some("0.4.0"), None),
            ("ready", Some("0.5.0"), Some("0.5.0")),
        ] {
            h.reply("update_status", Ok(update(phase, version)));
            assert_eq!(h.settings.poll(false).update_ready.as_deref(), expected);
            h.advance(3);
            assert!(h.settings.poll(false).update_ready.is_none());
        }
    }

    #[test]
    fn disconnected_workers_settle_and_failed_reads_retry_on_normal_interval() {
        let mut h = Harness::ready();
        let updated_at = h.resource(UPDATE).updated_at;
        h.advance(10);
        h.settings.poll(true);
        h.disconnect("update_status");
        h.disconnect("get_metrics_cache");
        h.settings.poll(true);
        assert!(!h.resource(UPDATE).pending);
        assert!(!h.resource(CACHE).pending);
        assert!(h.resource(UPDATE).error.is_some());
        assert_eq!(h.resource(UPDATE).updated_at, updated_at);
        h.advance(2);
        h.settings.poll(true);
        assert_eq!(h.count("update_status"), 2);
        h.advance(1);
        h.settings.poll(true);
        assert_eq!(h.count("update_status"), 3);
        assert_eq!(h.count("get_metrics_cache"), 2);
        h.advance(7);
        h.settings.poll(true);
        assert_eq!(h.count("update_status"), 3);
        assert_eq!(h.count("get_metrics_cache"), 3);
        h.reply("update_status", Ok(update("current", None)));
        h.settings.poll(false);
        assert!(h.resource(UPDATE).error.is_none());
    }

    #[test]
    fn capabilities_and_backend_phases_guard_mutations() {
        let mut h = Harness::new();
        assert!(!h.settings.set_launch_at_login(true));
        assert!(!h.settings.clear_cache());
        assert!(!h.settings.check_updates());
        assert!(!h.settings.install_update());
        h.settings.refresh();
        h.reply(
            "get_startup_settings",
            Ok(json!({"available":false,"enabled":false})),
        );
        h.reply("get_metrics_cache", Ok(cache(0)));
        h.reply(
            "update_status",
            Ok(json!({"enabled":false,"phase":"ready"})),
        );
        h.settings.poll(false);
        assert!(!h.settings.set_launch_at_login(true));
        assert!(!h.settings.clear_cache());
        assert!(!h.settings.install_update());
        for phase in ["checking", "downloading", "installing", "unknown"] {
            h.advance(3);
            h.settings.poll(false);
            h.reply("update_status", Ok(update(phase, None)));
            h.settings.poll(false);
            assert!(!h.settings.check_updates());
            assert!(!h.settings.install_update());
        }
    }

    #[test]
    fn launch_failure_settles_state_without_automatically_repeating_writes() {
        let mut h = Harness::ready();
        let attempts = Rc::new(Cell::new(0));
        let count = attempts.clone();
        h.settings.dispatch = Box::new(move |_, _| {
            count.set(count.get() + 1);
            Err("worker could not start".into())
        });
        assert!(!h.settings.clear_cache());
        assert!(!h.resource(CACHE_CLEAR).pending);
        assert_eq!(
            h.resource(CACHE_CLEAR).error.as_deref(),
            Some("worker could not start")
        );
        h.settings.poll(false); // One cache status read after the failed write.
        assert_eq!(attempts.get(), 2);
        assert!(!h.settings.poll(false).changed);
        assert_eq!(attempts.get(), 2);
    }

    #[test]
    fn transport_panic_and_request_timeouts_are_bounded() {
        assert!(guarded_call(|| panic!("transport panicked")).is_err());
        assert_eq!(guarded_call(|| Ok(json!(true))), Ok(json!(true)));
        assert_eq!(
            request_timeout("get_startup_settings"),
            Duration::from_secs(30)
        );
        assert_eq!(
            request_timeout("set_launch_at_login"),
            Duration::from_secs(30)
        );
        assert_eq!(
            request_timeout("get_metrics_cache"),
            Duration::from_secs(30)
        );
        assert_eq!(request_timeout("update_status"), Duration::from_secs(30));
        assert_eq!(
            request_timeout("clear_metrics_cache"),
            Duration::from_secs(60)
        );
        assert_eq!(
            request_timeout("check_for_updates"),
            Duration::from_secs(660)
        );
        assert_eq!(request_timeout("install_update"), Duration::from_secs(600));
    }
}
