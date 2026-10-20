//! Main-thread dispatch and platform operations for the standalone host.
use newport_backend::{application::DesktopCommands, desktop_integration::DesktopIntegration};
use serde_json::{Value, json};

pub enum Action {
    #[cfg(any(target_os = "macos", windows))]
    Install(
        Box<crate::updates::install::Request>,
        std::sync::mpsc::SyncSender<Result<(), String>>,
    ),
    Run(Box<dyn FnOnce() + Send>),
    Activate(tokio::sync::oneshot::Sender<Result<(), String>>),
    #[cfg(any(target_os = "macos", windows))]
    Tray(newport_platform::tray::Action),
    #[cfg(any(target_os = "macos", windows))]
    TrayStatus(Option<(usize, bool)>),
    Quit,
}
#[derive(Clone)]
pub struct Integration(async_channel::Sender<Action>);
impl Integration {
    pub fn new() -> (Self, async_channel::Receiver<Action>) {
        let (sender, receiver) = async_channel::bounded(64);
        (Self(sender), receiver)
    }
    pub fn close(&self) {
        self.0.close();
    }
    #[cfg(any(target_os = "macos", windows))]
    pub fn install(&self, request: crate::updates::install::Request) -> Result<(), String> {
        // Called by the updater's blocking preparation worker. Keep ownership
        // there until the UI has checked unsaved buffers, rather than arming a
        // restart that might be rejected later.
        let (reply, accepted) = std::sync::mpsc::sync_channel(1);
        self.0
            .try_send(Action::Install(Box::new(request), reply))
            .map_err(|_| "The application cannot restart right now. Try again.".to_owned())?;
        // The UI persists intent (signature checks, fsync) before replying.
        await_reply(&accepted, || self.0.is_closed(), INSTALL_REPLY_LIMIT)
    }
    pub async fn quit(&self) {
        let _ = self.0.send(Action::Quit).await;
    }
    #[cfg(any(target_os = "macos", windows))]
    pub fn tray_action(&self, action: newport_platform::tray::Action) {
        if let Err(error) = self.0.try_send(Action::Tray(action)) {
            eprintln!("Tray action unavailable: {error}");
        }
    }
    #[cfg(any(target_os = "macos", windows))]
    pub async fn tray_status(&self, status: Option<(usize, bool)>) -> bool {
        self.0.send(Action::TrayStatus(status)).await.is_ok()
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
    fn on_main_thread(&self, action: Box<dyn FnOnce() + Send>) -> Result<(), String> {
        self.0
            .try_send(Action::Run(action))
            .map_err(|error| format!("Desktop dispatch unavailable: {error}"))
    }
    fn clipboard_revision(&self) -> Option<isize> {
        newport_platform::clipboard::revision()
    }
    fn clipboard_offer(&self, previous: Option<isize>) -> Option<(isize, Vec<String>)> {
        newport_platform::clipboard::offer(previous)
    }
    fn read_clipboard_format(&self, revision: isize, format: &str) -> Option<Vec<u8>> {
        newport_platform::clipboard::read_format(revision, format)
    }
    fn open_url(&self, url: &str) -> Result<(), String> {
        open::that(url).map_err(|error| error.to_string())
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
                    .0
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
            _ => Err(json!(format!("Unknown native command: {method}"))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
    async fn migration_host_reports_unavailable_installation_features_and_refuses_writes() {
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
