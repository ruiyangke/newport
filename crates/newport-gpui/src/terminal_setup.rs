//! A GUI launch owns account/setup guidance; ordinary SSH never waits for it.
use super::*;
use crate::terminal_view::TerminalView;
use newport_client::account::{Action, NoticeKind};
use newport_domain::terminal::GuiEnvironment;
use remote_apps::Entitlement;
use std::{
    collections::{HashMap, VecDeque},
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

type Reply = Box<dyn FnOnce(Result<GuiEnvironment, String>) + Send>;

#[derive(Clone)]
struct Origin {
    server: String,
    id: uuid::Uuid,
    view: WeakEntity<TerminalView>,
}
impl Origin {
    fn live(&self, cx: &App) -> bool {
        self.view.upgrade().is_some_and(|view| {
            let view = view.read(cx);
            view.session_id() == Some(self.id) && view.active()
        })
    }
}
struct Request {
    server: String,
    application: String,
    manual: bool,
    origin: Option<Origin>,
    reply: Option<Reply>,
    cancelled: Arc<AtomicBool>,
    routed: bool,
    return_page: Page,
}
impl Request {
    fn live(&self, cx: &App) -> bool {
        !self.cancelled.load(Ordering::Acquire)
            && self.origin.as_ref().is_none_or(|origin| origin.live(cx))
    }
}
impl Drop for Request {
    fn drop(&mut self) {
        if let Some(reply) = self.reply.take() {
            reply(Err(
                "GUI launch cancelled. Run the command again to retry.".into()
            ));
        }
    }
}
#[derive(Default)]
struct Requests {
    queue: VecDeque<Request>,
    terminals: HashMap<uuid::Uuid, Origin>,
}
impl Global for Requests {}
fn requests(cx: &mut App) -> &mut Requests {
    if !cx.has_global::<Requests>() {
        cx.set_global(Requests::default());
    }
    cx.global_mut::<Requests>()
}
pub(crate) fn register(id: uuid::Uuid, server: &str, view: WeakEntity<TerminalView>, cx: &mut App) {
    let stale: Vec<_> = cx
        .try_global::<Requests>()
        .map(|r| {
            r.terminals
                .iter()
                .filter_map(|(id, origin)| {
                    (origin.view.entity_id() == view.entity_id() || !origin.live(cx)).then_some(*id)
                })
                .collect()
        })
        .unwrap_or_default();
    let requests = requests(cx);
    for id in stale {
        requests.terminals.remove(&id);
    }
    requests.terminals.insert(
        id,
        Origin {
            id,
            server: server.into(),
            view,
        },
    );
}
pub(crate) fn request(server: &str, manual: bool, cx: &mut App) {
    if !remote_apps::available(cx) {
        return;
    }
    let requests = requests(cx);
    if requests.queue.len() >= 32
        || requests
            .queue
            .iter()
            .any(|r| r.origin.is_none() && r.server == server)
    {
        return;
    }
    requests.queue.push_back(Request {
        server: server.into(),
        application: "GUI app".into(),
        manual,
        origin: None,
        reply: None,
        cancelled: Arc::new(AtomicBool::new(false)),
        routed: false,
        return_page: Page::Terminal,
    });
}
pub(crate) fn request_launch(
    server: String,
    terminal: String,
    application: String,
    reply: Reply,
    cancelled: Arc<AtomicBool>,
    cx: &mut App,
) {
    let origin = uuid::Uuid::parse_str(&terminal)
        .ok()
        .and_then(|id| cx.try_global::<Requests>()?.terminals.get(&id).cloned())
        .filter(|origin| origin.server == server && origin.live(cx));
    if !remote_apps::available(cx) || origin.is_none() || cancelled.load(Ordering::Acquire) {
        reply(Err("The originating terminal is no longer connected. Run the command again in its current session.".into()));
        return;
    }
    let requests = requests(cx);
    if requests.queue.len() >= 32 {
        reply(Err("Too many pending GUI launches. Try again.".into()));
        return;
    }
    requests.queue.push_back(Request {
        server,
        application,
        manual: false,
        origin,
        reply: Some(reply),
        cancelled,
        routed: false,
        return_page: Page::Terminal,
    });
}

#[derive(Clone, Copy, Default, PartialEq)]
enum Stage {
    #[default]
    Account,
    Starting,
    Setup,
    Failed,
}
#[derive(Default)]
pub(super) struct State {
    dismissed: Option<Instant>,
    dismissed_server: Option<String>,
    open: Option<String>,
    error: Option<String>,
    current: Option<Request>,
    display: Option<Rc<crate::terminal_gui::Display>>,
    stage: Stage,
    return_to: Option<(Origin, Page)>,
    syncing: bool,
}
struct Step {
    detail: String,
    action: Option<(&'static str, Action)>,
}
fn step(entitlement: Entitlement, seat_message: Option<&str>) -> Step {
    let (detail, action) = match entitlement {
        Entitlement::Unknown => ("Checking your account…".into(), None),
        Entitlement::SignedOut => (
            "Sign in to open this app. GUI apps require Newport Pro or a free trial.".into(),
            Some(("Sign in", Action::SignIn))),
        Entitlement::Waiting => (
            "Finish signing in in your browser. This launch will continue when access is ready.".into(),
            Some(("Open sign-in page", Action::SignIn))),
        Entitlement::TrialAvailable => (
            "Start a free 14-day trial to open GUI apps on this computer. No card needed.".into(),
            Some(("Start free trial", Action::StartTrial))),
        Entitlement::TrialUsedHere => (
            "This computer has already used a free trial. Upgrade to Newport Pro to open this app.".into(),
            Some(("Upgrade to Pro", Action::OpenBilling))),
        Entitlement::Ended { .. } => (
            "There’s no active Newport Pro license on this computer. Activate Pro in billing to continue.".into(),
            Some(("Open billing", Action::OpenBilling))),
        Entitlement::FreeSeat => (
            seat_message.filter(|s| !s.trim().is_empty()).unwrap_or("Assign a Pro seat to this computer to open this app.").into(),
            Some(("Manage computers", Action::OpenPortal))),
        Entitlement::TrialElsewhere => (
            "Your free trial is on another computer. Upgrade to Newport Pro to open GUI apps here.".into(),
            Some(("Upgrade to Pro", Action::OpenBilling))),
        Entitlement::Pro => ("Starting the app…".into(), None),
    };
    Step { detail, action }
}
fn setup_ready(status: &Value) -> bool {
    remote_apps::launch_agent(status).is_ok()
        && !status["missing"].as_array().is_some_and(|missing| {
            missing
                .iter()
                .any(|item| item["id"] == "xvfb" || item["id"] == "dbus")
        })
}
impl Newport {
    fn begin_terminal_display(&mut self, cx: &mut Context<Self>) {
        let Some(request) = &self.terminal_setup.current else {
            return;
        };
        remote_apps::follow_account(self.account.status(), cx);
        self.terminal_setup.display = remote_apps::terminal_display(&request.server, cx);
        self.terminal_setup.error = None;
        self.terminal_setup.stage = if self.terminal_setup.display.is_some() {
            Stage::Starting
        } else {
            self.terminal_setup.error =
                Some("Couldn’t prepare GUI apps. Retry the connection.".into());
            Stage::Failed
        };
    }
    pub(super) fn poll_terminal_setup(&mut self, cx: &mut Context<Self>) -> bool {
        let mut changed = false;
        if self.terminal_setup.current.is_none() {
            while let Some(mut request) = if cx.has_global::<Requests>() {
                cx.global_mut::<Requests>().queue.pop_front()
            } else {
                None
            } {
                let quiet = !request.manual
                    && request.reply.is_none()
                    && self.terminal_setup.dismissed_server.as_deref() == Some(&request.server)
                    && self
                        .terminal_setup
                        .dismissed
                        .is_some_and(|at| at.elapsed() < Duration::from_secs(3));
                if quiet
                    || !request.live(cx)
                    || !rows(&self.snapshot["config"]["servers"])
                        .iter()
                        .any(|s| text(s, "id") == request.server)
                {
                    continue;
                }
                if request
                    .origin
                    .as_ref()
                    .is_some_and(|origin| self.project_terminal_sessions(cx).contains(&origin.id))
                {
                    request.return_page = Page::Projects;
                }
                self.terminal_setup.current = Some(request);
                self.terminal_setup.stage = Stage::Account;
                self.terminal_setup.error = None;
                changed = true;
                break;
            }
        }
        let Some(request) = &self.terminal_setup.current else {
            return changed;
        };
        if !request.live(cx)
            || !rows(&self.snapshot["config"]["servers"])
                .iter()
                .any(|s| text(s, "id") == request.server)
        {
            self.dismiss_terminal_setup();
            return true;
        }
        let entitlement = Entitlement::of(self.account.status());
        if entitlement != Entitlement::Pro {
            if self.terminal_setup.stage != Stage::Account {
                self.terminal_setup.display = None;
                self.terminal_setup.stage = Stage::Account;
                changed = true;
            }
            return changed;
        }
        match self.terminal_setup.stage {
            Stage::Account => {
                self.begin_terminal_display(cx);
                changed = true;
            }
            Stage::Setup => {
                let ready = self
                    .resources
                    .get(&remote_apps::status_key(&request.server))
                    .is_some_and(|read| {
                        !read.pending && read.error.is_none() && setup_ready(&read.value)
                    });
                if ready {
                    self.begin_terminal_display(cx);
                    changed = true;
                }
            }
            _ => {}
        }
        if self.terminal_setup.stage == Stage::Starting
            && let Some(result) = self
                .terminal_setup
                .display
                .as_ref()
                .and_then(|display| display.poll())
        {
            changed = true;
            match result {
                Ok(Some(environment)) => {
                    let mut request = self.terminal_setup.current.take().unwrap();
                    let display = self.terminal_setup.display.take().unwrap();
                    if let Some(origin) = &request.origin {
                        if let Some(view) = origin.view.upgrade().filter(|_| origin.live(cx)) {
                            view.update(cx, |view, _| view.retain_gui(display));
                        } else {
                            return true;
                        }
                        if request.routed {
                            self.terminal_setup.return_to =
                                Some((origin.clone(), request.return_page));
                        }
                    }
                    if let Some(reply) = request.reply.take() {
                        reply(Ok(environment));
                    } else {
                        self.notification = Some(Notification::success(
                            "Run the app by its command name in the terminal, or launch it from Remote apps.",
                        ));
                    }
                }
                Ok(None) => {
                    self.terminal_setup.display = None;
                    self.terminal_setup.stage = Stage::Setup;
                }
                Err(error) => {
                    self.terminal_setup.display = None;
                    self.terminal_setup.error = Some(error);
                    self.terminal_setup.stage = Stage::Failed;
                }
            }
        }
        changed
    }
    pub(super) fn terminal_setup_notice(&mut self, notice: &newport_client::account::Notice) {
        if self.terminal_setup.current.is_some() && self.terminal_setup.stage == Stage::Account {
            self.terminal_setup.error =
                (notice.kind == NoticeKind::Error).then(|| notice.message.clone());
        }
    }
    fn terminal_launch_server(&mut self, server: String) {
        if self.server == server {
            return;
        }
        self.save_workspace_selection();
        self.suspend_connections();
        self.suspend_project_library();
        self.reset();
        self.server = server;
        self.restore_git = self.git_destinations.get(&self.server).cloned();
        self.restore_connections();
    }
    pub(super) fn sync_terminal_setup(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.poll_terminal_setup(cx);
        if self.terminal_setup.syncing
            || (self.terminal_setup.current.is_none()
                && self.terminal_setup.open.is_none()
                && self.terminal_setup.return_to.is_none())
        {
            return;
        }
        self.terminal_setup.syncing = true;
        cx.defer_in(window, |this, window, cx| {
            this.terminal_setup.syncing = false;
            let entitlement = Entitlement::of(this.account.status());
            let loading = entitlement == Entitlement::Unknown
                && this.account.status().phase != newport_client::account::Phase::SignedIn;
            let wants_dialog = this.terminal_setup.current.is_some()
                && matches!(this.terminal_setup.stage, Stage::Account | Stage::Failed)
                && !loading;
            if !wants_dialog && this.terminal_setup.open.take().is_some() {
                window.close_dialog(cx);
                this.modal_open = window.has_active_dialog(cx);
                cx.notify();
            }
            if window.has_active_dialog(cx) || this.server_form.is_some() {
                return;
            }
            if let Some((origin, page)) = this
                .terminal_setup
                .return_to
                .take()
                .filter(|(o, _)| o.live(cx))
            {
                this.terminal_launch_server(origin.server.clone());
                this.navigate(page, cx);
                if page == Page::Projects {
                    for retained in &this.code_workspaces {
                        let editor = retained.editor.clone();
                        let terminal = editor.read(cx).terminal.clone().filter(|terminal| {
                            terminal.read(cx).session_ids(cx).contains(&origin.id)
                        });
                        if let Some(terminal) = terminal {
                            editor.update(cx, |editor, cx| {
                                editor.terminal_visible = true;
                                if editor.narrow {
                                    editor.show_tree = false;
                                }
                                cx.notify();
                            });
                            terminal.update(cx, |workspace, cx| {
                                workspace.select_session(origin.id, window, cx);
                            });
                            break;
                        }
                    }
                } else if let Some(workspace) = this.terminals.get(&origin.server) {
                    workspace.update(cx, |workspace, cx| {
                        workspace.select_session(origin.id, window, cx);
                    });
                }
                if let Some(view) = origin.view.upgrade() {
                    view.read(cx).focus.clone().focus(window, cx);
                }
                cx.notify();
            }
            if this.terminal_setup.stage == Stage::Setup
                && let Some(request) = this.terminal_setup.current.as_mut().filter(|r| !r.routed)
            {
                request.routed = true;
                let server = request.server.clone();
                this.terminal_launch_server(server);
                this.navigate(Page::RemoteApps, cx);
                this.focus.focus(window, cx);
                cx.notify();
                return;
            }
            if !wants_dialog {
                return;
            }
            let server = this.terminal_setup.current.as_ref().unwrap().server.clone();
            this.terminal_setup.open = Some(server);
            let owner = cx.weak_entity();
            window.open_dialog(cx, move |dialog, window, _| {
                let close = owner.clone();
                let body = owner.clone();
                dialog
                    .title(
                        crate::controls::dialog_title("Open a GUI app")
                            .id("terminal-gui-setup-title")
                            .role(accesskit::Role::Heading)
                            .aria_level(1)
                            .aria_label("Open a GUI app"),
                    )
                    .w(px(480.))
                    .max_w((window.viewport_size().width - px(32.)).max(px(240.)))
                    .p(px(24.))
                    .rounded(px(crate::ui::RADIUS))
                    .border_0()
                    .centered(true)
                    .overlay_closable(false)
                    .on_ok(|_, _, _| false)
                    .on_close(move |_, _, cx| {
                        let _ = close.update(cx, |this, cx| {
                            if this.terminal_setup.open.is_some() {
                                this.dismiss_terminal_setup();
                                cx.notify();
                            }
                        });
                    })
                    .content(move |content, window, cx| {
                        let body = body
                            .update(cx, |this, cx| this.terminal_setup_body(cx))
                            .unwrap_or_else(|_| div().into_any_element());
                        content.child(crate::project_dialog::scroll_body(
                            body,
                            window.viewport_size().height,
                        ))
                    })
            });
            cx.notify();
        });
    }
    fn dismiss_terminal_setup(&mut self) {
        if let Some(request) = self.terminal_setup.current.take() {
            self.terminal_setup.dismissed_server = Some(request.server.clone());
            if request.routed
                && let Some(origin) = &request.origin
            {
                self.terminal_setup.return_to = Some((origin.clone(), request.return_page));
            }
        }
        self.terminal_setup.display = None;
        self.terminal_setup.error = None;
        self.terminal_setup.dismissed = Some(Instant::now());
        // Keep `open` until sync removes this dialog, including cancellation from the remote process.
    }
    pub(super) fn terminal_launch_banner(&self, cx: &mut Context<Self>) -> Option<Div> {
        let request = self
            .terminal_setup
            .current
            .as_ref()
            .filter(|r| r.routed && r.server == self.server)?;
        Some(
            div()
                .flex()
                .items_center()
                .gap(px(12.))
                .px(px(20.))
                .py(px(8.))
                .child(div().flex_1().child(format!(
                    "Complete setup to open {} in your terminal.",
                    request.application
                )))
                .child(
                    crate::controls::form_button("terminal-gui-cancel-setup")
                        .label("Cancel launch")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.dismiss_terminal_setup();
                            cx.notify();
                        })),
                ),
        )
    }
    fn terminal_setup_body(&self, cx: &mut Context<Self>) -> AnyElement {
        let status = self.account.status();
        let entitlement = Entitlement::of(status);
        let failed = self.terminal_setup.stage == Stage::Failed;
        let license_missing = entitlement == Entitlement::Unknown
            && status.phase == newport_client::account::Phase::SignedIn;
        let mut step = step(
            entitlement,
            status.license.as_ref().and_then(|l| l.message.as_deref()),
        );
        if license_missing {
            step.detail = "Newport couldn’t load your license. Refresh your account to check access to GUI apps.".into();
        }
        if failed {
            step.detail =
                "Couldn’t start the GUI session. Check the server connection and try again.".into();
            step.action = None;
        }
        let busy = self.account.pending().is_some() || status.signing_out;
        let waiting = self
            .terminal_setup
            .current
            .as_ref()
            .is_some_and(|r| r.reply.is_some());
        let help = if waiting {
            "Your app is waiting in its original terminal. Cancel to return to the shell."
        } else {
            "Shell commands remain available. After setup, run your GUI app command again."
        };
        div()
            .flex()
            .flex_col()
            .gap(px(16.))
            .pt(px(8.))
            .child(crate::project_dialog::description(
                "terminal-gui-setup-detail",
                step.detail,
            ))
            .child(crate::project_dialog::description(
                "terminal-gui-setup-help",
                help,
            ))
            .when_some(self.terminal_setup.error.clone(), |body, error| {
                body.child(
                    crate::controls::alert("terminal-gui-setup-error", error, cx).test_support(),
                )
            })
            .child(
                crate::project_dialog::actions()
                    .child(
                        crate::controls::form_button("terminal-gui-continue")
                            .label(if waiting {
                                "Cancel launch"
                            } else {
                                "Continue with terminal"
                            })
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.dismiss_terminal_setup();
                                this.terminal_setup.open = None;
                                window.close_dialog(cx);
                                this.modal_open = window.has_active_dialog(cx);
                                cx.notify();
                            })),
                    )
                    .when_some(step.action, |actions, (label, action)| {
                        actions.child(
                            crate::controls::form_button("terminal-gui-account")
                                .primary()
                                .label(label)
                                .loading(busy)
                                .disabled(busy)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.terminal_setup.error = None;
                                    this.run_account(action, cx);
                                })),
                        )
                    })
                    .when(failed, |actions| {
                        actions.child(
                            crate::controls::form_button("terminal-gui-retry")
                                .primary()
                                .label("Retry")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.terminal_setup.stage = Stage::Account;
                                    this.terminal_setup.error = None;
                                    cx.notify();
                                })),
                        )
                    })
                    .when(license_missing, |actions| {
                        actions.child(
                            crate::controls::form_button("terminal-gui-refresh")
                                .primary()
                                .label("Refresh account")
                                .disabled(busy)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.terminal_setup.error = None;
                                    this.account.refresh();
                                    cx.notify();
                                })),
                        )
                    }),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::{request, step};
    use crate::{
        Newport, Page,
        remote_apps::{self, Entitlement},
    };
    use gpui_kit::component::WindowExt;
    use gpui_kit::{
        self as gpui, AppContext, Entity, Modifiers, TestAppContext, VisualTestContext, px, size,
    };
    use newport_client::account::{Account, Action};
    use serde_json::{Value, json};
    use std::{cell::RefCell, rc::Rc};

    fn frame(cx: &mut VisualTestContext) {
        crate::page_perf_tests::frame(cx);
    }

    fn labels(cx: &mut VisualTestContext) -> Vec<String> {
        cx.update(|window, _| gpui::base::test_support::snapshots(window))
            .iter()
            .filter_map(|node| node.label().map(str::to_owned))
            .collect()
    }

    fn click(cx: &mut VisualTestContext, label: &str) {
        let bounds = cx
            .update(|window, _| gpui::base::test_support::snapshots(window))
            .into_iter()
            .find(|node| {
                node.label() == Some(label)
                    && node
                        .path()
                        .last()
                        .is_some_and(|id| id.to_string().starts_with("terminal-gui-"))
            })
            .unwrap_or_else(|| panic!("missing {label}"))
            .bounds();
        cx.simulate_click(bounds.center(), Modifiers::none());
        frame(cx);
    }

    fn account(status: Value) -> Account {
        let mut account = Account::with_replies(move |_| Ok(status.clone()));
        account.poll();
        account.poll();
        account
    }

    fn fixture(
        cx: &mut TestAppContext,
        status: Value,
    ) -> (Entity<Newport>, &mut VisualTestContext) {
        cx.update(|cx| {
            gpui::component::init(cx);
            crate::theme::install(false, cx);
            remote_apps::start_scripted(cx);
        });
        let mut owner = None;
        let (_, visual) = cx.add_window_view(|window, cx| {
            let view = cx.new(|cx| {
                let mut view = crate::page_perf_tests::owner(window, cx);
                Newport::observe_modal_visibility(window, cx);
                view.account = account(status);
                view
            });
            owner = Some(view.clone());
            gpui::base::Root::new(view, window, cx)
        });
        visual.simulate_resize(size(px(700.), px(500.)));
        frame(visual);
        (owner.unwrap(), visual)
    }

    fn request_setup(view: &Entity<Newport>, cx: &mut VisualTestContext, manual: bool) {
        view.update(cx, |view, cx| {
            request("fixture", manual, cx);
            view.poll_terminal_setup(cx);
            cx.notify();
        });
        frame(cx);
    }

    #[gpui::test]
    fn opening_a_terminal_without_a_license_does_not_request_setup(cx: &mut TestAppContext) {
        let (view, cx) = fixture(cx, json!({"status":"signedOut"}));
        view.update(cx, |v, cx| {
            v.page = Page::Terminal;
            assert!(remote_apps::terminal_display("fixture", cx).is_none());
            assert!(!v.poll_terminal_setup(cx));
            cx.notify();
        });
        frame(cx);
        assert!(!cx.update(|window, cx| window.has_active_dialog(cx)));
        request_setup(&view, cx, false);
        assert!(cx.update(|window, cx| window.has_active_dialog(cx)));
    }

    #[gpui::test]
    fn terminal_setup_dismissal_is_respected_and_manual_setup_stays_available(
        cx: &mut TestAppContext,
    ) {
        let (view, cx) = fixture(cx, json!({"status":"signedOut"}));
        request_setup(&view, cx, false);
        assert!(labels(cx).contains(&"Sign in".into()));
        assert!(view.read_with(cx, |v, _| v.terminal_setup.open.is_some()));
        cx.simulate_keystrokes("escape");
        frame(cx);
        assert!(!cx.update(|window, cx| window.has_active_dialog(cx)));
        request_setup(&view, cx, false);
        assert!(!cx.update(|window, cx| window.has_active_dialog(cx)));
        request_setup(&view, cx, true);
        click(cx, "Continue with terminal");
        assert!(!cx.update(|window, cx| window.has_active_dialog(cx)));
        assert!(view.read_with(cx, |v, _| v.terminal_setup.dismissed.is_some()));
        view.update(cx, |v, _| {
            v.terminal_setup.dismissed =
                Some(std::time::Instant::now() - std::time::Duration::from_secs(4));
        });
        request_setup(&view, cx, false);
        assert!(cx.update(|window, cx| window.has_active_dialog(cx)));
    }

    #[gpui::test]
    fn terminal_setup_waits_for_account_and_does_not_upsell_pro(cx: &mut TestAppContext) {
        let (view, cx) = fixture(cx, json!({"status":"loading"}));
        request_setup(&view, cx, false);
        assert!(!cx.update(|window, cx| window.has_active_dialog(cx)));
        view.update(cx, |v, cx| {
            v.account =
                account(json!({"status":"signedIn","license":{"state":"active","seat":"pro"}}));
            cx.notify();
        });
        frame(cx);
        assert!(!cx.update(|window, cx| window.has_active_dialog(cx)));
        assert!(!labels(cx).contains(&"Start free trial".into()));
        assert!(view.read_with(cx, |v, _| v.page == Page::RemoteApps));
        assert!(!cx.update(|window, cx| window.has_active_dialog(cx)));
    }

    #[gpui::test]
    fn terminal_setup_missing_license_offers_recovery(cx: &mut TestAppContext) {
        let (view, cx) = fixture(cx, json!({"status":"signedIn","license":null}));
        request_setup(&view, cx, false);
        assert!(labels(cx).contains(&"Refresh account".into()));
        assert!(!labels(cx).contains(&"Start free trial".into()));
        click(cx, "Continue with terminal");
        assert!(!cx.update(|window, cx| window.has_active_dialog(cx)));
    }

    #[gpui::test]
    fn terminal_setup_updates_in_place_and_displays_action_errors(cx: &mut TestAppContext) {
        let (view, cx) = fixture(cx, json!({"status":"signedOut"}));
        let status = Rc::new(RefCell::new(json!({"status":"signedOut"})));
        let calls = Rc::new(RefCell::new(Vec::new()));
        view.update(cx, |v, _| {
            let (status, calls) = (status.clone(), calls.clone());
            v.account = Account::with_replies(move |method| {
                calls.borrow_mut().push(method);
                if method == "account_start_trial" {
                    return Err("Network unavailable".into());
                }
                if method == "account_sign_in" {
                    *status.borrow_mut() = json!({"status":"waiting"});
                }
                Ok(status.borrow().clone())
            });
            v.account.poll();
            v.account.poll();
        });
        request_setup(&view, cx, false);
        click(cx, "Sign in");
        view.update(cx, |v, cx| {
            let effects = v.account.poll();
            v.apply_account_effects(effects, cx);
            cx.notify();
        });
        frame(cx);
        assert!(calls.borrow().contains(&"account_sign_in"));
        assert!(labels(cx).contains(&"Open sign-in page".into()));
        *status.borrow_mut() = json!({"status":"signedIn","license":{"state":"none"}});
        view.update(cx, |v, cx| {
            let (status, calls) = (status.clone(), calls.clone());
            v.account = Account::with_replies(move |method| {
                calls.borrow_mut().push(method);
                if method == "account_start_trial" {
                    return Err("Network unavailable".into());
                }
                Ok(status.borrow().clone())
            });
            v.account.poll();
            let effects = v.account.poll();
            v.apply_account_effects(effects, cx);
            cx.notify();
        });
        frame(cx);
        assert!(labels(cx).contains(&"Start free trial".into()));
        click(cx, "Start free trial");
        view.update(cx, |v, cx| {
            let effects = v.account.poll();
            v.apply_account_effects(effects, cx);
            cx.notify();
        });
        frame(cx);
        assert!(view.read_with(cx, |v, _| {
            v.terminal_setup
                .error
                .as_ref()
                .is_some_and(|e| e.contains("Couldn’t start the free trial"))
        }));
        assert!(
            labels(cx)
                .iter()
                .any(|label| label.contains("Couldn’t start the free trial"))
        );
        assert!(cx.update(|window, cx| window.has_active_dialog(cx)));
    }

    #[test]
    fn terminal_setup_actions_match_license_and_seat_requirements() {
        for (license, action) in [
            (json!({"state":"none"}), Action::StartTrial),
            (
                json!({"state":"none","trialUsedHere":true}),
                Action::OpenBilling,
            ),
            (
                json!({"state":"expired","trialUsed":true}),
                Action::OpenBilling,
            ),
            (json!({"state":"active","seat":"free"}), Action::OpenPortal),
            (json!({"state":"trial","seat":"free"}), Action::OpenBilling),
        ] {
            let status =
                serde_json::from_value(json!({"status":"signedIn","license":license})).unwrap();
            assert_eq!(
                step(Entitlement::of(&status), None).action.unwrap().1,
                action
            );
        }
    }

    fn ready_environment() -> newport_domain::terminal::GuiEnvironment {
        newport_domain::terminal::GuiEnvironment(
            [
                ("DISPLAY", Some(":75")),
                ("XAUTHORITY", Some("/tmp/newport-test/Xauthority")),
                ("WAYLAND_DISPLAY", None),
                ("WAYLAND_SOCKET", None),
            ]
            .into_iter()
            .map(|(k, v)| (k.to_owned(), v.map(str::to_owned)))
            .collect(),
        )
    }

    fn waiting_launch(
        view: &Entity<Newport>,
        cx: &mut VisualTestContext,
        server: &str,
    ) -> (
        std::sync::mpsc::Receiver<Result<newport_domain::terminal::GuiEnvironment, String>>,
        std::sync::Arc<std::sync::atomic::AtomicBool>,
    ) {
        let (send, receive) = std::sync::mpsc::channel();
        let cancelled = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let flag = cancelled.clone();
        view.update(cx, |_, cx| {
            super::requests(cx).queue.push_back(super::Request {
                server: server.into(),
                application: "chromium".into(),
                manual: false,
                origin: None,
                reply: Some(Box::new(move |result| {
                    let _ = send.send(result);
                })),
                cancelled: flag,
                routed: false,
                return_page: Page::Terminal,
            });
            cx.notify();
        });
        frame(cx);
        (receive, cancelled)
    }

    #[gpui::test]
    fn licensed_ready_launch_has_no_modal_and_replies_once(cx: &mut TestAppContext) {
        let (view, cx) = fixture(
            cx,
            json!({"status":"signedIn","license":{"state":"active","seat":"pro"}}),
        );
        cx.update(|_, cx| {
            cx.set_global(crate::terminal_gui::TestProbe(Ok(
                Some(ready_environment()),
            )))
        });
        let (reply, _) = waiting_launch(&view, cx, "fixture");
        assert_eq!(reply.try_recv().unwrap().unwrap(), ready_environment());
        assert!(!cx.update(|window, cx| window.has_active_dialog(cx)));
        frame(cx);
        assert!(reply.try_recv().is_err());
    }

    #[gpui::test]
    fn login_success_closes_the_modal_and_continues_the_waiting_launch(cx: &mut TestAppContext) {
        let (view, cx) = fixture(cx, json!({"status":"signedOut"}));
        cx.update(|_, cx| {
            cx.set_global(crate::terminal_gui::TestProbe(Ok(
                Some(ready_environment()),
            )))
        });
        let (reply, _) = waiting_launch(&view, cx, "fixture");
        assert!(labels(cx).contains(&"Sign in".into()));
        assert!(reply.try_recv().is_err());
        view.update(cx, |v, cx| {
            v.account =
                account(json!({"status":"signedIn","license":{"state":"trial","seat":"pro"}}));
            cx.notify();
        });
        frame(cx);
        assert_eq!(reply.try_recv().unwrap().unwrap(), ready_environment());
        assert!(!cx.update(|window, cx| window.has_active_dialog(cx)));
        assert!(view.read_with(cx, |v, _| v.page != Page::RemoteApps));
    }

    #[gpui::test]
    fn missing_software_routes_to_the_origin_server_then_continues_after_setup(
        cx: &mut TestAppContext,
    ) {
        let (view, cx) = fixture(
            cx,
            json!({"status":"signedIn","license":{"state":"active","seat":"pro"}}),
        );
        view.update(cx, |v, _| {
            let mut other = v.snapshot["config"]["servers"][0].clone();
            other["id"] = json!("other");
            other["name"] = json!("Other");
            v.snapshot["config"]["servers"]
                .as_array_mut()
                .unwrap()
                .push(other);
            v.server = "other".into();
        });
        let (reply, _) = waiting_launch(&view, cx, "fixture");
        assert!(reply.try_recv().is_err());
        assert!(view.read_with(cx, |v, _| v.server == "fixture"
            && v.page == Page::RemoteApps));
        assert!(!cx.update(|window, cx| window.has_active_dialog(cx)));
        cx.update(|_, cx| {
            cx.set_global(crate::terminal_gui::TestProbe(Ok(
                Some(ready_environment()),
            )))
        });
        view.update(cx, |v, cx| {
            let key = remote_apps::status_key("fixture");
            v.resources.fence(&key);
            let generation = v.resources.begin_refresh(&key, true).unwrap();
            v.resources.complete(
                &key,
                generation,
                Ok(json!({"agent":"/installed/run-agent","missing":[]})),
            );
            cx.notify();
        });
        frame(cx);
        assert_eq!(reply.try_recv().unwrap().unwrap(), ready_environment());
        assert!(view.read_with(cx, |v, _| v.terminal_setup.current.is_none()));
    }

    #[gpui::test]
    fn connection_failure_offers_retry_without_installation_or_duplicate_execution(
        cx: &mut TestAppContext,
    ) {
        let (view, cx) = fixture(
            cx,
            json!({"status":"signedIn","license":{"state":"active","seat":"pro"}}),
        );
        cx.update(|_, cx| {
            cx.set_global(crate::terminal_gui::TestProbe(Err(
                "SSH connection lost".into()
            )))
        });
        let (reply, _) = waiting_launch(&view, cx, "fixture");
        assert!(labels(cx).contains(&"Retry".into()));
        assert!(reply.try_recv().is_err());
        assert!(view.read_with(cx, |v, _| v.page != Page::RemoteApps));
        cx.update(|_, cx| {
            cx.set_global(crate::terminal_gui::TestProbe(Ok(
                Some(ready_environment()),
            )))
        });
        click(cx, "Retry");
        assert_eq!(reply.try_recv().unwrap().unwrap(), ready_environment());
        assert!(!cx.update(|window, cx| window.has_active_dialog(cx)));
    }

    #[gpui::test]
    fn cancelled_launch_never_runs_after_login_and_queued_launches_are_preserved(
        cx: &mut TestAppContext,
    ) {
        let (view, cx) = fixture(cx, json!({"status":"signedOut"}));
        let (first, cancelled) = waiting_launch(&view, cx, "fixture");
        let (second, _) = waiting_launch(&view, cx, "fixture");
        cancelled.store(true, std::sync::atomic::Ordering::Release);
        frame(cx);
        assert!(first.try_recv().unwrap().is_err());
        cx.update(|_, cx| {
            cx.set_global(crate::terminal_gui::TestProbe(Ok(
                Some(ready_environment()),
            )))
        });
        view.update(cx, |v, cx| {
            v.account =
                account(json!({"status":"signedIn","license":{"state":"active","seat":"pro"}}));
            cx.notify();
        });
        frame(cx);
        assert_eq!(second.try_recv().unwrap().unwrap(), ready_environment());
        assert!(first.try_recv().is_err());
    }

    #[gpui::test]
    fn unknown_terminal_is_rejected_before_any_account_prompt(cx: &mut TestAppContext) {
        let (_, cx) = fixture(cx, json!({"status":"signedOut"}));
        let (send, reply) = std::sync::mpsc::channel();
        cx.update(|_, cx| {
            super::request_launch(
                "fixture".into(),
                uuid::Uuid::new_v4().to_string(),
                "chromium".into(),
                Box::new(move |result| {
                    send.send(result).unwrap();
                }),
                std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
                cx,
            )
        });
        frame(cx);
        assert!(reply.try_recv().unwrap().is_err());
        assert!(!cx.update(|window, cx| window.has_active_dialog(cx)));
    }
}
