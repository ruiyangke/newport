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
    pub expected_connection: serde_json::Value,
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
    /// What the server said about the running app.
    notice: Option<api::Notice>,
}

pub(crate) struct Sessions {
    shared: RefCell<shared_apps::Controller>,
    sessions: Rc<RefCell<Vec<Session>>>,
    next: Cell<u64>,
    /// Set when session state changes off the page's update path.
    changed: Arc<AtomicBool>,
    /// Record sessions without streaming, for tests.
    scripted: bool,
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
        let scope = shared_apps::Scope::new(launch.server, launch.expected_connection)?;
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
                    (Some(ending), _) => api::SessionState::Ended {
                        ending: ending.clone(),
                    },
                    (None, _) if session.running => api::SessionState::Running,
                    (None, _) => api::SessionState::Starting,
                },
                notice: session.notice.filter(|_| session.ended.is_none()),
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

    /// Stop every session, as signing out does: a signed-out computer runs
    /// nothing on the account.
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

    /// Running sessions that failed since last asked, worded for a toast.
    pub fn take_notices(&self) -> Vec<String> {
        std::mem::take(&mut *self.notices.lock().unwrap())
    }

    /// Stop the applications owned by this launcher before quitting.
    pub fn shutdown(&self, _cx: &mut App) {
        self.stop_all();
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
            notice: None,
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
            let notice = application.notice().map(notice);
            if session.notice != notice {
                session.notice = notice;
                self.changed.store(true, Ordering::Release);
            }
            let status = application.status();
            if session.shared_status.as_ref() != Some(&status) {
                if session.shown && session.ended.is_none() {
                    // Its windows are gone, wherever the person is looking.
                    let notice = match &status {
                        shared_apps::Status::Unavailable(message) => {
                            Some(format!("{}: {message}", session.app_name))
                        }
                        shared_apps::Status::DisplayStopped => Some(format!(
                            "{} closed: the connection to the server dropped or Needle stopped.",
                            session.app_name
                        )),
                        _ => None,
                    };
                    if let Some(notice) = notice {
                        self.notices.lock().unwrap().push(notice);
                    }
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

fn notice(notice: shared_apps::Notice) -> api::Notice {
    match notice {
        shared_apps::Notice::DialogsElsewhere(shared_apps::Elsewhere::ServerScreen) => {
            api::Notice::DialogsOnServerScreen
        }
        shared_apps::Notice::DialogsElsewhere(shared_apps::Elsewhere::OtherDisplay) => {
            api::Notice::DialogsOnOtherMac
        }
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
        shared_apps::Status::Starting
        | shared_apps::Status::Background
        | shared_apps::Status::Running
        | shared_apps::Status::Stopping
        | shared_apps::Status::Unknown(_) => None,
    }
}

/// Application intent stays on the existing shared display.
fn launch_target(request: &api::LaunchRequest) -> Result<shared_apps::Target, String> {
    let target = shared_apps::Target::Desktop {
        id: request.app_id.clone(),
        uris: Vec::new(),
        action: request.action.clone(),
    };
    target.validate()?;
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
        assert_eq!(
            launch_target(&desktop).unwrap(),
            shared_apps::Target::Desktop {
                id: desktop.app_id,
                uris: vec![],
                action: desktop.action,
            }
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
    fn notices_show_only_while_the_session_is_active() {
        let sessions = Sessions::new();
        sessions.record("server", "chromium".into(), "Chromium".into());
        sessions.sessions.borrow_mut()[0].shared_status = Some(shared_apps::Status::Running);
        sessions.sessions.borrow_mut()[0].notice = Some(api::Notice::DialogsOnServerScreen);
        assert_eq!(
            sessions.list()[0].notice,
            Some(api::Notice::DialogsOnServerScreen)
        );
        sessions.sessions.borrow_mut()[0].ended = Some(Some(api::Ending::HandedOff));
        assert_eq!(sessions.list()[0].notice, None);

        assert_eq!(
            notice(shared_apps::Notice::DialogsElsewhere(
                shared_apps::Elsewhere::ServerScreen
            )),
            api::Notice::DialogsOnServerScreen
        );
        assert_eq!(
            notice(shared_apps::Notice::DialogsElsewhere(
                shared_apps::Elsewhere::OtherDisplay
            )),
            api::Notice::DialogsOnOtherMac
        );
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
