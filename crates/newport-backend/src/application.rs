//! Service composition. The application supplies platform adapters;
//! this owner keeps its profile locked until RPC and background work have drained.
use crate::{
    account::Account,
    desktop_integration::Desktop,
    files, git, manager,
    metrics_db::MetricsDb,
    preferences::Preferences,
    profile::{Instance, Store},
    rpc, terminal,
    workers::Workers,
};
use newport_protocol::desktop::{self, Failure, Operation, Reply};
use std::{
    future::Future,
    path::{Path, PathBuf},
    sync::Arc,
};
use tokio::sync::oneshot;

/// Updater, startup registration and window activation belong to the host.
pub trait DesktopCommands: Send + Sync + 'static {
    fn dispatch(
        &self,
        operation: &desktop::host::Operation,
    ) -> impl Future<Output = Result<desktop::host::Reply, Failure>> + Send;
}

/// Dropping the handle requests graceful shutdown. Keep the Tokio runtime alive
/// and call `stop` to wait for cleanup before exiting or restarting the process.
pub struct Application {
    connections: tokio::sync::watch::Receiver<newport_domain::connectivity::Snapshot>,
    endpoint: PathBuf,
    shutdown: Option<oneshot::Sender<()>>,
    task: tokio::task::JoinHandle<std::io::Result<()>>,
    result: Option<Result<(), String>>,
}

/// Profile ownership reserved before any service, migration or credential read.
/// The native host uses this while resolving an interrupted application update.
pub struct Reservation {
    store: Store,
    instance: Instance,
}
impl Reservation {
    /// Snapshot before credentials, migrations, SQLite, or workers are opened.
    /// Only the process holding both applicable profile locks may call this.
    pub fn prepare_upgrade(&self, version: &str) -> Result<(), String> {
        crate::profile::upgrade::preserve(&self.store.directory, version)
    }

    pub async fn start<H: DesktopCommands>(
        self,
        desktop: Desktop,
        commands: H,
    ) -> Result<Application, String> {
        Application::start_reserved(self, desktop, commands).await
    }
}

impl Application {
    pub fn reserve(store: Store) -> Result<Option<Reservation>, String> {
        let Some(instance) = Instance::acquire(&store.directory)
            .map_err(|error| format!("Cannot acquire Newport instance lock: {error}"))?
        else {
            return Ok(None);
        };
        // Owning the profile now, nothing else writes here: remove what
        // interrupted saves and earlier builds left behind.
        crate::profile::leftovers::sweep(&store.directory);
        Ok(Some(Reservation { store, instance }))
    }
    /// Returns `None` for an already-owned profile. All startup failures preserve
    /// the old data and release ownership. No listener is published until the
    /// services are initialized. Must be called inside the host's Tokio runtime.
    pub async fn start<H: DesktopCommands>(
        store: Store,
        desktop: Desktop,
        commands: H,
    ) -> Result<Option<Self>, String> {
        let Some(reserved) = Self::reserve(store)? else {
            return Ok(None);
        };
        reserved.start(desktop, commands).await.map(Some)
    }

    async fn start_reserved<H: DesktopCommands>(
        reserved: Reservation,
        desktop: Desktop,
        commands: H,
    ) -> Result<Self, String> {
        let Reservation { store, instance } = reserved;
        let directory = store.directory.clone();
        let preferences = Preferences::open(&directory)?;
        let manager = manager::Manager::new(store);
        let credentials = crate::credentials::register(manager.store.clone())?;
        let account = Account::new(
            manager.store.clone(),
            desktop.clone(),
            desktop.app_version(),
        );
        let earlier = || {
            if manager.load_error.is_some() {
                return None;
            }
            let key = manager.store.earlier_key()?;
            Some((key, manager.config.servers.clone()))
        };
        let metrics = MetricsDb::open(&directory.join("metrics.sqlite3"), earlier).await?;
        let integration_access = manager.integration_access();
        let state = Arc::new(State {
            manager: crate::manager::share(manager),
            integration_access,
            metrics,
            terminals: terminal::Sessions::default(),
            files: files::Operations::persistent(&directory),
            pending: git::pending::PendingOperations::new(directory.join("git-operations.json")),
            preferences,
            account,
            desktop,
        });
        let endpoint = newport_platform::ipc::profile_endpoint(&directory);
        let listener = match rpc::Server::bind_owned(&endpoint) {
            Ok(listener) => listener,
            Err(error) => {
                state.metrics.close().await;
                return Err(format!("Cannot open the backend socket: {error}"));
            }
        };
        let events = listener.events();
        let connections = state.manager.sessions.connections.subscribe();
        let workers = Workers::start(
            state.manager.clone(),
            state.metrics.clone(),
            state.desktop.clone(),
            state.account.clone(),
            state.preferences.clone(),
        );
        let (shutdown, wait) = oneshot::channel();
        let task = tokio::spawn(async move {
            // These guards survive a dropped client handle and cancelled stop waiter.
            let _instance = instance;
            let _credentials = credentials;
            let host = Host {
                state: state.clone(),
                events,
                commands,
            };
            let result = listener
                .run_until(host, async {
                    let _ = wait.await;
                })
                .await;
            workers.stop().await;
            state.account.shutdown();
            state.terminals.close_server(None).await;
            state.files.shutdown().await;
            crate::manager::shutdown(&state.manager).await;
            state.metrics.close().await;
            result
        });
        Ok(Self {
            connections,
            endpoint,
            shutdown: Some(shutdown),
            task,
            result: None,
        })
    }

    pub fn endpoint(&self) -> &Path {
        &self.endpoint
    }

    /// Live application-wide connectivity. Consumers observe; only the shared
    /// server session owner can change a server's state.
    pub fn connections(
        &self,
    ) -> tokio::sync::watch::Receiver<newport_domain::connectivity::Snapshot> {
        self.connections.clone()
    }

    /// Resolves after cleanup; also exposes unexpected listener failure to the host.
    pub async fn wait(&mut self) -> Result<(), String> {
        if let Some(result) = &self.result {
            return result.clone();
        }
        let result = (&mut self.task)
            .await
            .map_err(|error| format!("Backend task failed: {error}"))
            .and_then(|result| result.map_err(|error| format!("Backend listener failed: {error}")));
        self.result = Some(result.clone());
        result
    }

    pub async fn stop(mut self) -> Result<(), String> {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        self.wait().await
    }
}

struct State {
    manager: manager::Shared,
    integration_access: crate::integration_permissions::Access,
    metrics: MetricsDb,
    terminals: terminal::Sessions,
    files: files::Operations,
    pending: git::pending::PendingOperations,
    preferences: Preferences,
    account: Account,
    desktop: Desktop,
}

struct Host<H> {
    state: Arc<State>,
    events: rpc::EventLog,
    commands: H,
}
impl<H: DesktopCommands> rpc::Host for Host<H> {
    fn instance_id(&self) -> uuid::Uuid {
        self.state.manager.instance_id
    }

    async fn dispatch(&self, operation: &Operation) -> Result<Reply, Failure> {
        let state = &self.state;
        let events = self.events.clone();
        let services = rpc::services::Services {
            integration_access: &state.integration_access,
            features: rpc::features::Services {
                preferences: &self.state.preferences,
                manager: &state.manager,
                metrics: &state.metrics,
                files: &state.files,
                terminals: &state.terminals,
            },
            git: rpc::git::Services {
                identity: &state.preferences,
                manager: &state.manager,
                pending: &state.pending,
                events: Arc::new(move |event| {
                    events.record(desktop::events::Event::GitCommand(Box::new(event)))
                }),
            },
            preferences: &state.preferences,
            account: &state.account,
            desktop: state.desktop.clone(),
        };
        match services.dispatch(operation).await {
            Some(result) => result,
            None => match operation {
                Operation::Host(operation) => self
                    .commands
                    .dispatch(operation)
                    .await
                    .map(|reply| Reply::Host(Box::new(reply))),
                _ => Err(rpc::failure::operation("Unsupported desktop operation")),
            },
        }
    }

    fn link_status(
        &self,
        server: uuid::Uuid,
    ) -> Option<tokio::sync::watch::Receiver<newport_ssh::Status>> {
        self.state
            .manager
            .owner(server)
            .ok()
            .map(|owner| owner.status())
    }

    async fn open_terminal(
        &self,
        request: rpc::TerminalOpen,
    ) -> Result<rpc::TerminalChannel, Failure> {
        let owner = self
            .state
            .manager
            .owner(request.server)
            .map_err(rpc::failure::operation)?;
        let server = owner.server().clone();
        // Every terminal names the server's display where this computer may
        // use it, and none waits for it: using the tab shows the display
        // here, starting it when needed. Failures leave an ordinary shell.
        let display = terminal::display::access(
            request.server,
            self.state.account.clone(),
            self.state.preferences.clone(),
            self.state.desktop.clone(),
        )
        .await;
        let io = self
            .state
            .terminals
            .open_bound_with_display(
                request.session,
                server,
                (u32::from(request.columns), u32::from(request.rows)),
                owner,
                display,
                if request.attach {
                    terminal::Hold::Attach(request.colors)
                } else if request.persist {
                    terminal::Hold::WhenAvailable(request.colors)
                } else {
                    terminal::Hold::No
                },
            )
            .map_err(rpc::failure::operation)?;
        Ok(rpc::TerminalChannel {
            io,
            guard: Box::new(TerminalLease {
                state: self.state.clone(),
                session: request.session,
            }),
        })
    }
    async fn open_remote_stream(
        &self,
        server: uuid::Uuid,
        command: String,
    ) -> Result<rpc::RemoteChannel, Failure> {
        let owner = self
            .state
            .manager
            .owner(server)
            .map_err(rpc::failure::operation)?;
        if owner.is_revoked() {
            return Err(rpc::failure::operation(crate::server_session::STALE));
        }
        // The channel lasts as long as the server's link, which its session
        // closes when it is retired.
        let channel = crate::remote::command::stream(owner.link(), &command)
            .await
            .map_err(rpc::failure::operation)?;
        Ok(rpc::RemoteChannel {
            channel: channel.into(),
            guard: Box::new(()),
        })
    }
    async fn open_remote_apps(
        &self,
        server: uuid::Uuid,
        arguments: Vec<String>,
    ) -> Result<rpc::RemoteChannel, Failure> {
        let owner = self
            .state
            .manager
            .owner(server)
            .map_err(rpc::failure::operation)?;
        let needle = owner
            .needle(owner.server())
            .map_err(rpc::failure::operation)?;
        let (channel, worker) = needle
            .launch_channel(crate::agent::Service::RemoteApps { arguments })
            .await
            .map_err(rpc::failure::runtime)?;
        Ok(rpc::RemoteChannel {
            channel,
            guard: Box::new((worker, needle)),
        })
    }
    async fn open_lsp(&self, request: rpc::LspOpen) -> Result<rpc::LspChannel, Failure> {
        let manager = &self.state.manager;
        if !manager.configuration_available() {
            return Err(rpc::failure::operation(
                "Server configuration is unavailable.",
            ));
        }
        let (profile, owner) = manager
            .current(request.server)
            .map_err(rpc::failure::operation)?;
        if !rpc::configuration::matches_scope(
            &profile,
            &request.expected,
            owner.revision(),
            manager.instance_id,
        ) {
            return Err(rpc::failure::operation(
                "The server connection changed. Reopen the project.",
            ));
        }
        let needle = owner.needle(&profile).map_err(rpc::failure::operation)?;
        let stream = needle
            .launch(crate::agent::Service::Lsp {
                root: request.root,
                server: request.server_name,
                envrc: request.envrc,
            })
            .await
            .map_err(rpc::failure::runtime)?;
        Ok(rpc::LspChannel {
            stream: Box::new(stream),
            guard: Box::new(needle),
        })
    }
}

struct TerminalLease {
    state: Arc<State>,
    session: uuid::Uuid,
}
impl Drop for TerminalLease {
    fn drop(&mut self) {
        self.state.terminals.cancel(self.session);
    }
}

#[cfg(test)]
mod tests;
