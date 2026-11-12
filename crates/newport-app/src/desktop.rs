//! Main-thread dispatch and platform operations for the application.
use newport_backend::{application::DesktopCommands, desktop_integration::DesktopIntegration};
use newport_protocol::{
    common::{Error, Present, Retry},
    desktop::{Failure, host},
};

pub enum Action {
    #[cfg(any(target_os = "macos", windows))]
    Install(
        Box<crate::updates::install::Request>,
        std::sync::mpsc::SyncSender<Result<(), String>>,
    ),
    Run(Box<dyn FnOnce() + Send>),
    GuiSetup(String),
    ClipboardFiles {
        server: String,
        files: newport_backend::desktop_integration::ClipboardFiles,
        reply: newport_backend::desktop_integration::ClipboardFilesReply,
        cancelled: std::sync::Arc<std::sync::atomic::AtomicBool>,
    },
    ClipboardFilesStatus {
        server: String,
        status: newport_backend::desktop_integration::ClipboardFilesStatus,
    },
    Activate(tokio::sync::oneshot::Sender<Result<(), String>>),
    /// A taskbar pin or shortcut asked for a remote app: its registry entry,
    /// `None` when Newport no longer knows the ID. Acknowledged once the
    /// window has the request, when another process sent it.
    OpenRemoteApp {
        entry: Option<newport_domain::remote_app_registry::Entry>,
        action: Option<String>,
        acknowledge: Option<tokio::sync::oneshot::Sender<Result<(), String>>>,
    },
    #[cfg(any(target_os = "macos", windows))]
    Tray(newport_platform::tray::Action),
    #[cfg(any(target_os = "macos", windows))]
    TrayStatus(Option<(usize, bool)>),
    Quit,
}
#[derive(Clone)]
pub struct Integration {
    sender: async_channel::Sender<Action>,
    #[cfg(newport_remote_apps)]
    graphical: crate::graphical::Registry,
    #[cfg(any(target_os = "macos", windows))]
    clipboard: std::sync::Arc<newport_backend::desktop_integration::NativeClipboard>,
    #[cfg(any(target_os = "macos", windows))]
    clipboard_router: std::sync::Arc<newport_backend::clipboard_router::Router>,
    /// Set once the profile is owned.
    remote_apps:
        std::sync::Arc<std::sync::OnceLock<newport_backend::remote_app_registry::RemoteApps>>,
}
impl Integration {
    pub fn new() -> (Self, async_channel::Receiver<Action>) {
        let (sender, receiver) = async_channel::bounded(64);
        #[cfg(target_os = "macos")]
        let clipboard = {
            let owner = match std::env::current_exe() {
                Ok(executable) => Some(newport_platform::clipboard::helper::owner::Owner::new(
                    executable,
                    newport_platform::clipboard::helper::Board::General {},
                )),
                Err(error) => {
                    eprintln!("Clipboard helper unavailable: {error}");
                    None
                }
            };
            std::sync::Arc::new(newport_backend::desktop_integration::NativeClipboard::new(
                owner,
            ))
        };
        #[cfg(windows)]
        let clipboard = {
            let owner = match std::env::current_exe() {
                Ok(executable) => {
                    Some(newport_platform::clipboard::provider::windows::Owner::with_files(
                        newport_platform::clipboard::provider::windows::WorkerExecutable::production(executable),
                        newport_platform::clipboard::files::open_retained,
                    ))
                }
                Err(error) => {
                    eprintln!("Clipboard helper unavailable: {error}");
                    None
                }
            };
            std::sync::Arc::new(newport_backend::desktop_integration::NativeClipboard::new(
                owner,
            ))
        };
        #[cfg(any(target_os = "macos", windows))]
        let clipboard_router = std::sync::Arc::new(match newport_backend::profile::directory() {
            Ok(profile) => newport_backend::clipboard_router::Router::with_file_downloads(
                clipboard.clone(),
                &profile,
            ),
            Err(_) => newport_backend::clipboard_router::Router::new(clipboard.clone()),
        });
        #[cfg(all(newport_remote_apps, any(target_os = "macos", windows)))]
        let graphical = crate::graphical::Registry::new(Some((*clipboard_router).clone()));
        #[cfg(all(newport_remote_apps, not(any(target_os = "macos", windows))))]
        let graphical = crate::graphical::Registry::default();
        (
            Self {
                sender,
                #[cfg(newport_remote_apps)]
                graphical,
                #[cfg(any(target_os = "macos", windows))]
                clipboard,
                #[cfg(any(target_os = "macos", windows))]
                clipboard_router,
                remote_apps: Default::default(),
            },
            receiver,
        )
    }
    /// The registry a removed or renamed server's remote apps are kept in.
    pub fn set_remote_apps(&self, remote_apps: newport_backend::remote_app_registry::RemoteApps) {
        let _ = self.remote_apps.set(remote_apps);
    }
    #[cfg(newport_remote_apps)]
    pub fn graphical(&self) -> crate::graphical::Registry {
        self.graphical.clone()
    }
    #[cfg(newport_remote_apps)]
    async fn display_profile(
        &self,
        read: impl FnOnce() -> Option<(f64, [u32; 2])> + Send + 'static,
    ) -> Result<Option<newport_protocol::remote_apps::display::DisplayProfile>, String> {
        let (reply, received) = tokio::sync::oneshot::channel();
        let graphical = self.graphical.clone();
        self.on_main_thread(Box::new(move || {
            // AppKit metrics must be sampled on the UI thread for this launch,
            // not cached at startup or read by the blocking license worker.
            if !reply.is_closed() && !graphical.is_closed() {
                let profile = read()
                    .map(|(scale, area)| {
                        newport_protocol::remote_apps::display::DisplayProfile::from_metrics(
                            scale, area,
                        )
                    })
                    .transpose();
                let _ = reply.send(profile);
            }
        }))?;
        tokio::select! {
            biased;
            _ = self.graphical.closed() => Err("Application is closing".into()),
            result = tokio::time::timeout(std::time::Duration::from_secs(5), received) => {
                result
                    .map_err(|_| "Host display discovery timed out".to_owned())?
                    .map_err(|_| "Desktop closed before display discovery".to_owned())?
            }
        }
    }
    pub fn close(&self) {
        self.sender.close();
        #[cfg(newport_remote_apps)]
        self.graphical.close();
        #[cfg(any(target_os = "macos", windows))]
        {
            self.clipboard_router.close();
            self.clipboard.close();
        }
    }
    pub async fn shutdown(&self) -> Result<(), String> {
        self.close();
        #[cfg(any(target_os = "macos", windows))]
        {
            self.clipboard.shutdown().await?;
            // Copied server files live until Newport quits.
            let router = self.clipboard_router.clone();
            let _ = tokio::task::spawn_blocking(move || router.clear_downloads()).await;
        }
        Ok(())
    }
    #[cfg(any(target_os = "macos", windows))]
    pub fn install(&self, request: crate::updates::install::Request) -> Result<(), String> {
        // Called by the updater's blocking preparation worker. Keep ownership
        // there until the UI has checked unsaved buffers, rather than arming a
        // restart that might be rejected later.
        let (reply, accepted) = std::sync::mpsc::sync_channel(1);
        self.sender
            .try_send(Action::Install(Box::new(request), reply))
            .map_err(|_| "The application cannot restart right now. Try again.".to_owned())?;
        // The UI persists intent (signature checks, fsync) before replying.
        await_reply(&accepted, || self.sender.is_closed(), INSTALL_REPLY_LIMIT)
    }
    /// Opens a remote app once the window is up, for the launch that names
    /// it.
    pub fn open_remote_app(
        &self,
        entry: Option<newport_domain::remote_app_registry::Entry>,
        action: Option<String>,
    ) {
        if let Err(error) = self.sender.try_send(Action::OpenRemoteApp {
            entry,
            action,
            acknowledge: None,
        }) {
            eprintln!("Cannot open the remote app: {error}");
        }
    }
    pub async fn quit(&self) {
        let _ = self.sender.send(Action::Quit).await;
    }
    #[cfg(any(target_os = "macos", windows))]
    pub fn tray_action(&self, action: newport_platform::tray::Action) {
        if let Err(error) = self.sender.try_send(Action::Tray(action)) {
            eprintln!("Tray action unavailable: {error}");
        }
    }
    #[cfg(any(target_os = "macos", windows))]
    pub async fn tray_status(&self, status: Option<(usize, bool)>) -> bool {
        self.sender.send(Action::TrayStatus(status)).await.is_ok()
    }
}
#[cfg(any(target_os = "macos", windows, test))]
const INSTALL_REPLY_LIMIT: std::time::Duration = std::time::Duration::from_secs(30);

/// Stop waiting as soon as the app starts quitting: its main thread will never
/// reply, and staging cleanup at shutdown waits for this worker.
#[cfg(any(target_os = "macos", windows, test))]
fn await_reply(
    accepted: &std::sync::mpsc::Receiver<Result<(), String>>,
    closed: impl Fn() -> bool,
    limit: std::time::Duration,
) -> Result<(), String> {
    let deadline = std::time::Instant::now() + limit;
    loop {
        match accepted.recv_timeout(std::time::Duration::from_millis(50)) {
            Ok(result) => return result,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout)
                if !closed() && std::time::Instant::now() < deadline => {}
            Err(_) => {
                return Err("The application did not accept the restart. Try again.".into());
            }
        }
    }
}
impl DesktopIntegration for Integration {
    fn server_removed(&self, server: uuid::Uuid) {
        if let Some(remote_apps) = self.remote_apps.get() {
            remote_apps.forget_server(&server.to_string());
        }
        #[cfg(all(target_os = "macos", newport_remote_apps))]
        crate::launchers::server_removed(&server.to_string());
    }
    fn server_renamed(&self, server: uuid::Uuid, name: &str) {
        if let Some(remote_apps) = self.remote_apps.get() {
            // As the apps' windows and the Remote apps page name it.
            #[cfg(newport_remote_apps)]
            let name = newport_remote_provider_stubs::names::display_name(name);
            remote_apps.rename_server(&server.to_string(), name.as_ref());
        }
        #[cfg(all(target_os = "macos", newport_remote_apps))]
        crate::launchers::server_renamed(&server.to_string(), name);
    }
    #[cfg(newport_remote_apps)]
    fn maintain_graphical(
        &self,
        account: newport_backend::account::Account,
    ) -> newport_backend::desktop_integration::GraphicalMaintenance {
        self.graphical.maintain_account(account)
    }
    fn supports_shared_graphical(&self) -> bool {
        cfg!(newport_remote_apps)
    }
    #[cfg(newport_remote_apps)]
    fn remote_apps_settings(&self, settings: &newport_backend::preferences::remote_apps::Settings) {
        self.graphical.set_separate_apps(settings.separate_apps);
    }
    #[cfg(newport_remote_apps)]
    fn graphical_factory(
        &self,
        license: newport_backend::account::RemoteLicense,
        settings: newport_backend::preferences::remote_apps::Settings,
    ) -> newport_backend::graphical_session::FactoryFuture {
        let desktop = self.clone();
        Box::pin(async move {
            let display = desktop
                .display_profile(newport_remote_provider_gpui::platform::launch_display_metrics)
                .await?;
            tokio::task::spawn_blocking(move || {
                desktop.graphical.factory(license, settings, display)
            })
            .await
            .map_err(|_| "Graphical factory preparation stopped".to_owned())?
        })
    }
    #[cfg(any(target_os = "macos", windows))]
    fn clipboard_access(
        &self,
    ) -> Option<std::sync::Arc<dyn newport_backend::desktop_integration::ClipboardAccess>> {
        Some(self.clipboard_router.clone())
    }
    #[cfg(any(target_os = "macos", windows))]
    fn clipboard_router(&self) -> Option<newport_backend::clipboard_router::Router> {
        Some(self.clipboard_router.as_ref().clone())
    }
    fn on_main_thread(&self, action: Box<dyn FnOnce() + Send>) -> Result<(), String> {
        self.sender
            .try_send(Action::Run(action))
            .map_err(|error| format!("Desktop dispatch unavailable: {error}"))
    }
    // The native clipboard is reached only through clipboard_access, and never
    // from the UI thread. Without it (Linux) there is no clipboard to read.
    fn clipboard_revision(&self) -> Option<isize> {
        None
    }
    fn clipboard_offer(&self, _previous: Option<isize>) -> Option<(isize, Vec<String>)> {
        None
    }
    fn read_clipboard_format(&self, _revision: isize, _format: &str) -> Option<Vec<u8>> {
        None
    }
    fn open_url(&self, url: &str) -> Result<(), String> {
        open::that(url).map_err(|error| error.to_string())
    }
    fn supports_gui_notices(&self) -> bool {
        newport_gpui::supports_terminal_gui_setup()
    }
    fn request_gui_setup(&self, server: &str) {
        let _ = self.sender.try_send(Action::GuiSetup(server.to_owned()));
    }
    fn request_clipboard_files(
        &self,
        server: &str,
        files: newport_backend::desktop_integration::ClipboardFiles,
        reply: newport_backend::desktop_integration::ClipboardFilesReply,
        cancelled: std::sync::Arc<std::sync::atomic::AtomicBool>,
    ) {
        if let Err(error) = self.sender.try_send(Action::ClipboardFiles {
            server: server.into(),
            files,
            reply,
            cancelled,
        }) && let Action::ClipboardFiles { reply, .. } = error.into_inner()
        {
            reply(false);
        }
    }
    fn clipboard_files_status(
        &self,
        server: &str,
        status: newport_backend::desktop_integration::ClipboardFilesStatus,
    ) {
        let _ = self.sender.try_send(Action::ClipboardFilesStatus {
            server: server.into(),
            status,
        });
    }
    fn app_version(&self) -> String {
        env!("CARGO_PKG_VERSION").into()
    }
}

pub struct Commands {
    integration: Integration,
    startup: newport_platform::startup::Startup,
    updates: crate::updates::Service,
    remote_apps: Option<newport_backend::remote_app_registry::RemoteApps>,
}
impl Commands {
    pub fn new(integration: Integration, startup: newport_platform::startup::Startup) -> Self {
        Self {
            integration,
            startup,
            updates: crate::updates::Service::disabled(),
            remote_apps: None,
        }
    }

    pub fn with_updates(mut self, updates: crate::updates::Service) -> Self {
        self.updates = updates;
        self
    }

    /// The registry that pins and shortcuts name remote apps by.
    pub fn with_remote_apps(
        mut self,
        remote_apps: newport_backend::remote_app_registry::RemoteApps,
    ) -> Self {
        self.remote_apps = Some(remote_apps);
        self
    }

    async fn open_remote_app(&self, params: &host::OpenRemoteAppArgs) -> Result<(), String> {
        let id = &params.id;
        if !newport_domain::remote_app_identity::is_valid_app_user_model_id(id) {
            return Err("id must be a remote app ID".into());
        }
        if params.action.as_ref().is_some_and(String::is_empty) {
            return Err("action must be nonempty text".into());
        }
        let action = params.action.clone();
        let entry = self.remote_apps.as_ref().and_then(|apps| apps.find(id));
        let (acknowledge, received) = tokio::sync::oneshot::channel();
        self.integration
            .sender
            .try_send(Action::OpenRemoteApp {
                entry,
                action,
                acknowledge: Some(acknowledge),
            })
            .map_err(|error| format!("Desktop dispatch unavailable: {error}"))?;
        tokio::time::timeout(std::time::Duration::from_secs(5), received)
            .await
            .map_err(|_| "Opening the remote app timed out")?
            .map_err(|_| "Desktop closed before opening the remote app")?
    }
}
fn failure(code: &str, message: impl Into<String>, retry: Retry) -> Failure {
    Failure {
        error: Error {
            code: code.into(),
            message: message.into(),
            retry,
        },
        detail: None,
    }
}
impl Commands {
    async fn startup(&self, enabled: Option<bool>) -> Result<host::Startup, Failure> {
        let startup = self.startup.clone();
        let settings = tokio::task::spawn_blocking(move || match enabled {
            Some(enabled) => startup.set_enabled(enabled),
            None => startup.settings(),
        })
        .await
        .map_err(|error| failure("host_task_failed", error.to_string(), Retry::Unknown))?
        .map_err(|error| failure("startup_unavailable", error, Retry::No))?;
        Ok(host::Startup {
            enabled: settings.enabled,
            available: settings.available,
        })
    }
}
impl DesktopCommands for Commands {
    async fn dispatch(&self, operation: &host::Operation) -> Result<host::Reply, Failure> {
        match operation {
            host::Operation::Activate(_) => {
                let (acknowledge, activated) = tokio::sync::oneshot::channel();
                self.integration
                    .sender
                    .try_send(Action::Activate(acknowledge))
                    .map_err(|error| {
                        failure(
                            "desktop_unavailable",
                            format!("Desktop activation unavailable: {error}"),
                            Retry::No,
                        )
                    })?;
                tokio::time::timeout(std::time::Duration::from_secs(5), activated)
                    .await
                    .map_err(|_| {
                        failure(
                            "desktop_timeout",
                            "Desktop activation timed out",
                            Retry::Unknown,
                        )
                    })?
                    .map_err(|_| {
                        failure(
                            "desktop_closed",
                            "Desktop closed before activation",
                            Retry::Unknown,
                        )
                    })?
                    .map_err(|error| failure("desktop_activation_failed", error, Retry::Unknown))?;
                Ok(host::Reply::Activate(Box::new(Present {})))
            }
            host::Operation::OpenRemoteApp(params) => {
                self.open_remote_app(params)
                    .await
                    .map_err(|error| failure("desktop_open_failed", error, Retry::Unknown))?;
                Ok(host::Reply::OpenRemoteApp(Box::new(Present {})))
            }
            host::Operation::StartupSettings(_) => Ok(host::Reply::StartupSettings(Box::new(
                self.startup(None).await?,
            ))),
            host::Operation::SetLaunchAtLogin(params) => Ok(host::Reply::SetLaunchAtLogin(
                Box::new(self.startup(Some(params.enabled)).await?),
            )),
            host::Operation::UpdateStatus(_) => {
                Ok(host::Reply::UpdateStatus(Box::new(self.updates.status())))
            }
            host::Operation::CheckForUpdates(_) => {
                self.updates
                    .check()
                    .await
                    .map_err(|error| failure("update_check_failed", error, Retry::Unknown))?;
                Ok(host::Reply::CheckForUpdates(Box::new(Present {})))
            }
            host::Operation::InstallUpdate(params) => {
                if params.confirmed_version.is_empty() {
                    return Err(failure(
                        "invalid_request",
                        "The confirmed update version is required.",
                        Retry::No,
                    ));
                }
                self.updates
                    .install(&params.confirmed_version, self.integration.clone())
                    .await
                    .map_err(|error| failure("update_install_failed", error, Retry::Unknown))?;
                Ok(host::Reply::InstallUpdate(Box::new(Present {})))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(newport_remote_apps)]
    #[tokio::test]
    async fn graphical_display_reads_fresh_metrics_only_after_ui_dispatch() {
        let (desktop, requests) = Integration::new();
        for (factor, area) in [(2., [1440, 900]), (1.5, [1920, 1080]), (1., [1280, 720])] {
            let called = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
            let read = called.clone();
            let worker = desktop.clone();
            let pending = tokio::spawn(async move {
                worker
                    .display_profile(move || {
                        read.store(true, std::sync::atomic::Ordering::SeqCst);
                        Some((factor, area))
                    })
                    .await
            });
            let Action::Run(action) = requests.recv().await.unwrap() else {
                panic!("Expected main-thread display discovery");
            };
            assert!(!called.load(std::sync::atomic::Ordering::SeqCst));
            assert!(!pending.is_finished());
            action();
            let profile = pending.await.unwrap().unwrap().unwrap();
            assert_eq!(profile.work_area, area);
            assert_eq!(
                f64::from(profile.scale.numerator) / f64::from(profile.scale.denominator),
                factor
            );
        }
        desktop.close();
    }

    #[cfg(newport_remote_apps)]
    #[tokio::test]
    async fn graphical_display_cancels_on_shutdown_without_running_queued_appkit_work() {
        let (desktop, requests) = Integration::new();
        let worker = desktop.clone();
        let pending = tokio::spawn(async move {
            worker
                .display_profile(|| panic!("Cancelled display discovery must not run"))
                .await
        });
        let Action::Run(action) = requests.recv().await.unwrap() else {
            panic!("Expected main-thread display discovery");
        };
        desktop.close();
        // The UI may drain its queue before the waiting worker is scheduled.
        action();
        assert_eq!(
            pending.await.unwrap().unwrap_err(),
            "Application is closing"
        );
        assert!(desktop.display_profile(|| None).await.is_err());
    }

    #[cfg(any(target_os = "macos", windows))]
    #[tokio::test]
    async fn clipboard_drivers_share_one_router_and_its_shutdown_fence() {
        let (desktop, _requests) = Integration::new();
        let access = desktop.clipboard_access().unwrap();
        let router = desktop.clone().clipboard_router().unwrap();
        router.close();
        // Closing the policy owner fences the terminal metadata/read path too,
        // without starting a helper or accessing the user's clipboard.
        assert!(access.offer(None).await.is_none());
        assert!(access.read(1, "text/plain".into()).await.is_none());
        desktop.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn activation_waits_for_ui_acknowledgement_and_propagates_failures() {
        for outcome in [Ok(()), Err("Window unavailable".to_owned())] {
            let (desktop, requests) = Integration::new();
            let command = tokio::spawn(async move {
                Commands::new(desktop, newport_platform::startup::Startup::disabled())
                    .dispatch(&host::Operation::Activate(Box::new(Present {})))
                    .await
            });
            let Action::Activate(reply) = requests.recv().await.unwrap() else {
                panic!("Expected activation");
            };
            assert!(
                !command.is_finished(),
                "Queued work must not report completion"
            );
            reply.send(outcome.clone()).unwrap();
            assert_eq!(
                command
                    .await
                    .unwrap()
                    .map(|reply| matches!(reply, host::Reply::Activate(_)))
                    .map_err(|error| error.error.message),
                outcome.map(|_| true)
            );
        }
        let (desktop, requests) = Integration::new();
        let command = tokio::spawn(async move {
            Commands::new(desktop, newport_platform::startup::Startup::disabled())
                .dispatch(&host::Operation::Activate(Box::new(Present {})))
                .await
        });
        drop(requests.recv().await.unwrap());
        assert_eq!(
            command
                .await
                .unwrap()
                .map(|_| ())
                .map_err(|error| error.error.message),
            Err("Desktop closed before activation".into())
        );
    }

    #[tokio::test]
    async fn a_remote_app_request_carries_its_registry_entry_to_the_window() {
        use newport_domain::remote_app_registry::App;
        const ID: &str = "Newport.Remote.6fe02d2fc8ee3025544aa1e7.db207836d948804bf9711c8c";
        let profile = tempfile::tempdir().unwrap();
        let apps = newport_backend::remote_app_registry::RemoteApps::open(
            profile.path(),
            newport_backend::remote_app_registry::Launcher {
                executable: "newport.exe".into(),
                data_directory: None,
                programs_folder: None,
                pinned_folder: None,
                app_ids_key: None,
                jump_lists: false,
            },
        );
        apps.record(newport_backend::remote_app_registry::RecordedApp {
            id: ID.into(),
            server: "s1".into(),
            server_name: "lab".into(),
            app: App::DesktopId("calc.desktop".into()),
            name: "Calculator".into(),
            actions: Vec::new(),
        });
        for (params, known) in [
            (
                host::OpenRemoteAppArgs {
                    id: ID.into(),
                    action: Some("new-window".into()),
                },
                true,
            ),
            (
                host::OpenRemoteAppArgs {
                    id: "Newport.Remote.a.b".into(),
                    action: None,
                },
                false,
            ),
        ] {
            let (desktop, requests) = Integration::new();
            let commands = Commands::new(desktop, newport_platform::startup::Startup::disabled())
                .with_remote_apps(apps.clone());
            let command = tokio::spawn(async move {
                commands
                    .dispatch(&host::Operation::OpenRemoteApp(Box::new(params)))
                    .await
            });
            let Action::OpenRemoteApp {
                entry,
                action,
                acknowledge,
            } = requests.recv().await.unwrap()
            else {
                panic!("Expected a remote app");
            };
            assert_eq!(entry.is_some(), known);
            if known {
                assert_eq!(entry.unwrap().app, App::DesktopId("calc.desktop".into()));
                assert_eq!(action.as_deref(), Some("new-window"));
            }
            assert!(!command.is_finished(), "Waits for the window");
            acknowledge.unwrap().send(Ok(())).unwrap();
            assert!(matches!(
                command.await.unwrap(),
                Ok(host::Reply::OpenRemoteApp(_))
            ));
        }
        let (desktop, _requests) = Integration::new();
        let commands = Commands::new(desktop, newport_platform::startup::Startup::disabled());
        for params in [
            host::OpenRemoteAppArgs {
                id: String::new(),
                action: None,
            },
            host::OpenRemoteAppArgs {
                id: "not an id".into(),
                action: None,
            },
            host::OpenRemoteAppArgs {
                id: ID.into(),
                action: Some(String::new()),
            },
        ] {
            assert!(
                commands
                    .dispatch(&host::Operation::OpenRemoteApp(Box::new(params)))
                    .await
                    .is_err()
            );
        }
    }

    #[tokio::test]
    async fn unavailable_ui_refuses_activation() {
        let (desktop, _requests) = Integration::new();
        for _ in 0..64 {
            desktop.on_main_thread(Box::new(|| {})).unwrap();
        }
        let commands = Commands::new(
            desktop.clone(),
            newport_platform::startup::Startup::disabled(),
        );
        assert!(
            commands
                .dispatch(&host::Operation::Activate(Box::new(Present {})))
                .await
                .is_err()
        );
        desktop.close();
        assert!(
            commands
                .dispatch(&host::Operation::Activate(Box::new(Present {})))
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn cancelled_activation_closes_queued_acknowledgement() {
        let (desktop, requests) = Integration::new();
        let command = tokio::spawn(async move {
            Commands::new(desktop, newport_platform::startup::Startup::disabled())
                .dispatch(&host::Operation::Activate(Box::new(Present {})))
                .await
        });
        let Action::Activate(reply) = requests.recv().await.unwrap() else {
            panic!("Expected activation");
        };
        command.abort();
        assert!(command.await.unwrap_err().is_cancelled());
        assert!(
            reply.is_closed(),
            "UI must recognize abandoned activation requests"
        );
    }

    #[test]
    fn dispatch_is_bounded_and_closed_hosts_reject_work_without_executing_it() {
        let (desktop, requests) = Integration::new();
        let executed = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        for _ in 0..64 {
            let executed = executed.clone();
            desktop
                .on_main_thread(Box::new(move || {
                    executed.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                }))
                .unwrap();
        }
        assert!(desktop.on_main_thread(Box::new(|| {})).is_err());
        assert_eq!(executed.load(std::sync::atomic::Ordering::SeqCst), 0);
        let Action::Run(action) = requests.try_recv().unwrap() else {
            panic!("Expected desktop action");
        };
        action();
        assert_eq!(executed.load(std::sync::atomic::Ordering::SeqCst), 1);
        desktop.close();
        assert!(desktop.on_main_thread(Box::new(|| {})).is_err());
    }

    #[tokio::test]
    async fn typed_startup_commands_keep_explicit_false_and_refuse_disabled_registration() {
        let (desktop, _requests) = Integration::new();
        let commands = Commands::new(desktop, newport_platform::startup::Startup::disabled());
        for enabled in [true, false] {
            let failure = commands
                .dispatch(&host::Operation::SetLaunchAtLogin(Box::new(
                    host::SetLaunchAtLoginArgs { enabled },
                )))
                .await
                .unwrap_err();
            assert_eq!(failure.error.code, "startup_unavailable");
            assert_eq!(failure.error.retry, Retry::No);
            assert_eq!(
                failure.error.message,
                "Launch at login is available in the installed app."
            );
        }
    }

    #[tokio::test]
    async fn disabled_host_reports_unavailable_installation_features_and_refuses_writes() {
        let (desktop, _requests) = Integration::new();
        let commands = Commands::new(desktop, newport_platform::startup::Startup::disabled());
        let host::Reply::StartupSettings(settings) = commands
            .dispatch(&host::Operation::StartupSettings(Box::new(Present {})))
            .await
            .unwrap()
        else {
            panic!("Expected startup settings");
        };
        assert!(!settings.available);
        let host::Reply::UpdateStatus(status) = commands
            .dispatch(&host::Operation::UpdateStatus(Box::new(Present {})))
            .await
            .unwrap()
        else {
            panic!("Expected update status");
        };
        assert!(!status.enabled);
        for operation in [
            host::Operation::SetLaunchAtLogin(Box::new(host::SetLaunchAtLoginArgs {
                enabled: true,
            })),
            host::Operation::CheckForUpdates(Box::new(Present {})),
            host::Operation::InstallUpdate(Box::new(host::InstallUpdateArgs {
                confirmed_version: "1.0.0".into(),
            })),
        ] {
            assert!(commands.dispatch(&operation).await.is_err());
        }
    }

    #[test]
    fn install_reply_wait_ends_when_the_app_quits_or_the_limit_passes() {
        use std::time::{Duration, Instant};
        let (reply, accepted) = std::sync::mpsc::sync_channel(1);
        reply.send(Ok(())).unwrap();
        assert_eq!(
            await_reply(&accepted, || false, Duration::from_secs(5)),
            Ok(())
        );
        let started = Instant::now();
        assert!(await_reply(&accepted, || true, Duration::from_secs(30)).is_err());
        assert!(started.elapsed() < Duration::from_secs(1));
        let started = Instant::now();
        assert!(await_reply(&accepted, || false, Duration::from_millis(100)).is_err());
        assert!(started.elapsed() < Duration::from_secs(1));
        drop(reply);
        assert!(await_reply(&accepted, || false, Duration::from_secs(30)).is_err());
    }
}
