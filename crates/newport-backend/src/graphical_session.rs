//! Backend-owned display lifetime. GUI adapters implement the wire protocol;
//! consumers retain demand, never decide another consumer's transport lifetime.
pub mod launch;
pub mod transport;
use std::{
    collections::{BTreeMap, VecDeque},
    future::Future,
    pin::Pin,
    sync::{
        Arc, Mutex, Weak,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};
use tokio::{
    sync::{RwLock, RwLockReadGuard, RwLockWriteGuard, watch},
    task::AbortHandle,
};
#[cfg(test)]
use uuid::Uuid;

const LIMIT: usize = 4; // Bound overlap while pinned old builds drain.
const FINISHED_LIMIT: usize = 64; // Metadata only; never retain an old driver.
/// Endings kept per finished display: the requests its adapter still knows.
const ENDINGS_LIMIT: u64 = 64;
pub(crate) const ENDED: &str = "Needle's display on the server stopped. Open the app again.";
/// How often the server is asked who shows the account's display, while
/// another computer showed it last.
const VIEWER_CHECK: Duration = Duration::from_secs(5);
/// A display shown here again never knew the apps launched before another
/// computer showed it, so it can't quit them.
const QUIT_SHOWN_AGAIN: &str =
    "Quit it in its own window: another computer showed it since it opened here.";

pub use newport_protocol::desktop::applications::{Identity, Runtime};
fn validate_runtime(runtime: &Runtime) -> Result<(), String> {
    if runtime.instance.is_nil() || !newport_protocol::product::valid_build(&runtime.build_sha256) {
        return Err("Invalid graphical runtime identity".into());
    }
    Ok(())
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Demand {
    Clipboard,
    Launch,
    /// Someone uses a terminal on this computer, so it shows the account's
    /// display while that terminal is its shell's active one.
    ActiveTerminal,
}
impl Demand {
    fn index(self) -> usize {
        match self {
            Self::Clipboard => 0,
            Self::Launch => 1,
            Self::ActiveTerminal => 2,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DriverState {
    Ready {
        windows: u32,
        applications: u32,
    },
    Failed(String),
    /// Another computer took the account's display over and shows it now.
    /// Its apps keep running there.
    ShownElsewhere(launch::ShownOn),
    Closed,
}
/// Who shows the account's display, as the server reports it now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Live {
    Stopped,
    /// It runs, and no computer shows it.
    Unviewed,
    /// Another computer shows it.
    Viewed(launch::ShownOn),
    /// This computer shows it, as its own relay left from before a sleep
    /// does.
    Here,
}
/// The adapter owns protocol IO and reports resource state without a UI drain.
/// Publish application demand before completing a launch, including applications
/// that have not mapped a window yet. Keep counts current until those apps exit.
/// stop must fence new operations synchronously and request bounded cleanup.
pub trait Driver: Send + Sync + 'static {
    fn state(&self) -> watch::Receiver<DriverState>;
    /// Admit synchronously to this display only. Return Err only when the
    /// request was not queued. Before returning Ok, publish pending demand;
    /// retain that demand even if the reply future is never polled or dropped.
    /// The reply must not retain this driver or its transport. Serialize
    /// admission with stop, and classify lost acknowledgments as Unknown.
    fn launch(&self, target: launch::Target) -> Result<launch::Reply, String>;
    /// Submit an individually observable launch. These operations share the
    /// same admission/stop fence as launch; they never create another display.
    fn launch_application(&self, _target: launch::Target) -> Result<launch::Submitted, String> {
        Err("Individual application control is unavailable".into())
    }
    fn application_state(&self, _request: std::num::NonZeroU64) -> Result<launch::State, String> {
        Err("Individual application control is unavailable".into())
    }
    /// Why an application had no window, when the remote side said.
    fn application_ending(
        &self,
        _request: std::num::NonZeroU64,
    ) -> Result<Option<launch::Ending>, String> {
        Ok(None)
    }
    fn stop_application(&self, _request: std::num::NonZeroU64) -> Result<bool, String> {
        Err("Individual application control is unavailable".into())
    }
    /// The apps with windows on the display, by the desktop entry the display
    /// matched them to, whoever started them.
    fn applications(&self) -> Vec<launch::Shown> {
        Vec::new()
    }
    /// Someone used a terminal of this display's server here: what its
    /// programs copy reaches this computer while no display's window has
    /// focus.
    fn stand(&self) {}
    fn stop(&self);
}
/// Returned only after a validated GUI handshake and clipboard readiness.
/// Even rejected or late attachment results request cleanup when discarded.
pub struct Attached {
    pub driver: Arc<dyn Driver>,
}
impl Drop for Attached {
    fn drop(&mut self) {
        self.driver.stop();
    }
}
pub type AttachFuture = Pin<Box<dyn Future<Output = Result<Attached, String>> + Send>>;
pub type FactoryFuture = Pin<Box<dyn Future<Output = Result<Arc<dyn Factory>, String>> + Send>>;
pub trait Factory: Send + Sync + 'static {
    /// Check current admission even when an already-running display is reused
    /// and attach is never called. This must not perform IO or block.
    fn validate(&self) -> Result<(), String>;
    /// Dropping this future must cancel attachment, including any spawned IO.
    /// The supplied bridge owns exactly one already-admitted SSH channel;
    /// adapters must keep blocking reads/writes and finish off executor threads.
    /// The backend retains the registered worker and pinned transport lease.
    /// The relay shows the account's stable display, whose profile the
    /// adapter requests.
    fn attach(
        &self,
        channel: transport::Channel,
        identity: Identity,
        server: ServerLabel,
    ) -> AttachFuture;
}

/// The server a display belongs to. The desktop names the display's apps by
/// it, and keys anything it keeps for them by its stable id.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServerLabel {
    pub id: uuid::Uuid,
    pub name: String,
}

pub(crate) trait Fence: Send + Sync {
    fn validate(&self) -> Result<(), String>;
}
pub(crate) struct Prepared {
    pub identity: Identity,
    pub attached: Attached,
    pub fence: Box<dyn Fence>,
}
impl Prepared {
    fn validate(&self, runtime: &Runtime, generation: u64) -> Result<(), String> {
        self.fence.validate()?;
        if &self.identity.runtime != runtime
            || self.identity.display_generation != generation
            || self.identity.worker.is_nil()
        {
            return Err("Graphical session identity changed during preparation".into());
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Phase {
    Preparing,
    Ready,
    Failed(String),
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Snapshot {
    pub generation: u64,
    pub runtime: Runtime,
    pub phase: Phase,
    pub clipboard: u32,
    pub launches: u32,
    pub active_terminals: u32,
    pub windows: u32,
    pub applications: u32,
}
struct Entry {
    runtime: Runtime,
    phase: Phase,
    demand: [u32; 3],
    windows: u32,
    applications: u32,
    /// Session-local launch IDs are consecutive and never reused. Captured
    /// before releasing launch demand, including already-finished requests.
    launched: Arc<AtomicU64>,
    prepared: Option<Prepared>,
    updates: Option<watch::Receiver<DriverState>>,
    task: Option<AbortHandle>,
}
impl Entry {
    fn activity(&self) -> Result<(u32, u32), String> {
        let Some(updates) = &self.updates else {
            return Ok((0, 0));
        };
        match &*updates.borrow() {
            DriverState::Ready {
                windows,
                applications,
            } => Ok((*windows, *applications)),
            DriverState::Failed(error) => Err(error.clone()),
            DriverState::ShownElsewhere(_) | DriverState::Closed => Err(ENDED.into()),
        }
    }
    /// The computer showing its display now, when another took it over.
    fn shown_elsewhere(&self) -> Option<launch::ShownOn> {
        match &*self.updates.as_ref()?.borrow() {
            DriverState::ShownElsewhere(shown) => Some(shown.clone()),
            _ => None,
        }
    }
    fn validate(&self, generation: u64) -> Result<&Prepared, String> {
        if let Phase::Failed(error) = &self.phase {
            return Err(error.clone());
        }
        let prepared = self
            .prepared
            .as_ref()
            .ok_or("Graphical session is still preparing")?;
        prepared.validate(&self.runtime, generation)?;
        self.activity()?;
        Ok(prepared)
    }
    fn unused(&self) -> bool {
        self.demand == [0; 3] && self.windows == 0 && self.applications == 0
    }
}
impl Drop for Entry {
    fn drop(&mut self) {
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}
#[derive(Default)]
struct State {
    closed: bool,
    next: u64,
    entries: BTreeMap<u64, Entry>,
    finished: VecDeque<Finished>,
    /// The computer showing the account's display, from when it took the
    /// display over until a display is ready here again, or the server says
    /// it no longer shows it.
    elsewhere: Option<launch::ShownOn>,
    /// Counts changes to `elsewhere`, so an answer about an older one is
    /// dropped.
    elsewhere_changes: u64,
    /// When the server was last asked who shows the display, and whether an
    /// answer is still awaited.
    viewer_asked: Option<Instant>,
    viewer_asking: bool,
    /// The apps whose windows the display showed here last, kept while
    /// another computer shows it.
    applications: Vec<launch::Shown>,
}
/// A display retired after normal completion, with the endings its adapter
/// reported for its latest requests.
struct Finished {
    identity: Identity,
    last: u64,
    endings: Vec<(std::num::NonZeroU64, launch::Ending)>,
    apps: Apps,
}
/// Where the apps of a finished display are.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Apps {
    /// They ended with it.
    Ended,
    /// Another computer took it over: they run there.
    Elsewhere,
    /// This computer shows them again, on the display of this generation,
    /// and they go with it.
    Here(u64),
}
impl State {
    fn finished(&self, application: &launch::Application) -> Option<&Finished> {
        if self.closed {
            return None;
        }
        self.finished.iter().find(|finished| {
            finished.identity == application.display && application.request.get() <= finished.last
        })
    }
    fn completed(&self, application: &launch::Application) -> bool {
        self.finished(application).is_some()
    }
    /// Some once the application's display finished normally. The apps of
    /// one taken over are shown elsewhere until this computer shows them
    /// again.
    fn ending(&self, application: &launch::Application) -> Option<Option<launch::Ending>> {
        self.finished(application).map(|finished| {
            finished
                .endings
                .iter()
                .find(|(request, _)| *request == application.request)
                .map(|(_, ending)| ending.clone())
                .or_else(|| match finished.apps {
                    Apps::Elsewhere => self.elsewhere.clone().map(|shown| {
                        launch::Ending::ShownElsewhere(Box::new(
                            newport_protocol::desktop::applications::OptionalDetail {
                                detail: shown.computer,
                            },
                        ))
                    }),
                    Apps::Ended | Apps::Here(_) => None,
                })
        })
    }
    /// The display showing a taken-over application here again.
    fn shown_here(&self, application: &launch::Application) -> Option<u64> {
        match self.finished(application)?.apps {
            Apps::Here(generation) => Some(generation),
            Apps::Ended | Apps::Elsewhere => None,
        }
    }
    /// Moves the apps of displays in one place to another.
    fn move_apps(&mut self, from: Apps, to: Apps) {
        for finished in &mut self.finished {
            if finished.apps == from {
                finished.apps = to;
            }
        }
    }
    fn set_elsewhere(&mut self, shown: Option<launch::ShownOn>) {
        self.elsewhere = shown;
        self.elsewhere_changes = self.elsewhere_changes.wrapping_add(1);
    }
    /// Follows the display to where the server says it is shown, also when
    /// this computer hasn't shown it. Which apps have windows there is not
    /// known here, and once no computer shows it, the apps it took there
    /// read as ended.
    fn follow_viewer(&mut self, live: Live) -> bool {
        let Some(shown) = &self.elsewhere else {
            // Not while a display here shows it, or is about to.
            if !self.entries.is_empty() {
                return false;
            }
            let Live::Viewed(viewer) = live else {
                return false;
            };
            self.set_elsewhere(Some(viewer));
            self.applications.clear();
            return true;
        };
        match live {
            Live::Viewed(viewer) if viewer == *shown => return false,
            Live::Viewed(viewer) => self.set_elsewhere(Some(viewer)),
            Live::Here => return false,
            Live::Unviewed | Live::Stopped => {
                self.set_elsewhere(None);
                self.move_apps(Apps::Elsewhere, Apps::Ended);
            }
        }
        self.applications.clear();
        true
    }
    fn record_finished(&mut self, finished: Finished) {
        if self.finished.len() == FINISHED_LIMIT {
            self.finished.pop_front();
        }
        self.finished.push_back(finished);
    }
    fn retire_unused(&mut self, generation: u64) -> Option<Entry> {
        self.retire_unused_with(generation, Vec::new())
    }
    fn retire_unused_with(
        &mut self,
        generation: u64,
        endings: Vec<(std::num::NonZeroU64, launch::Ending)>,
    ) -> Option<Entry> {
        let entry = self.entries.get(&generation)?;
        if !entry.unused() {
            return None;
        }
        // Failure/disconnect must never become evidence that an app finished
        // normally. Only a valid idle display can produce this receipt.
        if let Ok(prepared) = entry.validate(generation) {
            let last = entry.launched.load(Ordering::Acquire);
            let identity = prepared.identity.clone();
            // The apps it showed again closed their windows with it.
            self.move_apps(Apps::Here(generation), Apps::Ended);
            self.applications.clear();
            if last > 0 {
                self.record_finished(Finished {
                    identity,
                    last,
                    endings,
                    apps: Apps::Ended,
                });
            }
        }
        self.entries.remove(&generation)
    }
}
struct Shared {
    state: Mutex<State>,
    changes: watch::Sender<u64>,
}
impl Default for Shared {
    fn default() -> Self {
        Self {
            state: Mutex::default(),
            changes: watch::channel(0).0,
        }
    }
}
impl Shared {
    fn notify(&self) {
        self.changes
            .send_modify(|revision| *revision = revision.wrapping_add(1));
    }
    fn release(&self, generation: u64, kind: Demand) {
        let (retired, error) = {
            let mut state = self
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let Some(entry) = state.entries.get_mut(&generation) else {
                return;
            };
            entry.demand[kind.index()] -= 1;
            if let Some(shown) = entry.shown_elsewhere() {
                drop(state);
                self.shown_elsewhere(generation, shown);
                return;
            }
            match entry.activity() {
                Ok((windows, applications)) => {
                    // An app may publish its first window immediately before
                    // its launch token drops, before our watcher gets to run.
                    entry.windows = windows;
                    entry.applications = applications;
                    (
                        if entry.unused() {
                            state.retire_unused(generation)
                        } else {
                            None
                        },
                        None,
                    )
                }
                Err(error) => (None, Some(error)),
            }
        };
        drop(retired); // Adapter stop callbacks never run under the owner lock.
        if let Some(error) = error {
            self.failed(generation, error);
        } else {
            self.notify();
        }
    }
    /// Another computer shows the display now. It ends here at once for every
    /// holder, as an ended display does, and its apps, with those it showed
    /// again, are recorded as running there rather than failed.
    fn shown_elsewhere(&self, generation: u64, shown: launch::ShownOn) {
        let retired = {
            let mut state = self
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let Some(entry) = state.entries.remove(&generation) else {
                return;
            };
            let last = entry.launched.load(Ordering::Acquire);
            if let Some(prepared) = entry.prepared.as_ref().filter(|_| last > 0) {
                let identity = prepared.identity.clone();
                state.record_finished(Finished {
                    identity,
                    last,
                    endings: Vec::new(),
                    apps: Apps::Elsewhere,
                });
            }
            state.move_apps(Apps::Here(generation), Apps::Elsewhere);
            state.set_elsewhere(Some(shown));
            entry
        };
        drop(retired);
        self.notify();
    }
    /// Records the apps the ready display of `generation` shows, asked of its
    /// adapter outside the owner lock.
    fn observe_applications(&self, generation: u64) {
        let driver = {
            let state = self
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let Some(entry) = state.entries.get(&generation) else {
                return;
            };
            match (&entry.phase, &entry.prepared) {
                (Phase::Ready, Some(prepared)) => prepared.attached.driver.clone(),
                _ => return,
            }
        };
        let mut applications = driver.applications();
        applications.sort_by(|a, b| a.app_id.cmp(&b.app_id));
        let changed = {
            let mut state = self
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if !state.entries.get(&generation).is_some_and(|entry| {
                entry.phase == Phase::Ready && entry.shown_elsewhere().is_none()
            }) || state.applications == applications
            {
                return;
            }
            state.applications = applications;
            true
        };
        if changed {
            self.notify();
        }
    }
    fn failed(&self, generation: u64, message: String) {
        let (prepared, task) = {
            let mut state = self
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let Some(entry) = state.entries.get_mut(&generation) else {
                return;
            };
            entry.phase = Phase::Failed(message);
            entry.windows = 0;
            entry.applications = 0;
            entry.updates = None;
            let taken = (entry.prepared.take(), entry.task.take());
            state.applications.clear();
            taken
        };
        if let Some(task) = task {
            task.abort();
        }
        drop(prepared);
        let retired = {
            let mut state = self
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if state.entries.get(&generation).is_some_and(Entry::unused) {
                state.entries.remove(&generation)
            } else {
                None
            }
        };
        drop(retired);
        self.notify();
    }
}
#[derive(Default)]
pub(crate) struct Slot {
    shared: Arc<Shared>,
    admission: RwLock<()>,
}
impl Slot {
    /// Check an exact ready display without creating demand or adopting a
    /// replacement. Clipboard control must validate before and after its IO.
    pub(crate) fn validate_identity(&self, identity: &Identity) -> Result<(), String> {
        let state = self
            .shared
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if state.closed {
            return Err(ENDED.into());
        }
        let entry = state
            .entries
            .get(&identity.display_generation)
            .ok_or(ENDED)?;
        let prepared = entry.validate(identity.display_generation)?;
        if prepared.identity != *identity {
            return Err("Graphical display identity changed".into());
        }
        Ok(())
    }
    pub(crate) async fn admission(&self) -> Result<RwLockReadGuard<'_, ()>, String> {
        let guard = self.admission.read().await;
        if self
            .shared
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .closed
        {
            return Err(ENDED.into());
        }
        Ok(guard)
    }
    /// Ends the display here if another computer took it over, before its
    /// watcher does, so its apps read as running there and not as failed.
    fn settle_takeover(&self, generation: u64) {
        let shown = self
            .shared
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .entries
            .get(&generation)
            .and_then(Entry::shown_elsewhere);
        if let Some(shown) = shown {
            self.shared.shown_elsewhere(generation, shown);
        }
    }
    fn completed(&self, application: &launch::Application) -> bool {
        self.settle_takeover(application.display.display_generation);
        self.shared
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .completed(application)
    }
    /// Some for a taken-over application this computer shows again: Ok
    /// while the display showing it is ready, which runs it without knowing
    /// it, and that display's failure once it is not.
    fn shown_again(&self, application: &launch::Application) -> Option<Result<(), String>> {
        let generation = self
            .shared
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .shown_here(application)?;
        self.settle_takeover(generation);
        let state = self
            .shared
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if state.shown_here(application) != Some(generation) {
            return None;
        }
        Some(match state.entries.get(&generation) {
            Some(entry) => entry.validate(generation).map(drop),
            None => Err(ENDED.into()),
        })
    }
    fn application_driver(
        &self,
        application: &launch::Application,
    ) -> Result<Option<Arc<dyn Driver>>, String> {
        self.settle_takeover(application.display.display_generation);
        let state = self
            .shared
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if state.completed(application) {
            return Ok(None);
        }
        let entry = state
            .entries
            .get(&application.display.display_generation)
            .ok_or(ENDED)?;
        let prepared = entry.validate(application.display.display_generation)?;
        if prepared.identity != application.display {
            return Err("Application display identity changed".into());
        }
        Ok(Some(prepared.attached.driver.clone()))
    }
    pub(crate) fn application_state(
        &self,
        application: &launch::Application,
    ) -> Result<launch::State, String> {
        if let Some(shown) = self.shown_again(application) {
            return shown.map(|()| launch::State::Running);
        }
        let Some(driver) = self.application_driver(application)? else {
            return Ok(launch::State::Ended);
        };
        let result = driver
            .application_state(application.request)
            .and_then(|value| self.validate_identity(&application.display).map(|()| value));
        // The last group may finish and retire the display during observation.
        if result.is_err() && self.completed(application) {
            return Ok(launch::State::Ended);
        }
        result
    }
    /// Kept after the display retires, for the latest requests it reported.
    pub(crate) fn application_ending(
        &self,
        application: &launch::Application,
    ) -> Result<Option<launch::Ending>, String> {
        if let Some(ending) = self
            .shared
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .ending(application)
        {
            return Ok(ending);
        }
        let Some(driver) = self.application_driver(application)? else {
            return Ok(self
                .shared
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .ending(application)
                .flatten());
        };
        let result = driver
            .application_ending(application.request)
            .and_then(|value| self.validate_identity(&application.display).map(|()| value));
        if result.is_err()
            && self.completed(application)
            && let Some(ending) = self
                .shared
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .ending(application)
        {
            return Ok(ending);
        }
        result
    }
    pub(crate) fn stop_application(
        &self,
        application: &launch::Application,
    ) -> Result<bool, String> {
        if let Some(shown) = self.shown_again(application) {
            shown?;
            return Err(QUIT_SHOWN_AGAIN.into());
        }
        // Clone under the ownership lock; the adapter serializes this action
        // with stop. No channel or lease can be opened by an old handle.
        let Some(driver) = self.application_driver(application)? else {
            return Ok(false);
        };
        let result = driver.stop_application(application.request);
        if result.is_err() && self.completed(application) {
            return Ok(false);
        }
        result
    }
    /// Someone used a terminal of this server here: the display shown here
    /// takes what its programs copy, when one is.
    pub(crate) fn stand(&self) {
        let driver = {
            let state = self
                .shared
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if state.closed {
                return;
            }
            state
                .entries
                .iter()
                .rev()
                .filter(|(_, entry)| entry.phase == Phase::Ready)
                .find_map(|(generation, entry)| entry.validate(*generation).ok())
                .map(|prepared| prepared.attached.driver.clone())
        };
        if let Some(driver) = driver {
            driver.stand();
        }
    }
    /// The account's display as this computer knows it: where it is shown
    /// when another computer took it over, and the apps it showed here last.
    pub(crate) fn display(&self) -> launch::Display {
        let state = self
            .shared
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if state.closed {
            return launch::Display::default();
        }
        launch::Display {
            shown_on: state.elsewhere.clone(),
            applications: state.applications.clone(),
        }
    }
    /// Starts asking the server who shows the account's display: while this
    /// computer doesn't show it, one question at a time, and at most every
    /// [`VIEWER_CHECK`].
    pub(crate) fn viewer_check(&self) -> Option<ViewerCheck> {
        let mut state = self
            .shared
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if state.closed
            || (state.elsewhere.is_none() && !state.entries.is_empty())
            || state.viewer_asking
            || state
                .viewer_asked
                .is_some_and(|asked| asked.elapsed() < VIEWER_CHECK)
        {
            return None;
        }
        state.viewer_asked = Some(Instant::now());
        state.viewer_asking = true;
        Some(ViewerCheck {
            shared: Arc::downgrade(&self.shared),
            elsewhere_changes: state.elsewhere_changes,
        })
    }
    /// What the server answered `check`, unless this computer learned where
    /// the display is shown since it asked.
    pub(crate) fn viewer_checked(&self, check: ViewerCheck, live: Live) {
        let changed = {
            let mut state = self
                .shared
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            state.elsewhere_changes == check.elsewhere_changes && state.follow_viewer(live)
        };
        drop(check);
        if changed {
            self.shared.notify();
        }
    }
    pub(crate) async fn maintenance(&self) -> RwLockWriteGuard<'_, ()> {
        self.admission.write().await
    }
    pub fn snapshot(&self) -> Vec<Snapshot> {
        self.shared
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .entries
            .iter()
            .map(|(&generation, entry)| Snapshot {
                generation,
                runtime: entry.runtime.clone(),
                phase: entry.phase.clone(),
                clipboard: entry.demand[0],
                launches: entry.demand[1],
                active_terminals: entry.demand[2],
                windows: entry.windows,
                applications: entry.applications,
            })
            .collect()
    }
    #[cfg(test)]
    pub async fn drained(&self) {
        let mut changes = self.shared.changes.subscribe();
        while !self
            .shared
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .entries
            .is_empty()
        {
            if changes.changed().await.is_err() {
                break;
            }
        }
    }
    pub fn close(&self) {
        self.shared
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .closed = true;
        self.end_all();
    }
    /// Ends every display now, whoever still holds it, and keeps accepting
    /// new ones.
    pub fn end_all(&self) {
        let retired = {
            let mut state = self
                .shared
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            state.finished.clear();
            state.set_elsewhere(None);
            std::mem::take(&mut state.entries)
        };
        drop(retired);
        self.shared.notify();
    }
    pub(crate) async fn acquire<F, Fut>(
        &self,
        runtime: Runtime,
        kind: Demand,
        start: F,
    ) -> Result<Lease, String>
    where
        F: FnOnce(u64) -> Fut + Send + 'static,
        Fut: Future<Output = Result<Prepared, String>> + Send + 'static,
    {
        validate_runtime(&runtime)?;
        let mut changes = self.shared.changes.subscribe();
        let (generation, create) = {
            let mut state = self
                .shared
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if state.closed {
                return Err(ENDED.into());
            }
            let found = state.entries.iter().find_map(|(&id, entry)| {
                (entry.runtime == runtime && !matches!(entry.phase, Phase::Failed(_))).then_some(id)
            });
            let generation = if let Some(id) = found {
                id
            } else {
                if state.entries.len() >= LIMIT {
                    return Err("Previous graphical sessions are still in use. Close one before starting another session.".into());
                }
                state.next = state
                    .next
                    .checked_add(1)
                    .ok_or("Graphical session generations exhausted")?;
                let id = state.next;
                state.entries.insert(
                    id,
                    Entry {
                        runtime: runtime.clone(),
                        phase: Phase::Preparing,
                        demand: [0; 3],
                        windows: 0,
                        applications: 0,
                        launched: Default::default(),
                        prepared: None,
                        updates: None,
                        task: None,
                    },
                );
                id
            };
            let entry = state.entries.get_mut(&generation).unwrap();
            entry.demand[kind.index()] = entry.demand[kind.index()]
                .checked_add(1)
                .ok_or("Too many graphical session consumers")?;
            (generation, found.is_none())
        };
        let lease = Lease {
            shared: Arc::downgrade(&self.shared),
            generation,
            kind,
        };
        self.shared.notify();
        if create {
            let weak = Arc::downgrade(&self.shared);
            let task = tokio::spawn(async move {
                let mut interrupted = Startup {
                    shared: weak.clone(),
                    generation,
                    armed: true,
                };
                let result = tokio::time::timeout(Duration::from_secs(30), start(generation))
                    .await
                    .map_err(|_| "Graphical session preparation timed out".to_owned())
                    .and_then(|result| result)
                    .and_then(|prepared| {
                        prepared.validate(&runtime, generation)?;
                        Ok(prepared)
                    });
                let Some(shared) = weak.upgrade() else { return };
                match result {
                    Err(error) => shared.failed(generation, error),
                    Ok(prepared) => {
                        let updates = prepared.attached.driver.state();
                        let mut state = shared
                            .state
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner);
                        if let Some(entry) = state.entries.get_mut(&generation) {
                            entry.updates = Some(updates.clone());
                            if let Ok((windows, applications)) = entry.activity() {
                                entry.windows = windows;
                                entry.applications = applications;
                            }
                            entry.prepared = Some(prepared);
                            entry.phase = Phase::Ready;
                            let watcher = tokio::spawn(watch_driver(weak, generation, updates));
                            entry.task = Some(watcher.abort_handle());
                            // This computer shows the account's display, and
                            // the apps another computer showed, again.
                            state.set_elsewhere(None);
                            state.move_apps(Apps::Elsewhere, Apps::Here(generation));
                        } else {
                            drop(state);
                            drop(prepared);
                            interrupted.armed = false;
                            return;
                        }
                        drop(state);
                        shared.notify();
                    }
                }
                interrupted.armed = false;
            });
            let mut state = self
                .shared
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if let Some(entry) = state.entries.get_mut(&generation) {
                // A fast attachment may already have installed its watcher.
                if entry.phase == Phase::Preparing {
                    entry.task = Some(task.abort_handle());
                }
            } else {
                task.abort();
            }
        }
        loop {
            match lease.phase()? {
                Phase::Ready => {
                    lease.identity()?;
                    return Ok(lease);
                }
                Phase::Failed(error) => return Err(error),
                Phase::Preparing => (),
            }
            changes.changed().await.map_err(|_| ENDED)?;
        }
    }
}
/// A question to the server about who shows the account's display. Dropped
/// unanswered, it lets the next one be asked.
pub(crate) struct ViewerCheck {
    shared: Weak<Shared>,
    elsewhere_changes: u64,
}
impl Drop for ViewerCheck {
    fn drop(&mut self) {
        if let Some(shared) = self.shared.upgrade() {
            shared
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .viewer_asking = false;
        }
    }
}
struct Startup {
    shared: Weak<Shared>,
    generation: u64,
    armed: bool,
}
impl Drop for Startup {
    fn drop(&mut self) {
        if self.armed
            && let Some(shared) = self.shared.upgrade()
        {
            shared.failed(
                self.generation,
                "Graphical session preparation was interrupted".into(),
            );
        }
    }
}
fn read_endings(driver: &dyn Driver, last: u64) -> Vec<(std::num::NonZeroU64, launch::Ending)> {
    (last.saturating_sub(ENDINGS_LIMIT) + 1..=last)
        .filter_map(std::num::NonZeroU64::new)
        .filter_map(|request| {
            let ending = driver.application_ending(request).ok().flatten()?;
            Some((request, ending))
        })
        .collect()
}
async fn watch_driver(
    shared: Weak<Shared>,
    generation: u64,
    mut updates: watch::Receiver<DriverState>,
) {
    // An unused display is retired on the pass after its endings were read,
    // outside the owner lock, and only if it is still unused then.
    let mut endings = None;
    loop {
        let Some(owner) = shared.upgrade() else {
            return;
        };
        let read = endings.take();
        let mut unread = None;
        let (retired, error, changed) = {
            let mut state = owner
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let Some(entry) = state.entries.get_mut(&generation) else {
                return;
            };
            if entry.phase != Phase::Ready {
                return;
            }
            // Read under the same lock as release. An older observation must
            // not overwrite a newer count already used by a releasing caller.
            let value = updates.borrow_and_update().clone();
            let error = entry
                .prepared
                .as_ref()
                .and_then(|prepared| prepared.fence.validate().err());
            match value {
                DriverState::Ready {
                    windows,
                    applications,
                } if error.is_none() => {
                    let changed = (entry.windows, entry.applications) != (windows, applications);
                    entry.windows = windows;
                    entry.applications = applications;
                    let retired = match (entry.unused(), read) {
                        (true, Some(read)) => state.retire_unused_with(generation, read),
                        (true, None) => {
                            unread = entry.prepared.as_ref().map(|prepared| {
                                (
                                    prepared.attached.driver.clone(),
                                    entry.launched.load(Ordering::Acquire),
                                )
                            });
                            None
                        }
                        (false, _) => None,
                    };
                    (retired, None, changed)
                }
                DriverState::Failed(message) => (None, Some(message), false),
                DriverState::ShownElsewhere(shown) => {
                    drop(state);
                    owner.shown_elsewhere(generation, shown);
                    return;
                }
                _ => (None, Some(error.unwrap_or_else(|| ENDED.into())), false),
            }
        };
        if let Some(error) = error {
            owner.failed(generation, error);
            return;
        }
        if retired.is_some() {
            drop(retired);
            owner.notify();
            return;
        }
        if changed {
            owner.notify();
        }
        owner.observe_applications(generation);
        if let Some((driver, last)) = unread {
            drop(owner);
            endings = Some(read_endings(&*driver, last));
            continue;
        }
        // The existing SSH lease also has a bounded disconnect fallback. Do
        // not broadcast unchanged snapshots merely because that check woke up.
        drop(owner);
        tokio::select! {
            result = updates.changed() => if result.is_err() {
                if let Some(owner) = shared.upgrade() { owner.failed(generation, ENDED.into()); }
                return;
            },
            _ = tokio::time::sleep(Duration::from_millis(250)) => (),
        }
    }
}
/// A demand token for one exact display generation. It never adopts a replacement.
pub struct Lease {
    shared: Weak<Shared>,
    generation: u64,
    kind: Demand,
}
impl Lease {
    /// Wait for this exact generation to become unusable. Watching must not
    /// retain the owner or silently follow a replacement display.
    pub(crate) async fn unavailable(&self) -> String {
        let Some(shared) = self.shared.upgrade() else {
            return ENDED.into();
        };
        let mut changes = shared.changes.subscribe();
        drop(shared);
        loop {
            if let Err(error) = self.identity() {
                return error;
            }
            if changes.changed().await.is_err() {
                return ENDED.into();
            }
        }
    }
    pub fn generation(&self) -> u64 {
        self.generation
    }
    /// Holds the display until it shows a window or runs an app, as a
    /// launch holds it until its app is counted; or until `wait` passes or
    /// the display ends.
    pub(crate) async fn hold_until_used(self, wait: Duration) {
        let Some(shared) = self.shared.upgrade() else {
            return;
        };
        let mut changes = shared.changes.subscribe();
        drop(shared);
        let used = async {
            loop {
                {
                    let Some(shared) = self.shared.upgrade() else {
                        return;
                    };
                    let state = shared
                        .state
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    match state.entries.get(&self.generation) {
                        Some(entry) if entry.phase == Phase::Ready => {
                            if entry.windows > 0 || entry.applications > 0 {
                                return;
                            }
                        }
                        _ => return,
                    }
                }
                if changes.changed().await.is_err() {
                    return;
                }
            }
        };
        let _ = tokio::time::timeout(wait, used).await;
    }
    /// Its display is gone, as after every display was ended.
    pub fn ended(&self) -> bool {
        self.shared.upgrade().is_none_or(|shared| {
            !shared
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .entries
                .contains_key(&self.generation)
        })
    }
    /// Its display failed, as when its worker exits: the next acquisition
    /// starts another.
    pub fn failed(&self) -> bool {
        matches!(self.phase(), Ok(Phase::Failed(_)))
    }
    fn phase(&self) -> Result<Phase, String> {
        let shared = self.shared.upgrade().ok_or(ENDED)?;
        let state = shared
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        Ok(state
            .entries
            .get(&self.generation)
            .ok_or(ENDED)?
            .phase
            .clone())
    }
    pub fn identity(&self) -> Result<Identity, String> {
        let shared = self.shared.upgrade().ok_or(ENDED)?;
        let state = shared
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let entry = state.entries.get(&self.generation).ok_or(ENDED)?;
        let prepared = entry.validate(self.generation)?;
        Ok(prepared.identity.clone())
    }
    /// Submit once to this exact generation. The caller has already authorized
    /// the application request. Driver callbacks run outside the owner lock;
    /// a concurrent close fences admission in the adapter. Holding this lease
    /// keeps the display alive until the driver publishes pending demand.
    pub fn launch_application(&self, target: launch::Target) -> Result<launch::Started, String> {
        let (display, driver, launched) = {
            let shared = self.shared.upgrade().ok_or(ENDED)?;
            let state = shared
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let entry = state.entries.get(&self.generation).ok_or(ENDED)?;
            let prepared = entry.validate(self.generation)?;
            (
                prepared.identity.clone(),
                prepared.attached.driver.clone(),
                entry.launched.clone(),
            )
        };
        let submitted = driver.launch_application(target)?;
        launched.fetch_max(submitted.request.get(), Ordering::AcqRel);
        Ok(launch::Started {
            application: launch::Application {
                display,
                request: submitted.request,
            },
            pending: launch::Pending::new(submitted.reply),
        })
    }
    /// What this display's programs copy reaches this computer while no
    /// display's window has focus.
    pub(crate) fn stand(&self) {
        let driver = self.shared.upgrade().and_then(|shared| {
            let state = shared
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let entry = state.entries.get(&self.generation)?;
            let prepared = entry.validate(self.generation).ok()?;
            Some(prepared.attached.driver.clone())
        });
        if let Some(driver) = driver {
            driver.stand();
        }
    }
    pub fn launch(&self, target: launch::Target) -> Result<launch::Pending, String> {
        let driver = {
            let shared = self.shared.upgrade().ok_or(ENDED)?;
            let state = shared
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let entry = state.entries.get(&self.generation).ok_or(ENDED)?;
            entry.validate(self.generation)?.attached.driver.clone()
        };
        driver.launch(target).map(launch::Pending::new)
    }
}
impl Drop for Lease {
    fn drop(&mut self) {
        if let Some(shared) = self.shared.upgrade() {
            shared.release(self.generation, self.kind);
        }
    }
}

#[cfg(test)]
#[path = "graphical_session_tests.rs"]
mod tests;
