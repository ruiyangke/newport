//! Optional GUI display preparation and terminal ownership, separate from PTY I/O.
use crate::{
    remote_apps_api as api,
    remote_apps_sessions::{Launch, Sessions},
};
use gpui_kit::App;
use newport_domain::terminal::GuiEnvironment;
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    time::{Duration, Instant},
};

pub(crate) struct Display {
    sessions: Rc<Sessions>,
    id: Cell<Option<api::SessionId>>,
    pub clipboard: bool,
    allowed: Cell<bool>,
    // None while checking; Ok(false) when no streaming agent is installed.
    checked: RefCell<Option<Result<bool, String>>>,
    started: Instant,
    ready: Cell<bool>,
    #[cfg(test)]
    scripted: Option<Result<Option<GuiEnvironment>, String>>,
}
#[cfg(test)]
pub(crate) struct TestProbe(pub Result<Option<GuiEnvironment>, String>);
#[cfg(test)]
impl gpui_kit::Global for TestProbe {}
impl Display {
    pub fn start(
        server: String,
        sessions: Rc<Sessions>,
        clipboard: bool,
        cx: &mut App,
    ) -> Rc<Self> {
        let display = Rc::new(Self {
            sessions,
            clipboard,
            allowed: Cell::new(clipboard),
            id: Cell::new(None),
            checked: RefCell::new(None),
            started: Instant::now(),
            ready: Cell::new(false),
            #[cfg(test)]
            scripted: cx.try_global::<TestProbe>().map(|p| p.0.clone()),
        });
        #[cfg(test)]
        if display.scripted.is_some() {
            return display;
        }
        let weak = Rc::downgrade(&display);
        cx.spawn(async move |cx| {
            let queried = server.clone();
            let result = cx
                .background_executor()
                .spawn(async move {
                    let backend = newport_client::backend::Backend::discover();
                    let status = backend.call_with_timeout(
                        "remote_apps_status",
                        serde_json::json!({"id":queried,"refresh":true}),
                        Duration::from_secs(15),
                    )?;
                    let settings = backend
                        .call_with_timeout(
                            "remote_apps_settings",
                            serde_json::json!({}),
                            Duration::from_secs(5),
                        )
                        .unwrap_or_default();
                    Ok::<_, String>((status, settings))
                })
                .await;
            let Some(display) = weak.upgrade() else {
                return;
            };
            let result = result.and_then(|(status, settings)| {
                if status["agent"].as_str().is_none_or(str::is_empty) {
                    return Ok(false);
                }
                let Ok(agent) = crate::remote_apps::launch_agent(&status) else {
                    return Ok(false);
                };
                let missing = status["missing"].as_array().is_some_and(|items| {
                    items
                        .iter()
                        .any(|item| item["id"] == "xvfb" || item["id"] == "dbus")
                });
                if missing {
                    return Ok(false);
                }
                let id = cx.update(|cx| {
                    display.sessions.start_terminal(
                        Launch {
                            server: &server,
                            server_name: &server,
                            agent: &agent,
                            x11: true,
                            settings: crate::remote_apps::decode_settings(&settings),
                            clipboard,
                        },
                        cx,
                    )
                })?;
                display.id.set(Some(id));
                display
                    .sessions
                    .set_session_clipboard(id, display.allowed.get());
                Ok(true)
            });
            *display.checked.borrow_mut() = Some(result);
        })
        .detach();
        display
    }
    pub fn poll(&self) -> Option<Result<Option<GuiEnvironment>, String>> {
        #[cfg(test)]
        if let Some(result) = &self.scripted {
            return Some(result.clone());
        }
        match self.checked.borrow().as_ref() {
            Some(Err(error)) => Some(Err(error.clone())),
            Some(Ok(false)) => Some(Ok(None)),
            _ => {
                self.id
                    .get()
                    .and_then(|id| self.sessions.terminal_environment(id))
                    .map(|result| {
                        result.map(|environment| {
                            self.ready.set(true);
                            Some(environment)
                        })
                    })
                    .or_else(|| {
                        (!self.ready.get() && self.started.elapsed() > Duration::from_secs(55))
                            .then(|| {
                                Err(
                                "GUI display startup timed out. Check the server connection and retry."
                                    .into(),
                            )
                            })
                    })
            }
        }
    }
    pub fn error(&self) -> Option<String> {
        if let Some(Err(error)) = self.checked.borrow().as_ref() {
            return Some(error.clone());
        }
        self.id
            .get()
            .and_then(|id| self.sessions.terminal_error(id))
    }
    pub fn reusable(&self) -> bool {
        #[cfg(test)]
        if let Some(result) = &self.scripted {
            return matches!(result, Ok(Some(_)));
        }
        !matches!(self.checked.borrow().as_ref(), Some(Ok(false))) && self.error().is_none()
    }
    pub fn set_clipboard_permission(&self, enabled: bool) {
        self.allowed.set(enabled);
    }
}
impl Drop for Display {
    fn drop(&mut self) {
        if let Some(id) = self.id.get() {
            self.sessions.release_terminal(id);
        }
    }
}
