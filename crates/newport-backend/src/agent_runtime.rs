//! Session-owned runtime control channel, separate from feature traffic.
use crate::ssh::ExecSession;
use serde::Serialize;
use std::{
    sync::{
        Arc, Mutex as StdMutex, Weak,
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    sync::{Semaphore, mpsc, oneshot, watch},
    task::JoinHandle,
};

#[path = "agent_runtime_worker.rs"]
mod worker;
pub use worker::WorkerStream;
pub(crate) use worker::{Handle, Worker};

pub(crate) const BUSY: &str = "Needle services are busy. Retry shortly.";
pub(crate) const UPGRADED: &str = "Needle runtime upgraded";
const MAX_RETIRING: usize = 4;

#[allow(dead_code)]
#[path = "../../../tools/agent/src/runtime_protocol.rs"]
pub mod protocol;

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub status: &'static str,
    pub transport_generation: u64,
    pub hello: Option<protocol::Hello>,
    pub error: Option<String>,
    pub retryable: bool,
    pub failures: u32,
    pub instances: Vec<RuntimeInstance>,
    #[serde(skip)]
    retry_at: Option<Instant>,
    #[serde(skip)]
    startup_epoch: u64,
}
impl Default for Snapshot {
    fn default() -> Self {
        Self {
            status: "unknown",
            transport_generation: 0,
            hello: None,
            error: None,
            retryable: false,
            failures: 0,
            instances: Vec::new(),
            retry_at: None,
            startup_epoch: 0,
        }
    }
}
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeInstance {
    pub instance_id: String,
    pub build_sha256: String,
    pub transport_generation: u64,
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
            && current.transport_generation == self.generation
            && current.startup_epoch == self.epoch
        {
            record_failure(
                &mut current,
                "Needle runtime startup was interrupted. Retry when connected.".into(),
                true,
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
#[derive(Default)]
struct Connection {
    control: Option<Arc<Control>>,
    retiring: Vec<Weak<Control>>,
    startup: Option<Startup>,
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
    snapshot: Arc<StdMutex<Snapshot>>,
    permissions: Arc<crate::integration_permissions::Gate>,
}
impl Slot {
    pub fn new(permissions: Arc<crate::integration_permissions::Gate>) -> Self {
        Self {
            connection: Arc::default(),
            changes: watch::channel(0).0,
            sequence: AtomicU64::new(0),
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
                transport_generation: c.generation,
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
        connection: &ExecSession,
        session: uuid::Uuid,
        generation: u64,
        hash: &str,
        validate: impl Fn() -> Result<(), String>,
    ) -> Result<(), String> {
        if !protocol::valid_build(hash) {
            return Err("Invalid candidate hash".into());
        }
        validate()?;
        {
            let state = self.connection.lock().unwrap();
            if state.control.as_ref().is_some_and(|c| {
                !c.is_closed() && c.generation == generation && c.hello.build_sha256 == hash
            }) {
                state.drain_other_builds(hash);
                return Ok(());
            }
        }
        let command = format!(
            "exec \"$HOME/.local/share/newport/needle/builds/{hash}/newport-agent\" runtime --stdio"
        );
        let (mut stream, hello) = tokio::time::timeout(
            Duration::from_secs(15),
            start_at(connection, session, generation, &command),
        )
        .await
        .map_err(|_| "Needle upgrade handoff timed out")?
        .map_err(|e| e.message)?;
        if hello.build_sha256 != hash {
            return Err("Needle upgrade candidate identity changed".into());
        }
        exchange(&mut stream, &hello.instance_id, protocol::Operation::Ping)
            .await
            .and_then(accepted)?;
        let candidate = Control::run(
            stream,
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
        validate: impl Fn() -> Result<(), String>,
    ) -> Result<(), String> {
        let mut state = self.connection.lock().unwrap();
        validate()?;
        state.prune();
        if state.control.as_ref().is_some_and(|c| {
            !c.is_closed()
                && c.generation == candidate.generation
                && c.hello.build_sha256 == candidate.hello.build_sha256
        }) {
            state.drain_other_builds(&candidate.hello.build_sha256);
            return Ok(());
        }
        if state.retiring.len() >= MAX_RETIRING
            && state
                .control
                .as_ref()
                .is_some_and(|c| Arc::strong_count(c) > 1)
        {
            return Err(BUSY.into());
        }
        if candidate.is_closed() {
            return Err("Needle upgrade candidate disconnected".into());
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
            transport_generation: candidate.generation,
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
        connection: &ExecSession,
        session: uuid::Uuid,
        generation: u64,
        service: &str,
        version: u32,
        validate: impl Fn() -> Result<(), String>,
    ) -> Result<Worker, String> {
        loop {
            let hello = self
                .ensure(connection, session, generation, &validate)
                .await?;
            hello.require(service, version).map_err(|e| e.to_string())?;
            let worker = {
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
                        return Err(BUSY.into());
                    }
                    state.control = None;
                    state.retiring.push(Arc::downgrade(&control));
                    *self.snapshot.lock().unwrap() = Snapshot {
                        transport_generation: generation,
                        startup_epoch: self.sequence.fetch_add(1, Ordering::AcqRel) + 1,
                        ..Snapshot::default()
                    };
                    continue;
                }
                let permit = control
                    .workers
                    .clone()
                    .try_acquire_owned()
                    .map_err(|_| BUSY)?;
                // Count before sending: cancelled or lost registration replies
                // may still have consumed a one-use ticket on the server.
                control.registrations.fetch_add(1, Ordering::AcqRel);
                Worker {
                    runtime: Handle(control),
                    id: uuid::Uuid::new_v4(),
                    _permit: permit,
                }
            };
            self.register(worker.runtime.hello(), worker.id, service, version)
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
    async fn register(
        &self,
        hello: &protocol::Hello,
        worker: uuid::Uuid,
        service: &str,
        version: u32,
    ) -> Result<(), String> {
        self.request(
            &hello.instance_id,
            protocol::Operation::Register {
                worker_id: worker.to_string(),
                service: service.into(),
                version,
            },
        )
        .await
        .map(|_| ())
    }
    pub async fn permissions(
        &self,
        runtime: &str,
        worker: uuid::Uuid,
        epoch: u64,
        permissions: protocol::integration::Permissions,
    ) -> Result<protocol::integration::Applied, String> {
        let control = self
            .connection
            .lock()
            .unwrap()
            .find(runtime)
            .ok_or("Needle runtime is not connected")?;
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
        .ok_or_else(|| "Needle did not acknowledge applied permissions".into())
    }
    async fn request(
        &self,
        runtime: &str,
        operation: protocol::Operation,
    ) -> Result<protocol::Reply, String> {
        let commands = {
            let state = self.connection.lock().unwrap();
            let control = state
                .find(runtime)
                .ok_or("Needle runtime changed before worker registration")?;
            control.commands.clone()
        };
        let (reply, response) = oneshot::channel();
        commands
            .try_send(Call { operation, reply })
            .map_err(|error| match error {
                mpsc::error::TrySendError::Full(_) => BUSY,
                mpsc::error::TrySendError::Closed(_) => "Needle runtime disconnected",
            })?;
        let reply = tokio::time::timeout(Duration::from_secs(10), response)
            .await
            .map_err(|_| "Needle control request timed out")?
            .map_err(|_| "Needle runtime disconnected during control request")??;
        if let Some(error) = &reply.error {
            if error == "Needle worker registration limit reached" {
                return Err(BUSY.into());
            }
            return Err(error.clone());
        }
        Ok(reply)
    }
    pub async fn ensure(
        &self,
        connection: &ExecSession,
        session_id: uuid::Uuid,
        generation: u64,
        validate: impl Fn() -> Result<(), String>,
    ) -> Result<protocol::Hello, String> {
        self.ensure_with(generation, validate, || {
            let connection = connection.with_server(connection.server().clone());
            let snapshot = self.snapshot.clone();
            let permissions = self.permissions.clone();
            async move {
                let (stream, hello) = tokio::time::timeout(
                    Duration::from_secs(15),
                    start(&connection, session_id, generation),
                )
                .await
                .map_err(|_| StartupFailure::temporary("Needle runtime handshake timed out."))??;
                Ok(Control::run(
                    stream,
                    hello,
                    generation,
                    snapshot,
                    permissions,
                ))
            }
        })
        .await
    }
    async fn ensure_with<F, Fut>(
        &self,
        generation: u64,
        validate: impl Fn() -> Result<(), String>,
        create: F,
    ) -> Result<protocol::Hello, String>
    where
        F: Fn() -> Fut,
        Fut: std::future::Future<Output = Result<Control, StartupFailure>> + Send + 'static,
    {
        let mut changes = self.changes.subscribe();
        loop {
            changes.borrow_and_update();
            {
                let mut state = self.connection.lock().unwrap();
                validate()?;
                if let Some(control) = &state.control
                    && control.generation == generation
                    && !control.is_closed()
                    && self.current_snapshot().status == "ready"
                {
                    return Ok(control.hello.clone());
                }
                if state
                    .control
                    .as_ref()
                    .is_some_and(|c| c.generation != generation)
                {
                    self.clear(&mut state);
                }
                state.control = None;
                if state
                    .startup
                    .as_ref()
                    .is_some_and(|s| s.generation != generation)
                {
                    self.clear(&mut state);
                }
                let previous = self.current_snapshot();
                if previous.transport_generation == generation
                    && previous.status == "failed"
                    && (!previous.retryable
                        || previous.retry_at.is_some_and(|at| at > Instant::now()))
                {
                    return Err(previous
                        .error
                        .unwrap_or_else(|| "Needle runtime unavailable".into()));
                }
                if state
                    .startup
                    .as_ref()
                    .is_some_and(|s| s.task.as_ref().is_none_or(|task| task.is_finished()))
                {
                    state.startup = None;
                }
                if state.startup.is_none() {
                    let epoch = self.sequence.fetch_add(1, Ordering::AcqRel) + 1;
                    *self.snapshot.lock().unwrap() = Snapshot {
                        status: "starting",
                        transport_generation: generation,
                        startup_epoch: epoch,
                        failures: if previous.transport_generation == generation {
                            previous.failures
                        } else {
                            0
                        },
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
                    let start = create();
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
                                snapshot.retryable = false;
                                snapshot.retry_at = None;
                                state.control = Some(Arc::new(control));
                            }
                            Err(error) => {
                                snapshot.hello = None;
                                record_failure(&mut snapshot, error.message, error.retryable);
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
            changes
                .changed()
                .await
                .map_err(|_| "Needle session closed".to_owned())?;
        }
    }
}
impl Control {
    fn run(
        mut stream: Stream,
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
            let fail = |error: String| {
                if !failure_state.swap(true, Ordering::AcqRel) {
                    permissions.disconnect_if(bound_epoch.load(Ordering::Acquire));
                    record_control_failure(&snapshot, generation, &runtime_id, error, started);
                }
            };
            let mut timer = tokio::time::interval_at(
                tokio::time::Instant::now() + Duration::from_secs(15),
                Duration::from_secs(15),
            );
            timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            let error = loop {
                tokio::select! {
                    biased;
                    _ = timer.tick() => {
                        if let Err(error) = exchange(&mut stream, &runtime_id, protocol::Operation::Ping).await.and_then(accepted) { break error; }
                    }
                    call = calls.recv() => {
                        let Some(call) = call else { break "Needle control closed".into() };
                        let result = exchange(&mut stream, &runtime_id, call.operation).await;
                        let failure = result.as_ref().err().cloned();
                        if let Some(error) = &failure {
                            fail(error.clone());
                        }
                        let _ = call.reply.send(result);
                        if let Some(error) = failure { break error; }
                    }
                }
            };
            fail(error);
        });
        Control {
            commands,
            generation,
            workers: Arc::new(Semaphore::new(hello.limits.max_workers)),
            hello,
            heartbeat,
            failed,
            permission_epoch,
            registrations: AtomicUsize::new(0),
            drain: watch::channel(false).0,
        }
    }
}

fn record_failure(snapshot: &mut Snapshot, error: String, retryable: bool) {
    snapshot.status = "failed";
    snapshot.error = Some(error);
    snapshot.retryable = retryable;
    snapshot.failures = snapshot.failures.saturating_add(1);
    snapshot.retry_at = retryable.then(|| {
        Instant::now()
            + crate::session_transport::backoff(
                snapshot.failures,
                uuid::Uuid::new_v4().as_u128() as u64,
            )
    });
}
fn record_control_failure(
    snapshot: &StdMutex<Snapshot>,
    generation: u64,
    runtime: &str,
    error: String,
    started: Instant,
) {
    let mut current = snapshot.lock().unwrap();
    if current.status == "ready"
        && current.transport_generation == generation
        && current
            .hello
            .as_ref()
            .is_some_and(|h| h.instance_id == runtime)
    {
        if started.elapsed() >= Duration::from_secs(60) {
            current.failures = 0;
        }
        record_failure(&mut current, error, true);
    }
}
struct StartupFailure {
    message: String,
    retryable: bool,
}
impl StartupFailure {
    fn temporary(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            retryable: true,
        }
    }
    fn permanent(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            retryable: false,
        }
    }
}
type Stream = russh::ChannelStream<russh::client::Msg>;
async fn start(
    connection: &ExecSession,
    session_id: uuid::Uuid,
    generation: u64,
) -> Result<(Stream, protocol::Hello), StartupFailure> {
    if !crate::agent::is_installed(connection)
        .await
        .map_err(StartupFailure::temporary)?
    {
        return Err(StartupFailure::permanent(
            "Needle is not installed. Open Overview to set up this server.",
        ));
    }
    start_at(
        connection,
        session_id,
        generation,
        "exec \"$HOME/.local/bin/newport-agent\" runtime --stdio",
    )
    .await
}
async fn start_at(
    connection: &ExecSession,
    session_id: uuid::Uuid,
    generation: u64,
    command: &str,
) -> Result<(Stream, protocol::Hello), StartupFailure> {
    let mut stream = connection
        .stream(command)
        .await
        .map_err(StartupFailure::temporary)?;
    let hello: protocol::Hello = read(&mut stream).await.map_err(|e| {
        StartupFailure::permanent(format!(
            "Needle runtime handshake failed. Update or repair Needle: {e}"
        ))
    })?;
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
    Ok((stream, hello))
}
/// Verify a staged build using the desktop's actual control protocol before
/// activation. This temporary runtime never replaces the session's live slot.
pub(crate) async fn verify_candidate(connection: &ExecSession, hash: &str) -> Result<(), String> {
    if !protocol::valid_build(hash) {
        return Err("Invalid candidate hash".into());
    }
    tokio::time::timeout(Duration::from_secs(15), async {
        let command = format!(
            "exec \"$HOME/.local/share/newport/needle/builds/{hash}/newport-agent\" runtime --stdio"
        );
        let (mut stream, hello) = start_at(connection, uuid::Uuid::new_v4(), 1, &command)
            .await
            .map_err(|e| e.message)?;
        if hello.build_sha256 != hash {
            return Err("Candidate runtime has an unexpected executable identity".into());
        }
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
        exchange(
            &mut stream,
            &hello.instance_id,
            protocol::Operation::Shutdown,
        )
        .await
        .and_then(accepted)
    })
    .await
    .map_err(|_| "Candidate runtime verification timed out".to_owned())?
}
async fn read<T: serde::de::DeserializeOwned>(
    input: &mut (impl AsyncRead + Unpin),
) -> Result<T, String> {
    let length = input.read_u32().await.map_err(|e| e.to_string())? as usize;
    if length == 0 || length > protocol::MAX_FRAME {
        return Err("Runtime control frame exceeds limit".into());
    }
    let mut bytes = vec![0; length];
    input
        .read_exact(&mut bytes)
        .await
        .map_err(|e| e.to_string())?;
    protocol::decode(&bytes).map_err(|e| e.to_string())
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
        let reply: protocol::Reply = read(stream).await?;
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
    struct Released(Arc<tokio::sync::Notify>);
    impl Drop for Released {
        fn drop(&mut self) {
            self.0.notify_one();
        }
    }
    #[tokio::test]
    async fn upgrade_handoff_preserves_admitted_owners_and_rejects_stale_or_full_publication() {
        let slot = test_slot();
        slot.ensure_with(1, || Ok(()), || async { Ok(fake_control(1)) })
            .await
            .unwrap();
        let original = slot.connection.lock().unwrap().control.clone().unwrap();
        let mut rejected = fake_control(1);
        rejected.hello.build_sha256 = "1".repeat(64);
        assert_eq!(
            slot.publish_candidate(rejected, || Err("retired owner".into()))
                .unwrap_err(),
            "retired owner"
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
            BUSY
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
                1,
                || Ok(()),
                || {
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
            BUSY
        );
        assert_eq!(slot.snapshot().status, "ready");
        pending.recv().await.unwrap();
        let respond = async {
            let call = pending.recv().await.unwrap();
            call.reply
                .send(Ok(protocol::Reply {
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
            "Needle runtime disconnected"
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
                    1,
                    || Ok(()),
                    || {
                        calls.fetch_add(1, Ordering::SeqCst);
                        let started = started.clone();
                        let release = release.clone();
                        async move {
                            started.notify_one();
                            release.notified().await;
                            Ok(fake_control(1))
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
                    1,
                    || Ok(()),
                    || {
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
                    .unwrap_err(),
                "incompatible"
            );
        }
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(slot.snapshot().failures, 1);
        assert_eq!(
            slot.ensure_with(1, || Ok(()), || async { Ok(fake_control(1)) })
                .await
                .unwrap_err(),
            "incompatible"
        );
        slot.retry_failed().await;
        slot.ensure_with(1, || Ok(()), || async { Ok(fake_control(1)) })
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
                    1,
                    || {
                        if valid.load(Ordering::Acquire) {
                            Ok(())
                        } else {
                            Err("retired".into())
                        }
                    },
                    || {
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
            .ensure_with(1, || Ok(()), || async { Ok(fake_control(1)) })
            .await
            .unwrap();
        assert_eq!(waiter.await.unwrap().unwrap_err(), "retired");
        tokio::time::timeout(Duration::from_secs(1), released.notified())
            .await
            .unwrap();
        assert_eq!(slot.snapshot().status, "ready");
        assert_eq!(slot.snapshot().hello.unwrap().instance_id, next.instance_id);
        slot.reset().await;
    }
    #[tokio::test]
    async fn dropping_owner_aborts_startup_even_after_all_waiters_cancel() {
        let slot = test_slot();
        let started = Arc::new(tokio::sync::Notify::new());
        let released = Arc::new(tokio::sync::Notify::new());
        let waiter = {
            let slot = slot.clone();
            let started = started.clone();
            let released = released.clone();
            tokio::spawn(async move {
                slot.ensure_with(
                    1,
                    || Ok(()),
                    || {
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
        waiter.abort();
        let _ = waiter.await;
        assert_eq!(slot.snapshot().status, "starting");
        drop(slot);
        tokio::time::timeout(Duration::from_secs(1), released.notified())
            .await
            .expect("owner drop must not detach startup");
    }
    #[test]
    fn interrupted_startup_clears_busy_state_without_overwriting_a_new_generation() {
        let snapshot = Arc::new(StdMutex::new(Snapshot {
            status: "starting",
            transport_generation: 1,
            ..Snapshot::default()
        }));
        let old = Starting {
            snapshot: snapshot.clone(),
            generation: 1,
            epoch: 0,
            changes: watch::channel(0).0,
        };
        snapshot.lock().unwrap().transport_generation = 2;
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
            record_failure(&mut snapshot, "incompatible".into(), false);
            assert!(snapshot.retry_at.is_none());
        }
        assert_eq!(slot.wait_for_retry().await.unwrap_err(), "incompatible");
        slot.retry_failed().await;
        assert_eq!(slot.snapshot().status, "unknown");
        for attempt in 1..=8 {
            let mut snapshot = slot.snapshot.lock().unwrap();
            record_failure(&mut snapshot, "control EOF".into(), true);
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
