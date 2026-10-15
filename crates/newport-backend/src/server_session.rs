//! Application-owned server sessions. Consumers lease channels, never ownership
//! of the shared SSH transport. Profile revisions and transport generations fence
//! results from an obsolete connection.
use crate::{agent, manager::Shared, model::Server, ssh::ExecSession};
use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex as StdMutex, Weak,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
};
use tokio::sync::watch;
use uuid::Uuid;

const STALE: &str = "Server session changed. Reconnect before retrying.";

#[derive(Default)]
pub struct Registry {
    receipts: Arc<crate::lifecycle_receipts::Store>,
    entries: StdMutex<HashMap<Uuid, Published>>,
    pub(crate) connected: Arc<tokio::sync::Notify>,
    stopped: AtomicBool,
    cleanup: StdMutex<tokio::task::JoinSet<()>>,
}

struct Published {
    profile: Server,
    owner: Arc<ServerSession>,
}
impl std::ops::Deref for Published {
    type Target = Arc<ServerSession>;
    fn deref(&self) -> &Self::Target {
        &self.owner
    }
}

impl Registry {
    pub fn new(directory: &std::path::Path) -> Self {
        let mut registry = Self::default();
        registry.receipts = Arc::new(crate::lifecycle_receipts::Store::new(
            directory.join("needle-operations.json"),
        ));
        registry
    }
    pub(crate) fn get(&self, id: Uuid) -> Option<Arc<ServerSession>> {
        self.entries
            .lock()
            .expect("server sessions")
            .get(&id)
            .map(|entry| entry.owner.clone())
    }
    pub(crate) fn current(&self, id: Uuid) -> Option<(Server, Arc<ServerSession>)> {
        self.entries
            .lock()
            .expect("server sessions")
            .get(&id)
            .map(|entry| (entry.profile.clone(), entry.owner.clone()))
    }
    /// Validate and publish intent atomically with registry replacement. No
    /// network or persistence work occurs under this short synchronous lock.
    pub(crate) fn integration_preference(
        &self,
        id: Uuid,
        instance: Uuid,
        expected: &serde_json::Value,
        browser: bool,
        enabled: bool,
    ) -> Result<
        (
            Arc<ServerSession>,
            crate::integration_permissions::Preference,
        ),
        String,
    > {
        let entries = self.entries.lock().expect("server sessions");
        let owner = entries
            .get(&id)
            .ok_or("The server connection changed. Review its integrations and try again.")?;
        let server = serde_json::to_value(owner.server()).map_err(|e| e.to_string())?;
        let scope = newport_domain::connection_scope::connection(
            &server,
            &serde_json::json!({
                "instanceId": instance, "runtime":{"connectionRevisions":{id.to_string():owner.revision}}
            }),
        );
        if owner.is_revoked() || scope != *expected {
            return Err(
                "The server connection changed. Review its integrations and try again.".into(),
            );
        }
        let preference = owner.permissions.begin_preference(browser, enabled);
        Ok((owner.owner.clone(), preference))
    }

    pub fn runtime_status(&self) -> HashMap<Uuid, crate::agent_runtime::Snapshot> {
        self.all()
            .iter()
            .map(|owner| (owner.server.id, owner.agent_runtime.snapshot()))
            .collect()
    }
    pub fn lifecycle_operations(&self) -> HashMap<Uuid, serde_json::Value> {
        self.all()
            .iter()
            .map(|owner| (owner.server.id, owner.lifecycle_operations()))
            .collect()
    }

    pub(crate) fn all(&self) -> Vec<Arc<ServerSession>> {
        self.entries
            .lock()
            .expect("server sessions")
            .values()
            .map(|entry| entry.owner.clone())
            .collect()
    }
    pub(crate) fn profiles(&self) -> Vec<(Server, u64)> {
        self.entries
            .lock()
            .expect("server sessions")
            .values()
            .filter(|entry| !entry.is_revoked())
            .map(|entry| (entry.profile.clone(), entry.revision))
            .collect()
    }
    pub fn installation_status(&self) -> HashMap<Uuid, crate::agent_lifecycle::Status> {
        self.all()
            .iter()
            .map(|owner| (owner.server.id, owner.lifecycle.snapshot()))
            .collect()
    }
    pub fn installation_progress(&self) -> HashMap<Uuid, crate::agent_install::Progress> {
        self.all()
            .iter()
            .filter_map(|owner| {
                owner
                    .lifecycle
                    .progress
                    .snapshot()
                    .map(|progress| (owner.server.id, progress))
            })
            .collect()
    }

    pub fn permissions(&self) -> HashMap<Uuid, crate::integration_permissions::Snapshot> {
        self.entries
            .lock()
            .expect("server sessions")
            .iter()
            .map(|(id, session)| (*id, session.permissions.snapshot()))
            .collect()
    }
    pub fn session(&self, server: &Server, revision: u64) -> Arc<ServerSession> {
        let mut entries = self.entries.lock().expect("server sessions");
        if self.stopped.load(Ordering::Acquire) {
            let stopped = Arc::new(ServerSession::new(
                server.clone(),
                revision,
                self.connected.clone(),
                self.receipts.clone(),
            ));
            stopped.cancelled.send_replace(true);
            return stopped;
        }
        if let Some(existing) = entries.get_mut(&server.id)
            && existing.revision == revision
            && existing.server.same_connection(server)
            && !*existing.cancelled.borrow()
        {
            if !crate::metrics_sampler::same_sampling_profile(&existing.profile, server) {
                existing.metrics.clear();
            }
            existing.profile = server.clone();
            return existing.owner.clone();
        }
        let session = Arc::new(ServerSession::new(
            server.clone(),
            revision,
            self.connected.clone(),
            self.receipts.clone(),
        ));
        if let Some(previous) = entries.insert(
            server.id,
            Published {
                profile: server.clone(),
                owner: session.clone(),
            },
        ) {
            self.retire(previous.owner);
        }
        session
    }

    pub fn invalidate(&self, id: Uuid) {
        if let Some(session) = self.entries.lock().expect("server sessions").remove(&id) {
            self.retire(session.owner);
        }
    }

    fn retire(&self, session: Arc<ServerSession>) {
        session.permissions.disconnected(None);
        session.cancelled.send_replace(true);
        session.close_queries();
        // Revoke immediately, and track asynchronous cleanup until shutdown.
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            let mut cleanup = self.cleanup.lock().expect("session cleanup");
            while cleanup.try_join_next().is_some() {}
            cleanup.spawn_on(
                async move {
                    session.close().await;
                },
                &runtime,
            );
        }
    }

    pub(crate) fn request_shutdown(&self) {
        let entries = self.entries.lock().expect("server sessions");
        self.stopped.store(true, Ordering::Release);
        for session in entries.values() {
            session.permissions.disconnected(None);
            session.cancelled.send_replace(true);
            session.close_queries();
        }
    }
    pub async fn shutdown(&self) {
        self.request_shutdown();
        let sessions = std::mem::take(&mut *self.entries.lock().expect("server sessions"));
        for session in sessions.into_values() {
            session.close().await;
        }
        let mut cleanup = std::mem::take(&mut *self.cleanup.lock().expect("session cleanup"));
        while cleanup.join_next().await.is_some() {}
    }
}

impl Drop for Registry {
    fn drop(&mut self) {
        // Tasks may temporarily retain an owner; dropping the registry must
        // still release demand rather than keeping those owners alive forever.
        for owner in self.entries.get_mut().expect("server sessions").values() {
            owner.permissions.disconnected(None);
            owner.cancelled.send_replace(true);
            owner.close_queries();
        }
    }
}

pub struct ServerSession {
    session_id: Uuid,
    pub(crate) connectivity: StdMutex<crate::connectivity::Connectivity>,
    pub(crate) created_at: std::time::Instant,
    pub(crate) agent_runtime: crate::agent_runtime::Slot,
    pub(crate) lifecycle: crate::agent_lifecycle::State,
    pub permissions: Arc<crate::integration_permissions::Gate>,
    server: Server,
    revision: u64,
    cancelled: watch::Sender<bool>,
    transport: crate::session_transport::Transport,
    generation: Arc<AtomicU64>,
    pub(crate) monitor: crate::monitor::Channels,
    pub(crate) metrics: crate::metrics_sampler::Slot,
    pub(crate) git: crate::git::Slot,
    pub(crate) integration: crate::integration_service::Slot,
    pub(crate) queries: crate::server_queries::Queries,
    pub(crate) ports: std::sync::OnceLock<crate::query::Query<Vec<crate::ports::DiscoveredPort>>>,
}

impl ServerSession {
    pub(crate) fn lifecycle_operations(&self) -> serde_json::Value {
        match self.lifecycle.receipts.operations_snapshot(&self.server) {
            Ok(rows) => {
                let active = self
                    .lifecycle
                    .progress
                    .snapshot()
                    .filter(|p| p.stage != "ready" && p.stage != "error")
                    .and_then(|p| p.operation_id);
                serde_json::json!({"operations": rows.operations, "removals": rows.removals,
                    "helperRecoveries": rows.helper_recoveries, "activeOperationId": active,
                    "helperHistory": self.lifecycle.helper_history.snapshot()})
            }
            Err(error) => serde_json::json!({"error": error}),
        }
    }

    pub fn integration_session_id(&self) -> Uuid {
        self.session_id
    }
    fn new(
        server: Server,
        revision: u64,
        connected: Arc<tokio::sync::Notify>,
        receipts: Arc<crate::lifecycle_receipts::Store>,
    ) -> Self {
        let permissions = Arc::new(crate::integration_permissions::Gate::new(
            server.clipboard_enabled,
            server.browser_enabled,
        ));
        let (cancelled, retired) = watch::channel(false);
        let generation = Arc::new(AtomicU64::new(0));
        let transport = crate::session_transport::Transport::new(
            server.clone(),
            retired,
            generation.clone(),
            connected,
            permissions.clone(),
        );
        Self {
            session_id: Uuid::new_v4(),
            connectivity: StdMutex::default(),
            created_at: std::time::Instant::now(),
            agent_runtime: crate::agent_runtime::Slot::new(permissions.clone()),
            lifecycle: crate::agent_lifecycle::State::new(receipts),
            permissions,
            server,
            revision,
            cancelled,
            transport,
            generation,
            monitor: Default::default(),
            metrics: Default::default(),
            git: Default::default(),
            integration: Default::default(),
            ports: Default::default(),
            queries: Default::default(),
        }
    }
    pub(crate) fn transport_demand(&self) -> crate::session_transport::Demand {
        self.transport.demand()
    }
    pub fn transport_snapshot(&self) -> crate::session_transport::Snapshot {
        self.transport.snapshot()
    }
    pub(crate) async fn wait_for_transport(&self) -> Result<(), String> {
        self.transport.ready().await
    }
    pub fn retry_transport(&self) {
        self.transport.retry();
    }
    pub(crate) fn has_transport(&self) -> bool {
        self.transport.observe().is_some()
    }

    pub(crate) fn cancellation(&self) -> watch::Receiver<bool> {
        self.cancelled.subscribe()
    }

    pub fn is_revoked(&self) -> bool {
        *self.cancelled.borrow()
    }

    pub fn server(&self) -> &Server {
        &self.server
    }

    pub(crate) fn revision(&self) -> u64 {
        self.revision
    }

    fn close_queries(&self) {
        self.metrics.clear();
        self.queries.close();
        if let Some(ports) = self.ports.get() {
            ports.close();
        }
    }

    pub(crate) fn require_agent_available(&self) -> Result<(), String> {
        self.lifecycle
            .receipts
            .require_agent_available(&self.server)
    }

    async fn close(&self) {
        self.close_queries();
        self.permissions.disconnected(None);
        self.lifecycle.tasks.stop().await;
        self.integration.stop().await;
        self.git.close().await;
        self.transport.close().await;
        self.agent_runtime.reset().await;
    }

    async fn drain_agent_services(&self) -> Result<(), String> {
        // Durable removal intent already denies new demand. Do not revoke the
        // ServerSession or its transport; native SSH features retain ownership.
        self.permissions.disconnected(None);
        self.integration.pause().await;
        tokio::time::timeout(std::time::Duration::from_secs(30), async {
            self.git.drain().await;
            self.monitor.drain().await;
        })
        .await
        .map_err(|_| {
            "Agent work is still running. Removal remains pending; retry after it finishes."
                .to_owned()
        })?;
        self.agent_runtime.reset().await;
        Ok(())
    }

    /// Does not wait for authentication and cannot initiate a connection.
    pub(crate) async fn connected_lease(self: &Arc<Self>) -> Option<Lease> {
        if self.is_revoked() {
            return None;
        }
        let (connection, generation) = self.transport.observe()?;
        Some(Lease {
            runtime: StdMutex::new(None),
            worker_id: StdMutex::new(None),
            owner: Arc::downgrade(self),
            connection: connection.with_server(self.server.clone()),
            generation,
            _demand: StdMutex::new(None),
        })
    }

    pub async fn acquire(self: &Arc<Self>, server: &Server) -> Result<Lease, String> {
        if !self.server.same_connection(server) || self.is_revoked() {
            return Err(STALE.into());
        }
        let (connection, generation, demand) = self.transport.acquire().await?;
        if self.is_revoked() {
            return Err(STALE.into());
        }
        Ok(Lease {
            runtime: StdMutex::new(None),
            worker_id: StdMutex::new(None),
            owner: Arc::downgrade(self),
            connection: connection.with_server(server.clone()),
            generation,
            _demand: StdMutex::new(Some(demand)),
        })
    }
}

/// A service lease deliberately exposes no transport close operation.
pub struct Lease {
    _demand: StdMutex<Option<crate::session_transport::Demand>>,
    runtime: StdMutex<Option<crate::agent_runtime::Handle>>,
    worker_id: StdMutex<Option<Uuid>>,
    owner: Weak<ServerSession>,
    connection: ExecSession,
    generation: u64,
}
impl Lease {
    pub(crate) async fn terminal(
        &self,
        size: &mut (u32, u32),
        input: &mut tokio::sync::mpsc::Receiver<crate::terminal::Input>,
        output: &tokio::sync::mpsc::Sender<crate::terminal::Event>,
        selector: &str,
    ) -> anyhow::Result<crate::ssh::TerminalEnd> {
        self.validate().map_err(anyhow::Error::msg)?;
        self.connection
            .terminal(size, input, output, selector)
            .await
    }
    pub(crate) fn release_demand(&self) {
        self._demand.lock().unwrap().take();
    }
    pub(crate) fn generation(&self) -> u64 {
        self.generation
    }
    pub fn is_closed(&self) -> bool {
        self.owner.upgrade().is_none_or(|owner| {
            *owner.cancelled.borrow()
                || self.generation != owner.generation.load(Ordering::Acquire)
                || self.connection.is_closed()
                || self
                    .runtime
                    .lock()
                    .unwrap()
                    .as_ref()
                    .is_some_and(|runtime| runtime.is_closed())
        })
    }
    pub fn validate(&self) -> Result<(), String> {
        if self.is_closed() {
            Err(STALE.into())
        } else {
            Ok(())
        }
    }
    pub(crate) fn require_agent_available(&self) -> Result<(), String> {
        self.validate()?;
        self.owner.upgrade().ok_or(STALE)?.require_agent_available()
    }
    pub(crate) fn is_draining(&self) -> bool {
        self.runtime
            .lock()
            .unwrap()
            .as_ref()
            .is_some_and(|r| r.is_draining())
    }
    /// Used only between requests, after the caller releases its old channel.
    pub(crate) fn release_draining_runtime(&self) {
        let mut runtime = self.runtime.lock().unwrap();
        if runtime.as_ref().is_some_and(|r| r.is_draining()) {
            *runtime = None;
        }
    }
    pub(crate) async fn wait_for_upgrade(&self) {
        let runtime = self.runtime.lock().unwrap().clone();
        if let Some(runtime) = runtime {
            runtime.draining().await;
        } else {
            std::future::pending::<()>().await;
        }
    }
    pub async fn execute(&self, command: &str, input: Option<&[u8]>) -> Result<String, String> {
        self.validate()?;
        let result = self.connection.execute(command, input).await;
        self.validate()?;
        result
    }
    pub async fn launch(
        &self,
        service: agent::Service,
    ) -> Result<(crate::agent_runtime::WorkerStream, String), String> {
        self.validate()?;
        let owner = self.owner.upgrade().ok_or(STALE)?;
        owner.require_agent_available()?;
        let (name, version) = service.capability();
        let worker = owner
            .agent_runtime
            .worker(
                &self.connection,
                owner.session_id,
                self.generation,
                name,
                version,
                || {
                    self.validate()?;
                    owner.require_agent_available()
                },
            )
            .await?;
        self.validate()?;
        owner.require_agent_available()?;
        *self.runtime.lock().unwrap() = Some(worker.runtime.clone());
        *self.worker_id.lock().unwrap() = Some(worker.id);
        self.validate()?;
        let result =
            agent::launch_worker(&self.connection, service, worker.runtime.hello(), worker.id)
                .await
                .map(|(stream, output)| (worker.stream(stream), output));
        self.validate()?;
        owner.require_agent_available()?;
        result
    }
    pub async fn apply_permissions(
        &self,
        epoch: u64,
        desired: agent::control::Permissions,
    ) -> Result<(), String> {
        self.validate()?;
        let owner = self.owner.upgrade().ok_or(STALE)?;
        let runtime = self
            .runtime
            .lock()
            .unwrap()
            .as_ref()
            .map(|runtime| runtime.hello().instance_id.clone())
            .ok_or("Needle worker is not registered")?;
        let worker = self
            .worker_id
            .lock()
            .unwrap()
            .ok_or("Needle worker is not registered")?;
        if !owner.permissions.browser_enabled() {
            self.callbacks().clear();
        }
        let applied = owner
            .agent_runtime
            .permissions(&runtime, worker, epoch, desired)
            .await?;
        self.validate()?;
        if applied.permissions.revision != desired.revision {
            return Err("Needle acknowledged a different permission revision".into());
        }
        if !owner.permissions.acknowledge_for(epoch, applied)
            && owner.permissions.snapshot().desired.revision == desired.revision
        {
            return Err("Needle acknowledged permissions that were not requested".into());
        }
        Ok(())
    }
    /// Runs beside feature traffic, never behind its writes or acknowledgements.
    pub async fn watch_permissions(&self, epoch: u64, applied: impl Fn()) -> Result<(), String> {
        let owner = self.owner.upgrade().ok_or(STALE)?;
        let mut changes = owner.permissions.subscribe();
        loop {
            let desired = *changes.borrow_and_update();
            self.apply_permissions(epoch, desired).await?;
            applied();
            changes
                .changed()
                .await
                .map_err(|_| "Permission state closed")?;
        }
    }
    pub async fn install(&self, force: bool, integrations: bool) -> Result<String, String> {
        let owner = self.owner.upgrade().ok_or(STALE)?;
        let operation = owner
            .lifecycle
            .receipts
            .for_server(self.connection.server())?
            .into_iter()
            .find(|r| {
                !r.phase.terminal()
                    && r.intent.kind.is_deployment()
                    && r.intent.integrations == integrations
            })
            .map(|r| r.intent.operation_id)
            .unwrap_or_else(Uuid::new_v4);
        self.install_operation(force, integrations, operation).await
    }
    pub async fn install_operation(
        &self,
        force: bool,
        integrations: bool,
        operation_id: Uuid,
    ) -> Result<String, String> {
        self.run_lifecycle(
            operation_id,
            crate::lifecycle_task::Action::Install { integrations },
            force,
        )
        .await
    }
    async fn install_operation_inner(
        &self,
        force: bool,
        integrations: bool,
        operation_id: Uuid,
    ) -> Result<String, String> {
        self.validate()?;
        let owner = self.owner.upgrade().ok_or(STALE)?;
        let operation = owner.lifecycle.operation.lock().await;
        self.validate()?;
        let progress = owner.lifecycle.progress.attempt_for(Some(operation_id));
        let result: Result<String, String> = async {
            let deployment = crate::agent_lifecycle::manage(
                &self.connection,
                &progress,
                force,
                integrations,
                &owner.lifecycle.receipts,
                operation_id,
            )
            .await?;
            self.validate()?;
            if deployment.already_complete {
                return Ok(deployment.output);
            }
            progress.update("verifying", 0, 0, None);
            owner
                .agent_runtime
                .adopt(
                    &self.connection,
                    owner.session_id,
                    self.generation,
                    &deployment.build_sha256,
                    || self.validate(),
                )
                .await?;
            let status = crate::agent_lifecycle::check(&self.connection).await;
            self.validate()?;
            owner.lifecycle.record(self.generation, &status);
            owner
                .lifecycle
                .receipts
                .update(
                    operation_id,
                    Some(crate::lifecycle_receipts::Phase::Ready),
                    deployment.setup_notice,
                )
                .await?;
            Ok(deployment.output)
        }
        .await;
        if let Err(error) = &result
            && owner
                .lifecycle
                .receipts
                .get(operation_id)?
                .is_some_and(|r| {
                    r.server_id == self.connection.server().id
                        && r.connection
                            == crate::lifecycle_receipts::scope(self.connection.server())
                        && r.intent.kind.is_deployment()
                        && r.intent.integrations == integrations
                })
        {
            owner
                .lifecycle
                .receipts
                .update(operation_id, None, Some(error.clone()))
                .await?;
        }
        progress.finish(&result);
        drop(operation);
        result
    }
    pub async fn resume_installation(&self, id: Uuid) -> Result<String, String> {
        let owner = self.owner.upgrade().ok_or(STALE)?;
        let record = owner
            .lifecycle
            .receipts
            .get(id)?
            .ok_or("Needle operation not found")?;
        if record.intent.kind.is_deployment() {
            self.install_operation(false, record.intent.integrations, id)
                .await
        } else {
            self.configure_integrations_operation(id).await
        }
    }
    pub async fn recover_installation(&self, id: Uuid) -> Result<String, String> {
        self.run_lifecycle(id, crate::lifecycle_task::Action::Recover, false)
            .await
    }
    pub async fn remove_agent(&self, id: Uuid, build: &str) -> Result<String, String> {
        if !crate::agent_runtime::protocol::valid_build(build) {
            return Err("Invalid Needle removal build".into());
        }
        self.run_lifecycle(
            id,
            crate::lifecycle_task::Action::Remove {
                build: build
                    .as_bytes()
                    .try_into()
                    .map_err(|_| "Invalid Needle removal build")?,
            },
            false,
        )
        .await
    }

    pub async fn restore_removed_agent(&self, id: Uuid) -> Result<String, String> {
        self.run_lifecycle(id, crate::lifecycle_task::Action::RestoreRemoval, false)
            .await
    }

    pub async fn restore_standalone_helpers(
        &self,
        id: Uuid,
        build: &str,
        fingerprint: &str,
    ) -> Result<String, String> {
        let valid = crate::agent_runtime::protocol::valid_build;
        if id.is_nil() || !valid(build) || !valid(fingerprint) {
            return Err("Invalid standalone helper restoration request".into());
        }
        self.run_lifecycle(
            id,
            crate::lifecycle_task::Action::RestoreHelpers {
                build: build
                    .as_bytes()
                    .try_into()
                    .map_err(|_| "Invalid helper build")?,
                ownership: fingerprint
                    .as_bytes()
                    .try_into()
                    .map_err(|_| "Invalid helper ownership")?,
            },
            false,
        )
        .await
    }

    async fn restore_standalone_helpers_inner(
        &self,
        id: Uuid,
        build: &str,
        fingerprint: &str,
    ) -> Result<String, String> {
        self.validate()?;
        let owner = self.owner.upgrade().ok_or(STALE)?;
        let _guard = owner.lifecycle.operation.lock().await;
        self.validate()?;
        let progress = owner.lifecycle.progress.attempt_for(Some(id));
        progress.update("restoring-helpers", 0, 0, None);
        let result = crate::agent_lifecycle::helpers::recovery::restore(
            &self.connection,
            &owner.lifecycle.receipts,
            id,
            build,
            fingerprint,
            || self.validate(),
        )
        .await;
        self.validate()?;
        owner.lifecycle.helper_history.invalidate();
        progress.finish(&result);
        result
    }

    async fn removal_inner(&self, id: Uuid, build: Option<&str>) -> Result<String, String> {
        use crate::lifecycle_receipts::RemovalPhase;
        self.validate()?;
        let owner = self.owner.upgrade().ok_or(STALE)?;
        let _operation = owner.lifecycle.operation.lock().await;
        self.validate()?;
        let progress = owner.lifecycle.progress.attempt_for(Some(id));
        progress.update(
            if build.is_some() {
                "removing"
            } else {
                "restoring"
            },
            0,
            0,
            None,
        );
        let result: Result<String, String> = async {
            let record = match build {
                Some(build) => {
                    crate::agent_lifecycle::removal::prepare(
                        &self.connection,
                        &owner.lifecycle.receipts,
                        id,
                        build,
                    )
                    .await?
                }
                None => owner
                    .lifecycle
                    .receipts
                    .removal(self.connection.server(), id)?
                    .ok_or("Needle removal not found")?,
            };
            self.validate()?;
            if record.phase == RemovalPhase::Restored {
                if build.is_none() && owner.require_agent_available().is_ok() {
                    owner.integration.resume(owner.clone()).await?;
                }
                return Ok("This Needle removal was already restored or cancelled.".into());
            }
            if build.is_some() && record.phase == RemovalPhase::Removed {
                return Ok("This Needle removal already completed.".into());
            }
            if build.is_some() && record.phase == RemovalPhase::Restoring {
                return Err(
                    "Continue restoring this removal before starting another operation.".into(),
                );
            }
            // Persist the recovery decision before draining. Restart must not
            // turn an interrupted restore into a new attempt at removing files.
            let record = if build.is_none() && record.phase != RemovalPhase::Restoring {
                owner
                    .lifecycle
                    .receipts
                    .update_removal(
                        self.connection.server(),
                        id,
                        record.phase,
                        RemovalPhase::Restoring,
                        None,
                    )
                    .await?
            } else {
                record
            };
            owner.drain_agent_services().await?;
            self.validate()?;
            let message = crate::agent_lifecycle::removal::execute(
                &self.connection,
                &owner.lifecycle.receipts,
                record,
                build.is_none(),
            )
            .await?;
            self.validate()?;
            let status = crate::agent_lifecycle::check(&self.connection).await;
            self.validate()?;
            owner.lifecycle.record(self.generation, &status);
            if owner
                .lifecycle
                .receipts
                .require_agent_available(self.connection.server())
                .is_ok()
                && let Err(error) = owner.integration.resume(owner.clone()).await
            {
                return Ok(format!(
                    "{message} Integration reconnection failed: {error}"
                ));
            }
            Ok(message)
        }
        .await;
        if let Err(error) = &result
            && let Some(record) = owner
                .lifecycle
                .receipts
                .removal(self.connection.server(), id)?
        {
            owner
                .lifecycle
                .receipts
                .update_removal(
                    self.connection.server(),
                    id,
                    record.phase,
                    record.phase,
                    Some(error.clone()),
                )
                .await?;
        }
        progress.finish(&result);
        result
    }
    async fn recover_installation_inner(&self, id: Uuid) -> Result<String, String> {
        self.validate()?;
        let owner = self.owner.upgrade().ok_or(STALE)?;
        let _operation = owner.lifecycle.operation.lock().await;
        self.validate()?;
        let progress = owner.lifecycle.progress.attempt_for(Some(id));
        progress.update("recovering", 0, 0, None);
        let result = async {
            let previous =
                crate::agent_lifecycle::recover(&self.connection, &owner.lifecycle.receipts, id)
                    .await?;
            self.validate()?;
            let mut message = if owner
                .lifecycle
                .receipts
                .get(id)?
                .is_some_and(|r| !r.intent.kind.is_deployment())
            {
                "Shell setup restored. Needle was left unchanged."
            } else {
                "Unfinished Needle installation restored."
            }
            .to_owned();
            if let Some(previous) = previous {
                if previous == "missing" {
                    owner.agent_runtime.reset().await;
                } else {
                    if let Err(error) = owner
                        .agent_runtime
                        .adopt(
                            &self.connection,
                            owner.session_id,
                            self.generation,
                            &previous,
                            || self.validate(),
                        )
                        .await
                    {
                        self.validate()?;
                        // Restoration is explicit and may return to a legacy
                        // build that cannot host the current control protocol.
                        // Stop this desktop runtime instead of leaving recovery
                        // permanently unfinished after confirmed remote rollback.
                        owner.agent_runtime.reset().await;
                        message = format!(
                            "Previous installation restored. Agent services stopped: {error}"
                        );
                    }
                }
            }
            let status = crate::agent_lifecycle::check(&self.connection).await;
            self.validate()?;
            owner.lifecycle.record(self.generation, &status);
            owner
                .lifecycle
                .receipts
                .update(id, Some(crate::lifecycle_receipts::Phase::RolledBack), None)
                .await?;
            Ok::<_, String>(message)
        }
        .await;
        if let Err(error) = &result
            && owner.lifecycle.receipts.get(id)?.is_some_and(|r| {
                r.server_id == self.connection.server().id
                    && r.connection == crate::lifecycle_receipts::scope(self.connection.server())
            })
        {
            owner
                .lifecycle
                .receipts
                .update(id, None, Some(error.clone()))
                .await?;
        }
        progress.finish(&result);
        result
    }
    pub async fn configure_integrations(&self) -> Result<String, String> {
        let owner = self.owner.upgrade().ok_or(STALE)?;
        let id = owner
            .lifecycle
            .receipts
            .for_server(self.connection.server())?
            .into_iter()
            .find(|r| !r.phase.terminal() && !r.intent.kind.is_deployment())
            .map(|r| r.intent.operation_id)
            .unwrap_or_else(Uuid::new_v4);
        self.configure_integrations_operation(id).await
    }
    pub async fn configure_integrations_operation(&self, id: Uuid) -> Result<String, String> {
        self.run_lifecycle(id, crate::lifecycle_task::Action::Helpers, false)
            .await
    }
    async fn configure_integrations_operation_inner(&self, id: Uuid) -> Result<String, String> {
        self.validate()?;
        let owner = self.owner.upgrade().ok_or(STALE)?;
        let _operation = owner.lifecycle.operation.lock().await;
        self.validate()?;
        let progress = owner.lifecycle.progress.attempt_for(Some(id));
        progress.update("configuring", 0, 0, None);
        let result = crate::agent_lifecycle::configure_integrations(
            &self.connection,
            &owner.lifecycle.receipts,
            id,
        )
        .await;
        self.validate()?;
        if let Err(error) = &result
            && owner.lifecycle.receipts.get(id)?.is_some_and(|r| {
                r.server_id == self.connection.server().id
                    && r.connection == crate::lifecycle_receipts::scope(self.connection.server())
                    && !r.intent.kind.is_deployment()
            })
        {
            owner
                .lifecycle
                .receipts
                .update(id, None, Some(error.clone()))
                .await?;
        }
        progress.finish(&result);
        result
    }
    async fn run_lifecycle(
        &self,
        id: Uuid,
        action: crate::lifecycle_task::Action,
        force: bool,
    ) -> Result<String, String> {
        use crate::lifecycle_task::{Action, Request};
        self.validate()?;
        let owner = self.owner.upgrade().ok_or(STALE)?;
        // Give the owned task its own demand and a lease of this exact transport
        // generation. It cannot silently reconnect and continue old consent.
        let lease = Lease {
            owner: self.owner.clone(),
            connection: self
                .connection
                .with_server(self.connection.server().clone()),
            generation: self.generation,
            runtime: StdMutex::new(None),
            worker_id: StdMutex::new(None),
            _demand: StdMutex::new(Some(owner.transport_demand())),
        };
        owner
            .lifecycle
            .tasks
            .run(
                Request {
                    id,
                    action,
                    generation: self.generation,
                },
                owner.cancellation(),
                async move {
                    match action {
                        Action::Install { integrations } => {
                            lease.install_operation_inner(force, integrations, id).await
                        }
                        Action::Helpers => lease.configure_integrations_operation_inner(id).await,
                        Action::Recover => lease.recover_installation_inner(id).await,
                        Action::Remove { build } => {
                            lease
                                .removal_inner(
                                    id,
                                    Some(
                                        std::str::from_utf8(&build)
                                            .map_err(|_| "Invalid Needle removal build")?,
                                    ),
                                )
                                .await
                        }
                        Action::RestoreRemoval => lease.removal_inner(id, None).await,
                        Action::RestoreHelpers { build, ownership } => {
                            lease
                                .restore_standalone_helpers_inner(
                                    id,
                                    std::str::from_utf8(&build)
                                        .map_err(|_| "Invalid helper build")?,
                                    std::str::from_utf8(&ownership)
                                        .map_err(|_| "Invalid helper ownership")?,
                                )
                                .await
                        }
                        Action::Keep { index, fingerprint } => {
                            let owner = lease.owner.upgrade().ok_or(STALE)?;
                            let _guard = owner.lifecycle.operation.lock().await;
                            lease.validate()?;
                            let result = crate::agent_lifecycle::review_helpers(
                                &lease.connection,
                                &owner.lifecycle.receipts,
                                id,
                                Some((
                                    index,
                                    std::str::from_utf8(&fingerprint).map_err(|e| e.to_string())?,
                                )),
                            )
                            .await?;
                            lease.validate()?;
                            serde_json::to_string(&result).map_err(|e| e.to_string())
                        }
                    }
                },
            )
            .await
    }
    pub async fn review_helpers(
        &self,
        id: Uuid,
    ) -> Result<crate::lifecycle_receipts::protocol::HelperReview, String> {
        let owner = self.owner.upgrade().ok_or(STALE)?;
        let _guard = owner.lifecycle.operation.lock().await;
        self.validate()?;
        let result = crate::agent_lifecycle::review_helpers(
            &self.connection,
            &owner.lifecycle.receipts,
            id,
            None,
        )
        .await;
        self.validate()?;
        result
    }
    pub async fn keep_helper(
        &self,
        id: Uuid,
        index: usize,
        fingerprint: String,
    ) -> Result<crate::lifecycle_receipts::protocol::HelperReview, String> {
        if index >= 32 || !crate::agent_runtime::protocol::valid_build(&fingerprint) {
            return Err("Invalid file review decision".into());
        }
        let fingerprint = fingerprint
            .as_bytes()
            .try_into()
            .map_err(|_| "Invalid file fingerprint")?;
        let value = self
            .run_lifecycle(
                id,
                crate::lifecycle_task::Action::Keep { index, fingerprint },
                false,
            )
            .await?;
        serde_json::from_str(&value).map_err(|e| e.to_string())
    }
    pub async fn check_installation(&self) -> Result<crate::agent_lifecycle::Status, String> {
        self.check_installation_inner(false).await
    }
    pub(crate) async fn check_installation_if_due(
        &self,
    ) -> Result<crate::agent_lifecycle::Status, String> {
        self.check_installation_inner(true).await
    }
    async fn check_installation_inner(
        &self,
        only_if_due: bool,
    ) -> Result<crate::agent_lifecycle::Status, String> {
        self.validate()?;
        let owner = self.owner.upgrade().ok_or(STALE)?;
        let _operation = owner.lifecycle.operation.lock().await;
        self.validate()?;
        if only_if_due
            && !owner
                .lifecycle
                .due(self.generation, std::time::Instant::now())
        {
            return Ok(owner.lifecycle.snapshot());
        }
        let result = crate::agent_lifecycle::check(&self.connection).await;
        self.validate()?;
        owner.lifecycle.record(self.generation, &result);
        let history = match &result {
            Ok(status) => match status.installed_hash.as_deref() {
                Some(hash) => {
                    crate::agent_lifecycle::helpers::inspect(&self.connection, hash).await
                }
                None => Ok(None),
            },
            Err(error) => Err(error.clone()),
        };
        self.validate()?;
        owner.lifecycle.helper_history.record(history);
        result
    }
    pub async fn require_installed(&self) -> Result<(), String> {
        self.validate()?;
        let result = agent::require_installed(&self.connection).await;
        self.validate()?;
        result
    }
    pub fn callbacks(&self) -> crate::ssh::callback::Callbacks {
        self.connection.callbacks()
    }
}

/// Lease the published server owner without waiting for profile persistence.
pub async fn acquire(state: &Shared, expected: &Server) -> Result<Lease, String> {
    let owner = state.owner(expected.id)?;
    if !owner.server().same_connection(expected) {
        return Err(STALE.into());
    }
    owner.acquire(expected).await
}

/// Resolve the authoritative service owner without connecting.
pub async fn resolve(state: &Shared, id: Uuid) -> Result<Arc<ServerSession>, String> {
    state.owner(id)
}

#[cfg(test)]
#[path = "server_removal_tests.rs"]
mod removal_tests;

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn connection_replacement_retires_query_cache_without_affecting_other_servers() {
        let registry = Registry::default();
        let a = profile();
        let b = profile();
        let old = registry.session(&a, 1);
        let other = registry.session(&b, 1);
        let policy = crate::query::Policy {
            stale_after: std::time::Duration::from_secs(30),
            retry_after: std::time::Duration::from_secs(10),
            timeout: std::time::Duration::from_secs(1),
            observation_lease: std::time::Duration::from_secs(5),
        };
        for owner in [&old, &other] {
            assert!(
                owner
                    .ports
                    .set(crate::query::Query::new(policy, || async {
                        Ok(vec![crate::ports::DiscoveredPort {
                            port: 8080,
                            ..Default::default()
                        }])
                    }))
                    .is_ok()
            );
            crate::ports::query(owner).refresh().await.unwrap();
        }
        let new = registry.session(&a, 2);
        assert!(new.ports.get().is_none());
        assert!(crate::ports::query(&old).snapshot().data.is_none());
        assert!(crate::ports::query(&old).refresh().await.is_err());
        assert_eq!(
            crate::ports::query(&other).snapshot().data.unwrap()[0].port,
            8080
        );
        registry.shutdown().await;
    }

    fn profile() -> Server {
        serde_json::from_value(serde_json::json!({
            "id": Uuid::new_v4(), "name": "Session fixture", "sshUser": "fixture",
            "sshHost": "127.0.0.1", "sshPort": 9
        }))
        .unwrap()
    }

    #[tokio::test]
    async fn persisted_removal_blocks_services_without_revoking_the_server_session() {
        let directory = tempfile::tempdir().unwrap();
        let server = profile();
        let id = Uuid::new_v4();
        {
            let registry = Registry::new(directory.path());
            registry
                .receipts
                .begin_removal(&server, id, &"7".repeat(64))
                .await
                .unwrap();
        }
        let registry = Registry::new(directory.path());
        let owner = registry.session(&server, 0);
        assert!(!owner.is_revoked());
        assert!(owner.require_agent_available().is_err());
        assert!(
            crate::monitor::snapshot(&owner)
                .await
                .unwrap_err()
                .contains("removal")
        );
        assert!(
            owner.connected_lease().await.is_none(),
            "blocked metrics must not connect"
        );
        let snapshot = registry.lifecycle_operations();
        assert_eq!(
            snapshot[&server.id]["removals"][0]["operationId"],
            id.to_string()
        );
        let desired = owner.permissions.snapshot().desired;
        assert_eq!(desired.browser, server.browser_enabled);
        assert_eq!(desired.clipboard, server.clipboard_enabled);
    }

    #[test]
    fn consent_and_preferences_keep_owner_but_connection_revision_revokes_it() {
        let registry = Registry::default();
        let mut server = profile();
        let first = registry.session(&server, 0);
        server.agent_install_approved = true;
        server.setup_complete = true;
        server.browser_enabled = true;
        assert!(Arc::ptr_eq(&first, &registry.session(&server, 0)));
        let replacement = registry.session(&server, 1);
        assert!(!Arc::ptr_eq(&first, &replacement));
        assert!(first.is_revoked());
        assert!(!replacement.is_revoked());
    }

    #[tokio::test]
    async fn inspection_does_not_create_transport_and_retired_progress_is_isolated() {
        let registry = Registry::default();
        let server = profile();
        let old = registry.session(&server, 0);
        assert!(old.connected_lease().await.is_none());
        assert_eq!(old.generation.load(Ordering::Acquire), 0);
        let attempt = old.lifecycle.progress.attempt();
        attempt.update("uploading", 1024, 2048, None);
        assert_eq!(registry.installation_progress()[&server.id].sent, 1024);
        let next = registry.session(&server, 1);
        let replacement = next.lifecycle.progress.attempt();
        replacement.update("uploading", 512, 4096, None);
        drop(attempt);
        old.lifecycle
            .record(1, &Err("obsolete discovery failure".into()));
        assert_eq!(registry.installation_progress()[&server.id].sent, 512);
        assert!(registry.installation_progress()[&server.id].error.is_none());
        assert!(registry.installation_status()[&server.id].error.is_none());
        assert!(next.connected_lease().await.is_none());
        drop(replacement);
    }

    #[tokio::test]
    async fn revoked_owner_cannot_connect_and_shutdown_cannot_reopen() {
        let registry = Registry::default();
        let server = profile();
        let owner = registry.session(&server, 0);
        registry.invalidate(server.id);
        assert_eq!(owner.acquire(&server).await.err().unwrap(), STALE);
        let next = registry.session(&server, 1);
        registry.shutdown().await;
        assert!(next.is_revoked());
        assert_eq!(
            registry
                .session(&server, 1)
                .acquire(&server)
                .await
                .err()
                .unwrap(),
            STALE
        );
    }

    #[tokio::test]
    #[ignore = "Run with scripts/test-ssh.sh server_session::tests"]
    async fn removal_admission_keeps_existing_ssh_transport_usable() {
        let registry = Registry::default();
        let mut server = profile();
        server.ssh_port = std::env::var("NEWPORT_TEST_SSH_PORT")
            .unwrap()
            .parse()
            .unwrap();
        server.identity_file = Some(std::env::var("NEWPORT_TEST_IDENTITY").unwrap());
        let owner = registry.session(&server, 0);
        let lease = owner.acquire(&server).await.unwrap();
        assert_eq!(
            lease.execute("printf connected", None).await.unwrap(),
            "connected"
        );
        let generation = owner.generation.load(Ordering::Acquire);
        // This tests desktop admission, not remote removal. No remote files are
        // changed; the pending intent must leave native SSH channels available.
        registry
            .receipts
            .begin_removal(&server, Uuid::new_v4(), &"8".repeat(64))
            .await
            .unwrap();
        assert!(
            lease
                .launch(agent::Service::Git)
                .await
                .err()
                .unwrap()
                .contains("removal")
        );
        assert!(
            lease
                .launch(agent::Service::Monitor)
                .await
                .err()
                .unwrap()
                .contains("removal")
        );
        assert_eq!(
            lease.execute("printf still-connected", None).await.unwrap(),
            "still-connected"
        );
        let fresh = owner.acquire(&server).await.unwrap();
        assert_eq!(
            fresh.execute("printf shared", None).await.unwrap(),
            "shared"
        );
        assert_eq!(owner.generation.load(Ordering::Acquire), generation);
        assert_eq!(owner.agent_runtime.snapshot().status, "unknown");
        assert!(!owner.is_revoked());
        registry.shutdown().await;
    }

    #[tokio::test]
    #[ignore = "Run with scripts/test-ssh.sh server_session::tests"]
    async fn shared_runtime_negotiates_once_and_does_not_serialize_feature_channels() {
        let registry = Registry::default();
        let mut server = profile();
        server.ssh_port = std::env::var("NEWPORT_TEST_SSH_PORT")
            .unwrap()
            .parse()
            .unwrap();
        server.identity_file = Some(std::env::var("NEWPORT_TEST_IDENTITY").unwrap());
        let owner = registry.session(&server, 0);
        let a = owner.acquire(&server).await.unwrap();
        let b = owner.acquire(&server).await.unwrap();
        let (first, second) =
            tokio::join!(a.launch(agent::Service::Git), b.launch(agent::Service::Git));
        let first = first.unwrap();
        let second = second.unwrap();
        let before = owner.agent_runtime.snapshot();
        assert_eq!(before.status, "ready");
        assert_eq!(before.transport_generation, 1);
        let hello = before.hello.unwrap();
        assert!(crate::agent_runtime::protocol::valid_build(
            &hello.build_sha256
        ));
        assert_eq!(owner.generation.load(Ordering::Acquire), 1);
        drop(first);
        let after = owner
            .agent_runtime
            .ensure(&b.connection, owner.session_id, 1, || b.validate())
            .await
            .unwrap();
        assert_eq!(
            hello.instance_id, after.instance_id,
            "one shared control process for concurrent feature startup"
        );
        assert_eq!(b.execute("printf ok", None).await.unwrap(), "ok");
        drop(second);
        owner.agent_runtime.reset().await;
        assert!(
            a.is_closed() && b.is_closed(),
            "retiring runtime fences its worker leases"
        );
        let fresh = owner.acquire(&server).await.unwrap();
        assert_eq!(fresh.execute("printf ok", None).await.unwrap(), "ok");
        assert_eq!(
            owner.generation.load(Ordering::Acquire),
            1,
            "runtime retirement keeps the SSH transport"
        );
        registry.shutdown().await;
        assert_eq!(owner.agent_runtime.snapshot().status, "unknown");
    }

    #[tokio::test]
    #[ignore = "Run with scripts/test-ssh.sh server_session::tests"]
    async fn supervisor_recovers_lost_transport_once_for_all_services_with_backoff() {
        let registry = Registry::default();
        let mut server = profile();
        server.ssh_port = std::env::var("NEWPORT_TEST_SSH_PORT")
            .unwrap()
            .parse()
            .unwrap();
        server.identity_file = Some(std::env::var("NEWPORT_TEST_IDENTITY").unwrap());
        let owner = registry.session(&server, 0);
        let lease = owner.acquire(&server).await.unwrap();
        assert_eq!(owner.transport_snapshot().generation, 1);
        lease.connection.abort_transport();
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            while !lease.connection.is_closed() {
                tokio::time::sleep(std::time::Duration::from_millis(1)).await;
            }
        })
        .await
        .unwrap();
        tokio::time::timeout(
            std::time::Duration::from_secs(10),
            owner.wait_for_transport(),
        )
        .await
        .unwrap()
        .unwrap();
        assert!(lease.is_closed());
        assert_eq!(owner.transport_snapshot().generation, 2);
        let (a, b) = tokio::join!(owner.acquire(&server), owner.acquire(&server));
        let (a, b) = (a.unwrap(), b.unwrap());
        assert_eq!(a.generation(), b.generation());
        assert_eq!(a.execute("printf ok", None).await.unwrap(), "ok");
        let lost = tokio::time::Instant::now();
        a.connection.abort_transport();
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            while !a.connection.is_closed() {
                tokio::time::sleep(std::time::Duration::from_millis(1)).await;
            }
        })
        .await
        .unwrap();
        tokio::time::timeout(
            std::time::Duration::from_secs(12),
            owner.wait_for_transport(),
        )
        .await
        .unwrap()
        .unwrap();
        assert!(
            lost.elapsed() >= std::time::Duration::from_secs(3),
            "repeated transport drops must back off"
        );
        assert_eq!(owner.transport_snapshot().generation, 3);
        assert!(a.is_closed() && b.is_closed());
        registry.shutdown().await;
    }

    #[tokio::test]
    #[ignore = "Run with scripts/test-ssh.sh server_session::tests"]
    async fn background_inspection_only_uses_connected_transports() {
        let directory = tempfile::tempdir().unwrap();
        let mut manager =
            crate::manager::Manager::new(crate::profile::Store::for_test(directory.path().into()));
        let saved_only = profile();
        let idle = profile();
        let mut active = profile();
        active.ssh_port = std::env::var("NEWPORT_TEST_SSH_PORT")
            .unwrap()
            .parse()
            .unwrap();
        active.identity_file = Some(std::env::var("NEWPORT_TEST_IDENTITY").unwrap());
        manager.config.servers = vec![saved_only.clone(), idle.clone(), active.clone()];
        let registry = manager.sessions.clone();
        let idle_owner = registry.session(&idle, 0);
        let owner = registry.session(&active, 0);
        let state = crate::manager::share(manager);
        let worker = tokio::spawn(crate::agent_lifecycle::run(state));
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        assert_eq!(registry.all().len(), 2, "saved-only profile was not opened");
        assert!(idle_owner.connected_lease().await.is_none());
        assert_eq!(idle_owner.generation.load(Ordering::Acquire), 0);
        assert_eq!(owner.lifecycle.snapshot().checked_at, 0);
        let lease = owner.acquire(&active).await.unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(10), async {
            while owner.lifecycle.snapshot().checked_at == 0 {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("connection establishment must trigger discovery");
        assert_eq!(owner.lifecycle.snapshot().installed, Some(true));
        let previous = owner.lifecycle.snapshot().checked_at;
        registry.connected.notify_one();
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        assert_eq!(
            owner.lifecycle.snapshot().checked_at,
            previous,
            "no repeated check before hourly deadline"
        );
        owner.close().await;
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            while !lease.is_closed() {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("closed SSH transport must become observable");
        assert!(owner.connected_lease().await.is_none());
        assert_eq!(owner.transport_snapshot().generation, 1);
        registry.connected.notify_one();
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        assert_eq!(
            owner.generation.load(Ordering::Acquire),
            1,
            "the observer must not reconnect"
        );
        let _reconnected = owner.acquire(&active).await.unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(10), async {
            while owner.lifecycle.snapshot().checked_at == previous {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("new generation must be checked immediately");
        assert_eq!(owner.generation.load(Ordering::Acquire), 2);
        assert_eq!(registry.all().len(), 2);
        assert!(idle_owner.connected_lease().await.is_none());
        worker.abort();
        let _ = worker.await;
        registry.shutdown().await;
    }

    #[tokio::test]
    #[ignore = "Run with scripts/test-ssh.sh server_session::tests"]
    async fn shared_transport_survives_lease_drop_and_channel_failure() {
        let registry = Registry::default();
        let mut server = profile();
        server.ssh_port = std::env::var("NEWPORT_TEST_SSH_PORT")
            .unwrap()
            .parse()
            .unwrap();
        server.identity_file = Some(std::env::var("NEWPORT_TEST_IDENTITY").unwrap());
        let owner = registry.session(&server, 0);
        let (a, b) = tokio::join!(owner.acquire(&server), owner.acquire(&server));
        let a = a.unwrap();
        let b = b.unwrap();
        assert_eq!(
            owner.generation.load(Ordering::Acquire),
            1,
            "one authentication for concurrent consumers"
        );
        assert_eq!(a.execute("printf ok", None).await.unwrap(), "ok");
        let observed = owner.connected_lease().await.unwrap();
        let status = observed.check_installation_if_due().await.unwrap();
        assert_eq!(status.installed, Some(true));
        assert_eq!(status.update_available, None);
        let checked_at = status.checked_at;
        assert_eq!(
            observed
                .check_installation_if_due()
                .await
                .unwrap()
                .checked_at,
            checked_at
        );
        assert_eq!(owner.generation.load(Ordering::Acquire), 1);
        assert_eq!(
            registry.installation_status()[&server.id].checked_at,
            checked_at
        );
        assert!(
            a.execute("fail", None)
                .await
                .unwrap_err()
                .contains("status 7")
        );
        drop(a);
        assert_eq!(b.execute("printf ok", None).await.unwrap(), "ok");
        server.agent_install_approved = true;
        let approved = owner.acquire(&server).await.unwrap();
        assert!(approved.connection.server().agent_install_approved);
        assert_eq!(owner.generation.load(Ordering::Acquire), 1);
        registry.invalidate(server.id);
        assert!(b.is_closed());
        assert_eq!(b.execute("printf ok", None).await.unwrap_err(), STALE);
        let next = registry.session(&server, 1);
        assert_eq!(registry.installation_status()[&server.id].installed, None);
        assert!(next.connected_lease().await.is_none());
        assert_eq!(
            next.acquire(&server)
                .await
                .unwrap()
                .execute("printf ok", None)
                .await
                .unwrap(),
            "ok"
        );
        registry.shutdown().await;
        assert!(approved.is_closed());
    }
    #[tokio::test]
    #[ignore = "Run with scripts/test-ssh.sh server_session::tests"]
    async fn registration_rotation_keeps_old_channels_alive_and_bounds_overlap() {
        use crate::git::client::Client;
        let registry = Registry::default();
        let mut server = profile();
        server.ssh_port = std::env::var("NEWPORT_TEST_SSH_PORT")
            .unwrap()
            .parse()
            .unwrap();
        server.identity_file = Some(std::env::var("NEWPORT_TEST_IDENTITY").unwrap());
        let owner = registry.session(&server, 0);
        let mut held = Vec::new();
        for generation in 0..5 {
            let lease = owner.acquire(&server).await.unwrap();
            let (stream, _) = lease.launch(agent::Service::Git).await.unwrap();
            let (mut client, _) = Client::start(stream).await.unwrap();
            client.ping().await.unwrap();
            held.push((lease, client));
            assert_eq!(owner.agent_runtime.retiring_for_test(), generation);
            let snapshot = owner.agent_runtime.snapshot();
            assert_eq!(snapshot.instances.len(), generation + 1);
            assert_eq!(
                snapshot.instances.iter().filter(|i| i.retiring).count(),
                generation
            );
            assert!(
                snapshot
                    .instances
                    .iter()
                    .all(|i| i.connected && i.transport_generation == 1 && i.worker_leases == 1)
            );
            assert_eq!(snapshot.instances[0].registrations, 1);
            assert_eq!(
                snapshot.instances[0].instance_id,
                snapshot.hello.unwrap().instance_id
            );
            owner.agent_runtime.exhaust_registration_budget_for_test();
        }
        let identity = owner.agent_runtime.snapshot().hello.unwrap().instance_id;
        let next = owner.acquire(&server).await.unwrap();
        assert_eq!(
            next.launch(agent::Service::Git).await.err().unwrap(),
            crate::agent_runtime::BUSY
        );
        assert_eq!(
            owner.agent_runtime.snapshot().hello.unwrap().instance_id,
            identity
        );
        for (lease, client) in &mut held {
            assert!(!lease.is_closed());
            client.ping().await.unwrap();
        }
        // The channel itself retains its generation after the service lease ends.
        let (old_lease, mut old_channel) = held.remove(0);
        drop(old_lease);
        assert_eq!(owner.agent_runtime.retiring_for_test(), 4);
        old_channel.ping().await.unwrap();
        drop(old_channel);
        assert_eq!(owner.agent_runtime.retiring_for_test(), 3);
        let (stream, _) = next.launch(agent::Service::Git).await.unwrap();
        let (mut fresh, _) = Client::start(stream).await.unwrap();
        fresh.ping().await.unwrap();
        assert_ne!(
            owner.agent_runtime.snapshot().hello.unwrap().instance_id,
            identity
        );
        assert_eq!(owner.transport_snapshot().generation, 1);
        registry.shutdown().await;
        for (lease, mut client) in held {
            assert!(lease.is_closed());
            assert!(client.ping().await.is_err());
        }
        assert!(fresh.ping().await.is_err());
    }

    #[tokio::test]
    #[ignore = "Run with scripts/test-ssh.sh server_session::tests"]
    async fn saturated_worker_admission_is_retryable_without_runtime_or_transport_replacement() {
        let registry = Registry::default();
        let mut server = profile();
        server.ssh_port = std::env::var("NEWPORT_TEST_SSH_PORT")
            .unwrap()
            .parse()
            .unwrap();
        server.identity_file = Some(std::env::var("NEWPORT_TEST_IDENTITY").unwrap());
        let owner = registry.session(&server, 0);
        let mut held = Vec::new();
        for _ in 0..crate::agent_runtime::protocol::MAX_WORKERS {
            let lease = owner.acquire(&server).await.unwrap();
            let (stream, _) = lease.launch(agent::Service::Git).await.unwrap();
            let (client, _) = crate::git::client::Client::start(stream).await.unwrap();
            held.push((lease, client));
        }
        let identity = owner.agent_runtime.snapshot().hello.unwrap().instance_id;
        let extra = owner.acquire(&server).await.unwrap();
        assert_eq!(
            extra.launch(agent::Service::Git).await.err().unwrap(),
            crate::agent_runtime::BUSY
        );
        assert_eq!(owner.agent_runtime.snapshot().status, "ready");
        drop(held.pop());
        let (stream, _) = tokio::time::timeout(std::time::Duration::from_secs(3), async {
            loop {
                match extra.launch(agent::Service::Git).await {
                    Ok(value) => break value,
                    Err(error) if error == crate::agent_runtime::BUSY => {
                        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                    }
                    Err(error) => panic!("unexpected admission error: {error}"),
                }
            }
        })
        .await
        .unwrap();
        let (mut client, _) = crate::git::client::Client::start(stream).await.unwrap();
        client.ping().await.unwrap();
        assert_eq!(
            owner.agent_runtime.snapshot().hello.unwrap().instance_id,
            identity
        );
        assert_eq!(owner.transport_snapshot().generation, 1);
        registry.shutdown().await;
    }
}
