//! Session-owned runtime control channel, separate from feature traffic. The
//! runtime runs on a session channel of its own, so losing that channel's
//! connection ends the runtime and fences everything that ran on it.
use crate::remote::command;
use newport_ssh::{Fate, Link};
use serde::Serialize;
use std::{
    sync::{
        Arc, Mutex as StdMutex, Weak,
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    sync::{Semaphore, mpsc, oneshot, watch},
    task::JoinHandle,
    time::Instant,
};

#[path = "agent_runtime_worker.rs"]
mod worker;
pub use worker::WorkerStream;
pub(crate) use worker::{Handle, Worker};

pub(crate) const BUSY: &str = "Needle services are busy. Retry shortly.";
pub(crate) const UPGRADED: &str = "Needle runtime upgraded";
const MAX_RETIRING: usize = 4;
/// How long a runtime may take to say hello once its channel is open.
const HANDSHAKE: Duration = Duration::from_secs(15);

#[allow(dead_code)]
#[path = "../../../tools/agent/src/runtime_protocol.rs"]
pub mod protocol;

/// Why Needle couldn't serve a request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    /// The link couldn't open a channel, or lost the connection under one.
    Link(newport_ssh::Error),
    /// The server session changed; nothing more runs on it.
    Stale,
    /// Needle isn't installed on the server.
    NotInstalled,
    /// Needle is busy, restarting or was cut off; trying again soon can work.
    Interrupted(String),
    /// Needle can't serve this until something on the server changes, such
    /// as an update.
    Unavailable(String),
}
impl Error {
    /// Whether trying again can work without anything changing on the
    /// server: the link recovers on its own or on Retry, and an interrupted
    /// runtime restarts.
    pub(crate) fn transient(&self) -> bool {
        matches!(self, Error::Link(_) | Error::Interrupted(_))
    }
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Link(error) => error.fmt(f),
            Error::Stale => f.write_str(crate::server_session::STALE),
            Error::NotInstalled => f.write_str(crate::agent::NOT_INSTALLED),
            Error::Interrupted(message) | Error::Unavailable(message) => f.write_str(message),
        }
    }
}
impl std::error::Error for Error {}
impl From<Error> for String {
    fn from(error: Error) -> Self {
        error.to_string()
    }
}
impl From<command::Error> for Error {
    fn from(error: command::Error) -> Self {
        match error {
            command::Error::Link(error) => Error::Link(error),
            other => Error::Interrupted(other.to_string()),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ClipboardRoute {
    Applied(u64),
    Retired,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub status: &'static str,
    /// Which runtime start this is.
    pub generation: u64,
    pub hello: Option<protocol::Hello>,
    pub error: Option<String>,
    pub retryable: bool,
    pub failures: u32,
    pub instances: Vec<RuntimeInstance>,
    #[serde(skip)]
    cause: Option<Error>,
    #[serde(skip)]
    retry_at: Option<Instant>,
    /// The failure was the connection's loss, not Needle's.
    #[serde(skip)]
    lost: bool,
    #[serde(skip)]
    startup_epoch: u64,
}
impl Default for Snapshot {
    fn default() -> Self {
        Self {
            status: "unknown",
            generation: 0,
            hello: None,
            error: None,
            retryable: false,
            failures: 0,
            instances: Vec::new(),
            cause: None,
            retry_at: None,
            lost: false,
            startup_epoch: 0,
        }
    }
}
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeInstance {
    pub instance_id: String,
    pub build_sha256: String,
    pub generation: u64,
    pub retiring: bool,
    pub draining: bool,
    pub connected: bool,
    pub registrations: usize,
    pub worker_leases: usize,
}
// A failed or cancelled owned startup must not leave observers permanently busy.
struct Starting {
    snapshot: Arc<StdMutex<Snapshot>>,
    generation: u64,
    epoch: u64,
    changes: watch::Sender<u64>,
}
impl Drop for Starting {
    fn drop(&mut self) {
        let mut current = self.snapshot.lock().unwrap();
        if current.status == "starting"
            && current.generation == self.generation
            && current.startup_epoch == self.epoch
        {
            record_failure(
                &mut current,
                Error::Interrupted("Needle runtime startup was interrupted. Reconnecting…".into()),
                true,
                false,
            );
        }
        drop(current);
        self.changes
            .send_modify(|version| *version = version.wrapping_add(1));
    }
}
struct Call {
    operation: protocol::Operation,
    reply: oneshot::Sender<Result<protocol::Reply, String>>,
}
struct Control {
    commands: mpsc::Sender<Call>,
    generation: u64,
    hello: protocol::Hello,
    heartbeat: JoinHandle<()>,
    failed: Arc<AtomicBool>,
    permission_epoch: Arc<AtomicU64>,
    clipboard_revision: StdMutex<u64>,
    registrations: AtomicUsize,
    workers: Arc<Semaphore>,
    drain: watch::Sender<bool>,
}
impl Control {
    fn is_closed(&self) -> bool {
        self.failed.load(Ordering::Acquire) || self.heartbeat.is_finished()
    }
    fn stop(&self) {
        self.failed.store(true, Ordering::Release);
        self.heartbeat.abort();
    }
}
impl Drop for Control {
    fn drop(&mut self) {
        self.stop();
    }
}
struct Startup {
    generation: u64,
    epoch: u64,
    task: Option<JoinHandle<()>>,
}
impl Drop for Startup {
    fn drop(&mut self) {
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}
/// A caller waiting in [`Slot::ensure_with`]. When the last one leaves, a
/// start in progress is cancelled, so nothing keeps the link logging in for
/// a runtime nobody needs.
struct Waiting<'a>(&'a Slot);
impl<'a> Waiting<'a> {
    fn new(slot: &'a Slot) -> Self {
        slot.connection.lock().unwrap().waiters += 1;
        Self(slot)
    }
}
impl Drop for Waiting<'_> {
    fn drop(&mut self) {
        let mut state = self.0.connection.lock().unwrap();
        state.waiters -= 1;
        if state.waiters > 0
            || state
                .startup
                .as_ref()
                .is_none_or(|s| s.task.as_ref().is_none_or(|task| task.is_finished()))
        {
            return;
        }
        // Fence the start's publication first: cancelling it is no failure.
        let failures = self.0.current_snapshot().failures;
        *self.0.snapshot.lock().unwrap() = Snapshot {
            startup_epoch: self.0.sequence.fetch_add(1, Ordering::AcqRel) + 1,
            failures,
            ..Snapshot::default()
        };
        state.startup = None;
        self.0.changes.send_modify(|v| *v = v.wrapping_add(1));
    }
}
#[derive(Default)]
struct Connection {
    control: Option<Arc<Control>>,
    retiring: Vec<Weak<Control>>,
    startup: Option<Startup>,
    /// Callers waiting for the runtime to start.
    waiters: usize,
}
impl Connection {
    fn drain_other_builds(&self, hash: &str) {
        for control in self.retiring.iter().filter_map(Weak::upgrade) {
            if control.hello.build_sha256 != hash {
                control.drain.send_replace(true);
            }
        }
    }
    fn find(&self, id: &str) -> Option<Arc<Control>> {
        self.control
            .iter()
            .cloned()
            .chain(self.retiring.iter().filter_map(Weak::upgrade))
            .find(|c| c.hello.instance_id == id && !c.is_closed())
    }
    fn prune(&mut self) {
        self.retiring
            .retain(|c| c.upgrade().is_some_and(|c| !c.is_closed()));
    }
}
impl Drop for Connection {
    fn drop(&mut self) {
        for control in self
            .control
            .iter()
            .cloned()
            .chain(self.retiring.iter().filter_map(Weak::upgrade))
        {
            control.stop();
        }
    }
}
pub struct Slot {
    connection: Arc<StdMutex<Connection>>,
    changes: watch::Sender<u64>,
    sequence: AtomicU64,
    generations: AtomicU64,
    snapshot: Arc<StdMutex<Snapshot>>,
    permissions: Arc<crate::integration_permissions::Gate>,
}
impl Slot {
    pub fn new(permissions: Arc<crate::integration_permissions::Gate>) -> Self {
        Self {
            connection: Arc::default(),
            changes: watch::channel(0).0,
            sequence: AtomicU64::new(0),
            generations: AtomicU64::new(0),
            snapshot: Arc::default(),
            permissions,
        }
    }
    #[cfg(test)]
    pub(crate) async fn stop_remote_for_test(&self) {
        let runtime = self.current_snapshot().hello.unwrap().instance_id;
        self.stop_runtime_for_test(&runtime).await;
    }
    #[cfg(test)]
    pub(crate) async fn stop_runtime_for_test(&self, runtime: &str) {
        self.request(runtime, protocol::Operation::Shutdown)
            .await
            .unwrap();
    }
    /// A feature EOF may precede the next heartbeat. Probe the existing control
    /// channel without creating a runtime or touching SSH recovery.
    pub async fn probe(&self) {
        let current = self.current_snapshot();
        if current.status == "ready"
            && let Some(hello) = current.hello
        {
            let _ = self
                .request(&hello.instance_id, protocol::Operation::Ping)
                .await;
        }
    }
    fn next_generation(&self) -> u64 {
        self.generations.fetch_add(1, Ordering::AcqRel) + 1
    }
    /// Which start of the runtime `instance` is, while it runs.
    pub(crate) fn generation_of(&self, instance: &str) -> Option<u64> {
        self.connection
            .lock()
            .unwrap()
            .find(instance)
            .map(|control| control.generation)
    }
    pub async fn wait_for_retry(&self) -> Result<(), String> {
        let snapshot = self.current_snapshot();
        if snapshot.status != "failed" {
            return Ok(());
        }
        if !snapshot.retryable {
            return Err(snapshot
                .error
                .unwrap_or_else(|| "Needle requires attention".into()));
        }
        if let Some(at) = snapshot.retry_at {
            tokio::time::sleep(at.saturating_duration_since(Instant::now())).await;
        }
        Ok(())
    }
    fn current_snapshot(&self) -> Snapshot {
        self.snapshot.lock().unwrap().clone()
    }
    pub fn snapshot(&self) -> Snapshot {
        let mut state = self.connection.lock().unwrap();
        state.prune();
        let mut snapshot = self.current_snapshot();
        snapshot.instances = state
            .control
            .iter()
            .map(|c| (c.clone(), false))
            .chain(
                state
                    .retiring
                    .iter()
                    .filter_map(Weak::upgrade)
                    .map(|c| (c, true)),
            )
            .map(|(c, retiring)| RuntimeInstance {
                instance_id: c.hello.instance_id.clone(),
                build_sha256: c.hello.build_sha256.clone(),
                generation: c.generation,
                retiring,
                draining: *c.drain.borrow(),
                connected: !c.is_closed(),
                registrations: c.registrations.load(Ordering::Acquire),
                worker_leases: c
                    .hello
                    .limits
                    .max_workers
                    .saturating_sub(c.workers.available_permits()),
            })
            .collect();
        snapshot
    }
    fn clear_current(&self, state: &mut Connection) {
        // Fence startup publication before aborting its owned task.
        *self.snapshot.lock().unwrap() = Snapshot {
            startup_epoch: self.sequence.fetch_add(1, Ordering::AcqRel) + 1,
            ..Snapshot::default()
        };
        state.startup = None;
        if let Some(control) = state.control.take() {
            control.stop();
        }
        self.changes.send_modify(|v| *v = v.wrapping_add(1));
    }
    fn clear(&self, state: &mut Connection) {
        self.clear_current(state);
        for control in state.retiring.drain(..).filter_map(|c| c.upgrade()) {
            control.stop();
        }
    }
    pub async fn reset(&self) {
        self.permissions.disconnected(None);
        self.clear(&mut self.connection.lock().unwrap());
    }
    /// Explicit retries leave both healthy controls and shared startup intact.
    pub async fn retry_failed(&self) {
        let mut state = self.connection.lock().unwrap();
        if self.current_snapshot().status == "failed"
            || (state.startup.is_none() && state.control.as_ref().is_none_or(|c| c.is_closed()))
        {
            self.clear_current(&mut state);
        }
    }
    /// Adopt the explicitly activated, verified build without interrupting
    /// admitted operations. Candidate startup leaves the old owner untouched.
    pub(crate) async fn adopt(
        &self,
        link: &Link,
        session: uuid::Uuid,
        hash: &str,
        validate: impl Fn() -> Result<(), Error>,
    ) -> Result<(), Error> {
        if !protocol::valid_build(hash) {
            return Err(Error::Unavailable("Invalid candidate hash".into()));
        }
        validate()?;
        {
            let state = self.connection.lock().unwrap();
            if state
                .control
                .as_ref()
                .is_some_and(|c| !c.is_closed() && c.hello.build_sha256 == hash)
            {
                state.drain_other_builds(hash);
                return Ok(());
            }
        }
        let command = format!(
            "exec \"$HOME/.local/share/newport/needle/builds/{hash}/newport-agent\" runtime --stdio"
        );
        let generation = self.next_generation();
        let (mut stream, fate, hello) = start_at(link, session, generation, &command)
            .await
            .map_err(|failure| failure.error)?;
        if hello.build_sha256 != hash {
            return Err(Error::Unavailable(
                "Needle upgrade candidate identity changed".into(),
            ));
        }
        exchange(&mut stream, &hello.instance_id, protocol::Operation::Ping)
            .await
            .and_then(accepted)
            .map_err(Error::Interrupted)?;
        let candidate = Control::run(
            stream,
            fate,
            hello,
            generation,
            self.snapshot.clone(),
            self.permissions.clone(),
        );
        self.publish_candidate(candidate, validate)
    }
    fn publish_candidate(
        &self,
        candidate: Control,
        validate: impl Fn() -> Result<(), Error>,
    ) -> Result<(), Error> {
        let mut state = self.connection.lock().unwrap();
        validate()?;
        state.prune();
        if state
            .control
            .as_ref()
            .is_some_and(|c| !c.is_closed() && c.hello.build_sha256 == candidate.hello.build_sha256)
        {
            state.drain_other_builds(&candidate.hello.build_sha256);
            return Ok(());
        }
        if state.retiring.len() >= MAX_RETIRING
            && state
                .control
                .as_ref()
                .is_some_and(|c| Arc::strong_count(c) > 1)
        {
            return Err(Error::Interrupted(BUSY.into()));
        }
        if candidate.is_closed() {
            return Err(Error::Interrupted(
                "Needle upgrade candidate disconnected".into(),
            ));
        }
        // Fence any old owned startup before aborting it. The new control is
        // published before consumers observe their handoff notification.
        let epoch = self.sequence.fetch_add(1, Ordering::AcqRel) + 1;
        state.startup = None;
        if let Some(previous) = state.control.take() {
            state.retiring.push(Arc::downgrade(&previous));
            previous.drain.send_replace(true);
        }
        for control in state.retiring.iter().filter_map(Weak::upgrade) {
            control.drain.send_replace(true);
        }
        *self.snapshot.lock().unwrap() = Snapshot {
            status: "ready",
            generation: candidate.generation,
            hello: Some(candidate.hello.clone()),
            startup_epoch: epoch,
            ..Snapshot::default()
        };
        state.control = Some(Arc::new(candidate));
        self.changes.send_modify(|v| *v = v.wrapping_add(1));
        Ok(())
    }
    pub(crate) async fn worker(
        &self,
        link: &Link,
        session: uuid::Uuid,
        service: &str,
        version: u32,
        validate: impl Fn() -> Result<(), Error>,
    ) -> Result<Worker, Error> {
        loop {
            let hello = self.ensure(link, session, &validate).await?;
            // Needle itself checks for Git installed since it started.
            if service == "git" {
                hello.require_compatible(service, version).map(|_| ())
            } else {
                hello.require(service, version)
            }
            .map_err(|e| Error::Unavailable(e.to_string()))?;
            let mut worker = {
                let mut state = self.connection.lock().unwrap();
                validate()?;
                state.prune();
                let Some(control) = state
                    .control
                    .clone()
                    .filter(|c| c.hello.instance_id == hello.instance_id && !c.is_closed())
                else {
                    continue;
                };
                if control.registrations.load(Ordering::Acquire) >= hello.limits.max_registrations {
                    // Bound overlap without killing admitted work. The current
                    // slot and this local reference account for two strong refs.
                    if state.retiring.len() >= MAX_RETIRING && Arc::strong_count(&control) > 2 {
                        return Err(Error::Interrupted(BUSY.into()));
                    }
                    state.control = None;
                    state.retiring.push(Arc::downgrade(&control));
                    *self.snapshot.lock().unwrap() = Snapshot {
                        startup_epoch: self.sequence.fetch_add(1, Ordering::AcqRel) + 1,
                        ..Snapshot::default()
                    };
                    continue;
                }
                let permit = control
                    .workers
                    .clone()
                    .try_acquire_owned()
                    .map_err(|_| Error::Interrupted(BUSY.into()))?;
                // Count before sending: cancelled or lost registration replies
                // may still have consumed a one-use ticket on the server.
                control.registrations.fetch_add(1, Ordering::AcqRel);
                Worker {
                    runtime: Handle(control),
                    id: uuid::Uuid::new_v4(),
                    endpoint: None,
                    _permit: permit,
                }
            };
            worker.endpoint = self
                .prepare(worker.runtime.hello(), worker.id, service, version)
                .await?;
            return Ok(worker);
        }
    }
    #[cfg(test)]
    pub(crate) fn exhaust_registration_budget_for_test(&self) {
        let state = self.connection.lock().unwrap();
        let control = state.control.as_ref().unwrap();
        control
            .registrations
            .store(control.hello.limits.max_registrations, Ordering::Release);
    }
    #[cfg(test)]
    pub(crate) fn retiring_for_test(&self) -> usize {
        self.connection
            .lock()
            .unwrap()
            .retiring
            .iter()
            .filter(|c| c.strong_count() > 0)
            .count()
    }
    async fn prepare(
        &self,
        hello: &protocol::Hello,
        worker: uuid::Uuid,
        service: &str,
        version: u32,
    ) -> Result<Option<protocol::worker_stream::Endpoint>, Error> {
        let unavailable = |message: &str| Error::Unavailable(message.into());
        if hello
            .require(
                protocol::worker_stream::CAPABILITY,
                protocol::worker_stream::VERSION,
            )
            .is_ok()
        {
            let reply = self
                .request(
                    &hello.instance_id,
                    protocol::Operation::PrepareWorkerStream {
                        worker_id: worker.to_string(),
                        service: service.into(),
                        version,
                    },
                )
                .await?;
            let endpoint = reply
                .worker_stream
                .ok_or_else(|| unavailable("Runtime omitted its worker stream endpoint"))?;
            endpoint
                .validate()
                .map_err(|_| unavailable("Invalid worker stream endpoint"))?;
            if std::path::Path::new(&endpoint.path).parent()
                != std::path::Path::new(&hello.worker_socket).parent()
                || endpoint.path == hello.worker_socket
            {
                return Err(unavailable(
                    "Worker stream endpoint belongs to a different runtime",
                ));
            }
            Ok(Some(endpoint))
        } else {
            self.request(
                &hello.instance_id,
                protocol::Operation::Register {
                    worker_id: worker.to_string(),
                    service: service.into(),
                    version,
                },
            )
            .await?;
            Ok(None)
        }
    }
    pub async fn permissions(
        &self,
        runtime: &str,
        worker: uuid::Uuid,
        epoch: u64,
        permissions: protocol::integration::Permissions,
    ) -> Result<protocol::integration::Applied, Error> {
        let control = self
            .connection
            .lock()
            .unwrap()
            .find(runtime)
            .ok_or_else(|| Error::Interrupted("Needle runtime is not connected".into()))?;
        control.permission_epoch.fetch_max(epoch, Ordering::AcqRel);
        self.request(
            runtime,
            protocol::Operation::Permissions {
                worker_id: worker.to_string(),
                permissions,
            },
        )
        .await?
        .applied
        .ok_or_else(|| Error::Unavailable("Needle did not acknowledge applied permissions".into()))
    }
    /// Only the owner of an admitted graphical worker calls this. Revisions
    /// belong to that exact runtime, including an old build still draining.
    pub(crate) async fn clipboard_route(
        &self,
        identity: &crate::graphical_session::Identity,
        exports: bool,
        focused: bool,
    ) -> Result<u64, String> {
        match self
            .clipboard_route_outcome(identity, exports, focused)
            .await?
        {
            ClipboardRoute::Applied(revision) => Ok(revision),
            ClipboardRoute::Retired => {
                Err("Graphical clipboard provider retired during display preparation".into())
            }
        }
    }
    pub(crate) async fn clipboard_route_outcome(
        &self,
        identity: &crate::graphical_session::Identity,
        exports: bool,
        focused: bool,
    ) -> Result<ClipboardRoute, String> {
        let control = self
            .connection
            .lock()
            .unwrap()
            .find(&identity.runtime.instance.to_string())
            .ok_or("Clipboard runtime is not connected")?;
        if control.generation != identity.runtime.transport_generation
            || control.hello.build_sha256 != identity.runtime.build_sha256
            || identity.worker.is_nil()
        {
            return Err("Clipboard display belongs to a different runtime generation".into());
        }
        control
            .hello
            .require(protocol::clipboard::SERVICE_CAPABILITY, 1)
            .map_err(|e| e.to_string())?;
        let typed = control
            .hello
            .require(
                protocol::clipboard::SERVICE_CAPABILITY,
                protocol::clipboard::ROUTE_RESULT_VERSION,
            )
            .is_ok();
        let (revision, response) = {
            // Allocate and enqueue together. Concurrent focus/permission/setup
            // operations cannot put a lower revision behind a higher one.
            let mut sequence = control.clipboard_revision.lock().unwrap();
            let revision = sequence
                .checked_add(1)
                .ok_or("Clipboard route revisions exhausted")?;
            let route = protocol::clipboard::Route {
                revision,
                worker_id: Some(identity.worker.to_string()),
                exports,
                focused: exports && focused,
            };
            let response = control.enqueue(if typed {
                protocol::Operation::ClipboardRouteV2 { route }
            } else {
                protocol::Operation::ClipboardRoute { route }
            })?;
            *sequence = revision;
            (revision, response)
        };
        let reply = control_response(response).await?;
        if control.is_closed() {
            return Err("Clipboard runtime ended during authorization".into());
        }
        if reply.runtime_id != identity.runtime.instance.to_string()
            || reply.applied.is_some()
            || reply.worker_stream.is_some()
        {
            return Err("Invalid clipboard route acknowledgment".into());
        }
        match (typed, reply.clipboard_route) {
            (false, None) => Ok(ClipboardRoute::Applied(revision)),
            (true, Some(reply))
                if reply.revision == revision
                    && reply.worker_id.as_deref() == Some(identity.worker.to_string().as_str()) =>
            {
                Ok(match reply.outcome {
                    protocol::clipboard::RouteOutcome::Applied => ClipboardRoute::Applied(revision),
                    protocol::clipboard::RouteOutcome::Retired => ClipboardRoute::Retired,
                })
            }
            _ => Err("Invalid clipboard route acknowledgment".into()),
        }
    }
    async fn request(
        &self,
        runtime: &str,
        operation: protocol::Operation,
    ) -> Result<protocol::Reply, Error> {
        let control = {
            let state = self.connection.lock().unwrap();
            state.find(runtime).ok_or_else(|| {
                Error::Interrupted("Needle runtime changed before worker registration".into())
            })?
        };
        control_response(control.enqueue(operation)?).await
    }
    /// The running runtime's hello, starting the runtime when none runs.
    /// Callers share one start; a temporary failure is retried after a
    /// growing pause while anyone waits.
    pub async fn ensure(
        &self,
        link: &Link,
        session_id: uuid::Uuid,
        validate: impl Fn() -> Result<(), Error>,
    ) -> Result<protocol::Hello, Error> {
        self.ensure_with(Some(link.status()), validate, |generation| {
            let link = link.clone();
            let snapshot = self.snapshot.clone();
            let permissions = self.permissions.clone();
            async move {
                let (stream, fate, hello) = start(&link, session_id, generation).await?;
                Ok(Control::run(
                    stream,
                    fate,
                    hello,
                    generation,
                    snapshot,
                    permissions,
                ))
            }
        })
        .await
    }
    /// Starts the runtime with `create`, or waits for the start in progress.
    /// A failure from losing the connection doesn't hold the next start once
    /// `link` is ready again.
    async fn ensure_with<F, Fut>(
        &self,
        mut link: Option<watch::Receiver<newport_ssh::Status>>,
        validate: impl Fn() -> Result<(), Error>,
        create: F,
    ) -> Result<protocol::Hello, Error>
    where
        F: Fn(u64) -> Fut,
        Fut: std::future::Future<Output = Result<Control, StartupFailure>> + Send + 'static,
    {
        let mut changes = self.changes.subscribe();
        let _waiting = Waiting::new(self);
        let ready =
            |status: &newport_ssh::Status| matches!(status.reach, newport_ssh::Reach::Ready { .. });
        loop {
            changes.borrow_and_update();
            let mut retry_at = None;
            let mut lost = false;
            {
                let mut state = self.connection.lock().unwrap();
                validate()?;
                if let Some(control) = &state.control
                    && !control.is_closed()
                    && self.current_snapshot().status == "ready"
                {
                    return Ok(control.hello.clone());
                }
                state.control = None;
                let mut previous = self.current_snapshot();
                if previous.status == "failed" {
                    if !previous.retryable {
                        return Err(previous.cause.unwrap_or_else(|| {
                            Error::Unavailable(
                                previous
                                    .error
                                    .unwrap_or_else(|| "Needle runtime unavailable".into()),
                            )
                        }));
                    }
                    lost = previous.lost;
                    if lost && link.as_ref().is_some_and(|link| ready(&link.borrow())) {
                        previous.failures = 0;
                    } else {
                        retry_at = previous.retry_at.filter(|at| *at > Instant::now());
                    }
                }
                if state
                    .startup
                    .as_ref()
                    .is_some_and(|s| s.task.as_ref().is_none_or(|task| task.is_finished()))
                {
                    state.startup = None;
                }
                if state.startup.is_none() && retry_at.is_none() {
                    let epoch = self.sequence.fetch_add(1, Ordering::AcqRel) + 1;
                    let generation = self.next_generation();
                    *self.snapshot.lock().unwrap() = Snapshot {
                        status: "starting",
                        generation,
                        startup_epoch: epoch,
                        failures: previous.failures,
                        ..Snapshot::default()
                    };
                    let owner = Arc::downgrade(&self.connection);
                    let snapshot = self.snapshot.clone();
                    let guard = Starting {
                        snapshot: snapshot.clone(),
                        generation,
                        epoch,
                        changes: self.changes.clone(),
                    };
                    let start = create(generation);
                    let task = tokio::spawn(async move {
                        let _guard = guard;
                        let result = start.await;
                        let Some(owner) = owner.upgrade() else { return };
                        let mut state = owner.lock().unwrap();
                        if !state
                            .startup
                            .as_ref()
                            .is_some_and(|s| s.epoch == epoch && s.generation == generation)
                        {
                            return;
                        }
                        // This is the finishing task; dropping its handle here is safe.
                        state.startup.as_mut().unwrap().task.take();
                        state.startup = None;
                        let mut snapshot = snapshot.lock().unwrap();
                        match result {
                            Ok(control) => {
                                snapshot.status = "ready";
                                snapshot.hello = Some(control.hello.clone());
                                snapshot.error = None;
                                snapshot.cause = None;
                                snapshot.retryable = false;
                                snapshot.retry_at = None;
                                state.control = Some(Arc::new(control));
                            }
                            Err(failure) => {
                                snapshot.hello = None;
                                record_failure(
                                    &mut snapshot,
                                    failure.error,
                                    failure.retryable,
                                    failure.lost,
                                );
                            }
                        }
                    });
                    state.startup = Some(Startup {
                        generation,
                        epoch,
                        task: Some(task),
                    });
                }
            }
            // All feature callers share the same retry deadline and startup.
            // Never hold a lock while backing off; reset/retirement wakes us
            // early, and so does the link's return after a lost connection.
            if let Some(at) = retry_at {
                let reconnected = async {
                    match link.as_mut().filter(|_| lost) {
                        Some(link) => {
                            if link.wait_for(ready).await.is_err() {
                                std::future::pending::<()>().await;
                            }
                        }
                        None => std::future::pending().await,
                    }
                };
                tokio::select! {
                    _ = tokio::time::sleep_until(at) => {},
                    () = reconnected => {},
                    result = changes.changed() => {
                        result.map_err(|_| closed())?;
                    }
                }
            } else {
                changes.changed().await.map_err(|_| closed())?;
            }
        }
    }
}
fn closed() -> Error {
    Error::Interrupted("Needle session closed".into())
}
impl Control {
    fn enqueue(
        &self,
        operation: protocol::Operation,
    ) -> Result<oneshot::Receiver<Result<protocol::Reply, String>>, Error> {
        let disconnected = || Error::Interrupted("Needle runtime disconnected".into());
        if self.is_closed() {
            return Err(disconnected());
        }
        let (reply, response) = oneshot::channel();
        self.commands
            .try_send(Call { operation, reply })
            .map_err(|error| match error {
                mpsc::error::TrySendError::Full(_) => Error::Interrupted(BUSY.into()),
                mpsc::error::TrySendError::Closed(_) => disconnected(),
            })?;
        Ok(response)
    }
    /// Serves the runtime's control channel until it fails, or the
    /// connection under it is lost.
    fn run(
        mut stream: Stream,
        fate: Fate,
        hello: protocol::Hello,
        generation: u64,
        snapshot: Arc<StdMutex<Snapshot>>,
        permissions: Arc<crate::integration_permissions::Gate>,
    ) -> Self {
        let runtime_id = hello.instance_id.clone();
        let started = Instant::now();
        let failed = Arc::new(AtomicBool::new(false));
        let failure_state = failed.clone();
        let permission_epoch = Arc::new(AtomicU64::new(0));
        let bound_epoch = permission_epoch.clone();
        let (commands, mut calls) = mpsc::channel::<Call>(16);
        let heartbeat = tokio::spawn(async move {
            let fail = |failure: (String, bool)| {
                if !failure_state.swap(true, Ordering::AcqRel) {
                    permissions.disconnect_if(bound_epoch.load(Ordering::Acquire));
                    record_control_failure(&snapshot, generation, &runtime_id, failure, started);
                }
            };
            let mut timer = tokio::time::interval_at(
                tokio::time::Instant::now() + Duration::from_secs(15),
                Duration::from_secs(15),
            );
            timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            let failure = loop {
                tokio::select! {
                    biased;
                    lost = fate.lost() => break (format!("Needle runtime disconnected: {lost}"), true),
                    _ = timer.tick() => {
                        if let Err(error) = exchange(&mut stream, &runtime_id, protocol::Operation::Ping).await.and_then(accepted) { break (error, false); }
                    }
                    call = calls.recv() => {
                        let Some(call) = call else { break ("Needle control closed".into(), false) };
                        let result = exchange(&mut stream, &runtime_id, call.operation).await;
                        let failure = result.as_ref().err().cloned();
                        if let Some(error) = &failure {
                            fail((error.clone(), false));
                        }
                        let _ = call.reply.send(result);
                        if let Some(error) = failure { break (error, false); }
                    }
                }
            };
            fail(failure);
        });
        Control {
            commands,
            generation,
            workers: Arc::new(Semaphore::new(hello.limits.max_workers)),
            hello,
            heartbeat,
            failed,
            permission_epoch,
            clipboard_revision: StdMutex::new(0),
            registrations: AtomicUsize::new(0),
            drain: watch::channel(false).0,
        }
    }
}

async fn control_response(
    response: oneshot::Receiver<Result<protocol::Reply, String>>,
) -> Result<protocol::Reply, Error> {
    let interrupted = |message: &str| Error::Interrupted(message.into());
    let reply = tokio::time::timeout(Duration::from_secs(10), response)
        .await
        .map_err(|_| interrupted("Needle control request timed out"))?
        .map_err(|_| interrupted("Needle runtime disconnected during control request"))?
        .map_err(Error::Interrupted)?;
    if let Some(error) = &reply.error {
        return Err(if error == "Needle worker registration limit reached" {
            interrupted(BUSY)
        } else {
            Error::Unavailable(error.clone())
        });
    }
    Ok(reply)
}

fn record_failure(snapshot: &mut Snapshot, error: Error, retryable: bool, lost: bool) {
    snapshot.status = "failed";
    snapshot.error = Some(error.to_string());
    snapshot.cause = Some(error);
    snapshot.retryable = retryable;
    snapshot.lost = lost;
    snapshot.failures = snapshot.failures.saturating_add(1);
    snapshot.retry_at = retryable.then(|| {
        Instant::now() + backoff(snapshot.failures, uuid::Uuid::new_v4().as_u128() as u64)
    });
}
/// How long to wait before the `attempt`th retry: doubling from 2 s to 30 s,
/// with jitter from `random`.
fn backoff(attempt: u32, random: u64) -> Duration {
    let base = 2_000u64.saturating_mul(1u64 << attempt.saturating_sub(1).min(4));
    // 80–120% jitter, bounded even at the maximum backoff.
    Duration::from_millis((base * (800 + random % 401) / 1000).min(30_000))
}
fn record_control_failure(
    snapshot: &StdMutex<Snapshot>,
    generation: u64,
    runtime: &str,
    (error, lost): (String, bool),
    started: Instant,
) {
    let mut current = snapshot.lock().unwrap();
    if current.status == "ready"
        && current.generation == generation
        && current
            .hello
            .as_ref()
            .is_some_and(|h| h.instance_id == runtime)
    {
        if started.elapsed() >= Duration::from_secs(60) {
            current.failures = 0;
        }
        record_failure(&mut current, Error::Interrupted(error), true, lost);
    }
}
#[derive(Debug)]
struct StartupFailure {
    error: Error,
    retryable: bool,
    /// The connection was lost under the start.
    lost: bool,
}
impl StartupFailure {
    fn temporary(message: impl Into<String>) -> Self {
        Self {
            error: Error::Interrupted(message.into()),
            retryable: true,
            lost: false,
        }
    }
    fn permanent(message: impl Into<String>) -> Self {
        Self {
            error: Error::Unavailable(message.into()),
            retryable: false,
            lost: false,
        }
    }
    /// The link recovers by itself unless it was closed or the server takes
    /// no sessions at all.
    fn link(error: newport_ssh::Error) -> Self {
        Self {
            retryable: !matches!(
                error,
                newport_ssh::Error::SessionsDisabled | newport_ssh::Error::Closed
            ),
            lost: matches!(error, newport_ssh::Error::Lost(_)),
            error: Error::Link(error),
        }
    }
    fn command(error: command::Error) -> Self {
        match error {
            command::Error::Link(error) => Self::link(error),
            other => Self::temporary(other.to_string()),
        }
    }
}
type Stream = russh::ChannelStream<russh::client::Msg>;
async fn start(
    link: &Link,
    session_id: uuid::Uuid,
    generation: u64,
) -> Result<(Stream, Fate, protocol::Hello), StartupFailure> {
    if !crate::agent::is_installed(link)
        .await
        .map_err(StartupFailure::command)?
    {
        return Err(StartupFailure {
            error: Error::NotInstalled,
            retryable: false,
            lost: false,
        });
    }
    start_at(link, session_id, generation, &runtime_command()).await
}
/// Starts this Newport's own build where it's on the server, so one desktop's
/// sessions never depend on which build another desktop made the entry
/// point; otherwise the entry point's.
fn runtime_command() -> String {
    let build = |architecture| {
        crate::agent_payload::payload(architecture).map_or("", |payload| payload.sha256)
    };
    format!(
        "case \"$(uname -m)\" in x86_64) b={} ;; aarch64|arm64) b={} ;; *) b= ;; esac; \
        own=\"$HOME/.local/share/newport/needle/builds/$b/newport-agent\"; \
        if test -n \"$b\" && test -x \"$own\"; then exec \"$own\" runtime --stdio; fi; \
        exec \"$HOME/.local/bin/newport-agent\" runtime --stdio",
        build("x86_64"),
        build("aarch64"),
    )
}
/// Starts a runtime on a session channel of its own and says hello. The
/// handshake's deadline starts once the channel is open: opening may wait
/// for a login.
async fn start_at(
    link: &Link,
    session_id: uuid::Uuid,
    generation: u64,
    command: &str,
) -> Result<(Stream, Fate, protocol::Hello), StartupFailure> {
    let channel = command::stream(link, command)
        .await
        .map_err(StartupFailure::command)?;
    let fate = channel.fate();
    let mut stream = channel.into_stream();
    let handshake = async {
        let hello: protocol::Hello = read(&mut stream).await?;
        hello
            .validate()
            .map_err(|e| StartupFailure::permanent(e.to_string()))?;
        exchange(
            &mut stream,
            &hello.instance_id,
            protocol::Operation::Initialize {
                version: protocol::VERSION,
                session_id: session_id.to_string(),
                transport_generation: generation,
            },
        )
        .await
        .and_then(accepted)
        .map_err(StartupFailure::temporary)?;
        Ok(hello)
    };
    let hello = tokio::time::timeout(HANDSHAKE, handshake)
        .await
        .map_err(|_| StartupFailure::temporary("Needle runtime handshake timed out."))
        .and_then(|result| result)
        // A handshake cut off by the connection's loss is the link's to
        // report.
        .map_err(|failure| fate.error().map_or(failure, StartupFailure::link))?;
    Ok((stream, fate, hello))
}
/// Verify a staged build using the desktop's actual control protocol before
/// activation. This temporary runtime never replaces the session's live slot.
pub(crate) async fn verify_candidate(
    link: &Link,
    hash: &str,
) -> Result<newport_release::agent_product::Product, String> {
    verify_product_at(
        link,
        hash,
        &format!(
            "exec \"$HOME/.local/share/newport/needle/builds/{hash}/newport-agent\" runtime --stdio"
        ),
    )
    .await
}

/// The installed build's product. A build is identified by its hash, so
/// each is started once to ask: every start shows the build as in use,
/// which would hold back its replacement and cost a runtime per check.
pub(crate) async fn inspect_product(
    link: &Link,
    hash: &str,
) -> Result<newport_release::agent_product::Product, String> {
    once_per_build(hash, || {
        verify_product_at(
            link,
            hash,
            "exec \"$HOME/.local/bin/newport-agent\" runtime --stdio",
        )
    })
    .await
}

async fn once_per_build<F, Fut>(
    hash: &str,
    inspect: F,
) -> Result<newport_release::agent_product::Product, String>
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = Result<newport_release::agent_product::Product, String>>,
{
    static KNOWN: std::sync::LazyLock<
        std::sync::Mutex<
            std::collections::HashMap<String, newport_release::agent_product::Product>,
        >,
    > = std::sync::LazyLock::new(Default::default);
    if let Some(product) = KNOWN.lock().unwrap().get(hash) {
        return Ok(product.clone());
    }
    let product = inspect().await?;
    KNOWN
        .lock()
        .unwrap()
        .insert(hash.to_owned(), product.clone());
    Ok(product)
}

async fn verify_product_at(
    link: &Link,
    hash: &str,
    command: &str,
) -> Result<newport_release::agent_product::Product, String> {
    if !protocol::valid_build(hash) {
        return Err("Invalid candidate hash".into());
    }
    tokio::time::timeout(Duration::from_secs(15), async {
        let (mut stream, _, hello) = start_at(link, uuid::Uuid::new_v4(), 1, command)
            .await
            .map_err(|failure| failure.error.to_string())?;
        if hello.build_sha256 != hash {
            return Err("Candidate runtime has an unexpected executable identity".into());
        }
        let product = newport_release::agent_product::Product::from_capabilities(
            &hello.versions,
            hello.capabilities.iter().map(|(name, capability)| {
                (
                    name.as_str(),
                    capability.versions.as_slice(),
                    capability.available,
                )
            }),
        )?;
        // An absent optional dependency may disable its capability, but every
        // advertised service must speak the version supported by this desktop.
        for (name, version) in [
            ("monitor", 1),
            ("git", crate::git::protocol::VERSION),
            ("integration", protocol::integration::VERSION),
        ] {
            if hello
                .capabilities
                .get(name)
                .is_none_or(|c| !c.versions.contains(&version))
            {
                return Err(format!("Candidate {name} protocol is incompatible"));
            }
        }
        exchange(&mut stream, &hello.instance_id, protocol::Operation::Ping)
            .await
            .and_then(accepted)?;
        stop_verifier(&mut stream, &hello.instance_id).await?;
        Ok(product)
    })
    .await
    .map_err(|_| "Candidate runtime verification timed out".to_owned())?
}

async fn stop_verifier(
    stream: &mut (impl AsyncRead + AsyncWrite + Unpin),
    runtime: &str,
) -> Result<(), String> {
    exchange(stream, runtime, protocol::Operation::Shutdown)
        .await
        .and_then(accepted)?;
    // The reply precedes process cleanup. An immediate installation check must
    // not mistake our own verifier for another desktop's busy runtime.
    let mut trailing = [0; 1];
    match tokio::time::timeout(Duration::from_secs(5), stream.read(&mut trailing)).await {
        Ok(Ok(0)) => Ok(()),
        Ok(Ok(_)) => Err("Candidate runtime sent data after shutdown".into()),
        Ok(Err(error)) => Err(format!("Candidate runtime shutdown failed: {error}")),
        Err(_) => Err("Candidate runtime did not exit after shutdown".into()),
    }
}
async fn read<T: serde::de::DeserializeOwned>(
    input: &mut (impl AsyncRead + Unpin),
) -> Result<T, StartupFailure> {
    let interrupted = |error: std::io::Error| {
        StartupFailure::temporary(format!("Needle connection interrupted: {error}"))
    };
    let length = input.read_u32().await.map_err(interrupted)? as usize;
    if length == 0 || length > protocol::MAX_FRAME {
        return Err(StartupFailure::permanent(
            "Runtime control frame exceeds limit",
        ));
    }
    let mut bytes = vec![0; length];
    input.read_exact(&mut bytes).await.map_err(interrupted)?;
    protocol::decode(&bytes).map_err(|error| {
        StartupFailure::permanent(format!(
            "Needle runtime handshake failed. Update or repair Needle: {error}"
        ))
    })
}
async fn exchange(
    stream: &mut (impl AsyncRead + AsyncWrite + Unpin),
    runtime_id: &str,
    operation: protocol::Operation,
) -> Result<protocol::Reply, String> {
    let request = protocol::Request {
        id: uuid::Uuid::new_v4().to_string(),
        runtime_id: runtime_id.into(),
        operation,
    };
    let bytes = protocol::encode(&request).map_err(|e| e.to_string())?;
    tokio::time::timeout(Duration::from_secs(5), async {
        stream
            .write_u32(bytes.len() as u32)
            .await
            .map_err(|e| e.to_string())?;
        stream.write_all(&bytes).await.map_err(|e| e.to_string())?;
        stream.flush().await.map_err(|e| e.to_string())?;
        let reply: protocol::Reply = read(stream)
            .await
            .map_err(|failure| failure.error.to_string())?;
        if reply.id != request.id || reply.runtime_id != runtime_id {
            return Err("Obsolete or mismatched runtime response".into());
        }
        Ok(reply)
    })
    .await
    .map_err(|_| "Needle runtime control timed out".to_owned())?
}

fn accepted(reply: protocol::Reply) -> Result<(), String> {
    match reply.error {
        Some(error) => Err(error),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retries_back_off_within_bounds() {
        assert!(backoff(1, 0) >= Duration::from_millis(1600));
        for attempt in 1..=40 {
            assert!(backoff(attempt, 400) <= Duration::from_secs(30));
            assert!(backoff(attempt + 1, 0) >= backoff(attempt, 0));
        }
    }

    #[tokio::test]
    async fn each_build_is_started_once_to_learn_its_product() {
        let product: newport_release::agent_product::Product = serde_json::from_value(
            serde_json::json!({"variant": "full", "controlVersions": [1], "services": {}}),
        )
        .unwrap();
        let hash = format!("{:064x}", rand_hash());
        let starts = std::sync::atomic::AtomicUsize::new(0);
        let inspect = || async {
            starts.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(product.clone())
        };
        assert_eq!(
            super::once_per_build(&hash, inspect).await.unwrap(),
            product
        );
        assert_eq!(
            super::once_per_build(&hash, inspect).await.unwrap(),
            product
        );
        assert_eq!(starts.load(std::sync::atomic::Ordering::SeqCst), 1);
        // A failed start is asked again.
        let other = format!("{:064x}", rand_hash());
        assert!(
            super::once_per_build(&other, || async { Err("offline".into()) })
                .await
                .is_err()
        );
        assert!(super::once_per_build(&other, inspect).await.is_ok());
        assert_eq!(starts.load(std::sync::atomic::Ordering::SeqCst), 2);
    }

    fn rand_hash() -> u128 {
        uuid::Uuid::new_v4().as_u128()
    }

    #[tokio::test(start_paused = true)]
    async fn verifier_shutdown_waits_for_eof_and_rejects_trailing_data_or_a_hung_exit() {
        use std::future::Future;
        for outcome in ["closed", "trailing", "hung"] {
            let (mut client, mut peer) = tokio::io::duplex(4096);
            let runtime = uuid::Uuid::new_v4().to_string();
            let (acknowledged, ack) = oneshot::channel();
            let (finish, exit) = oneshot::channel();
            let task = tokio::spawn(async move {
                let request: protocol::Request = read(&mut peer).await.unwrap();
                assert!(matches!(request.operation, protocol::Operation::Shutdown));
                let reply = protocol::Reply {
                    clipboard_route: None,
                    worker_stream: None,
                    applied: None,
                    id: request.id,
                    runtime_id: request.runtime_id,
                    error: None,
                };
                let bytes = protocol::encode(&reply).unwrap();
                peer.write_u32(bytes.len() as u32).await.unwrap();
                peer.write_all(&bytes).await.unwrap();
                acknowledged.send(()).unwrap();
                let _ = exit.await;
                if outcome == "trailing" {
                    peer.write_all(b"unexpected").await.unwrap();
                }
            });
            let stopped = stop_verifier(&mut client, &runtime);
            tokio::pin!(stopped);
            tokio::select! {
                biased;
                result = &mut stopped => panic!("shutdown returned before acknowledgement: {result:?}"),
                _ = ack => {},
            }
            assert!(
                std::future::poll_fn(|cx| std::task::Poll::Ready(
                    stopped.as_mut().poll(cx).is_pending()
                ))
                .await
            );
            if outcome == "hung" {
                assert_eq!(
                    stopped.await.unwrap_err(),
                    "Candidate runtime did not exit after shutdown"
                );
                finish.send(()).unwrap();
            } else {
                finish.send(()).unwrap();
                let result = stopped.await;
                if outcome == "closed" {
                    result.unwrap();
                } else {
                    assert_eq!(
                        result.unwrap_err(),
                        "Candidate runtime sent data after shutdown"
                    );
                }
            }
            task.await.unwrap();
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_runtime_starts_this_newports_own_build_where_it_is_staged() {
        use std::os::unix::fs::PermissionsExt;
        let home = tempfile::tempdir().unwrap();
        let script = |path: std::path::PathBuf, name: &str| {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, format!("#!/bin/sh\necho {name} \"$@\"\n")).unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
        };
        let run = || {
            let output = std::process::Command::new("sh")
                .arg("-c")
                .arg(runtime_command())
                .env("HOME", home.path())
                .output()
                .unwrap();
            String::from_utf8(output.stdout).unwrap()
        };
        script(home.path().join(".local/bin/newport-agent"), "entry");
        assert_eq!(run(), "entry runtime --stdio\n");
        let machine = std::process::Command::new("uname")
            .arg("-m")
            .output()
            .unwrap()
            .stdout;
        let own = crate::agent_payload::payload(String::from_utf8_lossy(&machine).trim())
            .unwrap()
            .sha256;
        script(
            home.path()
                .join(".local/share/newport/needle/builds")
                .join(own)
                .join("newport-agent"),
            "own",
        );
        assert_eq!(run(), "own runtime --stdio\n");
    }

    fn fake_control(generation: u64) -> Control {
        let hello = protocol::Hello {
            protocol: protocol::PROTOCOL.into(),
            versions: vec![protocol::VERSION],
            instance_id: uuid::Uuid::new_v4().to_string(),
            build_sha256: "0".repeat(64),
            worker_socket: "/tmp/unused-test-runtime.sock".into(),
            capabilities: Default::default(),
            dependencies: Default::default(),
            limits: protocol::Limits {
                max_control_bytes: protocol::MAX_FRAME,
                max_inflight: 1,
                max_workers: protocol::MAX_WORKERS,
                max_registrations: protocol::MAX_REGISTRATIONS,
            },
        };
        let (commands, receiver) = mpsc::channel(1);
        Control {
            commands,
            generation,
            hello,
            failed: Arc::new(AtomicBool::new(false)),
            permission_epoch: Arc::new(AtomicU64::new(0)),
            clipboard_revision: StdMutex::new(0),
            registrations: AtomicUsize::new(0),
            drain: watch::channel(false).0,
            workers: Arc::new(Semaphore::new(protocol::MAX_WORKERS)),
            heartbeat: tokio::spawn(async move {
                let _receiver = receiver;
                std::future::pending::<()>().await;
            }),
        }
    }
    fn test_slot() -> Arc<Slot> {
        Arc::new(Slot::new(Arc::new(
            crate::integration_permissions::Gate::new(false, false),
        )))
    }
    mod clipboard_routes {
        include!("agent_runtime_clipboard_tests.rs");
    }

    fn clipboard_control(
        slot: &Slot,
    ) -> (
        Arc<Control>,
        mpsc::Receiver<Call>,
        crate::graphical_session::Identity,
    ) {
        clipboard_control_version(slot, false)
    }
    fn clipboard_control_version(
        slot: &Slot,
        typed: bool,
    ) -> (
        Arc<Control>,
        mpsc::Receiver<Call>,
        crate::graphical_session::Identity,
    ) {
        let mut control = fake_control(7);
        control.hello.capabilities.insert(
            "clipboard-service".into(),
            protocol::Capability {
                versions: if typed { vec![1, 2] } else { vec![1] },
                available: true,
                reason: None,
            },
        );
        let (commands, calls) = mpsc::channel(16);
        control.commands = commands;
        let identity = crate::graphical_session::Identity {
            runtime: crate::graphical_session::Runtime {
                instance: control.hello.instance_id.parse().unwrap(),
                build_sha256: control.hello.build_sha256.clone(),
                transport_generation: 7,
            },
            session: uuid::Uuid::new_v4(),
            worker: uuid::Uuid::new_v4(),
            display_generation: 1,
        };
        let control = Arc::new(control);
        let mut connection = slot.connection.lock().unwrap();
        if let Some(old) = connection.control.replace(control.clone()) {
            connection.retiring.push(Arc::downgrade(&old));
        }
        (control, calls, identity)
    }
    fn route_reply(
        call: Call,
        identity: &crate::graphical_session::Identity,
        error: Option<String>,
    ) -> u64 {
        let protocol::Operation::ClipboardRoute { route } = call.operation else {
            panic!("not a clipboard route")
        };
        assert_eq!(route.worker_id, Some(identity.worker.to_string()));
        assert!(!route.focused || route.exports);
        let _ = call.reply.send(Ok(protocol::Reply {
            clipboard_route: None,
            id: "fixture".into(),
            runtime_id: identity.runtime.instance.to_string(),
            error,
            applied: None,
            worker_stream: None,
        }));
        route.revision
    }
    #[tokio::test]
    async fn clipboard_routes_are_enqueued_monotonically_and_bound_to_runtime_identity() {
        let slot = test_slot();
        let (_control, mut calls, identity) = clipboard_control(&slot);
        for variant in 0..4 {
            let mut invalid = identity.clone();
            match variant {
                0 => invalid.runtime.instance = uuid::Uuid::new_v4(),
                1 => invalid.runtime.build_sha256 = "f".repeat(64),
                2 => invalid.runtime.transport_generation += 1,
                _ => invalid.worker = uuid::Uuid::nil(),
            }
            assert!(slot.clipboard_route(&invalid, true, true).await.is_err());
        }
        assert!(calls.try_recv().is_err());
        let mut tasks = tokio::task::JoinSet::new();
        for index in 0..16 {
            let slot = slot.clone();
            let identity = identity.clone();
            tasks.spawn(async move { slot.clipboard_route(&identity, index % 2 == 0, true).await });
        }
        for revision in 1..=16 {
            let call = tokio::time::timeout(Duration::from_secs(1), calls.recv())
                .await
                .unwrap()
                .unwrap();
            assert_eq!(route_reply(call, &identity, None), revision);
        }
        let mut received = std::collections::BTreeSet::new();
        while let Some(result) = tasks.join_next().await {
            received.insert(result.unwrap().unwrap());
        }
        assert_eq!(received, (1..=16).collect());
    }
    #[tokio::test]
    async fn cancelled_and_rejected_routes_consume_revisions_and_draining_runtime_keeps_its_counter()
     {
        let slot = test_slot();
        let (old_control, mut calls, identity) = clipboard_control(&slot);
        let request = {
            let slot = slot.clone();
            let identity = identity.clone();
            tokio::spawn(async move { slot.clipboard_route(&identity, true, true).await })
        };
        let call = calls.recv().await.unwrap();
        request.abort();
        assert!(request.await.unwrap_err().is_cancelled());
        assert_eq!(route_reply(call, &identity, None), 1);
        let (_new_control, mut new_calls, new_identity) = clipboard_control(&slot);
        assert!(!old_control.is_closed());
        let respond = async {
            assert_eq!(
                route_reply(
                    calls.recv().await.unwrap(),
                    &identity,
                    Some("worker unavailable".into())
                ),
                2
            );
        };
        let (result, ()) = tokio::join!(slot.clipboard_route(&identity, false, false), respond);
        assert_eq!(result.unwrap_err(), "worker unavailable");
        let respond = async {
            assert_eq!(route_reply(calls.recv().await.unwrap(), &identity, None), 3);
            assert_eq!(
                route_reply(new_calls.recv().await.unwrap(), &new_identity, None),
                1
            );
        };
        let (old, new, ()) = tokio::join!(
            slot.clipboard_route(&identity, true, false),
            slot.clipboard_route(&new_identity, false, false),
            respond
        );
        assert_eq!(old.unwrap(), 3);
        assert_eq!(new.unwrap(), 1);
    }
    #[tokio::test]
    async fn route_ack_from_closed_control_does_not_authorize() {
        let slot = test_slot();
        let (control, mut calls, identity) = clipboard_control(&slot);
        let respond = async {
            let call = calls.recv().await.unwrap();
            control.stop();
            route_reply(call, &identity, None);
        };
        let (result, ()) = tokio::join!(slot.clipboard_route(&identity, true, true), respond);
        assert!(result.is_err());
    }
    struct Released(Arc<tokio::sync::Notify>);
    impl Drop for Released {
        fn drop(&mut self) {
            self.0.notify_one();
        }
    }
    #[tokio::test]
    async fn worker_stream_negotiation_preserves_legacy_and_rejects_other_runtime_endpoints() {
        for (available, path, succeeds) in [
            (false, None, true),
            (true, Some("/tmp/streams.sock"), true),
            (true, None, false),
            (true, Some("/tmp/other/streams.sock"), false),
            (true, Some("/tmp/unused-test-runtime.sock"), false),
        ] {
            let slot = test_slot();
            let mut control = fake_control(1);
            if available {
                control.hello.capabilities.insert(
                    protocol::worker_stream::CAPABILITY.into(),
                    protocol::Capability {
                        versions: vec![protocol::worker_stream::VERSION],
                        available: true,
                        reason: None,
                    },
                );
            }
            let (commands, mut pending) = mpsc::channel(1);
            control.commands = commands;
            let control = StdMutex::new(Some(control));
            let hello = slot
                .ensure_with(
                    None,
                    || Ok(()),
                    |_| {
                        let control = control.lock().unwrap().take().unwrap();
                        async move { Ok(control) }
                    },
                )
                .await
                .unwrap();
            let worker = uuid::Uuid::new_v4();
            let respond = async {
                let call = pending.recv().await.unwrap();
                match call.operation {
                    protocol::Operation::PrepareWorkerStream {
                        worker_id,
                        service,
                        version,
                    } if available => {
                        assert_eq!(worker_id, worker.to_string());
                        assert_eq!(service, "monitor");
                        assert_eq!(version, 1);
                    }
                    protocol::Operation::Register { worker_id, .. } if !available => {
                        assert_eq!(worker_id, worker.to_string())
                    }
                    _ => panic!("Wrong worker registration operation"),
                }
                let endpoint = path.map(|path| {
                    protocol::worker_stream::Endpoint::new(path.into(), "b".repeat(64)).unwrap()
                });
                call.reply
                    .send(Ok(protocol::Reply {
                        clipboard_route: None,
                        id: "fixture".into(),
                        runtime_id: hello.instance_id.clone(),
                        error: None,
                        applied: None,
                        worker_stream: endpoint,
                    }))
                    .unwrap();
            };
            let (result, ()) = tokio::join!(slot.prepare(&hello, worker, "monitor", 1), respond);
            assert_eq!(result.is_ok(), succeeds);
            if let Ok(endpoint) = result {
                assert_eq!(endpoint.is_some(), available);
            }
            assert_eq!(slot.snapshot().status, "ready");
            slot.reset().await;
        }
    }
    #[tokio::test]
    async fn upgrade_handoff_preserves_admitted_owners_and_rejects_stale_or_full_publication() {
        let slot = test_slot();
        slot.ensure_with(
            None,
            || Ok(()),
            |generation| async move { Ok(fake_control(generation)) },
        )
        .await
        .unwrap();
        let original = slot.connection.lock().unwrap().control.clone().unwrap();
        let mut rejected = fake_control(1);
        rejected.hello.build_sha256 = "1".repeat(64);
        assert_eq!(
            slot.publish_candidate(rejected, || Err(Error::Stale))
                .unwrap_err(),
            Error::Stale
        );
        assert!(!*original.drain.borrow());
        let same_build = fake_control(1);
        slot.publish_candidate(same_build, || Ok(())).unwrap();
        assert_eq!(
            slot.snapshot().hello.unwrap().instance_id,
            original.hello.instance_id
        );
        assert!(!*original.drain.borrow());
        let mut held = vec![original];
        for n in 1..=4 {
            let mut candidate = fake_control(1);
            candidate.hello.build_sha256 = format!("{n:x}").repeat(64);
            slot.publish_candidate(candidate, || Ok(())).unwrap();
            assert!(held.iter().all(|c| *c.drain.borrow() && !c.is_closed()));
            held.push(slot.connection.lock().unwrap().control.clone().unwrap());
        }
        // A normal budget rotation may already have opened the activated
        // build before lifecycle publication; matching current builds must
        // still signal older retained builds to drain.
        held[0].drain.send_replace(false);
        let mut already_adopted = fake_control(1);
        already_adopted.hello.build_sha256 = "4".repeat(64);
        slot.publish_candidate(already_adopted, || Ok(())).unwrap();
        assert!(*held[0].drain.borrow());
        let current = slot.snapshot().hello.unwrap().instance_id;
        let mut blocked = fake_control(1);
        blocked.hello.build_sha256 = "5".repeat(64);
        assert_eq!(
            slot.publish_candidate(blocked, || Ok(())).unwrap_err(),
            Error::Interrupted(BUSY.into())
        );
        assert_eq!(slot.snapshot().hello.unwrap().instance_id, current);
        assert!(!*held.last().unwrap().drain.borrow());
        held.remove(0);
        let mut candidate = fake_control(1);
        candidate.hello.build_sha256 = "5".repeat(64);
        slot.publish_candidate(candidate, || Ok(())).unwrap();
        assert_ne!(slot.snapshot().hello.unwrap().instance_id, current);
        assert!(held.iter().all(|c| *c.drain.borrow() && !c.is_closed()));
        slot.reset().await;
        assert!(held.iter().all(|c| c.is_closed()));
    }
    #[tokio::test]
    async fn control_queue_saturation_is_retryable_and_preserves_runtime() {
        let slot = test_slot();
        let mut control = fake_control(1);
        let (commands, mut pending) = mpsc::channel(1);
        control.commands = commands.clone();
        let control = StdMutex::new(Some(control));
        let hello = slot
            .ensure_with(
                None,
                || Ok(()),
                |_| {
                    let control = control.lock().unwrap().take().unwrap();
                    async move { Ok(control) }
                },
            )
            .await
            .unwrap();
        let (reply, _) = oneshot::channel();
        assert!(
            commands
                .try_send(Call {
                    operation: protocol::Operation::Ping,
                    reply,
                })
                .is_ok()
        );
        assert_eq!(
            slot.request(&hello.instance_id, protocol::Operation::Ping)
                .await
                .err()
                .unwrap(),
            Error::Interrupted(BUSY.into())
        );
        assert_eq!(slot.snapshot().status, "ready");
        pending.recv().await.unwrap();
        let respond = async {
            let call = pending.recv().await.unwrap();
            call.reply
                .send(Ok(protocol::Reply {
                    clipboard_route: None,
                    worker_stream: None,
                    id: "test".into(),
                    runtime_id: hello.instance_id.clone(),
                    error: None,
                    applied: None,
                }))
                .unwrap();
        };
        let (result, _) = tokio::join!(
            slot.request(&hello.instance_id, protocol::Operation::Ping),
            respond
        );
        assert!(result.is_ok());
        assert_eq!(
            slot.snapshot().hello.unwrap().instance_id,
            hello.instance_id
        );
        drop(pending);
        assert_eq!(
            slot.request(&hello.instance_id, protocol::Operation::Ping)
                .await
                .err()
                .unwrap(),
            Error::Interrupted("Needle runtime disconnected".into())
        );
        slot.reset().await;
    }
    #[tokio::test]
    async fn cancelling_first_waiter_preserves_shared_startup_and_retry_does_not_restart_it() {
        let slot = test_slot();
        let started = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Notify::new());
        let calls = Arc::new(AtomicU64::new(0));
        let spawn = || {
            let slot = slot.clone();
            let started = started.clone();
            let release = release.clone();
            let calls = calls.clone();
            tokio::spawn(async move {
                slot.ensure_with(
                    None,
                    || Ok(()),
                    |generation| {
                        calls.fetch_add(1, Ordering::SeqCst);
                        let started = started.clone();
                        let release = release.clone();
                        async move {
                            started.notify_one();
                            release.notified().await;
                            Ok(fake_control(generation))
                        }
                    },
                )
                .await
            })
        };
        let first = spawn();
        started.notified().await;
        let second = spawn();
        tokio::task::yield_now().await;
        first.abort();
        let _ = first.await;
        slot.retry_failed().await;
        assert_eq!(slot.snapshot().status, "starting");
        release.notify_one();
        let hello = tokio::time::timeout(Duration::from_secs(1), second)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(
            slot.snapshot().hello.unwrap().instance_id,
            hello.instance_id
        );
        slot.reset().await;
    }
    #[tokio::test(start_paused = true)]
    async fn temporary_startup_failure_retries_without_limit_and_shares_each_attempt() {
        let slot = test_slot();
        let calls = AtomicU64::new(0);
        let began = Instant::now();
        let create = |generation| {
            let attempt = calls.fetch_add(1, Ordering::SeqCst);
            async move {
                if attempt < 12 {
                    Err(StartupFailure::temporary("connection interrupted"))
                } else {
                    Ok(fake_control(generation))
                }
            }
        };
        let (first, second) = tokio::join!(
            slot.ensure_with(None, || Ok(()), &create),
            slot.ensure_with(None, || Ok(()), &create),
        );
        assert_eq!(first.unwrap().instance_id, second.unwrap().instance_id);
        assert_eq!(calls.load(Ordering::SeqCst), 13);
        assert!(began.elapsed() > Duration::from_secs(120));
        assert_eq!(slot.snapshot().status, "ready");
    }

    #[tokio::test(start_paused = true)]
    async fn a_lost_connection_does_not_hold_the_next_start_once_the_link_is_ready() {
        let slot = test_slot();
        let reaching = |reach| newport_ssh::Status {
            reach,
            ..Default::default()
        };
        let (link, status) = watch::channel(reaching(newport_ssh::Reach::Reconnecting {
            attempt: 1,
            next: None,
        }));
        let (attempted, mut attempts) = mpsc::unbounded_channel();
        let calls = AtomicU64::new(0);
        let ensure = slot.ensure_with(
            Some(status),
            || Ok(()),
            |generation| {
                attempted.send(Instant::now()).unwrap();
                let lost = calls.fetch_add(1, Ordering::SeqCst) < 2;
                async move {
                    if lost {
                        Err(StartupFailure::link(newport_ssh::Error::Lost(
                            "connection reset".into(),
                        )))
                    } else {
                        Ok(fake_control(generation))
                    }
                }
            },
        );
        let reconnect = async {
            let mut times = Vec::new();
            for _ in 0..3 {
                times.push(attempts.recv().await.unwrap());
                if times.len() == 2 {
                    // The second loss, then the link logs in again.
                    tokio::time::sleep(Duration::from_millis(100)).await;
                    link.send_replace(reaching(newport_ssh::Reach::Ready { connections: 1 }));
                }
            }
            times
        };
        let (result, times) = tokio::join!(ensure, reconnect);
        result.unwrap();
        assert!(
            times[2] - times[1] < Duration::from_secs(1),
            "the start after the link's return waits for no backoff: {:?}",
            times[2] - times[1]
        );
        assert_eq!(slot.snapshot().failures, 0);
    }

    #[tokio::test]
    async fn a_start_nobody_waits_for_is_cancelled_without_counting_as_a_failure() {
        let slot = test_slot();
        let started = tokio::sync::Notify::new();
        let released = Arc::new(tokio::sync::Notify::new());
        let create = |_| {
            started.notify_one();
            let released = Released(released.clone());
            async move {
                let _released = released;
                std::future::pending::<Result<Control, StartupFailure>>().await
            }
        };
        let cancelled = released.notified();
        tokio::select! {
            result = slot.ensure_with(None, || Ok(()), &create) => {
                panic!("the start can't finish: {:?}", result.err())
            }
            () = started.notified() => {}
        }
        tokio::time::timeout(Duration::from_secs(5), cancelled)
            .await
            .expect("the start to be cancelled");
        let snapshot = slot.snapshot();
        assert_ne!(snapshot.status, "failed");
        assert_eq!(snapshot.failures, 0);
        assert!(slot.connection.lock().unwrap().startup.is_none());
    }

    #[tokio::test(start_paused = true)]
    async fn cancelled_retry_waiter_leaves_no_background_retry_loop() {
        let slot = test_slot();
        let calls = AtomicU64::new(0);
        let mut changes = slot.changes.subscribe();
        let retry = slot.ensure_with(
            None,
            || Ok(()),
            |_| {
                calls.fetch_add(1, Ordering::SeqCst);
                async { Err(StartupFailure::temporary("connection interrupted")) }
            },
        );
        tokio::select! {
            result = retry => panic!("transient failure must keep waiting: {result:?}"),
            _ = changes.changed() => {},
        }
        assert_eq!(slot.snapshot().status, "failed");
        tokio::time::advance(Duration::from_secs(60)).await;
        tokio::task::yield_now().await;
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn retirement_wakes_backoff_and_does_not_restart_old_generation() {
        let slot = test_slot();
        let calls = AtomicU64::new(0);
        let valid = AtomicBool::new(true);
        let mut changes = slot.changes.subscribe();
        let retry = slot.ensure_with(
            None,
            || {
                if valid.load(Ordering::SeqCst) {
                    Ok(())
                } else {
                    Err(Error::Stale)
                }
            },
            |_| {
                calls.fetch_add(1, Ordering::SeqCst);
                async { Err(StartupFailure::temporary("connection interrupted")) }
            },
        );
        let retire = async {
            changes.changed().await.unwrap();
            valid.store(false, Ordering::SeqCst);
            slot.reset().await;
        };
        let (result, ()) = tokio::join!(retry, retire);
        assert_eq!(result.unwrap_err(), Error::Stale);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(slot.snapshot().status, "unknown");
    }

    #[tokio::test]
    async fn interrupted_handshake_is_retryable_but_invalid_protocol_is_not() {
        for bytes in [vec![], vec![0, 0], vec![0, 0, 0, 4, 1, 2]] {
            let failure = read::<protocol::Hello>(&mut bytes.as_slice())
                .await
                .unwrap_err();
            assert!(failure.retryable, "{}", failure.error);
        }
        for bytes in [
            vec![0, 0, 0, 0],
            u32::MAX.to_be_bytes().to_vec(),
            vec![0, 0, 0, 1, 0xc1],
        ] {
            let failure = read::<protocol::Hello>(&mut bytes.as_slice())
                .await
                .unwrap_err();
            assert!(!failure.retryable, "{}", failure.error);
        }
    }

    #[tokio::test]
    async fn failed_startup_wakes_all_waiters_and_requires_explicit_retry() {
        let slot = test_slot();
        let started = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Notify::new());
        let calls = Arc::new(AtomicU64::new(0));
        let spawn = || {
            let slot = slot.clone();
            let started = started.clone();
            let release = release.clone();
            let calls = calls.clone();
            tokio::spawn(async move {
                slot.ensure_with(
                    None,
                    || Ok(()),
                    |_| {
                        calls.fetch_add(1, Ordering::SeqCst);
                        let started = started.clone();
                        let release = release.clone();
                        async move {
                            started.notify_one();
                            release.notified().await;
                            Err(StartupFailure::permanent("incompatible"))
                        }
                    },
                )
                .await
            })
        };
        let first = spawn();
        started.notified().await;
        let second = spawn();
        tokio::task::yield_now().await;
        release.notify_one();
        for waiter in [first, second] {
            assert_eq!(
                tokio::time::timeout(Duration::from_secs(1), waiter)
                    .await
                    .expect("all waiters must observe startup failure")
                    .unwrap()
                    .unwrap_err()
                    .to_string(),
                "incompatible"
            );
        }
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(slot.snapshot().failures, 1);
        assert_eq!(
            slot.ensure_with(
                None,
                || Ok(()),
                |generation| async move { Ok(fake_control(generation)) }
            )
            .await
            .unwrap_err()
            .to_string(),
            "incompatible"
        );
        slot.retry_failed().await;
        slot.ensure_with(
            None,
            || Ok(()),
            |generation| async move { Ok(fake_control(generation)) },
        )
        .await
        .unwrap();
        assert_eq!(slot.snapshot().status, "ready");
        slot.reset().await;
    }
    #[tokio::test]
    async fn retirement_interrupts_startup_and_old_completion_cannot_replace_same_generation() {
        let slot = test_slot();
        let started = Arc::new(tokio::sync::Notify::new());
        let released = Arc::new(tokio::sync::Notify::new());
        let valid = Arc::new(std::sync::atomic::AtomicBool::new(true));
        let waiter = {
            let slot = slot.clone();
            let started = started.clone();
            let released = released.clone();
            let valid = valid.clone();
            tokio::spawn(async move {
                slot.ensure_with(
                    None,
                    || {
                        if valid.load(Ordering::Acquire) {
                            Ok(())
                        } else {
                            Err(Error::Stale)
                        }
                    },
                    |_| {
                        let started = started.clone();
                        let released = released.clone();
                        async move {
                            let _released = Released(released);
                            started.notify_one();
                            std::future::pending::<Result<Control, StartupFailure>>().await
                        }
                    },
                )
                .await
            })
        };
        started.notified().await;
        valid.store(false, Ordering::Release);
        tokio::time::timeout(Duration::from_millis(100), slot.reset())
            .await
            .expect("reset cannot wait for handshake");
        let next = slot
            .ensure_with(
                None,
                || Ok(()),
                |generation| async move { Ok(fake_control(generation)) },
            )
            .await
            .unwrap();
        assert_eq!(waiter.await.unwrap().unwrap_err(), Error::Stale);
        tokio::time::timeout(Duration::from_secs(1), released.notified())
            .await
            .unwrap();
        assert_eq!(slot.snapshot().status, "ready");
        assert_eq!(slot.snapshot().hello.unwrap().instance_id, next.instance_id);
        slot.reset().await;
    }
    #[test]
    fn interrupted_startup_clears_busy_state_without_overwriting_a_new_generation() {
        let snapshot = Arc::new(StdMutex::new(Snapshot {
            status: "starting",
            generation: 1,
            ..Snapshot::default()
        }));
        let old = Starting {
            snapshot: snapshot.clone(),
            generation: 1,
            epoch: 0,
            changes: watch::channel(0).0,
        };
        snapshot.lock().unwrap().generation = 2;
        drop(old);
        assert_eq!(snapshot.lock().unwrap().status, "starting");
        drop(Starting {
            snapshot: snapshot.clone(),
            generation: 2,
            epoch: 0,
            changes: watch::channel(0).0,
        });
        assert_eq!(snapshot.lock().unwrap().status, "failed");
        assert!(snapshot.lock().unwrap().retryable);
    }
    #[tokio::test]
    async fn retry_state_distinguishes_actionable_failures_and_caps_repeated_losses() {
        let slot = Slot::new(Arc::new(crate::integration_permissions::Gate::new(
            false, false,
        )));
        {
            let mut snapshot = slot.snapshot.lock().unwrap();
            record_failure(
                &mut snapshot,
                Error::Unavailable("incompatible".into()),
                false,
                false,
            );
            assert!(snapshot.retry_at.is_none());
        }
        assert_eq!(slot.wait_for_retry().await.unwrap_err(), "incompatible");
        slot.retry_failed().await;
        assert_eq!(slot.snapshot().status, "unknown");
        for attempt in 1..=8 {
            let mut snapshot = slot.snapshot.lock().unwrap();
            record_failure(
                &mut snapshot,
                Error::Interrupted("control EOF".into()),
                true,
                false,
            );
            assert_eq!(snapshot.failures, attempt);
            let delay = snapshot
                .retry_at
                .unwrap()
                .saturating_duration_since(Instant::now());
            assert!(delay <= Duration::from_secs(30));
            assert!(delay >= Duration::from_millis(1_590));
            assert!(snapshot.retryable);
        }
    }
    #[tokio::test]
    async fn control_rejects_unbounded_frames_and_stale_replies() {
        assert!(
            read::<protocol::Reply>(&mut &u32::MAX.to_be_bytes()[..])
                .await
                .is_err()
        );
        let (mut client, mut server) = tokio::io::duplex(32768);
        let peer = tokio::spawn(async move {
            let request: protocol::Request = read(&mut server).await.unwrap();
            let reply = protocol::Reply {
                clipboard_route: None,
                worker_stream: None,
                applied: None,
                id: request.id,
                runtime_id: uuid::Uuid::new_v4().to_string(),
                error: None,
            };
            let bytes = protocol::encode(&reply).unwrap();
            server.write_u32(bytes.len() as u32).await.unwrap();
            server.write_all(&bytes).await.unwrap();
        });
        assert!(
            exchange(
                &mut client,
                &uuid::Uuid::new_v4().to_string(),
                protocol::Operation::Ping
            )
            .await
            .unwrap_err()
            .contains("mismatched")
        );
        peer.await.unwrap();
    }
}
