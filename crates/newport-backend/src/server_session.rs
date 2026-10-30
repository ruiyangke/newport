//! Application-owned server sessions. Consumers open channels on the server's
//! link and never own it. Profile revisions fence results from an obsolete
//! configuration; Needle's runtime is fenced by its own channel.
use crate::{agent, manager::Shared, model::Server, needle::Needle};
use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex as StdMutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
};
use tokio::sync::watch;
use uuid::Uuid;

pub(crate) const STALE: &str = "Server session changed. Reconnect before retrying.";

#[derive(Default)]
pub struct Registry {
    pub(crate) connections: crate::connection_state::Store,
    routing_namespace: Option<Uuid>,
    entries: Entries,
    /// Woken each time a server's link becomes ready.
    pub(crate) ready: Arc<tokio::sync::Notify>,
    stopped: AtomicBool,
    cleanup: StdMutex<tokio::task::JoinSet<()>>,
    /// Set once sleep, wake and network changes reach every server's link.
    forwarding: Arc<std::sync::OnceLock<()>>,
}

struct Published {
    profile: Server,
    owner: Arc<ServerSession>,
}
type Entries = Arc<StdMutex<HashMap<Uuid, Published>>>;
impl std::ops::Deref for Published {
    type Target = Arc<ServerSession>;
    fn deref(&self) -> &Self::Target {
        &self.owner
    }
}

impl Registry {
    pub fn new(directory: &std::path::Path) -> Self {
        Self::with_instance(directory, Uuid::new_v4())
    }
    pub(crate) fn with_instance(directory: &std::path::Path, instance: Uuid) -> Self {
        let mut registry = Self::default();
        registry.connections = crate::connection_state::Store::new(instance);
        registry.routing_namespace = match crate::clipboard::routing_identity(directory) {
            Ok(identity) => Some(identity),
            Err(error) => {
                // Keep existing connections usable if local persistence fails,
                // but never invent an identity shared with another profile.
                log::warn!(
                    "Cannot persist integration routing identity; terminal selectors will last only for this application session: {error}"
                );
                Some(Uuid::new_v4())
            }
        };
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
                self.routing_id(server.id),
                revision,
                self.ready.clone(),
                None,
                std::sync::Weak::new(),
                Arc::default(),
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
            self.routing_id(server.id),
            revision,
            self.ready.clone(),
            Some(self.connections.register(server.id, revision)),
            Arc::downgrade(&self.entries),
            self.forwarding.clone(),
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

    fn routing_id(&self, server: Uuid) -> Uuid {
        let Some(namespace) = self.routing_namespace else {
            return Uuid::new_v4();
        };
        use sha2::{Digest, Sha256};
        let mut digest = Sha256::new();
        digest.update(b"Newport integration routing v1\0");
        digest.update(namespace.as_bytes());
        digest.update(server.as_bytes());
        let mut bytes: [u8; 16] = digest.finalize()[..16].try_into().unwrap();
        // UUIDv8: application-defined SHA-256 derivation, scoped to profile/server.
        bytes[6] = (bytes[6] & 0x0f) | 0x80;
        bytes[8] = (bytes[8] & 0x3f) | 0x80;
        Uuid::from_bytes(bytes)
    }

    pub fn invalidate(&self, id: Uuid) {
        if let Some(session) = self.entries.lock().expect("server sessions").remove(&id) {
            self.connections.remove(id);
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
        self.connections.clear();
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

/// The link's error once it has stopped trying, when a request would wait
/// for Retry.
pub(crate) async fn stopped(
    mut status: watch::Receiver<newport_ssh::Status>,
) -> newport_ssh::Error {
    loop {
        if let newport_ssh::Reach::Failed(error) = &status.borrow_and_update().reach {
            return error.clone();
        }
        if status.changed().await.is_err() {
            return std::future::pending().await;
        }
    }
}

/// Passes sleep, wake and network changes to every server's link from the
/// first link on, which is made on the runtime the links run on.
fn forward_recovery(
    started: &std::sync::OnceLock<()>,
    entries: &std::sync::Weak<StdMutex<HashMap<Uuid, Published>>>,
) {
    started.get_or_init(|| {
        let (events, received) = tokio::sync::mpsc::unbounded_channel();
        crate::recovery::listen(move |event| events.send(event).is_ok());
        let entries = entries.clone();
        tokio::spawn(settle(received, move |event| {
            let Some(entries) = entries.upgrade() else {
                return false;
            };
            forward(&entries, event);
            true
        }));
    });
}

/// How long a wake or network change settles before the links check their
/// connections.
const SETTLE: std::time::Duration = std::time::Duration::from_millis(500);
/// How long an action someone waits for rides out logins that fail only for
/// now, such as an unreachable server, while the link tries again.
const UNREACHABLE_PATIENCE: std::time::Duration = std::time::Duration::from_secs(10);

/// The link's error once logins fail in a way waiting won't fix: at once when
/// trying again can't help, otherwise when they still fail after `patience`.
async fn link_failure(
    mut status: watch::Receiver<newport_ssh::Status>,
    patience: std::time::Duration,
) -> newport_ssh::Error {
    use newport_ssh::{Login, Reach, Retry};
    status.borrow_and_update();
    let deadline = tokio::time::Instant::now() + patience;
    let mut patient = true;
    loop {
        tokio::select! {
            changed = status.changed() => {
                if changed.is_err() {
                    return std::future::pending().await;
                }
            }
            () = tokio::time::sleep_until(deadline), if patient => patient = false,
        }
        let current = status.borrow_and_update();
        match (&current.reach, &current.login) {
            (Reach::Ready { .. }, _) => {}
            (Reach::Failed(error), _) => return error.clone(),
            (_, Some(Login::Failed(error))) if !patient || error.retry() != Retry::Backoff => {
                return error.clone();
            }
            _ => {}
        }
    }
}

/// Passes a sleep on at once. A wake or network change waits for [`SETTLE`],
/// and the events of a burst pass on as one: a wake when there was one.
/// Ends once `forward` returns false.
async fn settle(
    mut events: tokio::sync::mpsc::UnboundedReceiver<crate::recovery::Event>,
    mut forward: impl FnMut(crate::recovery::Event) -> bool,
) {
    use crate::recovery::Event;
    let mut held: Option<(Event, tokio::time::Instant)> = None;
    loop {
        let event = match held {
            Some((event, at)) => tokio::select! {
                next = events.recv() => next,
                _ = tokio::time::sleep_until(at) => {
                    held = None;
                    if !forward(event) {
                        return;
                    }
                    continue;
                }
            },
            None => events.recv().await,
        };
        let Some(event) = event else {
            return;
        };
        let at = held.map_or_else(|| tokio::time::Instant::now() + SETTLE, |(_, at)| at);
        held = match (event, held) {
            (Event::Sleep, _) => {
                if !forward(Event::Sleep) {
                    return;
                }
                None
            }
            (Event::NetworkChanged, Some((Event::Wake, _))) | (Event::Wake, _) => {
                Some((Event::Wake, at))
            }
            (Event::NetworkChanged, _) => Some((Event::NetworkChanged, at)),
        };
    }
}

/// Passes a recovery event to every link a server session has opened.
fn forward(entries: &StdMutex<HashMap<Uuid, Published>>, event: crate::recovery::Event) {
    use crate::recovery::Event;
    for entry in entries.lock().expect("server sessions").values() {
        let Some(link) = entry.link.get() else {
            continue;
        };
        match event {
            Event::Sleep => link.sleep(),
            Event::Wake => link.wake(),
            Event::NetworkChanged => link.network_changed(),
        }
    }
}

impl Drop for Registry {
    fn drop(&mut self) {
        self.connections.clear();
        // Tasks may temporarily retain an owner; dropping the registry must
        // still release demand rather than keeping those owners alive forever.
        for owner in self.entries.lock().expect("server sessions").values() {
            owner.permissions.disconnected(None);
            owner.cancelled.send_replace(true);
            owner.close_queries();
        }
    }
}

pub struct ServerSession {
    session_id: Uuid,
    pub(crate) agent_runtime: crate::agent_runtime::Slot,
    pub(crate) lifecycle: crate::agent_lifecycle::State,
    pub permissions: Arc<crate::integration_permissions::Gate>,
    server: Server,
    revision: u64,
    cancelled: watch::Sender<bool>,
    link: std::sync::OnceLock<newport_ssh::Link>,
    /// The registry's servers, where a jump host's session is found.
    jumps: std::sync::Weak<StdMutex<HashMap<Uuid, Published>>>,
    /// The registry's: set once recovery events reach every link.
    forwarding: Arc<std::sync::OnceLock<()>>,
    /// The link's status, readable before the link exists and without
    /// making it.
    status: watch::Sender<newport_ssh::Status>,
    publication: Option<crate::connection_state::Publisher>,
    /// How many times the link has become ready; Needle is checked again
    /// after each.
    reached: Arc<AtomicU64>,
    /// Woken each time a server's link becomes ready.
    ready: Arc<tokio::sync::Notify>,
    pub(crate) terminal_shells: crate::terminal::shell::Registry,
    pub(crate) monitor: crate::monitor::Channels,
    pub(crate) metrics: crate::metrics_sampler::Slot,
    pub(crate) git: crate::git::Slot,
    pub(crate) graphical: crate::graphical_session::Slot,
    pub(crate) integration: crate::integration_service::Slot,
    pub(crate) queries: crate::server_queries::Queries,
    pub(crate) files: crate::files::vfs::Vfs,
    pub(crate) ports: std::sync::OnceLock<crate::query::Query<Vec<crate::ports::DiscoveredPort>>>,
}

impl ServerSession {
    /// A launch found the runtime on another build than this Newport bundles.
    /// Asks for the replacement under the automatic update's rules; a refusal
    /// or failure leaves the launch on the existing runtime, and the failure
    /// shows in the server's lifecycle status.
    async fn update_stale_runtime(self: &Arc<Self>, runtime_build: &str) -> bool {
        let status = match self.check_installation_if_due().await {
            Ok(status) => status,
            Err(_) => self.lifecycle.snapshot(),
        };
        if !self
            .lifecycle
            .runtime_update_allowed(&status, runtime_build, &self.server)
        {
            return false;
        }
        match self.update_needle_for_feature().await {
            Ok(()) => true,
            Err(error) => {
                log::warn!("Needle update before a Remote Apps launch did not complete: {error:?}");
                false
            }
        }
    }
    /// One display per verified runtime generation, owned below presentation.
    /// The caller must finish account/setup authorization before acquiring GUI
    /// demand. A terminal calls it only to join a display that is already
    /// running, bounded by `terminal::display::JOIN_WAIT`, or for a GUI app
    /// launched in it.
    pub async fn acquire_graphical(
        self: &Arc<Self>,
        demand: crate::graphical_session::Demand,
        factory: Arc<dyn crate::graphical_session::Factory>,
    ) -> Result<crate::graphical_session::Lease, String> {
        use crate::graphical_session::{Identity, Prepared, Runtime};
        factory.validate()?;
        let _admission = self.graphical.admission().await?;
        factory.validate()?;
        let needle = Arc::new(self.needle(&self.server)?);
        let ensure = || {
            self.agent_runtime
                .ensure(self.link(), self.session_id, || needle.validate())
        };
        let mut hello = ensure().await?;
        if self.update_stale_runtime(&hello.build_sha256).await {
            hello = ensure().await?;
        }
        hello
            .require("remote-apps", 1)
            .map_err(|error| error.to_string())?;
        let runtime = Runtime {
            instance: Uuid::parse_str(&hello.instance_id)
                .map_err(|_| "Invalid runtime identity")?,
            build_sha256: hello.build_sha256.clone(),
            transport_generation: self
                .agent_runtime
                .generation_of(&hello.instance_id)
                .ok_or("Needle runtime changed before display preparation")?,
        };
        let session = self.session_id;
        let bus_policy = factory.terminal_bus_policy();
        hello
            .require("gui-launch", 4)
            .map_err(|error| error.to_string())?;
        let expected = runtime.clone();
        let attaching = factory.clone();
        let owner = self.clone();
        let display = self
            .graphical
            .acquire_policy(
                runtime,
                bus_policy,
                demand,
                move |display_generation| async move {
                    let (channel, worker) = needle
                        .launch_channel(agent::Service::RemoteApps {
                            arguments: vec!["--display".into(), "wayland".into()],
                        })
                        .await?;
                    let admitted = worker.runtime.hello();
                    if admitted.instance_id != expected.instance.to_string()
                        || admitted.build_sha256 != expected.build_sha256
                    {
                        return Err(
                        "Agent runtime changed during graphical preparation; retry the new session"
                            .into(),
                    );
                    }
                    let identity = Identity {
                        runtime: expected,
                        session,
                        worker: worker.id,
                        display_generation,
                    };
                    let attached = attaching
                        .attach(
                            crate::graphical_session::transport::Channel::attach(channel),
                            identity.clone(),
                        )
                        .await?;
                    attached.environment.validate()?;
                    attached
                        .terminal_bus
                        .validate(bus_policy, &attached.environment)?;
                    needle.validate()?;
                    // A ready display serves native selections before a GUI window
                    // or host export grant exists. Provider assignment is not
                    // permission to write the host clipboard.
                    owner
                        .agent_runtime
                        .clipboard_route(&identity, false, false)
                        .await?;
                    needle.validate()?;
                    Ok(Prepared {
                        identity,
                        attached,
                        fence: Box::new(GraphicalFence {
                            needle,
                            _worker: worker,
                        }),
                    })
                },
            )
            .await?;
        factory.validate()?;
        Ok(display)
    }
    pub fn graphical_sessions(&self) -> Vec<crate::graphical_session::Snapshot> {
        self.graphical.snapshot()
    }

    /// Submit an authorized application request on the shared display. The
    /// adapter publishes pending application demand before this short launch
    /// lease is released. Observing or abandoning the result opens no channel.
    pub async fn launch_graphical(
        self: &Arc<Self>,
        target: crate::graphical_session::launch::Target,
        factory: Arc<dyn crate::graphical_session::Factory>,
    ) -> Result<crate::graphical_session::launch::Pending, String> {
        let lease = self
            .acquire_graphical(crate::graphical_session::Demand::Launch, factory)
            .await?;
        lease.launch(target)
    }

    pub async fn launch_application(
        self: &Arc<Self>,
        target: crate::graphical_session::launch::Target,
        factory: Arc<dyn crate::graphical_session::Factory>,
    ) -> Result<crate::graphical_session::launch::Started, String> {
        let lease = self
            .acquire_graphical(crate::graphical_session::Demand::Launch, factory)
            .await?;
        lease.launch_application(target)
    }
    pub fn application_state(
        &self,
        application: &crate::graphical_session::launch::Application,
    ) -> Result<crate::graphical_session::launch::State, String> {
        self.graphical.application_state(application)
    }
    pub fn application_ending(
        &self,
        application: &crate::graphical_session::launch::Application,
    ) -> Result<Option<crate::graphical_session::launch::Ending>, String> {
        self.graphical.application_ending(application)
    }
    pub fn application_notice(
        &self,
        application: &crate::graphical_session::launch::Application,
    ) -> Result<Option<crate::graphical_session::launch::Notice>, String> {
        self.graphical.application_notice(application)
    }
    pub fn stop_application(
        &self,
        application: &crate::graphical_session::launch::Application,
    ) -> Result<bool, String> {
        self.graphical.stop_application(application)
    }

    pub fn integration_session_id(&self) -> Uuid {
        self.session_id
    }
    fn new(
        server: Server,
        session_id: Uuid,
        revision: u64,
        ready: Arc<tokio::sync::Notify>,
        publication: Option<crate::connection_state::Publisher>,
        jumps: std::sync::Weak<StdMutex<HashMap<Uuid, Published>>>,
        forwarding: Arc<std::sync::OnceLock<()>>,
    ) -> Self {
        let permissions = Arc::new(crate::integration_permissions::Gate::new(
            server.clipboard_enabled,
            server.browser_enabled,
        ));
        let cancelled = watch::channel(false).0;
        Self {
            session_id,
            agent_runtime: crate::agent_runtime::Slot::new(permissions.clone()),
            lifecycle: crate::agent_lifecycle::State::default(),
            permissions,
            server,
            revision,
            cancelled,
            link: std::sync::OnceLock::new(),
            jumps,
            forwarding,
            status: watch::channel(Default::default()).0,
            publication,
            reached: Arc::default(),
            ready,
            monitor: Default::default(),
            metrics: Default::default(),
            git: Default::default(),
            graphical: Default::default(),
            terminal_shells: Default::default(),
            integration: Default::default(),
            ports: Default::default(),
            queries: Default::default(),
            files: Default::default(),
        }
    }
    /// Everything Newport has open to the server, made on first use, which
    /// must be on the Tokio runtime. A session can be made outside it.
    pub fn link(&self) -> &newport_ssh::Link {
        self.link_behind(&mut Vec::new())
    }
    /// The link, made after the links of the servers in `behind`, which
    /// jump through this one. A chain that leads back to one of them fails
    /// rather than waiting for its own link.
    fn link_behind(&self, behind: &mut Vec<Uuid>) -> &newport_ssh::Link {
        self.link.get_or_init(|| {
            forward_recovery(&self.forwarding, &self.jumps);
            let link = crate::ssh::link(&self.server, |id| self.jump_link(id, behind));
            if crate::recovery::subscribe().sleeping() {
                link.sleep();
            }
            follow(
                &link,
                Follower {
                    status: self.status.clone(),
                    publication: self.publication.clone(),
                    reached: self.reached.clone(),
                    ready: self.ready.clone(),
                },
            );
            link
        })
    }
    /// The link of the saved server this one jumps through, shared with every
    /// other server behind it.
    fn jump_link(
        &self,
        id: Uuid,
        behind: &mut Vec<Uuid>,
    ) -> Result<newport_ssh::Link, newport_ssh::Error> {
        let unsaved = || newport_ssh::Error::InvalidJump("it is no longer saved".into());
        let looped = || newport_ssh::Error::InvalidJump("its jump hosts lead back to it".into());
        behind.push(self.server.id);
        let jump = {
            let entries = self.jumps.upgrade().ok_or_else(unsaved)?;
            let entries = entries.lock().expect("server sessions");
            // The chain as saved now, so two links never wait for each other.
            let mut next = Some(id);
            let mut chain = Vec::new();
            while let Some(id) = next {
                if behind.contains(&id) || chain.contains(&id) {
                    return Err(looped());
                }
                chain.push(id);
                next = match &entries.get(&id).ok_or_else(unsaved)?.owner.server.proxy {
                    Some(crate::model::Proxy::Jump(next)) => Some(*next),
                    _ => None,
                };
            }
            entries[&id].owner.clone()
        };
        Ok(jump.link_behind(behind).clone())
    }
    /// The link's status, which stays idle until something uses the link.
    pub(crate) fn status(&self) -> watch::Receiver<newport_ssh::Status> {
        self.status.subscribe()
    }
    #[cfg(test)]
    pub(crate) fn set_status_for_test(&self, status: newport_ssh::Status) {
        self.status.send_replace(status);
    }
    /// The link's status as the app's pages read it.
    pub(crate) fn transport(&self) -> crate::connection_state::Transport {
        crate::connection_state::Transport::new(&self.status.borrow(), self.reached())
    }
    /// Whether the link has a ready connection now. Never opens one.
    pub(crate) fn link_ready(&self) -> bool {
        self.link.get().is_some_and(|link| {
            matches!(
                link.status().borrow().reach,
                newport_ssh::Reach::Ready { .. }
            )
        })
    }
    /// How many times the link has become ready.
    pub(crate) fn reached(&self) -> u64 {
        self.reached.load(Ordering::Acquire)
    }
    /// Try again now, for an action that needs the server: clears a login
    /// failure that stopped automatic attempts. A login waiting for approval
    /// keeps its prompt.
    pub fn retry(&self) {
        self.link().retry();
    }
    /// The person asked to connect again: as [`ServerSession::retry`], and a
    /// login waiting for approval, whose prompt may be hidden, starts afresh.
    pub fn retry_fresh(&self) {
        self.link().retry_fresh();
    }
    /// The person asked to connect again: logs in afresh and waits until the
    /// link reaches the server, or fails with the error that stops it again.
    /// The link takes the retry first, so the failure it clears is never
    /// reported, and the status shows the retry once this returns.
    pub(crate) async fn reconnect(&self) -> Result<(), newport_ssh::Error> {
        self.retry_fresh();
        self.link()
            .open_unless_stopped(newport_ssh::Open::Session)
            .await
            .map(drop)
    }
    /// Runs an action someone waits for, but ends it with the link's error
    /// once logins fail in a way waiting won't fix: the link would keep
    /// trying, and the person should hear why the server can't be reached.
    pub(crate) async fn unless_unreachable<T, E: From<newport_ssh::Error>>(
        &self,
        work: impl std::future::Future<Output = Result<T, E>>,
    ) -> Result<T, E> {
        tokio::select! {
            result = work => result,
            error = link_failure(self.link().status(), UNREACHABLE_PATIENCE) => Err(error.into()),
        }
    }
    /// Browser logins relayed from the Mac to the server's loopback.
    pub fn callbacks(&self) -> crate::ssh::callback::Callbacks {
        crate::ssh::callback::Callbacks::new(self.link().clone())
    }
    /// Needle on the server, for a consumer of `server`'s current
    /// configuration.
    pub fn needle(self: &Arc<Self>, server: &Server) -> Result<Needle, String> {
        if !self.server.same_connection(server) || self.is_revoked() {
            return Err(STALE.into());
        }
        Ok(Needle::new(self))
    }
    /// Whether anything has opened the link yet.
    #[cfg(test)]
    pub(crate) fn has_link(&self) -> bool {
        self.link.get().is_some()
    }

    pub(crate) fn cancellation(&self) -> watch::Receiver<bool> {
        self.cancelled.subscribe()
    }

    pub fn is_revoked(&self) -> bool {
        *self.cancelled.borrow()
    }

    fn validate(&self) -> Result<(), crate::agent_runtime::Error> {
        if self.is_revoked() {
            Err(crate::agent_runtime::Error::Stale)
        } else {
            Ok(())
        }
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
        self.files.listings.close();
        if let Some(ports) = self.ports.get() {
            ports.close();
        }
    }

    async fn close(&self) {
        self.graphical.close();
        self.close_queries();
        self.permissions.disconnected(None);
        self.lifecycle.tasks.stop().await;
        self.integration.stop().await;
        self.git.close().await;
        self.agent_runtime.reset().await;
    }

    /// Stops everything that runs on Needle, without waiting for it to
    /// finish: open terminals keep their display, so waiting never ends.
    async fn stop_agent_services(&self) {
        // Don't revoke the ServerSession or its link; native SSH features
        // keep using them.
        self.permissions.disconnected(None);
        self.integration.stop().await;
        self.git.close().await;
        self.monitor.drain().await;
        self.graphical.end_all();
        self.agent_runtime.reset().await;
    }

    #[cfg(test)]
    pub(crate) async fn adopt_build_for_test(&self, hash: &str) -> Result<(), String> {
        self.validate()?;
        crate::agent_runtime::verify_candidate(self.link(), hash).await?;
        Ok(self
            .agent_runtime
            .adopt(self.link(), self.session_id, hash, || self.validate())
            .await?)
    }
    /// Inspect the exact installed product without activating or replacing it.
    pub(crate) async fn inspect_product(
        &self,
        hash: &str,
    ) -> Result<newport_release::agent_product::Product, String> {
        self.validate()?;
        let product = crate::agent_runtime::inspect_product(self.link(), hash).await?;
        self.validate()?;
        Ok(product)
    }
    /// Installs or updates Needle, as the person asked, under `server`'s
    /// current approval.
    pub async fn install(
        self: &Arc<Self>,
        server: &Server,
        integrations: bool,
    ) -> Result<String, String> {
        self.run_lifecycle(
            server,
            crate::lifecycle_task::Action::Install { integrations },
        )
        .await
    }
    /// Enabling sharing also opts into its command links and shell setup. Keep
    /// this separate from deployment so live workers retain their installed build.
    pub(crate) async fn setup_integrations(
        self: &Arc<Self>,
        server: &Server,
    ) -> Result<(), String> {
        let output = self
            .run_lifecycle(server, crate::lifecycle_task::Action::SetupIntegrations)
            .await?;
        if let Some(notice) = crate::agent_lifecycle::setup_notice(&output) {
            log::warn!("{notice}");
        }
        Ok(())
    }
    /// Uninstalls Needle: nothing of it stays on the server. Final.
    pub async fn uninstall(self: &Arc<Self>, server: &Server) -> Result<String, String> {
        self.run_lifecycle(server, crate::lifecycle_task::Action::Uninstall)
            .await
    }
    async fn install_inner(
        &self,
        server: &Server,
        integrations: bool,
        trigger: crate::agent_lifecycle::Trigger,
    ) -> Result<String, String> {
        use crate::agent_lifecycle::Trigger;
        self.validate()?;
        let _operation = self.lifecycle.operation.lock().await;
        self.validate()?;
        if trigger != Trigger::Manual {
            // A feature waiting on a required update hears why it did not run.
            let skip = |message: &str| match trigger {
                Trigger::Required => Err(message.to_owned()),
                _ => {
                    self.lifecycle.skipped_automatic_update();
                    Ok(message.to_owned())
                }
            };
            if !server.agent_install_approved {
                return skip(
                    "Needle installation requires approval. Open Overview to set up this server.",
                );
            }
            // Recheck under the lifecycle lock: a manual install, an
            // uninstall or another desktop may have changed it since.
            let status = crate::agent_lifecycle::check(self.link()).await;
            self.validate()?;
            self.lifecycle.record(self.reached(), &status);
            if !trigger.allows(&status?) {
                return skip("The installed Needle can’t be replaced automatically.");
            }
        }
        let progress = self.lifecycle.progress.attempt();
        let result: Result<String, String> = async {
            let deployment = crate::agent_lifecycle::manage(
                self.link(),
                server,
                &progress,
                integrations,
                trigger,
            )
            .await?;
            self.validate()?;
            progress.update("verifying", 0, 0, None);
            self.agent_runtime
                .adopt(
                    self.link(),
                    self.session_id,
                    &deployment.build_sha256,
                    || self.validate(),
                )
                .await?;
            let status = crate::agent_lifecycle::check(self.link()).await;
            self.validate()?;
            self.lifecycle.record(self.reached(), &status);
            Ok(match deployment.setup_notice {
                Some(notice) => format!("{} {notice}", deployment.output),
                None => deployment.output,
            })
        }
        .await;
        progress.finish(&result);
        result
    }
    async fn setup_integrations_inner(&self) -> Result<String, String> {
        self.validate()?;
        let _operation = self.lifecycle.operation.lock().await;
        self.validate()?;
        agent::require_installed(self.link()).await?;
        let output = crate::remote::command::stdout(
            self.link(),
            "\"$HOME/.local/bin/newport-agent\" install 2>&1",
            None,
        )
        .await;
        self.validate()?;
        Ok(output?)
    }
    async fn uninstall_inner(&self) -> Result<String, String> {
        self.validate()?;
        let _operation = self.lifecycle.operation.lock().await;
        // Keep new display preparation out of the drain/removal interval.
        // Existing leases may still release their demand while this is held.
        let _graphical_maintenance = self.graphical.maintenance().await;
        self.validate()?;
        let progress = self.lifecycle.progress.attempt();
        progress.update("uninstalling", 0, 0, None);
        let result: Result<String, String> = async {
            self.stop_agent_services().await;
            self.validate()?;
            crate::agent_lifecycle::uninstall(self.link()).await?;
            self.validate()?;
            let status = crate::agent_lifecycle::check(self.link()).await;
            self.validate()?;
            self.lifecycle.record(self.reached(), &status);
            Ok("Needle uninstalled. SSH terminals and forwarding remain available.".into())
        }
        .await;
        progress.finish(&result);
        result
    }
    /// Runs one lifecycle action as the session's own task: dropping the
    /// wait doesn't stop it, and retiring the session does.
    async fn run_lifecycle(
        self: &Arc<Self>,
        server: &Server,
        action: crate::lifecycle_task::Action,
    ) -> Result<String, String> {
        use crate::{
            agent_lifecycle::Trigger,
            lifecycle_task::{Action, Request},
        };
        if !self.server.same_connection(server) {
            return Err(STALE.into());
        }
        self.validate()?;
        let owner = self.clone();
        let server = server.clone();
        self.lifecycle
            .tasks
            .run(
                Request {
                    action,
                    generation: self.reached(),
                },
                self.cancellation(),
                async move {
                    let work = async {
                        match action {
                            Action::AutoUpdate => {
                                owner
                                    .install_inner(&server, false, Trigger::Automatic)
                                    .await
                            }
                            Action::RequiredUpdate => {
                                owner.install_inner(&server, false, Trigger::Required).await
                            }
                            Action::Install { integrations } => {
                                owner
                                    .install_inner(&server, integrations, Trigger::Manual)
                                    .await
                            }
                            Action::SetupIntegrations => owner.setup_integrations_inner().await,
                            Action::Uninstall => owner.uninstall_inner().await,
                        }
                    };
                    let work = async { work.await.map_err(crate::remote::Failure::from) };
                    Ok(owner.unless_unreachable(work).await?)
                },
            )
            .await
    }
    pub async fn check_installation(&self) -> Result<crate::agent_lifecycle::Status, String> {
        self.check_installation_inner(false).await
    }
    pub(crate) async fn automatically_update_needle(
        self: &Arc<Self>,
        status: &crate::agent_lifecycle::Status,
    ) -> Result<(), String> {
        self.validate()?;
        if !status.automatic_update_available()
            || !self.lifecycle.automatic_update_unblocked(&self.server)
            || !self
                .lifecycle
                .claim_automatic_update(std::time::Instant::now())
        {
            return Ok(());
        }
        let result = self
            .run_lifecycle(&self.server, crate::lifecycle_task::Action::AutoUpdate)
            .await
            .map(|_| ());
        self.lifecycle
            .finish_automatic_update(std::time::Instant::now(), status, &result);
        result
    }
    /// Replaces a Needle that is too old for a feature this desktop needs,
    /// unless an update is already running or recently failed. Approval and
    /// the replacement rules of a manual install still apply.
    pub(crate) async fn update_needle_for_feature(
        self: &Arc<Self>,
    ) -> Result<(), crate::agent_lifecycle::RequiredUpdate> {
        use crate::agent_lifecycle::RequiredUpdate;
        let failed = |error: String| RequiredUpdate::Failed {
            error,
            at: std::time::Instant::now(),
        };
        self.validate().map_err(|error| failed(error.into()))?;
        if !self.server.agent_install_approved {
            return Err(failed(
                "Needle installation requires approval. Open Overview to set up this server."
                    .into(),
            ));
        }
        self.lifecycle
            .claim_required_update(std::time::Instant::now())?;
        let claim = self.lifecycle.required_guard();
        let before = self.lifecycle.snapshot();
        let result = self
            .run_lifecycle(&self.server, crate::lifecycle_task::Action::RequiredUpdate)
            .await;
        claim.finish(&result);
        let outcome = result.as_ref().map(|_| ()).map_err(Clone::clone);
        self.lifecycle.note_update(&before, &outcome);
        result.map(|_| ()).map_err(failed)
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
        let _operation = self.lifecycle.operation.lock().await;
        self.validate()?;
        if only_if_due
            && !self
                .lifecycle
                .due(self.reached(), std::time::Instant::now())
        {
            return Ok(self.lifecycle.snapshot());
        }
        let result = self
            .unless_unreachable(async {
                crate::agent_lifecycle::check(self.link())
                    .await
                    .map_err(crate::remote::Failure::from)
            })
            .await
            .map_err(String::from);
        self.validate()?;
        self.lifecycle.checked(self.reached(), result)
    }
    pub async fn require_installed(&self) -> Result<(), String> {
        self.validate()?;
        let result = self
            .unless_unreachable(async {
                Ok::<_, crate::remote::Failure>(agent::require_installed(self.link()).await)
            })
            .await?;
        self.validate()?;
        if result == Err(crate::agent_runtime::Error::NotInstalled) {
            self.lifecycle.found_missing(self.reached());
        }
        Ok(result?)
    }
}

/// Where a server's link status goes.
struct Follower {
    status: watch::Sender<newport_ssh::Status>,
    publication: Option<crate::connection_state::Publisher>,
    reached: Arc<AtomicU64>,
    /// Woken each time the link becomes ready.
    ready: Arc<tokio::sync::Notify>,
}

/// Mirrors the link's status, publishes it to the app, and counts each time
/// the link becomes ready.
fn follow(link: &newport_ssh::Link, to: Follower) {
    let mut status = link.status();
    tokio::spawn(async move {
        let mut was_ready = false;
        loop {
            let current = status.borrow_and_update().clone();
            let is_ready = matches!(current.reach, newport_ssh::Reach::Ready { .. });
            if is_ready && !was_ready {
                to.reached.fetch_add(1, Ordering::AcqRel);
                to.ready.notify_one();
            }
            was_ready = is_ready;
            if let Some(publication) = &to.publication {
                publication.status(&current, to.reached.load(Ordering::Acquire));
            }
            to.status.send_replace(current);
            if status.changed().await.is_err() {
                return;
            }
        }
    });
}

struct GraphicalFence {
    needle: Arc<Needle>,
    _worker: crate::agent_runtime::Worker,
}
impl crate::graphical_session::Fence for GraphicalFence {
    fn validate(&self) -> Result<(), String> {
        Ok(self.needle.validate()?)
    }
}

/// Resolve the authoritative service owner without connecting.
pub async fn resolve(state: &Shared, id: Uuid) -> Result<Arc<ServerSession>, String> {
    state.owner(id)
}

#[cfg(test)]
mod tests {
    use super::*;

    mod link_failure {
        use super::super::link_failure;
        use newport_ssh::{Error, Login, Reach, Status};
        use std::time::Duration;
        use tokio::{sync::watch, task::JoinHandle, time::advance};

        const PATIENCE: Duration = Duration::from_secs(10);

        fn status(reach: Reach, login: Option<Login>) -> Status {
            Status {
                reach,
                login,
                waiting: 1,
            }
        }
        fn failed(error: Error) -> Status {
            status(Reach::Connecting, Some(Login::Failed(error)))
        }
        async fn watching() -> (watch::Sender<Status>, JoinHandle<Error>) {
            let (sender, receiver) = watch::channel(status(Reach::Connecting, None));
            let watcher = tokio::spawn(link_failure(receiver, PATIENCE));
            tokio::task::yield_now().await;
            (sender, watcher)
        }
        async fn settle() {
            for _ in 0..4 {
                tokio::task::yield_now().await;
            }
        }

        #[tokio::test(start_paused = true)]
        async fn an_unreachable_server_is_waited_on_while_the_link_tries_again() {
            let (sender, watcher) = watching().await;
            sender
                .send(failed(Error::Unreachable("No route to host".into())))
                .unwrap();
            settle().await;
            assert!(!watcher.is_finished());
            sender
                .send(status(Reach::Connecting, Some(Login::InProgress)))
                .unwrap();
            sender
                .send(status(Reach::Ready { connections: 1 }, None))
                .unwrap();
            advance(PATIENCE * 2).await;
            settle().await;
            assert!(!watcher.is_finished());
            watcher.abort();
        }

        #[tokio::test(start_paused = true)]
        async fn a_server_still_unreachable_after_the_patience_ends_the_wait() {
            let (sender, watcher) = watching().await;
            sender.send(failed(Error::TimedOut)).unwrap();
            advance(PATIENCE - Duration::from_secs(1)).await;
            settle().await;
            assert!(!watcher.is_finished());
            advance(Duration::from_secs(1)).await;
            assert_eq!(watcher.await.unwrap(), Error::TimedOut);
        }

        #[tokio::test(start_paused = true)]
        async fn failures_retrying_cannot_fix_end_the_wait_at_once() {
            for end in [
                failed(Error::AuthRefused),
                status(Reach::Failed(Error::SessionsDisabled), None),
            ] {
                let expected = match (&end.reach, &end.login) {
                    (Reach::Failed(error), _) | (_, Some(Login::Failed(error))) => error.clone(),
                    _ => unreachable!(),
                };
                let (sender, watcher) = watching().await;
                sender.send(end).unwrap();
                assert_eq!(watcher.await.unwrap(), expected);
            }
        }
    }

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

    #[tokio::test]
    async fn sleep_and_wake_reach_a_servers_open_link() {
        use crate::recovery::Event;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let mut server = profile();
        server.ssh_port = listener.local_addr().unwrap().port();
        let registry = Registry::default();
        let link = registry.session(&server, 0).link().clone();
        // The registry's own forwarding, without the process-wide events
        // other tests report.
        forward(&registry.entries, Event::Sleep);
        let open = tokio::spawn({
            let link = link.clone();
            async move { link.open(newport_ssh::Open::Session).await }
        });
        let dialed =
            tokio::time::timeout(std::time::Duration::from_millis(300), listener.accept()).await;
        forward(&registry.entries, Event::Wake);
        assert!(dialed.is_err(), "a sleeping link must not dial");
        tokio::time::timeout(std::time::Duration::from_secs(5), listener.accept())
            .await
            .expect("the woken link to dial")
            .unwrap();
        open.abort();
    }

    #[tokio::test(start_paused = true)]
    async fn a_burst_of_wakes_and_network_changes_reaches_the_links_once_settled() {
        use crate::recovery::Event;
        use std::time::Duration;
        let (events, received) = tokio::sync::mpsc::unbounded_channel();
        let seen = Arc::new(StdMutex::new(Vec::new()));
        let started = tokio::time::Instant::now();
        tokio::spawn(settle(received, {
            let seen = seen.clone();
            move |event| {
                seen.lock()
                    .unwrap()
                    .push((format!("{event:?}"), started.elapsed()));
                true
            }
        }));
        let seen = || seen.lock().unwrap().clone();
        events.send(Event::NetworkChanged).unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;
        events.send(Event::Wake).unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;
        events.send(Event::NetworkChanged).unwrap();
        tokio::time::sleep(Duration::from_millis(250)).await;
        assert!(seen().is_empty(), "nothing passes before the burst settles");
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(seen(), [("Wake".to_owned(), SETTLE)]);

        // A sleep passes at once, and the change held before it is dropped.
        events.send(Event::NetworkChanged).unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;
        events.send(Event::Sleep).unwrap();
        tokio::time::sleep(Duration::from_secs(2)).await;
        let after = seen();
        assert_eq!(after.len(), 2, "{after:?}");
        assert_eq!(after[1].0, "Sleep");
    }

    /// An SSH agent with the fixture agent's keys that never answers a sign
    /// request, like one waiting for the person to approve it. Counts the
    /// sign requests it holds.
    async fn unapproving_agent(
        socket: &std::path::Path,
        upstream: std::path::PathBuf,
    ) -> Arc<AtomicU64> {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        async fn message(stream: &mut tokio::net::UnixStream) -> Option<Vec<u8>> {
            let length = stream.read_u32().await.ok()?;
            let mut body = vec![0; length as usize];
            stream.read_exact(&mut body).await.ok()?;
            Some(body)
        }
        let signs = Arc::new(AtomicU64::new(0));
        let listener = tokio::net::UnixListener::bind(socket).unwrap();
        let counted = signs.clone();
        tokio::spawn(async move {
            while let Ok((mut client, _)) = listener.accept().await {
                let upstream = upstream.clone();
                let signs = counted.clone();
                tokio::spawn(async move {
                    while let Some(request) = message(&mut client).await {
                        let reply = match request.first() {
                            // SSH_AGENTC_REQUEST_IDENTITIES
                            Some(11) => {
                                let mut agent =
                                    tokio::net::UnixStream::connect(&upstream).await.unwrap();
                                agent.write_u32(request.len() as u32).await.unwrap();
                                agent.write_all(&request).await.unwrap();
                                message(&mut agent).await.unwrap()
                            }
                            // SSH_AGENTC_SIGN_REQUEST
                            Some(13) => {
                                signs.fetch_add(1, Ordering::AcqRel);
                                std::future::pending::<()>().await;
                                unreachable!()
                            }
                            // SSH_AGENT_FAILURE
                            _ => vec![5],
                        };
                        client.write_u32(reply.len() as u32).await.unwrap();
                        client.write_all(&reply).await.unwrap();
                    }
                });
            }
        });
        signs
    }

    #[tokio::test]
    #[ignore = "Run with cargo xtask test ssh -- server_session::tests"]
    async fn a_routine_retry_keeps_a_login_waiting_for_approval_and_a_fresh_one_replaces_it() {
        use std::time::Duration;
        let directory = tempfile::Builder::new().tempdir_in("/tmp").unwrap();
        let upstream = std::env::var_os("SSH_AUTH_SOCK").unwrap();
        let socket = directory.path().join("agent.sock");
        let signs = unapproving_agent(&socket, upstream.clone().into()).await;
        struct Restore(std::ffi::OsString);
        impl Drop for Restore {
            fn drop(&mut self) {
                unsafe { std::env::set_var("SSH_AUTH_SOCK", &self.0) };
            }
        }
        let _restore = Restore(upstream);
        // The fixture suite runs one test at a time.
        unsafe { std::env::set_var("SSH_AUTH_SOCK", &socket) };
        let mut server = profile();
        server.ssh_port = std::env::var("NEWPORT_TEST_SSH_PORT")
            .unwrap()
            .parse()
            .unwrap();
        let registry = Registry::default();
        let owner = registry.session(&server, 0);
        let open = tokio::spawn({
            let link = owner.link().clone();
            async move { link.open(newport_ssh::Open::Session).await }
        });
        let mut status = owner.link().status();
        tokio::time::timeout(
            Duration::from_secs(10),
            status.wait_for(|s| s.login == Some(newport_ssh::Login::WaitingForApproval)),
        )
        .await
        .expect("the login to wait for approval")
        .unwrap();
        let signed = |count| {
            let signs = signs.clone();
            async move {
                tokio::time::timeout(Duration::from_secs(10), async {
                    while signs.load(Ordering::Acquire) < count {
                        tokio::time::sleep(Duration::from_millis(20)).await;
                    }
                })
                .await
            }
        };
        signed(1).await.expect("the agent to hold a sign request");

        crate::monitor::retry_installation(&owner).await;
        owner.retry();
        tokio::time::sleep(Duration::from_millis(500)).await;
        assert_eq!(signs.load(Ordering::Acquire), 1, "no second prompt");
        assert_eq!(
            owner.link().status().borrow().login,
            Some(newport_ssh::Login::WaitingForApproval)
        );

        owner.retry_fresh();
        signed(2).await.expect("a fresh retry to ask again");
        open.abort();
        registry.shutdown().await;
    }

    fn profile() -> Server {
        serde_json::from_value(serde_json::json!({
            "id": Uuid::new_v4(), "name": "Session fixture", "sshUser": "fixture",
            "sshHost": "127.0.0.1", "sshPort": 9
        }))
        .unwrap()
    }

    fn behind(jump: Uuid) -> Server {
        let mut server = profile();
        server.ssh_host = "inner.invalid".into();
        server.proxy = Some(crate::model::Proxy::Jump(jump));
        server
    }

    /// Why the link stopped, once something asks it for a session.
    async fn failure(link: &newport_ssh::Link) -> newport_ssh::Error {
        let mut status = link.status();
        let open = tokio::spawn({
            let link = link.clone();
            async move { link.open(newport_ssh::Open::Session).await }
        });
        let failed = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                if let newport_ssh::Reach::Failed(error) = &status.borrow_and_update().reach {
                    return error.clone();
                }
                status.changed().await.unwrap();
            }
        })
        .await
        .expect("the link to fail");
        open.abort();
        failed
    }

    #[tokio::test]
    async fn a_server_behind_a_jump_host_dials_through_the_jump_hosts_link() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let mut bastion = profile();
        bastion.ssh_port = listener.local_addr().unwrap().port();
        let registry = Registry::default();
        let jump = registry.session(&bastion, 0);
        let inner = registry.session(&behind(bastion.id), 0);
        let link = inner.link().clone();
        assert!(jump.has_link(), "the jump host's own link carries the dial");
        let open = tokio::spawn(async move { link.open(newport_ssh::Open::Session).await });
        let dialed =
            tokio::time::timeout(std::time::Duration::from_secs(5), listener.accept()).await;
        assert!(dialed.is_ok(), "the server behind it dials the jump host");
        open.abort();
        registry.shutdown().await;
    }

    #[tokio::test]
    async fn a_jump_host_that_is_missing_or_leads_back_fails_the_link() {
        let registry = Registry::default();
        let orphan = registry.session(&behind(Uuid::new_v4()), 0);
        assert_eq!(
            failure(orphan.link()).await,
            newport_ssh::Error::InvalidJump("it is no longer saved".into())
        );
        // Saving refuses a loop; the link still mustn't wait on itself.
        let mut a = behind(Uuid::new_v4());
        let b = behind(a.id);
        a.proxy = Some(crate::model::Proxy::Jump(b.id));
        let a = registry.session(&a, 0);
        registry.session(&b, 0);
        assert_eq!(
            failure(a.link()).await,
            newport_ssh::Error::InvalidJump("its jump hosts lead back to it".into())
        );
        registry.shutdown().await;
    }

    #[tokio::test]
    async fn terminal_routing_survives_restart_and_owner_replacement_but_isolates_profiles() {
        let directory = tempfile::tempdir().unwrap();
        let other_directory = tempfile::tempdir().unwrap();
        let server = profile();
        let first = Registry::new(directory.path());
        let owner = first.session(&server, 0);
        let routing = owner.integration_session_id();
        let replaced = first.session(&server, 1);
        assert!(owner.is_revoked());
        assert_eq!(replaced.integration_session_id(), routing);
        assert!(!replaced.has_link());
        assert_ne!(
            first.session(&profile(), 0).integration_session_id(),
            routing
        );
        first.shutdown().await;

        let restarted = Registry::new(directory.path());
        assert_eq!(
            restarted.session(&server, 0).integration_session_id(),
            routing
        );
        let other = Registry::new(other_directory.path());
        assert_ne!(other.session(&server, 0).integration_session_id(), routing);
        restarted.shutdown().await;
        other.shutdown().await;
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
        assert!(old.link.get().is_none());
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
        assert!(next.link.get().is_none());
        drop(replacement);
    }

    #[tokio::test]
    async fn revoked_owner_cannot_connect_and_shutdown_cannot_reopen() {
        let registry = Registry::default();
        let server = profile();
        let owner = registry.session(&server, 0);
        registry.invalidate(server.id);
        assert_eq!(owner.needle(&server).err().unwrap(), STALE);
        let next = registry.session(&server, 1);
        registry.shutdown().await;
        assert!(next.is_revoked());
        assert_eq!(
            registry.session(&server, 1).needle(&server).err().unwrap(),
            STALE
        );
    }

    #[tokio::test]
    #[ignore = "Run with cargo xtask test ssh -- server_session::tests"]
    async fn servers_behind_a_jump_host_open_sessions_over_its_one_login() {
        let registry = Registry::default();
        let mut bastion = profile();
        bastion.ssh_port = std::env::var("NEWPORT_TEST_SSH_PORT")
            .unwrap()
            .parse()
            .unwrap();
        bastion.identity_file = Some(std::env::var("NEWPORT_TEST_IDENTITY").unwrap());
        // The fixture forwards to its own SSH port, so it is its own jump host.
        let mut inner = bastion.clone();
        inner.id = Uuid::new_v4();
        inner.proxy = Some(crate::model::Proxy::Jump(bastion.id));
        let mut deeper = inner.clone();
        deeper.id = Uuid::new_v4();
        deeper.proxy = Some(crate::model::Proxy::Jump(inner.id));
        let jump = registry.session(&bastion, 0);
        let inner = registry.session(&inner, 0);
        let deeper = registry.session(&deeper, 0);
        let run = |link: &newport_ssh::Link| {
            let link = link.clone();
            async move { crate::remote::command::stdout(&link, "printf ok", None).await }
        };
        let (a, b, c) = tokio::join!(run(inner.link()), run(inner.link()), run(deeper.link()));
        assert_eq!(
            (a.unwrap(), b.unwrap(), c.unwrap()),
            ("ok".into(), "ok".into(), "ok".into())
        );
        assert_eq!(jump.reached(), 1, "one jump host login for every server");
        assert_eq!(inner.reached(), 1);
        assert_eq!(deeper.reached(), 1);
        registry.shutdown().await;
    }

    #[tokio::test]
    #[ignore = "Run with cargo xtask test ssh -- server_session::tests"]
    async fn shared_runtime_negotiates_once_and_does_not_serialize_feature_channels() {
        let registry = Registry::default();
        let mut server = profile();
        server.ssh_port = std::env::var("NEWPORT_TEST_SSH_PORT")
            .unwrap()
            .parse()
            .unwrap();
        server.identity_file = Some(std::env::var("NEWPORT_TEST_IDENTITY").unwrap());
        let owner = registry.session(&server, 0);
        let a = owner.needle(&server).unwrap();
        let b = owner.needle(&server).unwrap();
        let (first, second) =
            tokio::join!(a.launch(agent::Service::Git), b.launch(agent::Service::Git));
        let first = first.unwrap();
        let second = second.unwrap();
        let before = owner.agent_runtime.snapshot();
        assert_eq!(before.status, "ready");
        assert_eq!(before.generation, 1);
        let hello = before.hello.unwrap();
        assert!(crate::agent_runtime::protocol::valid_build(
            &hello.build_sha256
        ));
        drop(first);
        let after = owner
            .agent_runtime
            .ensure(owner.link(), owner.session_id, || b.validate())
            .await
            .unwrap();
        assert_eq!(
            hello.instance_id, after.instance_id,
            "one shared control process for concurrent feature startup"
        );
        let ok = || crate::remote::command::stdout(owner.link(), "printf ok", None);
        assert_eq!(ok().await.unwrap(), "ok");
        drop(second);
        owner.agent_runtime.reset().await;
        assert!(
            a.is_closed() && b.is_closed(),
            "retiring runtime fences what ran on it"
        );
        assert_eq!(ok().await.unwrap(), "ok", "the link outlives the runtime");
        registry.shutdown().await;
        assert_eq!(owner.agent_runtime.snapshot().status, "unknown");
    }

    #[tokio::test]
    #[ignore = "Run with cargo xtask test ssh -- server_session::tests"]
    async fn background_inspection_only_uses_ready_links() {
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
        // Publishing profiles registers owners without opening links.
        let saved_owner = registry.get(saved_only.id).unwrap();
        assert_eq!(registry.all().len(), 3);
        assert!(saved_owner.link.get().is_none());
        assert_eq!(saved_owner.lifecycle.snapshot().checked_at, 0);
        assert!(idle_owner.link.get().is_none());
        assert_eq!(owner.lifecycle.snapshot().checked_at, 0);
        let ok = || crate::remote::command::stdout(owner.link(), "printf ok", None);
        assert_eq!(ok().await.unwrap(), "ok");
        tokio::time::timeout(std::time::Duration::from_secs(10), async {
            while owner.lifecycle.snapshot().checked_at == 0 {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("a ready link must trigger discovery");
        assert_eq!(owner.lifecycle.snapshot().installed, Some(true));
        let previous = owner.lifecycle.snapshot().checked_at;
        registry.ready.notify_one();
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        assert_eq!(
            owner.lifecycle.snapshot().checked_at,
            previous,
            "no repeated check before hourly deadline"
        );
        let reached = owner.reached();
        let _ = crate::remote::command::run(owner.link(), "drop", None).await;
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            while owner.link_ready() {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("a lost connection must become observable");
        registry.ready.notify_one();
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        assert!(!owner.link_ready(), "the observer must not reconnect");
        assert_eq!(ok().await.unwrap(), "ok");
        tokio::time::timeout(std::time::Duration::from_secs(10), async {
            while owner.lifecycle.snapshot().checked_at == previous {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("a link ready again must be checked immediately");
        assert!(owner.reached() > reached);
        assert_eq!(registry.all().len(), 3);
        assert!(saved_owner.link.get().is_none());
        assert_eq!(saved_owner.lifecycle.snapshot().checked_at, 0);
        assert!(idle_owner.link.get().is_none());
        worker.abort();
        let _ = worker.await;
        registry.shutdown().await;
    }

    #[tokio::test]
    #[ignore = "Run with cargo xtask test ssh -- server_session::tests"]
    async fn the_link_serves_concurrent_commands_on_one_login_and_closes_with_the_session() {
        let registry = Registry::default();
        let mut server = profile();
        server.ssh_port = std::env::var("NEWPORT_TEST_SSH_PORT")
            .unwrap()
            .parse()
            .unwrap();
        server.identity_file = Some(std::env::var("NEWPORT_TEST_IDENTITY").unwrap());
        let owner = registry.session(&server, 0);
        let run = |command| crate::remote::command::stdout(owner.link(), command, None);
        let (a, b) = tokio::join!(run("printf ok"), run("printf ok"));
        assert_eq!((a.unwrap(), b.unwrap()), ("ok".into(), "ok".into()));
        let status = owner.check_installation_if_due().await.unwrap();
        assert_eq!(status.installed, Some(true));
        assert_eq!(status.update_available, None);
        let checked_at = status.checked_at;
        assert_eq!(
            owner.check_installation_if_due().await.unwrap().checked_at,
            checked_at
        );
        assert_eq!(
            registry.installation_status()[&server.id].checked_at,
            checked_at
        );
        assert!(
            run("fail")
                .await
                .unwrap_err()
                .to_string()
                .contains("status 7")
        );
        assert_eq!(run("printf ok").await.unwrap(), "ok");
        assert_eq!(owner.reached(), 1, "one login for every command");
        let mut status = owner.status();
        registry.invalidate(server.id);
        drop(owner);
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            while status.changed().await.is_ok() {}
        })
        .await
        .expect("retiring the session closes its link");
        let next = registry.session(&server, 1);
        assert_eq!(registry.installation_status()[&server.id].installed, None);
        assert!(next.link.get().is_none());
        assert_eq!(
            crate::remote::command::stdout(next.link(), "printf ok", None)
                .await
                .unwrap(),
            "ok"
        );
        registry.shutdown().await;
    }
    #[tokio::test]
    #[ignore = "Run with cargo xtask test ssh -- server_session::tests"]
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
            let lease = owner.needle(&server).unwrap();
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
                    .all(|i| i.connected && i.worker_leases == 1)
            );
            assert_eq!(snapshot.instances[0].registrations, 1);
            assert_eq!(
                snapshot.instances[0].instance_id,
                snapshot.hello.unwrap().instance_id
            );
            owner.agent_runtime.exhaust_registration_budget_for_test();
        }
        let identity = owner.agent_runtime.snapshot().hello.unwrap().instance_id;
        let next = owner.needle(&server).unwrap();
        assert_eq!(
            next.launch(agent::Service::Git).await.err().unwrap(),
            crate::agent_runtime::Error::Interrupted(crate::agent_runtime::BUSY.into())
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
        registry.shutdown().await;
        for (lease, mut client) in held {
            assert!(lease.is_closed());
            assert!(client.ping().await.is_err());
        }
        assert!(fresh.ping().await.is_err());
    }

    #[tokio::test]
    #[ignore = "Run with cargo xtask test ssh -- server_session::tests"]
    async fn saturated_worker_admission_is_retryable_without_runtime_replacement() {
        let registry = Registry::default();
        let mut server = profile();
        // Test Needle's worker bound independently of sshd's session bound.
        server.ssh_user = "fixture-many-sessions".into();
        server.ssh_port = std::env::var("NEWPORT_TEST_SSH_PORT")
            .unwrap()
            .parse()
            .unwrap();
        server.identity_file = Some(std::env::var("NEWPORT_TEST_IDENTITY").unwrap());
        let owner = registry.session(&server, 0);
        let mut held = Vec::new();
        for _ in 0..crate::agent_runtime::protocol::MAX_WORKERS {
            let lease = owner.needle(&server).unwrap();
            let (stream, _) = lease.launch(agent::Service::Git).await.unwrap();
            let (client, _) = crate::git::client::Client::start(stream).await.unwrap();
            held.push((lease, client));
        }
        let identity = owner.agent_runtime.snapshot().hello.unwrap().instance_id;
        let busy = crate::agent_runtime::Error::Interrupted(crate::agent_runtime::BUSY.into());
        let extra = owner.needle(&server).unwrap();
        assert_eq!(extra.launch(agent::Service::Git).await.err().unwrap(), busy);
        assert_eq!(owner.agent_runtime.snapshot().status, "ready");
        drop(held.pop());
        let (stream, _) = tokio::time::timeout(std::time::Duration::from_secs(3), async {
            loop {
                match extra.launch(agent::Service::Git).await {
                    Ok(value) => break value,
                    Err(error) if error == busy => {
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
        registry.shutdown().await;
    }
}
