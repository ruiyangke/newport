//! The application composes the public backend with the private GUI client.
//! This registry holds only weak handles; backend demand owns the connection.
use newport_backend::{
    account::RemoteLicense,
    clipboard_router::{Router, display as clipboard_display},
    graphical_session::{self as backend, Driver as _, transport},
    preferences::remote_apps::{Codec, Settings},
};
use newport_domain::terminal::{SharedDisplayMode, SharedGuiEnvironment, gui_protocol};
use newport_remote_client::{
    RemoteSession, SessionLicense, SessionOptions, SessionOutcome, SessionState,
    diagnostics::Transport,
};
use newport_remote_protocol::{performance::CodecMode, terminal::SharedEnvironment};
use newport_remote_transport::{self as client_transport, Cancellation};
use std::{
    collections::BTreeMap,
    sync::{
        Arc, Mutex, Weak,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
use tokio::sync::watch;
mod license;

#[derive(Clone)]
pub struct Registry(Arc<RegistryState>);
struct RegistryState {
    clients: Mutex<BTreeMap<String, Weak<Driver>>>,
    closed: watch::Sender<bool>,
    presentation: watch::Sender<()>,
    clients_changed: watch::Sender<()>,
    clipboard: Option<Router>,
    account:
        std::sync::OnceLock<watch::Receiver<newport_backend::account::remote_sessions::Policy>>,
}
/// A short-lived UI attachment candidate. Backend demand owns its lifetime;
/// callers must not retain this client while no presenter is attached.
pub struct Presentation {
    pub key: String,
    pub connection: Arc<RemoteSession>,
    pub hud: bool,
    owner: Weak<Driver>,
}
impl Presentation {
    /// Claim only when attaching a presenter, not while enumerating candidates.
    /// The returned weak handle cannot retain the graphical driver or transport.
    pub fn clipboard_presenter(&self) -> Option<clipboard_display::Presenter> {
        let driver = self.owner.upgrade()?;
        let _admission = driver.operations.lock().unwrap();
        if driver.publication.stopped.load(Ordering::Acquire)
            || !matches!(
                *driver.publication.state.borrow(),
                backend::DriverState::Ready { .. }
            )
        {
            return None;
        }
        driver.publication.clipboard.get()?.presenter()
    }
}
impl Default for Registry {
    fn default() -> Self {
        Self::new(None)
    }
}
impl Registry {
    pub fn new(clipboard: Option<Router>) -> Self {
        Self(Arc::new(RegistryState {
            clients: Mutex::new(BTreeMap::new()),
            closed: watch::channel(false).0,
            presentation: watch::channel(()).0,
            clients_changed: watch::channel(()).0,
            clipboard,
            account: Default::default(),
        }))
    }
}
impl Registry {
    pub fn maintain_account(
        &self,
        account: newport_backend::account::Account,
    ) -> newport_backend::desktop_integration::GraphicalMaintenance {
        license::maintain(self.clone(), account.remote_session_policy(), move || {
            let account = account.clone();
            async move { account.remote_license().await }
        })
    }
    #[cfg(test)]
    pub fn factory(
        &self,
        license: RemoteLicense,
        settings: Settings,
    ) -> Result<Arc<dyn backend::Factory>, String> {
        self.factory_for_mode(license, settings, None, SharedDisplayMode::Mixed)
    }
    pub fn factory_for_mode(
        &self,
        license: RemoteLicense,
        settings: Settings,
        display: Option<newport_remote_protocol::display::DisplayProfile>,
        mode: SharedDisplayMode,
    ) -> Result<Arc<dyn backend::Factory>, String> {
        let license = SessionLicense {
            token: license.token.to_string(),
            machine: license.machine,
            trusted_time: license.trusted_time,
        };
        // Must run before ServerSession::acquire_graphical, not merely when
        // attaching to an already-started worker. No server IO is done here.
        let verified = license
            .verify()
            .map_err(|error| error.message().to_owned())?;
        let account = self
            .0
            .account
            .get()
            .map(|policy| license::Guard::new(policy.clone(), verified.device()))
            .transpose()?;
        if ![15, 30, 60].contains(&settings.fps_goal) {
            return Err("Frame rate must be 15, 30 or 60 fps".into());
        }
        let options = SessionOptions {
            shared_display: true,
            shared_display_mode: private_mode(mode),
            terminal_owned_bus: cfg!(feature = "experimental-terminal-owned-bus"),
            track_applications: true,
            native_sizing: true,
            display,
            settings: newport_remote_client::SessionSettings {
                audio: settings.audio,
                av_sync: settings.audio,
                max_fps: settings.fps_goal,
                codec_mode: match settings.codec {
                    Codec::Auto => CodecMode::Auto,
                    Codec::Lossless => CodecMode::Lossless,
                    Codec::Fast => CodecMode::Fast,
                },
                // Shared sessions negotiate only the runtime clipboard. Host
                // permission and GUI focus routing belong to the shared router.
                clipboard: false,
                ime: true,
                ..Default::default()
            },
            license: Some(license),
            ..Default::default()
        };
        options.settings.validate()?;
        if *self.0.closed.borrow() {
            return Err("Application is closing".into());
        }
        Ok(Arc::new(Factory {
            registry: self.clone(),
            options,
            hud: settings.hud,
            account,
        }))
    }
    /// Metadata-only wakeups, also sent after a presenter releases ownership.
    /// Subscribe before inspecting candidates so no concurrent window is lost.
    pub fn observe_presentation(&self) -> watch::Receiver<()> {
        self.0.presentation.subscribe()
    }
    pub fn recheck_presentation(&self) {
        self.0.presentation.send_replace(());
    }
    pub fn is_closed(&self) -> bool {
        *self.0.closed.borrow()
    }
    pub async fn closed(&self) {
        let mut closed = self.0.closed.subscribe();
        let _ = closed.wait_for(|closed| *closed).await;
    }
    pub fn presentation_candidates(&self) -> Vec<Presentation> {
        let clients = {
            let clients = self.0.clients.lock().unwrap();
            if self.is_closed() {
                return Vec::new();
            }
            clients
                .iter()
                .filter_map(|(key, client)| client.upgrade().map(|client| (key.clone(), client)))
                .collect::<Vec<_>>()
        };
        // Never hold the registry lock while inspecting client state or
        // releasing a driver: its final drop stops the client and publishes.
        clients
            .into_iter()
            .filter_map(|(key, client)| {
                let ready = matches!(
                    *client.publication.state.borrow(),
                    backend::DriverState::Ready { .. }
                );
                (ready && client.connection.presentation_pending()).then(|| Presentation {
                    key,
                    connection: client.connection.clone(),
                    hud: client.hud,
                    owner: Arc::downgrade(&client),
                })
            })
            .collect()
    }
    pub fn close(&self) {
        let clients = {
            let mut clients = self.0.clients.lock().unwrap();
            self.0.closed.send_replace(true);
            std::mem::take(&mut *clients)
                .into_values()
                .filter_map(|client| client.upgrade())
                .collect::<Vec<_>>()
        };
        for client in clients {
            client.stop();
        }
        self.recheck_presentation();
        self.0.clients_changed.send_replace(());
    }
    fn register(&self, driver: &Arc<Driver>) -> Result<(), String> {
        let mut clients = self.0.clients.lock().unwrap();
        license::validate(driver.account.as_ref())?;
        if *self.0.closed.borrow() {
            return Err("Application is closing".into());
        }
        clients.retain(|_, client| client.strong_count() != 0);
        let key = format!(
            "{}:{}",
            driver.identity.session, driver.identity.display_generation
        );
        if clients.contains_key(&key) {
            return Err("Graphical display is already attached".into());
        }
        clients.insert(key, Arc::downgrade(driver));
        drop(clients);
        // The first window may have arrived before registration.
        self.recheck_presentation();
        self.0.clients_changed.send_replace(());
        Ok(())
    }
}

struct Factory {
    registry: Registry,
    options: SessionOptions,
    hud: bool,
    account: Option<license::Guard>,
}
impl backend::Factory for Factory {
    fn terminal_bus_policy(&self) -> backend::terminal_bus::Policy {
        if self.options.terminal_owned_bus {
            backend::terminal_bus::Policy::OwnedNative
        } else {
            backend::terminal_bus::Policy::PreserveInherited
        }
    }
    fn display_mode(&self) -> SharedDisplayMode {
        match self.options.shared_display_mode {
            newport_remote_client::SharedDisplayMode::Mixed => SharedDisplayMode::Mixed,
            newport_remote_client::SharedDisplayMode::Wayland => SharedDisplayMode::Wayland,
        }
    }
    fn validate(&self) -> Result<(), String> {
        license::validate(self.account.as_ref())?;
        if self.registry.is_closed() {
            return Err("Application is closing".into());
        }
        Ok(())
    }
    fn attach(
        &self,
        channel: transport::Channel,
        identity: backend::Identity,
    ) -> backend::AttachFuture {
        let registry = self.registry.clone();
        let options = self.options.clone();
        let mode = self.display_mode();
        let bus_policy = self.terminal_bus_policy();
        let hud = self.hud;
        let account = self.account.clone();
        Box::pin(async move {
            license::validate(account.as_ref())?;
            if *registry.0.closed.borrow() {
                return Err("Application is closing".into());
            }
            let (reader, writer, transport) = channel.into_parts();
            let cancellation = Cancellation::default();
            let mut guard = CancelAttachment {
                cancellation: cancellation.clone(),
                transport: transport.clone(),
                armed: true,
            };
            let channel = client_transport::Channel::new(
                reader,
                writer,
                TransportDriver {
                    control: transport,
                    observer: Mutex::new(None),
                },
            )
            .with_cancellation(cancellation);
            // License/machine checks and unused-channel disposal can block.
            // Dropping the JoinHandle cannot cancel a blocking job, so the
            // guard above fences its channel even if that job has not started.
            let mut closing = registry.0.closed.subscribe();
            let connecting =
                tokio::task::spawn_blocking(move || RemoteSession::attach(channel, options));
            let connection = tokio::select! {
                _ = license::revoked(account.as_ref()) => return Err("Remote apps account access changed".into()),
                _ = closing.wait_for(|closed| *closed) => return Err("Application is closing".into()),
                connection = connecting => connection.map_err(|_| "Graphical client attachment stopped".to_owned())??,
            };
            let connection = Arc::new(connection);
            let publication = Arc::new(Publication {
                state: watch::channel(backend::DriverState::Closed).0,
                stopped: AtomicBool::new(false),
                presentation: registry.0.presentation.clone(),
                clipboard: Default::default(),
            });
            let observed = publication.clone();
            let control = connection.control();
            control
                .observe_activity_status(Arc::new(move |activity| observed.publish(activity)))?;
            let mut startup = StartupWait::default();
            let environment = loop {
                let ready = matches!(
                    *publication.state.borrow(),
                    backend::DriverState::Ready { .. }
                );
                match startup.step(control.state(), ready, Instant::now(), || {
                    control.diagnostics().transport
                }) {
                    Startup::Wait => {}
                    Startup::Ready => {
                        let environment = control
                            .shared_environment()
                            .ok_or("Shared graphical environment is unavailable")?;
                        break project_environment(&environment, &identity, mode)?;
                    }
                    Startup::Failed(error) => return Err(error),
                }
                tokio::select! {
                    _ = license::revoked(account.as_ref()) => return Err("Remote apps account access changed".into()),
                    _ = closing.wait_for(|closed| *closed) => return Err("Application is closing".into()),
                    _ = tokio::time::sleep(Duration::from_millis(10)) => {},
                }
            };
            let terminal_bus = match bus_policy {
                backend::terminal_bus::Policy::OwnedNative if control.terminal_owned_bus() => {
                    backend::terminal_bus::Attachment::owned(&environment)?
                }
                backend::terminal_bus::Policy::PreserveInherited
                    if !control.terminal_owned_bus() =>
                {
                    backend::terminal_bus::Attachment::default()
                }
                _ => {
                    return Err(
                        "Graphical client did not acknowledge the selected terminal bus policy"
                            .into(),
                    );
                }
            };
            if let Some(router) = &registry.0.clipboard {
                publication.attach_clipboard(router, identity.clone())?;
            }
            let presentation = registry.0.presentation.clone();
            let subscription = connection.subscribe_presentation(move || {
                presentation.send_replace(());
            });
            let driver = Arc::new(Driver {
                identity,
                connection,
                publication,
                operations: Mutex::new(()),
                hud,
                _presentation: subscription,
                account,
            });
            registry.register(&driver)?;
            guard.armed = false;
            Ok(backend::Attached {
                terminal_bus,
                environment,
                driver,
            })
        })
    }
}
/// Session activity follows Welcome as its own packet.
const STARTUP_ACTIVITY_TIMEOUT: Duration = Duration::from_secs(5);
/// A session that stopped during startup says why once its transport is done.
const STARTUP_FINISH_TIMEOUT: Duration = Duration::from_secs(10);

enum Startup {
    Wait,
    /// Welcome was validated and the display reported its first activity.
    Ready,
    Failed(String),
}
/// Bounded waits while a graphical session starts.
#[derive(Default)]
struct StartupWait {
    activity: Option<Instant>,
    finish: Option<Instant>,
}
impl StartupWait {
    fn step(
        &mut self,
        state: SessionState,
        ready: bool,
        now: Instant,
        transport: impl FnOnce() -> Transport,
    ) -> Startup {
        match state {
            SessionState::Connecting => Startup::Wait,
            // Activity can arrive before the complete Welcome has been
            // validated; it alone must never make a shell ready.
            SessionState::Running if ready => Startup::Ready,
            SessionState::Running => {
                if now >= *self.activity.get_or_insert(now + STARTUP_ACTIVITY_TIMEOUT) {
                    Startup::Failed("Shared graphical activity is unavailable".into())
                } else {
                    Startup::Wait
                }
            }
            SessionState::Stopping => {
                if now >= *self.finish.get_or_insert(now + STARTUP_FINISH_TIMEOUT) {
                    Startup::Failed(startup_failure(None, &transport()))
                } else {
                    Startup::Wait
                }
            }
            SessionState::Finished(outcome) => {
                Startup::Failed(startup_failure(Some(outcome), &transport()))
            }
        }
    }
}
/// Why a graphical session ended before it was ready: the session's own
/// failure, and how the agent exited when that adds to it.
fn startup_failure(outcome: Option<SessionOutcome>, transport: &Transport) -> String {
    let mut message = match outcome {
        Some(SessionOutcome::Failed(error)) => error,
        _ => "Graphical session ended during startup".into(),
    };
    let mut details = Vec::new();
    if let Some(code) = transport.exit_code.filter(|code| *code != 0) {
        details.push(format!("the agent exited with status {code}"));
    } else if transport.exited && transport.success == Some(false) {
        details.push("the agent was stopped by a signal".into());
    }
    if let Some(line) = transport
        .stderr_tail
        .lines()
        .map(str::trim)
        .rfind(|line| !line.is_empty())
        .filter(|line| !message.contains(line))
    {
        details.push(line.to_owned());
    }
    if !details.is_empty() {
        message = format!("{message} ({})", details.join("; "));
    }
    message
}

struct CancelAttachment {
    cancellation: Cancellation,
    transport: transport::Control,
    armed: bool,
}
impl Drop for CancelAttachment {
    fn drop(&mut self) {
        if self.armed {
            self.cancellation.cancel();
            self.transport.cancel();
        }
    }
}
struct Publication {
    state: watch::Sender<backend::DriverState>,
    stopped: AtomicBool,
    presentation: watch::Sender<()>,
    clipboard: std::sync::OnceLock<clipboard_display::Display>,
}
impl Publication {
    fn attach_clipboard(&self, router: &Router, identity: backend::Identity) -> Result<(), String> {
        let mut result = Err("Graphical display ended before clipboard attachment".into());
        // Serialize attachment with stop and activity loss, after the complete
        // Welcome/environment has been validated. No initial Connecting state
        // can retire the registration before the display is ready.
        self.state.send_if_modified(|state| {
            if !self.stopped.load(Ordering::Acquire)
                && matches!(state, backend::DriverState::Ready { .. })
            {
                result = router
                    .register_display(identity)
                    .ok_or_else(|| "Clipboard display is already registered or retired".to_owned())
                    .and_then(|display| {
                        self.clipboard
                            .set(display)
                            .map_err(|_| "Clipboard display is already attached".to_owned())
                    });
            }
            false
        });
        result
    }
    fn publish(
        &self,
        activity: Result<Option<newport_remote_client::session_activity::Activity>, String>,
    ) {
        // Check the stop fence while holding the watch write lock. A late IO
        // callback cannot overwrite stop's Closed with an older Ready snapshot.
        let changed = self.state.send_if_modified(|state| {
            let next = if self.stopped.load(Ordering::Acquire) {
                backend::DriverState::Closed
            } else if matches!(state, backend::DriverState::Failed(_)) {
                // A later shutdown notification must not erase the cause, and
                // an obsolete activity update cannot revive a failed display.
                return false;
            } else {
                match activity {
                    Ok(Some(activity)) => {
                        match activity.applications.checked_add(activity.pending_launches) {
                            Some(applications) => backend::DriverState::Ready {
                                windows: activity.windows,
                                applications,
                            },
                            None => {
                                backend::DriverState::Failed("Application demand overflow".into())
                            }
                        }
                    }
                    Ok(None) => backend::DriverState::Closed,
                    Err(error) => backend::DriverState::Failed(error),
                }
            };
            if *state == next {
                return false;
            }
            if !matches!(next, backend::DriverState::Ready { .. })
                && let Some(clipboard) = self.clipboard.get()
            {
                clipboard.revoke();
            }
            *state = next;
            true
        });
        if changed {
            // The activity observer runs under the private controller lock.
            // Only notify here; do not inspect the client or call UI code.
            self.presentation.send_replace(());
        }
    }
    fn stop(&self) {
        self.stopped.store(true, Ordering::Release);
        self.state.send_modify(|state| {
            if let Some(clipboard) = self.clipboard.get() {
                clipboard.revoke();
            }
            *state = backend::DriverState::Closed;
        });
        self.presentation.send_replace(());
    }
}
struct Driver {
    identity: backend::Identity,
    connection: Arc<RemoteSession>,
    publication: Arc<Publication>,
    operations: Mutex<()>,
    hud: bool,
    _presentation: newport_remote_client::PresentationSubscription,
    account: Option<license::Guard>,
}
impl backend::Driver for Driver {
    fn state(&self) -> watch::Receiver<backend::DriverState> {
        self.publication.state.subscribe()
    }
    fn launch(&self, target: backend::launch::Target) -> Result<backend::launch::Reply, String> {
        self.submit(target).map(|submitted| submitted.reply)
    }
    fn launch_application(
        &self,
        target: backend::launch::Target,
    ) -> Result<backend::launch::Submitted, String> {
        self.submit(target)
    }
    fn application_state(
        &self,
        request: std::num::NonZeroU64,
    ) -> Result<backend::launch::State, String> {
        use newport_remote_client::ApplicationState;
        let _admission = self.operations.lock().unwrap();
        if self.publication.stopped.load(Ordering::Acquire) {
            return Err("Graphical session has stopped".into());
        }
        Ok(
            match self.connection.control().application_state(request.get())? {
                ApplicationState::Pending => backend::launch::State::Pending,
                ApplicationState::Opening => backend::launch::State::Opening,
                ApplicationState::Background => backend::launch::State::Background,
                ApplicationState::Running => backend::launch::State::Running,
                ApplicationState::Stopping => backend::launch::State::Stopping,
                ApplicationState::Ended => backend::launch::State::Ended,
            },
        )
    }
    fn application_ending(
        &self,
        request: std::num::NonZeroU64,
    ) -> Result<Option<backend::launch::Ending>, String> {
        use backend::launch::Ending;
        use newport_remote_client::ApplicationEnding;
        let _admission = self.operations.lock().unwrap();
        if self.publication.stopped.load(Ordering::Acquire) {
            return Err("Graphical session has stopped".into());
        }
        // Lifecycle-v1 or feedback-v1 results, whichever the agent granted.
        Ok(
            match self
                .connection
                .control()
                .application_ending(request.get())?
            {
                Some(ApplicationEnding::Failed(message)) => Some(Ending::Failed(message)),
                Some(ApplicationEnding::EndedWithoutWindow(output)) => {
                    Some(Ending::NoWindow(output))
                }
                Some(ApplicationEnding::HandedOff) => Some(Ending::HandedOff),
                None => None,
            },
        )
    }
    fn application_notice(
        &self,
        request: std::num::NonZeroU64,
    ) -> Result<Option<backend::launch::Notice>, String> {
        use backend::launch::{Elsewhere, Notice};
        use newport_remote_protocol::session_application_notice as wire;
        let _admission = self.operations.lock().unwrap();
        if self.publication.stopped.load(Ordering::Acquire) {
            return Err("Graphical session has stopped".into());
        }
        Ok(
            match self
                .connection
                .control()
                .application_notice(request.get())?
            {
                Some(wire::Notice::DialogsElsewhere { display }) => {
                    Some(Notice::DialogsElsewhere(match display {
                        wire::Elsewhere::ServerScreen => Elsewhere::ServerScreen,
                        wire::Elsewhere::OtherDisplay => Elsewhere::OtherDisplay,
                    }))
                }
                None => None,
            },
        )
    }
    fn stop_application(&self, request: std::num::NonZeroU64) -> Result<bool, String> {
        let _admission = self.operations.lock().unwrap();
        if self.publication.stopped.load(Ordering::Acquire) {
            return Err("Graphical session has stopped".into());
        }
        self.connection.control().stop_application(request.get())
    }
    fn stop(&self) {
        let _admission = self.operations.lock().unwrap();
        self.publication.stop();
        self.connection.control().terminate();
    }
}
impl Driver {
    fn submit(
        &self,
        target: backend::launch::Target,
    ) -> Result<backend::launch::Submitted, String> {
        use backend::launch::{Outcome, Target};
        use newport_remote_protocol::session_launch as protocol;
        let target = match target {
            Target::Command { argv } => protocol::Target::Command { argv },
            Target::Desktop { id, uris, action } => protocol::Target::Desktop { id, uris, action },
        };
        target.validate()?;
        let pending = {
            let _admission = self.operations.lock().unwrap();
            license::validate(self.account.as_ref())?;
            if self.publication.stopped.load(Ordering::Acquire) {
                return Err("Graphical session has stopped".into());
            }
            // The client publishes pending activity before returning. Its
            // request table retains demand independently of this reply handle.
            self.connection.control().launch(target)?
        };
        let request = pending
            .request
            .try_into()
            .map_err(|_| "Invalid application request identity")?;
        Ok(backend::launch::Submitted {
            request,
            reply: Box::pin(async move {
                loop {
                    if let Some(result) = pending.try_result() {
                        return match result {
                            Ok(protocol::Outcome::Accepted) => Outcome::Accepted,
                            Ok(protocol::Outcome::Failed { message }) => Outcome::Rejected(message),
                            Err(message) => Outcome::Unknown(message),
                        };
                    }
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            }),
        })
    }
}
impl Drop for Driver {
    fn drop(&mut self) {
        self.stop();
    }
}

fn private_mode(mode: SharedDisplayMode) -> newport_remote_client::SharedDisplayMode {
    match mode {
        SharedDisplayMode::Mixed => newport_remote_client::SharedDisplayMode::Mixed,
        SharedDisplayMode::Wayland => newport_remote_client::SharedDisplayMode::Wayland,
    }
}

fn project_environment(
    environment: &SharedEnvironment,
    identity: &backend::Identity,
    mode: SharedDisplayMode,
) -> Result<SharedGuiEnvironment, String> {
    // The client accepts the application policy only after its negotiation;
    // without it this map has no policy names and projects as before.
    private_mode(mode).validate_application_environment(environment)?;
    let (environment, application) = environment.split_application();
    let environment = &environment;
    let get = |name: &str| {
        environment
            .0
            .get(name)
            .and_then(Option::as_deref)
            .ok_or_else(|| format!("Missing shared display variable {name}"))
    };
    if get("NEWPORT_SESSION")? != identity.session.to_string() {
        return Err("Graphical environment belongs to a different runtime session".into());
    }
    let result = SharedGuiEnvironment {
        session: identity.session,
        display: environment.0["DISPLAY"].clone(),
        authority: environment.0["XAUTHORITY"].clone(),
        wayland_socket: format!("{}/{}", get("XDG_RUNTIME_DIR")?, get("WAYLAND_DISPLAY")?),
        fallback_bus: Some(get("DBUS_SESSION_BUS_ADDRESS")?.into()),
        toolkit: environment
            .0
            .iter()
            .filter(|(name, _)| {
                gui_protocol::environment::allowed(name)
                    && !matches!(
                        name.as_str(),
                        "DISPLAY" | "XAUTHORITY" | "WAYLAND_DISPLAY" | "WAYLAND_SOCKET"
                    )
            })
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect(),
        application,
    };
    mode.validate_environment(&result)?;
    Ok(result)
}

struct TransportDriver {
    control: transport::Control,
    observer: Mutex<Option<client_transport::Observer>>,
}
impl client_transport::Driver for TransportDriver {
    fn cancel(&self) {
        self.control.cancel();
    }
    fn observe(&self, observer: client_transport::Observer) {
        *self.observer.lock().unwrap() = Some(observer);
    }
    fn finish(&self) -> Result<(), String> {
        let report = self.control.finish();
        let observer = self.observer.lock().unwrap().take();
        if let Some(observer) = observer {
            match &report {
                Ok(report) => {
                    for chunk in report.stderr.chunks(32 * 1024) {
                        observer(client_transport::Diagnostic::Output(chunk.to_vec()));
                    }
                    observer(client_transport::Diagnostic::OutputComplete);
                    if report.status.is_some() || report.signal.is_some() {
                        observer(client_transport::Diagnostic::Exit {
                            success: report.failure().is_none(),
                            code: report.status.and_then(|status| i32::try_from(status).ok()),
                        });
                    }
                }
                Err(error) => observer(client_transport::Diagnostic::OutputError(error.clone())),
            }
        }
        match report?.failure() {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests;
#[cfg(test)]
mod startup_tests {
    use super::*;
    fn waits(startup: Startup) -> bool {
        matches!(startup, Startup::Wait)
    }
    fn failure(startup: Startup) -> String {
        match startup {
            Startup::Failed(error) => error,
            _ => panic!("Expected a failure"),
        }
    }
    #[test]
    fn running_waits_a_bounded_time_for_its_first_activity() {
        let now = Instant::now();
        let mut startup = StartupWait::default();
        assert!(waits(startup.step(
            SessionState::Running,
            false,
            now,
            Transport::default
        )));
        let later = now + STARTUP_ACTIVITY_TIMEOUT / 2;
        assert!(waits(startup.step(
            SessionState::Running,
            false,
            later,
            Transport::default
        )));
        assert!(matches!(
            startup.step(SessionState::Running, true, later, Transport::default),
            Startup::Ready
        ));
        let late = now + STARTUP_ACTIVITY_TIMEOUT;
        assert_eq!(
            failure(startup.step(SessionState::Running, false, late, Transport::default)),
            "Shared graphical activity is unavailable"
        );
        // Activity alone, before Welcome, is not ready.
        assert!(waits(StartupWait::default().step(
            SessionState::Connecting,
            true,
            now,
            Transport::default
        )));
    }
    #[test]
    fn a_stopping_session_reports_its_own_failure_once_finished() {
        let now = Instant::now();
        let mut startup = StartupWait::default();
        assert!(waits(startup.step(
            SessionState::Stopping,
            false,
            now,
            Transport::default
        )));
        let transport = || Transport {
            exited: true,
            success: Some(false),
            exit_code: Some(1),
            stderr_tail: "starting\nnewport-remote-agent: Xwayland exited\n".into(),
            ..Transport::default()
        };
        let finished = SessionState::Finished(SessionOutcome::Failed("Remote output ended".into()));
        assert_eq!(
            failure(startup.step(finished, false, now, transport)),
            "Remote output ended (the agent exited with status 1; newport-remote-agent: Xwayland exited)"
        );
        // A failure that already says it is not repeated.
        let said = SessionState::Finished(SessionOutcome::Failed(
            "newport-remote-agent: Xwayland exited".into(),
        ));
        let quiet = || Transport {
            stderr_tail: "newport-remote-agent: Xwayland exited".into(),
            ..Transport::default()
        };
        assert_eq!(
            failure(StartupWait::default().step(said, false, now, quiet)),
            "newport-remote-agent: Xwayland exited"
        );
        // Without an outcome in time, the generic reason remains.
        let late = now + STARTUP_FINISH_TIMEOUT;
        assert_eq!(
            failure(startup.step(SessionState::Stopping, false, late, Transport::default)),
            "Graphical session ended during startup"
        );
        assert_eq!(
            failure(StartupWait::default().step(
                SessionState::Finished(SessionOutcome::RemoteClosed),
                false,
                now,
                Transport::default
            )),
            "Graphical session ended during startup"
        );
    }
}
