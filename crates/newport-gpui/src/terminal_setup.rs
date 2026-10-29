//! Account and setup guidance for a GUI app launched in a terminal. The
//! terminal never waits for it: its shell stays connected and usable whether
//! the guidance is followed, dismissed or fails.
use super::*;
use crate::terminal_view::TerminalView;
use newport_client::account::{Action, NoticeKind};
use remote_apps::Entitlement;
use std::{
    collections::{HashMap, VecDeque},
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

type Reply = Box<dyn FnOnce(Result<(), String>) + Send>;

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
pub(crate) fn request(server: &str, cx: &mut App) {
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
        application: "GUI apps".into(),
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
    error: Option<String>,
    current: Option<Request>,
    readiness: Option<Rc<crate::terminal_gui::Readiness>>,
    stage: Stage,
    return_to: Option<(Origin, Page)>,
    syncing: bool,
    /// The terminal showing the card, and the card itself.
    anchor: Option<WeakEntity<TerminalView>>,
    card: Option<Entity<Card>>,
}
/// The guidance card in the terminal whose command is waiting. It never takes
/// focus or covers the shell; only its buttons act on the launch.
pub(crate) struct Card {
    owner: WeakEntity<Newport>,
    _observer: Subscription,
}
impl Render for Card {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let owner = self.owner.clone();
        match self.owner.upgrade() {
            Some(newport) => newport.read(cx).terminal_setup_card(owner, cx),
            None => div().into_any_element(),
        }
    }
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
/// Needle can open apps: it is installed, and nothing every app needs is
/// missing. Otherwise Set up leads to Remote apps, which says what to install.
pub(crate) fn setup_ready(status: &Value) -> bool {
    remote_apps::launch_agent(status).is_ok()
        && serde_json::from_value::<Vec<crate::remote_apps_api::Gap>>(status["gaps"].clone())
            .unwrap_or_default()
            .iter()
            .all(crate::remote_apps_api::Gap::optional)
}
impl Newport {
    fn begin_terminal_readiness(&mut self, cx: &mut Context<Self>) {
        let Some(request) = &self.terminal_setup.current else {
            return;
        };
        remote_apps::follow_account(self.account.status(), cx);
        self.terminal_setup.readiness = Some(crate::terminal_gui::Readiness::start(
            request.server.clone(),
            cx,
        ));
        self.terminal_setup.error = None;
        self.terminal_setup.stage = Stage::Starting;
    }
    pub(super) fn poll_terminal_setup(&mut self, cx: &mut Context<Self>) -> bool {
        let mut changed = false;
        if self.terminal_setup.current.is_none() {
            while let Some(mut request) = if cx.has_global::<Requests>() {
                cx.global_mut::<Requests>().queue.pop_front()
            } else {
                None
            } {
                let quiet = request.reply.is_none()
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
                self.terminal_setup.readiness = None;
                self.terminal_setup.stage = Stage::Account;
                changed = true;
            }
            return changed;
        }
        match self.terminal_setup.stage {
            Stage::Account => {
                self.begin_terminal_readiness(cx);
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
                    self.begin_terminal_readiness(cx);
                    changed = true;
                }
            }
            _ => {}
        }
        if self.terminal_setup.stage == Stage::Starting
            && let Some(result) = self
                .terminal_setup
                .readiness
                .as_ref()
                .and_then(|check| check.poll())
        {
            changed = true;
            match result {
                Ok(true) => {
                    let mut request = self.terminal_setup.current.take().unwrap();
                    self.terminal_setup.readiness = None;
                    if let Some(origin) = &request.origin {
                        if !origin.live(cx) {
                            return true;
                        }
                        if request.routed {
                            self.terminal_setup.return_to =
                                Some((origin.clone(), request.return_page));
                        }
                    }
                    if let Some(reply) = request.reply.take() {
                        reply(Ok(()));
                    } else {
                        self.notification = Some(Notification::success(
                            "Run the app by its command name in the terminal, or launch it from Remote apps.",
                        ));
                    }
                }
                Ok(false) => {
                    self.terminal_setup.readiness = None;
                    self.terminal_setup.stage = Stage::Setup;
                }
                Err(error) => {
                    self.terminal_setup.readiness = None;
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
                && self.terminal_setup.anchor.is_none()
                && self.terminal_setup.return_to.is_none())
        {
            return;
        }
        self.terminal_setup.syncing = true;
        cx.defer_in(window, |this, window, cx| {
            this.terminal_setup.syncing = false;
            this.place_terminal_setup_card(cx);
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
        });
    }
    /// The terminal that shows the card: the one whose command is waiting, or
    /// for a notice without one, a terminal on its server. None while the
    /// account loads or the GUI session starts.
    fn terminal_setup_anchor(&self, cx: &App) -> Option<Entity<TerminalView>> {
        let request = self.terminal_setup.current.as_ref()?;
        let status = self.account.status();
        let loading = Entitlement::of(status) == Entitlement::Unknown
            && status.phase != newport_client::account::Phase::SignedIn;
        if loading || self.terminal_setup.stage == Stage::Starting {
            return None;
        }
        if let Some(origin) = &request.origin {
            return origin.live(cx).then(|| origin.view.upgrade()).flatten();
        }
        let terminals: Vec<_> = cx
            .try_global::<Requests>()?
            .terminals
            .values()
            .filter(|origin| origin.server == request.server && origin.live(cx))
            .filter_map(|origin| origin.view.upgrade())
            .collect();
        self.terminals
            .get(&request.server)
            .and_then(|workspace| workspace.read(cx).active_view())
            .filter(|active| {
                terminals
                    .iter()
                    .any(|t| t.entity_id() == active.entity_id())
            })
            .or_else(|| terminals.iter().find(|t| t.read(cx).visible()).cloned())
            .or_else(|| terminals.first().cloned())
    }
    fn place_terminal_setup_card(&mut self, cx: &mut Context<Self>) {
        let anchor = self.terminal_setup_anchor(cx);
        let placed = self
            .terminal_setup
            .anchor
            .take()
            .and_then(|view| view.upgrade());
        self.terminal_setup.anchor = anchor.as_ref().map(Entity::downgrade);
        if anchor.as_ref().map(Entity::entity_id) == placed.as_ref().map(Entity::entity_id) {
            return;
        }
        if let Some(placed) = placed {
            placed.update(cx, |view, cx| view.set_card(None, cx));
        }
        let Some(anchor) = anchor else {
            return;
        };
        let card = match &self.terminal_setup.card {
            Some(card) => card.clone(),
            None => {
                let owner = cx.entity();
                let card = cx.new(|cx| Card {
                    owner: owner.downgrade(),
                    _observer: cx.observe(&owner, |_, _, cx| cx.notify()),
                });
                self.terminal_setup.card = Some(card.clone());
                card
            }
        };
        anchor.update(cx, |view, cx| view.set_card(Some(card.into()), cx));
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
        self.terminal_setup.readiness = None;
        self.terminal_setup.error = None;
        self.terminal_setup.dismissed = Some(Instant::now());
    }
    /// Opens Remote Apps on the launch's server to finish its setup. The
    /// launch continues, and returns to its terminal, once setup is ready.
    fn set_up_terminal_gui(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(request) = self.terminal_setup.current.as_mut() else {
            return;
        };
        request.routed = true;
        let server = request.server.clone();
        self.terminal_launch_server(server);
        self.navigate(Page::RemoteApps, cx);
        self.focus.focus(window, cx);
        cx.notify();
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
    fn terminal_setup_card(&self, owner: WeakEntity<Self>, cx: &App) -> AnyElement {
        let Some(request) = &self.terminal_setup.current else {
            return div().into_any_element();
        };
        let status = self.account.status();
        let entitlement = Entitlement::of(status);
        let license_missing = entitlement == Entitlement::Unknown
            && status.phase == newport_client::account::Phase::SignedIn;
        let busy = self.account.pending().is_some() || status.signing_out;
        let mut step = step(
            entitlement,
            status.license.as_ref().and_then(|l| l.message.as_deref()),
        );
        let computer = if cfg!(target_os = "macos") {
            "your Mac"
        } else {
            "this computer"
        };
        enum Primary {
            Account(&'static str, Action),
            Refresh,
            Retry,
            SetUp,
        }
        let primary = match self.terminal_setup.stage {
            Stage::Setup => {
                let server = rows(&self.snapshot["config"]["servers"])
                    .iter()
                    .find(|s| text(s, "id") == request.server)
                    .map(|s| text(s, "name"))
                    .filter(|name| !name.is_empty())
                    .unwrap_or_else(|| request.server.clone());
                step.detail = format!(
                    "Set up {server} to open {} on {computer}.",
                    request.application
                );
                Some(Primary::SetUp)
            }
            Stage::Failed => {
                step.detail =
                    "Couldn’t start the GUI session. Check the server connection and try again."
                        .into();
                Some(Primary::Retry)
            }
            _ if license_missing => {
                step.detail = "Newport couldn’t load your license. Refresh your account to check access to GUI apps.".into();
                Some(Primary::Refresh)
            }
            _ => step
                .action
                .map(|(label, action)| Primary::Account(label, action)),
        };
        // Only a waiting command has a launch to cancel.
        let cancel = if request.reply.is_some() {
            "Cancel launch"
        } else {
            "Dismiss"
        };
        let title = format!("Open {} on {computer}", request.application);
        let act = |f: fn(&mut Self, &mut Window, &mut Context<Self>)| {
            let owner = owner.clone();
            move |_: &ClickEvent, window: &mut Window, cx: &mut App| {
                let _ = owner.update(cx, |this, cx| f(this, window, cx));
            }
        };
        let dismiss = |this: &mut Self, _: &mut Window, cx: &mut Context<Self>| {
            this.dismiss_terminal_setup();
            cx.notify();
        };
        let theme = cx.theme();
        div()
            .id("terminal-gui-card")
            .role(accesskit::Role::Group)
            .aria_label(title.clone())
            .test_support()
            .flex()
            .flex_col()
            .gap(px(10.))
            .p(px(12.))
            .rounded(px(crate::ui::RADIUS))
            .border_1()
            .border_color(theme.border)
            .bg(theme.popover)
            .text_color(theme.popover_foreground)
            .text_size(px(crate::ui::FONT_SIZE))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(
                        Icon::new(IconName::LayoutGrid)
                            .size(px(crate::controls::ICON_SIZE))
                            .text_color(theme.muted_foreground)
                            .flex_shrink_0(),
                    )
                    .child(
                        div()
                            .id("terminal-gui-title")
                            .role(accesskit::Role::Heading)
                            .aria_level(2)
                            .aria_label(title.clone())
                            .flex_1()
                            .min_w_0()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(title),
                    )
                    .child(
                        crate::controls::compact_icon_button("terminal-gui-close")
                            .ghost()
                            .tab_stop(false)
                            .icon(
                                Icon::new(IconName::Close)
                                    .size(px(crate::controls::ICON_SIZE))
                                    .text_color(theme.muted_foreground),
                            )
                            .accessibility_label(cancel)
                            .tooltip(cancel)
                            .on_click(act(dismiss)),
                    ),
            )
            .child(
                crate::project_dialog::description("terminal-gui-setup-detail", step.detail)
                    .text_color(theme.muted_foreground),
            )
            .when_some(self.terminal_setup.error.clone(), |card, error| {
                card.child(
                    crate::controls::alert("terminal-gui-setup-error", error, cx).test_support(),
                )
            })
            .child(
                crate::project_dialog::actions()
                    .child(
                        crate::controls::button("terminal-gui-cancel")
                            .tab_stop(false)
                            .label(cancel)
                            .on_click(act(dismiss)),
                    )
                    .when_some(primary, |actions, primary| {
                        let button = crate::controls::button("terminal-gui-primary")
                            .primary()
                            .tab_stop(false);
                        actions.child(match primary {
                            Primary::Account(label, action) => {
                                let owner = owner.clone();
                                button.label(label).loading(busy).disabled(busy).on_click(
                                    move |_, _, cx| {
                                        let _ = owner.update(cx, |this, cx| {
                                            this.terminal_setup.error = None;
                                            this.run_account(action, cx);
                                        });
                                    },
                                )
                            }
                            Primary::Refresh => button
                                .label("Refresh account")
                                .disabled(busy)
                                .on_click(act(|this, _, cx| {
                                    this.terminal_setup.error = None;
                                    this.account.refresh();
                                    cx.notify();
                                })),
                            Primary::Retry => button.label("Retry").on_click(act(|this, _, cx| {
                                this.terminal_setup.stage = Stage::Account;
                                this.terminal_setup.error = None;
                                cx.notify();
                            })),
                            Primary::SetUp => {
                                button.label("Set up").on_click(act(|this, window, cx| {
                                    this.set_up_terminal_gui(window, cx)
                                }))
                            }
                        })
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
        terminal_view::{
            TerminalView,
            input_tests::{Peer, attached, wait},
        },
    };
    use gpui_kit::component::WindowExt;
    use gpui_kit::{
        self as gpui, AppContext, Context, Entity, IntoElement, Modifiers, ParentElement, Render,
        Styled, TestAppContext, VisualTestContext, Window, div, prelude::*, px, size,
    };
    use newport_client::account::{Account, Action};
    use serde_json::{Value, json};
    use std::{
        cell::RefCell,
        rc::Rc,
        sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
            mpsc::Receiver,
        },
    };

    /// Newport above two terminals on the same server: the one whose command
    /// waits, and another tab.
    struct Host {
        newport: Entity<Newport>,
        origin: Entity<TerminalView>,
        other: Entity<TerminalView>,
    }
    impl Render for Host {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
                .size_full()
                .flex()
                .flex_col()
                .child(
                    div()
                        .flex_1()
                        .min_h_0()
                        .overflow_hidden()
                        .child(self.newport.clone()),
                )
                .child(
                    div()
                        .id("origin-pane")
                        .h(px(220.))
                        .flex_shrink_0()
                        .child(self.origin.clone()),
                )
                .child(
                    div()
                        .id("other-pane")
                        .h(px(160.))
                        .flex_shrink_0()
                        .child(self.other.clone()),
                )
        }
    }

    struct Fixture {
        view: Entity<Newport>,
        terminal: Entity<TerminalView>,
        other: Entity<TerminalView>,
        session: uuid::Uuid,
        peer: Peer,
        _other_peer: Peer,
    }

    fn frame(cx: &mut VisualTestContext) {
        // Placing the card follows the frame that found the request.
        crate::page_perf_tests::frame(cx);
        crate::page_perf_tests::frame(cx);
    }

    fn labels(cx: &mut VisualTestContext) -> Vec<String> {
        cx.update(|window, _| gpui::base::test_support::snapshots(window))
            .iter()
            .filter_map(|node| node.label().map(str::to_owned))
            .collect()
    }

    /// The pane that shows the card, if any.
    fn card(cx: &mut VisualTestContext) -> Option<&'static str> {
        let panes: Vec<_> = cx
            .update(|window, _| gpui::base::test_support::snapshots(window))
            .into_iter()
            .filter(|node| {
                node.path()
                    .last()
                    .is_some_and(|id| id.to_string() == "terminal-gui-card")
            })
            .map(|node| {
                let path: Vec<_> = node.path().iter().map(|id| id.to_string()).collect();
                assert!(path.iter().any(|id| id == "ssh-terminal"));
                if path.iter().any(|id| id == "origin-pane") {
                    "origin"
                } else {
                    "other"
                }
            })
            .collect();
        assert!(panes.len() <= 1, "one card at a time: {panes:?}");
        panes.first().copied()
    }

    fn click_node(cx: &mut VisualTestContext, matches: impl Fn(&str, Option<&str>) -> bool) {
        let bounds = cx
            .update(|window, _| gpui::base::test_support::snapshots(window))
            .into_iter()
            .find(|node| {
                node.path().last().is_some_and(|id| {
                    let id = id.to_string();
                    id.starts_with("terminal-gui-") && matches(&id, node.label())
                })
            })
            .expect("missing card control")
            .bounds();
        cx.simulate_click(bounds.center(), Modifiers::none());
        frame(cx);
    }

    fn click(cx: &mut VisualTestContext, label: &str) {
        click_node(cx, |id, l| l == Some(label) && id != "terminal-gui-close");
    }

    fn close(cx: &mut VisualTestContext) {
        click_node(cx, |id, _| id == "terminal-gui-close");
    }

    fn focused(f: &Fixture, cx: &mut VisualTestContext) -> bool {
        cx.update(|window, cx| f.terminal.read(cx).focus.is_focused(window))
    }

    fn no_dialog(cx: &mut VisualTestContext) -> bool {
        !cx.update(|window, cx| window.has_active_dialog(cx))
    }

    fn assert_host_label(cx: &mut VisualTestContext, mac: &str, other: &str) {
        let expected = if cfg!(target_os = "macos") {
            mac
        } else {
            other
        };
        let unexpected = if cfg!(target_os = "macos") {
            other
        } else {
            mac
        };
        let labels = labels(cx);
        assert!(labels.iter().any(|label| label == expected));
        assert!(!labels.iter().any(|label| label == unexpected));
    }

    fn account(status: Value) -> Account {
        let mut account = Account::with_replies(move |_| Ok(status.clone()));
        account.poll();
        account.poll();
        account
    }

    fn fixture(cx: &mut TestAppContext, status: Value) -> (Fixture, &mut VisualTestContext) {
        cx.update(|cx| {
            gpui::component::init(cx);
            crate::theme::install(false, cx);
            remote_apps::start_scripted(cx);
        });
        let (peer, other_peer) = (Peer::new(), Peer::new());
        let mut parts = None;
        let (_, visual) = cx.add_window_view(|window, cx| {
            let view = cx.new(|cx| {
                let mut view = crate::page_perf_tests::owner(window, cx);
                Newport::observe_modal_visibility(window, cx);
                view.account = account(status);
                view
            });
            let origin = cx.new(|cx| attached(&peer, "fixture", cx));
            let other = cx.new(|cx| attached(&other_peer, "fixture", cx));
            parts = Some((view.clone(), origin.clone(), other.clone()));
            let host = cx.new(|_| Host {
                newport: view,
                origin,
                other,
            });
            gpui::base::Root::new(host, window, cx)
        });
        let (view, terminal, other) = parts.unwrap();
        visual.simulate_resize(size(px(900.), px(900.)));
        crate::page_perf_tests::frame(visual);
        for terminal in [&terminal, &other] {
            wait(terminal, visual, |view| view.ready());
            visual.update(|_, cx| {
                let id = terminal.read(cx).session_id().unwrap();
                super::register(id, "fixture", terminal.downgrade(), cx);
            });
        }
        let session = terminal.read_with(visual, |view, _| view.session_id().unwrap());
        visual.update(|window, cx| terminal.read(cx).focus.clone().focus(window, cx));
        frame(visual);
        (
            Fixture {
                view,
                terminal,
                other,
                session,
                peer,
                _other_peer: other_peer,
            },
            visual,
        )
    }

    fn request_setup(view: &Entity<Newport>, cx: &mut VisualTestContext) {
        view.update(cx, |view, cx| {
            request("fixture", cx);
            view.poll_terminal_setup(cx);
            cx.notify();
        });
        frame(cx);
    }

    fn waiting_launch(
        f: &Fixture,
        cx: &mut VisualTestContext,
    ) -> (Receiver<Result<(), String>>, Arc<AtomicBool>) {
        let (send, receive) = std::sync::mpsc::channel();
        let cancelled = Arc::new(AtomicBool::new(false));
        let flag = cancelled.clone();
        let terminal = f.session.to_string();
        f.view.update(cx, |_, cx| {
            super::request_launch(
                "fixture".into(),
                terminal,
                "chromium".into(),
                Box::new(move |result| {
                    let _ = send.send(result);
                }),
                flag,
                cx,
            );
            cx.notify();
        });
        frame(cx);
        (receive, cancelled)
    }

    fn sign_in(f: &Fixture, cx: &mut VisualTestContext, status: Value) {
        f.view.update(cx, |v, cx| {
            v.account = account(status);
            cx.notify();
        });
        frame(cx);
    }

    #[gpui::test]
    fn opening_a_terminal_without_a_license_does_not_request_setup(cx: &mut TestAppContext) {
        let (f, cx) = fixture(cx, json!({"status":"signedOut"}));
        f.view.update(cx, |v, cx| {
            v.page = Page::Terminal;
            assert!(!v.poll_terminal_setup(cx));
            cx.notify();
        });
        frame(cx);
        assert_eq!(card(cx), None);
        request_setup(&f.view, cx);
        assert!(card(cx).is_some());
        assert_host_label(
            cx,
            "Open GUI apps on your Mac",
            "Open GUI apps on this computer",
        );
        assert!(no_dialog(cx));
    }

    #[gpui::test]
    fn terminal_setup_dismissal_is_respected(cx: &mut TestAppContext) {
        let (f, cx) = fixture(cx, json!({"status":"signedOut"}));
        request_setup(&f.view, cx);
        assert!(labels(cx).contains(&"Sign in".into()));
        // Esc belongs to the shell.
        cx.simulate_keystrokes("escape");
        frame(cx);
        assert!(card(cx).is_some());
        close(cx);
        assert_eq!(card(cx), None);
        request_setup(&f.view, cx);
        assert_eq!(card(cx), None);
        f.view.update(cx, |v, _| {
            v.terminal_setup.dismissed =
                Some(std::time::Instant::now() - std::time::Duration::from_secs(4));
        });
        request_setup(&f.view, cx);
        click(cx, "Dismiss");
        assert_eq!(card(cx), None);
        assert!(
            f.view
                .read_with(cx, |v, _| v.terminal_setup.dismissed.is_some())
        );
        f.view.update(cx, |v, _| {
            v.terminal_setup.dismissed =
                Some(std::time::Instant::now() - std::time::Duration::from_secs(4));
        });
        request_setup(&f.view, cx);
        assert!(card(cx).is_some());
    }

    #[gpui::test]
    fn terminal_setup_waits_for_account_and_does_not_upsell_pro(cx: &mut TestAppContext) {
        let (f, cx) = fixture(cx, json!({"status":"loading"}));
        cx.update(|_, cx| cx.set_global(crate::terminal_gui::TestProbe(Ok(false))));
        request_setup(&f.view, cx);
        assert_eq!(card(cx), None);
        sign_in(
            &f,
            cx,
            json!({"status":"signedIn","license":{"state":"active","seat":"pro"}}),
        );
        assert!(!labels(cx).contains(&"Start free trial".into()));
        assert!(labels(cx).contains(&"Set up".into()));
        assert!(f.view.read_with(cx, |v, _| v.page != Page::RemoteApps));
        assert!(no_dialog(cx));
    }

    #[gpui::test]
    fn terminal_setup_missing_license_offers_recovery(cx: &mut TestAppContext) {
        let (f, cx) = fixture(cx, json!({"status":"signedIn","license":null}));
        request_setup(&f.view, cx);
        assert!(labels(cx).contains(&"Refresh account".into()));
        assert!(!labels(cx).contains(&"Start free trial".into()));
        click(cx, "Dismiss");
        assert_eq!(card(cx), None);
    }

    #[gpui::test]
    fn terminal_setup_updates_in_place_and_displays_action_errors(cx: &mut TestAppContext) {
        let (f, cx) = fixture(cx, json!({"status":"signedOut"}));
        let status = Rc::new(RefCell::new(json!({"status":"signedOut"})));
        let calls = Rc::new(RefCell::new(Vec::new()));
        f.view.update(cx, |v, _| {
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
        request_setup(&f.view, cx);
        click(cx, "Sign in");
        f.view.update(cx, |v, cx| {
            let effects = v.account.poll();
            v.apply_account_effects(effects, cx);
            cx.notify();
        });
        frame(cx);
        assert!(calls.borrow().contains(&"account_sign_in"));
        assert!(labels(cx).contains(&"Open sign-in page".into()));
        *status.borrow_mut() = json!({"status":"signedIn","license":{"state":"none"}});
        f.view.update(cx, |v, cx| {
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
        f.view.update(cx, |v, cx| {
            let effects = v.account.poll();
            v.apply_account_effects(effects, cx);
            cx.notify();
        });
        frame(cx);
        assert!(f.view.read_with(cx, |v, _| {
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
        assert!(card(cx).is_some());
        assert!(no_dialog(cx));
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

    #[gpui::test]
    fn the_card_shows_in_its_terminal_and_never_takes_the_keyboard(cx: &mut TestAppContext) {
        let (f, cx) = fixture(cx, json!({"status":"signedOut"}));
        let (reply, _) = waiting_launch(&f, cx);
        assert_eq!(card(cx), Some("origin"));
        assert_host_label(
            cx,
            "Open chromium on your Mac",
            "Open chromium on this computer",
        );
        assert!(labels(cx).contains(&"Cancel launch".into()));
        assert!(!labels(cx).contains(&"Continue with terminal".into()));
        assert!(no_dialog(cx));
        assert!(focused(&f, cx));
        // Typing, Esc included, reaches the shell and leaves the launch waiting.
        cx.simulate_keystrokes("l s escape");
        wait(&f.terminal, cx, |_| f.peer.written().len() >= 3);
        assert_eq!(f.peer.written(), b"ls\x1b");
        frame(cx);
        assert_eq!(card(cx), Some("origin"));
        assert!(reply.try_recv().is_err());
        // Pressing a card button keeps the terminal's focus.
        click(cx, "Sign in");
        assert!(focused(&f, cx));
        cx.simulate_keystrokes("x");
        wait(&f.terminal, cx, |_| f.peer.written().len() >= 4);
        assert_eq!(f.peer.written(), b"ls\x1bx");
        // × cancels the launch; the shell stays connected and focused.
        close(cx);
        assert_eq!(card(cx), None);
        assert!(reply.try_recv().unwrap().is_err());
        assert!(focused(&f, cx));
        assert!(f.terminal.read_with(cx, |t, _| t.ready()));
    }

    #[gpui::test]
    fn the_card_leaves_with_its_terminal_session(cx: &mut TestAppContext) {
        let (f, cx) = fixture(cx, json!({"status":"signedOut"}));
        let (reply, _) = waiting_launch(&f, cx);
        assert_eq!(card(cx), Some("origin"));
        f.terminal.update(cx, |t, cx| t.disconnect(cx));
        f.view.update(cx, |_, cx| cx.notify());
        frame(cx);
        assert_eq!(card(cx), None);
        assert!(reply.try_recv().unwrap().is_err());
        assert!(f.other.read_with(cx, |t, _| t.ready()));
    }

    #[gpui::test]
    fn licensed_ready_launch_has_no_card_and_replies_once(cx: &mut TestAppContext) {
        let (f, cx) = fixture(
            cx,
            json!({"status":"signedIn","license":{"state":"active","seat":"pro"}}),
        );
        cx.update(|_, cx| cx.set_global(crate::terminal_gui::TestProbe(Ok(true))));
        let (reply, _) = waiting_launch(&f, cx);
        reply.try_recv().unwrap().unwrap();
        assert_eq!(card(cx), None);
        assert!(no_dialog(cx));
        frame(cx);
        assert!(reply.try_recv().is_err());
    }

    #[gpui::test]
    fn login_success_removes_the_card_and_continues_the_waiting_launch(cx: &mut TestAppContext) {
        let (f, cx) = fixture(cx, json!({"status":"signedOut"}));
        cx.update(|_, cx| cx.set_global(crate::terminal_gui::TestProbe(Ok(true))));
        let (reply, _) = waiting_launch(&f, cx);
        assert!(labels(cx).contains(&"Sign in".into()));
        assert!(reply.try_recv().is_err());
        sign_in(
            &f,
            cx,
            json!({"status":"signedIn","license":{"state":"trial","seat":"pro"}}),
        );
        reply.try_recv().unwrap().unwrap();
        assert_eq!(card(cx), None);
        assert!(f.view.read_with(cx, |v, _| v.page != Page::RemoteApps));
    }

    #[gpui::test]
    fn missing_software_waits_for_set_up_then_continues_after_setup(cx: &mut TestAppContext) {
        let (f, cx) = fixture(
            cx,
            json!({"status":"signedIn","license":{"state":"active","seat":"pro"}}),
        );
        f.view.update(cx, |v, _| {
            let mut other = v.snapshot["config"]["servers"][0].clone();
            other["id"] = json!("other");
            other["name"] = json!("Other");
            v.snapshot["config"]["servers"]
                .as_array_mut()
                .unwrap()
                .push(other);
            v.server = "other".into();
            v.page = Page::Terminal;
        });
        cx.update(|_, cx| cx.set_global(crate::terminal_gui::TestProbe(Ok(false))));
        let (reply, _) = waiting_launch(&f, cx);
        assert!(reply.try_recv().is_err());
        // Nothing moves until Set up is pressed.
        assert_eq!(card(cx), Some("origin"));
        let name = f.view.read_with(cx, |v, _| {
            crate::text(&v.snapshot["config"]["servers"][0], "name")
        });
        assert_host_label(
            cx,
            &format!("Set up {name} to open chromium on your Mac."),
            &format!("Set up {name} to open chromium on this computer."),
        );
        assert!(
            f.view
                .read_with(cx, |v, _| v.server == "other" && v.page == Page::Terminal)
        );
        assert!(focused(&f, cx));
        click(cx, "Set up");
        assert!(f.view.read_with(cx, |v, _| v.server == "fixture"
            && v.page == Page::RemoteApps));
        assert!(no_dialog(cx));
        cx.update(|_, cx| cx.set_global(crate::terminal_gui::TestProbe(Ok(true))));
        f.view.update(cx, |v, cx| {
            let key = remote_apps::status_key("fixture");
            v.resources.fence(&key);
            let generation = v.resources.begin_refresh(&key, true).unwrap();
            v.resources.complete(
                &key,
                generation,
                Ok(json!({"agent":"newport-agent","installedVariant":"full","missing":[]})),
            );
            cx.notify();
        });
        frame(cx);
        reply.try_recv().unwrap().unwrap();
        assert!(
            f.view
                .read_with(cx, |v, _| v.terminal_setup.current.is_none()
                    && v.page == Page::Terminal)
        );
        assert_eq!(card(cx), None);
        assert!(focused(&f, cx));
    }

    #[gpui::test]
    fn connection_failure_offers_retry_without_installation_or_duplicate_execution(
        cx: &mut TestAppContext,
    ) {
        let (f, cx) = fixture(
            cx,
            json!({"status":"signedIn","license":{"state":"active","seat":"pro"}}),
        );
        cx.update(|_, cx| {
            cx.set_global(crate::terminal_gui::TestProbe(Err(
                "SSH connection lost".into()
            )))
        });
        let (reply, _) = waiting_launch(&f, cx);
        assert!(labels(cx).contains(&"Retry".into()));
        assert!(reply.try_recv().is_err());
        assert!(f.view.read_with(cx, |v, _| v.page != Page::RemoteApps));
        cx.update(|_, cx| cx.set_global(crate::terminal_gui::TestProbe(Ok(true))));
        click(cx, "Retry");
        reply.try_recv().unwrap().unwrap();
        assert_eq!(card(cx), None);
    }

    #[gpui::test]
    fn cancelled_launch_never_runs_after_login_and_queued_launches_are_preserved(
        cx: &mut TestAppContext,
    ) {
        let (f, cx) = fixture(cx, json!({"status":"signedOut"}));
        let (first, cancelled) = waiting_launch(&f, cx);
        let (second, _) = waiting_launch(&f, cx);
        cancelled.store(true, Ordering::Release);
        frame(cx);
        assert!(first.try_recv().unwrap().is_err());
        assert_eq!(card(cx), Some("origin"));
        cx.update(|_, cx| cx.set_global(crate::terminal_gui::TestProbe(Ok(true))));
        sign_in(
            &f,
            cx,
            json!({"status":"signedIn","license":{"state":"active","seat":"pro"}}),
        );
        second.try_recv().unwrap().unwrap();
        assert!(first.try_recv().is_err());
    }

    #[gpui::test]
    fn guidance_for_a_waiting_launch_leaves_the_terminal_connected(cx: &mut TestAppContext) {
        let (f, cx) = fixture(cx, json!({"status":"signedOut"}));
        let connected = |cx: &mut VisualTestContext| {
            f.terminal.read_with(cx, |terminal, _| {
                terminal.status.connected()
                    && terminal.status.steady()
                    && terminal.status.error().is_none()
            })
        };
        // Sign-in guidance, then cancelling it: the launch fails, the shell stays.
        let (reply, _) = waiting_launch(&f, cx);
        assert!(labels(cx).contains(&"Sign in".into()));
        assert!(connected(cx));
        click(cx, "Cancel launch");
        assert!(reply.try_recv().unwrap().is_err());
        assert!(connected(cx));
        // A failed readiness check offers Retry; ignoring it changes nothing.
        f.view.update(cx, |v, cx| {
            v.account =
                account(json!({"status":"signedIn","license":{"state":"active","seat":"pro"}}));
            v.terminal_setup.dismissed = None;
            cx.notify();
        });
        cx.update(|_, cx| {
            cx.set_global(crate::terminal_gui::TestProbe(Err(
                "Needle isn’t installed".into(),
            )))
        });
        let (reply, _) = waiting_launch(&f, cx);
        assert!(labels(cx).contains(&"Retry".into()));
        assert!(reply.try_recv().is_err());
        assert!(connected(cx));
        assert!(f.view.read_with(cx, |v, _| v.page != Page::RemoteApps));
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
                Arc::new(AtomicBool::new(false)),
                cx,
            )
        });
        frame(cx);
        assert!(reply.try_recv().unwrap().is_err());
        assert_eq!(card(cx), None);
        assert!(no_dialog(cx));
    }
    #[test]
    fn terminal_launches_wait_for_software_every_app_needs() {
        let mut status = json!({"agent":"newport-agent", "installedVariant":"full",
            "gaps": [{"requirement": "audio", "state": "missing"}]});
        assert!(super::setup_ready(&status), "only some apps need sound");
        status["gaps"] = json!([{"requirement": "xwayland", "state": "missing"}]);
        assert!(super::setup_ready(&status), "only X11 apps need XWayland");
        for requirement in ["dbus", "fonts"] {
            status["gaps"] = json!([{"requirement": requirement, "state": "missing"}]);
            assert!(!super::setup_ready(&status), "{requirement}");
        }
    }
}
