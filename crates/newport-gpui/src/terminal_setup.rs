//! Account and setup guidance for GUI apps in a terminal, when a program
//! there needs a display Newport can't show yet. The terminal never waits for
//! it: its shell stays connected and usable whether the guidance is followed,
//! dismissed or fails.
use super::*;
use crate::terminal_view::TerminalView;
use newport_client::account::Action;
use newport_protocol::desktop::account::{NoticeKind, Phase};
use remote_apps::Entitlement;
use std::{collections::HashMap, rc::Rc};

/// A terminal that can show the guidance card.
#[derive(Clone)]
struct Origin {
    server: String,
    id: uuid::Uuid,
    view: WeakEntity<TerminalView>,
    /// When it registered, so a card's terminal is chosen the same way
    /// whatever order the terminals are kept in.
    registered: u64,
}
impl Origin {
    fn live(&self, cx: &App) -> bool {
        self.view.upgrade().is_some_and(|view| {
            let view = view.read(cx);
            view.session_id() == Some(self.id) && view.active()
        })
    }
}
/// A server whose terminal asked for GUI apps.
struct Request {
    server: String,
    /// Set up was pressed: Remote apps is open on its server.
    routed: bool,
}
#[derive(Default)]
struct Requests {
    queue: std::collections::VecDeque<Request>,
    terminals: HashMap<uuid::Uuid, Origin>,
    registrations: u64,
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
    requests.registrations += 1;
    let registered = requests.registrations;
    requests.terminals.insert(
        id,
        Origin {
            id,
            server: server.into(),
            view,
            registered,
        },
    );
}
pub(crate) fn request(server: &str, cx: &mut App) {
    if !remote_apps::available(cx) {
        return;
    }
    let requests = requests(cx);
    if requests.queue.len() >= 32 || requests.queue.iter().any(|r| r.server == server) {
        return;
    }
    requests.queue.push_back(Request {
        server: server.into(),
        routed: false,
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
    syncing: bool,
    /// The terminal showing the card, and the card itself.
    anchor: Option<WeakEntity<TerminalView>>,
    card: Option<Entity<Card>>,
}
/// The guidance card in a terminal on the server. It never takes focus or
/// covers the shell; only its buttons act.
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
            "Sign in to open GUI apps. They require Newport Pro or a free trial.".into(),
            Some(("Sign in", Action::SignIn))),
        Entitlement::Waiting => (
            "Finish signing in in your browser. GUI apps open once access is ready.".into(),
            Some(("Open sign-in page", Action::SignIn))),
        Entitlement::TrialAvailable => (
            "Start a free 14-day trial to open GUI apps on this computer. No card needed.".into(),
            Some(("Start free trial", Action::StartTrial))),
        Entitlement::TrialUsedHere => (
            "This computer has already used a free trial. Upgrade to Newport Pro to open GUI apps.".into(),
            Some(("Upgrade to Pro", Action::OpenBilling))),
        Entitlement::Ended { .. } => (
            "There’s no active Newport Pro license on this computer. Activate Pro in billing to continue.".into(),
            Some(("Open billing", Action::OpenBilling))),
        Entitlement::FreeSeat => (
            seat_message.filter(|s| !s.trim().is_empty()).unwrap_or("Assign a Pro seat to this computer to open GUI apps.").into(),
            Some(("Manage computers", Action::OpenPortal))),
        Entitlement::TrialElsewhere => (
            "Your free trial is on another computer. Upgrade to Newport Pro to open GUI apps here.".into(),
            Some(("Upgrade to Pro", Action::OpenBilling))),
        Entitlement::Pro => ("Checking the server…".into(), None),
    };
    Step { detail, action }
}
/// Needle can open apps: it is installed, and nothing every app needs is
/// missing. Otherwise Set up leads to Remote apps, which says what to install.
pub(crate) fn setup_ready(status: &newport_protocol::desktop::applications::Availability) -> bool {
    remote_apps::launch_agent(status).is_ok()
        && status
            .gaps
            .iter()
            .all(newport_domain::requirements::optional)
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
            while let Some(request) = if cx.has_global::<Requests>() {
                cx.global_mut::<Requests>().queue.pop_front()
            } else {
                None
            } {
                let quiet = self.terminal_setup.dismissed_server.as_deref()
                    == Some(&request.server)
                    && self
                        .terminal_setup
                        .dismissed
                        .is_some_and(|at| at.elapsed() < Duration::from_secs(3));
                if quiet || self.saved_server(&request.server).is_none() {
                    continue;
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
        if self.saved_server(&request.server).is_none() {
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
                let key = remote_apps::status_key(&request.server);
                let ready = self.resources.get(&key).is_some_and(|read| {
                    !read.pending
                        && read.error.is_none()
                        && self
                            .remote_apps_readings
                            .status(&self.resources, &key)
                            .is_some_and(setup_ready)
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
                    self.terminal_setup.current = None;
                    self.terminal_setup.readiness = None;
                    self.notification = Some(Notification::success(
                        "Run the app by its command name in the terminal, or launch it from Remote apps.",
                    ));
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
    pub(super) fn terminal_launch_server(&mut self, server: String) {
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
            || (self.terminal_setup.current.is_none() && self.terminal_setup.anchor.is_none())
        {
            return;
        }
        self.terminal_setup.syncing = true;
        cx.defer_in(window, |this, _, cx| {
            this.terminal_setup.syncing = false;
            this.place_terminal_setup_card(cx);
        });
    }
    /// The terminal that shows the card: one on its server, the one in front
    /// when it can (its workspace's selected one, else the one with focus,
    /// else one shown), the newest among equals. None while the account
    /// loads or the server is checked.
    fn terminal_setup_anchor(&self, cx: &App) -> Option<Entity<TerminalView>> {
        let request = self.terminal_setup.current.as_ref()?;
        let status = self.account.status();
        let loading =
            Entitlement::of(status) == Entitlement::Unknown && status.state != Phase::SignedIn;
        if loading || self.terminal_setup.stage == Stage::Starting {
            return None;
        }
        let mut origins: Vec<_> = cx
            .try_global::<Requests>()?
            .terminals
            .values()
            .filter(|origin| origin.server == request.server && origin.live(cx))
            .collect();
        origins.sort_by_key(|origin| std::cmp::Reverse(origin.registered));
        let terminals: Vec<_> = origins
            .into_iter()
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
            .or_else(|| terminals.iter().find(|t| t.read(cx).focused()).cloned())
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
            self.terminal_setup.dismissed_server = Some(request.server);
        }
        self.terminal_setup.readiness = None;
        self.terminal_setup.error = None;
        self.terminal_setup.dismissed = Some(Instant::now());
    }
    /// Opens Remote Apps on the server to finish its setup. The guidance
    /// closes once setup is ready.
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
        self.terminal_setup
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
                .child(
                    div()
                        .flex_1()
                        .child("Complete setup to open GUI apps from your terminal."),
                )
                .child(
                    crate::controls::form_button("terminal-gui-cancel-setup")
                        .label("Dismiss")
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
        let license_missing =
            entitlement == Entitlement::Unknown && status.state == Phase::SignedIn;
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
                let server = self
                    .saved_server(&request.server)
                    .map(|s| s.name.clone())
                    .filter(|name| !name.is_empty())
                    .unwrap_or_else(|| request.server.clone());
                step.detail = format!("Set up {server} to open GUI apps on {computer}.");
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
        let cancel = "Dismiss";
        let title = format!("Open GUI apps on {computer}");
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
    use newport_protocol::desktop::account::{License, Phase, Status};
    use std::{cell::RefCell, rc::Rc};

    /// Newport above two terminals on the same server.
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

    fn account(status: Status) -> Account {
        let mut account = Account::with_replies(move |_| Ok(status.clone()));
        account.poll();
        account.poll();
        account
    }

    /// The owner's selected server.
    const SERVER_ID: &str = "00000000-0000-4000-8000-000000000001";

    fn fixture(cx: &mut TestAppContext, status: Status) -> (Fixture, &mut VisualTestContext) {
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
            let origin = cx.new(|cx| attached(&peer, SERVER_ID, cx));
            let other = cx.new(|cx| attached(&other_peer, SERVER_ID, cx));
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
                super::register(id, SERVER_ID, terminal.downgrade(), cx);
            });
        }
        // The person is in the origin terminal, in the window in front.
        visual.update(|window, cx| {
            window.activate_window();
            terminal.read(cx).focus.clone().focus(window, cx)
        });
        frame(visual);
        (
            Fixture {
                view,
                terminal,
                peer,
                _other_peer: other_peer,
            },
            visual,
        )
    }

    fn request_setup(view: &Entity<Newport>, cx: &mut VisualTestContext) {
        view.update(cx, |view, cx| {
            request(SERVER_ID, cx);
            view.poll_terminal_setup(cx);
            cx.notify();
        });
        frame(cx);
    }

    fn sign_in(f: &Fixture, cx: &mut VisualTestContext, status: Status) {
        f.view.update(cx, |v, cx| {
            v.account = account(status);
            cx.notify();
        });
        frame(cx);
    }

    #[gpui::test]
    fn opening_a_terminal_without_a_license_does_not_request_setup(cx: &mut TestAppContext) {
        let (f, cx) = fixture(cx, crate::account_fixture::status(Phase::SignedOut));
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
        let (f, cx) = fixture(cx, crate::account_fixture::status(Phase::SignedOut));
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
        let (f, cx) = fixture(cx, crate::account_fixture::status(Phase::Loading));
        cx.update(|_, cx| cx.set_global(crate::terminal_gui::TestProbe(Ok(false))));
        request_setup(&f.view, cx);
        assert_eq!(card(cx), None);
        sign_in(
            &f,
            cx,
            crate::account_fixture::signed_in(Some(crate::account_fixture::license(
                "active",
                Some("pro"),
            ))),
        );
        assert!(!labels(cx).contains(&"Start free trial".into()));
        assert!(labels(cx).contains(&"Set up".into()));
        assert!(f.view.read_with(cx, |v, _| v.page != Page::RemoteApps));
        assert!(no_dialog(cx));
    }

    #[gpui::test]
    fn terminal_setup_missing_license_offers_recovery(cx: &mut TestAppContext) {
        let (f, cx) = fixture(cx, crate::account_fixture::signed_in(None));
        request_setup(&f.view, cx);
        assert!(labels(cx).contains(&"Refresh account".into()));
        assert!(!labels(cx).contains(&"Start free trial".into()));
        click(cx, "Dismiss");
        assert_eq!(card(cx), None);
    }

    #[gpui::test]
    fn terminal_setup_updates_in_place_and_displays_action_errors(cx: &mut TestAppContext) {
        let (f, cx) = fixture(cx, crate::account_fixture::status(Phase::SignedOut));
        let status = Rc::new(RefCell::new(crate::account_fixture::status(
            Phase::SignedOut,
        )));
        let calls = Rc::new(RefCell::new(Vec::new()));
        f.view.update(cx, |v, _| {
            let (status, calls) = (status.clone(), calls.clone());
            v.account = Account::with_replies(move |method| {
                calls.borrow_mut().push(method);
                if method == newport_protocol::desktop::Kind::AccountStartTrial {
                    return Err("Network unavailable".into());
                }
                if method == newport_protocol::desktop::Kind::AccountSignIn {
                    *status.borrow_mut() = crate::account_fixture::status(Phase::Waiting);
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
        assert!(
            calls
                .borrow()
                .contains(&newport_protocol::desktop::Kind::AccountSignIn)
        );
        assert!(labels(cx).contains(&"Open sign-in page".into()));
        *status.borrow_mut() =
            crate::account_fixture::signed_in(Some(crate::account_fixture::license("none", None)));
        f.view.update(cx, |v, cx| {
            let (status, calls) = (status.clone(), calls.clone());
            v.account = Account::with_replies(move |method| {
                calls.borrow_mut().push(method);
                if method == newport_protocol::desktop::Kind::AccountStartTrial {
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
        let license = crate::account_fixture::license;
        for (license, action) in [
            (license("none", None), Action::StartTrial),
            (
                License {
                    trial_used_here: Some(true),
                    ..license("none", None)
                },
                Action::OpenBilling,
            ),
            (
                License {
                    trial_used: Some(true),
                    ..license("expired", None)
                },
                Action::OpenBilling,
            ),
            (license("active", Some("free")), Action::OpenPortal),
            (license("trial", Some("free")), Action::OpenBilling),
        ] {
            let status = crate::account_fixture::signed_in(Some(license));
            assert_eq!(
                step(Entitlement::of(&status), None).action.unwrap().1,
                action
            );
        }
    }

    #[gpui::test]
    fn the_card_shows_in_a_terminal_and_never_takes_the_keyboard(cx: &mut TestAppContext) {
        let (f, cx) = fixture(cx, crate::account_fixture::status(Phase::SignedOut));
        request_setup(&f.view, cx);
        assert!(card(cx).is_some());
        assert!(labels(cx).contains(&"Dismiss".into()));
        assert!(no_dialog(cx));
        assert!(focused(&f, cx));
        // Typing, Esc included, reaches the shell and leaves the card.
        cx.simulate_keystrokes("l s escape");
        wait(&f.terminal, cx, |_| f.peer.written().len() >= 3);
        assert_eq!(f.peer.written(), b"ls\x1b");
        frame(cx);
        assert!(card(cx).is_some());
        // Pressing a card button keeps the terminal's focus.
        click(cx, "Sign in");
        assert!(focused(&f, cx));
        cx.simulate_keystrokes("x");
        wait(&f.terminal, cx, |_| f.peer.written().len() >= 4);
        assert_eq!(f.peer.written(), b"ls\x1bx");
        // × closes it; the shell stays connected and focused.
        close(cx);
        assert_eq!(card(cx), None);
        assert!(focused(&f, cx));
        assert!(f.terminal.read_with(cx, |t, _| t.ready()));
    }

    #[gpui::test]
    fn a_ready_licensed_server_needs_no_card(cx: &mut TestAppContext) {
        let (f, cx) = fixture(
            cx,
            crate::account_fixture::signed_in(Some(crate::account_fixture::license(
                "active",
                Some("pro"),
            ))),
        );
        cx.update(|_, cx| cx.set_global(crate::terminal_gui::TestProbe(Ok(true))));
        request_setup(&f.view, cx);
        frame(cx);
        assert_eq!(card(cx), None);
        assert!(no_dialog(cx));
        assert!(
            f.view
                .read_with(cx, |v, _| v.terminal_setup.current.is_none())
        );
    }

    #[gpui::test]
    fn signing_in_removes_the_card(cx: &mut TestAppContext) {
        let (f, cx) = fixture(cx, crate::account_fixture::status(Phase::SignedOut));
        cx.update(|_, cx| cx.set_global(crate::terminal_gui::TestProbe(Ok(true))));
        request_setup(&f.view, cx);
        assert!(labels(cx).contains(&"Sign in".into()));
        sign_in(
            &f,
            cx,
            crate::account_fixture::signed_in(Some(crate::account_fixture::license(
                "trial",
                Some("pro"),
            ))),
        );
        assert_eq!(card(cx), None);
        assert!(
            f.view
                .read_with(cx, |v, _| v.terminal_setup.current.is_none()
                    && v.page != Page::RemoteApps)
        );
    }

    #[gpui::test]
    fn missing_software_waits_for_set_up_then_closes_once_set_up(cx: &mut TestAppContext) {
        let (f, cx) = fixture(
            cx,
            crate::account_fixture::signed_in(Some(crate::account_fixture::license(
                "active",
                Some("pro"),
            ))),
        );
        f.view.update(cx, |v, _| {
            let servers = &mut v.snapshot_mut().config.servers;
            let mut other = servers[0].clone();
            other.id = crate::snapshot_fixture::SECOND;
            other.name = "Other".into();
            servers.push(other);
            v.server = crate::snapshot_fixture::SECOND.to_string();
            v.page = Page::Terminal;
        });
        cx.update(|_, cx| cx.set_global(crate::terminal_gui::TestProbe(Ok(false))));
        request_setup(&f.view, cx);
        // Nothing moves until Set up is pressed.
        assert!(card(cx).is_some());
        let name = f.view.read_with(cx, |v, _| v.servers()[0].name.clone());
        assert_host_label(
            cx,
            &format!("Set up {name} to open GUI apps on your Mac."),
            &format!("Set up {name} to open GUI apps on this computer."),
        );
        assert!(f.view.read_with(cx, |v, _| v.server
            == crate::snapshot_fixture::SECOND.to_string()
            && v.page == Page::Terminal));
        assert!(focused(&f, cx));
        click(cx, "Set up");
        assert!(f.view.read_with(cx, |v, _| v.server == SERVER_ID
            && v.page == Page::RemoteApps));
        assert!(no_dialog(cx));
        cx.update(|_, cx| cx.set_global(crate::terminal_gui::TestProbe(Ok(true))));
        f.view.update(cx, |v, cx| {
            let key = remote_apps::status_key(SERVER_ID);
            v.resources.fence(&key);
            let generation = v.resources.begin_refresh(&key, true).unwrap();
            v.answer_remote_apps(
                &key,
                generation,
                crate::remote_apps_fixture::status_reply(crate::remote_apps_fixture::installed(
                    "newport-agent",
                    "ours",
                )),
            );
            cx.notify();
        });
        frame(cx);
        assert!(
            f.view
                .read_with(cx, |v, _| v.terminal_setup.current.is_none())
        );
        assert_eq!(card(cx), None);
    }

    #[gpui::test]
    fn connection_failure_offers_retry_without_installation(cx: &mut TestAppContext) {
        let (f, cx) = fixture(
            cx,
            crate::account_fixture::signed_in(Some(crate::account_fixture::license(
                "active",
                Some("pro"),
            ))),
        );
        cx.update(|_, cx| {
            cx.set_global(crate::terminal_gui::TestProbe(Err(
                "SSH connection lost".into()
            )))
        });
        request_setup(&f.view, cx);
        assert!(labels(cx).contains(&"Retry".into()));
        // On the terminal in use, of the server's two.
        assert_eq!(card(cx), Some("origin"));
        assert!(f.view.read_with(cx, |v, _| v.page != Page::RemoteApps));
        cx.update(|_, cx| cx.set_global(crate::terminal_gui::TestProbe(Ok(true))));
        click(cx, "Retry");
        frame(cx);
        assert!(
            f.view
                .read_with(cx, |v, _| v.terminal_setup.current.is_none())
        );
        assert_eq!(card(cx), None);
    }

    #[gpui::test]
    fn guidance_leaves_the_terminal_connected(cx: &mut TestAppContext) {
        let (f, cx) = fixture(cx, crate::account_fixture::status(Phase::SignedOut));
        let connected = |cx: &mut VisualTestContext| {
            f.terminal.read_with(cx, |terminal, _| {
                terminal.status.connected()
                    && terminal.status.steady()
                    && terminal.status.error().is_none()
            })
        };
        // Sign-in guidance, then dismissing it: the shell stays.
        request_setup(&f.view, cx);
        assert!(labels(cx).contains(&"Sign in".into()));
        assert!(connected(cx));
        click(cx, "Dismiss");
        assert!(connected(cx));
        // A failed readiness check offers Retry; ignoring it changes nothing.
        f.view.update(cx, |v, cx| {
            v.account = account(crate::account_fixture::signed_in(Some(
                crate::account_fixture::license("active", Some("pro")),
            )));
            v.terminal_setup.dismissed = None;
            cx.notify();
        });
        cx.update(|_, cx| {
            cx.set_global(crate::terminal_gui::TestProbe(Err(
                "Needle isn’t installed".into(),
            )))
        });
        request_setup(&f.view, cx);
        assert!(labels(cx).contains(&"Retry".into()));
        assert!(connected(cx));
        assert!(f.view.read_with(cx, |v, _| v.page != Page::RemoteApps));
    }

    #[test]
    fn setup_is_ready_once_the_server_has_what_every_app_needs() {
        use crate::remote_apps_fixture::{installed, missing};
        use newport_protocol::desktop::applications::Requirement;
        let mut status = installed("newport-agent", "ours");
        status.gaps = vec![missing(Requirement::Audio)];
        assert!(super::setup_ready(&status), "only some apps need sound");
        status.gaps = vec![missing(Requirement::Xwayland)];
        assert!(super::setup_ready(&status), "only X11 apps need XWayland");
        for requirement in [Requirement::Dbus, Requirement::Fonts] {
            status.gaps = vec![missing(requirement)];
            assert!(!super::setup_ready(&status), "{requirement:?}");
        }
    }
}
