//! Main-thread dispatch and platform operations for the application.
use newport_backend::{application::DesktopCommands, desktop_integration::DesktopIntegration};
use serde_json::{Value, json};

pub enum Action {
    #[cfg(any(target_os = "macos", windows))]
    Install(
        Box<crate::updates::install::Request>,
        std::sync::mpsc::SyncSender<Result<(), String>>,
    ),
    Run(Box<dyn FnOnce() + Send>),
    GuiSetup(String),
    GuiLaunch {
        server: String,
        terminal: String,
        application: String,
        reply: newport_backend::desktop_integration::GuiReply,
        cancelled: std::sync::Arc<std::sync::atomic::AtomicBool>,
    },
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
    #[cfg(target_os = "macos")]
    clipboard: std::sync::Arc<newport_backend::desktop_integration::NativeClipboard>,
    #[cfg(target_os = "macos")]
    clipboard_router: std::sync::Arc<newport_backend::clipboard_router::Router>,
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
        #[cfg(target_os = "macos")]
        let clipboard_router = std::sync::Arc::new(match newport_backend::profile::directory() {
            Ok(profile) => newport_backend::clipboard_router::Router::with_file_downloads(
                clipboard.clone(),
                &profile,
            ),
            Err(_) => newport_backend::clipboard_router::Router::new(clipboard.clone()),
        });
        #[cfg(all(newport_remote_apps, target_os = "macos"))]
        let graphical = crate::graphical::Registry::new(Some((*clipboard_router).clone()));
        #[cfg(all(newport_remote_apps, not(target_os = "macos")))]
        let graphical = crate::graphical::Registry::default();
        (
            Self {
                sender,
                #[cfg(newport_remote_apps)]
                graphical,
                #[cfg(target_os = "macos")]
                clipboard,
                #[cfg(target_os = "macos")]
                clipboard_router,
            },
            receiver,
        )
    }
    #[cfg(newport_remote_apps)]
    pub fn graphical(&self) -> crate::graphical::Registry {
        self.graphical.clone()
    }
    #[cfg(newport_remote_apps)]
    async fn display_profile(
        &self,
        read: impl FnOnce() -> Option<(f64, [u32; 2])> + Send + 'static,
    ) -> Result<Option<newport_remote_protocol::display::DisplayProfile>, String> {
        let (reply, received) = tokio::sync::oneshot::channel();
        let graphical = self.graphical.clone();
        self.on_main_thread(Box::new(move || {
            // AppKit metrics must be sampled on the UI thread for this launch,
            // not cached at startup or read by the blocking license worker.
            if !reply.is_closed() && !graphical.is_closed() {
                let profile = read()
                    .map(|(scale, area)| {
                        newport_remote_protocol::display::DisplayProfile::from_metrics(scale, area)
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
        #[cfg(target_os = "macos")]
        {
            self.clipboard_router.close();
            self.clipboard.close();
        }
    }
    pub async fn shutdown(&self) {
        self.close();
        #[cfg(target_os = "macos")]
        {
            self.clipboard.shutdown().await;
            // Copied server files live until Newport quits.
            let router = self.clipboard_router.clone();
            let _ = tokio::task::spawn_blocking(move || router.clear_downloads()).await;
        }
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
    #[cfg(target_os = "macos")]
    fn clipboard_access(
        &self,
    ) -> Option<std::sync::Arc<dyn newport_backend::desktop_integration::ClipboardAccess>> {
        Some(self.clipboard_router.clone())
    }
    #[cfg(target_os = "macos")]
    fn clipboard_router(&self) -> Option<newport_backend::clipboard_router::Router> {
        Some(self.clipboard_router.as_ref().clone())
    }
    fn on_main_thread(&self, action: Box<dyn FnOnce() + Send>) -> Result<(), String> {
        self.sender
            .try_send(Action::Run(action))
            .map_err(|error| format!("Desktop dispatch unavailable: {error}"))
    }
    fn clipboard_revision(&self) -> Option<isize> {
        #[cfg(target_os = "macos")]
        {
            None
        }
        #[cfg(not(target_os = "macos"))]
        {
            newport_platform::clipboard::revision()
        }
    }
    fn clipboard_offer(&self, previous: Option<isize>) -> Option<(isize, Vec<String>)> {
        #[cfg(target_os = "macos")]
        {
            let _ = previous;
            None
        }
        #[cfg(not(target_os = "macos"))]
        {
            newport_platform::clipboard::offer(previous)
        }
    }
    fn read_clipboard_format(&self, revision: isize, format: &str) -> Option<Vec<u8>> {
        #[cfg(target_os = "macos")]
        {
            let _ = (revision, format);
            None
        }
        #[cfg(not(target_os = "macos"))]
        {
            newport_platform::clipboard::read_format(revision, format)
        }
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
    fn request_gui_launch(
        &self,
        server: &str,
        terminal: &str,
        application: &str,
        reply: newport_backend::desktop_integration::GuiReply,
        cancelled: std::sync::Arc<std::sync::atomic::AtomicBool>,
    ) {
        let _ = self.sender.try_send(Action::GuiLaunch {
            server: server.into(),
            terminal: terminal.into(),
            application: application.into(),
            reply,
            cancelled,
        });
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
}
impl Commands {
    pub fn new(integration: Integration, startup: newport_platform::startup::Startup) -> Self {
        Self {
            integration,
            startup,
            updates: crate::updates::Service::disabled(),
        }
    }

    pub fn with_updates(mut self, updates: crate::updates::Service) -> Self {
        self.updates = updates;
        self
    }
}
impl DesktopCommands for Commands {
    async fn dispatch(&self, method: &str, params: Value) -> Result<Value, Value> {
        match method {
            newport_platform::ipc::protocol::ACTIVATE_WINDOW => {
                let (acknowledge, activated) = tokio::sync::oneshot::channel();
                self.integration
                    .sender
                    .try_send(Action::Activate(acknowledge))
                    .map_err(|error| json!(format!("Desktop activation unavailable: {error}")))?;
                tokio::time::timeout(std::time::Duration::from_secs(5), activated)
                    .await
                    .map_err(|_| json!("Desktop activation timed out"))?
                    .map_err(|_| json!("Desktop closed before activation"))?
                    .map_err(|error| json!(error))?;
                Ok(Value::Null)
            }
            "get_startup_settings" | "set_launch_at_login" => {
                let enabled = if method == "set_launch_at_login" {
                    Some(
                        params
                            .get("enabled")
                            .and_then(Value::as_bool)
                            .ok_or_else(|| json!("enabled must be a boolean"))?,
                    )
                } else {
                    None
                };
                let startup = self.startup.clone();
                let settings = tokio::task::spawn_blocking(move || match enabled {
                    Some(enabled) => startup.set_enabled(enabled),
                    None => startup.settings(),
                })
                .await
                .map_err(|error| json!(error.to_string()))?
                .map_err(|error| json!(error))?;
                Ok(json!({"enabled":settings.enabled,"available":settings.available}))
            }
            "update_status" => Ok(self.updates.status()),
            "check_for_updates" => self
                .updates
                .check()
                .await
                .map(|_| Value::Null)
                .map_err(|error| json!(error)),
            "install_update" => {
                let version = params
                    .get("version")
                    .and_then(Value::as_str)
                    .filter(|value| !value.is_empty())
                    .ok_or_else(|| json!("The confirmed update version is required."))?;
                self.updates
                    .install(version, self.integration.clone())
                    .await
                    .map(|_| Value::Null)
                    .map_err(|error| json!(error))
            }
            // GPUI owns native file pickers and uses explicit-path transfer RPCs.
            _ => Err(json!(format!("Unknown command: {method}"))),
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

    #[cfg(target_os = "macos")]
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
        desktop.shutdown().await;
    }

    #[tokio::test]
    async fn activation_waits_for_ui_acknowledgement_and_propagates_failures() {
        for outcome in [Ok(()), Err("Window unavailable".to_owned())] {
            let (desktop, requests) = Integration::new();
            let command = tokio::spawn(async move {
                Commands::new(desktop, newport_platform::startup::Startup::disabled())
                    .dispatch(
                        newport_platform::ipc::protocol::ACTIVATE_WINDOW,
                        Value::Null,
                    )
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
                command.await.unwrap(),
                outcome.map(|_| Value::Null).map_err(|error| json!(error))
            );
        }
        let (desktop, requests) = Integration::new();
        let command = tokio::spawn(async move {
            Commands::new(desktop, newport_platform::startup::Startup::disabled())
                .dispatch(
                    newport_platform::ipc::protocol::ACTIVATE_WINDOW,
                    Value::Null,
                )
                .await
        });
        drop(requests.recv().await.unwrap());
        assert_eq!(
            command.await.unwrap(),
            Err(json!("Desktop closed before activation"))
        );
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
                .dispatch(
                    newport_platform::ipc::protocol::ACTIVATE_WINDOW,
                    Value::Null
                )
                .await
                .is_err()
        );
        desktop.close();
        assert!(
            commands
                .dispatch(
                    newport_platform::ipc::protocol::ACTIVATE_WINDOW,
                    Value::Null
                )
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn cancelled_activation_closes_queued_acknowledgement() {
        let (desktop, requests) = Integration::new();
        let command = tokio::spawn(async move {
            Commands::new(desktop, newport_platform::startup::Startup::disabled())
                .dispatch(
                    newport_platform::ipc::protocol::ACTIVATE_WINDOW,
                    Value::Null,
                )
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
    async fn startup_commands_validate_the_boolean_before_accessing_registration() {
        let (desktop, _requests) = Integration::new();
        let commands = Commands::new(desktop, newport_platform::startup::Startup::disabled());
        for params in [
            Value::Null,
            json!({}),
            json!({"enabled":"true"}),
            json!({"enabled":1}),
        ] {
            assert_eq!(
                commands.dispatch("set_launch_at_login", params).await,
                Err(json!("enabled must be a boolean"))
            );
        }
        for enabled in [true, false] {
            assert_eq!(
                commands
                    .dispatch("set_launch_at_login", json!({"enabled":enabled}))
                    .await,
                Err(json!("Launch at login is available in the installed app."))
            );
        }
    }

    #[tokio::test]
    async fn disabled_host_reports_unavailable_installation_features_and_refuses_writes() {
        let (desktop, _requests) = Integration::new();
        let commands = Commands::new(desktop, newport_platform::startup::Startup::disabled());
        assert_eq!(
            commands
                .dispatch("get_startup_settings", Value::Null)
                .await
                .unwrap()["available"],
            false
        );
        assert_eq!(
            commands
                .dispatch("update_status", Value::Null)
                .await
                .unwrap()["enabled"],
            false
        );
        for method in [
            "set_launch_at_login",
            "check_for_updates",
            "install_update",
            "unknown",
        ] {
            assert!(
                commands
                    .dispatch(method, json!({"enabled":true}))
                    .await
                    .is_err()
            );
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
