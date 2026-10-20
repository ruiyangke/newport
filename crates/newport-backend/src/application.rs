//! Tauri-free service composition. The application supplies platform adapters;
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
use serde_json::Value;
use std::{
    future::Future,
    path::{Path, PathBuf},
    sync::Arc,
};
use tokio::sync::oneshot;

/// Updater, startup registration and legacy native dialogs belong to the host.
pub trait DesktopCommands: Send + Sync + 'static {
    fn dispatch(
        &self,
        method: &str,
        args: Value,
    ) -> impl Future<Output = Result<Value, Value>> + Send;
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
    pub fn reserve(store: Store, isolated: bool) -> Result<Option<Reservation>, String> {
        let Some(instance) = Instance::acquire(&store.directory, isolated)
            .map_err(|error| format!("Cannot acquire Newport instance lock: {error}"))?
        else {
            return Ok(None);
        };
        Ok(Some(Reservation { store, instance }))
    }
    /// Returns `None` for an already-owned profile. All startup failures preserve
    /// the old data and release ownership. No listener is published until the
    /// services are initialized. Must be called inside the host's Tokio runtime.
    pub async fn start<H: DesktopCommands>(
        store: Store,
        isolated: bool,
        desktop: Desktop,
        commands: H,
    ) -> Result<Option<Self>, String> {
        let Some(reserved) = Self::reserve(store, isolated)? else {
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
        let key = match &manager.load_error {
            Some(error) => Err(error.clone()),
            None => manager.store.vault_key().map(|key| key.to_vec()),
        };
        let metrics = MetricsDb::open(&directory.join("metrics.sqlite3"), key).await?;
        let integration_access = manager.integration_access();
        let state = Arc::new(State {
            manager: crate::manager::share(manager),
            integration_access,
            metrics,
            terminals: terminal::Sessions::default(),
            files: files::Operations::persistent(&directory),
            git: git::Sessions::default(),
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
                return Err(format!("Cannot listen for native clients: {error}"));
            }
        };
        let events = listener.events();
        let connections = state.manager.sessions.connections.subscribe();
        let workers = Workers::start(
            state.manager.clone(),
            state.metrics.clone(),
            state.desktop.clone(),
            state.account.clone(),
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
            state.git.shutdown().await;
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
    git: git::Sessions,
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
    async fn dispatch(&self, method: &str, args: Value) -> Result<Value, Value> {
        let state = &self.state;
        let events = self.events.clone();
        let services = rpc::services::Services {
            integration_access: &state.integration_access,
            features: rpc::features::Services {
                manager: &state.manager,
                metrics: &state.metrics,
                files: &state.files,
                terminals: &state.terminals,
            },
            git: rpc::git::Services {
                identity: &state.preferences,
                manager: &state.manager,
                sessions: &state.git,
                pending: &state.pending,
                events: Arc::new(move |event| events.record(event)),
            },
            preferences: &state.preferences,
            account: &state.account,
            desktop: state.desktop.clone(),
        };
        match services.dispatch(method, &args).await {
            Some(result) => result,
            None => self.commands.dispatch(method, args).await,
        }
    }

    async fn open_terminal(&self, request: rpc::TerminalOpen) -> Result<Box<dyn Send>, String> {
        crate::commands::terminal_open(
            &self.state.manager,
            &self.state.terminals,
            request.id,
            request.session,
            request.cols,
            request.rows,
        )
        .await?;
        Ok(Box::new(TerminalLease {
            state: self.state.clone(),
            session: request.session,
        }))
    }
    async fn open_remote_stream(
        &self,
        server: uuid::Uuid,
        command: String,
    ) -> Result<rpc::RemoteChannel, String> {
        let owner = self.state.manager.owner(server)?;
        let config = owner.server().clone();
        let lease = owner.acquire(&config).await?;
        let channel = lease.exec_channel(&command).await?;
        Ok(rpc::RemoteChannel {
            channel,
            guard: Box::new(lease),
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
