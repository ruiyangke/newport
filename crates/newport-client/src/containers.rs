//! Retained Containers requests for one immutable server connection.
//!
//! Selection uses a stable container name; reads and mutations capture its
//! current immutable ID. Every kind has one physical worker, including after
//! selection changes or invalidation. Hidden controllers drain their workers
//! and retain mutations without scheduling background reads.
use crate::containers_stats::{StatsCache, StatsHistory};
use crate::{backend::Backend, resources::Resource};
use newport_domain::containers::{
    available_container_verbs, can_remove_container, container_shell_command, display_name,
    forwardable, valid_container, valid_container_id as valid_id, valid_project, with_volumes,
};
use newport_domain::{forwarding, forwarding_batch::Destination};
use newport_protocol::{
    desktop::{self, containers as ipc},
    monitor::{
        compose::{self, Action as ComposeAction, Project as ComposeProject},
        containers::{
            self as wire, Container, ContainerInspect, ContainerLogEntry, ContainerLogPage,
            ContainerPort, ContainerState, ContainerVerb, ContainersSnapshot, DockerDisk,
            ProjectVerb, PruneTarget, Pruned,
        },
    },
};
mod reply;
use std::{
    collections::{HashMap, HashSet, VecDeque},
    sync::{Arc, mpsc},
    thread,
    time::{Duration, Instant},
};

const SNAPSHOT_INTERVAL: Duration = Duration::from_secs(5);
const LOG_INTERVAL: Duration = Duration::from_secs(2);
const INSPECT_STALE: Duration = Duration::from_secs(15);
const COMPOSE_STALE: Duration = Duration::from_secs(30);
const DISK_STALE: Duration = Duration::from_secs(30);
/// Measuring volume sizes walks them on disk; keep a reading for longer.
const DISK_VOLUMES_STALE: Duration = Duration::from_secs(120);
const MAX_ENTRIES: usize = 2_000;
const MAX_INSPECTS: usize = 32;
const MAX_LOGS: usize = 8;
const MAX_PROJECTS: usize = 16;
const MAX_OUTPUTS: usize = 8;
const MAX_ACTIONS: usize = 32;
const MAX_COMPLETIONS: usize = 64;
const MAX_FINISHED_JOBS: usize = 256;
const MAX_OUTPUT_BYTES: usize = 1_048_576;
const WORKER_ERROR: &str =
    "Containers worker stopped before confirming the result. Refresh before trying again.";

type ReplyOutcome = Result<ipc::Reply, String>;
type Dispatch = dyn Fn(ipc::Operation) -> Result<mpsc::Receiver<ReplyOutcome>, String>;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum ActionTarget {
    Container(String),
    Project(String),
    Remove(Vec<String>),
    Prune(PruneTarget),
}

/// A confirmed operation captures IDs, never a mutable selection or name alone.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    Container {
        container: String,
        name: String,
        verb: ContainerVerb,
    },
    Project {
        project: String,
        verb: ProjectVerb,
        expected_ids: Vec<String>,
    },
    Remove {
        containers: Vec<String>,
        names: Vec<String>,
    },
    Prune {
        what: PruneTarget,
    },
}
impl Action {
    pub fn target(&self) -> ActionTarget {
        match self {
            Self::Container { container, .. } => ActionTarget::Container(container.clone()),
            Self::Project { project, .. } => ActionTarget::Project(project.clone()),
            Self::Remove { containers, .. } => {
                let mut ids = containers.clone();
                ids.sort();
                ActionTarget::Remove(ids)
            }
            Self::Prune { what } => ActionTarget::Prune(*what),
        }
    }
}

#[derive(Clone, Debug)]
pub struct ActionStatus {
    pub action: Action,
    pub pending: bool,
    pub error: Option<String>,
}
#[derive(Clone, Debug)]
pub struct ActionCompletion {
    pub action: Action,
    /// The agent's receipt, already checked against the action's kind.
    pub result: Result<ipc::Reply, String>,
}
impl ActionCompletion {
    /// What a cleanup removed and freed.
    pub fn pruned(&self) -> Option<Pruned> {
        match &self.result {
            Ok(ipc::Reply::Prune(pruned)) => Some(**pruned),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Selection {
    Container(String),
    Project(String),
}
#[derive(Clone, Debug, PartialEq, Eq)]
enum LogTarget {
    Container(String),
    Project(String),
}

/// Stable identity and entry positions within one cached log dataset.
/// Request revisions may advance with no new lines; these positions do not.
#[derive(Clone, Copy, Debug)]
pub struct LogWindow<'a> {
    pub entries: &'a [ContainerLogEntry],
    /// Unique to this cache allocation/reset within the retained controller.
    pub epoch: u64,
    /// Sequence number of the oldest retained entry, including evicted prefixes.
    pub first: u64,
    /// Exclusive total of accepted entries, including entries no longer retained.
    pub end: u64,
}

struct LogCache {
    slot: Slot<ContainerLogPage>,
    epoch: u64,
    first: u64,
    end: u64,
}
impl LogCache {
    fn new(epoch: u64) -> Self {
        Self {
            slot: Slot::default(),
            epoch,
            first: 0,
            end: 0,
        }
    }
    fn complete(
        &mut self,
        generation: u64,
        result: Result<ContainerLogPage, String>,
        now: Instant,
    ) -> bool {
        // A response superseded by reset/eviction cannot advance the dataset.
        if self.slot.generation != generation {
            return false;
        }
        let mut added = 0;
        let result = result.map(|mut page| {
            added = page.entries.len() as u64;
            if let Some(previous) = &self.slot.data {
                let mut entries = previous.entries.clone();
                entries.append(&mut page.entries);
                page.entries = entries;
                page.cursor = page.cursor.or_else(|| previous.cursor.clone());
            }
            let excess = page.entries.len().saturating_sub(MAX_ENTRIES);
            page.entries.drain(..excess);
            page
        });
        let accepted = finish(&mut self.slot, generation, result, now);
        if accepted && self.slot.resource.error.is_none() {
            self.end += added;
            self.first = self.end - self.slot.data.as_ref().unwrap().entries.len() as u64;
        }
        accepted
    }
}

struct Slot<T> {
    resource: Resource,
    data: Option<T>,
    generation: u64,
    dirty: bool,
}
impl<T> Default for Slot<T> {
    fn default() -> Self {
        Self {
            resource: Resource::default(),
            data: None,
            generation: 0,
            dirty: true,
        }
    }
}
impl<T> Slot<T> {
    fn invalidate(&mut self) {
        self.generation = 0;
        self.dirty = true;
        self.resource.pending = false;
        self.resource.manual_pending = false;
    }
    fn stale(&self, now: Instant, age: Duration) -> bool {
        self.dirty
            || self
                .resource
                .updated_at
                .is_none_or(|at| now.saturating_duration_since(at) >= age)
    }
    fn begin(&mut self, generation: u64) -> &mut Resource {
        self.generation = generation;
        &mut self.resource
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Kind {
    Snapshot,
    Inspect,
    Logs,
    Compose,
    Disk,
    DiskVolumes,
    Jobs,
    Output,
    Action,
    ComposeStart,
    ComposeCancel,
}
const READS: [Kind; 8] = [
    Kind::Snapshot,
    Kind::Inspect,
    Kind::Compose,
    Kind::Disk,
    Kind::DiskVolumes,
    Kind::Logs,
    Kind::Jobs,
    Kind::Output,
];
#[derive(Clone, Debug, PartialEq)]
enum Job {
    Snapshot,
    Inspect(String),
    Logs(LogTarget),
    Compose(String),
    Disk,
    DiskVolumes,
    Jobs,
    Output(String),
    Action(Action),
    ComposeStart {
        project: String,
        action: ComposeAction,
    },
    ComposeCancel {
        job: String,
        project: String,
    },
}
impl Job {
    fn kind(&self) -> Kind {
        match self {
            Self::Snapshot => Kind::Snapshot,
            Self::Inspect(_) => Kind::Inspect,
            Self::Logs(_) => Kind::Logs,
            Self::Compose(_) => Kind::Compose,
            Self::Disk => Kind::Disk,
            Self::DiskVolumes => Kind::DiskVolumes,
            Self::Jobs => Kind::Jobs,
            Self::Output(_) => Kind::Output,
            Self::Action(_) => Kind::Action,
            Self::ComposeStart { .. } => Kind::ComposeStart,
            Self::ComposeCancel { .. } => Kind::ComposeCancel,
        }
    }
}
struct Pending {
    job: Job,
    generation: u64,
    receiver: mpsc::Receiver<ReplyOutcome>,
}
struct ComposeReceipt {
    project: String,
    job: String,
    generation: u64,
}

pub struct Containers {
    server: String,
    retired: bool,
    visible: bool,
    selection: Option<Selection>,
    selected_id: Option<String>,
    selected_project: Option<String>,
    selected_logs: Option<LogTarget>,
    logs_active: bool,
    live: bool,
    disk_active: bool,
    snapshot: Slot<ContainersSnapshot>,
    stats: StatsCache,
    inspects: VecDeque<(String, Slot<ContainerInspect>)>,
    logs: VecDeque<(LogTarget, LogCache)>,
    next_log_epoch: u64,
    projects: VecDeque<(String, Slot<ComposeProject>)>,
    /// The quick reading; volumes may be listed without sizes.
    disk: Slot<DockerDisk>,
    /// Volume sizes, measured apart because they take seconds.
    disk_volumes: Slot<DockerDisk>,
    /// The quick reading with measured volumes merged in, for presentation.
    disk_view: Option<DockerDisk>,
    /// Advances whenever `disk_view` changes, including when only the
    /// separately measured volumes arrive.
    disk_view_revision: u64,
    jobs: Slot<compose::Jobs>,
    outputs: VecDeque<(String, Slot<compose::Job>)>,
    selected_job: Option<String>,
    activity_active: bool,
    started_jobs: HashMap<String, ComposeReceipt>,
    cancelling_jobs: HashMap<String, ComposeReceipt>,
    finished_jobs: VecDeque<String>,
    observed_compose_jobs: VecDeque<(String, String)>,
    compose_completions: VecDeque<compose::Job>,
    compose_start: Slot<compose::Started>,
    compose_start_project: Option<String>,
    compose_cancel: Slot<compose::Done>,
    compose_cancel_job: Option<String>,
    actions: VecDeque<(ActionTarget, ActionStatus)>,
    action_completions: VecDeque<ActionCompletion>,
    action_resource: Resource,
    wanted: HashSet<Kind>,
    manual: HashSet<Kind>,
    pending: HashMap<Kind, Pending>,
    next: u64,
    changed: bool,
    dispatch: Box<Dispatch>,
    now: Box<dyn Fn() -> Instant>,
}

impl Containers {
    pub fn new(backend: Backend, server: String) -> Self {
        let backend = Arc::new(backend);
        Self::with_dispatch(
            server,
            Box::new(move |operation| {
                let (sender, receiver) = mpsc::sync_channel(1);
                let backend = backend.clone();
                thread::Builder::new()
                    .name("newport-containers".into())
                    .spawn(move || {
                        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                            let timeout = request_timeout(&operation);
                            let response = backend
                                .execute_with_timeout(
                                    desktop::Operation::Containers(Box::new(operation)),
                                    timeout,
                                )
                                .map_err(|error| error.to_string())?;
                            let desktop::Reply::Containers(response) = response else {
                                return Err("Unexpected Containers response family.".into());
                            };
                            Ok(*response)
                        }))
                        .unwrap_or_else(|_| Err(WORKER_ERROR.into()));
                        let _ = sender.send(result);
                    })
                    .map_err(|error| format!("Could not start Containers request: {error}"))?;
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
            visible: false,
            selection: None,
            selected_id: None,
            selected_project: None,
            selected_logs: None,
            logs_active: false,
            live: false,
            disk_active: false,
            snapshot: Slot::default(),
            stats: StatsCache::default(),
            inspects: VecDeque::new(),
            logs: VecDeque::new(),
            next_log_epoch: 0,
            projects: VecDeque::new(),
            disk: Slot::default(),
            disk_volumes: Slot::default(),
            disk_view: None,
            disk_view_revision: 0,
            jobs: Slot::default(),
            outputs: VecDeque::new(),
            selected_job: None,
            activity_active: false,
            started_jobs: HashMap::new(),
            cancelling_jobs: HashMap::new(),
            finished_jobs: VecDeque::new(),
            observed_compose_jobs: VecDeque::new(),
            compose_completions: VecDeque::new(),
            compose_start: Slot::default(),
            compose_start_project: None,
            compose_cancel: Slot::default(),
            compose_cancel_job: None,
            actions: VecDeque::new(),
            action_completions: VecDeque::new(),
            action_resource: Resource::default(),
            wanted: HashSet::new(),
            manual: HashSet::new(),
            pending: HashMap::new(),
            next: 0,
            changed: false,
            dispatch,
            now,
        }
    }

    pub fn resource(&self, key: &str) -> Option<&Resource> {
        match key {
            "snapshot" => Some(&self.snapshot.resource),
            "container-inspect" => {
                Some(&slot(&self.inspects, self.selected_id.as_ref()?)?.resource)
            }
            "container-logs" => Some(
                &slot(&self.logs, self.selected_logs.as_ref()?)?
                    .slot
                    .resource,
            ),
            "compose-detail" => {
                Some(&slot(&self.projects, self.selected_project.as_ref()?)?.resource)
            }
            "docker-disk" => Some(&self.disk.resource),
            "docker-disk-volumes" => Some(&self.disk_volumes.resource),
            "action" => Some(&self.action_resource),
            "compose-jobs" => Some(&self.jobs.resource),
            "compose-output" => Some(&slot(&self.outputs, self.selected_job.as_ref()?)?.resource),
            "compose-start" => (self.compose_start_project.is_some()
                && self.compose_start_project == self.selected_project)
                .then_some(&self.compose_start.resource),
            "compose-cancel" => (self.compose_cancel_job.is_some()
                && self.compose_cancel_job == self.selected_job)
                .then_some(&self.compose_cancel.resource),
            _ => None,
        }
    }
    pub fn snapshot(&self) -> Option<&ContainersSnapshot> {
        self.snapshot.data.as_ref()
    }
    /// Statistics belong to this immutable server connection and full engine
    /// ID. Reading or changing selection never appends a sample.
    pub fn stats_history(&self, container_id: &str) -> Option<&StatsHistory> {
        self.stats.get(container_id)
    }
    pub fn inspect(&self) -> Option<&ContainerInspect> {
        slot(&self.inspects, self.selected_id.as_ref()?)?
            .data
            .as_ref()
    }
    pub fn logs(&self) -> Option<&ContainerLogPage> {
        slot(&self.logs, self.selected_logs.as_ref()?)?
            .slot
            .data
            .as_ref()
    }
    /// An allocated target has an empty window even while its first read is
    /// loading. Use `logs()`/`resource()` for loading and error presentation.
    pub fn log_window(&self) -> Option<LogWindow<'_>> {
        let cache = slot(&self.logs, self.selected_logs.as_ref()?)?;
        Some(LogWindow {
            entries: cache
                .slot
                .data
                .as_ref()
                .map_or(&[], |page| page.entries.as_slice()),
            epoch: cache.epoch,
            first: cache.first,
            end: cache.end,
        })
    }
    pub fn compose_project(&self) -> Option<&ComposeProject> {
        slot(&self.projects, self.selected_project.as_ref()?)?
            .data
            .as_ref()
    }
    pub fn disk(&self) -> Option<&DockerDisk> {
        self.disk_view.as_ref()
    }
    /// Changes whenever [`Self::disk`] does; derived presentation keys on it.
    pub fn disk_revision(&self) -> u64 {
        self.disk_view_revision
    }
    /// Every listed job across projects, newest first, without output.
    pub fn jobs(&self) -> Option<&compose::Jobs> {
        self.jobs.data.as_ref()
    }
    fn job_rows(&self) -> &[compose::Job] {
        self.jobs().map_or(&[], |jobs| &jobs.jobs)
    }
    pub fn selected_job(&self) -> Option<&str> {
        self.selected_job.as_deref()
    }
    /// The selected job with its retained contiguous output suffix.
    pub fn job_output(&self) -> Option<&compose::Job> {
        slot(&self.outputs, self.selected_job.as_ref()?)?
            .data
            .as_ref()
    }
    pub fn selected_name(&self) -> Option<&str> {
        match &self.selection {
            Some(Selection::Container(name)) => Some(name),
            _ => None,
        }
    }
    pub fn selected_project(&self) -> Option<&str> {
        self.selected_project.as_deref()
    }
    pub fn selected_container(&self) -> Option<&Container> {
        let name = self.selected_name()?;
        self.snapshot()?
            .containers
            .iter()
            .find(|container| container.name == name)
    }
    pub fn has_pending(&self) -> bool {
        !self.pending.is_empty()
    }
    /// Physical mutation ownership is global to this connection; status text
    /// from `resource` remains scoped to the selected project or job.
    pub fn compose_run_pending(&self) -> bool {
        self.pending.contains_key(&Kind::ComposeStart)
    }
    pub fn compose_cancel_pending(&self) -> bool {
        self.pending.contains_key(&Kind::ComposeCancel)
    }
    /// The job the selected project's last Compose command started, unless
    /// that start failed.
    pub fn compose_started(&self) -> Option<&str> {
        self.resource("compose-start")
            .filter(|row| row.error.is_none())
            .and(self.compose_start.data.as_ref())
            .map(|started| started.job.as_str())
    }
    pub fn action_status(&self, target: &ActionTarget) -> Option<&ActionStatus> {
        self.actions
            .iter()
            .find(|(key, _)| key == target)
            .map(|(_, status)| status)
    }
    /// Single-container progress also includes an exact member of a pending
    /// batch removal. Project-wide commands have their own progress surface.
    pub fn pending_container_action(&self, container_id: &str) -> Option<&ActionStatus> {
        self.actions
            .iter()
            .map(|(_, status)| status)
            .find(|status| {
                status.pending
                    && match &status.action {
                        Action::Container { container, .. } => container == container_id,
                        Action::Remove { containers, .. } => {
                            containers.iter().any(|id| id == container_id)
                        }
                        Action::Project { .. } | Action::Prune { .. } => false,
                    }
            })
    }
    pub fn take_action_completions(&mut self) -> Vec<ActionCompletion> {
        self.action_completions.drain(..).collect()
    }
    /// Terminal outcomes for jobs this connection observed running or started.
    /// Historical finished jobs stay silent; each queued receipt drains once.
    pub fn take_compose_completions(&mut self) -> Vec<compose::Job> {
        self.compose_completions.drain(..).collect()
    }

    /// Drains existing physical requests without dispatching anything else.
    pub fn retire(&mut self) {
        self.retired = true;
        self.clear_selection();
        self.snapshot = Slot::default();
        self.stats = StatsCache::default();
        self.inspects.clear();
        self.logs.clear();
        self.projects.clear();
        self.outputs.clear();
        self.disk = Slot::default();
        self.disk_volumes = Slot::default();
        self.disk_view = None;
        self.jobs = Slot::default();
        self.selected_job = None;
        self.started_jobs.clear();
        self.cancelling_jobs.clear();
        self.observed_compose_jobs.clear();
        self.compose_completions.clear();
        self.wanted.clear();
        self.manual.clear();
        self.changed = true;
    }
    pub fn refresh(&mut self) {
        self.request(Kind::Snapshot, true);
    }
    pub fn server_id(&self) -> &str {
        &self.server
    }
    pub fn retry_inspect(&mut self) {
        self.request(Kind::Inspect, true);
    }
    pub fn retry_compose(&mut self) {
        self.request(Kind::Compose, true);
    }
    pub fn retry_logs(&mut self) {
        self.request(Kind::Logs, true);
    }
    pub fn retry_disk(&mut self) {
        self.request(Kind::Disk, true);
        // A refresh re-measures volumes too, but only where the quick reading
        // could not size them; engines before API 1.42 size them in it.
        if self
            .disk
            .data
            .as_ref()
            .is_some_and(|disk| !disk.volumes_measured)
        {
            self.request(Kind::DiskVolumes, true);
        }
    }
    /// Re-measure volume sizes alone, after that measurement failed.
    pub fn retry_disk_volumes(&mut self) {
        self.request(Kind::DiskVolumes, true);
    }
    pub fn retry_jobs(&mut self) {
        self.request(Kind::Jobs, true);
    }
    pub fn retry_job_output(&mut self) {
        self.request(Kind::Output, true);
    }
    /// Call when opening a details tab whose selection has not changed.
    pub fn revalidate_details(&mut self) {
        if self.retired {
            return;
        }
        self.want_stale_details((self.now)());
        for kind in [Kind::Inspect, Kind::Compose, Kind::Disk] {
            self.read(kind);
        }
    }
    pub fn reset_logs(&mut self) {
        if self.retired {
            return;
        }
        if let Some(key) = self.selected_logs.clone() {
            let replacement = self.new_log_cache();
            if let Some(slot) = slot_mut(&mut self.logs, &key) {
                *slot = replacement;
            }
            self.changed = true;
            self.request(Kind::Logs, true);
        }
    }
    pub fn select_container(&mut self, name: String) -> bool {
        if self.retired || !valid_container(&name) {
            return false;
        }
        self.select(Selection::Container(name));
        true
    }
    pub fn select_project(&mut self, project: String) -> bool {
        if self.retired || !valid_project(&project) {
            return false;
        }
        self.select(Selection::Project(project));
        true
    }
    fn select(&mut self, selection: Selection) {
        if self.selection.as_ref() != Some(&selection) {
            self.selection = Some(selection);
            self.selected_job = None;
            self.wanted.remove(&Kind::Output);
            self.changed = true;
        }
        self.reconcile_selection();
        self.want_stale_details((self.now)());
        self.drive_reads();
    }
    pub fn clear_selection(&mut self) {
        self.selection = None;
        self.selected_id = None;
        self.selected_project = None;
        self.selected_logs = None;
        self.selected_job = None;
        self.activity_active = false;
        self.logs_active = false;
        self.live = false;
        for kind in [Kind::Inspect, Kind::Compose, Kind::Logs, Kind::Output] {
            self.wanted.remove(&kind);
            self.manual.remove(&kind);
        }
        self.changed = true;
    }
    pub fn set_logs_active(&mut self, active: bool, live: bool) {
        if self.retired {
            return;
        }
        let newly_active = active && !self.logs_active;
        let newly_live = live && !self.live;
        self.changed |= self.logs_active != active || self.live != live;
        self.logs_active = active;
        self.live = live;
        if active && (newly_live || newly_active && self.logs().is_none()) {
            self.request(Kind::Logs, false);
        }
        if !active {
            self.wanted.remove(&Kind::Logs);
        }
    }
    pub fn set_disk_active(&mut self, active: bool) {
        if self.retired {
            return;
        }
        let newly_active = active && !self.disk_active;
        self.disk_active = active;
        if newly_active && self.disk.stale((self.now)(), DISK_STALE) {
            self.request(Kind::Disk, false);
        }
        if !active {
            for kind in [Kind::Disk, Kind::DiskVolumes] {
                self.wanted.remove(&kind);
                self.manual.remove(&kind);
            }
        }
    }

    pub fn can_action(&self, action: &Action) -> bool {
        self.action_blocked_reason(action).is_none()
    }
    /// Build a terminal handoff for the exact observed container. This never
    /// dispatches a request or resolves the target through the current selection.
    pub fn shell_command(&self, container_id: &str, name: &str) -> Result<String, &'static str> {
        if let Some(reason) = self.shell_blocked_reason(container_id, name) {
            return Err(reason);
        }
        container_shell_command(container_id, self.snapshot().unwrap().engine.podman)
            .ok_or("The selected container identity is invalid.")
    }
    pub fn shell_blocked_reason(&self, container_id: &str, name: &str) -> Option<&'static str> {
        self.running_container(
            container_id,
            name,
            "Start the container to open a shell in it.",
        )
        .err()
    }
    /// Resolve a click against the latest accepted identity and exact binding.
    /// This prepares a destination only; transport ownership belongs to the
    /// forwarding controller and is independent of the page selection.
    pub fn port_destination(
        &self,
        container_id: &str,
        name: &str,
        port: &ContainerPort,
    ) -> Result<Destination, &'static str> {
        let container = self.running_container(
            container_id,
            name,
            "Start the container to reach this port.",
        )?;
        if !container.ports.contains(port) {
            return Err("The selected port binding has changed. Review the current container.");
        }
        if !forwardable(port) {
            return Err(
                "Only published TCP ports bound to loopback or every address can be forwarded.",
            );
        }
        Ok(Destination {
            address: forwarding::host(
                port.ip
                    .as_deref()
                    .filter(|address| !address.is_empty())
                    .unwrap_or("0.0.0.0"),
            ),
            port: port.host_port.expect("forwardable published port"),
            name: display_name(container).to_owned(),
        })
    }
    fn running_container(
        &self,
        container_id: &str,
        name: &str,
        stopped_reason: &'static str,
    ) -> Result<&Container, &'static str> {
        if let Some(reason) = self.engine_change_blocked_reason() {
            return Err(reason);
        }
        let container = self
            .snapshot()
            .expect("checked successful engine snapshot")
            .containers
            .iter()
            .find(|container| {
                valid_id(container_id) && container.id == container_id && container.name == name
            })
            .ok_or("The selected container has changed. Review the current selection.")?;
        if container.state != ContainerState::Running {
            return Err(stopped_reason);
        }
        if container
            .compose
            .as_ref()
            .is_some_and(|labels| self.project_busy(&labels.project))
        {
            return Err("Wait for this project's Compose operation to finish.");
        }
        Ok(container)
    }
    pub fn action_blocked_reason(&self, action: &Action) -> Option<&'static str> {
        if let Some(reason) = self.engine_change_blocked_reason() {
            return Some(reason);
        }
        let snapshot = self.snapshot().expect("checked successful engine snapshot");
        let project_busy = match action {
            Action::Container { container, .. } => snapshot
                .containers
                .iter()
                .filter(|row| row.id == *container)
                .filter_map(|row| row.compose.as_ref())
                .any(|labels| self.project_busy(&labels.project)),
            Action::Project { project, .. } => self.project_busy(project),
            Action::Remove { containers, .. } => snapshot
                .containers
                .iter()
                .filter(|row| containers.contains(&row.id))
                .filter_map(|row| row.compose.as_ref())
                .any(|labels| self.project_busy(&labels.project)),
            Action::Prune { .. } => self.any_project_busy(),
        };
        if project_busy {
            return Some("Wait for this project's Compose operation to finish.");
        }
        let allowed = match action {
            Action::Container {
                container,
                name,
                verb,
            } => {
                valid_id(container)
                    && snapshot.containers.iter().any(|c| {
                        c.id == *container
                            && c.name == *name
                            && available_container_verbs(c).contains(verb)
                    })
            }
            Action::Project {
                project,
                expected_ids,
                ..
            } => {
                let mut current: Vec<_> = snapshot
                    .containers
                    .iter()
                    .filter(|c| {
                        c.compose
                            .as_ref()
                            .is_some_and(|labels| labels.project == *project && !labels.oneoff)
                    })
                    .map(|c| c.id.clone())
                    .collect();
                let mut expected = expected_ids.clone();
                current.sort();
                expected.sort();
                valid_project(project) && valid_ids(expected_ids) && expected == current
            }
            Action::Remove { containers, names } => {
                valid_ids(containers)
                    && containers.len() == names.len()
                    && containers.iter().zip(names).all(|(id, name)| {
                        snapshot
                            .containers
                            .iter()
                            .any(|c| c.id == *id && c.name == *name && can_remove_container(c))
                    })
            }
            Action::Prune { .. } => true,
        };
        (!allowed).then_some("The selected containers or their available actions have changed. Review the current selection.")
    }
    fn engine_change_blocked_reason(&self) -> Option<&'static str> {
        if self.retired {
            return Some("The server connection has changed.");
        }
        if self.pending.contains_key(&Kind::Action) {
            return Some("Wait for the current container operation to finish.");
        }
        if self.snapshot.resource.error.is_some() {
            return Some("Refresh containers successfully before changing them.");
        }
        if self.snapshot.dirty {
            return Some("Waiting for current container state.");
        }
        if self.snapshot().is_none() {
            return Some("Waiting for current container state.");
        }
        None
    }
    fn project_busy(&self, project: &str) -> bool {
        let observed_running = self.job_rows().iter().any(|job| {
            job.project == project
                && job.state.is_running()
                && !self.finished_jobs.contains(&job.job)
        });
        let output_running = self.outputs.iter().any(|(id, slot)| {
            slot.data.as_ref().is_some_and(|job| {
                job.project == project
                    && job.state.is_running()
                    && !self.finished_jobs.contains(id)
                    // The list includes every running job. A newer list can
                    // supersede an old output cache after a finished job expires.
                    && (self.jobs.dirty || self.jobs.resource.revision < slot.resource.revision)
            })
        });
        let starting = self.pending.get(&Kind::ComposeStart).is_some_and(|pending| {
            matches!(&pending.job, Job::ComposeStart { project: target, .. } if target == project)
        });
        // A successful start may precede its first jobs-list response. Keep its
        // ownership until that post-start list or a terminal output is observed.
        let awaiting_observation = self.started_jobs.contains_key(project);
        let cancelling = self
            .pending
            .get(&Kind::ComposeCancel)
            .is_some_and(|pending| {
                matches!(&pending.job, Job::ComposeCancel { project: target, .. } if target == project)
            }) || self.cancelling_jobs.values().any(|receipt| receipt.project == project);
        observed_running || output_running || starting || awaiting_observation || cancelling
    }
    fn any_project_busy(&self) -> bool {
        self.pending.contains_key(&Kind::ComposeStart)
            || self.pending.contains_key(&Kind::ComposeCancel)
            || !self.started_jobs.is_empty()
            || !self.cancelling_jobs.is_empty()
            || self
                .job_rows()
                .iter()
                .any(|job| self.project_busy(&job.project))
            || self
                .outputs
                .iter()
                .filter_map(|(_, slot)| slot.data.as_ref())
                .any(|job| self.project_busy(&job.project))
    }

    pub fn can_compose_run(&self, project: &str) -> bool {
        self.compose_run_blocked_reason(project).is_none()
    }
    pub fn compose_run_blocked_reason(&self, project: &str) -> Option<&'static str> {
        if let Some(reason) = self.engine_change_blocked_reason() {
            return Some(reason);
        }
        if !valid_project(project) {
            return Some("Choose a valid Compose project.");
        }
        if self.pending.contains_key(&Kind::ComposeStart) {
            return Some("Wait for the Compose command to start.");
        }
        if self.project_busy(project) {
            return Some("Wait for this project's Compose operation to finish.");
        }
        if self.started_jobs.len() >= MAX_ACTIONS {
            return Some("Refresh Compose jobs before starting another command.");
        }
        let Some(slot) = slot(&self.projects, &project.to_owned()) else {
            return Some("Read this project's Compose configuration before running a command.");
        };
        if slot.resource.error.is_some() {
            return Some(
                "Refresh this project's Compose configuration successfully before running a command.",
            );
        }
        if slot.dirty || slot.data.is_none() {
            return Some("Waiting for current Compose configuration.");
        }
        // The reply has no project field; its request-keyed slot owns the
        // project identity.
        if slot.data.as_ref().unwrap().cli.is_none() {
            return Some("Install the Compose CLI on the server to run this command.");
        }
        // `configError` describes the optional `config --format json` read,
        // not action capability. The agent separately rechecks files and CLI
        // when starting a command; Down can even run without readable files.
        None
    }
    /// One confirmed engine mutation at a time; statuses and receipts retain
    /// their original target across navigation and connection retirement.
    pub fn action(&mut self, action: Action) -> bool {
        if !self.can_action(&action) {
            return false;
        }
        let target = action.target();
        self.actions.retain(|(old, _)| old != &target);
        self.actions.push_back((
            target,
            ActionStatus {
                action: action.clone(),
                pending: true,
                error: None,
            },
        ));
        while self.actions.len() > MAX_ACTIONS {
            self.actions.pop_front();
        }
        self.invalidate_readings();
        self.launch(Job::Action(action), true)
    }

    pub fn select_job(&mut self, job: String) -> bool {
        if self.retired || !valid_job(&job) {
            return false;
        }
        self.select_job_internal(job);
        self.read(Kind::Output);
        true
    }
    /// Resolve Activity selection without dispatching or stealing a different project's job.
    pub fn preferred_job(&self, project: &str) -> Option<&str> {
        if self.retired {
            return None;
        }
        if let Some(job) = self.selected_job.as_deref()
            && self.job_belongs_to(project, job)
        {
            return Some(job);
        }
        if let Some(receipt) = self.started_jobs.get(project) {
            return Some(&receipt.job);
        }
        self.job_rows()
            .iter()
            .find(|job| job.project == project)
            .map(|job| job.job.as_str())
    }
    fn job_belongs_to(&self, project: &str, job: &str) -> bool {
        self.started_jobs
            .get(project)
            .is_some_and(|receipt| receipt.job == job)
            || self
                .job_rows()
                .iter()
                .any(|row| row.job == job && row.project == project)
            || slot(&self.outputs, &job.to_owned()).is_some_and(|slot| {
                slot.data
                    .as_ref()
                    .is_some_and(|output| output.project == project)
                    && (self.jobs.dirty || self.jobs.resource.revision < slot.resource.revision)
            })
    }
    /// Check a captured Activity target before changing the current selection.
    pub fn has_project_job(&self, project: &str, job: &str) -> bool {
        !self.retired && valid_job(job) && self.job_belongs_to(project, job)
    }
    pub fn select_project_job(&mut self, project: &str, job: String) -> bool {
        if self.selected_project.as_deref() != Some(project) || !self.job_belongs_to(project, &job)
        {
            return false;
        }
        self.select_job(job)
    }
    pub fn clear_job_selection(&mut self) {
        self.selected_job = None;
        self.wanted.remove(&Kind::Output);
        self.manual.remove(&Kind::Output);
        self.changed = true;
    }
    /// Automatic output reads run only while Activity is shown. Explicit selection
    /// or retry still requests one read; an existing worker always settles.
    pub fn set_activity_active(&mut self, active: bool) {
        if self.retired {
            return;
        }
        let newly_active = active && !self.activity_active;
        self.changed |= self.activity_active != active;
        self.activity_active = active;
        if newly_active
            && self.selected_job.is_some()
            && (self.job_output().is_none_or(|job| job.state.is_running())
                || self
                    .resource("compose-output")
                    .is_some_and(|row| row.error.is_some()))
        {
            self.wanted.insert(Kind::Output);
            if self.visible {
                self.read(Kind::Output);
            }
        }
        if !active {
            self.wanted.remove(&Kind::Output);
        }
    }
    fn select_job_internal(&mut self, job: String) {
        touch(&mut self.outputs, &job, MAX_OUTPUTS);
        if self.selected_job.as_ref() != Some(&job) {
            self.selected_job = Some(job);
            self.wanted.remove(&Kind::Output);
            self.manual.remove(&Kind::Output);
            self.changed = true;
        }
        if self
            .job_output()
            .is_none_or(|output| output.state.is_running())
        {
            self.wanted.insert(Kind::Output);
        }
    }
    pub fn compose_run(&mut self, project: String, action: ComposeAction) -> bool {
        if action.validate().is_err() || !self.can_compose_run(&project) {
            return false;
        }
        self.compose_start_project = Some(project.clone());
        self.launch(Job::ComposeStart { project, action }, true)
    }
    pub fn compose_cancel(&mut self, job: String) -> bool {
        if self.retired
            || !valid_job(&job)
            || self.pending.contains_key(&Kind::ComposeCancel)
            || self.compose_cancelling(&job)
            || self.cancelling_jobs.len() >= MAX_ACTIONS
        {
            return false;
        }
        let Some(project) = self.running_job_project(&job).map(str::to_owned) else {
            return false;
        };
        self.compose_cancel_job = Some(job.clone());
        self.launch(Job::ComposeCancel { job, project }, true)
    }
    pub fn compose_cancelling(&self, job: &str) -> bool {
        self.cancelling_jobs.contains_key(job)
            || self.pending.get(&Kind::ComposeCancel).is_some_and(|pending| matches!(&pending.job, Job::ComposeCancel { job: target, .. } if target == job))
    }
    fn running_job_project(&self, job: &str) -> Option<&str> {
        if self.finished_jobs.iter().any(|finished| finished == job) {
            return None;
        }
        if let Some(output) = slot(&self.outputs, &job.to_owned())
            && (self.jobs.dirty || self.jobs.resource.revision < output.resource.revision)
            && let Some(data) = output.data.as_ref()
        {
            return data.state.is_running().then_some(data.project.as_str());
        }
        self.job_rows()
            .iter()
            .find(|row| row.job == job && row.state.is_running())
            .map(|row| row.project.as_str())
    }
    fn observe_compose_running(&mut self, job: &str, project: &str) {
        if self.retired
            || !valid_job(job)
            || !valid_project(project)
            || self.finished_jobs.iter().any(|id| id == job)
            || self.observed_compose_jobs.iter().any(|(id, _)| id == job)
        {
            return;
        }
        self.observed_compose_jobs
            .push_back((job.to_owned(), project.to_owned()));
        while self.observed_compose_jobs.len() > MAX_FINISHED_JOBS {
            self.observed_compose_jobs.pop_front();
        }
    }
    fn compose_completion(&self, job: &compose::Job) -> Option<compose::Job> {
        (!self.retired
            && !job.state.is_running()
            && self
                .observed_compose_jobs
                .iter()
                .any(|(id, project)| job.job == *id && job.project == *project))
        .then(|| job.clone())
    }
    fn finish_compose_observation(&mut self, job: &str, receipt: Option<compose::Job>) {
        if self.compose_job_completed(job)
            && let Some(receipt) = receipt
        {
            self.compose_completions.push_back(receipt);
            while self.compose_completions.len() > MAX_COMPLETIONS {
                self.compose_completions.pop_front();
            }
        }
    }
    /// One invalidation per observed finished job, including jobs whose output
    /// was never opened. Repeated list/output polls do not reset fresh readings.
    pub fn compose_job_completed(&mut self, job_id: &str) -> bool {
        if self.retired || !valid_job(job_id) || self.finished_jobs.iter().any(|id| id == job_id) {
            return false;
        }
        self.finished_jobs.push_back(job_id.to_owned());
        self.observed_compose_jobs.retain(|(id, _)| id != job_id);
        self.started_jobs.retain(|_, receipt| receipt.job != job_id);
        self.cancelling_jobs.remove(job_id);
        while self.finished_jobs.len() > MAX_FINISHED_JOBS {
            self.finished_jobs.pop_front();
        }
        self.invalidate_readings();
        self.changed = true;
        true
    }

    pub fn poll(&mut self, visible: bool) -> bool {
        let now = (self.now)();
        let reactivated = visible && !self.visible;
        self.changed |= self.visible != visible;
        self.visible = visible;
        // Settle mutations and job completion observations before accepting a
        // concurrent read, so their invalidations reject pre-mutation replies.
        for kind in [
            Kind::Action,
            Kind::ComposeStart,
            Kind::ComposeCancel,
            Kind::Jobs,
            Kind::Output,
            Kind::Snapshot,
            Kind::Inspect,
            Kind::Compose,
            Kind::Disk,
            Kind::DiskVolumes,
            Kind::Logs,
        ] {
            let result =
                self.pending
                    .get(&kind)
                    .and_then(|pending| match pending.receiver.try_recv() {
                        Ok(result) => Some(result),
                        Err(mpsc::TryRecvError::Empty) => None,
                        Err(mpsc::TryRecvError::Disconnected) => Some(Err(WORKER_ERROR.into())),
                    });
            if let Some(result) = result {
                let pending = self.pending.remove(&kind).unwrap();
                self.complete(pending.job, pending.generation, result, now);
            }
        }
        if visible && !self.retired {
            if reactivated {
                self.want_stale_details(now);
            }
            if due(&self.snapshot.resource, now, SNAPSHOT_INTERVAL) {
                self.wanted.insert(Kind::Snapshot);
            }
            let jobs_interval = if self.job_rows().iter().any(|job| job.state.is_running()) {
                Duration::from_millis(1_500)
            } else {
                Duration::from_secs(15)
            };
            if due(&self.jobs.resource, now, jobs_interval) {
                self.wanted.insert(Kind::Jobs);
            }
            if self.logs_active
                && self.live
                && self
                    .resource("container-logs")
                    .is_some_and(|row| due(row, now, LOG_INTERVAL))
            {
                self.wanted.insert(Kind::Logs);
            }
            if self.activity_active
                && self.selected_job.is_some()
                && self.job_output().is_none_or(|job| job.state.is_running())
                && self
                    .resource("compose-output")
                    .is_some_and(|row| due(row, now, Duration::from_secs(1)))
            {
                self.wanted.insert(Kind::Output);
            }
            self.drive_reads();
        }
        std::mem::take(&mut self.changed)
    }

    fn reconcile_selection(&mut self) {
        let (id, project) = match &self.selection {
            Some(Selection::Container(name)) => self
                .snapshot()
                .and_then(|snapshot| snapshot.containers.iter().find(|c| c.name == *name))
                .map(|c| {
                    (
                        Some(c.id.clone()),
                        c.compose.as_ref().map(|labels| labels.project.clone()),
                    )
                })
                .unwrap_or_default(),
            Some(Selection::Project(project)) => (None, Some(project.clone())),
            None => (None, None),
        };
        if self.selected_id != id {
            self.selected_id = id.clone();
            self.wanted.remove(&Kind::Inspect);
            self.manual.remove(&Kind::Inspect);
            if let Some(id) = &id {
                touch(&mut self.inspects, id, MAX_INSPECTS);
                if slot(&self.inspects, id)
                    .is_some_and(|slot| slot.stale((self.now)(), INSPECT_STALE))
                {
                    self.wanted.insert(Kind::Inspect);
                }
            }
            self.changed = true;
        }
        if self.selected_project != project {
            self.selected_project = project.clone();
            self.wanted.remove(&Kind::Compose);
            self.manual.remove(&Kind::Compose);
            if let Some(project) = &project {
                touch(&mut self.projects, project, MAX_PROJECTS);
                if slot(&self.projects, project)
                    .is_some_and(|slot| slot.stale((self.now)(), COMPOSE_STALE))
                {
                    self.wanted.insert(Kind::Compose);
                }
            }
            self.changed = true;
        }
        let logs = match &self.selection {
            Some(Selection::Container(_)) => id.map(LogTarget::Container),
            Some(Selection::Project(_)) => project.map(LogTarget::Project),
            None => None,
        };
        if self.selected_logs != logs {
            self.selected_logs = logs.clone();
            self.wanted.remove(&Kind::Logs);
            self.manual.remove(&Kind::Logs);
            if let Some(logs) = &logs {
                self.touch_logs(logs);
                if self.logs_active && self.logs().is_none() {
                    self.wanted.insert(Kind::Logs);
                }
            }
            self.changed = true;
        }
    }
    fn new_log_cache(&mut self) -> LogCache {
        self.next_log_epoch += 1;
        LogCache::new(self.next_log_epoch)
    }
    fn touch_logs(&mut self, key: &LogTarget) {
        let cache = self
            .logs
            .iter()
            .position(|(candidate, _)| candidate == key)
            .and_then(|index| self.logs.remove(index))
            .map(|(_, cache)| cache)
            .unwrap_or_else(|| self.new_log_cache());
        self.logs.push_back((key.clone(), cache));
        while self.logs.len() > MAX_LOGS {
            self.logs.pop_front();
        }
    }
    fn want_stale_details(&mut self, now: Instant) {
        if self
            .selected_id
            .as_ref()
            .and_then(|id| slot(&self.inspects, id))
            .is_some_and(|slot| slot.stale(now, INSPECT_STALE))
        {
            self.wanted.insert(Kind::Inspect);
        }
        if self
            .selected_project
            .as_ref()
            .and_then(|project| slot(&self.projects, project))
            .is_some_and(|slot| slot.stale(now, COMPOSE_STALE))
        {
            self.wanted.insert(Kind::Compose);
        }
        if self.disk_active && self.disk.stale(now, DISK_STALE) {
            self.wanted.insert(Kind::Disk);
        }
    }
    fn invalidate_readings(&mut self) {
        self.snapshot.invalidate();
        self.disk.invalidate();
        self.disk_volumes.invalidate();
        for (_, slot) in &mut self.inspects {
            slot.invalidate();
        }
        for (_, slot) in &mut self.projects {
            slot.invalidate();
        }
        if !self.retired {
            self.wanted.insert(Kind::Snapshot);
            if self.selected_id.is_some() {
                self.wanted.insert(Kind::Inspect);
            }
            if self.selected_project.is_some() {
                self.wanted.insert(Kind::Compose);
            }
            if self.disk_active {
                self.wanted.insert(Kind::Disk);
            }
        }
        self.changed = true;
    }
    fn merge_disk(&mut self) {
        let view = self
            .disk
            .data
            .clone()
            .map(|quick| match &self.disk_volumes.data {
                Some(volumes) => with_volumes(quick, volumes),
                None => quick,
            });
        if view != self.disk_view {
            self.changed = true;
            self.disk_view_revision += 1;
            self.disk_view = view;
        }
    }
    fn request(&mut self, kind: Kind, manual: bool) {
        if self.retired {
            return;
        }
        self.wanted.insert(kind);
        if manual {
            self.manual.insert(kind);
        }
        self.read(kind);
    }
    fn drive_reads(&mut self) {
        for kind in READS {
            self.read(kind);
        }
    }
    fn read(&mut self, kind: Kind) {
        if self.retired || !self.wanted.contains(&kind) {
            return;
        }
        if matches!(kind, Kind::Disk | Kind::DiskVolumes) && (!self.visible || !self.disk_active) {
            return;
        }
        if self.pending.contains_key(&Kind::Action)
            && matches!(
                kind,
                Kind::Snapshot | Kind::Inspect | Kind::Compose | Kind::Disk | Kind::DiskVolumes
            )
        {
            return;
        }
        let job = match kind {
            Kind::Snapshot => Some(Job::Snapshot),
            Kind::Inspect => self.selected_id.clone().map(Job::Inspect),
            Kind::Compose => self.selected_project.clone().map(Job::Compose),
            Kind::Logs => self.selected_logs.clone().map(Job::Logs),
            Kind::Disk => Some(Job::Disk),
            Kind::DiskVolumes => Some(Job::DiskVolumes),
            Kind::Jobs => Some(Job::Jobs),
            Kind::Output => self.selected_job.clone().map(Job::Output),
            _ => None,
        };
        let Some(job) = job else {
            self.wanted.remove(&kind);
            self.manual.remove(&kind);
            return;
        };
        if let Some(pending) = self.pending.get(&kind) {
            if pending.job == job && self.job_generation(&job) == Some(pending.generation) {
                let manual = self.manual.remove(&kind);
                self.wanted.remove(&kind);
                if manual && let Some(row) = self.job_resource_mut(&job) {
                    let changed = !row.manual_pending;
                    row.manual_pending = true;
                    self.changed |= changed;
                }
            }
            return;
        }
        self.wanted.remove(&kind);
        let manual = self.manual.remove(&kind);
        self.launch(job, manual);
    }
    fn operation(&self, job: &Job, refresh: bool) -> Result<ipc::Operation, String> {
        let server = self
            .server
            .parse()
            .map_err(|_| "Invalid server identity.".to_owned())?;
        Ok(match job {
            Job::Snapshot => {
                ipc::Operation::Snapshot(Box::new(ipc::SnapshotArgs { server, refresh }))
            }
            Job::Inspect(container) => ipc::Operation::Inspect(Box::new(ipc::InspectArgs {
                server,
                container: container.clone(),
                refresh,
            })),
            Job::Compose(project) => {
                ipc::Operation::ComposeProject(Box::new(ipc::ComposeProjectArgs {
                    server,
                    project: project.clone(),
                    refresh,
                }))
            }
            Job::Logs(target) => {
                let page = slot(&self.logs, target).and_then(|cache| cache.slot.data.as_ref());
                let target = match target {
                    LogTarget::Container(container) => {
                        wire::LogTarget::Container(container.clone())
                    }
                    LogTarget::Project(project) => wire::LogTarget::Project(project.clone()),
                };
                ipc::Operation::Logs(Box::new(ipc::LogsArgs {
                    server,
                    request: wire::LogsRequest {
                        target,
                        cursor: page.and_then(|page| page.cursor.clone()),
                        lines: page.is_none().then_some(300),
                    },
                }))
            }
            Job::Output(job) => ipc::Operation::ComposeJob(Box::new(ipc::ComposeJobArgs {
                server,
                job: job.clone(),
                offset: slot(&self.outputs, job)
                    .and_then(|slot| slot.data.as_ref())
                    .map_or(0, |output| output.offset),
            })),
            Job::Disk | Job::DiskVolumes => ipc::Operation::Disk(Box::new(ipc::DiskArgs {
                server,
                part: if matches!(job, Job::Disk) {
                    ipc::DiskPart::Quick
                } else {
                    ipc::DiskPart::Volumes
                },
                refresh,
            })),
            Job::Jobs => ipc::Operation::ComposeJobs(Box::new(ipc::ComposeJobsArgs { server })),
            Job::ComposeStart { project, action } => {
                ipc::Operation::ComposeRun(Box::new(ipc::ComposeRunArgs {
                    server,
                    project: project.clone(),
                    action: action.clone(),
                }))
            }
            Job::ComposeCancel { job, .. } => {
                ipc::Operation::ComposeCancel(Box::new(ipc::ComposeCancelArgs {
                    server,
                    job: job.clone(),
                }))
            }
            Job::Action(action) => match action {
                Action::Container {
                    container, verb, ..
                } => ipc::Operation::Action(Box::new(ipc::ActionArgs {
                    server,
                    container: container.clone(),
                    verb: *verb,
                })),
                Action::Project {
                    project,
                    verb,
                    expected_ids,
                } => ipc::Operation::ProjectAction(Box::new(ipc::ProjectActionArgs {
                    server,
                    project: project.clone(),
                    expected_ids: expected_ids.clone(),
                    verb: *verb,
                })),
                Action::Remove { containers, .. } => {
                    ipc::Operation::Remove(Box::new(ipc::RemoveArgs {
                        server,
                        containers: containers.clone(),
                    }))
                }
                Action::Prune { what } => ipc::Operation::Prune(Box::new(ipc::PruneArgs {
                    server,
                    what: *what,
                })),
            },
        })
    }
    fn job_generation(&self, job: &Job) -> Option<u64> {
        Some(match job {
            Job::Snapshot => self.snapshot.generation,
            Job::Inspect(id) => slot(&self.inspects, id)?.generation,
            Job::Logs(target) => slot(&self.logs, target)?.slot.generation,
            Job::Compose(project) => slot(&self.projects, project)?.generation,
            Job::Disk => self.disk.generation,
            Job::DiskVolumes => self.disk_volumes.generation,
            Job::Jobs => self.jobs.generation,
            Job::Output(job) => slot(&self.outputs, job)?.generation,
            Job::ComposeStart { .. } => self.compose_start.generation,
            Job::ComposeCancel { .. } => self.compose_cancel.generation,
            Job::Action(_) => return None,
        })
    }
    fn job_resource_mut(&mut self, job: &Job) -> Option<&mut Resource> {
        Some(match job {
            Job::Snapshot => &mut self.snapshot.resource,
            Job::Inspect(id) => &mut slot_mut(&mut self.inspects, id)?.resource,
            Job::Logs(target) => &mut slot_mut(&mut self.logs, target)?.slot.resource,
            Job::Compose(project) => &mut slot_mut(&mut self.projects, project)?.resource,
            Job::Disk => &mut self.disk.resource,
            Job::DiskVolumes => &mut self.disk_volumes.resource,
            Job::Jobs => &mut self.jobs.resource,
            Job::Output(job) => &mut slot_mut(&mut self.outputs, job)?.resource,
            Job::ComposeStart { .. } => &mut self.compose_start.resource,
            Job::ComposeCancel { .. } => &mut self.compose_cancel.resource,
            Job::Action(_) => &mut self.action_resource,
        })
    }
    fn launch(&mut self, job: Job, manual: bool) -> bool {
        if self.retired || self.pending.contains_key(&job.kind()) {
            return false;
        }
        self.next += 1;
        let generation = self.next;
        let row = match &job {
            Job::Snapshot => self.snapshot.begin(generation),
            Job::Inspect(id) => slot_mut(&mut self.inspects, id).unwrap().begin(generation),
            Job::Logs(target) => slot_mut(&mut self.logs, target)
                .unwrap()
                .slot
                .begin(generation),
            Job::Compose(project) => slot_mut(&mut self.projects, project)
                .unwrap()
                .begin(generation),
            Job::Disk => self.disk.begin(generation),
            Job::DiskVolumes => self.disk_volumes.begin(generation),
            Job::Jobs => self.jobs.begin(generation),
            Job::Output(job) => slot_mut(&mut self.outputs, job).unwrap().begin(generation),
            Job::ComposeStart { .. } => self.compose_start.begin(generation),
            Job::ComposeCancel { .. } => self.compose_cancel.begin(generation),
            Job::Action(_) => &mut self.action_resource,
        };
        row.pending = true;
        row.manual_pending = manual;
        if row.updated_at.is_none()
            || matches!(
                job,
                Job::Action(_) | Job::ComposeStart { .. } | Job::ComposeCancel { .. }
            )
        {
            row.error = None;
        }
        self.changed |= row.display_pending();
        let request = self.operation(&job, manual);
        match request.and_then(|request| (self.dispatch)(request)) {
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
    fn complete(&mut self, job: Job, generation: u64, result: ReplyOutcome, now: Instant) {
        // Mutation receipts still settle after retirement; read results cannot
        // repopulate a retired connection's caches or schedule new requests.
        if self.retired
            && !matches!(
                job,
                Job::Action(_) | Job::ComposeStart { .. } | Job::ComposeCancel { .. }
            )
        {
            return;
        }
        let accepted = match job {
            Job::Snapshot => {
                let accepted = finish(
                    &mut self.snapshot,
                    generation,
                    result.and_then(reply::snapshot),
                    now,
                );
                if accepted && self.snapshot.resource.error.is_none() {
                    self.stats.record(self.snapshot.data.as_ref().unwrap(), now);
                    self.reconcile_selection();
                }
                accepted
            }
            Job::Inspect(id) => {
                let result = result.and_then(reply::inspect).and_then(|inspect| {
                    if inspect.id == id {
                        Ok(inspect)
                    } else {
                        Err("Inspect response does not match the selected container.".into())
                    }
                });
                slot_mut(&mut self.inspects, &id)
                    .is_some_and(|slot| finish_changed(slot, generation, result, now))
            }
            Job::Logs(target) => slot_mut(&mut self.logs, &target)
                .is_some_and(|cache| cache.complete(generation, result.and_then(reply::logs), now)),
            Job::Compose(project) => slot_mut(&mut self.projects, &project).is_some_and(|slot| {
                finish_changed(
                    slot,
                    generation,
                    result.and_then(reply::compose_project),
                    now,
                )
            }),
            Job::Disk => {
                let accepted = finish_changed(
                    &mut self.disk,
                    generation,
                    result.and_then(reply::disk),
                    now,
                );
                if accepted {
                    self.merge_disk();
                    // Volumes are measured only when the quick reading could
                    // not size them, so engines that measure on the quick
                    // reading never scan twice. A
                    // volume created since the last measurement is measured
                    // at once rather than when that measurement goes stale.
                    if self.disk_view.as_ref().is_some_and(|d| !d.volumes_measured)
                        && (self.disk_volumes.data.is_some()
                            || self.disk_volumes.stale(now, DISK_VOLUMES_STALE))
                    {
                        self.wanted.insert(Kind::DiskVolumes);
                    }
                }
                accepted
            }
            Job::DiskVolumes => {
                let accepted = finish_changed(
                    &mut self.disk_volumes,
                    generation,
                    result.and_then(reply::disk),
                    now,
                );
                if accepted {
                    self.merge_disk();
                }
                accepted
            }
            Job::Jobs => {
                let result = result.and_then(reply::compose_jobs).and_then(validate_jobs);
                let accepted = finish(&mut self.jobs, generation, result, now);
                if accepted && self.jobs.resource.error.is_none() {
                    let observed: HashMap<String, (String, bool)> = self
                        .job_rows()
                        .iter()
                        .map(|job| {
                            (
                                job.job.clone(),
                                (job.project.clone(), job.state.is_running()),
                            )
                        })
                        .collect();
                    for (job, (project, running)) in &observed {
                        if *running {
                            self.observe_compose_running(job, project);
                        }
                    }
                    let revision = self.jobs.resource.revision;
                    self.started_jobs
                        .retain(|_, receipt| revision <= receipt.generation);
                    self.cancelling_jobs.retain(|job, receipt| {
                        revision <= receipt.generation
                            || observed.get(job).is_some_and(|(project, running)| {
                                *running && *project == receipt.project
                            })
                    });
                    let completed: Vec<_> = self
                        .job_rows()
                        .iter()
                        .filter(|job| !job.state.is_running())
                        .map(|job| (job.job.clone(), self.compose_completion(job)))
                        .collect();
                    for (job, receipt) in completed {
                        self.finish_compose_observation(&job, receipt);
                    }
                }
                accepted
            }
            Job::Output(job) => {
                let accepted = if let Some(slot) = slot_mut(&mut self.outputs, &job) {
                    let result = result
                        .and_then(reply::compose_job)
                        .and_then(|next| append_output(&job, slot.data.as_ref(), next));
                    finish(slot, generation, result, now)
                } else {
                    false
                };
                if accepted
                    && let Some(value) = slot(&self.outputs, &job)
                        .filter(|slot| slot.resource.error.is_none())
                        .and_then(|slot| slot.data.as_ref())
                {
                    if value.state.is_running() {
                        let project = value.project.clone();
                        self.observe_compose_running(&job, &project);
                    } else {
                        let receipt = self.compose_completion(value);
                        self.finish_compose_observation(&job, receipt);
                    }
                }
                accepted
            }
            Job::Action(action) => {
                self.invalidate_readings();
                let result = result.and_then(|reply| validate_action_result(&action, reply));
                if let Some((_, status)) = self
                    .actions
                    .iter_mut()
                    .find(|(target, _)| *target == action.target())
                {
                    *status = ActionStatus {
                        action: action.clone(),
                        pending: false,
                        error: result.as_ref().err().cloned(),
                    };
                }
                let settled = result.as_ref().map(|_| false).map_err(Clone::clone);
                self.action_completions
                    .push_back(ActionCompletion { action, result });
                while self.action_completions.len() > MAX_COMPLETIONS {
                    self.action_completions.pop_front();
                }
                self.action_resource.finish_owned(generation, settled, now);
                true
            }
            Job::ComposeStart { project, .. } => {
                let result = result.and_then(reply::compose_started).and_then(|started| {
                    if valid_job(&started.job) {
                        Ok(started)
                    } else {
                        Err("Missing Compose job identifier.".into())
                    }
                });
                let accepted = finish(&mut self.compose_start, generation, result, now);
                if !self.retired {
                    if accepted
                        && self.compose_start.resource.error.is_none()
                        && let Some(job) = self
                            .compose_start
                            .data
                            .as_ref()
                            .map(|started| started.job.clone())
                    {
                        self.observe_compose_running(&job, &project);
                        self.started_jobs.insert(
                            project.clone(),
                            ComposeReceipt {
                                project: project.clone(),
                                job: job.to_owned(),
                                generation,
                            },
                        );
                    }
                    self.jobs.invalidate();
                    self.wanted.insert(Kind::Jobs);
                    if self.compose_start.resource.error.is_none()
                        && self.selected_project.as_deref() == Some(&project)
                        && let Some(job) = self
                            .compose_start
                            .data
                            .as_ref()
                            .map(|started| started.job.clone())
                    {
                        self.select_job_internal(job);
                        if !self.activity_active {
                            self.wanted.remove(&Kind::Output);
                        }
                    }
                    // Failed starts can have changed the engine before losing
                    // their response. Successful jobs refresh again on finish.
                    self.invalidate_readings();
                }
                accepted
            }
            Job::ComposeCancel { job, project } => {
                let accepted = finish(
                    &mut self.compose_cancel,
                    generation,
                    result.and_then(reply::compose_done).and_then(|done| {
                        if done.ok {
                            Ok(done)
                        } else {
                            Err(INVALID_OUTCOME.into())
                        }
                    }),
                    now,
                );
                if !self.retired {
                    if accepted
                        && self.compose_cancel.resource.error.is_none()
                        && !self.finished_jobs.iter().any(|finished| finished == &job)
                    {
                        self.cancelling_jobs.insert(
                            job.clone(),
                            ComposeReceipt {
                                project,
                                job: job.clone(),
                                generation,
                            },
                        );
                    }
                    self.jobs.invalidate();
                    self.wanted.insert(Kind::Jobs);
                    if self.activity_active && self.selected_job.as_ref() == Some(&job) {
                        self.wanted.insert(Kind::Output);
                    }
                    self.invalidate_readings();
                }
                accepted
            }
        };
        self.changed |= accepted;
    }
}

fn slot<'a, K: PartialEq, T>(cache: &'a VecDeque<(K, T)>, key: &K) -> Option<&'a T> {
    cache
        .iter()
        .find(|(candidate, _)| candidate == key)
        .map(|(_, slot)| slot)
}
fn slot_mut<'a, K: PartialEq, T>(cache: &'a mut VecDeque<(K, T)>, key: &K) -> Option<&'a mut T> {
    cache
        .iter_mut()
        .find(|(candidate, _)| candidate == key)
        .map(|(_, slot)| slot)
}
fn touch<K: PartialEq + Clone, T>(cache: &mut VecDeque<(K, Slot<T>)>, key: &K, max: usize) {
    let slot = cache
        .iter()
        .position(|(candidate, _)| candidate == key)
        .and_then(|i| cache.remove(i))
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
fn finish<T: PartialEq>(
    slot: &mut Slot<T>,
    generation: u64,
    result: Result<T, String>,
    now: Instant,
) -> bool {
    if slot.generation != generation {
        return false;
    }
    let result = result.map(|value| {
        let changed = slot.data.as_ref() != Some(&value);
        slot.data = Some(value);
        slot.dirty = false;
        changed
    });
    slot.resource.finish_owned(generation, result, now);
    true
}
// Only passive detail reads use this. Snapshots advance statistics, log reads
// advance sequence positions, and job/action receipts must always be consumed.
fn finish_changed<T: PartialEq>(
    slot: &mut Slot<T>,
    generation: u64,
    result: Result<T, String>,
    now: Instant,
) -> bool {
    let presentation = slot.resource.presentation_revision;
    finish(slot, generation, result, now) && presentation != slot.resource.presentation_revision
}
fn valid_ids(ids: &[String]) -> bool {
    !ids.is_empty()
        && ids.len() <= 500
        && ids.iter().all(|id| valid_id(id))
        && ids.iter().collect::<HashSet<_>>().len() == ids.len()
}
fn valid_job(job: &str) -> bool {
    !job.is_empty()
        && job.len() <= 64
        && job.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
}
const INVALID_OUTCOME: &str = "Invalid Containers action outcome.";
/// Accepts only the receipt kind the action asks for.
fn validate_action_result(action: &Action, reply: ipc::Reply) -> Result<ipc::Reply, String> {
    match (action, &reply) {
        (Action::Container { .. }, ipc::Reply::Action(done))
        | (Action::Project { .. }, ipc::Reply::ProjectAction(done))
            if done.ok =>
        {
            Ok(reply)
        }
        (Action::Remove { .. }, ipc::Reply::Remove(_))
        | (Action::Prune { .. }, ipc::Reply::Prune(_)) => Ok(reply),
        _ => Err(INVALID_OUTCOME.into()),
    }
}
fn validate_job(job: &compose::Job) -> Result<(), String> {
    if valid_job(&job.job) {
        Ok(())
    } else {
        Err("Invalid Compose job response.".into())
    }
}
fn validate_jobs(jobs: compose::Jobs) -> Result<compose::Jobs, String> {
    jobs.jobs.iter().try_for_each(validate_job)?;
    Ok(jobs)
}
fn append_output(
    job: &str,
    previous: Option<&compose::Job>,
    mut next: compose::Job,
) -> Result<compose::Job, String> {
    validate_job(&next)?;
    if next.job != job {
        return Err("Compose output does not match the selected job.".into());
    }
    let suffix = next.output.as_str();
    let end = next.offset;
    // Agent offsets count cumulative UTF-8 bytes. Its retained suffix may begin
    // after our last read if the agent evicted output while Activity was hidden.
    let start = end
        .checked_sub(suffix.len() as u64)
        .ok_or("Compose output exceeds its cumulative byte offset.")?;
    let mut prefix = "";
    if let Some(previous) = previous {
        validate_job(previous)?;
        let previous_end = previous.offset;
        let previous_output = previous.output.as_str();
        let previous_start = previous_end
            .checked_sub(previous_output.len() as u64)
            .ok_or("Compose output exceeds its cumulative byte offset.")?;
        if previous.job != job || end < previous_end {
            return Err("Compose output does not match the selected job.".into());
        }
        if start <= previous_end {
            let overlap_start = start.max(previous_start);
            let old_overlap = previous_output.get((overlap_start - previous_start) as usize..);
            let new_overlap =
                suffix.get((overlap_start - start) as usize..(previous_end - start) as usize);
            if old_overlap.is_none() || old_overlap != new_overlap {
                return Err("Compose output overlaps inconsistently with earlier output.".into());
            }
            if previous_start < start {
                prefix = previous_output
                    .get(..(start - previous_start) as usize)
                    .ok_or("Compose output does not start at a UTF-8 boundary.")?;
            }
        }
        // A gap replaces the old text. Joining across missing bytes would give
        // earlier rows false absolute offsets, even if the text looks unchanged.
    }
    next.output = bounded_output(prefix, suffix);
    Ok(next)
}

/// Allocate only the retained contiguous suffix, never an oversized concatenation.
fn bounded_output(prefix: &str, suffix: &str) -> String {
    if suffix.len() >= MAX_OUTPUT_BYTES {
        let mut start = suffix.len() - MAX_OUTPUT_BYTES;
        while !suffix.is_char_boundary(start) {
            start += 1;
        }
        return suffix[start..].to_owned();
    }
    let mut start = prefix.len().saturating_sub(MAX_OUTPUT_BYTES - suffix.len());
    while !prefix.is_char_boundary(start) {
        start += 1;
    }
    let mut output = String::with_capacity(prefix.len() - start + suffix.len());
    output.push_str(&prefix[start..]);
    output.push_str(suffix);
    output
}
fn request_timeout(operation: &ipc::Operation) -> Duration {
    Duration::from_secs(match operation {
        ipc::Operation::Action(_)
        | ipc::Operation::ProjectAction(_)
        | ipc::Operation::Remove(_) => 90,
        ipc::Operation::Prune(_) => 135,
        ipc::Operation::ComposeProject(_) => 40,
        ipc::Operation::ComposeRun(_)
        | ipc::Operation::ComposeCancel(_)
        | ipc::Operation::ComposeJobs(_)
        | ipc::Operation::ComposeJob(_) => 20,
        _ => 30,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use compose::State;
    use serde_json::{Value, json};
    use std::{
        cell::{Cell, RefCell},
        rc::Rc,
    };
    use wire::{ComposeLabels, ContainerLogEntry, DiskTotals, DockerTotals, DockerVolume};

    #[test]
    fn an_action_accepts_only_its_own_successful_receipt() {
        let done = |ok| Box::new(wire::Done { ok });
        let start = Action::Container {
            container: "c1".into(),
            name: "web".into(),
            verb: ContainerVerb::Start,
        };
        assert!(validate_action_result(&start, ipc::Reply::Action(done(true))).is_ok());
        for reply in [
            ipc::Reply::Action(done(false)),
            ipc::Reply::ProjectAction(done(true)),
        ] {
            assert_eq!(
                validate_action_result(&start, reply).unwrap_err(),
                INVALID_OUTCOME
            );
        }
    }

    // Test observations are one-way projections of the typed producer output.
    fn observe(operation: &ipc::Operation) -> (&'static str, Value) {
        let refresh = |mut value: Value, enabled| {
            if enabled {
                value["refresh"] = Value::Bool(true);
            }
            value
        };
        match operation {
            ipc::Operation::Snapshot(v) => (
                "containers_snapshot",
                refresh(json!({"id":v.server}), v.refresh),
            ),
            ipc::Operation::Inspect(v) => (
                "container_inspect",
                refresh(json!({"id":v.server,"container":v.container}), v.refresh),
            ),
            ipc::Operation::Logs(v) => {
                let (container, project) = match &v.request.target {
                    wire::LogTarget::Container(v) => (Some(v), None),
                    wire::LogTarget::Project(v) => (None, Some(v)),
                };
                (
                    "container_logs",
                    json!({"id":v.server,"container":container,"project":project,"cursor":v.request.cursor,"lines":v.request.lines}),
                )
            }
            ipc::Operation::Action(v) => (
                "container_action",
                json!({"id":v.server,"container":v.container,"verb":v.verb.as_str()}),
            ),
            ipc::Operation::ProjectAction(v) => (
                "project_action",
                json!({"id":v.server,"project":v.project,"verb":v.verb.as_str(),"expectedIds":v.expected_ids}),
            ),
            ipc::Operation::Remove(v) => (
                "containers_remove",
                json!({"id":v.server,"containers":v.containers}),
            ),
            ipc::Operation::Prune(v) => (
                "docker_prune",
                json!({"id":v.server,"what":v.what.as_str()}),
            ),
            ipc::Operation::Disk(v) => (
                "docker_disk",
                refresh(
                    json!({"id":v.server,"part":match v.part { ipc::DiskPart::Full => "full", ipc::DiskPart::Quick => "quick", ipc::DiskPart::Volumes => "volumes" }}),
                    v.refresh,
                ),
            ),
            ipc::Operation::ComposeProject(v) => (
                "compose_project",
                refresh(json!({"id":v.server,"project":v.project}), v.refresh),
            ),
            ipc::Operation::ComposeRun(v) => (
                "compose_run",
                json!({"id":v.server,"project":v.project,"action":v.action}),
            ),
            ipc::Operation::ComposeJob(v) => (
                "compose_job",
                json!({"id":v.server,"job":v.job,"offset":v.offset}),
            ),
            ipc::Operation::ComposeCancel(v) => {
                ("compose_cancel", json!({"id":v.server,"job":v.job}))
            }
            ipc::Operation::ComposeJobs(v) => ("compose_jobs", json!({"id":v.server})),
        }
    }

    struct Request {
        method: &'static str,
        args: Value,
        sender: Option<mpsc::SyncSender<ReplyOutcome>>,
    }
    struct Harness {
        controller: Containers,
        requests: Rc<RefCell<Vec<Request>>>,
        now: Rc<Cell<Instant>>,
        fail_next: Rc<RefCell<Option<String>>>,
    }
    impl Harness {
        fn new() -> Self {
            let requests = Rc::new(RefCell::new(Vec::new()));
            let outgoing = requests.clone();
            let now = Rc::new(Cell::new(Instant::now()));
            let clock = now.clone();
            let fail_next = Rc::new(RefCell::new(None));
            let failure = fail_next.clone();
            let controller = Containers::with_dispatch(
                "00000000-0000-0000-0000-00000000000a".into(),
                Box::new(move |operation| {
                    let (method, args) = observe(&operation);
                    if let Some(error) = failure.borrow_mut().take() {
                        return Err(error);
                    }
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
                controller,
                requests,
                now,
                fail_next,
            }
        }
        fn count(&self, method: &str) -> usize {
            self.requests
                .borrow()
                .iter()
                .filter(|request| request.method == method)
                .count()
        }
        fn total(&self) -> usize {
            self.requests.borrow().len()
        }
        fn last_args(&self, method: &str) -> Value {
            self.requests
                .borrow()
                .iter()
                .rev()
                .find(|request| request.method == method)
                .unwrap()
                .args
                .clone()
        }
        /// Answers the pending Compose configuration read.
        fn reply_project(&mut self, result: Result<ComposeProject, String>) {
            self.requests
                .borrow_mut()
                .iter_mut()
                .find(|request| request.method == "compose_project" && request.sender.is_some())
                .unwrap()
                .sender
                .take()
                .unwrap()
                .send(result.map(|project| ipc::Reply::ComposeProject(Box::new(project))))
                .unwrap();
        }
        fn reply(&mut self, method: &str, result: ReplyOutcome) {
            self.requests
                .borrow_mut()
                .iter_mut()
                .find(|request| request.method == method && request.sender.is_some())
                .unwrap()
                .sender
                .take()
                .unwrap()
                .send(result)
                .unwrap();
        }
        /// Answers the pending output read for one job.
        fn reply_output(&mut self, result: Result<compose::Job, String>) {
            self.reply(
                "compose_job",
                result.map(|job| ipc::Reply::ComposeJob(Box::new(job))),
            );
        }
        fn disconnect(&mut self, method: &str) {
            self.requests
                .borrow_mut()
                .iter_mut()
                .find(|request| request.method == method && request.sender.is_some())
                .unwrap()
                .sender
                .take();
        }
        fn advance(&self, seconds: u64) {
            self.now.set(self.now.get() + Duration::from_secs(seconds));
        }
        fn snapshot(&mut self, containers: Vec<Container>) {
            self.controller.refresh();
            self.reply("containers_snapshot", Ok(snapshot(containers)));
            self.controller.poll(true);
        }
        fn details(&mut self, id: &str, name: &str, project: &str) {
            self.controller.select_container(name.into());
            self.reply("container_inspect", Ok(inspect(id, name)));
            self.reply_project(Ok(marked(project)));
            self.controller.poll(true);
        }
        fn compose_details(&mut self, project: &str) {
            self.controller.select_project(project.to_owned());
            self.reply_project(Ok(compose_detail(project)));
            self.controller.poll(true);
        }
    }
    fn id(digit: char) -> String {
        digit.to_string().repeat(64)
    }
    fn container(digit: char, name: &str, project: &str) -> Container {
        Container {
            id: id(digit),
            name: name.into(),
            state: ContainerState::Running,
            compose: Some(ComposeLabels {
                project: project.into(),
                service: name.into(),
                ..Default::default()
            }),
            ..Default::default()
        }
    }
    fn snapshot(containers: Vec<Container>) -> ipc::Reply {
        ipc::Reply::Snapshot(Box::new(ContainersSnapshot {
            containers,
            ..Default::default()
        }))
    }
    fn disk_reading(size: u64) -> DockerDisk {
        DockerDisk {
            totals: DockerTotals {
                images: DiskTotals {
                    count: 1,
                    size,
                    reclaimable: size / 2,
                },
                ..Default::default()
            },
            volumes_measured: true,
            ..Default::default()
        }
    }
    fn disk(size: u64) -> ipc::Reply {
        ipc::Reply::Disk(Box::new(disk_reading(size)))
    }
    fn inspect(id: &str, name: &str) -> ipc::Reply {
        ipc::Reply::Inspect(Box::new(ContainerInspect {
            id: id.into(),
            name: name.into(),
            ..Default::default()
        }))
    }
    fn entry(at: i64, message: &str) -> ContainerLogEntry {
        ContainerLogEntry {
            at: Some(at),
            message: message.into(),
            ..Default::default()
        }
    }
    fn page(entries: Vec<ContainerLogEntry>, cursor: Option<&str>) -> ipc::Reply {
        ipc::Reply::Logs(Box::new(ContainerLogPage {
            entries,
            cursor: cursor.map(str::to_owned),
        }))
    }
    fn logs(message: &str, cursor: Option<&str>) -> ipc::Reply {
        page(vec![entry(1, message)], cursor)
    }
    fn job(name: &str, state: State, offset: u64, output: &str) -> compose::Job {
        compose::Job {
            job: name.into(),
            project: "app".into(),
            command: "up".into(),
            state,
            exit_code: (state == State::Succeeded).then_some(0),
            started_at: 1000,
            finished_at: (!state.is_running()).then_some(2000),
            output: output.into(),
            offset,
        }
    }
    fn listing(jobs: Vec<compose::Job>) -> ipc::Reply {
        ipc::Reply::ComposeJobs(Box::new(compose::Jobs { jobs }))
    }
    fn started(job: &str) -> ipc::Reply {
        ipc::Reply::ComposeRun(Box::new(compose::Started { job: job.into() }))
    }
    fn cancelled() -> ipc::Reply {
        ipc::Reply::ComposeCancel(Box::new(compose::Done { ok: true }))
    }
    fn container_done() -> ipc::Reply {
        ipc::Reply::Action(Box::new(wire::Done { ok: true }))
    }
    fn project_done() -> ipc::Reply {
        ipc::Reply::ProjectAction(Box::new(wire::Done { ok: true }))
    }
    fn removed(removed: u64) -> ipc::Reply {
        ipc::Reply::Remove(Box::new(wire::Removed { removed }))
    }
    fn pruned(removed: u64, reclaimed: u64) -> ipc::Reply {
        ipc::Reply::Prune(Box::new(Pruned { removed, reclaimed }))
    }
    fn stop(digit: char, name: &str) -> Action {
        Action::Container {
            container: id(digit),
            name: name.into(),
            verb: ContainerVerb::Stop,
        }
    }

    #[test]
    fn cached_detail_refreshes_are_quiet_but_errors_and_manual_reads_publish() {
        let mut h = Harness::new();
        h.snapshot(vec![container('a', "web", "app")]);
        h.details(&id('a'), "web", "app");
        h.controller.poll(false);
        assert!(!h.controller.poll(false));
        let revision = h.controller.resource("container-inspect").unwrap().revision;
        let read = |h: &mut Harness, result: ReplyOutcome, manual: bool, changed: bool| {
            assert!(h.controller.launch(Job::Inspect(id('a')), manual));
            assert_eq!(h.controller.poll(false), manual);
            assert_eq!(
                h.controller
                    .resource("container-inspect")
                    .unwrap()
                    .display_pending(),
                manual
            );
            h.reply("container_inspect", result);
            assert_eq!(h.controller.poll(false), changed);
        };
        read(&mut h, Ok(inspect(&id('a'), "web")), false, false);
        assert!(h.controller.resource("container-inspect").unwrap().revision > revision);
        read(&mut h, Err("Unavailable".into()), false, true);
        read(&mut h, Err("Unavailable".into()), false, false);
        read(&mut h, Ok(inspect(&id('a'), "web")), false, true);
        read(&mut h, Ok(inspect(&id('a'), "web")), true, true);
        read(&mut h, Ok(inspect(&id('a'), "renamed")), false, true);

        assert!(h.controller.launch(Job::Compose("app".into()), false));
        assert!(!h.controller.poll(false));
        h.reply_project(Ok(marked("app")));
        assert!(!h.controller.poll(false));

        assert!(h.controller.launch(Job::Disk, false));
        assert!(h.controller.poll(false), "initial read shows loading");
        h.reply("docker_disk", Ok(disk(100)));
        assert!(h.controller.poll(false));
        assert!(h.controller.launch(Job::Disk, false));
        assert!(!h.controller.poll(false));
        h.reply("docker_disk", Ok(disk(100)));
        assert!(!h.controller.poll(false));
    }

    #[test]
    fn identical_container_snapshots_still_advance_statistics_without_admission_redraw() {
        let mut h = Harness::new();
        let mut c = container('a', "web", "app");
        c.cpu = Some(5.);
        h.snapshot(vec![c.clone()]);
        h.controller.poll(false);
        let first = h.controller.stats.get(&id('a')).unwrap().cpu()[0];
        h.advance(5);
        assert!(h.controller.launch(Job::Snapshot, false));
        assert!(!h.controller.poll(false));
        h.reply("containers_snapshot", Ok(snapshot(vec![c])));
        assert!(h.controller.poll(false), "new statistic sample is visible");
        let history = h.controller.stats.get(&id('a')).unwrap();
        assert_eq!(history.cpu().len(), 2);
        assert!(history.cpu()[1].sequence > first.sequence);
        assert!(history.cpu()[1].received_at > first.received_at);
    }

    #[test]
    fn container_progress_is_pending_and_exact_including_batch_removal() {
        let mut h = Harness::new();
        h.snapshot(vec![
            container('a', "web", "app"),
            container('b', "worker", "app"),
        ]);
        assert!(h.controller.action(stop('a', "web")));
        assert!(h.controller.pending_container_action(&id('a')).is_some());
        assert!(h.controller.pending_container_action(&id('b')).is_none());
        h.reply("container_action", Err("fixture rejected stop".into()));
        h.controller.poll(true);
        assert!(h.controller.pending_container_action(&id('a')).is_none());

        let mut h = Harness::new();
        let stopped = |digit, name| Container {
            state: ContainerState::Exited,
            ..container(digit, name, "app")
        };
        h.snapshot(vec![
            stopped('a', "web"),
            stopped('b', "worker"),
            stopped('c', "other"),
        ]);
        assert!(h.controller.action(Action::Remove {
            containers: vec![id('a'), id('b')],
            names: vec!["web".into(), "worker".into()],
        }));
        assert!(h.controller.pending_container_action(&id('a')).is_some());
        assert!(h.controller.pending_container_action(&id('b')).is_some());
        assert!(h.controller.pending_container_action(&id('c')).is_none());
        h.reply("containers_remove", Ok(removed(2)));
        h.controller.poll(true);
        assert!(h.controller.pending_container_action(&id('b')).is_none());
    }

    #[test]
    fn read_kinds_are_independent_and_duplicate_manual_reads_coalesce() {
        let mut h = Harness::new();
        h.controller.poll(true);
        for _ in 0..10 {
            h.controller.refresh();
        }
        assert_eq!(h.count("containers_snapshot"), 1);
        assert!(h.controller.resource("snapshot").unwrap().manual_pending);
        h.reply(
            "containers_snapshot",
            Ok(snapshot(vec![container('a', "web", "app")])),
        );
        h.controller.poll(true);
        h.controller.select_container("web".into());
        h.controller.set_logs_active(true, true);
        for _ in 0..10 {
            h.controller.retry_inspect();
            h.controller.retry_logs();
            h.controller.retry_compose();
            h.controller.poll(true);
        }
        for method in ["container_inspect", "container_logs", "compose_project"] {
            assert_eq!(h.count(method), 1, "{method}");
        }
        for key in ["container-inspect", "container-logs", "compose-detail"] {
            assert!(h.controller.resource(key).unwrap().manual_pending);
        }
        assert_eq!(h.last_args("container_inspect")["container"], id('a'));
        assert_eq!(h.last_args("container_logs")["container"], id('a'));
        assert_eq!(h.last_args("container_logs")["project"], Value::Null);
        h.reply("container_logs", Ok(logs("one", Some("c1.1.1"))));
        h.controller.poll(true);
        assert_eq!(h.controller.logs().unwrap().entries[0].message, "one");
        assert!(h.controller.inspect().is_none());
        assert!(h.controller.compose_project().is_none());
    }

    #[test]
    fn selecting_another_target_never_leaks_late_detail_or_log_responses() {
        let mut h = Harness::new();
        h.snapshot(vec![
            container('a', "web", "app"),
            container('b', "database", "data"),
        ]);
        h.controller.select_container("web".into());
        h.controller.set_logs_active(true, false);
        h.controller.select_container("database".into());
        assert!(h.controller.inspect().is_none());
        assert!(h.controller.logs().is_none());
        assert!(h.controller.compose_project().is_none());
        for method in ["container_inspect", "container_logs", "compose_project"] {
            assert_eq!(h.count(method), 1);
        }
        h.reply("container_inspect", Ok(inspect(&id('a'), "web")));
        h.reply(
            "container_logs",
            Ok(logs("private web data", Some("c1.1.1"))),
        );
        h.reply_project(Ok(marked("app")));
        h.controller.poll(true);
        assert!(h.controller.inspect().is_none());
        assert!(h.controller.logs().is_none());
        assert!(h.controller.compose_project().is_none());
        assert_eq!(h.last_args("container_inspect")["container"], id('b'));
        assert_eq!(h.last_args("container_logs")["cursor"], Value::Null);
        assert_eq!(h.last_args("compose_project")["project"], "data");
        h.reply("container_inspect", Ok(inspect(&id('b'), "database")));
        h.reply("container_logs", Ok(logs("database log", None)));
        h.reply_project(Ok(marked("data")));
        h.controller.poll(true);
        assert_eq!(h.controller.inspect().unwrap().name, "database");
        assert_eq!(
            h.controller.logs().unwrap().entries[0].message,
            "database log"
        );
        assert_eq!(
            h.controller
                .compose_project()
                .unwrap()
                .working_dir
                .as_deref(),
            Some("data")
        );
        h.controller.select_project("app".into());
        assert!(h.controller.inspect().is_none());
        assert!(h.controller.logs().is_none());
        assert_eq!(h.last_args("container_logs")["container"], Value::Null);
        assert_eq!(h.last_args("container_logs")["project"], "app");
    }

    #[test]
    fn recreated_container_keeps_name_selection_but_resets_id_reads_and_cursor() {
        let mut h = Harness::new();
        h.snapshot(vec![container('a', "web", "app")]);
        h.details(&id('a'), "web", "app");
        h.controller.set_logs_active(true, true);
        h.reply("container_logs", Ok(logs("old", Some("c1.1.1"))));
        h.controller.poll(true);
        h.controller.retry_logs();
        h.controller.retry_inspect();
        assert_eq!(h.last_args("container_logs")["cursor"], "c1.1.1");
        h.snapshot(vec![container('b', "web", "app")]);
        assert_eq!(h.controller.selected_name(), Some("web"));
        assert_eq!(h.controller.selected_container().unwrap().id, id('b'));
        assert!(h.controller.inspect().is_none());
        assert!(h.controller.logs().is_none());
        // Old workers continue occupying their kind until physically drained.
        assert_eq!(h.count("container_inspect"), 2);
        assert_eq!(h.count("container_logs"), 2);
        h.reply("container_inspect", Ok(inspect(&id('a'), "web")));
        h.reply("container_logs", Ok(logs("late old", Some("c1.2.1"))));
        h.controller.poll(true);
        assert!(h.controller.inspect().is_none());
        assert!(h.controller.logs().is_none());
        assert_eq!(h.last_args("container_inspect")["container"], id('b'));
        assert_eq!(h.last_args("container_logs")["container"], id('b'));
        assert_eq!(h.last_args("container_logs")["cursor"], Value::Null);
        assert_eq!(h.last_args("container_logs")["lines"], 300);
        h.reply("container_logs", Ok(logs("new", Some("c1.3.1"))));
        h.controller.poll(true);
        assert_eq!(h.controller.logs().unwrap().entries.len(), 1);
        assert_eq!(h.controller.logs().unwrap().entries[0].message, "new");
    }

    #[test]
    fn cached_details_obey_stale_times_and_caches_stay_bounded() {
        let mut h = Harness::new();
        h.snapshot(vec![container('a', "web", "app")]);
        h.details(&id('a'), "web", "app");
        h.controller.clear_selection();
        h.controller.select_container("web".into());
        assert_eq!(h.count("container_inspect"), 1);
        assert_eq!(h.count("compose_project"), 1);
        h.advance(14);
        h.controller.select_container("web".into());
        assert_eq!(h.count("container_inspect"), 1);
        h.advance(1);
        h.controller.select_container("web".into());
        assert_eq!(h.count("container_inspect"), 2);
        assert_eq!(h.count("compose_project"), 1);
        h.advance(15);
        h.controller.select_container("web".into());
        assert_eq!(h.count("compose_project"), 2);
        let rows = (0..50)
            .map(|n| Container {
                id: format!("{n:064x}"),
                name: format!("web-{n}"),
                compose: Some(ComposeLabels {
                    project: format!("app-{n}"),
                    ..Default::default()
                }),
                ..Default::default()
            })
            .collect();
        h.snapshot(rows);
        h.controller.set_logs_active(true, false);
        for n in 0..50 {
            h.controller.select_container(format!("web-{n}"));
            h.controller.select_job(format!("job-{n}"));
        }
        assert_eq!(h.controller.inspects.len(), MAX_INSPECTS);
        assert_eq!(h.controller.projects.len(), MAX_PROJECTS);
        assert_eq!(h.controller.logs.len(), MAX_LOGS);
        assert_eq!(h.controller.outputs.len(), MAX_OUTPUTS);
        assert!(h.controller.pending.len() <= 10);
    }

    #[test]
    fn logs_follow_every_two_seconds_pause_hidden_and_keep_latest_two_thousand() {
        let mut h = Harness::new();
        h.snapshot(vec![container('a', "web", "app")]);
        h.controller.select_container("web".into());
        h.controller.set_logs_active(true, true);
        let entries = (0..2_100).map(|n| entry(n, &n.to_string())).collect();
        h.reply("container_logs", Ok(page(entries, Some("c1.1.1"))));
        h.controller.poll(true);
        assert_eq!(h.controller.logs().unwrap().entries.len(), 2_000);
        assert_eq!(h.controller.logs().unwrap().entries[0].message, "100");
        h.advance(1);
        h.controller.poll(true);
        assert_eq!(h.count("container_logs"), 1);
        h.advance(1);
        h.controller.poll(true);
        assert_eq!(h.count("container_logs"), 2);
        assert_eq!(h.last_args("container_logs")["lines"], Value::Null);
        h.reply("container_logs", Ok(logs("new", None)));
        h.controller.poll(false);
        assert_eq!(
            h.controller.logs().unwrap().cursor.as_deref(),
            Some("c1.1.1")
        );
        assert_eq!(h.controller.logs().unwrap().entries[0].message, "101");
        let count = h.total();
        h.advance(60);
        h.controller.poll(false);
        assert_eq!(h.total(), count);
        h.controller.poll(true);
        assert_eq!(h.count("container_logs"), 3);
        h.controller.set_logs_active(true, false);
        h.reply("container_logs", Ok(logs("last", None)));
        h.controller.poll(true);
        h.advance(60);
        h.controller.poll(true);
        assert_eq!(h.count("container_logs"), 3);
    }

    fn log_positions(controller: &Containers) -> (u64, u64, u64) {
        let window = controller.log_window().unwrap();
        assert_eq!(window.end - window.first, window.entries.len() as u64);
        (window.epoch, window.first, window.end)
    }

    #[test]
    fn log_window_counts_identical_entries_through_prefix_eviction() {
        let mut h = Harness::new();
        h.snapshot(vec![container('a', "web", "app")]);
        assert!(h.controller.log_window().is_none());
        h.controller.select_container("web".into());
        let epoch = log_positions(&h.controller).0;
        assert_eq!(log_positions(&h.controller), (epoch, 0, 0));
        assert!(h.controller.logs().is_none());
        h.controller.set_logs_active(true, false);
        let identical = entry(1, "identical line");
        h.reply(
            "container_logs",
            Ok(page(vec![identical.clone(); 2005], Some("c1.1.2005"))),
        );
        h.controller.poll(true);
        assert_eq!(log_positions(&h.controller), (epoch, 5, 2005));
        h.controller.retry_logs();
        h.reply(
            "container_logs",
            Ok(page(vec![identical; 3], Some("c1.1.2008"))),
        );
        h.controller.poll(true);
        assert_eq!(log_positions(&h.controller), (epoch, 8, 2008));
        assert!(
            h.controller
                .log_window()
                .unwrap()
                .entries
                .iter()
                .all(|line| line.message == "identical line")
        );
        assert!(h.controller.resource("container-logs").unwrap().has_value());
    }

    #[test]
    fn log_window_empty_polls_and_failures_preserve_positions_but_success_revises_resource() {
        let mut h = Harness::new();
        h.snapshot(vec![container('a', "web", "app")]);
        h.controller.select_container("web".into());
        h.controller.set_logs_active(true, false);
        h.reply("container_logs", Ok(logs("first", Some("c1.1.1"))));
        h.controller.poll(true);
        let positions = log_positions(&h.controller);
        let revision = h.controller.resource("container-logs").unwrap().revision;
        h.controller.retry_logs();
        assert_eq!(log_positions(&h.controller), positions);
        h.reply("container_logs", Ok(page(vec![], None)));
        h.controller.poll(true);
        assert_eq!(log_positions(&h.controller), positions);
        let updated = h.controller.resource("container-logs").unwrap().revision;
        assert!(updated > revision);
        assert_eq!(
            h.controller.logs().unwrap().cursor.as_deref(),
            Some("c1.1.1")
        );
        for failure in [Err("fixture read failed".into()), Ok(disk(1))] {
            h.controller.retry_logs();
            h.reply("container_logs", failure);
            h.controller.poll(true);
            assert_eq!(log_positions(&h.controller), positions);
            assert_eq!(
                h.controller.log_window().unwrap().entries[0].message,
                "first"
            );
            assert_eq!(
                h.controller.resource("container-logs").unwrap().revision,
                updated
            );
            assert!(
                h.controller
                    .resource("container-logs")
                    .unwrap()
                    .error
                    .is_some()
            );
        }
    }

    #[test]
    fn log_window_reset_changes_epoch_and_late_response_cannot_advance_it() {
        let mut h = Harness::new();
        h.snapshot(vec![container('a', "web", "app")]);
        h.controller.select_container("web".into());
        h.controller.set_logs_active(true, false);
        h.reply("container_logs", Ok(logs("same", Some("c1.1.1"))));
        h.controller.poll(true);
        let original = log_positions(&h.controller).0;
        h.controller.retry_logs();
        h.controller.reset_logs();
        let reset = log_positions(&h.controller).0;
        assert_ne!(reset, original);
        assert_eq!(log_positions(&h.controller), (reset, 0, 0));
        assert!(h.controller.logs().is_none());
        h.reply(
            "container_logs",
            Ok(logs("late old response", Some("c1.2.1"))),
        );
        h.controller.poll(true);
        assert_eq!(log_positions(&h.controller), (reset, 0, 0));
        assert!(h.controller.logs().is_none());
        assert_eq!(h.last_args("container_logs")["cursor"], Value::Null);
        assert_eq!(h.last_args("container_logs")["lines"], 300);
        h.reply("container_logs", Ok(logs("same", Some("c1.1.1"))));
        h.controller.poll(true);
        assert_eq!(log_positions(&h.controller), (reset, 0, 1));
        assert_eq!(
            h.controller.log_window().unwrap().entries[0].message,
            "same"
        );
    }

    #[test]
    fn log_window_navigation_reuses_cache_epoch_and_eviction_or_recreation_allocates_new_epoch() {
        let mut h = Harness::new();
        h.snapshot(vec![
            container('a', "web", "app"),
            container('b', "db", "app"),
        ]);
        h.controller.select_container("web".into());
        h.controller.set_logs_active(true, false);
        h.reply("container_logs", Ok(logs("same", Some("c1.1.1"))));
        h.controller.poll(true);
        let original = log_positions(&h.controller);
        h.controller.select_project("app".into());
        let project_epoch = log_positions(&h.controller).0;
        assert_ne!(project_epoch, original.0);
        assert!(h.controller.log_window().unwrap().entries.is_empty());
        h.reply(
            "container_logs",
            Ok(logs("same", Some("p1.aaaaaaaaaaaa-1-1"))),
        );
        h.controller.poll(true);
        h.controller.select_container("web".into());
        assert_eq!(log_positions(&h.controller), original);
        h.controller.select_project("app".into());
        assert_eq!(log_positions(&h.controller), (project_epoch, 0, 1));
        h.controller.set_logs_active(false, false);
        for index in 0..MAX_LOGS {
            h.controller.select_project(format!("cache-{index}"));
        }
        h.controller.select_container("web".into());
        let reallocated = log_positions(&h.controller).0;
        assert_ne!(reallocated, original.0);
        assert_eq!(log_positions(&h.controller), (reallocated, 0, 0));
        h.snapshot(vec![container('c', "web", "app")]);
        let recreated = log_positions(&h.controller).0;
        assert_ne!(recreated, reallocated);
        assert_eq!(log_positions(&h.controller), (recreated, 0, 0));
        h.controller.clear_selection();
        assert!(h.controller.log_window().is_none());
    }

    #[test]
    fn late_other_target_log_completion_does_not_change_selected_window_identity() {
        let mut h = Harness::new();
        h.snapshot(vec![
            container('a', "web", "app"),
            container('b', "db", "app"),
        ]);
        h.controller.select_container("web".into());
        h.controller.set_logs_active(true, false);
        let web_epoch = log_positions(&h.controller).0;
        h.controller.select_container("db".into());
        let db_epoch = log_positions(&h.controller).0;
        assert_ne!(db_epoch, web_epoch);
        h.reply("container_logs", Ok(logs("web data", Some("c1.1.1"))));
        h.controller.poll(true);
        assert_eq!(log_positions(&h.controller), (db_epoch, 0, 0));
        h.reply("container_logs", Ok(logs("db data", Some("c1.1.1"))));
        h.controller.poll(true);
        assert_eq!(log_positions(&h.controller), (db_epoch, 0, 1));
        h.controller.select_container("web".into());
        assert_eq!(log_positions(&h.controller), (web_epoch, 0, 1));
        assert_eq!(
            h.controller.log_window().unwrap().entries[0].message,
            "web data"
        );
        h.controller.retire();
        assert!(h.controller.log_window().is_none());
    }

    #[test]
    fn failed_refresh_keeps_last_data_and_wrong_inspect_id_is_rejected() {
        let mut h = Harness::new();
        h.snapshot(vec![container('a', "web", "app")]);
        h.details(&id('a'), "web", "app");
        let updated = h
            .controller
            .resource("container-inspect")
            .unwrap()
            .updated_at;
        h.controller.retry_inspect();
        h.reply("container_inspect", Ok(inspect(&id('b'), "database")));
        h.controller.poll(true);
        assert_eq!(h.controller.inspect().unwrap().id, id('a'));
        assert_eq!(
            h.controller
                .resource("container-inspect")
                .unwrap()
                .updated_at,
            updated
        );
        assert!(
            h.controller
                .resource("container-inspect")
                .unwrap()
                .error
                .as_ref()
                .unwrap()
                .contains("does not match")
        );
        h.controller.retry_inspect();
        assert!(
            h.controller
                .resource("container-inspect")
                .unwrap()
                .error
                .is_some()
        );
        h.reply("container_inspect", Ok(inspect(&id('a'), "web")));
        h.controller.poll(true);
        assert!(
            h.controller
                .resource("container-inspect")
                .unwrap()
                .error
                .is_none()
        );
    }

    #[test]
    fn mutation_error_invalidates_all_readings_and_retains_the_original_target() {
        let mut h = Harness::new();
        h.snapshot(vec![
            container('a', "web", "app"),
            container('b', "db", "db"),
        ]);
        h.details(&id('a'), "web", "app");
        h.controller.set_disk_active(true);
        h.reply("docker_disk", Ok(disk(10)));
        h.controller.poll(true);
        h.controller.refresh();
        h.controller.retry_inspect();
        h.controller.retry_compose();
        h.controller.retry_disk();
        let action = stop('a', "web");
        assert!(h.controller.action(action.clone()));
        assert!(!h.controller.action(action.clone()));
        h.controller.select_container("db".into());
        h.reply(
            "container_action",
            Err("stop timed out after partial progress".into()),
        );
        h.reply(
            "containers_snapshot",
            Ok(snapshot(vec![container('c', "stale", "old")])),
        );
        h.reply("container_inspect", Ok(inspect(&id('a'), "old web")));
        h.reply_project(Ok(marked("stale")));
        h.reply("docker_disk", Ok(disk(999)));
        h.controller.poll(true);
        assert_eq!(h.controller.snapshot().unwrap().containers[0].name, "web");
        assert_eq!(h.controller.disk().unwrap().totals.images.size, 10);
        assert_eq!(h.last_args("container_inspect")["container"], id('b'));
        for method in [
            "containers_snapshot",
            "container_inspect",
            "compose_project",
            "docker_disk",
        ] {
            assert_eq!(h.count(method), 3, "{method}");
        }
        let status = h.controller.action_status(&action.target()).unwrap();
        assert!(!status.pending);
        assert!(status.error.is_some());
        let receipts = h.controller.take_action_completions();
        assert_eq!(receipts.len(), 1);
        assert_eq!(receipts[0].action, action);
        assert!(receipts[0].result.is_err());
        assert!(h.controller.take_action_completions().is_empty());
    }

    #[test]
    fn project_mutation_requires_current_immutable_members_and_excludes_oneoffs() {
        let mut h = Harness::new();
        let mut oneoff = container('b', "migration", "app");
        oneoff.compose.as_mut().unwrap().oneoff = true;
        h.snapshot(vec![container('a', "web", "app"), oneoff]);
        let expected = Action::Project {
            project: "app".into(),
            verb: ProjectVerb::Restart,
            expected_ids: vec![id('a')],
        };
        assert!(!h.controller.action(Action::Project {
            project: "app".into(),
            verb: ProjectVerb::Restart,
            expected_ids: vec![id('a'), id('b')]
        }));
        assert!(!h.controller.action(stop('b', "web")));
        assert!(h.controller.action(expected));
        assert_eq!(
            h.last_args("project_action")["expectedIds"],
            json!([id('a')])
        );
        h.reply("project_action", Ok(project_done()));
        h.controller.poll(true);
        assert!(h.controller.snapshot.dirty);
        assert!(!h.controller.action(stop('a', "web")));
    }

    #[test]
    fn hidden_mutations_settle_without_dispatch_and_retirement_drains_without_replay() {
        let mut h = Harness::new();
        h.snapshot(vec![container('a', "web", "app")]);
        assert!(h.controller.action(stop('a', "web")));
        let count = h.total();
        h.reply("container_action", Ok(container_done()));
        h.controller.poll(false);
        assert_eq!(h.total(), count);
        assert_eq!(h.controller.take_action_completions().len(), 1);
        h.controller.poll(true);
        assert_eq!(h.count("containers_snapshot"), 2);
        h.reply(
            "containers_snapshot",
            Ok(snapshot(vec![container('a', "web", "app")])),
        );
        h.controller.poll(true);
        assert!(h.controller.action(stop('a', "web")));
        h.controller.retire();
        let count = h.total();
        h.reply("container_action", Err("late failure".into()));
        h.reply("compose_jobs", Ok(listing(vec![])));
        h.controller.poll(true);
        h.controller.refresh();
        h.controller.retry_jobs();
        assert_eq!(h.total(), count);
        assert!(!h.controller.has_pending());
        assert!(h.controller.snapshot().is_none());
        assert_eq!(h.controller.take_action_completions().len(), 1);
        assert!(!h.controller.compose_job_completed("finished"));
    }

    #[test]
    fn hidden_recreation_and_visibility_transition_wait_before_loading_new_details() {
        let mut h = Harness::new();
        h.snapshot(vec![container('a', "web", "app")]);
        h.details(&id('a'), "web", "app");
        h.controller.set_logs_active(true, true);
        h.reply(
            "container_logs",
            Ok(logs("before recreation", Some("c1.1.1"))),
        );
        h.controller.poll(true);
        h.controller.refresh();
        let count = h.total();
        h.reply(
            "containers_snapshot",
            Ok(snapshot(vec![container('b', "web", "other")])),
        );
        assert!(h.controller.poll(false));
        assert_eq!(h.total(), count);
        assert_eq!(h.controller.selected_name(), Some("web"));
        assert!(h.controller.logs().is_none());
        assert!(h.controller.inspect().is_none());
        assert!(h.controller.compose_project().is_none());
        assert!(!h.controller.poll(false));
        assert!(h.controller.poll(true));
        assert_eq!(h.last_args("container_inspect")["container"], id('b'));
        assert_eq!(h.last_args("container_logs")["cursor"], Value::Null);
        assert_eq!(h.last_args("compose_project")["project"], "other");
    }

    #[test]
    fn snapshot_ticks_do_not_retry_lazy_details_but_revisiting_does() {
        let mut h = Harness::new();
        h.snapshot(vec![container('a', "web", "app")]);
        h.details(&id('a'), "web", "app");
        h.advance(30);
        h.controller.poll(true);
        h.reply(
            "containers_snapshot",
            Ok(snapshot(vec![container('a', "web", "app")])),
        );
        h.controller.poll(true);
        assert_eq!(h.count("container_inspect"), 1);
        assert_eq!(h.count("compose_project"), 1);
        h.controller.revalidate_details();
        assert_eq!(h.count("container_inspect"), 2);
        assert_eq!(h.count("compose_project"), 2);
    }

    #[test]
    fn resetting_logs_waits_for_old_worker_and_discards_its_cursor() {
        let mut h = Harness::new();
        h.snapshot(vec![container('a', "web", "app")]);
        h.controller.select_container("web".into());
        h.controller.set_logs_active(true, false);
        h.reply("container_logs", Ok(logs("old", Some("c1.1.1"))));
        h.controller.poll(true);
        h.controller.retry_logs();
        h.controller.reset_logs();
        assert!(h.controller.logs().is_none());
        assert_eq!(h.count("container_logs"), 2);
        h.reply("container_logs", Ok(logs("discarded", Some("c1.2.1"))));
        h.controller.poll(true);
        assert!(h.controller.logs().is_none());
        assert_eq!(h.count("container_logs"), 3);
        assert_eq!(h.last_args("container_logs")["cursor"], Value::Null);
        assert_eq!(h.last_args("container_logs")["lines"], 300);
    }

    #[test]
    fn disconnected_and_unspawnable_mutations_release_ownership_and_refresh() {
        let mut h = Harness::new();
        h.snapshot(vec![container('a', "web", "app")]);
        h.controller.action(stop('a', "web"));
        h.disconnect("container_action");
        h.controller.poll(true);
        assert_eq!(
            h.controller.take_action_completions()[0]
                .result
                .as_ref()
                .unwrap_err(),
            WORKER_ERROR
        );
        h.reply(
            "containers_snapshot",
            Ok(snapshot(vec![container('a', "web", "app")])),
        );
        h.controller.poll(true);
        *h.fail_next.borrow_mut() = Some("cannot spawn".into());
        assert!(!h.controller.action(stop('a', "web")));
        assert!(!h.controller.resource("action").unwrap().pending);
        h.controller.poll(true);
        assert_eq!(h.count("containers_snapshot"), 3);
        assert_eq!(
            h.controller.take_action_completions()[0]
                .result
                .as_ref()
                .unwrap_err(),
            "cannot spawn"
        );
    }

    #[test]
    fn unselected_compose_completion_invalidates_once_before_concurrent_reads_settle() {
        let mut h = Harness::new();
        h.snapshot(vec![container('a', "web", "app")]);
        h.details(&id('a'), "web", "app");
        h.controller.refresh();
        h.controller.retry_inspect();
        h.controller.retry_compose();
        h.reply(
            "compose_jobs",
            Ok(listing(vec![job("job-a", State::Succeeded, 3, "")])),
        );
        h.reply(
            "containers_snapshot",
            Ok(snapshot(vec![container('b', "old snapshot", "app")])),
        );
        h.reply("container_inspect", Ok(inspect(&id('a'), "stale")));
        h.reply_project(Ok(marked("stale")));
        h.controller.poll(true);
        assert_eq!(h.controller.snapshot().unwrap().containers[0].name, "web");
        assert_eq!(h.controller.inspect().unwrap().name, "web");
        assert_eq!(h.count("containers_snapshot"), 3);
        assert_eq!(h.count("container_inspect"), 3);
        assert_eq!(h.count("compose_project"), 3);
        assert!(!h.controller.compose_job_completed("job-a"));
        h.reply(
            "containers_snapshot",
            Ok(snapshot(vec![container('a', "web", "app")])),
        );
        h.reply("container_inspect", Ok(inspect(&id('a'), "web")));
        h.reply_project(Ok(marked("fresh")));
        h.controller.poll(true);
        h.controller.retry_jobs();
        h.reply(
            "compose_jobs",
            Ok(listing(vec![job("job-a", State::Succeeded, 3, "")])),
        );
        h.controller.poll(true);
        assert_eq!(h.count("containers_snapshot"), 3);
        assert!(!h.controller.snapshot.dirty);
    }

    #[test]
    fn compose_jobs_poll_at_running_and_idle_intervals_only_while_visible() {
        let mut h = Harness::new();
        h.controller.poll(true);
        h.reply(
            "compose_jobs",
            Ok(listing(vec![job("job-a", State::Running, 0, "")])),
        );
        h.controller.poll(true);
        h.advance(1);
        h.controller.poll(true);
        assert_eq!(h.count("compose_jobs"), 1);
        h.now.set(h.now.get() + Duration::from_millis(500));
        h.controller.poll(true);
        assert_eq!(h.count("compose_jobs"), 2);
        h.reply("compose_jobs", Ok(listing(vec![])));
        h.controller.poll(true);
        h.advance(14);
        h.controller.poll(true);
        assert_eq!(h.count("compose_jobs"), 2);
        h.advance(1);
        h.controller.poll(false);
        assert_eq!(h.count("compose_jobs"), 2);
        h.controller.poll(true);
        assert_eq!(h.count("compose_jobs"), 3);
    }

    fn compose_detail(_project: &str) -> ComposeProject {
        ComposeProject {
            cli: Some(compose::Cli {
                kind: compose::CliKind::Plugin,
                version: "2.29.1".into(),
            }),
            working_dir: Some("/srv/app".into()),
            config_files: vec!["/srv/app/compose.yaml".into()],
            services: Some(vec![]),
            ..Default::default()
        }
    }
    /// A Compose reading told apart by its working directory.
    fn marked(working_dir: &str) -> ComposeProject {
        ComposeProject {
            working_dir: Some(working_dir.into()),
            ..Default::default()
        }
    }
    fn project_job(project: &str, id: &str, state: State) -> compose::Job {
        let mut value = job(id, state, 0, "");
        value.project = project.into();
        value
    }

    #[test]
    fn compose_start_requires_successful_current_project_cli_and_engine_readings() {
        let mut h = Harness::new();
        assert!(!h.controller.can_compose_run("app"));
        h.snapshot(vec![container('a', "web", "app")]);
        assert!(!h.controller.can_compose_run("app"));
        h.controller.select_project("app".into());
        assert!(!h.controller.can_compose_run("app"));
        for detail in [
            ComposeProject::default(),
            ComposeProject {
                config_error: Some("invalid config".into()),
                ..Default::default()
            },
        ] {
            h.reply_project(Ok(detail));
            h.controller.poll(true);
            assert!(!h.controller.compose_run(
                "app".into(),
                ComposeAction::Up {
                    services: vec![],
                    build: false,
                    pull: false,
                    force_recreate: false
                }
            ));
            h.controller.retry_compose();
        }
        h.reply_project(Ok(compose_detail("app")));
        h.controller.poll(true);
        assert!(h.controller.can_compose_run("app"));
        h.controller.retry_compose();
        h.reply_project(Err("unavailable".into()));
        h.controller.poll(true);
        assert!(!h.controller.can_compose_run("app"));
        h.controller.retry_compose();
        h.reply_project(Ok(compose_detail("app")));
        h.controller.poll(true);
        h.controller.refresh();
        h.reply("containers_snapshot", Err("engine unavailable".into()));
        h.controller.poll(true);
        assert!(!h.controller.can_compose_run("app"));
        assert_eq!(h.count("compose_run"), 0);
        h.controller.retire();
        assert!(!h.controller.can_compose_run("app"));
    }

    #[test]
    fn compose_inspection_diagnostics_do_not_disable_commands_with_a_known_cli() {
        for action in [
            ComposeAction::Up {
                services: vec![],
                build: false,
                pull: false,
                force_recreate: false,
            },
            ComposeAction::Up {
                services: vec!["web".into()],
                build: false,
                pull: false,
                force_recreate: true,
            },
            ComposeAction::Pull { services: vec![] },
            ComposeAction::Build {
                services: vec![],
                no_cache: false,
            },
            ComposeAction::Scale {
                service: "web".into(),
                replicas: 2,
            },
            ComposeAction::Down {
                volumes: false,
                remove_orphans: false,
            },
        ] {
            let mut h = Harness::new();
            h.snapshot(vec![container('a', "web", "app")]);
            h.controller.select_project("app".into());
            let mut detail = compose_detail("app");
            detail.services = None;
            detail.config_error =
                Some("Compose printed a configuration Newport couldn't read.".into());
            h.reply_project(Ok(detail));
            h.controller.poll(true);
            assert_eq!(h.controller.compose_run_blocked_reason("app"), None);
            assert!(!h.controller.compose_run(
                "app".into(),
                ComposeAction::Scale {
                    service: "web".into(),
                    replicas: 21
                }
            ));
            assert!(
                h.controller.compose_run("app".into(), action.clone()),
                "{action:?}"
            );
            assert_eq!(
                h.last_args("compose_run")["action"],
                serde_json::to_value(&action).unwrap()
            );
            assert_eq!(h.last_args("compose_run")["project"], "app");
            assert!(!h.controller.compose_run("app".into(), action));
            assert_eq!(h.count("compose_run"), 1);
        }
    }

    #[test]
    fn compose_down_is_available_with_missing_files_or_containers_and_a_known_cli() {
        for missing_files in [false, true] {
            let mut h = Harness::new();
            h.snapshot(vec![container('a', "web", "app")]);
            h.controller.select_project("app".into());
            let mut detail = compose_detail("app");
            detail.services = None;
            if missing_files {
                detail.files = vec![compose::File {
                    path: "/srv/app/compose.yaml".into(),
                    text: None,
                    error: Some("No such file".into()),
                }];
                detail.config_error =
                    Some("open /srv/app/compose.yaml: no such file or directory".into());
            } else {
                detail.config_files = vec![];
                detail.config_error = Some("Compose didn't record this project's files.".into());
            }
            h.reply_project(Ok(detail));
            h.controller.poll(true);
            h.snapshot(vec![]);
            assert!(h.controller.can_compose_run("app"));
            assert!(h.controller.compose_run(
                "app".into(),
                ComposeAction::Down {
                    volumes: false,
                    remove_orphans: false
                }
            ));
            assert_eq!(h.last_args("compose_run")["project"], "app");
            assert_eq!(h.count("compose_run"), 1);
        }
    }

    #[test]
    fn compose_backend_preflight_error_is_reported_without_replaying_and_missing_cli_has_a_reason()
    {
        let mut h = Harness::new();
        h.snapshot(vec![container('a', "web", "app")]);
        h.controller.select_project("app".into());
        let mut detail = compose_detail("app");
        detail.cli = None;
        detail.cli_error = Some("The Compose CLI isn't installed on this server.".into());
        detail.config_error = Some("Inspection failed.".into());
        h.reply_project(Ok(detail));
        h.controller.poll(true);
        assert_eq!(
            h.controller.compose_run_blocked_reason("app"),
            Some("Install the Compose CLI on the server to run this command.")
        );
        assert!(!h.controller.compose_run(
            "app".into(),
            ComposeAction::Down {
                volumes: false,
                remove_orphans: false
            }
        ));
        assert_eq!(h.count("compose_run"), 0);
        h.controller.retry_compose();
        let mut detail = compose_detail("app");
        detail.services = None;
        detail.config_error = Some("Could not inspect the project.".into());
        h.reply_project(Ok(detail));
        h.controller.poll(true);
        assert!(
            h.controller
                .compose_run("app".into(), ComposeAction::Pull { services: vec![] })
        );
        let error = "Newport can't read /srv/app/compose.yaml: Permission denied.";
        h.reply("compose_run", Err(error.into()));
        h.controller.poll(false);
        assert_eq!(
            h.controller
                .resource("compose-start")
                .unwrap()
                .error
                .as_deref(),
            Some(error)
        );
        assert!(!h.controller.compose_run_pending());
        assert!(
            !h.controller.can_compose_run("app"),
            "settlement still invalidates engine readings"
        );
        h.controller.poll(true);
        assert_eq!(
            h.count("compose_run"),
            1,
            "agent preflight failures never replay the action"
        );
    }

    #[test]
    fn compose_start_receipts_keep_each_projects_ownership_until_new_jobs_observation() {
        let mut h = Harness::new();
        let rows = vec![container('a', "web", "app"), container('b', "db", "other")];
        h.snapshot(rows.clone());
        h.compose_details("app");
        h.compose_details("other");
        h.reply("compose_jobs", Ok(listing(vec![])));
        h.controller.poll(true);
        h.controller.select_project("app".into());
        assert!(h.controller.compose_run(
            "app".into(),
            ComposeAction::Up {
                services: vec![],
                build: false,
                pull: false,
                force_recreate: false
            }
        ));
        assert!(
            !h.controller.can_compose_run("other"),
            "one physical start worker"
        );
        assert!(!h.controller.can_action(&Action::Prune {
            what: PruneTarget::DanglingImages
        }));
        assert!(!h.controller.can_action(&stop('a', "web")));
        assert!(
            h.controller.can_action(&stop('b', "db")),
            "unrelated project remains independent"
        );
        h.controller.select_project("other".into());
        h.reply("compose_run", Ok(started("job-a")));
        h.controller.poll(true);
        h.reply("containers_snapshot", Ok(snapshot(rows.clone())));
        h.reply_project(Ok(compose_detail("other")));
        h.controller.poll(true);
        assert!(!h.controller.can_compose_run("app"));
        assert_eq!(h.controller.preferred_job("app"), Some("job-a"));
        assert!(h.controller.can_compose_run("other"));
        assert!(h.controller.compose_run(
            "other".into(),
            ComposeAction::Up {
                services: vec![],
                build: false,
                pull: false,
                force_recreate: false
            }
        ));
        h.reply("compose_run", Ok(started("job-b")));
        h.controller.poll(false);
        assert!(h.controller.project_busy("app"));
        assert!(h.controller.project_busy("other"));
        assert_eq!(h.controller.preferred_job("app"), Some("job-a"));
        assert_eq!(h.controller.preferred_job("other"), Some("job-b"));
        // This list was already in flight before job-b was acknowledged.
        h.reply("compose_jobs", Ok(listing(vec![])));
        h.controller.poll(true);
        assert!(h.controller.project_busy("app"));
        assert!(h.controller.project_busy("other"));
        h.reply(
            "compose_jobs",
            Ok(listing(vec![
                project_job("other", "job-b", State::Running),
                project_job("app", "job-a", State::Running),
            ])),
        );
        h.controller.poll(true);
        assert!(h.controller.started_jobs.is_empty());
        assert!(h.controller.project_busy("app"));
        assert!(h.controller.project_busy("other"));
        assert_eq!(h.count("compose_run"), 2);
    }

    #[test]
    fn compose_start_cannot_overlap_an_existing_container_action_or_cleanup() {
        for action in [
            stop('a', "web"),
            Action::Prune {
                what: PruneTarget::DanglingImages,
            },
        ] {
            let mut h = Harness::new();
            h.snapshot(vec![container('a', "web", "app")]);
            h.compose_details("app");
            assert!(h.controller.can_compose_run("app"));
            assert!(h.controller.action(action));
            assert!(!h.controller.can_compose_run("app"));
            assert!(!h.controller.compose_run(
                "app".into(),
                ComposeAction::Up {
                    services: vec![],
                    build: false,
                    pull: false,
                    force_recreate: false
                }
            ));
            assert_eq!(h.count("compose_run"), 0);
        }
    }

    #[test]
    fn cancellation_receipt_stays_with_exact_job_until_terminal_observation() {
        let mut h = Harness::new();
        h.snapshot(vec![
            container('a', "web", "app"),
            container('b', "db", "other"),
        ]);
        h.compose_details("app");
        h.reply(
            "compose_jobs",
            Ok(listing(vec![
                project_job("app", "job-a", State::Running),
                project_job("other", "job-b", State::Running),
            ])),
        );
        h.controller.poll(true);
        assert!(h.controller.select_project_job("app", "job-a".into()));
        assert!(h.controller.compose_cancel("job-a".into()));
        h.controller.select_project("other".into());
        assert!(h.controller.compose_cancelling("job-a"));
        assert!(!h.controller.compose_cancelling("job-b"));
        h.reply("compose_cancel", Ok(cancelled()));
        h.controller.poll(true);
        assert!(!h.controller.compose_cancel_pending());
        assert!(h.controller.compose_cancelling("job-a"));
        assert!(!h.controller.compose_cancel("job-a".into()));
        assert!(!h.controller.can_action(&Action::Prune {
            what: PruneTarget::DanglingImages
        }));
        assert!(
            h.controller.compose_cancel("job-b".into()),
            "a different exact job remains cancellable"
        );
        h.reply("compose_cancel", Err("job-b cancel failed".into()));
        h.controller.poll(true);
        assert!(h.controller.compose_cancelling("job-a"));
        assert!(!h.controller.compose_cancelling("job-b"));
        // First reply is stale after job-b's cancellation completion invalidated it.
        h.reply("compose_jobs", Ok(listing(vec![])));
        h.controller.poll(true);
        assert!(h.controller.compose_cancelling("job-a"));
        h.reply(
            "compose_jobs",
            Ok(listing(vec![
                project_job("app", "job-a", State::Running),
                project_job("other", "job-b", State::Running),
            ])),
        );
        h.controller.poll(true);
        assert!(
            h.controller.compose_cancelling("job-a"),
            "running observation does not finish cancellation"
        );
        h.controller.retry_jobs();
        h.reply(
            "compose_jobs",
            Ok(listing(vec![
                project_job("app", "job-a", State::Cancelled),
                project_job("other", "job-b", State::Running),
            ])),
        );
        h.controller.poll(true);
        assert!(!h.controller.compose_cancelling("job-a"));
        assert!(!h.controller.compose_cancel("job-a".into()));
        assert_eq!(h.count("compose_cancel"), 2);
    }

    #[test]
    fn activity_visibility_gates_polling_but_settles_pending_output_and_allows_explicit_read() {
        let mut h = Harness::new();
        h.controller.select_job("job-a".into());
        h.reply_output(Ok(job("job-a", State::Running, 3, "one")));
        h.controller.poll(true);
        h.advance(2);
        h.controller.poll(true);
        assert_eq!(
            h.count("compose_job"),
            1,
            "inactive Activity gets no periodic reads"
        );
        h.controller.set_activity_active(true);
        assert_eq!(h.count("compose_job"), 2);
        h.controller.set_activity_active(false);
        h.reply_output(Ok(job("job-a", State::Running, 6, "two")));
        h.controller.poll(true);
        assert_eq!(h.controller.job_output().unwrap().output, "onetwo");
        h.advance(2);
        h.controller.poll(true);
        assert_eq!(h.count("compose_job"), 2);
        h.controller.poll(false);
        h.controller.set_activity_active(true);
        assert_eq!(h.count("compose_job"), 2);
        h.controller.poll(true);
        assert_eq!(h.count("compose_job"), 3);
        h.controller.poll(false);
        h.reply_output(Ok(job("job-a", State::Running, 9, "tri")));
        h.controller.poll(false);
        h.advance(5);
        h.controller.poll(false);
        assert_eq!(
            h.count("compose_job"),
            3,
            "hidden completion must not continue polling"
        );
        h.controller.set_activity_active(false);
        h.controller.poll(true);
        assert_eq!(h.count("compose_job"), 3);
        h.controller.retry_job_output();
        assert_eq!(
            h.count("compose_job"),
            4,
            "explicit retry can read once outside Activity"
        );
    }

    #[test]
    fn compose_receipts_do_not_automatically_read_output_outside_activity() {
        let mut h = Harness::new();
        h.snapshot(vec![container('a', "web", "app")]);
        h.compose_details("app");
        h.reply("compose_jobs", Ok(listing(vec![])));
        h.controller.poll(true);
        assert!(h.controller.compose_run(
            "app".into(),
            ComposeAction::Up {
                services: vec![],
                build: false,
                pull: false,
                force_recreate: false
            }
        ));
        h.reply("compose_run", Ok(started("job-a")));
        h.controller.poll(true);
        assert_eq!(h.controller.selected_job(), Some("job-a"));
        assert_eq!(h.count("compose_job"), 0);
        h.reply(
            "compose_jobs",
            Ok(listing(vec![job("job-a", State::Running, 0, "")])),
        );
        h.controller.poll(true);
        assert!(h.controller.compose_cancel("job-a".into()));
        h.reply("compose_cancel", Ok(cancelled()));
        h.controller.poll(true);
        assert!(h.controller.compose_cancelling("job-a"));
        assert_eq!(h.count("compose_job"), 0);
        h.controller.set_activity_active(true);
        assert_eq!(h.count("compose_job"), 1);
    }

    #[test]
    fn terminal_observation_while_cancel_is_pending_does_not_recreate_cancellation_receipt() {
        let mut h = Harness::new();
        h.snapshot(vec![container('a', "web", "app")]);
        h.compose_details("app");
        h.reply(
            "compose_jobs",
            Ok(listing(vec![job("job-a", State::Running, 0, "")])),
        );
        h.controller.poll(true);
        assert!(h.controller.select_project_job("app", "job-a".into()));
        assert!(h.controller.compose_cancel("job-a".into()));
        h.reply_output(Ok(job("job-a", State::Cancelled, 0, "")));
        h.controller.poll(true);
        assert!(
            h.controller.compose_cancelling("job-a"),
            "physical cancellation is still settling"
        );
        h.reply("compose_cancel", Ok(cancelled()));
        h.controller.poll(false);
        assert!(!h.controller.compose_cancelling("job-a"));
        assert!(!h.controller.compose_cancel("job-a".into()));
        assert_eq!(h.count("compose_cancel"), 1);
    }

    #[test]
    fn preferred_activity_job_uses_valid_choice_then_newest_and_rejects_old_project_callbacks() {
        let mut h = Harness::new();
        h.snapshot(vec![
            container('a', "web", "app"),
            container('b', "db", "other"),
        ]);
        h.compose_details("app");
        h.reply(
            "compose_jobs",
            Ok(listing(vec![
                project_job("app", "latest", State::Failed),
                project_job("app", "older", State::Running),
                project_job("other", "elsewhere", State::Running),
            ])),
        );
        h.controller.poll(true);
        assert_eq!(h.controller.preferred_job("app"), Some("latest"));
        assert!(h.controller.select_project_job("app", "older".into()));
        assert_eq!(h.controller.preferred_job("app"), Some("older"));
        h.controller.select_project("other".into());
        let reads = h.count("compose_job");
        assert!(!h.controller.select_project_job("app", "older".into()));
        assert!(!h.controller.select_project_job("other", "latest".into()));
        assert_eq!(h.count("compose_job"), reads);
        assert_eq!(h.controller.preferred_job("other"), Some("elsewhere"));
        assert!(h.controller.select_project_job("other", "elsewhere".into()));
        h.controller.retry_jobs();
        h.reply("compose_jobs", Ok(listing(vec![])));
        h.controller.poll(true);
        assert_eq!(h.controller.preferred_job("other"), None);
        h.controller.clear_job_selection();
        assert_eq!(h.controller.selected_job(), None);
    }

    #[test]
    fn compose_completion_history_is_silent_on_initial_list_and_output_reads() {
        let mut h = Harness::new();
        h.controller.poll(true);
        h.reply(
            "compose_jobs",
            Ok(listing(vec![
                job("past-success", State::Succeeded, 0, ""),
                job("past-failure", State::Failed, 0, ""),
                job("past-cancel", State::Cancelled, 0, ""),
            ])),
        );
        h.controller.poll(true);
        assert!(h.controller.take_compose_completions().is_empty());
        assert!(h.controller.select_job("past-success".into()));
        h.reply_output(Ok(job("past-success", State::Succeeded, 5, "done\n")));
        h.controller.poll(false);
        assert!(h.controller.take_compose_completions().is_empty());
        assert!(h.controller.select_job("unlisted-history".into()));
        h.reply_output(Ok(job("unlisted-history", State::Failed, 5, "fail\n")));
        h.controller.poll(false);
        assert!(h.controller.take_compose_completions().is_empty());
    }

    #[test]
    fn compose_completions_deduplicate_list_and_output_for_every_terminal_state() {
        for state in [State::Succeeded, State::Failed, State::Cancelled] {
            for output_first in [false, true] {
                let mut h = Harness::new();
                h.controller.poll(true);
                h.reply(
                    "compose_jobs",
                    Ok(listing(vec![job("job-a", State::Running, 0, "")])),
                );
                h.controller.poll(true);
                assert!(h.controller.select_job("job-a".into()));
                if output_first {
                    h.reply_output(Ok(job("job-a", state, 5, "done\n")));
                    h.controller.poll(false);
                } else {
                    h.controller.retry_jobs();
                    h.reply(
                        "compose_jobs",
                        Ok(listing(vec![job("job-a", state, 0, "")])),
                    );
                    h.controller.poll(false);
                }
                let receipts = h.controller.take_compose_completions();
                assert_eq!(receipts.len(), 1);
                assert_eq!(receipts[0].job, "job-a");
                assert_eq!(receipts[0].project, "app");
                assert_eq!(receipts[0].state, state);
                assert_eq!(receipts[0].started_at, 1000);
                assert_eq!(receipts[0].finished_at, Some(2000));
                assert_eq!(receipts[0].output, if output_first { "done\n" } else { "" });
                if output_first {
                    h.controller.retry_jobs();
                    h.reply(
                        "compose_jobs",
                        Ok(listing(vec![job("job-a", state, 0, "")])),
                    );
                } else {
                    h.reply_output(Ok(job("job-a", state, 5, "done\n")));
                }
                h.controller.poll(false);
                assert!(h.controller.take_compose_completions().is_empty());
                h.controller.select_job("different-history".into());
                h.reply_output(Ok(job("different-history", State::Succeeded, 0, "")));
                h.controller.poll(false);
                h.controller.select_job("job-a".into());
                h.controller.retry_job_output();
                h.reply_output(Ok(job("job-a", state, 5, "")));
                h.controller.poll(false);
                assert!(h.controller.take_compose_completions().is_empty());
            }
        }
    }

    #[test]
    fn compose_start_receipt_arms_hidden_completion_without_a_running_job_read() {
        let mut h = Harness::new();
        h.snapshot(vec![container('a', "web", "app")]);
        h.compose_details("app");
        h.reply("compose_jobs", Ok(listing(vec![])));
        h.controller.poll(true);
        assert!(h.controller.compose_run(
            "app".into(),
            ComposeAction::Up {
                services: vec![],
                build: false,
                pull: false,
                force_recreate: false
            }
        ));
        h.reply("compose_run", Ok(started("job-a")));
        h.controller.poll(false);
        assert!(h.controller.take_compose_completions().is_empty());
        h.controller.poll(true);
        h.reply(
            "compose_jobs",
            Ok(listing(vec![job("job-a", State::Failed, 0, "")])),
        );
        let dispatched = h.total();
        h.controller.poll(false);
        assert_eq!(
            h.total(),
            dispatched,
            "hidden settlement dispatches no follow-ups"
        );
        let receipts = h.controller.take_compose_completions();
        assert_eq!(receipts.len(), 1);
        assert_eq!(receipts[0].job, "job-a");
        assert_eq!(receipts[0].state, State::Failed);
        assert!(h.controller.take_compose_completions().is_empty());
    }

    #[test]
    fn compose_output_running_observation_owns_completion_and_rejects_other_project_receipt() {
        let mut h = Harness::new();
        h.controller.select_job("job-a".into());
        h.reply_output(Ok(job("job-a", State::Running, 0, "")));
        h.controller.poll(true);
        h.controller.retry_job_output();
        h.reply_output(Ok(job("job-a", State::Cancelled, 5, "done\n")));
        h.controller.poll(false);
        assert_eq!(
            h.controller.take_compose_completions()[0].state,
            State::Cancelled
        );

        h.controller.select_job("job-b".into());
        h.reply_output(Ok(job("job-b", State::Running, 0, "")));
        h.controller.poll(false);
        h.controller.retry_job_output();
        h.reply_output(Ok(project_job("other", "job-b", State::Failed)));
        h.controller.poll(false);
        assert!(h.controller.take_compose_completions().is_empty());
    }

    #[test]
    fn compose_completion_retirement_clears_queued_receipts_and_suppresses_late_reads() {
        let mut h = Harness::new();
        h.controller.poll(true);
        h.reply(
            "compose_jobs",
            Ok(listing(vec![
                job("job-a", State::Running, 0, ""),
                job("job-b", State::Running, 0, ""),
            ])),
        );
        h.controller.poll(true);
        h.controller.select_job("job-b".into());
        h.controller.retry_jobs();
        h.reply(
            "compose_jobs",
            Ok(listing(vec![
                job("job-a", State::Succeeded, 0, ""),
                job("job-b", State::Running, 0, ""),
            ])),
        );
        h.controller.poll(false);
        assert_eq!(h.controller.compose_completions.len(), 1);
        h.controller.retire();
        h.reply_output(Ok(job("job-b", State::Failed, 5, "fail\n")));
        h.controller.poll(false);
        assert!(h.controller.take_compose_completions().is_empty());
        assert!(h.controller.observed_compose_jobs.is_empty());
    }

    #[test]
    fn compose_completion_queue_keeps_latest_bounded_receipts_and_drains_once() {
        let mut h = Harness::new();
        let count = MAX_COMPLETIONS + 5;
        h.controller.poll(true);
        let jobs: Vec<_> = (0..count)
            .map(|i| job(&format!("job-{i}"), State::Running, 0, ""))
            .collect();
        h.reply("compose_jobs", Ok(listing(jobs)));
        h.controller.poll(true);
        h.controller.retry_jobs();
        let jobs: Vec<_> = (0..count)
            .map(|i| job(&format!("job-{i}"), State::Succeeded, 0, ""))
            .collect();
        h.reply("compose_jobs", Ok(listing(jobs)));
        h.controller.poll(false);
        let receipts = h.controller.take_compose_completions();
        assert_eq!(receipts.len(), MAX_COMPLETIONS);
        assert_eq!(receipts.first().unwrap().job, "job-5");
        assert_eq!(receipts.last().unwrap().job, format!("job-{}", count - 1));
        assert!(h.controller.take_compose_completions().is_empty());
        assert!(h.controller.observed_compose_jobs.is_empty());
    }

    #[test]
    fn captured_activity_target_preflight_is_read_only_and_rejects_expired_or_retired_jobs() {
        let mut h = Harness::new();
        h.controller.poll(true);
        h.reply(
            "compose_jobs",
            Ok(listing(vec![job("job-a", State::Succeeded, 0, "")])),
        );
        h.controller.poll(true);
        h.controller.select_project("other".into());
        let dispatched = h.total();
        assert!(h.controller.has_project_job("app", "job-a"));
        assert!(!h.controller.has_project_job("other", "job-a"));
        assert!(!h.controller.has_project_job("app", ""));
        assert_eq!(h.controller.selected_project(), Some("other"));
        assert_eq!(h.total(), dispatched);
        h.controller.retry_jobs();
        h.reply("compose_jobs", Ok(listing(vec![])));
        h.controller.poll(true);
        assert!(!h.controller.has_project_job("app", "job-a"));
        h.controller.retire();
        assert!(!h.controller.has_project_job("app", "job-a"));
    }

    #[test]
    fn compose_output_merges_contiguous_and_overlapping_byte_windows_without_duplicates() {
        for (previous, next, expected) in [
            (None, job("job-a", State::Running, 20, "tail"), "tail"),
            (
                Some(job("job-a", State::Running, 8, "abc\ndef\n")),
                job("job-a", State::Running, 12, "new\n"),
                "abc\ndef\nnew\n",
            ),
            (
                Some(job("job-a", State::Running, 8, "abc\ndef\n")),
                job("job-a", State::Running, 19, "def\nghijklmnop\n"),
                "abc\ndef\nghijklmnop\n",
            ),
            (
                Some(job("job-a", State::Running, 8, "def\n")),
                job("job-a", State::Running, 19, "abc\ndef\nghijklmnop\n"),
                "abc\ndef\nghijklmnop\n",
            ),
            (
                Some(job("job-a", State::Running, 8, "abc\ndef\n")),
                job("job-a", State::Running, 8, "def\n"),
                "abc\ndef\n",
            ),
            (
                Some(job("job-a", State::Running, 8, "abc\ndef\n")),
                job("job-a", State::Succeeded, 8, ""),
                "abc\ndef\n",
            ),
            (
                Some(job("job-a", State::Running, 5, "same\n")),
                job("job-a", State::Running, 10, "same\n"),
                "same\nsame\n",
            ),
            (
                Some(job("job-a", State::Running, 5, "same\n")),
                job("job-a", State::Running, 5, "same\n"),
                "same\n",
            ),
        ] {
            let mut expected_reply = next.clone();
            expected_reply.output = expected.into();
            assert_eq!(
                append_output("job-a", previous.as_ref(), next).unwrap(),
                expected_reply
            );
        }
    }

    #[test]
    fn compose_output_gaps_keep_only_the_new_contiguous_suffix_and_absolute_offsets() {
        let previous = job("job-a", State::Running, 4, "old\n");
        for (end, suffix) in [(100, "new\n"), (100, "old\n"), (100, ""), (u64::MAX, "é\n")] {
            let merged = append_output(
                "job-a",
                Some(&previous),
                job("job-a", State::Running, end, suffix),
            )
            .unwrap();
            assert_eq!(merged.output, suffix);
            assert_eq!(merged.offset, end);
            let lines = newport_domain::compose::output_lines(merged.output.as_str(), end).unwrap();
            if let Some(line) = lines.first() {
                assert_eq!(line.offset, end - suffix.len() as u64);
            }
        }
    }

    #[test]
    fn compose_output_overlap_preserves_utf8_boundaries_and_rejects_inconsistent_bytes() {
        let previous = job("job-a", State::Running, 107, "é🙂\n");
        let merged = append_output(
            "job-a",
            Some(&previous),
            job("job-a", State::Running, 110, "🙂\n€"),
        )
        .unwrap();
        assert_eq!(merged.output, "é🙂\n€");
        assert_eq!(merged.offset, 110);
        for next in [
            job("job-b", State::Running, 110, "🙂\n€"),
            job("job-a", State::Running, 106, ""),
            job("job-a", State::Running, 1, "é"),
            job("job-a", State::Running, 110, "🙃\n€"),
            // Start 101 lies inside the retained é; byte offsets are not chars.
            job("job-a", State::Running, 108, "x🙂\ny"),
            // End 107 would split the new emoji, which cannot extend old text.
            job("job-a", State::Running, 109, "🙂"),
        ] {
            assert!(
                append_output("job-a", Some(&previous), next.clone()).is_err(),
                "{next:?}"
            );
        }
        assert!(append_output("job-a", None, job("job-a", State::Running, 1, "é")).is_err());
        assert!(
            append_output(
                "job-a",
                Some(&job("job-a", State::Running, 1, "é")),
                job("job-a", State::Running, 2, "a")
            )
            .is_err()
        );
    }

    #[test]
    fn compose_output_caps_retention_before_allocating_the_merged_text() {
        let prefix = "€".repeat(MAX_OUTPUT_BYTES / 3);
        let previous_end = prefix.len() as u64;
        let previous = job("job-a", State::Running, previous_end, &prefix);
        let merged = append_output(
            "job-a",
            Some(&previous),
            job("job-a", State::Running, previous_end + 2, "é"),
        )
        .unwrap();
        let output = merged.output.as_str();
        assert_eq!(output, format!("{}é", &prefix[3..]));
        assert!(output.len() <= MAX_OUTPUT_BYTES);
        let large_suffix = "🙂".repeat(MAX_OUTPUT_BYTES / 4 + 2);
        let merged = append_output(
            "job-a",
            Some(&previous),
            job(
                "job-a",
                State::Running,
                previous_end + large_suffix.len() as u64,
                &large_suffix,
            ),
        )
        .unwrap();
        assert_eq!(merged.output.as_str(), &large_suffix[8..]);
        assert_eq!(merged.output.as_str().len(), MAX_OUTPUT_BYTES);
        let duplicate = append_output("job-a", Some(&merged), merged.clone()).unwrap();
        assert_eq!(duplicate, merged);
    }

    #[test]
    fn compose_output_gap_advances_next_read_but_invalid_reply_preserves_cache_and_cursor() {
        let mut h = Harness::new();
        h.controller.select_job("job-a".into());
        h.reply_output(Ok(job("job-a", State::Running, 4, "old\n")));
        h.controller.poll(true);
        h.controller.retry_job_output();
        assert_eq!(h.last_args("compose_job")["offset"], 4);
        h.reply_output(Ok(job("job-a", State::Running, 100, "new\n")));
        h.controller.poll(true);
        assert_eq!(h.controller.job_output().unwrap().output, "new\n");
        let revision = h.controller.resource("compose-output").unwrap().revision;
        h.controller.retry_job_output();
        assert_eq!(h.last_args("compose_job")["offset"], 100);
        h.reply_output(Ok(job("job-a", State::Succeeded, 104, "bad\nend\n")));
        h.controller.poll(true);
        assert_eq!(h.controller.job_output().unwrap().output, "new\n");
        assert_eq!(h.controller.job_output().unwrap().state, State::Running);
        assert_eq!(
            h.controller.resource("compose-output").unwrap().revision,
            revision
        );
        assert!(
            h.controller
                .resource("compose-output")
                .unwrap()
                .error
                .is_some()
        );
        assert!(!h.controller.finished_jobs.iter().any(|job| job == "job-a"));
        h.controller.retry_job_output();
        assert_eq!(h.last_args("compose_job")["offset"], 100);
        h.reply_output(Ok(job("job-a", State::Succeeded, 104, "end\n")));
        h.controller.poll(false);
        assert_eq!(h.controller.job_output().unwrap().output, "new\nend\n");
        assert!(
            h.controller
                .resource("compose-output")
                .unwrap()
                .error
                .is_none()
        );
    }

    #[test]
    fn compose_output_preserves_target_and_offset_and_bounds_utf8_output() {
        let mut h = Harness::new();
        h.controller.set_activity_active(true);
        h.controller.select_job("job-a".into());
        h.controller.select_job("job-b".into());
        assert_eq!(h.count("compose_job"), 1);
        h.reply_output(Ok(job("job-a", State::Running, 3, "old")));
        h.controller.poll(true);
        assert!(h.controller.job_output().is_none());
        assert_eq!(h.last_args("compose_job")["job"], "job-b");
        assert_eq!(h.last_args("compose_job")["offset"], 0);
        h.reply_output(Ok(job("job-b", State::Running, 3, "new")));
        h.controller.poll(true);
        h.advance(1);
        h.controller.poll(true);
        assert_eq!(h.last_args("compose_job")["offset"], 3);
        h.reply_output(Ok(job("job-b", State::Running, 1, "backwards")));
        h.controller.poll(true);
        assert_eq!(h.controller.job_output().unwrap().output, "new");
        assert!(
            h.controller
                .resource("compose-output")
                .unwrap()
                .error
                .is_some()
        );
        h.controller.retry_job_output();
        h.reply_output(Ok(job(
            "job-b",
            State::Succeeded,
            1_200_003,
            &"€".repeat(400_000),
        )));
        h.controller.poll(true);
        let output = h.controller.job_output().unwrap().output.as_str();
        assert!(output.len() <= MAX_OUTPUT_BYTES);
        assert!(output.ends_with('€'));
        let count = h.count("compose_job");
        h.advance(10);
        h.controller.poll(true);
        assert_eq!(h.count("compose_job"), count);
    }

    #[test]
    fn compose_run_and_cancel_survive_navigation_and_do_not_steal_another_projects_selection() {
        let mut h = Harness::new();
        h.snapshot(vec![container('a', "web", "app")]);
        h.compose_details("app");
        h.reply("compose_jobs", Ok(listing(vec![])));
        h.controller.poll(true);
        assert!(!h.controller.compose_run(
            "app".into(),
            ComposeAction::Scale {
                service: "web".into(),
                replicas: 21
            }
        ));
        assert!(h.controller.compose_run(
            "app".into(),
            ComposeAction::Up {
                services: vec![],
                build: false,
                pull: false,
                force_recreate: false
            }
        ));
        assert!(!h.controller.compose_run(
            "app".into(),
            ComposeAction::Up {
                services: vec![],
                build: false,
                pull: false,
                force_recreate: false
            }
        ));
        h.controller.select_project("other".into());
        let count = h.total();
        h.reply("compose_run", Ok(started("job-a")));
        h.controller.poll(false);
        assert_eq!(h.total(), count);
        assert_eq!(h.controller.selected_job(), None);
        h.controller.poll(true);
        h.reply(
            "compose_jobs",
            Ok(listing(vec![job("job-a", State::Running, 0, "")])),
        );
        h.controller.poll(true);
        assert!(h.controller.compose_cancel("job-a".into()));
        assert!(!h.controller.compose_cancel("job-a".into()));
        h.controller.retire();
        let count = h.total();
        h.reply("compose_cancel", Ok(cancelled()));
        h.controller.poll(true);
        assert_eq!(h.total(), count);
        assert!(!h.controller.compose_cancel_pending());
        assert!(!h.controller.compose_cancel.resource.pending);
    }

    #[test]
    fn compose_mutation_errors_stay_with_the_original_project_and_job() {
        let mut h = Harness::new();
        h.snapshot(vec![container('a', "web", "app")]);
        h.compose_details("app");
        assert!(h.controller.compose_run(
            "app".into(),
            ComposeAction::Up {
                services: vec![],
                build: false,
                pull: false,
                force_recreate: false
            }
        ));
        h.controller.select_project("other".into());
        assert!(h.controller.compose_run_pending());
        assert!(h.controller.resource("compose-start").is_none());
        h.reply("compose_run", Err("app failed to start".into()));
        h.controller.poll(false);
        assert!(!h.controller.compose_run_pending());
        assert!(h.controller.resource("compose-start").is_none());
        h.controller.select_project("app".into());
        assert_eq!(
            h.controller
                .resource("compose-start")
                .unwrap()
                .error
                .as_deref(),
            Some("app failed to start")
        );
        h.controller.select_job("job-a".into());
        h.reply_output(Ok(job("job-a", State::Running, 0, "")));
        h.controller.poll(false);
        assert!(h.controller.compose_cancel("job-a".into()));
        h.controller.select_job("job-b".into());
        assert!(h.controller.compose_cancel_pending());
        assert!(h.controller.resource("compose-cancel").is_none());
        h.reply("compose_cancel", Err("job-a could not cancel".into()));
        h.controller.poll(false);
        assert!(h.controller.resource("compose-cancel").is_none());
        h.controller.select_job("job-a".into());
        assert_eq!(
            h.controller
                .resource("compose-cancel")
                .unwrap()
                .error
                .as_deref(),
            Some("job-a could not cancel")
        );
    }

    #[test]
    fn disk_loads_only_when_selected_and_visible_and_reuses_fresh_cache() {
        let mut h = Harness::new();
        h.controller.set_disk_active(true);
        h.controller.retry_disk();
        h.controller.poll(false);
        assert_eq!(h.count("docker_disk"), 0);
        h.controller.poll(true);
        assert_eq!(h.count("docker_disk"), 1);
        for _ in 0..10 {
            h.controller.retry_disk();
        }
        assert_eq!(h.count("docker_disk"), 1);
        assert!(h.controller.resource("docker-disk").unwrap().manual_pending);
        h.reply("docker_disk", Ok(disk(100)));
        h.controller.poll(true);
        h.controller.set_disk_active(false);
        h.advance(29);
        h.controller.set_disk_active(true);
        assert_eq!(h.count("docker_disk"), 1);
        assert_eq!(h.controller.disk().unwrap().totals.size(), 100);
        h.controller.set_disk_active(false);
        h.advance(1);
        h.controller.set_disk_active(true);
        assert_eq!(h.count("docker_disk"), 2);
        h.reply("docker_disk", Ok(disk(90)));
        h.controller.poll(false);
        assert_eq!(h.controller.disk().unwrap().totals.size(), 90);
        h.advance(30);
        h.controller.retry_disk();
        h.controller.poll(false);
        assert_eq!(h.count("docker_disk"), 2);
        h.controller.set_disk_active(false);
        h.controller.poll(true);
        assert_eq!(h.count("docker_disk"), 2);
        h.controller.set_disk_active(true);
        assert_eq!(h.count("docker_disk"), 3);
    }

    fn volume(name: &str, size: Option<u64>, ref_count: u64) -> DockerVolume {
        DockerVolume {
            name: name.into(),
            driver: "local".into(),
            size,
            ref_count: Some(ref_count),
            ..Default::default()
        }
    }
    fn volumes_totals(count: u64, size: u64) -> DiskTotals {
        DiskTotals {
            count,
            size,
            reclaimable: 0,
        }
    }
    fn quick_reading(size: u64) -> DockerDisk {
        let mut quick = disk_reading(size);
        quick.volumes_measured = false;
        quick.volumes = vec![volume("data", None, 1)];
        quick.totals.volumes = volumes_totals(1, 0);
        quick
    }
    fn quick_disk(size: u64) -> ipc::Reply {
        ipc::Reply::Disk(Box::new(quick_reading(size)))
    }

    #[test]
    fn quick_disk_reading_shows_first_and_volumes_are_measured_once_after() {
        let mut h = Harness::new();
        h.controller.set_disk_active(true);
        h.controller.retry_disk();
        h.controller.poll(true);
        assert_eq!(h.count("docker_disk"), 1);
        assert_eq!(h.last_args("docker_disk")["part"], "quick");
        h.reply("docker_disk", Ok(quick_disk(100)));
        h.controller.poll(true);
        let quick_revision = h.controller.disk_revision();
        assert!(quick_revision > 0);
        let shown = h.controller.disk().unwrap();
        assert!(!shown.volumes_measured, "the quick reading shows at once");
        assert_eq!(shown.volumes[0].size, None);
        assert_eq!(h.count("docker_disk"), 2, "volume sizes are read after");
        assert_eq!(h.last_args("docker_disk")["part"], "volumes");
        let mut measured = disk_reading(0);
        measured.volumes = vec![volume("data", Some(700), 1)];
        measured.totals.volumes = volumes_totals(1, 700);
        h.reply("docker_disk", Ok(ipc::Reply::Disk(Box::new(measured))));
        h.controller.poll(true);
        assert!(
            h.controller.disk_revision() > quick_revision,
            "volumes arriving alone still change the presented reading"
        );
        let merged = h.controller.disk().unwrap();
        assert!(merged.volumes_measured);
        assert_eq!(merged.volumes[0].size, Some(700));
        assert_eq!(
            merged.totals.size(),
            800,
            "images from quick, volumes measured"
        );
        // A later quick refresh reuses the recent volume measurement.
        h.advance(31);
        h.controller.set_disk_active(false);
        h.controller.set_disk_active(true);
        h.controller.poll(true);
        assert_eq!(h.count("docker_disk"), 3);
        h.reply("docker_disk", Ok(quick_disk(90)));
        h.controller.poll(true);
        assert_eq!(h.count("docker_disk"), 3, "no rescan within two minutes");
        let reused = h.controller.disk().unwrap();
        assert!(reused.volumes_measured);
        assert_eq!(reused.totals.size(), 790);
        // A volume removed since the measurement leaves the merged reading.
        h.advance(31);
        h.controller.set_disk_active(false);
        h.controller.set_disk_active(true);
        h.controller.poll(true);
        assert_eq!(h.count("docker_disk"), 4);
        let mut removed = quick_reading(90);
        removed.volumes = vec![];
        removed.totals.volumes = volumes_totals(0, 0);
        h.reply("docker_disk", Ok(ipc::Reply::Disk(Box::new(removed))));
        h.controller.poll(true);
        let current = h.controller.disk().unwrap();
        assert!(current.volumes.is_empty(), "no stale volume rows");
        assert!(current.volumes_measured);
        assert_eq!(current.totals.volumes.count, 0);
        assert_eq!(current.totals.size(), 90);
        assert_eq!(h.count("docker_disk"), 4);
        // A volume created since is measured at once, not in two minutes.
        h.advance(31);
        h.controller.set_disk_active(false);
        h.controller.set_disk_active(true);
        h.controller.poll(true);
        let mut created = quick_reading(90);
        created.volumes = vec![volume("data", None, 1), volume("logs", None, 0)];
        created.totals.volumes = volumes_totals(2, 0);
        h.reply("docker_disk", Ok(ipc::Reply::Disk(Box::new(created))));
        h.controller.poll(true);
        let partial = h.controller.disk().unwrap();
        assert!(!partial.volumes_measured, "the new volume is not sized yet");
        assert_eq!(partial.volumes[0].size, Some(700));
        assert_eq!(partial.volumes[1].size, None);
        assert_eq!(h.count("docker_disk"), 6);
        assert_eq!(h.last_args("docker_disk")["part"], "volumes");
    }

    #[test]
    fn measured_quick_reading_never_scans_volumes_twice() {
        let mut h = Harness::new();
        h.controller.set_disk_active(true);
        h.controller.retry_disk();
        h.controller.poll(true);
        h.reply("docker_disk", Ok(disk(100)));
        h.controller.poll(true);
        assert!(h.controller.disk().unwrap().volumes_measured);
        h.controller.retry_disk();
        h.controller.poll(true);
        assert_eq!(h.count("docker_disk"), 2, "only the refresh itself");
        assert_eq!(h.last_args("docker_disk")["part"], "quick");
    }

    #[test]
    fn an_unexpected_disk_reply_keeps_typed_data_and_requires_retry_or_reactivation() {
        let mut h = Harness::new();
        h.controller.poll(true);
        h.controller.set_disk_active(true);
        h.reply("docker_disk", Ok(disk(100)));
        h.controller.poll(true);
        let updated = h.controller.resource("docker-disk").unwrap().updated_at;
        h.controller.retry_disk();
        h.reply("docker_disk", Ok(logs("not a disk reading", None)));
        h.controller.poll(true);
        assert_eq!(h.controller.disk().unwrap().totals.size(), 100);
        assert_eq!(
            h.controller.resource("docker-disk").unwrap().updated_at,
            updated
        );
        assert!(
            h.controller
                .resource("docker-disk")
                .unwrap()
                .error
                .as_deref()
                .unwrap()
                .starts_with("Unexpected Containers response")
        );
        h.advance(60);
        for _ in 0..10 {
            h.controller.set_disk_active(true);
            h.controller.poll(true);
        }
        assert_eq!(h.count("docker_disk"), 2);
        h.controller.retry_disk();
        assert!(
            h.controller
                .resource("docker-disk")
                .unwrap()
                .error
                .is_some()
        );
        h.reply("docker_disk", Ok(disk(75)));
        h.controller.poll(true);
        assert_eq!(h.controller.disk().unwrap().totals.size(), 75);
        assert!(
            h.controller
                .resource("docker-disk")
                .unwrap()
                .error
                .is_none()
        );
        assert!(!h.controller.resource("docker-disk").unwrap().manual_pending);
    }

    #[test]
    fn cleanup_rejects_pre_mutation_disk_results_and_reports_typed_reclaimed_bytes() {
        let mut h = Harness::new();
        h.snapshot(vec![container('a', "web", "app")]);
        h.controller.set_disk_active(true);
        h.reply("docker_disk", Ok(disk(100)));
        h.controller.poll(true);
        h.controller.retry_disk();
        let action = Action::Prune {
            what: PruneTarget::DanglingImages,
        };
        assert!(h.controller.action(action.clone()));
        assert!(!h.controller.action(action.clone()));
        assert_eq!(
            h.last_args("docker_prune"),
            json!({"id":"00000000-0000-0000-0000-00000000000a","what":"dangling-images"})
        );
        h.reply("docker_disk", Ok(disk(999)));
        h.reply("docker_prune", Ok(pruned(2, 40)));
        h.controller.poll(true);
        assert_eq!(h.controller.disk().unwrap().totals.size(), 100);
        assert_eq!(h.count("docker_disk"), 3);
        let receipts = h.controller.take_action_completions();
        assert_eq!(receipts.len(), 1);
        assert_eq!(receipts[0].action, action);
        assert_eq!(
            receipts[0].pruned(),
            Some(Pruned {
                removed: 2,
                reclaimed: 40
            })
        );
        assert_eq!(receipts[0].result.as_ref().unwrap(), &pruned(2, 40));
        h.reply("docker_disk", Ok(disk(60)));
        h.controller.poll(true);
        assert_eq!(h.controller.disk().unwrap().totals.size(), 60);
        assert_eq!(h.count("docker_prune"), 1);
    }

    #[test]
    fn failed_cleanup_queues_disk_refresh_until_reselected_and_never_replays() {
        let mut h = Harness::new();
        h.snapshot(vec![container('a', "web", "app")]);
        h.controller.set_disk_active(true);
        h.reply("docker_disk", Ok(disk(100)));
        h.controller.poll(true);
        let action = Action::Prune {
            what: PruneTarget::Volumes,
        };
        assert!(h.controller.action(action.clone()));
        h.controller.set_disk_active(false);
        h.reply("docker_prune", Err("cleanup connection lost".into()));
        h.controller.poll(false);
        assert_eq!(h.count("docker_disk"), 1);
        assert!(h.controller.disk.dirty);
        let receipts = h.controller.take_action_completions();
        assert!(receipts[0].result.is_err());
        assert_eq!(receipts[0].pruned(), None);
        assert_eq!(
            h.controller
                .action_status(&action.target())
                .unwrap()
                .error
                .as_deref(),
            Some("cleanup connection lost")
        );
        h.controller.poll(true);
        assert_eq!(h.count("docker_disk"), 1);
        h.controller.set_disk_active(true);
        assert_eq!(h.count("docker_disk"), 2);
        assert_eq!(h.controller.disk().unwrap().totals.size(), 100);
        assert_eq!(h.count("docker_prune"), 1);
        h.controller.retire();
        h.reply("docker_disk", Ok(disk(50)));
        h.controller.poll(true);
        h.controller.set_disk_active(true);
        h.controller.retry_disk();
        assert_eq!(h.count("docker_disk"), 2);
        assert!(h.controller.disk().is_none());
    }

    #[test]
    fn disk_completion_after_deselection_is_cached_without_followup_requests() {
        let mut h = Harness::new();
        h.controller.poll(true);
        h.controller.set_disk_active(true);
        h.controller.set_disk_active(false);
        h.reply("docker_disk", Ok(disk(80)));
        h.controller.poll(true);
        assert_eq!(h.controller.disk().unwrap().totals.size(), 80);
        h.advance(60);
        h.controller.retry_disk();
        h.controller.poll(true);
        assert_eq!(h.count("docker_disk"), 1);
        h.controller.set_disk_active(true);
        assert_eq!(h.count("docker_disk"), 2);
    }

    #[test]
    fn shell_command_is_scoped_to_observed_identity_and_does_not_dispatch_or_follow_selection() {
        let mut h = Harness::new();
        assert!(h.controller.shell_command(&id('a'), "web").is_err());
        h.snapshot(vec![
            container('a', "web", "app"),
            container('b', "db", "data"),
        ]);
        h.controller.select_container("db".into());
        let requests = h.total();
        let command = h.controller.shell_command(&id('a'), "web").unwrap();
        assert!(command.starts_with(&format!("docker exec -it '{}'", id('a'))));
        assert!(!command.contains(&id('b')));
        assert_eq!(h.total(), requests);
        assert!(h.controller.shell_command(&id('a'), "db").is_err());
        assert!(h.controller.shell_command(&id('c'), "web").is_err());
        assert!(h.controller.shell_command(&id('a')[..12], "web").is_err());
        h.controller.refresh();
        assert!(
            h.controller.shell_command(&id('a'), "web").is_ok(),
            "ordinary refresh retains last accepted state"
        );
        let mut podman = ContainersSnapshot {
            containers: vec![container('a', "web", "app")],
            ..Default::default()
        };
        podman.engine.podman = true;
        h.reply(
            "containers_snapshot",
            Ok(ipc::Reply::Snapshot(Box::new(podman))),
        );
        h.controller.poll(true);
        assert!(
            h.controller
                .shell_command(&id('a'), "web")
                .unwrap()
                .starts_with("podman exec -it")
        );
        h.snapshot(vec![container('c', "web", "app")]);
        assert!(h.controller.shell_command(&id('a'), "web").is_err());
        assert!(h.controller.shell_command(&id('c'), "web").is_ok());
    }

    #[test]
    fn shell_rechecks_running_state_failed_snapshots_mutations_and_retirement() {
        let mut h = Harness::new();
        for state in [
            ContainerState::Created,
            ContainerState::Running,
            ContainerState::Paused,
            ContainerState::Restarting,
            ContainerState::Removing,
            ContainerState::Exited,
            ContainerState::Dead,
        ] {
            let mut row = container('a', "web", "app");
            row.state = state;
            h.snapshot(vec![row]);
            assert_eq!(
                h.controller.shell_command(&id('a'), "web").is_ok(),
                state == ContainerState::Running,
                "{state:?}"
            );
        }
        h.snapshot(vec![container('a', "web", "app")]);
        h.controller.refresh();
        h.reply("containers_snapshot", Err("offline".into()));
        h.controller.poll(true);
        assert!(h.controller.shell_command(&id('a'), "web").is_err());
        h.snapshot(vec![container('a', "web", "app")]);
        assert!(h.controller.action(stop('a', "web")));
        assert!(h.controller.shell_command(&id('a'), "web").is_err());
        h.reply("container_action", Err("stop outcome uncertain".into()));
        h.controller.poll(true);
        assert!(h.controller.shell_command(&id('a'), "web").is_err());
        h.reply(
            "containers_snapshot",
            Ok(snapshot(vec![container('a', "web", "app")])),
        );
        h.controller.poll(true);
        assert!(h.controller.shell_command(&id('a'), "web").is_ok());
        h.controller.retire();
        assert!(h.controller.shell_command(&id('a'), "web").is_err());
    }

    #[test]
    fn shell_and_removal_respect_project_jobs_without_blocking_unrelated_projects() {
        let mut h = Harness::new();
        let mut stopped = container('c', "old-web", "app");
        stopped.state = ContainerState::Exited;
        let rows = vec![
            container('a', "web", "app"),
            container('b', "db", "other"),
            stopped,
        ];
        let removal = Action::Remove {
            containers: vec![id('c')],
            names: vec!["old-web".into()],
        };
        h.snapshot(rows.clone());
        h.compose_details("app");
        h.reply("compose_jobs", Ok(listing(vec![])));
        h.controller.poll(true);
        assert!(h.controller.can_action(&removal));
        assert!(h.controller.compose_run(
            "app".into(),
            ComposeAction::Up {
                services: vec![],
                build: false,
                pull: false,
                force_recreate: false
            }
        ));
        assert!(h.controller.shell_command(&id('a'), "web").is_err());
        assert!(h.controller.shell_command(&id('b'), "db").is_ok());
        assert!(!h.controller.can_action(&removal));
        assert!(!h.controller.can_action(&stop('a', "web")));
        h.reply("compose_run", Ok(started("job-a")));
        h.controller.poll(true);
        h.reply("containers_snapshot", Ok(snapshot(rows.clone())));
        h.controller.poll(true);
        assert!(
            h.controller.shell_command(&id('a'), "web").is_err(),
            "start remains owned before first jobs observation"
        );
        assert!(h.controller.shell_command(&id('b'), "db").is_ok());
        h.reply(
            "compose_jobs",
            Ok(listing(vec![job("job-a", State::Running, 0, "")])),
        );
        h.controller.poll(true);
        assert!(!h.controller.can_action(&removal));
        assert!(h.controller.compose_cancel("job-a".into()));
        assert!(h.controller.shell_command(&id('a'), "web").is_err());
        h.reply("compose_cancel", Ok(cancelled()));
        h.controller.poll(true);
        h.reply(
            "compose_jobs",
            Ok(listing(vec![job("job-a", State::Cancelled, 0, "")])),
        );
        h.reply("containers_snapshot", Ok(snapshot(rows.clone())));
        h.controller.poll(true);
        // The job-completion invalidation precedes the concurrent snapshot.
        h.reply("containers_snapshot", Ok(snapshot(rows)));
        h.controller.poll(true);
        assert!(h.controller.shell_command(&id('a'), "web").is_ok());
        assert!(h.controller.can_action(&removal));
    }

    #[test]
    fn removal_confirmation_rechecks_exact_identity_name_and_stopped_state() {
        let mut h = Harness::new();
        let captured = Action::Remove {
            containers: vec![id('a')],
            names: vec!["web".into()],
        };
        for state in [
            ContainerState::Created,
            ContainerState::Running,
            ContainerState::Paused,
            ContainerState::Restarting,
            ContainerState::Removing,
            ContainerState::Exited,
            ContainerState::Dead,
        ] {
            let mut row = container('a', "web", "app");
            row.state = state;
            h.snapshot(vec![row]);
            assert_eq!(
                h.controller.can_action(&captured),
                matches!(
                    state,
                    ContainerState::Created | ContainerState::Exited | ContainerState::Dead
                ),
                "{state:?}"
            );
        }
        let mut recreated = container('b', "web", "app");
        recreated.state = ContainerState::Exited;
        h.snapshot(vec![recreated]);
        assert!(!h.controller.action(captured));
        for action in [
            Action::Remove {
                containers: vec![id('b')],
                names: vec!["renamed".into()],
            },
            Action::Remove {
                containers: vec![id('b')],
                names: vec![],
            },
            Action::Remove {
                containers: vec![id('b'), id('b')],
                names: vec!["web".into(), "web".into()],
            },
            Action::Remove {
                containers: vec![id('b')[..12].into()],
                names: vec!["web".into()],
            },
        ] {
            assert!(!h.controller.action(action));
        }
        assert_eq!(h.count("containers_remove"), 0);
    }

    #[test]
    fn newer_compose_list_releases_an_expired_job_without_old_output_blocking_shell() {
        let mut h = Harness::new();
        h.snapshot(vec![container('a', "web", "app")]);
        h.reply(
            "compose_jobs",
            Ok(listing(vec![job("job-a", State::Running, 0, "")])),
        );
        h.controller.poll(true);
        h.controller.select_job("job-a".into());
        h.reply_output(Ok(job("job-a", State::Running, 0, "")));
        h.controller.poll(true);
        assert!(h.controller.shell_command(&id('a'), "web").is_err());
        h.controller.retry_jobs();
        h.reply("compose_jobs", Ok(listing(vec![])));
        h.controller.poll(true);
        assert!(h.controller.shell_command(&id('a'), "web").is_ok());
    }

    #[test]
    fn confirmed_removal_keeps_its_target_across_selection_error_and_retirement() {
        let mut h = Harness::new();
        let mut a = container('a', "web", "app");
        a.state = ContainerState::Exited;
        let mut b = container('b', "db", "data");
        b.state = ContainerState::Exited;
        let rows = vec![a, b];
        h.snapshot(rows.clone());
        let confirmed = Action::Remove {
            containers: vec![id('a')],
            names: vec!["web".into()],
        };
        h.controller.select_container("db".into());
        assert!(h.controller.action(confirmed.clone()));
        assert_eq!(
            h.last_args("containers_remove"),
            json!({"id":"00000000-0000-0000-0000-00000000000a","containers":[id('a')]})
        );
        assert!(!h.controller.action(confirmed.clone()));
        let requests = h.total();
        h.reply("containers_remove", Err("remove timed out".into()));
        h.controller.poll(false);
        assert_eq!(h.total(), requests);
        let receipts = h.controller.take_action_completions();
        assert_eq!(receipts[0].action, confirmed);
        assert_eq!(receipts[0].result.as_ref().unwrap_err(), "remove timed out");
        assert_eq!(h.controller.selected_name(), Some("db"));
        assert_eq!(
            h.controller
                .action_status(&confirmed.target())
                .unwrap()
                .error
                .as_deref(),
            Some("remove timed out")
        );
        h.snapshot(rows);
        assert!(h.controller.action(confirmed.clone()));
        h.controller.retire();
        let requests = h.total();
        h.reply("containers_remove", Ok(removed(1)));
        h.controller.poll(true);
        assert_eq!(h.total(), requests);
        let receipts = h.controller.take_action_completions();
        assert_eq!(receipts[0].action, confirmed);
        assert_eq!(receipts[0].result.as_ref().unwrap(), &removed(1));
        assert_eq!(h.count("containers_remove"), 2);
    }

    #[test]
    fn port_destination_validates_exact_observed_binding_and_normalizes_reachable_addresses() {
        use wire::PortProtocol;
        let mut h = Harness::new();
        let bindings = vec![
            ContainerPort {
                ip: Some("0.0.0.0".into()),
                host_port: Some(8080),
                container_port: 80,
                protocol: PortProtocol::Tcp,
            },
            ContainerPort {
                ip: Some("0:0:0:0:0:0:0:0".into()),
                host_port: Some(8081),
                container_port: 81,
                protocol: PortProtocol::Tcp,
            },
            ContainerPort {
                ip: None,
                host_port: Some(5353),
                container_port: 53,
                protocol: PortProtocol::Udp,
            },
            ContainerPort {
                ip: Some("10.0.0.2".into()),
                host_port: Some(9090),
                container_port: 90,
                protocol: PortProtocol::Tcp,
            },
            ContainerPort {
                ip: None,
                host_port: None,
                container_port: 3000,
                protocol: PortProtocol::Tcp,
            },
        ];
        let mut web = container('a', "app-web-1", "app");
        web.ports = bindings.clone();
        web.compose.as_mut().unwrap().service = "web".into();
        h.snapshot(vec![web, container('b', "db", "data")]);
        h.controller.select_container("db".into());
        let requests = h.total();
        let destination = h
            .controller
            .port_destination(&id('a'), "app-web-1", &bindings[0])
            .unwrap();
        assert_eq!(
            (
                destination.address.as_str(),
                destination.port,
                destination.name.as_str()
            ),
            ("127.0.0.1", 8080, "web")
        );
        let destination = h
            .controller
            .port_destination(&id('a'), "app-web-1", &bindings[1])
            .unwrap();
        assert_eq!(
            (destination.address.as_str(), destination.port),
            ("::1", 8081)
        );
        for binding in &bindings[2..] {
            assert!(
                h.controller
                    .port_destination(&id('a'), "app-web-1", binding)
                    .is_err()
            );
        }
        let mut changed = bindings[0].clone();
        changed.container_port = 82;
        assert!(
            h.controller
                .port_destination(&id('a'), "app-web-1", &changed)
                .is_err()
        );
        let mut changed = bindings[1].clone();
        changed.ip = Some("::".into());
        assert!(
            h.controller
                .port_destination(&id('a'), "app-web-1", &changed)
                .is_err(),
            "canonical equivalence does not replace exact observed binding"
        );
        assert!(
            h.controller
                .port_destination(&id('a'), "db", &bindings[0])
                .is_err()
        );
        assert_eq!(
            h.total(),
            requests,
            "projection must not dispatch transport"
        );
    }

    #[test]
    fn port_clicks_reject_stopped_recreated_stale_and_conflicting_targets() {
        use wire::PortProtocol;
        let mut h = Harness::new();
        let port = ContainerPort {
            ip: None,
            host_port: Some(8080),
            container_port: 80,
            protocol: PortProtocol::Tcp,
        };
        let mut row = container('a', "web", "app");
        row.ports = vec![port.clone()];
        assert!(
            h.controller
                .port_destination(&id('a'), "web", &port)
                .is_err()
        );
        for state in [
            ContainerState::Created,
            ContainerState::Running,
            ContainerState::Paused,
            ContainerState::Restarting,
            ContainerState::Removing,
            ContainerState::Exited,
            ContainerState::Dead,
        ] {
            row.state = state;
            h.snapshot(vec![row.clone()]);
            assert_eq!(
                h.controller
                    .port_destination(&id('a'), "web", &port)
                    .is_ok(),
                state == ContainerState::Running
            );
        }
        row.state = ContainerState::Running;
        row.id = id('b');
        h.snapshot(vec![row.clone()]);
        assert!(
            h.controller
                .port_destination(&id('a'), "web", &port)
                .is_err()
        );
        assert!(
            h.controller
                .port_destination(&id('b'), "web", &port)
                .is_ok()
        );
        h.controller.refresh();
        h.reply("containers_snapshot", Err("offline".into()));
        h.controller.poll(true);
        assert!(
            h.controller
                .port_destination(&id('b'), "web", &port)
                .is_err()
        );
        h.snapshot(vec![row.clone()]);
        assert!(h.controller.action(stop('b', "web")));
        assert!(
            h.controller
                .port_destination(&id('b'), "web", &port)
                .is_err()
        );
        h.reply("container_action", Err("uncertain".into()));
        h.controller.poll(true);
        assert!(
            h.controller
                .port_destination(&id('b'), "web", &port)
                .is_err()
        );
        h.reply("containers_snapshot", Ok(snapshot(vec![row])));
        h.controller.poll(true);
        h.compose_details("app");
        assert!(h.controller.compose_run(
            "app".into(),
            ComposeAction::Up {
                services: vec![],
                build: false,
                pull: false,
                force_recreate: false
            }
        ));
        let requests = h.total();
        assert!(
            h.controller
                .port_destination(&id('b'), "web", &port)
                .is_err()
        );
        assert_eq!(h.total(), requests);
        h.controller.retire();
        assert!(
            h.controller
                .port_destination(&id('b'), "web", &port)
                .is_err()
        );
    }
    fn measured(digit: char, cpu: Option<f64>, memory: Option<u64>) -> Container {
        Container {
            cpu,
            memory,
            ..container(digit, "web", "app")
        }
    }

    #[test]
    fn stats_sample_only_snapshot_receipts_and_survive_navigation_and_hidden_time() {
        use crate::containers_stats::{history_duration_label, history_span};
        let mut h = Harness::new();
        let first = h.now.get();
        h.snapshot(vec![measured('a', Some(1.), Some(10))]);
        let requests = h.total();
        for _ in 0..10 {
            h.controller.poll(true);
            assert_eq!(h.controller.stats_history(&id('a')).unwrap().cpu().len(), 1);
        }
        assert_eq!(
            h.total(),
            requests,
            "reading/rendering history does not dispatch"
        );
        h.controller.select_container("web".into());
        h.controller.select_project("app".into());
        h.controller.clear_selection();
        h.controller.poll(false);
        let requests = h.total();
        h.advance(137);
        h.controller.poll(false);
        assert_eq!(
            h.total(),
            requests,
            "hidden history must not introduce polling"
        );
        assert_eq!(h.controller.stats_history(&id('a')).unwrap().cpu().len(), 1);
        h.controller.poll(true);
        h.reply(
            "containers_snapshot",
            Ok(snapshot(vec![measured('a', Some(2.), None)])),
        );
        h.controller.poll(false);
        let history = h.controller.stats_history(&id('a')).unwrap();
        assert_eq!(history.cpu().len(), 2);
        assert_eq!(
            history.memory().len(),
            1,
            "unknown memory is not recorded as zero"
        );
        assert_eq!(history.cpu()[0].received_at, first);
        assert_eq!(history.cpu()[1].received_at, h.now.get());
        assert_eq!(history.cpu()[1].sequence, 2);
        assert_eq!(history_span(history.cpu()), Duration::from_secs(137));
        assert_eq!(history_duration_label(history.cpu()), "2m");
    }

    #[test]
    fn stats_failed_or_unexpected_snapshot_reply_does_not_sample_retained_values() {
        let mut h = Harness::new();
        h.snapshot(vec![measured('a', Some(1.), Some(10))]);
        for response in [Err("offline".into()), Ok(disk(1))] {
            h.advance(5);
            h.controller.refresh();
            h.reply("containers_snapshot", response);
            h.controller.poll(true);
            let history = h.controller.stats_history(&id('a')).unwrap();
            assert_eq!(history.cpu().len(), 1);
            assert_eq!(history.memory().len(), 1);
        }
        h.advance(8);
        h.snapshot(vec![measured('a', None, Some(20))]);
        let history = h.controller.stats_history(&id('a')).unwrap();
        assert_eq!(history.cpu().len(), 1);
        assert_eq!(history.memory().len(), 2);
        assert_eq!(history.memory()[1].sequence, 2);
        assert_eq!(
            crate::containers_stats::history_span(history.memory()),
            Duration::from_secs(18)
        );
    }

    #[test]
    fn stats_reject_pre_mutation_snapshot_and_clear_on_retirement() {
        let mut h = Harness::new();
        h.snapshot(vec![measured('a', Some(1.), Some(10))]);
        h.advance(5);
        h.controller.refresh();
        assert!(h.controller.action(stop('a', "web")));
        h.reply("container_action", Err("stop outcome unavailable".into()));
        h.reply(
            "containers_snapshot",
            Ok(snapshot(vec![measured('a', Some(999.), Some(999))])),
        );
        h.controller.poll(true);
        assert_eq!(h.controller.stats_history(&id('a')).unwrap().cpu().len(), 1);
        h.advance(7);
        h.reply(
            "containers_snapshot",
            Ok(snapshot(vec![measured('a', Some(2.), Some(20))])),
        );
        h.controller.poll(true);
        let history = h.controller.stats_history(&id('a')).unwrap();
        assert_eq!(
            history
                .cpu()
                .iter()
                .map(|sample| sample.value)
                .collect::<Vec<_>>(),
            [1., 2.]
        );
        assert_eq!(
            history.cpu()[1].sequence,
            2,
            "rejected generation cannot advance history"
        );
        h.controller.refresh();
        h.controller.retire();
        let requests = h.total();
        h.reply(
            "containers_snapshot",
            Ok(snapshot(vec![measured('a', Some(3.), Some(30))])),
        );
        h.controller.poll(true);
        assert!(h.controller.stats_history(&id('a')).is_none());
        assert_eq!(h.total(), requests);
    }

    #[test]
    fn stats_use_full_engine_identity_and_keep_non_running_gaps() {
        let mut h = Harness::new();
        h.snapshot(vec![measured('a', Some(1.), Some(10))]);
        h.advance(5);
        let mut stopped = measured('a', Some(80.), Some(90));
        stopped.state = ContainerState::Exited;
        h.snapshot(vec![stopped]);
        assert_eq!(h.controller.stats_history(&id('a')).unwrap().cpu().len(), 1);
        h.advance(5);
        h.snapshot(vec![measured('a', Some(2.), Some(20))]);
        assert_eq!(
            h.controller.stats_history(&id('a')).unwrap().cpu()[1].sequence,
            3
        );
        h.advance(5);
        h.snapshot(vec![measured('b', Some(3.), None)]);
        assert!(
            h.controller.stats_history(&id('a')).is_none(),
            "same name with a new engine ID starts fresh"
        );
        let history = h.controller.stats_history(&id('b')).unwrap();
        assert_eq!(history.cpu().len(), 1);
        assert_eq!(history.cpu()[0].value, 3.);
        assert!(history.memory().is_empty());
        let other_connection = Harness::new();
        assert!(
            other_connection
                .controller
                .stats_history(&id('b'))
                .is_none()
        );
    }
}
