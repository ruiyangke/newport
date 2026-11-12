//! The application composes the public backend with the private GUI client.
//! This registry holds only weak handles; backend demand owns the connection.
use newport_backend::{
    account::RemoteLicense,
    clipboard_router::{Router, display as clipboard_display},
    graphical_session::{self as backend, Driver as _, transport},
    preferences::remote_apps::{Codec, Settings},
};
use newport_protocol::remote_apps::performance::CodecMode;
use newport_remote_client::{
    RemoteSession, SessionLicense, SessionOptions, SessionOutcome, SessionState,
    diagnostics::Transport,
};
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
    /// The stored setting to present remote windows as separate apps.
    separate_apps: AtomicBool,
    account:
        std::sync::OnceLock<watch::Receiver<newport_backend::account::remote_sessions::Policy>>,
}
/// A short-lived UI attachment candidate. Backend demand owns its lifetime;
/// callers must not retain this client while no presenter is attached.
pub struct Presentation {
    pub key: String,
    pub connection: Arc<RemoteSession>,
    pub hud: bool,
    pub server: backend::ServerLabel,
    owner: Weak<Driver>,
}
impl Presentation {
    /// Claim only when attaching a presenter, not while enumerating candidates.
    /// The returned weak handle cannot retain the graphical driver or transport.
    pub fn clipboard_presenter(&self) -> Option<clipboard_display::Presenter> {
        let driver = self.owner.upgrade()?;
        let _admission = driver
            .operations
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
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
            separate_apps: AtomicBool::new(false),
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
    pub fn factory(
        &self,
        license: RemoteLicense,
        settings: Settings,
        display: Option<newport_protocol::remote_apps::display::DisplayProfile>,
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
                ime: true,
                #[cfg(windows)]
                file_upload: true,
                // Drag out of a remote app into Explorer, for text and files.
                #[cfg(windows)]
                reverse_drag: true,
                #[cfg(windows)]
                reverse_files: true,
                trace: cfg!(debug_assertions)
                    && std::env::var_os("NEWPORT_REMOTE_TRACE").is_some_and(|value| value == "1"),
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
    /// The setting as last saved. A change applies to displays already
    /// shown too: their presenters hand them to one in the new mode.
    pub fn set_separate_apps(&self, separate_apps: bool) {
        if self.0.separate_apps.swap(separate_apps, Ordering::AcqRel) != separate_apps {
            self.recheck_presentation();
        }
    }
    pub fn separate_apps(&self) -> bool {
        self.0.separate_apps.load(Ordering::Acquire)
    }
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
            let clients = self
                .0
                .clients
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
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
                    server: client.server.clone(),
                    owner: Arc::downgrade(&client),
                })
            })
            .collect()
    }
    pub fn close(&self) {
        let clients = {
            let mut clients = self
                .0
                .clients
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
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
        let mut clients = self
            .0
            .clients
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
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
        server: backend::ServerLabel,
    ) -> backend::AttachFuture {
        let registry = self.registry.clone();
        let options = self.options.clone();
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
                ending: AtomicBool::new(false),
                presentation: registry.0.presentation.clone(),
                clipboard: Default::default(),
            });
            let observed = publication.clone();
            let control = connection.control();
            let outcome = control.clone();
            control.observe_activity_status(Arc::new(move |activity| {
                if observed.publish(activity) {
                    // The observer runs under the client's lock, and the
                    // outcome follows once its transport is done.
                    let (publication, control) = (observed.clone(), outcome.clone());
                    let waiting = std::thread::Builder::new()
                        .name("graphical-ending".into())
                        .spawn(move || publication.finish(control.wait_timeout(ENDING_TIMEOUT)));
                    if waiting.is_err() {
                        observed.finish(None);
                    }
                }
            }))?;
            let mut startup = StartupWait::default();
            loop {
                let ready = matches!(
                    *publication.state.borrow(),
                    backend::DriverState::Ready { .. }
                );
                match startup.step(control.state(), ready, Instant::now(), || {
                    control.diagnostics().transport
                }) {
                    Startup::Wait => {}
                    Startup::Ready => break,
                    Startup::Failed(error) => return Err(error),
                }
                tokio::select! {
                    _ = license::revoked(account.as_ref()) => return Err("Remote apps account access changed".into()),
                    _ = closing.wait_for(|closed| *closed) => return Err("Application is closing".into()),
                    _ = tokio::time::sleep(Duration::from_millis(10)) => {},
                }
            }
            if let Some(router) = &registry.0.clipboard {
                publication.attach_clipboard(router, identity.clone())?;
            }
            let presentation = registry.0.presentation.clone();
            let subscription = connection.subscribe_presentation(move || {
                presentation.send_replace(());
            });
            let driver = Arc::new(Driver {
                identity,
                server,
                connection,
                publication,
                operations: Mutex::new(()),
                hud,
                _presentation: subscription,
                account,
            });
            registry.register(&driver)?;
            guard.armed = false;
            Ok(backend::Attached { driver })
        })
    }
}
/// Welcome initializes the activity state observed by the startup watcher.
const STARTUP_ACTIVITY_TIMEOUT: Duration = Duration::from_secs(5);
/// A session that stopped during startup says why once its transport is done.
const STARTUP_FINISH_TIMEOUT: Duration = Duration::from_secs(10);
/// A ready session that ended says how once its transport is done.
const ENDING_TIMEOUT: Duration = Duration::from_secs(10);

enum Startup {
    Wait,
    /// Welcome was validated and the activity state is available.
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
        Some(SessionOutcome::TakenOver(takeover)) => format!(
            "Shown on {}.",
            takeover.computer.as_deref().unwrap_or("another computer")
        ),
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
    /// The ready session reported that nothing is left; its outcome decides
    /// what to publish.
    ending: AtomicBool,
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
                        // The account's display, shown here: what any of its
                        // programs copies reaches this computer, terminals'
                        // included, without a focused window. Of several
                        // shown here, the one whose terminal was used last.
                        display.stand();
                        self.clipboard
                            .set(display)
                            .map_err(|_| "Clipboard display is already attached".to_owned())
                    });
            }
            false
        });
        result
    }
    /// True when a ready session reported that nothing is left: it is
    /// ending, and only its outcome says how. Call `finish` with it then.
    fn publish(
        &self,
        activity: Result<Option<newport_remote_client::session_activity::Activity>, String>,
    ) -> bool {
        let mut ending = false;
        // Check the stop fence while holding the watch write lock. A late IO
        // callback cannot overwrite stop's Closed with an older Ready snapshot.
        let changed = self.state.send_if_modified(|state| {
            let next = if self.stopped.load(Ordering::Acquire) {
                backend::DriverState::Closed
            } else if matches!(
                state,
                backend::DriverState::Failed(_) | backend::DriverState::ShownElsewhere(_)
            ) {
                // A later shutdown notification must not erase the cause, and
                // an obsolete activity update cannot revive a failed display.
                return false;
            } else {
                match activity {
                    // Nothing revives a session that is ending.
                    Ok(_) if self.ending.load(Ordering::Acquire) => return false,
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
                    Ok(None) if matches!(state, backend::DriverState::Ready { .. }) => {
                        self.ending.store(true, Ordering::Release);
                        ending = true;
                        if let Some(clipboard) = self.clipboard.get() {
                            clipboard.revoke();
                        }
                        return false;
                    }
                    Ok(None) => backend::DriverState::Closed,
                    Err(error) => backend::DriverState::Failed(error),
                }
            };
            self.replace(state, next)
        });
        if changed {
            // The activity observer runs under the private controller lock.
            // Only notify here; do not inspect the client or call UI code.
            self.presentation.send_replace(());
        }
        ending
    }
    /// How an ending session ended: shown on another computer, failed, or
    /// closed, which is also what a session that never says becomes.
    fn finish(&self, outcome: Option<SessionOutcome>) {
        let next = match outcome {
            Some(SessionOutcome::TakenOver(takeover)) => {
                backend::DriverState::ShownElsewhere(backend::launch::ShownOn {
                    computer: takeover.computer,
                    session: takeover.session,
                })
            }
            Some(SessionOutcome::Failed(error)) => backend::DriverState::Failed(error),
            _ => backend::DriverState::Closed,
        };
        let changed = self.state.send_if_modified(|state| {
            if self.stopped.load(Ordering::Acquire)
                || matches!(
                    state,
                    backend::DriverState::Failed(_) | backend::DriverState::ShownElsewhere(_)
                )
            {
                return false;
            }
            self.replace(state, next)
        });
        if changed {
            self.presentation.send_replace(());
        }
    }
    fn replace(&self, state: &mut backend::DriverState, next: backend::DriverState) -> bool {
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
    server: backend::ServerLabel,
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
        let _admission = self
            .operations
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
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
        let _admission = self
            .operations
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
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
                Some(ApplicationEnding::Failed(message)) => Some(Ending::Failed(Box::new(message))),
                Some(ApplicationEnding::EndedWithoutWindow(output)) => {
                    Some(Ending::NoWindow(Box::new(
                        newport_protocol::desktop::applications::OptionalDetail { detail: output },
                    )))
                }
                Some(ApplicationEnding::HandedOff) => Some(Ending::HandedOff(Box::new(
                    newport_protocol::common::Present {},
                ))),
                None => None,
            },
        )
    }
    fn stop_application(&self, request: std::num::NonZeroU64) -> Result<bool, String> {
        let _admission = self
            .operations
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if self.publication.stopped.load(Ordering::Acquire) {
            return Err("Graphical session has stopped".into());
        }
        self.connection.control().stop_application(request.get())
    }
    fn applications(&self) -> Vec<backend::launch::Shown> {
        if self.publication.stopped.load(Ordering::Acquire) {
            return Vec::new();
        }
        self.connection
            .control()
            .window_applications()
            .into_iter()
            .map(|(app_id, windows)| backend::launch::Shown { app_id, windows })
            .collect()
    }
    fn stand(&self) {
        if let Some(clipboard) = self.publication.clipboard.get() {
            clipboard.stand();
        }
    }
    fn stop(&self) {
        let _admission = self
            .operations
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
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
        use newport_protocol::remote_apps::session_launch as protocol;
        let target = match target {
            Target::Command(command) => protocol::Target::Command { argv: command.argv },
            Target::Desktop(desktop) => protocol::Target::Desktop {
                id: desktop.id,
                uris: desktop.uris,
                action: desktop.action,
            },
        };
        target.validate()?;
        let pending = {
            let _admission = self
                .operations
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
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
                            Ok(protocol::Outcome::Accepted) => {
                                Outcome::Accepted(Box::new(newport_protocol::common::Present {}))
                            }
                            Ok(protocol::Outcome::Failed { message }) => {
                                Outcome::Rejected(Box::new(message))
                            }
                            Err(message) => Outcome::Unknown(Box::new(message)),
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

struct TransportDriver {
    control: transport::Control,
    observer: Mutex<Option<client_transport::Observer>>,
}
impl client_transport::Driver for TransportDriver {
    fn cancel(&self) {
        self.control.cancel();
    }
    fn observe(&self, observer: client_transport::Observer) {
        *self
            .observer
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(observer);
    }
    fn finish(&self) -> Result<(), String> {
        let report = self.control.finish();
        let observer = self
            .observer
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
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
