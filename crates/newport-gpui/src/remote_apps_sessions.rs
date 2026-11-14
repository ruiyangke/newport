//! Launch intent and observed application status on the backend's shared display.
//! Display, clipboard, transport and license lifetimes belong to the backend;
//! this module submits launches and stops through typed application receipts.
use crate::remote_apps_api as api;
use gpui_kit::App;
use newport_client::remote_apps as shared_apps;
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

/// The selected connection and installed agent, as observed by the page.
pub(crate) struct Launch<'a> {
    pub server: &'a str,
    /// `None` before the snapshot lists the server.
    pub expected_connection: Option<newport_protocol::desktop::configuration::ConnectionScope>,
    pub agent: &'a str,
}

struct Session {
    shared: Option<shared_apps::Application>,
    shared_status: Option<shared_apps::Status>,
    id: api::SessionId,
    server: String,
    app_id: String,
    app_name: String,
    /// Windows are showing.
    running: bool,
    /// Windows appeared at some point, so a failure ends a running session
    /// rather than a launch.
    shown: bool,
    /// Set once the session has finished, with what to say about it if
    /// anything.
    ended: Option<Option<api::Ending>>,
    /// Quit was chosen before the windows appeared.
    cancelled: bool,
    /// Opened from outside the page, by a taskbar pin or shortcut: a launch
    /// that fails says so wherever the person is.
    announce: bool,
}

/// The connection a launch expects, as the backend checks it.
fn scope(launch: &Launch) -> Result<shared_apps::Scope, String> {
    let expected = newport_client::desktop_inputs::captured(launch.expected_connection.as_ref())
        .map_err(|error| error.to_string())?;
    shared_apps::Scope::new(launch.server, expected)
}

pub(crate) struct Sessions {
    shared: RefCell<shared_apps::Controller>,
    sessions: Rc<RefCell<Vec<Session>>>,
    next: Cell<u64>,
    /// Set when session state changes off the page's update path.
    changed: Arc<AtomicBool>,
    /// Record sessions without streaming, for tests.
    scripted: bool,
    /// How often a scripted page showed the server's apps here.
    #[cfg(test)]
    shows: Cell<usize>,
    /// Running sessions that failed, to tell the person wherever they are:
    /// their windows just vanished.
    notices: Arc<std::sync::Mutex<Vec<String>>>,
}

impl Sessions {
    pub fn new() -> Self {
        Self {
            shared: RefCell::new(shared_apps::Controller::new(
                crate::backend::Backend::discover(),
            )),
            sessions: Default::default(),
            next: Cell::new(0),
            changed: Default::default(),
            scripted: false,
            notices: Default::default(),
            #[cfg(test)]
            shows: Cell::new(0),
        }
    }

    /// Sessions that start immediately and stream nothing.
    #[cfg(test)]
    pub fn scripted() -> Self {
        Self {
            scripted: true,
            ..Self::new()
        }
    }

    /// A session whose windows haven't appeared yet.
    #[cfg(test)]
    pub fn record_starting(&self, server: &str, app_id: &str) {
        let id = self.record(server, app_id.into(), app_id.into());
        if let Some(session) = self.sessions.borrow_mut().iter_mut().find(|s| s.id == id) {
            session.running = false;
            session.shown = false;
        }
    }

    /// A launch that ended as `ending` says.
    #[cfg(test)]
    pub fn record_ended(&self, server: &str, app_id: &str, ending: api::Ending) {
        let id = self.record(server, app_id.into(), app_id.into());
        if let Some(session) = self.sessions.borrow_mut().iter_mut().find(|s| s.id == id) {
            session.running = false;
            session.ended = Some(Some(ending));
        }
    }

    /// How often the server's apps were shown here.
    #[cfg(test)]
    pub fn shows(&self) -> usize {
        self.shows.get()
    }

    /// A session another computer shows now.
    #[cfg(test)]
    pub fn record_shown_elsewhere(&self, server: &str, app_id: &str, computer: Option<&str>) {
        let id = self.record(server, app_id.into(), app_id.into());
        if let Some(session) = self.sessions.borrow_mut().iter_mut().find(|s| s.id == id) {
            session.running = false;
            session.shared_status = Some(shared_apps::Status::ShownElsewhere(
                computer.map(str::to_owned),
            ));
        }
    }

    /// Whether this build can stream at all.
    pub fn available(&self) -> bool {
        self.scripted || cfg!(newport_remote_apps)
    }

    /// True once after session state changed.
    pub fn take_changed(&self) -> bool {
        self.changed.swap(false, Ordering::AcqRel)
    }

    pub fn launch(
        &self,
        launch: Launch,
        request: api::LaunchRequest,
        cx: &mut App,
    ) -> Result<api::SessionId, String> {
        let target = launch_target(&request)?;
        let _ = cx;
        self.start_shared(launch, request.app_id, request.name, target)
    }

    fn start_shared(
        &self,
        launch: Launch,
        app_id: String,
        app_name: String,
        target: shared_apps::Target,
    ) -> Result<api::SessionId, String> {
        if launch.agent.is_empty() {
            return Err("Install the full Newport agent on this server first.".into());
        }
        if self.scripted {
            return Ok(self.record(launch.server, app_id, app_name));
        }
        if !self.available() {
            return Err("Remote apps aren’t part of this build.".into());
        }
        if self.active() >= 128 {
            return Err("Close an application before opening another.".into());
        }
        let scope = scope(&launch)?;
        let application = self.shared.borrow().launch(scope, target)?;
        let id = self.record(launch.server, app_id, app_name);
        self.sessions
            .borrow_mut()
            .iter_mut()
            .find(|session| session.id == id)
            .unwrap()
            .shared = Some(application);
        Ok(id)
    }

    /// Show the server's apps on this computer without launching one. A
    /// failure is told wherever the person is, as a failed session's is.
    pub fn show(&self, launch: Launch) -> Result<(), String> {
        if launch.agent.is_empty() {
            return Err("Install the full Newport agent on this server first.".into());
        }
        if self.scripted {
            #[cfg(test)]
            self.shows.set(self.shows.get() + 1);
            return Ok(());
        }
        if !self.available() {
            return Err("Remote apps aren’t part of this build.".into());
        }
        let scope = scope(&launch)?;
        let notices = self.notices.clone();
        self.shared.borrow().show(scope, move |result| {
            if let Err(message) = result {
                notices
                    .lock()
                    .unwrap()
                    .push(format!("Couldn’t show the server’s apps here: {message}"));
            }
        })
    }

    pub fn active(&self) -> usize {
        self.sessions
            .borrow()
            .iter()
            .filter(|session| session.ended.is_none() && !session.cancelled)
            .count()
    }

    /// Stop showing a session that ended, once its alert is dismissed.
    pub fn forget(&self, id: api::SessionId) {
        self.sessions
            .borrow_mut()
            .retain(|session| session.id != id || session.ended.is_none());
        self.changed.store(true, Ordering::Release);
    }

    pub fn list(&self) -> Vec<api::SessionSummary> {
        let mut sessions = self.sessions.borrow_mut();
        // Sessions that ended cleanly leave nothing to show.
        sessions.retain(|session| !matches!(session.ended, Some(None)));
        sessions
            .iter()
            .map(|session| api::SessionSummary {
                id: session.id,
                server: session.server.clone(),
                app_id: session.app_id.clone(),
                app_name: session.app_name.clone(),
                state: match (&session.ended, &session.shared_status) {
                    (_, Some(shared_apps::Status::Unknown(message))) => {
                        api::SessionState::Uncertain {
                            message: message.clone(),
                        }
                    }
                    (None, Some(shared_apps::Status::Stopping)) => api::SessionState::Stopping,
                    (None, Some(shared_apps::Status::Background)) => api::SessionState::Background,
                    (None, Some(shared_apps::Status::ShownElsewhere(computer))) => {
                        api::SessionState::ShownElsewhere {
                            computer: computer.clone(),
                        }
                    }
                    (Some(ending), _) => api::SessionState::Ended {
                        ending: ending.clone(),
                    },
                    (None, _) if session.running => api::SessionState::Running,
                    (None, _) => api::SessionState::Starting,
                },
            })
            .collect()
    }

    pub fn stop(&self, id: api::SessionId) {
        let mut sessions = self.sessions.borrow_mut();
        let Some(session) = sessions.iter_mut().find(|s| s.id == id) else {
            return;
        };
        if self.scripted {
            session.ended = Some(None);
            self.changed.store(true, Ordering::Release);
            return;
        }
        if let Some(application) = &session.shared {
            application.stop();
            self.changed.store(true, Ordering::Release);
            return;
        }
        // Still connecting: the start finishes by closing instead of showing.
        session.cancelled = true;
        session.ended = Some(None);
        self.changed.store(true, Ordering::Release);
    }

    /// Stop every session: a computer without Pro, or signed out, runs nothing
    /// on the account.
    pub fn stop_all(&self) {
        let ids: Vec<_> = self
            .sessions
            .borrow()
            .iter()
            .filter(|session| session.ended.is_none())
            .map(|session| session.id)
            .collect();
        for id in ids {
            self.stop(id);
        }
    }

    /// A launch that fails before its windows appear is told as a running
    /// session's failure is.
    pub fn announce(&self, id: api::SessionId) {
        if let Some(session) = self.sessions.borrow_mut().iter_mut().find(|s| s.id == id) {
            session.announce = true;
        }
    }

    /// Something to tell wherever the person is.
    pub fn notify(&self, message: String) {
        self.notices.lock().unwrap().push(message);
    }

    /// Running sessions that failed since last asked, worded for a toast.
    pub fn take_notices(&self) -> Vec<String> {
        std::mem::take(&mut *self.notices.lock().unwrap())
    }

    fn record(&self, server: &str, app_id: String, app_name: String) -> api::SessionId {
        let id = api::SessionId(self.next.get() + 1);
        self.next.set(id.0);
        self.sessions.borrow_mut().push(Session {
            shared: None,
            shared_status: None,
            id,
            server: server.to_owned(),
            app_id,
            app_name,
            running: self.scripted,
            shown: self.scripted,
            ended: None,
            cancelled: false,
            announce: false,
        });
        self.changed.store(true, Ordering::Release);
        id
    }

    pub fn poll(&self) {
        let mut applications = Vec::new();
        for session in self.sessions.borrow_mut().iter_mut() {
            let Some(application) = &session.shared else {
                continue;
            };
            let status = application.status();
            if session.shared_status.as_ref() != Some(&status) {
                if let Some(notice) = told(session, &status) {
                    self.notices.lock().unwrap().push(notice);
                }
                session.running = matches!(
                    status,
                    shared_apps::Status::Running | shared_apps::Status::Stopping
                );
                session.ended = ending(&status);
                session.shown |= matches!(status, shared_apps::Status::Running);
                session.shared_status = Some(status);
                self.changed.store(true, Ordering::Release);
            }
            if session.ended.is_none() {
                applications.push(application.clone());
            }
        }
        self.shared.borrow_mut().poll(applications);
    }
}

/// What to tell wherever the person is as a session's status changes: its
/// windows vanished, or a launch from a shortcut failed.
fn told(session: &Session, status: &shared_apps::Status) -> Option<String> {
    if session.shown && session.ended.is_none() {
        return match status {
            shared_apps::Status::Unavailable(message) => {
                Some(format!("{}: {message}", session.app_name))
            }
            shared_apps::Status::DisplayStopped => Some(format!(
                "{} closed: the connection to the server dropped or Needle stopped.",
                session.app_name
            )),
            _ => None,
        };
    }
    if !session.announce || session.shown || session.ended.is_some() {
        return None;
    }
    launch_failure(&session.app_name, &ending(status)??)
}

/// Why a launch never showed a window, worded for a toast.
fn launch_failure(app: &str, ending: &api::Ending) -> Option<String> {
    match ending {
        api::Ending::Refused { message, .. }
        | api::Ending::Unavailable(message)
        | api::Ending::Failed(message) => Some(format!("Couldn’t open {app}: {message}")),
        api::Ending::NoWindow(_) => Some(format!("{app} closed without opening a window.")),
        api::Ending::DisplayStopped => Some(format!(
            "Couldn’t open {app}: the connection to the server dropped or Needle stopped."
        )),
        // Its window opened in the instance that was already running.
        api::Ending::HandedOff => None,
    }
}

/// Some once the session finished, with what to say about it if anything.
fn ending(status: &shared_apps::Status) -> Option<Option<api::Ending>> {
    match status {
        shared_apps::Status::Ended => Some(None),
        shared_apps::Status::Refused { message, action } => Some(Some(api::Ending::Refused {
            message: message.clone(),
            action: *action,
        })),
        shared_apps::Status::Unavailable(message) => {
            Some(Some(api::Ending::Unavailable(message.clone())))
        }
        shared_apps::Status::Failed(message) => Some(Some(api::Ending::Failed(message.clone()))),
        shared_apps::Status::EndedWithoutWindow(output) => {
            Some(Some(api::Ending::NoWindow(output.clone())))
        }
        shared_apps::Status::HandedOff => Some(Some(api::Ending::HandedOff)),
        shared_apps::Status::DisplayStopped => Some(Some(api::Ending::DisplayStopped)),
        // Still observed: it runs here again once this computer shows it.
        shared_apps::Status::Starting
        | shared_apps::Status::Background
        | shared_apps::Status::Running
        | shared_apps::Status::Stopping
        | shared_apps::Status::ShownElsewhere(_)
        | shared_apps::Status::Unknown(_) => None,
    }
}

/// Application intent stays on the existing shared display.
fn launch_target(request: &api::LaunchRequest) -> Result<shared_apps::Target, String> {
    let target =
        shared_apps::Target::Desktop(Box::new(newport_protocol::desktop::applications::Desktop {
            id: request.app_id.clone(),
            uris: Vec::new(),
            action: request.action.clone(),
        }));
    target.validate().map_err(|error| error.to_string())?;
    Ok(target)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn desktop_launches_keep_the_application_and_its_action() {
        let desktop = api::LaunchRequest {
            app_id: "google-chrome.desktop".into(),
            name: "Chrome".into(),
            action: Some("new-private-window".into()),
        };
        assert!(
            matches!(launch_target(&desktop).unwrap(), shared_apps::Target::Desktop(value)
            if value.id == desktop.app_id && value.uris.is_empty() && value.action == desktop.action)
        );
    }

    #[test]
    fn a_launch_from_a_shortcut_that_fails_is_told_wherever_the_person_is() {
        let sessions = Sessions::new();
        let quiet = sessions.record("server", "a.desktop".into(), "A".into());
        let announced = sessions.record("server", "b.desktop".into(), "B".into());
        sessions.announce(announced);
        let failure = shared_apps::Status::Failed("b: not found".into());
        let notices: Vec<_> = sessions
            .sessions
            .borrow()
            .iter()
            .filter_map(|session| told(session, &failure))
            .collect();
        assert_eq!(notices, ["Couldn’t open B: b: not found"]);
        assert!(sessions.list().iter().any(|session| session.id == quiet));
        assert_eq!(
            launch_failure("B", &api::Ending::HandedOff),
            None,
            "an app already running takes the launch"
        );
        sessions.notify("This app was removed from Newport.".into());
        assert_eq!(
            sessions.take_notices(),
            ["This app was removed from Newport."]
        );
    }

    #[test]
    fn apps_without_a_window_run_in_the_background_and_hand_offs_end_with_a_note() {
        let sessions = Sessions::new();
        let id = sessions.record("server", "app".into(), "App".into());
        sessions.sessions.borrow_mut()[0].shared_status = Some(shared_apps::Status::Background);
        let list = sessions.list();
        assert_eq!(list[0].id, id);
        assert_eq!(list[0].state, api::SessionState::Background);
        assert!(list[0].state.is_active());
        assert_eq!(sessions.active(), 1);

        assert_eq!(ending(&shared_apps::Status::Background), None);
        assert_eq!(
            ending(&shared_apps::Status::HandedOff),
            Some(Some(api::Ending::HandedOff))
        );
    }

    #[test]
    fn an_app_whose_display_stopped_has_ended() {
        assert_eq!(
            ending(&shared_apps::Status::DisplayStopped),
            Some(Some(api::Ending::DisplayStopped))
        );
    }

    #[test]
    fn an_app_shown_on_another_computer_is_neither_active_here_nor_ended() {
        let sessions = Sessions::new();
        sessions.record("server", "app".into(), "App".into());
        let computer = Some("studio-mac".to_owned());
        let status = shared_apps::Status::ShownElsewhere(computer.clone());
        assert_eq!(ending(&status), None);
        sessions.sessions.borrow_mut()[0].shared_status = Some(status);
        let list = sessions.list();
        assert_eq!(
            list[0].state,
            api::SessionState::ShownElsewhere { computer }
        );
        assert!(!list[0].state.is_active());
    }

    #[test]
    fn an_app_shown_here_again_runs_and_reads_as_shown_elsewhere_after_another_takeover() {
        let sessions = Sessions::new();
        sessions.record("server", "app".into(), "App".into());
        let shown = shared_apps::Status::ShownElsewhere(Some("studio-mac".into()));
        for (status, state) in [
            (shown.clone(), None),
            (
                shared_apps::Status::Running,
                Some(api::SessionState::Running),
            ),
            (shown, None),
        ] {
            {
                let mut list = sessions.sessions.borrow_mut();
                list[0].running = matches!(status, shared_apps::Status::Running);
                list[0].ended = ending(&status);
                list[0].shared_status = Some(status);
            }
            let list = sessions.list();
            // Still tracked whichever computer shows it.
            assert_eq!(list.len(), 1);
            if let Some(state) = state {
                assert_eq!(list[0].state, state);
            } else {
                assert!(matches!(
                    list[0].state,
                    api::SessionState::ShownElsewhere { .. }
                ));
            }
        }
    }

    #[test]
    fn quitting_a_starting_session_ends_it() {
        let sessions = Sessions::new();
        let id = sessions.record("server", "app".into(), "App".into());
        assert_eq!(sessions.active(), 1);
        sessions.stop(id);
        assert_eq!(sessions.active(), 0);
        assert!(sessions.list().is_empty());
    }
}
