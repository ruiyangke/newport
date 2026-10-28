//! Backend-owned display lifetime. GUI adapters implement the wire protocol;
//! consumers retain demand, never decide another consumer's transport lifetime.
pub mod launch;
pub mod terminal_bus;
pub mod transport;
pub use newport_domain::terminal::SharedDisplayMode;
use newport_domain::terminal::SharedGuiEnvironment;
use std::{
    collections::{BTreeMap, VecDeque},
    future::Future,
    pin::Pin,
    sync::{
        Arc, Mutex, Weak,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
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

pub use newport_platform::ipc::applications::{Identity, Runtime};
fn validate_runtime(runtime: &Runtime) -> Result<(), String> {
    if runtime.instance.is_nil()
        || !crate::agent_runtime::protocol::valid_build(&runtime.build_sha256)
    {
        return Err("Invalid graphical runtime identity".into());
    }
    Ok(())
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Demand {
    Terminal,
    Clipboard,
    Launch,
}
impl Demand {
    fn index(self) -> usize {
        match self {
            Self::Terminal => 0,
            Self::Clipboard => 1,
            Self::Launch => 2,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DriverState {
    Ready { windows: u32, applications: u32 },
    Failed(String),
    Closed,
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
    /// What the remote side said about a running application, when it did.
    fn application_notice(
        &self,
        _request: std::num::NonZeroU64,
    ) -> Result<Option<launch::Notice>, String> {
        Ok(None)
    }
    fn stop_application(&self, _request: std::num::NonZeroU64) -> Result<bool, String> {
        Err("Individual application control is unavailable".into())
    }
    fn stop(&self);
}
/// Returned only after a validated GUI handshake and clipboard readiness.
/// Even rejected or late attachment results request cleanup when discarded.
pub struct Attached {
    pub environment: SharedGuiEnvironment,
    pub terminal_bus: terminal_bus::Attachment,
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
    /// Immutable local policy; never inferred from helper IPC environment.
    fn terminal_bus_policy(&self) -> terminal_bus::Policy {
        terminal_bus::Policy::PreserveInherited
    }
    /// Immutable for this attachment candidate and checked even when a display
    /// is reused. Existing callers retain mixed-mode behavior.
    fn display_mode(&self) -> SharedDisplayMode {
        SharedDisplayMode::Mixed
    }
    /// Check current admission even when an already-running display is reused
    /// and attach is never called. This must not perform IO or block.
    fn validate(&self) -> Result<(), String>;
    /// Dropping this future must cancel attachment, including any spawned IO.
    /// The supplied bridge owns exactly one already-admitted SSH channel;
    /// adapters must keep blocking reads/writes and finish off executor threads.
    /// The backend retains the registered worker and pinned transport lease.
    fn attach(&self, channel: transport::Channel, identity: Identity) -> AttachFuture;
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
    fn validate(
        &self,
        runtime: &Runtime,
        generation: u64,
        mode: SharedDisplayMode,
        bus_policy: terminal_bus::Policy,
    ) -> Result<(), String> {
        self.fence.validate()?;
        mode.validate_environment(&self.attached.environment)?;
        self.attached
            .terminal_bus
            .validate(bus_policy, &self.attached.environment)?;
        if &self.identity.runtime != runtime
            || self.identity.display_generation != generation
            || self.identity.worker.is_nil()
            || self.identity.session != self.attached.environment.session
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
    pub mode: SharedDisplayMode,
    pub phase: Phase,
    pub terminals: u32,
    pub clipboard: u32,
    pub launches: u32,
    pub windows: u32,
    pub applications: u32,
}
struct Entry {
    runtime: Runtime,
    mode: SharedDisplayMode,
    bus_policy: terminal_bus::Policy,
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
            DriverState::Closed => Err(ENDED.into()),
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
        prepared.validate(&self.runtime, generation, self.mode, self.bus_policy)?;
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
}
/// A display retired after normal completion, with the endings its adapter
/// reported for its latest requests.
struct Finished {
    identity: Identity,
    last: u64,
    endings: Vec<(std::num::NonZeroU64, launch::Ending)>,
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
    /// Some once the application's display finished normally.
    fn ending(&self, application: &launch::Application) -> Option<Option<launch::Ending>> {
        self.finished(application).map(|finished| {
            finished
                .endings
                .iter()
                .find(|(request, _)| *request == application.request)
                .map(|(_, ending)| ending.clone())
        })
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
            if last > 0 {
                if self.finished.len() == FINISHED_LIMIT {
                    self.finished.pop_front();
                }
                self.finished.push_back(Finished {
                    identity: prepared.identity.clone(),
                    last,
                    endings,
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
            let mut state = self.state.lock().unwrap();
            let Some(entry) = state.entries.get_mut(&generation) else {
                return;
            };
            entry.demand[kind.index()] -= 1;
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
    fn failed(&self, generation: u64, message: String) {
        let (prepared, task) = {
            let mut state = self.state.lock().unwrap();
            let Some(entry) = state.entries.get_mut(&generation) else {
                return;
            };
            entry.phase = Phase::Failed(message);
            entry.windows = 0;
            entry.applications = 0;
            entry.updates = None;
            (entry.prepared.take(), entry.task.take())
        };
        if let Some(task) = task {
            task.abort();
        }
        drop(prepared);
        let retired = {
            let mut state = self.state.lock().unwrap();
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
        let state = self.shared.state.lock().unwrap();
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
        if self.shared.state.lock().unwrap().closed {
            return Err(ENDED.into());
        }
        Ok(guard)
    }
    fn application_driver(
        &self,
        application: &launch::Application,
    ) -> Result<Option<Arc<dyn Driver>>, String> {
        let state = self.shared.state.lock().unwrap();
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
        let Some(driver) = self.application_driver(application)? else {
            return Ok(launch::State::Ended);
        };
        let result = driver
            .application_state(application.request)
            .and_then(|value| self.validate_identity(&application.display).map(|()| value));
        // The last group may finish and retire the display during observation.
        if result.is_err() && self.shared.state.lock().unwrap().completed(application) {
            return Ok(launch::State::Ended);
        }
        result
    }
    /// Kept after the display retires, for the latest requests it reported.
    pub(crate) fn application_ending(
        &self,
        application: &launch::Application,
    ) -> Result<Option<launch::Ending>, String> {
        if let Some(ending) = self.shared.state.lock().unwrap().ending(application) {
            return Ok(ending);
        }
        let Some(driver) = self.application_driver(application)? else {
            return Ok(self
                .shared
                .state
                .lock()
                .unwrap()
                .ending(application)
                .flatten());
        };
        let result = driver
            .application_ending(application.request)
            .and_then(|value| self.validate_identity(&application.display).map(|()| value));
        if result.is_err()
            && let Some(ending) = self.shared.state.lock().unwrap().ending(application)
        {
            return Ok(ending);
        }
        result
    }
    /// Only while the application's display is live; none after it retires.
    pub(crate) fn application_notice(
        &self,
        application: &launch::Application,
    ) -> Result<Option<launch::Notice>, String> {
        let Some(driver) = self.application_driver(application)? else {
            return Ok(None);
        };
        driver
            .application_notice(application.request)
            .and_then(|value| self.validate_identity(&application.display).map(|()| value))
    }
    pub(crate) fn stop_application(
        &self,
        application: &launch::Application,
    ) -> Result<bool, String> {
        // Clone under the ownership lock; the adapter serializes this action
        // with stop. No channel or lease can be opened by an old handle.
        let Some(driver) = self.application_driver(application)? else {
            return Ok(false);
        };
        let result = driver.stop_application(application.request);
        if result.is_err() && self.shared.state.lock().unwrap().completed(application) {
            return Ok(false);
        }
        result
    }
    pub(crate) async fn maintenance(&self) -> RwLockWriteGuard<'_, ()> {
        self.admission.write().await
    }
    pub fn snapshot(&self) -> Vec<Snapshot> {
        self.shared
            .state
            .lock()
            .unwrap()
            .entries
            .iter()
            .map(|(&generation, entry)| Snapshot {
                generation,
                runtime: entry.runtime.clone(),
                mode: entry.mode,
                phase: entry.phase.clone(),
                terminals: entry.demand[0],
                clipboard: entry.demand[1],
                launches: entry.demand[2],
                windows: entry.windows,
                applications: entry.applications,
            })
            .collect()
    }
    #[cfg(test)]
    pub async fn drained(&self) {
        let mut changes = self.shared.changes.subscribe();
        while !self.shared.state.lock().unwrap().entries.is_empty() {
            if changes.changed().await.is_err() {
                break;
            }
        }
    }
    pub fn close(&self) {
        self.shared.state.lock().unwrap().closed = true;
        self.end_all();
    }
    /// Ends every display now, whoever still holds it, and keeps accepting
    /// new ones.
    pub fn end_all(&self) {
        let retired = {
            let mut state = self.shared.state.lock().unwrap();
            state.finished.clear();
            std::mem::take(&mut state.entries)
        };
        drop(retired);
        self.shared.notify();
    }
    #[cfg(test)]
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
        self.acquire_mode(runtime, SharedDisplayMode::Mixed, kind, start)
            .await
    }
    #[cfg(test)]
    pub(crate) async fn acquire_mode<F, Fut>(
        &self,
        runtime: Runtime,
        mode: SharedDisplayMode,
        kind: Demand,
        start: F,
    ) -> Result<Lease, String>
    where
        F: FnOnce(u64) -> Fut + Send + 'static,
        Fut: Future<Output = Result<Prepared, String>> + Send + 'static,
    {
        self.acquire_policy(
            runtime,
            mode,
            terminal_bus::Policy::PreserveInherited,
            kind,
            start,
        )
        .await
    }
    pub(crate) async fn acquire_policy<F, Fut>(
        &self,
        runtime: Runtime,
        mode: SharedDisplayMode,
        bus_policy: terminal_bus::Policy,
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
            let mut state = self.shared.state.lock().unwrap();
            if state.closed {
                return Err(ENDED.into());
            }
            // A setting change must neither adopt the opposite display nor
            // create another clipboard/display owner beside existing apps.
            // Old build generations are included until all their demand drains.
            if state
                .entries
                .values()
                .any(|entry| entry.mode != mode && !matches!(entry.phase, Phase::Failed(_)))
            {
                return Err("Close this server's Remote Apps and graphical terminals before switching the shared display mode. Existing applications remain running.".into());
            }
            if state.entries.values().any(|entry| {
                entry.bus_policy != bus_policy && !matches!(entry.phase, Phase::Failed(_))
            }) {
                return Err("Close this server's Remote Apps and graphical terminals before switching the terminal bus policy. Existing applications remain running.".into());
            }
            let found = state.entries.iter().find_map(|(&id, entry)| {
                (entry.runtime == runtime
                    && entry.mode == mode
                    && !matches!(entry.phase, Phase::Failed(_)))
                .then_some(id)
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
                        mode,
                        bus_policy,
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
                        prepared.validate(&runtime, generation, mode, bus_policy)?;
                        Ok(prepared)
                    });
                let Some(shared) = weak.upgrade() else { return };
                match result {
                    Err(error) => shared.failed(generation, error),
                    Ok(prepared) => {
                        let updates = prepared.attached.driver.state();
                        let mut state = shared.state.lock().unwrap();
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
            let mut state = self.shared.state.lock().unwrap();
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
                    lease.environment()?;
                    return Ok(lease);
                }
                Phase::Failed(error) => return Err(error),
                Phase::Preparing => (),
            }
            changes.changed().await.map_err(|_| ENDED)?;
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
            let mut state = owner.state.lock().unwrap();
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
    /// Its display is gone, as after every display was ended.
    pub fn ended(&self) -> bool {
        self.shared.upgrade().is_none_or(|shared| {
            !shared
                .state
                .lock()
                .unwrap()
                .entries
                .contains_key(&self.generation)
        })
    }
    fn phase(&self) -> Result<Phase, String> {
        let shared = self.shared.upgrade().ok_or(ENDED)?;
        let state = shared.state.lock().unwrap();
        Ok(state
            .entries
            .get(&self.generation)
            .ok_or(ENDED)?
            .phase
            .clone())
    }
    pub fn environment(&self) -> Result<SharedGuiEnvironment, String> {
        let shared = self.shared.upgrade().ok_or(ENDED)?;
        let state = shared.state.lock().unwrap();
        let entry = state.entries.get(&self.generation).ok_or(ENDED)?;
        let prepared = entry.validate(self.generation)?;
        Ok(prepared.attached.environment.clone())
    }
    pub fn terminal_bus(&self) -> Result<terminal_bus::Attachment, String> {
        let shared = self.shared.upgrade().ok_or(ENDED)?;
        let state = shared.state.lock().unwrap();
        let entry = state.entries.get(&self.generation).ok_or(ENDED)?;
        Ok(entry
            .validate(self.generation)?
            .attached
            .terminal_bus
            .clone())
    }
    pub fn identity(&self) -> Result<Identity, String> {
        let shared = self.shared.upgrade().ok_or(ENDED)?;
        let state = shared.state.lock().unwrap();
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
            let state = shared.state.lock().unwrap();
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
    pub fn launch(&self, target: launch::Target) -> Result<launch::Pending, String> {
        let driver = {
            let shared = self.shared.upgrade().ok_or(ENDED)?;
            let state = shared.state.lock().unwrap();
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
