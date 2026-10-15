//! Retained Services requests for one immutable server connection.
//!
//! A page may disappear without releasing action ownership. Retire its controller
//! when the connection identity changes; an old action must never refresh a new
//! endpoint that happens to reuse the same server ID.
use crate::{backend::Backend, resources::Resource};
pub use newport_domain::services::{
    ActionResult, JournalPage, Scope, ServiceVerb, ServicesSnapshot, UnitFile,
};
use newport_domain::services::{action_blocked_reason, available_verbs, valid_unit};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use std::{
    collections::{HashMap, HashSet, VecDeque},
    sync::{Arc, mpsc},
    thread,
    time::{Duration, Instant},
};

const SNAPSHOT_INTERVAL: Duration = Duration::from_secs(5);
const LOG_INTERVAL: Duration = Duration::from_secs(2);
const UNIT_FILE_STALE_TIME: Duration = Duration::from_secs(10);
const MAX_ENTRIES: usize = 2_000;
const MAX_LOG_TARGETS: usize = 8;
const MAX_FILES: usize = 32;
const MAX_ACTIONS: usize = 32;
const MAX_CONCURRENT_ACTIONS: usize = 8;
const MAX_ACTION_COMPLETIONS: usize = 64;
const WORKER_ERROR: &str =
    "Services worker stopped before confirming the result. Refresh before trying again.";

type Outcome = Result<Value, String>;
type Dispatch = dyn Fn(&'static str, Value) -> Result<mpsc::Receiver<Outcome>, String>;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct LogTarget {
    pub unit: Option<String>,
    pub scope: Scope,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct LogKey {
    target: LogTarget,
    priority: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
struct UnitKey {
    unit: String,
    scope: Scope,
}

/// An action result belongs to both the unit name and its system/user scope.
#[derive(Clone, Debug)]
pub struct ActionStatus {
    pub verb: ServiceVerb,
    pub pending: bool,
    pub result: Option<ActionResult>,
    pub error: Option<String>,
}

/// A one-time outcome receipt, independent of the currently selected service.
#[derive(Clone, Debug)]
pub struct ActionCompletion {
    pub unit: String,
    pub scope: Scope,
    pub verb: ServiceVerb,
    pub result: Result<ActionResult, String>,
}

struct Slot<T> {
    resource: Resource,
    data: Option<T>,
    generation: u64,
}
impl<T> Default for Slot<T> {
    fn default() -> Self {
        Self {
            resource: Resource::default(),
            data: None,
            generation: 0,
        }
    }
}
impl<T> Slot<T> {
    fn invalidate(&mut self) {
        self.generation = 0;
        self.resource.pending = false;
        self.resource.manual_pending = false;
    }
}

#[derive(Clone)]
enum Job {
    Snapshot,
    Logs(LogKey),
    File(UnitKey),
    Action(UnitKey, ServiceVerb),
}
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
enum Kind {
    Snapshot,
    Logs,
    File,
    Action(UnitKey),
}
impl Job {
    fn kind(&self) -> Kind {
        match self {
            Self::Snapshot => Kind::Snapshot,
            Self::Logs(_) => Kind::Logs,
            Self::File(_) => Kind::File,
            Self::Action(key, _) => Kind::Action(key.clone()),
        }
    }
    fn method(&self) -> &'static str {
        match self {
            Self::Snapshot => "services_snapshot",
            Self::Logs(_) => "service_logs",
            Self::File(_) => "service_unit_file",
            Self::Action(..) => "service_action",
        }
    }
}
struct Pending {
    job: Job,
    generation: u64,
    receiver: mpsc::Receiver<Outcome>,
}

pub struct Services {
    server: String,
    retired: bool,
    snapshot: Slot<ServicesSnapshot>,
    logs: VecDeque<(LogKey, Slot<JournalPage>)>,
    files: VecDeque<(UnitKey, Slot<UnitFile>)>,
    actions: VecDeque<(UnitKey, ActionStatus)>,
    action_completions: VecDeque<ActionCompletion>,
    action_resource: Resource,
    selected_logs: Option<LogKey>,
    selected_file: Option<UnitKey>,
    logs_placeholder: Option<JournalPage>,
    live: bool,
    visible: bool,
    wanted_snapshot: bool,
    wanted_logs: bool,
    wanted_file: bool,
    action_snapshot_dirty: HashSet<UnitKey>,
    // Three independent reads and at most eight per-unit action workers.
    // Invalidation never releases a physical worker's slot.
    pending: HashMap<Kind, Pending>,
    next: u64,
    changed: bool,
    dispatch: Box<Dispatch>,
    now: Box<dyn Fn() -> Instant>,
}

impl Services {
    pub fn new(backend: Backend, server: String) -> Self {
        let backend = Arc::new(backend);
        Self::with_dispatch(
            server,
            Box::new(move |method, args| {
                let (sender, receiver) = mpsc::sync_channel(1);
                let backend = backend.clone();
                thread::Builder::new()
                    .name(format!("newport-{method}"))
                    .spawn(move || {
                        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                            backend.call_with_timeout(method, args, request_timeout(method))
                        }))
                        .unwrap_or_else(|_| Err(WORKER_ERROR.into()));
                        let _ = sender.send(result);
                    })
                    .map_err(|error| format!("Could not start Services request: {error}"))?;
                Ok(receiver)
            }),
            Box::new(Instant::now),
        )
    }

    fn with_dispatch(
        server: String,
        dispatch: Box<Dispatch>,
        now: Box<dyn Fn() -> Instant>,
    ) -> Self {
        Self {
            server,
            retired: false,
            snapshot: Slot::default(),
            logs: VecDeque::new(),
            files: VecDeque::new(),
            actions: VecDeque::new(),
            action_completions: VecDeque::new(),
            action_resource: Resource::default(),
            selected_logs: None,
            selected_file: None,
            logs_placeholder: None,
            live: false,
            visible: false,
            wanted_snapshot: false,
            wanted_logs: false,
            wanted_file: false,
            action_snapshot_dirty: HashSet::new(),
            pending: HashMap::new(),
            next: 0,
            changed: false,
            dispatch,
            now,
        }
    }

    /// Keys: snapshot, logs, unit-file, action. Logs/file refer to the selection.
    pub fn resource(&self, key: &str) -> Option<&Resource> {
        match key {
            "snapshot" => Some(&self.snapshot.resource),
            "logs" => Some(&self.log_slot(self.selected_logs.as_ref()?)?.resource),
            "unit-file" => Some(&self.file_slot(self.selected_file.as_ref()?)?.resource),
            "action" => Some(&self.action_resource),
            _ => None,
        }
    }
    pub fn snapshot(&self) -> Option<&ServicesSnapshot> {
        self.snapshot.data.as_ref()
    }
    /// Changing priority can temporarily retain the same target's lines. A new
    /// unit or scope never receives another target's placeholder.
    pub fn logs(&self) -> Option<&JournalPage> {
        self.log_slot(self.selected_logs.as_ref()?)?
            .data
            .as_ref()
            .or(self.logs_placeholder.as_ref())
    }
    pub fn unit_file(&self) -> Option<&UnitFile> {
        self.file_slot(self.selected_file.as_ref()?)?.data.as_ref()
    }
    pub fn action_status(&self, unit: &str, scope: Scope) -> Option<&ActionStatus> {
        self.actions
            .iter()
            .find(|(key, _)| key.scope == scope && key.unit == unit)
            .map(|(_, status)| status)
    }
    /// Drain outcome receipts after polling. Up to 64 are retained if a caller
    /// stops consuming notifications; scoped action status remains available.
    pub fn take_action_completions(&mut self) -> Vec<ActionCompletion> {
        self.action_completions.drain(..).collect()
    }
    pub fn has_pending(&self) -> bool {
        !self.pending.is_empty()
    }

    /// Prevent every future dispatch, including post-action refreshes. Workers
    /// already in flight are drained; no write is cancelled or replayed.
    pub fn retire(&mut self) {
        self.retired = true;
        self.snapshot = Slot::default();
        self.logs.clear();
        self.files.clear();
        self.clear_selection();
        self.wanted_snapshot = false;
        self.changed = true;
    }

    pub fn refresh(&mut self) {
        if !self.retired {
            self.wanted_snapshot = true;
            self.read_snapshot(true);
        }
    }
    pub fn select_logs(&mut self, target: LogTarget, priority: Option<String>, live: bool) -> bool {
        if self.retired
            || target
                .unit
                .as_deref()
                .is_some_and(|unit| !valid_unit(unit, true))
            || priority
                .as_deref()
                .is_some_and(|priority| !matches!(priority, "err" | "warning"))
        {
            return false;
        }
        let key = LogKey { target, priority };
        if self.selected_logs.as_ref() != Some(&key) {
            let same_target = self
                .selected_logs
                .as_ref()
                .is_some_and(|old| old.target == key.target);
            self.logs_placeholder = same_target.then(|| self.logs().cloned()).flatten();
            touch(&mut self.logs, &key, MAX_LOG_TARGETS);
            self.selected_logs = Some(key);
            self.wanted_logs = self
                .log_slot(self.selected_logs.as_ref().unwrap())
                .is_some_and(|slot| slot.data.is_none());
            self.changed = true;
        }
        self.changed |= self.live != live;
        self.live = live;
        self.read_logs(false);
        true
    }
    pub fn select_unit_file(&mut self, unit: String, scope: Scope) -> bool {
        if self.retired || !valid_unit(&unit, true) {
            return false;
        }
        let key = UnitKey { unit, scope };
        if self.selected_file.as_ref() != Some(&key) {
            touch(&mut self.files, &key, MAX_FILES);
            self.selected_file = Some(key);
            self.wanted_file = self.unit_file_stale((self.now)());
            self.changed = true;
        }
        self.read_file(false);
        true
    }
    pub fn clear_selection(&mut self) {
        self.selected_logs = None;
        self.selected_file = None;
        self.logs_placeholder = None;
        self.live = false;
        self.wanted_logs = false;
        self.wanted_file = false;
        self.changed = true;
    }
    pub fn retry_logs(&mut self) {
        if !self.retired {
            self.wanted_logs = self.selected_logs.is_some();
            self.read_logs(true);
        }
    }
    pub fn reset_logs(&mut self) {
        if self.retired {
            return;
        }
        if let Some(key) = self.selected_logs.clone() {
            if let Some(slot) = self.log_slot_mut(&key) {
                *slot = Slot::default();
            }
            self.logs_placeholder = None;
            self.wanted_logs = true;
            self.changed = true;
            self.read_logs(true);
        }
    }
    pub fn retry_unit_file(&mut self) {
        if !self.retired {
            self.wanted_file = self.selected_file.is_some();
            self.read_file(true);
        }
    }

    /// Ordinary background reads keep the last accepted action capabilities.
    /// After a mutation, capabilities require a new successful snapshot.
    pub fn can_action(&self, unit: &str, scope: Scope, verb: ServiceVerb) -> bool {
        let key = UnitKey {
            unit: unit.to_owned(),
            scope,
        };
        if self.retired
            || self.pending.contains_key(&Kind::Action(key.clone()))
            || self.pending_action_count() >= MAX_CONCURRENT_ACTIONS
            || self.action_snapshot_dirty.contains(&key)
            || !valid_unit(unit, false)
            || self.snapshot.resource.error.is_some()
        {
            return false;
        }
        let Some(snapshot) = self.snapshot.data.as_ref() else {
            return false;
        };
        let Some(service) = snapshot
            .services
            .iter()
            .find(|service| service.name == unit && service.scope == scope)
        else {
            return false;
        };
        action_blocked_reason(scope, snapshot.privilege).is_none()
            && (scope != Scope::User || snapshot.user.available)
            && available_verbs(service).allows(verb)
    }
    /// Stop/disable confirmation belongs to the UI. Dispatch rechecks current
    /// permissions and verbs; one action owns its scoped unit until it settles.
    pub fn action(&mut self, unit: String, scope: Scope, verb: ServiceVerb) -> bool {
        if !self.can_action(&unit, scope, verb) {
            return false;
        }
        let key = UnitKey { unit, scope };
        self.action_snapshot_dirty.insert(key.clone());
        self.snapshot.invalidate();
        self.actions.retain(|(old, _)| old != &key);
        self.actions.push_back((
            key.clone(),
            ActionStatus {
                verb,
                pending: true,
                result: None,
                error: None,
            },
        ));
        while self.actions.len() > MAX_ACTIONS {
            let settled = self
                .actions
                .iter()
                .position(|(_, status)| !status.pending)
                .expect("concurrent actions are bounded below retained statuses");
            self.actions.remove(settled);
        }
        self.launch(
            Job::Action(key.clone(), verb),
            json!({"id":self.server,"unit":key.unit,"scope":scope,"verb":verb}),
            true,
        )
    }

    /// Hidden pages still drain actions. Only visible pages schedule periodic
    /// snapshot/tail reads. Explicitly requested reads may finish while hidden.
    pub fn poll(&mut self, visible: bool) -> bool {
        let now = (self.now)();
        // Unit files are lazy, not periodically polled. Revisiting the page or
        // the file tab revalidates stale text while retaining the last good read.
        let reactivated = visible && !self.visible;
        self.visible = visible;
        let mut kinds: Vec<_> = self
            .pending
            .keys()
            .filter(|kind| matches!(kind, Kind::Action(_)))
            .cloned()
            .collect();
        kinds.sort();
        kinds.extend([Kind::Snapshot, Kind::Logs, Kind::File]);
        // Settle every ready write before accepting a concurrent snapshot.
        for kind in kinds {
            let result =
                self.pending
                    .get(&kind)
                    .and_then(|pending| match pending.receiver.try_recv() {
                        Ok(result) => Some(result),
                        Err(mpsc::TryRecvError::Empty) => None,
                        Err(mpsc::TryRecvError::Disconnected) => Some(Err(WORKER_ERROR.into())),
                    });
            if let Some(result) = result {
                let pending = self
                    .pending
                    .remove(&kind)
                    .expect("completed worker has an owner");
                self.complete(pending.job, pending.generation, result, now);
            }
        }
        if !self.retired {
            if reactivated && self.unit_file_stale(now) {
                self.wanted_file = true;
            }
            if visible && due(&self.snapshot.resource, now, SNAPSHOT_INTERVAL) {
                self.wanted_snapshot = true;
            }
            if visible
                && self.live
                && self
                    .selected_logs
                    .as_ref()
                    .and_then(|key| self.log_slot(key))
                    .is_some_and(|slot| due(&slot.resource, now, LOG_INTERVAL))
            {
                self.wanted_logs = true;
            }
            self.read_snapshot(false);
            self.read_logs(false);
            self.read_file(false);
        }
        std::mem::take(&mut self.changed)
    }

    fn read_snapshot(&mut self, manual: bool) {
        if self.retired || !self.wanted_snapshot {
            return;
        }
        if self.pending.contains_key(&Kind::Snapshot) {
            if self.snapshot.resource.pending {
                self.wanted_snapshot = false;
                if manual {
                    self.changed |= !self.snapshot.resource.manual_pending;
                    self.snapshot.resource.manual_pending = true;
                }
            }
            return;
        }
        self.wanted_snapshot = false;
        self.launch(Job::Snapshot, json!({"id":self.server}), manual);
    }
    fn read_logs(&mut self, manual: bool) {
        if self.retired || !self.wanted_logs {
            return;
        }
        let Some(key) = self.selected_logs.clone() else {
            return;
        };
        if self.pending.contains_key(&Kind::Logs) {
            if let Some(slot) = self.log_slot_mut(&key).filter(|slot| slot.resource.pending) {
                let changed = manual && !slot.resource.manual_pending;
                slot.resource.manual_pending |= manual;
                self.changed |= changed;
                self.wanted_logs = false;
            }
            return;
        }
        let previous = self.log_slot(&key).and_then(|slot| slot.data.as_ref());
        let args = json!({"id":self.server,"unit":key.target.unit,"scope":key.target.scope,"priority":key.priority,
            "cursor":previous.and_then(|page| page.cursor.as_deref()),"lines":if previous.and_then(|page|page.cursor.as_ref()).is_some() {None} else {Some(200)}});
        self.wanted_logs = false;
        self.launch(Job::Logs(key), args, manual);
    }
    fn read_file(&mut self, manual: bool) {
        if self.retired || !self.wanted_file {
            return;
        }
        let Some(key) = self.selected_file.clone() else {
            return;
        };
        if self.pending.contains_key(&Kind::File) {
            if let Some(slot) = self
                .file_slot_mut(&key)
                .filter(|slot| slot.resource.pending)
            {
                let changed = manual && !slot.resource.manual_pending;
                slot.resource.manual_pending |= manual;
                self.changed |= changed;
                self.wanted_file = false;
            }
            return;
        }
        self.wanted_file = false;
        let args = json!({"id":self.server,"unit":key.unit,"scope":key.scope});
        self.launch(Job::File(key), args, manual);
    }

    fn launch(&mut self, job: Job, args: Value, manual: bool) -> bool {
        if self.retired || self.pending.contains_key(&job.kind()) {
            return false;
        }
        self.next += 1;
        let generation = self.next;
        let row = match &job {
            Job::Snapshot => {
                self.snapshot.generation = generation;
                &mut self.snapshot.resource
            }
            Job::Logs(key) => {
                let slot = self.log_slot_mut(key).unwrap();
                slot.generation = generation;
                &mut slot.resource
            }
            Job::File(key) => {
                let slot = self.file_slot_mut(key).unwrap();
                slot.generation = generation;
                &mut slot.resource
            }
            Job::Action(..) => &mut self.action_resource,
        };
        row.pending = true;
        row.manual_pending = manual;
        if row.updated_at.is_none() || matches!(job, Job::Action(..)) {
            row.error = None;
        }
        self.changed |= row.display_pending();
        let mut args = args;
        if manual
            && matches!(
                job.method(),
                "services_snapshot"
                    | "service_unit_file"
                    | "containers_snapshot"
                    | "container_inspect"
                    | "docker_disk"
                    | "compose_project"
            )
        {
            args["refresh"] = Value::Bool(true);
        }
        match (self.dispatch)(job.method(), args) {
            Ok(receiver) => {
                self.pending.insert(
                    job.kind(),
                    Pending {
                        job,
                        generation,
                        receiver,
                    },
                );
                true
            }
            Err(error) => {
                self.complete(job, generation, Err(error), (self.now)());
                false
            }
        }
    }

    fn complete(&mut self, job: Job, generation: u64, result: Outcome, now: Instant) {
        if self.retired && !matches!(job, Job::Action(..)) {
            return;
        }
        let accepted = match job {
            Job::Snapshot => {
                let presentation = self.snapshot.resource.presentation_revision;
                let dirty_actions = self.action_snapshot_dirty.len();
                let accepted = finish(&mut self.snapshot, generation, result.and_then(decode), now);
                if accepted && self.snapshot.resource.error.is_none() {
                    self.action_snapshot_dirty
                        .retain(|key| self.pending.contains_key(&Kind::Action(key.clone())));
                }
                accepted
                    && (presentation != self.snapshot.resource.presentation_revision
                        || dirty_actions != self.action_snapshot_dirty.len())
            }
            Job::Logs(key) => {
                let Some(slot) = self.log_slot_mut(&key) else {
                    return;
                };
                if slot.generation != generation {
                    return;
                }
                let result = result.and_then(decode::<JournalPage>).map(|mut page| {
                    // Without a cursor the next response is another latest-lines
                    // snapshot, not an appendable continuation.
                    if let Some(previous) = &slot.data
                        && previous.cursor.is_some()
                    {
                        page.cursor = page.cursor.or_else(|| previous.cursor.clone());
                        let mut entries = previous.entries.clone();
                        entries.append(&mut page.entries);
                        page.entries = entries;
                    }
                    let excess = page.entries.len().saturating_sub(MAX_ENTRIES);
                    page.entries.drain(..excess);
                    page
                });
                let accepted = finish(slot, generation, result, now);
                if accepted && self.selected_logs.as_ref() == Some(&key) {
                    self.logs_placeholder = None;
                }
                accepted
            }
            Job::File(key) => self.file_slot_mut(&key).is_some_and(|slot| {
                let presentation = slot.resource.presentation_revision;
                finish(slot, generation, result.and_then(decode), now)
                    && presentation != slot.resource.presentation_revision
            }),
            Job::Action(key, verb) => {
                self.snapshot.invalidate();
                self.wanted_snapshot = !self.retired;
                let result = result.and_then(decode::<ActionResult>).and_then(|result| {
                    if result.ok
                        || (result.needs_password == Some(true)
                            && result
                                .command
                                .as_deref()
                                .is_some_and(|command| !command.is_empty()))
                    {
                        Ok(result)
                    } else {
                        Err("Invalid Services action outcome.".into())
                    }
                });
                if let Some((_, status)) =
                    self.actions.iter_mut().find(|(target, _)| target == &key)
                {
                    *status = ActionStatus {
                        verb,
                        pending: false,
                        result: result.as_ref().ok().cloned(),
                        error: result.as_ref().err().cloned(),
                    };
                }
                self.action_completions.push_back(ActionCompletion {
                    unit: key.unit,
                    scope: key.scope,
                    verb,
                    result: result.clone(),
                });
                while self.action_completions.len() > MAX_ACTION_COMPLETIONS {
                    self.action_completions.pop_front();
                }
                // Completion order can differ from dispatch order. Keep the
                // aggregate resource revision monotonic; receipts carry identity.
                self.next += 1;
                finish_resource(
                    &mut self.action_resource,
                    self.next,
                    result.and_then(encode),
                    now,
                );
                self.action_resource.pending = self.pending_action_count() > 0;
                self.action_resource.manual_pending = self.action_resource.pending;
                true
            }
        };
        self.changed |= accepted;
    }
    fn log_slot(&self, key: &LogKey) -> Option<&Slot<JournalPage>> {
        self.logs
            .iter()
            .find(|(target, _)| target == key)
            .map(|(_, slot)| slot)
    }
    fn pending_action_count(&self) -> usize {
        self.pending
            .keys()
            .filter(|kind| matches!(kind, Kind::Action(_)))
            .count()
    }
    fn log_slot_mut(&mut self, key: &LogKey) -> Option<&mut Slot<JournalPage>> {
        self.logs
            .iter_mut()
            .find(|(target, _)| target == key)
            .map(|(_, slot)| slot)
    }
    fn file_slot(&self, key: &UnitKey) -> Option<&Slot<UnitFile>> {
        self.files
            .iter()
            .find(|(target, _)| target == key)
            .map(|(_, slot)| slot)
    }
    fn unit_file_stale(&self, now: Instant) -> bool {
        self.selected_file
            .as_ref()
            .and_then(|key| self.file_slot(key))
            .is_some_and(|slot| {
                slot.resource
                    .updated_at
                    .is_none_or(|at| now.saturating_duration_since(at) >= UNIT_FILE_STALE_TIME)
            })
    }
    fn file_slot_mut(&mut self, key: &UnitKey) -> Option<&mut Slot<UnitFile>> {
        self.files
            .iter_mut()
            .find(|(target, _)| target == key)
            .map(|(_, slot)| slot)
    }
}

fn touch<K: PartialEq + Clone, T>(cache: &mut VecDeque<(K, Slot<T>)>, key: &K, max: usize) {
    let slot = cache
        .iter()
        .position(|(candidate, _)| candidate == key)
        .and_then(|index| cache.remove(index))
        .map(|(_, slot)| slot)
        .unwrap_or_default();
    cache.push_back((key.clone(), slot));
    while cache.len() > max {
        cache.pop_front();
    }
}
fn due(resource: &Resource, now: Instant, interval: Duration) -> bool {
    !resource.pending
        && resource
            .finished_at
            .is_none_or(|at| now.saturating_duration_since(at) >= interval)
}
fn decode<T: DeserializeOwned>(value: Value) -> Result<T, String> {
    serde_json::from_value(value).map_err(|error| format!("Invalid Services response: {error}"))
}
fn encode<T: Serialize>(value: T) -> Outcome {
    serde_json::to_value(value).map_err(|error| format!("Invalid Services response: {error}"))
}
fn finish<T: Serialize>(
    slot: &mut Slot<T>,
    generation: u64,
    result: Result<T, String>,
    now: Instant,
) -> bool {
    if slot.generation != generation {
        return false;
    }
    let result = result.and_then(|value| {
        let encoded = encode(&value)?;
        slot.data = Some(value);
        Ok(encoded)
    });
    finish_resource(&mut slot.resource, generation, result, now);
    true
}
fn finish_resource(resource: &mut Resource, generation: u64, result: Outcome, now: Instant) {
    resource.finish_read(generation, result, now, None);
}
fn request_timeout(method: &str) -> Duration {
    // Backend remote deadlines are 30s snapshot, 20s reads and 75s actions.
    Duration::from_secs(match method {
        "service_action" => 90,
        "services_snapshot" => 40,
        _ => 30,
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
        services: Services,
        requests: Rc<RefCell<Vec<Request>>>,
        now: Rc<Cell<Instant>>,
    }
    impl Harness {
        fn new() -> Self {
            let requests = Rc::new(RefCell::new(Vec::new()));
            let outgoing = requests.clone();
            let now = Rc::new(Cell::new(Instant::now()));
            let clock = now.clone();
            Self {
                services: Services::with_dispatch(
                    "server-a".into(),
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
                ),
                requests,
                now,
            }
        }
        fn ready() -> Self {
            let mut h = Self::new();
            h.services.refresh();
            h.reply("services_snapshot", Ok(snapshot("active", "root")));
            h.services.poll(true);
            h
        }
        fn advance(&self, seconds: u64) {
            self.now.set(self.now.get() + Duration::from_secs(seconds));
        }
        fn count(&self, method: &str) -> usize {
            self.requests
                .borrow()
                .iter()
                .filter(|r| r.method == method)
                .count()
        }
        fn args(&self, method: &str) -> Value {
            self.requests
                .borrow()
                .iter()
                .rev()
                .find(|r| r.method == method)
                .unwrap()
                .args
                .clone()
        }
        fn reply(&self, method: &str, result: Outcome) {
            self.requests
                .borrow_mut()
                .iter_mut()
                .find(|r| r.method == method && r.sender.is_some())
                .expect("pending request")
                .sender
                .take()
                .unwrap()
                .send(result)
                .unwrap();
        }
        fn disconnect(&self, method: &str) {
            self.requests
                .borrow_mut()
                .iter_mut()
                .find(|r| r.method == method && r.sender.is_some())
                .expect("pending request")
                .sender
                .take();
        }
        fn reply_action(&self, unit: &str, scope: Scope, result: Outcome) {
            self.requests
                .borrow_mut()
                .iter_mut()
                .find(|request| {
                    request.method == "service_action"
                        && request.args["unit"] == unit
                        && request.args["scope"] == scope.as_str()
                        && request.sender.is_some()
                })
                .expect("pending scoped action")
                .sender
                .take()
                .unwrap()
                .send(result)
                .unwrap();
        }
    }
    fn snapshot(active: &str, privilege: &str) -> Value {
        let unit = encode(newport_domain::services::ServiceUnit::default()).unwrap();
        let mut units = [unit.clone(), unit];
        for (unit, scope) in units.iter_mut().zip(["system", "user"]) {
            unit["name"] = json!("web.service");
            unit["scope"] = json!(scope);
            unit["load"] = json!("loaded");
            unit["active"] = json!(active);
            unit["sub"] = json!("running");
            unit["enabled"] = json!("enabled");
        }
        json!({"services":[
            units[0], units[1]
        ],"timers":[],"user":{"available":true,"reason":null},"privilege":privilege})
    }
    fn target(unit: Option<&str>, scope: Scope) -> LogTarget {
        LogTarget {
            unit: unit.map(str::to_owned),
            scope,
        }
    }
    fn logs(start: usize, count: usize, cursor: Option<&str>) -> Value {
        json!({"entries":(start..start+count).map(|i| json!({"at":i,"priority":6,"pid":1,"identifier":"web","message":i.to_string()})).collect::<Vec<_>>(),"cursor":cursor})
    }

    #[test]
    fn snapshot_polling_is_visible_independent_and_coalesced() {
        let mut h = Harness::new();
        h.services.poll(false);
        assert_eq!(h.count("services_snapshot"), 0);
        h.services.poll(true);
        h.services.refresh();
        h.services.refresh();
        assert_eq!(h.count("services_snapshot"), 1);
        assert!(h.services.resource("snapshot").unwrap().manual_pending);
        h.reply("services_snapshot", Ok(snapshot("active", "root")));
        h.services.poll(true);
        assert_eq!(h.count("services_snapshot"), 1);
        h.advance(5);
        h.services.poll(false);
        assert_eq!(h.count("services_snapshot"), 1);
        h.services.poll(true);
        assert_eq!(h.count("services_snapshot"), 2);
        let updated = h.services.resource("snapshot").unwrap().updated_at;
        h.services.refresh();
        assert_eq!(h.services.resource("snapshot").unwrap().updated_at, updated);
        h.services
            .select_logs(target(None, Scope::System), None, true);
        h.services
            .select_unit_file("web.service".into(), Scope::System);
        assert_eq!(h.count("service_logs"), 1);
        assert_eq!(h.count("service_unit_file"), 1);
    }

    #[test]
    fn background_snapshot_only_publishes_display_changes_and_manual_completion() {
        let mut h = Harness::ready();
        let revision = h.services.snapshot.resource.revision;
        let data_revision = h.services.snapshot.resource.data_revision;
        h.advance(5);
        assert!(!h.services.poll(true), "cached read admission is quiet");
        assert!(h.services.snapshot.resource.pending);
        assert!(!h.services.snapshot.resource.display_pending());
        h.reply("services_snapshot", Ok(snapshot("active", "root")));
        assert!(!h.services.poll(true), "unchanged reading is quiet");
        assert!(h.services.snapshot.resource.revision > revision);
        assert_eq!(h.services.snapshot.resource.data_revision, data_revision);
        assert_eq!(h.services.snapshot.resource.updated_at, Some(h.now.get()));

        h.advance(5);
        assert!(!h.services.poll(true));
        h.reply("services_snapshot", Err("Unavailable".into()));
        assert!(h.services.poll(true), "new failure remains visible");
        h.advance(5);
        assert!(!h.services.poll(true));
        h.reply("services_snapshot", Err("Unavailable".into()));
        assert!(!h.services.poll(true), "same failure remains quiet");
        h.advance(5);
        assert!(!h.services.poll(true));
        h.reply("services_snapshot", Ok(snapshot("active", "root")));
        assert!(h.services.poll(true), "recovery clears the error");

        h.advance(5);
        assert!(!h.services.poll(true));
        h.services.refresh();
        assert!(
            h.services.poll(true),
            "manual refresh joins the pending read"
        );
        assert!(h.services.snapshot.resource.display_pending());
        h.reply("services_snapshot", Ok(snapshot("active", "root")));
        assert!(h.services.poll(true), "manual completion clears activity");
        assert!(!h.services.snapshot.resource.display_pending());
        h.advance(5);
        assert!(!h.services.poll(true));
        h.reply("services_snapshot", Ok(snapshot("inactive", "root")));
        assert!(h.services.poll(true), "changed service state is published");
    }

    #[test]
    fn unchanged_post_action_snapshot_publishes_restored_capabilities() {
        let mut h = Harness::ready();
        assert!(
            h.services
                .action("web.service".into(), Scope::System, ServiceVerb::Restart)
        );
        h.reply("service_action", Ok(json!({"ok":true})));
        assert!(h.services.poll(true));
        assert_eq!(h.services.take_action_completions().len(), 1);
        assert!(
            !h.services
                .can_action("web.service", Scope::System, ServiceVerb::Restart)
        );
        let data_revision = h.services.snapshot.resource.data_revision;
        h.reply("services_snapshot", Ok(snapshot("active", "root")));
        assert!(
            h.services.poll(true),
            "capability restoration requires a redraw"
        );
        assert_eq!(h.services.snapshot.resource.data_revision, data_revision);
        assert!(
            h.services
                .can_action("web.service", Scope::System, ServiceVerb::Restart)
        );
        assert!(h.services.take_action_completions().is_empty());
    }

    #[test]
    fn action_owns_scoped_unit_across_navigation_and_invalidates_both_read_races() {
        for read_first in [false, true] {
            let mut h = Harness::ready();
            h.services.refresh();
            assert!(
                h.services
                    .action("web.service".into(), Scope::System, ServiceVerb::Stop)
            );
            assert!(
                !h.services
                    .action("web.service".into(), Scope::System, ServiceVerb::Restart)
            );
            h.services.clear_selection();
            h.services.refresh();
            h.advance(10);
            h.services.poll(false);
            assert_eq!(h.count("services_snapshot"), 2);
            assert_eq!(h.count("service_action"), 1);
            assert!(
                h.services
                    .action_status("web.service", Scope::System)
                    .unwrap()
                    .pending
            );
            assert!(
                h.services
                    .action_status("web.service", Scope::User)
                    .is_none()
            );
            h.reply("services_snapshot", Ok(snapshot("inactive", "none")));
            if read_first {
                h.services.poll(false);
            }
            h.reply("service_action", Ok(json!({"ok":true})));
            h.services.poll(false);
            assert_eq!(h.count("services_snapshot"), 3);
            assert_eq!(h.services.snapshot().unwrap().services[0].active, "active");
            assert!(
                !h.services
                    .action_status("web.service", Scope::System)
                    .unwrap()
                    .pending
            );
            if read_first {
                // A refresh started while the action ran is invalidated again
                // when it settles. It must drain before the recovery read.
                h.reply("services_snapshot", Ok(snapshot("active", "none")));
                h.services.poll(false);
                assert_eq!(h.count("services_snapshot"), 4);
            }
            h.reply("services_snapshot", Ok(snapshot("inactive", "root")));
            h.services.poll(true);
            assert_eq!(
                h.services.snapshot().unwrap().services[0].active,
                "inactive"
            );
            assert_eq!(h.count("service_action"), 1);
        }
    }

    #[test]
    fn invalidated_snapshot_keeps_worker_slot_until_returned() {
        let mut h = Harness::ready();
        h.services.refresh();
        assert!(
            h.services
                .action("web.service".into(), Scope::System, ServiceVerb::Restart)
        );
        h.reply("service_action", Ok(json!({"ok":true})));
        h.services.poll(false);
        for _ in 0..20 {
            h.advance(5);
            h.services.refresh();
            h.services.poll(true);
        }
        assert_eq!(h.count("services_snapshot"), 2);
        h.reply("services_snapshot", Ok(snapshot("inactive", "none")));
        h.services.poll(false);
        assert_eq!(h.services.snapshot().unwrap().services[0].active, "active");
        assert_eq!(h.count("services_snapshot"), 3);
    }

    #[test]
    fn same_named_system_and_user_units_act_concurrently_with_same_unit_dedup() {
        let mut h = Harness::ready();
        assert!(
            h.services
                .action("web.service".into(), Scope::System, ServiceVerb::Restart)
        );
        assert!(
            h.services
                .action("web.service".into(), Scope::User, ServiceVerb::Stop)
        );
        assert!(
            !h.services
                .action("web.service".into(), Scope::System, ServiceVerb::Stop)
        );
        assert!(
            !h.services
                .action("web.service".into(), Scope::User, ServiceVerb::Restart)
        );
        h.services.clear_selection();
        h.services.poll(false);
        assert_eq!(h.count("service_action"), 2);
        assert!(
            h.services
                .action_status("web.service", Scope::System)
                .unwrap()
                .pending
        );
        assert!(
            h.services
                .action_status("web.service", Scope::User)
                .unwrap()
                .pending
        );
        h.reply_action("web.service", Scope::User, Ok(json!({"ok":true})));
        h.services.poll(false);
        assert!(h.services.resource("action").unwrap().pending);
        assert!(
            h.services
                .action_status("web.service", Scope::System)
                .unwrap()
                .pending
        );
        assert!(
            !h.services
                .action_status("web.service", Scope::User)
                .unwrap()
                .pending
        );
        let outcomes = h.services.take_action_completions();
        assert_eq!(outcomes.len(), 1);
        assert_eq!(outcomes[0].scope, Scope::User);
        assert_eq!(outcomes[0].verb, ServiceVerb::Stop);
        assert!(outcomes[0].result.as_ref().unwrap().ok);
        assert!(h.services.take_action_completions().is_empty());
    }

    #[test]
    fn overlapping_completions_reject_stale_snapshot_and_clear_only_settled_units() {
        let mut h = Harness::ready();
        assert!(
            h.services
                .action("web.service".into(), Scope::System, ServiceVerb::Restart)
        );
        assert!(
            h.services
                .action("web.service".into(), Scope::User, ServiceVerb::Stop)
        );
        h.services.refresh();
        h.reply("services_snapshot", Ok(snapshot("inactive", "none")));
        h.reply_action(
            "web.service",
            Scope::User,
            Err("user action interrupted".into()),
        );
        h.services.poll(false);
        assert_eq!(h.services.snapshot().unwrap().services[0].active, "active");
        assert_eq!(h.count("services_snapshot"), 3);
        assert!(
            !h.services
                .can_action("web.service", Scope::User, ServiceVerb::Restart)
        );
        let failure = h.services.take_action_completions();
        assert_eq!(failure.len(), 1);
        assert_eq!(
            failure[0].result.as_ref().unwrap_err(),
            "user action interrupted"
        );

        // The user unit can be operated again after its recovery read even
        // though the unrelated system action is still running.
        h.reply("services_snapshot", Ok(snapshot("active", "root")));
        h.services.poll(false);
        assert!(
            !h.services
                .can_action("web.service", Scope::System, ServiceVerb::Restart)
        );
        assert!(
            h.services
                .can_action("web.service", Scope::User, ServiceVerb::Restart)
        );
        assert!(
            h.services
                .action("web.service".into(), Scope::User, ServiceVerb::Restart)
        );
        h.reply_action("web.service", Scope::System, Ok(json!({"ok":true})));
        h.reply_action("web.service", Scope::User, Ok(json!({"ok":false,"needsPassword":true,"command":"systemctl --user restart web.service"})));
        h.services.poll(false);
        assert!(!h.services.resource("action").unwrap().pending);
        let outcomes = h.services.take_action_completions();
        assert_eq!(outcomes.len(), 2);
        assert!(
            outcomes
                .iter()
                .find(|outcome| outcome.scope == Scope::System)
                .unwrap()
                .result
                .as_ref()
                .unwrap()
                .ok
        );
        assert_eq!(
            outcomes
                .iter()
                .find(|outcome| outcome.scope == Scope::User)
                .unwrap()
                .result
                .as_ref()
                .unwrap()
                .needs_password,
            Some(true)
        );
        assert!(
            !h.services
                .can_action("web.service", Scope::System, ServiceVerb::Restart)
        );
        assert!(
            !h.services
                .can_action("web.service", Scope::User, ServiceVerb::Restart)
        );
        h.reply("services_snapshot", Ok(snapshot("active", "root")));
        h.services.poll(false);
        assert!(
            h.services
                .can_action("web.service", Scope::System, ServiceVerb::Restart)
        );
        assert!(
            h.services
                .can_action("web.service", Scope::User, ServiceVerb::Restart)
        );
        assert!(h.services.take_action_completions().is_empty());
    }

    fn many_units(count: usize) -> Value {
        let mut snapshot = snapshot("active", "root");
        let template = snapshot["services"][0].clone();
        snapshot["services"] = Value::Array(
            (0..count)
                .map(|index| {
                    let mut unit = template.clone();
                    unit["name"] = json!(format!("app{index}.service"));
                    unit
                })
                .collect(),
        );
        snapshot
    }

    #[test]
    fn concurrent_workers_are_bounded_without_queuing_or_replaying_writes() {
        let mut h = Harness::new();
        h.services.refresh();
        h.reply("services_snapshot", Ok(many_units(9)));
        h.services.poll(false);
        for index in 0..MAX_CONCURRENT_ACTIONS {
            assert!(h.services.action(
                format!("app{index}.service"),
                Scope::System,
                ServiceVerb::Restart
            ));
        }
        assert!(
            !h.services
                .action("app8.service".into(), Scope::System, ServiceVerb::Restart)
        );
        h.services
            .select_logs(target(None, Scope::System), None, true);
        h.services
            .select_unit_file("app0.service".into(), Scope::System);
        h.services.refresh();
        assert_eq!(h.services.pending.len(), MAX_CONCURRENT_ACTIONS + 3);
        h.reply_action("app0.service", Scope::System, Ok(json!({"ok":true})));
        h.services.poll(false);
        assert_eq!(h.count("service_action"), MAX_CONCURRENT_ACTIONS);
        assert!(
            h.services
                .action("app8.service".into(), Scope::System, ServiceVerb::Restart)
        );
        assert_eq!(h.services.pending_action_count(), MAX_CONCURRENT_ACTIONS);
        assert!(
            !h.services
                .action("app0.service".into(), Scope::System, ServiceVerb::Restart)
        );
    }

    #[test]
    fn bounded_action_history_never_evicts_a_pending_unit() {
        let mut h = Harness::new();
        let snapshot = many_units(81);
        h.services.refresh();
        h.reply("services_snapshot", Ok(snapshot.clone()));
        h.services.poll(false);
        assert!(
            h.services
                .action("app0.service".into(), Scope::System, ServiceVerb::Restart)
        );
        for index in 1..81 {
            let unit = format!("app{index}.service");
            assert!(
                h.services
                    .action(unit.clone(), Scope::System, ServiceVerb::Restart)
            );
            h.reply_action(&unit, Scope::System, Ok(json!({"ok":true})));
            h.services.poll(false);
            h.reply("services_snapshot", Ok(snapshot.clone()));
            h.services.poll(false);
        }
        assert_eq!(h.services.actions.len(), MAX_ACTIONS);
        assert!(
            h.services
                .action_status("app0.service", Scope::System)
                .unwrap()
                .pending
        );
        assert_eq!(h.services.pending_action_count(), 1);
        assert_eq!(
            h.services.take_action_completions().len(),
            MAX_ACTION_COMPLETIONS
        );
        assert!(h.services.take_action_completions().is_empty());
    }

    #[test]
    fn retirement_drains_every_concurrent_action_without_refreshing_replacement() {
        let mut h = Harness::ready();
        assert!(
            h.services
                .action("web.service".into(), Scope::System, ServiceVerb::Restart)
        );
        assert!(
            h.services
                .action("web.service".into(), Scope::User, ServiceVerb::Stop)
        );
        h.services.retire();
        let count = h.requests.borrow().len();
        h.reply_action("web.service", Scope::User, Err("disconnected".into()));
        h.services.poll(false);
        assert!(h.services.has_pending());
        assert!(h.services.resource("action").unwrap().pending);
        assert!(
            !h.services
                .action("web.service".into(), Scope::User, ServiceVerb::Stop)
        );
        h.reply_action("web.service", Scope::System, Ok(json!({"ok":true})));
        h.services.poll(false);
        assert!(!h.services.has_pending());
        assert!(!h.services.resource("action").unwrap().pending);
        assert_eq!(h.requests.borrow().len(), count);
        assert_eq!(h.services.take_action_completions().len(), 2);
        h.services.poll(true);
        assert!(h.services.take_action_completions().is_empty());
        assert_eq!(h.requests.borrow().len(), count);
    }

    #[test]
    fn failed_action_refetches_without_replay_and_handover_is_scoped() {
        let mut h = Harness::ready();
        assert!(
            h.services
                .action("web.service".into(), Scope::System, ServiceVerb::Stop)
        );
        h.reply("service_action", Err("remote disconnected".into()));
        h.services.poll(false);
        assert_eq!(h.count("services_snapshot"), 2);
        assert_eq!(
            h.services
                .action_status("web.service", Scope::System)
                .unwrap()
                .error
                .as_deref(),
            Some("remote disconnected")
        );
        assert!(
            h.services
                .action_status("web.service", Scope::User)
                .is_none()
        );
        h.reply("services_snapshot", Ok(snapshot("active", "password")));
        h.services.poll(false);
        assert_eq!(h.count("service_action"), 1);
        assert!(
            h.services
                .action("web.service".into(), Scope::System, ServiceVerb::Stop)
        );
        h.reply("service_action", Ok(json!({"ok":false,"needsPassword":true,"command":"sudo systemctl stop web.service"})));
        h.services.poll(false);
        let status = h
            .services
            .action_status("web.service", Scope::System)
            .unwrap();
        assert_eq!(status.result.as_ref().unwrap().needs_password, Some(true));
        assert_eq!(
            status.result.as_ref().unwrap().command.as_deref(),
            Some("sudo systemctl stop web.service")
        );
        h.services.clear_selection();
        h.services.poll(false);
        assert!(
            h.services
                .action_status("web.service", Scope::System)
                .unwrap()
                .result
                .is_some()
        );
        assert!(h.services.resource("action").unwrap().revision > 0);
    }

    #[test]
    fn actions_recheck_capability_state_and_input_before_dispatch() {
        let mut h = Harness::ready();
        for unit in [
            "missing.service",
            "web.timer",
            "-x.service",
            "x;shutdown.service",
        ] {
            assert!(
                !h.services
                    .action(unit.into(), Scope::System, ServiceVerb::Restart)
            );
        }
        assert!(
            !h.services
                .action("web.service".into(), Scope::System, ServiceVerb::Start)
        );
        assert!(
            !h.services
                .action("web.service".into(), Scope::System, ServiceVerb::Enable)
        );
        h.services.refresh();
        h.reply("services_snapshot", Ok(snapshot("active", "none")));
        h.services.poll(false);
        assert!(
            !h.services
                .action("web.service".into(), Scope::System, ServiceVerb::Restart)
        );
        assert!(
            h.services
                .action("web.service".into(), Scope::User, ServiceVerb::Restart)
        );
        assert_eq!(h.args("service_action")["scope"], "user");
        h.reply("service_action", Ok(json!({"ok":true})));
        h.services.poll(false);
        let mut unavailable = snapshot("active", "root");
        unavailable["user"]["available"] = json!(false);
        h.reply("services_snapshot", Ok(unavailable));
        h.services.poll(false);
        assert!(
            !h.services
                .action("web.service".into(), Scope::User, ServiceVerb::Restart)
        );
        h.services.refresh();
        h.reply("services_snapshot", Err("offline".into()));
        h.services.poll(false);
        assert!(
            !h.services
                .action("web.service".into(), Scope::System, ServiceVerb::Restart)
        );
        assert_eq!(h.count("service_action"), 1);
    }

    #[test]
    fn logs_append_by_cursor_cap_retention_and_pause_when_hidden_or_not_live() {
        let mut h = Harness::ready();
        h.services
            .select_logs(target(Some("web.service"), Scope::System), None, true);
        assert_eq!(
            h.args("service_logs"),
            json!({"id":"server-a","unit":"web.service","scope":"system","priority":null,"cursor":null,"lines":200})
        );
        h.reply("service_logs", Ok(logs(0, 200, Some("c1"))));
        h.services.poll(true);
        h.advance(2);
        h.services.poll(false);
        assert_eq!(h.count("service_logs"), 1);
        h.services.poll(true);
        assert_eq!(h.args("service_logs")["cursor"], "c1");
        assert!(h.args("service_logs")["lines"].is_null());
        h.reply("service_logs", Ok(logs(200, 1900, Some("c2"))));
        h.services.poll(true);
        let page = h.services.logs().unwrap();
        assert_eq!(page.entries.len(), MAX_ENTRIES);
        assert_eq!(page.entries[0].message, "100");
        assert_eq!(page.cursor.as_deref(), Some("c2"));
        h.services
            .select_logs(target(Some("web.service"), Scope::System), None, false);
        h.advance(20);
        h.services.poll(true);
        assert_eq!(h.count("service_logs"), 2);
        h.services.retry_logs();
        h.services.retry_logs();
        assert_eq!(h.count("service_logs"), 3);
        h.reply("service_logs", Ok(logs(2100, 0, None)));
        h.services.poll(false);
        assert_eq!(h.services.logs().unwrap().cursor.as_deref(), Some("c2"));
        assert_eq!(h.count("service_logs"), 3);
    }

    #[test]
    fn priority_cache_uses_separate_cursor_and_same_unit_placeholder_only() {
        let mut h = Harness::ready();
        h.services
            .select_logs(target(Some("web.service"), Scope::System), None, false);
        h.reply("service_logs", Ok(logs(0, 2, Some("all"))));
        h.services.poll(false);
        h.services.select_logs(
            target(Some("web.service"), Scope::System),
            Some("err".into()),
            false,
        );
        assert_eq!(h.services.logs().unwrap().entries.len(), 2);
        assert!(h.services.resource("logs").unwrap().value.is_null());
        assert!(h.args("service_logs")["cursor"].is_null());
        assert_eq!(h.args("service_logs")["priority"], "err");
        h.reply("service_logs", Ok(logs(10, 1, Some("err"))));
        h.services.poll(false);
        assert_eq!(h.services.logs().unwrap().entries[0].message, "10");
        h.services
            .select_logs(target(Some("web.service"), Scope::System), None, false);
        assert_eq!(h.services.logs().unwrap().cursor.as_deref(), Some("all"));
        assert_eq!(h.count("service_logs"), 2);
        h.services
            .select_logs(target(Some("web.service"), Scope::User), None, false);
        assert!(h.services.logs().is_none());
        assert_eq!(h.args("service_logs")["scope"], "user");
        h.reply("service_logs", Err("user journal denied".into()));
        h.services.poll(false);
        assert_eq!(
            h.services.resource("logs").unwrap().error.as_deref(),
            Some("user journal denied")
        );
        h.services
            .select_logs(target(Some("web.service"), Scope::System), None, false);
        assert!(h.services.resource("logs").unwrap().error.is_none());
        assert_eq!(h.services.logs().unwrap().cursor.as_deref(), Some("all"));
    }

    #[test]
    fn switched_targets_and_reset_ignore_old_completions_with_bounded_workers() {
        let mut h = Harness::ready();
        h.services
            .select_logs(target(Some("web.service"), Scope::System), None, true);
        for i in 0..50 {
            h.services.select_logs(
                target(Some(&format!("unit{i}.service")), Scope::System),
                None,
                true,
            );
            h.services.poll(false);
        }
        assert_eq!(h.count("service_logs"), 1);
        assert!(h.services.logs.len() <= MAX_LOG_TARGETS);
        assert!(h.services.pending.len() <= 4);
        h.reply("service_logs", Ok(logs(0, 200, Some("old"))));
        h.services.poll(false);
        assert!(h.services.logs().is_none());
        assert_eq!(h.count("service_logs"), 2);
        assert_eq!(h.args("service_logs")["unit"], "unit49.service");
        h.services.reset_logs();
        h.services.reset_logs();
        assert_eq!(h.count("service_logs"), 2);
        h.reply("service_logs", Ok(logs(0, 200, Some("stale"))));
        h.services.poll(false);
        assert!(h.services.logs().is_none());
        assert_eq!(h.count("service_logs"), 3);
        assert!(h.args("service_logs")["cursor"].is_null());
        assert_eq!(h.args("service_logs")["lines"], 200);
        h.reply("service_logs", Ok(logs(200, 1, Some("new"))));
        h.services.poll(false);
        assert_eq!(h.services.logs().unwrap().entries[0].message, "200");
    }

    #[test]
    fn cursorless_pages_replace_instead_of_duplicating_lines() {
        let mut h = Harness::ready();
        h.services
            .select_logs(target(None, Scope::System), None, true);
        for _ in 0..3 {
            h.reply("service_logs", Ok(logs(0, 2, None)));
            h.services.poll(false);
            assert_eq!(h.services.logs().unwrap().entries.len(), 2);
            h.advance(2);
            h.services.poll(true);
            assert_eq!(h.args("service_logs")["lines"], 200);
        }
    }

    #[test]
    fn another_action_requires_successful_post_write_snapshot() {
        let mut h = Harness::new();
        h.services.refresh();
        h.reply("services_snapshot", Ok(snapshot("inactive", "root")));
        h.services.poll(true);
        assert!(
            h.services
                .action("web.service".into(), Scope::System, ServiceVerb::Start)
        );
        h.reply("service_action", Ok(json!({"ok":true})));
        h.services.poll(false);
        // The retained typed snapshot still says stopped, but is no longer an
        // authorization source for the next write.
        assert_eq!(
            h.services.snapshot().unwrap().services[0].active,
            "inactive"
        );
        assert!(
            !h.services
                .can_action("web.service", Scope::System, ServiceVerb::Start)
        );
        assert!(
            !h.services
                .action("web.service".into(), Scope::System, ServiceVerb::Start)
        );
        h.reply("services_snapshot", Err("connection closed".into()));
        h.services.poll(false);
        assert!(
            !h.services
                .action("web.service".into(), Scope::System, ServiceVerb::Start)
        );
        h.services.refresh();
        h.reply("services_snapshot", Ok(snapshot("active", "root")));
        h.services.poll(false);
        assert!(
            !h.services
                .can_action("web.service", Scope::System, ServiceVerb::Start)
        );
        assert!(
            h.services
                .can_action("web.service", Scope::System, ServiceVerb::Stop)
        );
        h.services.refresh();
        assert!(
            h.services
                .can_action("web.service", Scope::System, ServiceVerb::Stop)
        );
        assert_eq!(h.count("service_action"), 1);
    }

    #[test]
    fn malformed_reads_keep_last_good_data_and_cannot_clear_dirty_state() {
        let mut h = Harness::ready();
        h.services
            .select_logs(target(None, Scope::System), None, false);
        h.reply("service_logs", Ok(logs(0, 2, Some("c1"))));
        h.services.poll(false);
        h.services.retry_logs();
        h.reply("service_logs", Ok(json!({})));
        h.services.poll(false);
        assert!(h.services.resource("logs").unwrap().error.is_some());
        assert_eq!(h.services.logs().unwrap().entries.len(), 2);
        assert!(
            h.services
                .action("web.service".into(), Scope::System, ServiceVerb::Restart)
        );
        h.reply("service_action", Ok(json!({"ok":true})));
        h.services.poll(false);
        h.reply("services_snapshot", Ok(json!({})));
        h.services.poll(false);
        assert!(h.services.resource("snapshot").unwrap().error.is_some());
        assert_eq!(h.services.snapshot().unwrap().services.len(), 2);
        assert!(
            !h.services
                .can_action("web.service", Scope::System, ServiceVerb::Restart)
        );
    }

    #[test]
    fn unit_files_are_lazy_scoped_cached_and_physically_bounded() {
        let mut h = Harness::ready();
        assert_eq!(h.count("service_unit_file"), 0);
        assert!(
            h.services
                .select_unit_file("web.service".into(), Scope::System)
        );
        h.services.retry_unit_file();
        h.reply("service_unit_file", Ok(json!({"text":"system file"})));
        h.services.poll(false);
        assert_eq!(h.count("service_unit_file"), 1);
        h.services
            .select_unit_file("web.service".into(), Scope::User);
        assert!(h.services.unit_file().is_none());
        h.reply("service_unit_file", Ok(json!({"text":"user file"})));
        h.services.poll(false);
        h.services
            .select_unit_file("web.service".into(), Scope::System);
        assert_eq!(h.services.unit_file().unwrap().text, "system file");
        assert_eq!(h.count("service_unit_file"), 2);
        for i in 0..100 {
            h.services
                .select_unit_file(format!("unit{i}.timer"), Scope::System);
        }
        assert_eq!(h.count("service_unit_file"), 3);
        assert_eq!(h.services.files.len(), MAX_FILES);
        h.reply("service_unit_file", Ok(json!({"text":"old timer"})));
        h.services.poll(false);
        assert!(h.services.unit_file().is_none());
        assert_eq!(h.args("service_unit_file")["unit"], "unit99.timer");
        h.services.clear_selection();
        h.reply("service_unit_file", Ok(json!({"text":"latest timer"})));
        h.services.poll(false);
        assert!(h.services.unit_file().is_none());
        assert_eq!(h.count("service_unit_file"), 4);
    }

    #[test]
    fn unit_files_revalidate_on_stale_reactivation_without_polling_or_losing_text() {
        let mut h = Harness::ready();
        h.services
            .select_unit_file("web.service".into(), Scope::System);
        h.reply("service_unit_file", Ok(json!({"text":"original file"})));
        h.services.poll(true);
        let updated = h.services.resource("unit-file").unwrap().updated_at;

        h.advance(9);
        h.services.clear_selection();
        h.services
            .select_unit_file("web.service".into(), Scope::System);
        assert_eq!(h.count("service_unit_file"), 1, "fresh cache is reused");
        h.advance(1);
        h.services.poll(true);
        h.services
            .select_unit_file("web.service".into(), Scope::System);
        assert_eq!(h.count("service_unit_file"), 1, "active files do not poll");

        h.services.clear_selection();
        h.services
            .select_unit_file("web.service".into(), Scope::System);
        h.services
            .select_unit_file("web.service".into(), Scope::System);
        assert_eq!(h.count("service_unit_file"), 2, "stale revisit coalesces");
        assert!(h.services.resource("unit-file").unwrap().pending);
        assert_eq!(h.services.unit_file().unwrap().text, "original file");
        h.reply(
            "service_unit_file",
            Err("file temporarily unavailable".into()),
        );
        h.services.poll(true);
        assert_eq!(
            h.services.resource("unit-file").unwrap().updated_at,
            updated
        );
        assert_eq!(h.services.unit_file().unwrap().text, "original file");
        h.advance(30);
        h.services.poll(true);
        assert_eq!(
            h.count("service_unit_file"),
            2,
            "failures do not retry in a loop"
        );

        h.services.poll(false);
        assert_eq!(
            h.count("service_unit_file"),
            2,
            "hidden page does not refresh files"
        );
        h.services.poll(true);
        assert_eq!(
            h.count("service_unit_file"),
            3,
            "reopening the page retries stale text"
        );
        h.services.poll(false);
        h.services.poll(true);
        assert_eq!(h.count("service_unit_file"), 3, "in-flight reads coalesce");
        h.reply("service_unit_file", Ok(json!({"text":"edited file"})));
        h.services.poll(true);
        assert_eq!(h.services.unit_file().unwrap().text, "edited file");
        assert!(h.services.resource("unit-file").unwrap().error.is_none());
        h.services.poll(false);
        h.services.poll(true);
        assert_eq!(
            h.count("service_unit_file"),
            3,
            "fresh page revisit reuses text"
        );
    }

    #[test]
    fn failed_reads_preserve_freshness_and_recover_without_replaying_actions() {
        let mut h = Harness::ready();
        let updated = h.services.resource("snapshot").unwrap().updated_at;
        h.advance(5);
        h.services.poll(true);
        h.disconnect("services_snapshot");
        h.services.poll(true);
        assert_eq!(h.services.resource("snapshot").unwrap().updated_at, updated);
        assert!(h.services.resource("snapshot").unwrap().error.is_some());
        assert!(!h.services.resource("snapshot").unwrap().pending);
        h.advance(4);
        h.services.poll(true);
        assert_eq!(h.count("services_snapshot"), 2);
        h.advance(1);
        h.services.poll(true);
        assert_eq!(h.count("services_snapshot"), 3);
        h.reply("services_snapshot", Ok(snapshot("active", "root")));
        h.services.poll(true);
        assert!(h.services.resource("snapshot").unwrap().error.is_none());
        assert!(
            h.services
                .action("web.service".into(), Scope::System, ServiceVerb::Restart)
        );
        h.disconnect("service_action");
        h.services.poll(false);
        assert!(
            !h.services
                .action_status("web.service", Scope::System)
                .unwrap()
                .pending
        );
        assert!(
            h.services
                .action_status("web.service", Scope::System)
                .unwrap()
                .error
                .is_some()
        );
        assert_eq!(h.count("service_action"), 1);
        assert_eq!(h.count("services_snapshot"), 4);
    }

    #[test]
    fn retirement_drains_existing_workers_without_any_further_dispatch() {
        let mut h = Harness::ready();
        h.services.refresh();
        h.services
            .select_logs(target(None, Scope::System), None, true);
        h.services
            .select_unit_file("web.service".into(), Scope::System);
        assert!(
            h.services
                .action("web.service".into(), Scope::System, ServiceVerb::Stop)
        );
        assert_eq!(h.services.pending.len(), 4);
        h.services.retire();
        assert!(h.services.has_pending());
        assert!(h.services.snapshot().is_none());
        assert!(h.services.logs().is_none());
        assert!(h.services.unit_file().is_none());
        let count = h.requests.borrow().len();
        h.services.refresh();
        h.services.retry_logs();
        h.services.reset_logs();
        h.services.retry_unit_file();
        assert!(
            !h.services
                .select_logs(target(None, Scope::System), None, true)
        );
        assert!(
            !h.services
                .select_unit_file("web.service".into(), Scope::System)
        );
        assert!(
            !h.services
                .action("web.service".into(), Scope::System, ServiceVerb::Restart)
        );
        h.reply("service_action", Ok(json!({"ok":true})));
        h.reply("services_snapshot", Ok(snapshot("inactive", "root")));
        h.reply("service_logs", Ok(logs(0, 200, Some("old"))));
        h.reply("service_unit_file", Ok(json!({"text":"old"})));
        h.advance(100);
        h.services.poll(true);
        assert_eq!(h.requests.borrow().len(), count);
        assert!(!h.services.has_pending());
        assert!(h.services.snapshot().is_none());
        assert!(
            h.services
                .action_status("web.service", Scope::System)
                .unwrap()
                .result
                .as_ref()
                .unwrap()
                .ok
        );
        h.services.poll(false);
        assert_eq!(h.requests.borrow().len(), count);
    }

    #[test]
    fn invalid_selection_and_action_outcomes_do_not_create_false_success() {
        let mut h = Harness::ready();
        assert!(
            !h.services
                .select_logs(target(Some("../web.service"), Scope::System), None, true)
        );
        assert!(
            !h.services
                .select_logs(target(None, Scope::System), Some("debug".into()), true)
        );
        assert!(
            !h.services
                .select_unit_file("missing socket".into(), Scope::System)
        );
        assert_eq!(h.count("service_logs"), 0);
        assert!(
            h.services
                .action("web.service".into(), Scope::System, ServiceVerb::Stop)
        );
        h.reply("service_action", Ok(json!({"ok":false})));
        h.services.poll(false);
        let status = h
            .services
            .action_status("web.service", Scope::System)
            .unwrap();
        assert!(status.error.is_some());
        assert!(status.result.is_none());
        assert_eq!(h.services.resource("action").unwrap().revision, 0);
    }

    #[test]
    fn dispatch_failure_settles_without_repeating_mutations() {
        let mut h = Harness::ready();
        let count = Rc::new(Cell::new(0));
        let calls = count.clone();
        h.services.dispatch = Box::new(move |_, _| {
            calls.set(calls.get() + 1);
            Err("thread unavailable".into())
        });
        assert!(
            !h.services
                .action("web.service".into(), Scope::System, ServiceVerb::Stop)
        );
        assert!(!h.services.has_pending());
        assert!(
            !h.services
                .action_status("web.service", Scope::System)
                .unwrap()
                .pending
        );
        h.services.poll(false);
        assert_eq!(count.get(), 2); // action failure plus required snapshot recovery
        assert!(!h.services.poll(false));
        assert_eq!(count.get(), 2);
        assert_eq!(request_timeout("service_action"), Duration::from_secs(90));
        assert_eq!(
            request_timeout("services_snapshot"),
            Duration::from_secs(40)
        );
        assert_eq!(request_timeout("service_logs"), Duration::from_secs(30));
    }
}
