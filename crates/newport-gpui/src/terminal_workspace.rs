use crate::{
    terminal_status::{ExitPolicy, Phase},
    terminal_view::TerminalView,
    *,
};
use gpui_kit::{base::ElementExt, component::dialog::DialogButtonProps};
use newport_client::terminal_command::CommandDraft;

use newport_domain::terminal::MAX_TERMINALS;

/// Each server's tabs saved by the last run, taken by the server's
/// workspace when it is created.
pub(crate) struct RestoredTabs(pub std::collections::HashMap<String, SavedTerminals>);
impl Global for RestoredTabs {}

/// Where server workspaces and project editors save their tabs.
pub(crate) struct TabsSaver(pub crate::ui_state::Writer);
impl Global for TabsSaver {}

/// Where server workspaces ask for the server's held shells.
pub(crate) struct HeldShellsSource(
    pub fn(uuid::Uuid) -> Result<newport_domain::terminal::HeldShells, String>,
);
impl Global for HeldShellsSource {}

use crate::ui_state::SavedTerminals;
use newport_domain::terminal::HeldShells;

/// How often a server's workspace asks for the server's held shells.
const LIST_INTERVAL: Duration = Duration::from_secs(1);
/// How long a name this Mac gave a shell outranks the server's, which may
/// not have it yet.
const RENAME_SETTLE: Duration = Duration::from_secs(5);

struct TerminalTab {
    id: usize,
    /// Its number in the tab's title: the lowest no open tab has, so numbers
    /// start over once tabs close.
    number: usize,
    focus: FocusHandle,
    custom: Option<String>,
    /// A name for the server's copy once the shell is held: this Mac named
    /// the tab, or opened it named.
    unsent_name: Option<Option<String>>,
    /// When this Mac last named the tab.
    named_at: Option<Instant>,
    /// The other Macs attached to the tab's held shell.
    elsewhere: Vec<String>,
    view: Entity<TerminalView>,
    exit: ExitPolicy,
    _observer: Subscription,
    _links: Subscription,
}

struct HeldClose {
    id: usize,
    terminal: uuid::Uuid,
    connection: Option<configuration::ConnectionScope>,
    keyboard: bool,
}

struct Rename {
    id: usize,
    input: Entity<InputState>,
    _subscription: Subscription,
    _blur: Subscription,
}

pub struct Workspace {
    endpoint: String,
    project_root: Option<String>,
    immersive: bool,
    server: String,
    connection: Option<configuration::ConnectionScope>,
    tabs: Vec<TerminalTab>,
    active: usize,
    next: usize,
    rename: Option<Rename>,
    context_tab: Option<usize>,
    /// The tab being dragged to a new place, while a drag is active.
    dragging: Option<usize>,
    /// Where the pointer is across the tab strip while a tab is dragged.
    drag_pointer: Option<(Pixels, Bounds<Pixels>)>,
    /// Scrolls the tab strip while a dragged tab is held near its edge.
    _drag_scroll: Option<Task<()>>,
    tab_scroll: ScrollHandle,
    /// The width the tab row leaves free after the last tab, as last drawn.
    /// The strip is only as wide as its tabs, so it can't say how wide a tab
    /// may grow: the strip and this space together can.
    tab_spare: std::rc::Rc<std::cell::Cell<Pixels>>,
    revealed_tab: Option<(usize, Pixels, Pixels, Pixels)>,
    /// A server's own workspace, whose tabs' shells outlive Newport and are
    /// restored when it opens again. A project's terminals are its own.
    holdable: bool,
    /// The tabs last saved, so only changes are written.
    saved: Option<SavedTerminals>,
    /// Waiting for the server's first list, which decides the tabs, with
    /// the shell this Mac had selected there.
    pending: Option<Option<uuid::Uuid>>,
    /// The order this Mac last left the server's shells in, for the first list.
    restored_order: Vec<uuid::Uuid>,
    /// The agents' finishes this Mac had seen, by terminal, for the first list.
    restored_seen: std::collections::BTreeMap<uuid::Uuid, String>,
    /// Shells this Mac ended, until they leave the server's list: they must
    /// not come back as tabs meanwhile.
    ended: Vec<uuid::Uuid>,
    /// An explicit End is awaiting its result; duplicate gestures never replay it.
    closing: Vec<uuid::Uuid>,
    _list: Option<Task<()>>,
}

impl Workspace {
    pub(super) fn active_view(&self) -> Option<Entity<TerminalView>> {
        self.tabs.get(self.active).map(|tab| tab.view.clone())
    }
    pub(super) fn contains_focus(&self, window: &Window, cx: &App) -> bool {
        self.tabs.iter().any(|tab| {
            tab.focus.contains_focused(window, cx)
                || tab.view.read(cx).focus.contains_focused(window, cx)
        }) || self.rename.as_ref().is_some_and(|rename| {
            rename
                .input
                .read(cx)
                .focus_handle(cx)
                .contains_focused(window, cx)
        })
    }
    pub(super) fn focus_active(&self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(tab) = self.tabs.get(self.active) {
            tab.view.read(cx).focus.clone().focus(window, cx);
        }
    }
    pub(super) fn cycle_tab(&mut self, forward: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.tabs.is_empty() || self.rename.is_some() {
            return;
        }
        let count = self.tabs.len();
        self.active = (self.active + if forward { 1 } else { count - 1 }) % count;
        self.focus_active(window, cx);
        cx.notify();
    }
    /// Tab `number` (1–8), or the last tab for 9, as browsers number them.
    /// A new shell tab, focused, unless the tabs are at their limit.
    pub(super) fn new_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.finish_rename(true, false, window, cx);
        self.add(cx);
        self.opened(cx);
        if let Some(tab) = self.tabs.get(self.active) {
            let focus = tab.view.read(cx).focus.clone();
            focus.focus(window, cx);
        }
    }
    pub(super) fn select_tab(
        &mut self,
        number: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(index) = crate::navigation::tab_index(number, self.tabs.len()) else {
            return;
        };
        if self.rename.is_some() {
            return;
        }
        self.active = index;
        self.focus_active(window, cx);
        cx.notify();
    }
    pub fn deactivate(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        for tab in &self.tabs {
            tab.view.update(cx, |view, cx| view.deactivate(window, cx));
        }
    }
    pub fn open_find(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.rename.is_none()
            && let Some(tab) = self.tabs.get(self.active)
        {
            tab.view.update(cx, |view, cx| view.open_find(window, cx));
        }
    }
    pub fn set_immersive(&mut self, immersive: bool, cx: &mut Context<Self>) {
        if self.immersive != immersive {
            self.immersive = immersive;
            cx.notify();
        }
    }
    pub fn sync_connection(
        &mut self,
        connection: Option<configuration::ConnectionScope>,
        endpoint: String,
        cx: &mut Context<Self>,
    ) {
        self.endpoint = endpoint;
        if self.connection == connection {
            return;
        }
        self.connection = connection;
        for tab in &self.tabs {
            tab.view.update(cx, |view, cx| view.reset_connection(cx));
        }
        cx.notify();
    }
    pub(super) fn menu_state(&self, cx: &App) -> Option<(usize, bool, bool, bool)> {
        self.tab_menu_state(self.tabs.get(self.active)?.id, cx)
    }
    /// Whether tab `id` can move left and right.
    pub(super) fn tab_moves(&self, id: usize) -> (bool, bool) {
        let index = self.tabs.iter().position(|tab| tab.id == id);
        (
            index.is_some_and(|index| index > 0),
            index.is_some_and(|index| index + 1 < self.tabs.len()),
        )
    }
    /// Moves tab `id` one place, unless it is already at that end.
    pub(super) fn move_tab_by_one(&mut self, id: usize, forward: bool, cx: &mut Context<Self>) {
        let Some(index) = self.tabs.iter().position(|tab| tab.id == id) else {
            return;
        };
        if let Some(to) = if forward {
            index.checked_add(1)
        } else {
            index.checked_sub(1)
        } {
            self.move_tab(id, to, cx);
        }
    }
    /// Moves the selected tab one place.
    pub(super) fn move_active_tab(&mut self, forward: bool, cx: &mut Context<Self>) {
        if self.rename.is_some() {
            return;
        }
        if let Some(id) = self.tabs.get(self.active).map(|tab| tab.id) {
            self.move_tab_by_one(id, forward, cx);
        }
    }
    pub(super) fn tab_menu_state(&self, id: usize, cx: &App) -> Option<(usize, bool, bool, bool)> {
        if self.rename.is_some() {
            return None;
        }
        self.tabs.iter().find(|tab| tab.id == id).map(|tab| {
            (
                tab.id,
                tab.custom.is_some(),
                tab.view.read(cx).active(),
                self.tabs.len() > 1,
            )
        })
    }
    pub(super) fn rename(&mut self, id: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(tab) = self.tabs.iter().find(|tab| tab.id == id) else {
            return;
        };
        let title = tab
            .custom
            .clone()
            .unwrap_or_else(|| tab.view.read(cx).title.clone());
        let input = cx.new(|cx| {
            let mut input = InputState::new(window, cx);
            input.set_value(title, window, cx);
            input.select_all(window, cx);
            input
        });
        let subscription = cx.subscribe_in(
            &input,
            window,
            move |this, _, event: &gpui_kit::component::input::InputEvent, window, cx| {
                if this.rename.as_ref().is_none_or(|rename| rename.id != id) {
                    return;
                }
                match event {
                    gpui_kit::component::input::InputEvent::PressEnter { .. } => {
                        this.finish_rename(true, true, window, cx);
                    }
                    gpui_kit::component::input::InputEvent::Blur => {
                        // Clicking another control must keep the newly chosen focus.
                        this.finish_rename(true, false, window, cx);
                    }
                    _ => {}
                }
            },
        );
        let focus = input.read(cx).focus_handle(cx);
        let blur = cx.on_blur(&focus, window, move |this, window, cx| {
            if this.rename.as_ref().is_some_and(|rename| rename.id == id) {
                this.finish_rename(true, false, window, cx);
            }
        });
        self.rename = Some(Rename {
            id,
            input,
            _subscription: subscription,
            _blur: blur,
        });
        window.defer(cx, move |window, cx| focus.focus(window, cx));
        cx.notify();
    }
    fn finish_rename(
        &mut self,
        save: bool,
        restore_focus: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(rename) = self.rename.take() else {
            return;
        };
        if let Some(tab) = self.tabs.iter_mut().find(|tab| tab.id == rename.id) {
            if save {
                let name = custom_name(rename.input.read(cx).value().as_ref());
                if name != tab.custom {
                    tab.custom = name.clone();
                    tab.unsent_name = Some(name);
                }
            }
            if restore_focus {
                tab.focus.focus(window, cx);
            }
        }
        self.send_names(cx);
        cx.notify();
    }
    pub(super) fn reset_name(&mut self, id: usize, cx: &mut Context<Self>) {
        if let Some(tab) = self.tabs.iter_mut().find(|tab| tab.id == id) {
            tab.custom = None;
            tab.unsent_name = Some(None);
            self.send_names(cx);
            cx.notify();
        }
    }
    /// Sends names given on this Mac to the server, for every Mac's tab,
    /// once their shells are held. The server's name is its holder's.
    fn send_names(&mut self, cx: &mut Context<Self>) {
        let Ok(server) = uuid::Uuid::parse_str(&self.server) else {
            return;
        };
        for tab in &mut self.tabs {
            let Some(held) = tab.view.read(cx).held() else {
                continue;
            };
            let Some(name) = tab.unsent_name.take() else {
                continue;
            };
            let at = Instant::now();
            tab.named_at = Some(at);
            let id = tab.id;
            let connection = self.connection.clone();
            let task = cx.background_spawn(async move {
                newport_client::terminal_transport::rename_held(
                    server,
                    held,
                    &name.unwrap_or_default(),
                )
            });
            cx.spawn(async move |this, cx| {
                if let Err(error) = task.await {
                    let _ = this.update(cx, |this, cx| {
                        if this.connection != connection {
                            return;
                        }
                        if let Some(tab) = this.tabs.iter_mut().find(|tab| tab.id == id)
                            && tab.view.read(cx).held() == Some(held)
                            && tab.named_at == Some(at)
                            && tab.unsent_name.is_none()
                        {
                            // The next authoritative list restores the real name.
                            tab.named_at = None;
                            tab.view.update(cx, |view, cx| {
                                view.input_error = Some(error.message);
                                cx.notify();
                            });
                            cx.notify();
                        }
                    });
                }
            })
            .detach();
        }
    }
    pub(super) fn disconnect(&mut self, id: usize, cx: &mut Context<Self>) {
        if let Some(tab) = self.tabs.iter().find(|tab| tab.id == id) {
            tab.view.update(cx, |view, cx| {
                if view.active() {
                    view.disconnect(cx);
                }
            });
        }
    }
    pub(super) fn reconnect(&mut self, id: usize, window: &mut Window, cx: &mut Context<Self>) {
        let selected = self.tabs.get(self.active).is_some_and(|tab| tab.id == id);
        if let Some(tab) = self.tabs.iter().find(|tab| tab.id == id) {
            tab.view.update(cx, |view, cx| {
                if !view.active() {
                    view.reconnect(cx);
                    if selected {
                        view.focus.focus(window, cx);
                    }
                }
            });
        }
    }
    pub fn new(
        server: String,
        connection: Option<configuration::ConnectionScope>,
        endpoint: String,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut workspace = Self::empty(server, connection, endpoint, cx);
        // The server's shells come first: a new one only when it has none.
        if !workspace.hold(cx) {
            workspace.add(cx);
        }
        workspace
    }

    /// Makes this a server's own workspace: its tabs are the server's held
    /// shells, which outlive Newport and show on every Mac. False when no
    /// list will come to settle them.
    fn hold(&mut self, cx: &mut Context<Self>) -> bool {
        self.holdable = true;
        let Some(source) = cx.try_global::<HeldShellsSource>().map(|source| source.0) else {
            return false;
        };
        let Ok(server) = uuid::Uuid::parse_str(&self.server) else {
            return false;
        };
        let saved = cx
            .has_global::<RestoredTabs>()
            .then(|| cx.global_mut::<RestoredTabs>().0.remove(&self.server))
            .flatten()
            .unwrap_or_default();
        self.pending = Some(saved.selected);
        self.restored_order = saved.order;
        self.restored_seen = saved.seen;
        self._list = Some(cx.spawn(async move |this, cx| {
            loop {
                let listed = cx.background_spawn(async move { source(server) }).await;
                // A list that failed is asked for again; the tabs stay as they are.
                let updated = this.update(cx, |this, cx| {
                    if let Ok(listed) = listed {
                        this.reconcile(listed, cx);
                    }
                });
                if updated.is_err() {
                    break;
                }
                cx.background_executor().timer(LIST_INTERVAL).await;
            }
        }));
        true
    }

    /// A tab for a held shell, selected, attaching when first shown.
    fn add_held(&mut self, held: uuid::Uuid, cx: &mut Context<Self>) {
        self.add_tab(None, cx);
        self.tabs[self.active]
            .view
            .update(cx, |view, _| view.attach_to(held));
    }

    /// The terminal a tab shows: its held shell, or the one it is starting,
    /// which the server may list before the tab hears it is held.
    fn terminal(tab: &TerminalTab, cx: &App) -> Option<uuid::Uuid> {
        let view = tab.view.read(cx);
        view.held().or(view.session_id())
    }

    /// Settles the tabs on the server's list: its shells appear as tabs
    /// wherever they were started, take their names from it, and go when it
    /// no longer has them. An attached tab stays to show why its shell
    /// ended, unless it ended before the tab reached it, and a tab whose
    /// shell couldn't be held is this Mac's own.
    fn reconcile(&mut self, listed: HeldShells, cx: &mut Context<Self>) {
        let Some(mut shells) = listed.shells else {
            // A server that can't hold shells sends no list to wait for.
            if !listed.holds && self.pending.take().is_some() && self.tabs.is_empty() {
                self.add(cx);
            }
            return;
        };
        // The server's shells open in the order this Mac left them, and
        // shells it hasn't shown follow in the order they started.
        shells.sort_by_key(|shell| (shell.started, shell.terminal));
        let first = self.pending.take();
        if first.is_some() {
            let order = std::mem::take(&mut self.restored_order);
            shells.sort_by_key(|shell| {
                order
                    .iter()
                    .position(|terminal| *terminal == shell.terminal)
                    .unwrap_or(usize::MAX)
            });
        }
        self.send_names(cx);
        self.ended
            .retain(|ended| shells.iter().any(|shell| shell.terminal == *ended));
        let selected = self.tabs.get(self.active).map(|tab| tab.id);
        let count = self.tabs.len();
        self.tabs.retain(|tab| {
            let view = tab.view.read(cx);
            !view.ended_unseen()
                && view.held().is_none_or(|held| {
                    view.attached() || shells.iter().any(|shell| shell.terminal == held)
                })
        });
        let mut changed = self.tabs.len() != count;
        if changed {
            if let Some(rename) = &self.rename
                && self.tabs.iter().all(|tab| tab.id != rename.id)
            {
                self.rename = None;
            }
            self.active = selected
                .and_then(|selected| self.tabs.iter().position(|tab| tab.id == selected))
                .unwrap_or(self.active)
                .min(self.tabs.len().saturating_sub(1));
        }
        for shell in &shells {
            let known = self.ended.contains(&shell.terminal)
                || self
                    .tabs
                    .iter()
                    .any(|tab| Self::terminal(tab, cx) == Some(shell.terminal));
            if !known && self.tabs.len() < MAX_TERMINALS {
                // Another Mac's new shell joins the end without taking the
                // selection, and attaches when it is first shown.
                let (empty, active) = (self.tabs.is_empty(), self.active);
                self.add_held(shell.terminal, cx);
                if let Some(report) = self.restored_seen.remove(&shell.terminal) {
                    self.tabs[self.active]
                        .view
                        .update(cx, |view, _| view.remember_seen(report));
                }
                if !empty {
                    self.active = active;
                }
                changed = true;
            }
        }
        if let Some(selected) = first {
            // Finishes seen on shells that are gone are forgotten.
            self.restored_seen.clear();
            if self.tabs.is_empty() {
                self.add(cx);
            } else if count == 0 {
                // What this Mac had selected, or else the newest shell.
                self.active = selected
                    .and_then(|selected| {
                        self.tabs
                            .iter()
                            .position(|tab| tab.view.read(cx).held() == Some(selected))
                    })
                    .unwrap_or(self.tabs.len() - 1);
            }
            changed = true;
        }
        for tab in &mut self.tabs {
            let Some(held) = tab.view.read(cx).held() else {
                continue;
            };
            let Some(shell) = shells.iter().find(|shell| shell.terminal == held) else {
                continue;
            };
            // A tab attaches only when first shown; until then the list has its title.
            changed |= tab.view.update(cx, |view, _| {
                let title = view.show_listed_title(shell.title.as_deref());
                view.show_listed_agent(shell.agent.as_deref()) || title
            });
            let name = shell.name.as_deref().and_then(custom_name);
            let settled = tab.unsent_name.is_none()
                && (name == tab.custom
                    || tab
                        .named_at
                        .is_none_or(|named| named.elapsed() > RENAME_SETTLE));
            if settled {
                tab.named_at = None;
                changed |= tab.custom != name;
                tab.custom = name;
            }
            let mut elsewhere: Vec<String> = shell
                .attached
                .iter()
                .filter(|mac| mac.id != listed.desktop)
                .map(|mac| mac.name.clone())
                .collect();
            elsewhere.sort();
            elsewhere.dedup();
            changed |= tab.elsewhere != elsewhere;
            tab.elsewhere = elsewhere;
        }
        if changed {
            self.save(cx);
            cx.notify();
        }
    }

    /// The server's held shell this Mac has selected, and the order of its tabs.
    fn saved_tabs(&self, cx: &App) -> SavedTerminals {
        SavedTerminals {
            selected: self
                .tabs
                .get(self.active)
                .and_then(|tab| tab.view.read(cx).held()),
            order: self
                .tabs
                .iter()
                .filter_map(|tab| tab.view.read(cx).held())
                .collect(),
            seen: self
                .tabs
                .iter()
                .filter_map(|tab| {
                    let view = tab.view.read(cx);
                    Some((view.held()?, view.seen_finish()?))
                })
                .collect(),
        }
    }

    /// Starts dragging tab `id`, which selects it as a press on it would.
    fn start_drag(&mut self, id: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.finish_rename(true, false, window, cx);
        let Some(index) = self.tabs.iter().position(|tab| tab.id == id) else {
            return;
        };
        self.dragging = Some(id);
        self.active = index;
        self.tabs[index]
            .view
            .read(cx)
            .focus
            .clone()
            .focus(window, cx);
        cx.notify();
    }

    /// Follows the pointer across the tab strip, at `strip`, while a tab is
    /// dragged: the tab takes the place under it, and the strip scrolls while
    /// the pointer is held near either edge.
    fn drag_over_strip(&mut self, pointer: Pixels, strip: Bounds<Pixels>, cx: &mut Context<Self>) {
        self.drag_pointer = Some((pointer, strip));
        self.reorder_dragged(cx);
        if self._drag_scroll.is_none()
            && crate::project_workspace::drag_scroll_step(pointer, strip) != px(0.)
        {
            self._drag_scroll = Some(cx.spawn(async move |this, cx| {
                loop {
                    cx.background_executor()
                        .timer(Duration::from_millis(16))
                        .await;
                    let scrolling = this.update(cx, |this, cx| this.drag_scroll(cx));
                    if !matches!(scrolling, Ok(true)) {
                        break;
                    }
                }
            }));
        }
    }

    /// One step of scrolling the strip under a dragged tab. False once the
    /// drag ends or the pointer leaves the edges.
    fn drag_scroll(&mut self, cx: &mut Context<Self>) -> bool {
        let step = self
            .drag_pointer
            .filter(|_| self.dragging.is_some() && cx.has_active_drag())
            .map(|(pointer, strip)| crate::project_workspace::drag_scroll_step(pointer, strip))
            .unwrap_or(px(0.));
        if step == px(0.) {
            self._drag_scroll = None;
            return false;
        }
        if crate::project_workspace::scroll_strip(&self.tab_scroll, step) {
            self.reorder_dragged(cx);
            cx.notify();
        }
        true
    }

    /// Moves the dragged tab to the place under the pointer.
    fn reorder_dragged(&mut self, cx: &mut Context<Self>) {
        let (Some(id), Some((pointer, _))) = (self.dragging, self.drag_pointer) else {
            return;
        };
        let Some(from) = self.tabs.iter().position(|tab| tab.id == id) else {
            return;
        };
        let middles = crate::project_workspace::tab_middles(&self.tab_scroll, 0, self.tabs.len());
        if let Some(to) = crate::project_workspace::dragged_tab_target(from, &middles, pointer) {
            self.move_tab(id, to, cx);
        }
    }

    /// Moves tab `id` to `index`, keeping the selection on the same tab.
    fn move_tab(&mut self, id: usize, index: usize, cx: &mut Context<Self>) {
        let Some(from) = self.tabs.iter().position(|tab| tab.id == id) else {
            return;
        };
        if from == index || index >= self.tabs.len() {
            return;
        }
        let selected = self.tabs.get(self.active).map(|tab| tab.id);
        let tab = self.tabs.remove(from);
        self.tabs.insert(index, tab);
        if let Some(selected) = selected {
            self.active = self
                .tabs
                .iter()
                .position(|tab| tab.id == selected)
                .unwrap_or(self.active);
        }
        self.save(cx);
        cx.notify();
    }

    /// Writes this server's tabs when they changed.
    fn save(&mut self, cx: &mut Context<Self>) {
        if !self.holdable {
            return;
        }
        let saved = self.saved_tabs(cx);
        if self.saved.as_ref() != Some(&saved) {
            if let Some(saver) = cx.try_global::<TabsSaver>() {
                saver.0.terminals(self.server.clone(), saved.clone());
            }
            self.saved = Some(saved);
        }
    }

    pub(super) fn for_project(
        server: String,
        connection: Option<configuration::ConnectionScope>,
        root: String,
        cx: &mut Context<Self>,
    ) -> Result<Self, String> {
        CommandDraft::project_directory(&root)?;
        let mut workspace = Self::empty(server, connection, root.clone(), cx);
        workspace.project_root = Some(root);
        workspace.add(cx);
        Ok(workspace)
    }

    /// A first handover is the initial tab, not an extra tab beside a blank shell.
    pub(super) fn with_command(
        server: String,
        connection: Option<configuration::ConnectionScope>,
        endpoint: String,
        draft: CommandDraft,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut workspace = Self::empty(server, connection, endpoint, cx);
        workspace.hold(cx);
        workspace.add_command(draft, "Service command", cx);
        workspace
    }

    pub(super) fn with_shell(
        server: String,
        connection: Option<configuration::ConnectionScope>,
        endpoint: String,
        draft: CommandDraft,
        title: String,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut workspace = Self::empty(server, connection, endpoint, cx);
        workspace.hold(cx);
        workspace.add_shell(draft, title, cx);
        workspace
    }

    pub(super) fn open_shell(
        &mut self,
        draft: CommandDraft,
        title: String,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        if self.tabs.len() >= MAX_TERMINALS {
            return Err("Close a terminal tab before opening this container shell.".into());
        }
        self.add_shell(draft, title, cx);
        Ok(())
    }

    /// The selected tab, which someone just opened here.
    fn opened(&mut self, cx: &mut Context<Self>) {
        if let Some(tab) = self.tabs.get(self.active) {
            tab.view.update(cx, |view, _| view.opened());
        }
    }

    fn add_shell(&mut self, draft: CommandDraft, title: String, cx: &mut Context<Self>) {
        self.add(cx);
        self.opened(cx);
        let tab = &mut self.tabs[self.active];
        tab.custom = Some(custom_name(&title).unwrap_or_else(|| "Container shell".into()));
        tab.unsent_name = Some(tab.custom.clone());
        assert!(tab.view.update(cx, |view, _| view.prepare_command(draft)));
        cx.notify();
    }

    /// `server`'s workspace of the one tab `view`, which the test made.
    #[cfg(test)]
    pub(crate) fn showing(
        server: String,
        view: Entity<TerminalView>,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut workspace = Self::empty(server, None, "fixture@invalid".into(), cx);
        workspace.push(view, cx);
        workspace
    }

    fn empty(
        server: String,
        connection: Option<configuration::ConnectionScope>,
        endpoint: String,
        cx: &mut Context<Self>,
    ) -> Self {
        crate::connectivity::observe(cx, |this: &Self| &this.server, |_, cx| cx.notify()).detach();
        Self {
            endpoint,
            project_root: None,
            immersive: false,
            server,
            connection,
            tabs: vec![],
            active: 0,
            next: 0,
            rename: None,
            context_tab: None,
            dragging: None,
            drag_pointer: None,
            _drag_scroll: None,
            tab_scroll: ScrollHandle::new(),
            tab_spare: Default::default(),
            revealed_tab: None,
            holdable: false,
            saved: None,
            pending: None,
            restored_order: Vec::new(),
            restored_seen: Default::default(),
            ended: Vec::new(),
            closing: Vec::new(),
            _list: None,
        }
    }

    fn add(&mut self, cx: &mut Context<Self>) {
        let draft = self.project_root.as_ref().map(|root| {
            CommandDraft::project_directory(root).expect("validated project directory")
        });
        self.add_tab(draft, cx);
    }

    /// A new tab, with `draft` delivered once its shell is ready.
    fn add_tab(&mut self, draft: Option<CommandDraft>, cx: &mut Context<Self>) {
        if self.tabs.len() >= MAX_TERMINALS {
            return;
        }
        let view = cx.new(|cx| TerminalView::new(self.server.clone(), cx));
        let holdable = self.holdable;
        view.update(cx, |view, _| view.set_holdable(holdable));
        if let Some(draft) = draft {
            // A fresh view always takes its one draft.
            assert!(view.update(cx, |view, _| view.prepare_command(draft)));
        }
        self.push(view, cx);
    }

    fn push(&mut self, view: Entity<TerminalView>, cx: &mut Context<Self>) {
        // Output, selection and pointer changes repaint the terminal itself.
        // Only presentation changes need to rebuild the surrounding tab bar.
        let id = self.next;
        let mut previous = view.read(cx).tab_presentation();
        let observer = cx.observe(&view, move |this, view, cx| {
            // A tab's shell can become held, or end, while the tab is hidden.
            this.save(cx);
            let next = view.read(cx).tab_presentation();
            if next != previous {
                if next.1.phase != previous.1.phase
                    && let Some(tab) = this.tabs.iter_mut().find(|tab| tab.id == id)
                {
                    tab.exit.observe(&next.1.phase);
                }
                previous = next;
                cx.notify();
            }
        });
        let links = cx.subscribe(
            &view,
            |_, view, event: &crate::terminal_view::OpenLink, cx| {
                view.update(cx, |view, cx| view.open_link(event.0.clone(), cx));
            },
        );
        let number = free_number(self.tabs.iter().map(|tab| tab.number));
        self.tabs.push(TerminalTab {
            id,
            number,
            focus: cx.focus_handle(),
            custom: None,
            unsent_name: None,
            named_at: None,
            elsewhere: Vec::new(),
            view,
            exit: ExitPolicy::default(),
            _observer: observer,
            _links: links,
        });
        self.next += 1;
        self.active = self.tabs.len() - 1;
        cx.notify();
    }

    /// Handovers get their own tab so they cannot type into an existing program.
    pub(super) fn open_command(
        &mut self,
        draft: CommandDraft,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        if self.tabs.len() >= MAX_TERMINALS {
            return Err("Close a terminal tab before opening this service command.".into());
        }
        self.add_command(draft, "Service command", cx);
        Ok(())
    }

    /// A handover named for what it does, such as setting up a language server.
    pub(super) fn open_named_command(
        &mut self,
        draft: CommandDraft,
        title: &str,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        if self.tabs.len() >= MAX_TERMINALS {
            return Err("Close a terminal tab before opening this command.".into());
        }
        self.add_command(draft, title, cx);
        Ok(())
    }

    /// The handover replaces a project terminal's change of directory, so a
    /// command that needs the project folder changes into it itself.
    fn add_command(&mut self, draft: CommandDraft, title: &str, cx: &mut Context<Self>) {
        self.add_tab(Some(draft), cx);
        self.opened(cx);
        let tab = &mut self.tabs[self.active];
        tab.custom = Some(title.into());
        tab.unsent_name = Some(tab.custom.clone());
        cx.notify();
    }

    /// Commands that quitting Newport would end.
    pub fn running_commands(&self, cx: &App) -> usize {
        self.tabs
            .iter()
            .filter(|tab| tab.view.read(cx).quitting_ends_command())
            .count()
    }

    pub fn session_ids(&self, cx: &App) -> Vec<uuid::Uuid> {
        self.tabs
            .iter()
            .filter_map(|tab| tab.view.read(cx).session_id())
            .collect()
    }

    /// Like Terminal and Ghostty, a tab still running a command asks first,
    /// and so does a shell open on another Mac, where closing ends it too.
    pub(super) fn close(
        &mut self,
        id: usize,
        keyboard: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(tab) = self.tabs.iter().find(|tab| tab.id == id) else {
            return;
        };
        let running = {
            let view = tab.view.read(cx);
            view.session_id().is_some() && view.running_command()
        };
        let description = close_warning(running, &tab.elsewhere);
        let Some(description) = description.filter(|_| self.tabs.len() > 1) else {
            self.close_now(id, keyboard, window, cx);
            return;
        };
        let owner = cx.entity().downgrade();
        window.open_alert_dialog(cx, move |dialog, _, _| {
            let owner = owner.clone();
            dialog
                .title("Close this terminal?")
                .description(description.clone())
                .button_props(
                    DialogButtonProps::default()
                        .show_cancel(true)
                        .ok_text("Close")
                        .cancel_text("Cancel"),
                )
                .on_ok(move |_, window, cx| {
                    let _ = owner.update(cx, |this, cx| this.close_now(id, keyboard, window, cx));
                    true
                })
        });
    }

    fn close_now(
        &mut self,
        id: usize,
        keyboard: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // The last session remains available to reconnect.
        if self.tabs.len() <= 1 {
            return;
        }
        if let Some(tab) = self.tabs.iter().find(|tab| tab.id == id)
            && let Some(terminal) = tab.view.read(cx).held()
        {
            if self.closing.contains(&terminal) {
                return;
            }
            let Ok(server) = uuid::Uuid::parse_str(&self.server) else {
                tab.view.update(cx, |view, cx| {
                    view.input_error = Some("Invalid server identity. Reopen this server.".into());
                    cx.notify();
                });
                return;
            };
            self.closing.push(terminal);
            cx.notify();
            let close = HeldClose {
                id,
                terminal,
                connection: self.connection.clone(),
                keyboard,
            };
            let task = cx.background_spawn(async move {
                newport_client::terminal_transport::end_held(server, terminal)
            });
            cx.spawn_in(window, async move |this, cx| {
                let result = task.await;
                let _ = this.update_in(cx, |this, window, cx| {
                    this.finish_held_close(close, result, window, cx)
                });
            })
            .detach();
            return;
        }
        self.remove_closed_tab(id, keyboard, false, window, cx);
    }

    fn finish_held_close(
        &mut self,
        close: HeldClose,
        result: Result<(), newport_client::request_error::RequestError>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.closing.retain(|terminal| *terminal != close.terminal);
        cx.notify();
        if self.connection != close.connection {
            return;
        }
        let Some(tab) = self.tabs.iter().find(|tab| tab.id == close.id) else {
            return;
        };
        // Exit may clear held() before this acknowledgement reaches the UI;
        // the attached session still names the same terminal. A replacement
        // session has another id and must not be closed by this old result.
        if Self::terminal(tab, cx) != Some(close.terminal) {
            return;
        }
        match result {
            Ok(()) => {
                if !self.ended.contains(&close.terminal) {
                    self.ended.push(close.terminal);
                }
                self.remove_closed_tab(close.id, close.keyboard, true, window, cx);
            }
            Err(error) => {
                // A refused or uncertain End leaves the shell and its tab visible.
                tab.view.update(cx, |view, cx| {
                    view.input_error = Some(error.message);
                    cx.notify();
                });
                cx.notify();
            }
        }
    }

    fn remove_closed_tab(
        &mut self,
        id: usize,
        keyboard: bool,
        ended_remotely: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.tabs.len() <= 1 {
            if ended_remotely && let Some(tab) = self.tabs.iter().find(|tab| tab.id == id) {
                if let Some(held) = tab.view.read(cx).held()
                    && !self.ended.contains(&held)
                {
                    self.ended.push(held);
                }
                tab.view.update(cx, |view, cx| {
                    view.ended_remotely();
                    cx.notify();
                });
                cx.notify();
            }
            return;
        }
        if let Some(index) = self.tabs.iter().position(|tab| tab.id == id) {
            if self.rename.as_ref().is_some_and(|rename| rename.id == id) {
                self.rename = None;
            }
            // Closing a tab ends its shell, held or not: on every Mac.
            if let Some(held) = self.tabs[index].view.read(cx).held()
                && !self.ended.contains(&held)
            {
                self.ended.push(held);
            }
            self.tabs[index].view.update(cx, |view, _| {
                if ended_remotely {
                    view.close();
                } else {
                    view.end();
                }
            });
            self.tabs.remove(index);
            self.active = selected_after_close(self.active, index, self.tabs.len());
            if let Some(tab) = self.tabs.get(self.active) {
                let focus = if keyboard {
                    tab.focus.clone()
                } else {
                    tab.view.read(cx).focus.clone()
                };
                focus.focus(window, cx);
            }
            cx.notify();
        }
    }

    fn close_exited_tabs(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let mut restore_focus = false;
        // Exit is final. Failures, explicit disconnects and startup exits
        // retain their tabs. Walk backwards so each surviving session keeps
        // its identity and selection.
        for index in (0..self.tabs.len()).rev() {
            let tab = &self.tabs[index];
            if !tab.exit.should_close(&tab.view.read(cx).status.phase) {
                continue;
            }
            restore_focus |= tab.focus.contains_focused(window, cx)
                || tab.view.read(cx).focus.contains_focused(window, cx);
            if self
                .rename
                .as_ref()
                .is_some_and(|rename| rename.id == tab.id)
            {
                restore_focus |= self
                    .rename
                    .as_ref()
                    .unwrap()
                    .input
                    .read(cx)
                    .focus_handle(cx)
                    .contains_focused(window, cx);
                self.rename = None;
            }
            if self.context_tab == Some(tab.id) {
                self.context_tab = None;
            }
            tab.view.update(cx, |view, _| view.close());
            self.tabs.remove(index);
            self.active = selected_after_close(self.active, index, self.tabs.len());
        }
        if restore_focus {
            self.focus_active(window, cx);
        }
    }

    fn tab_key(
        &mut self,
        id: usize,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let key = &event.keystroke;
        if window.has_active_dialog(cx)
            || key.modifiers.platform
            || key.modifiers.control
            || key.modifiers.alt
            || key.modifiers.shift
        {
            return;
        }
        let Some(index) = self.tabs.iter().position(|tab| tab.id == id) else {
            return;
        };
        let tab = &self.tabs[index];
        if !tab.focus.is_focused(window) {
            return;
        }
        let Some(next) = tab_destination(index, &key.key, self.tabs.len()) else {
            return;
        };
        self.active = next;
        self.tabs[next].focus.focus(window, cx);
        window.prevent_default();
        cx.stop_propagation();
        cx.notify();
    }
}

// Only shorten the recognizable user@host:path prompt format. Command titles
// and explicitly named tabs keep their meaning; the full OSC title stays available.
/// The lowest tab number not in `taken`.
fn free_number(taken: impl Iterator<Item = usize> + Clone) -> usize {
    (0..)
        .find(|number| taken.clone().all(|taken| taken != *number))
        .expect("a free number")
}

fn automatic_tab_title(title: &str, number: usize) -> String {
    let title = title.trim();
    let prompt_path = title.split_once(':').filter(|(host, path)| {
        host.contains('@') && (path.starts_with('/') || path.starts_with('~'))
    });
    let base = if let Some((host, path)) = prompt_path {
        path.trim_end_matches('/')
            .rsplit('/')
            .next()
            .filter(|name| !name.is_empty() && *name != "~")
            .unwrap_or_else(|| host.rsplit('@').next().unwrap_or(host))
    } else if title.is_empty() || title == "Terminal" {
        "Terminal"
    } else {
        return title.to_owned();
    };
    if number == 0 {
        base.to_owned()
    } else {
        format!("{base} {}", number + 1)
    }
}

fn session_tab(id: usize, active: bool, cx: &App) -> crate::project_workspace::DocumentTab {
    crate::project_workspace::document_tab(
        ("terminal-tab", id),
        active,
        Some(
            Icon::new(IconName::Terminal)
                .size(px(16.))
                .flex_shrink_0()
                .into_any_element(),
        ),
        cx,
    )
}

/// The mark before a tab's title while an agent in it works, waits for the
/// person, has finished unseen or stopped with an error.
fn agent_mark(id: usize, mark: crate::terminal_agent::AgentMark, cx: &App) -> AnyElement {
    use crate::terminal_agent::AgentMark;
    let dark = cx.theme().is_dark();
    let mark_element = match mark {
        AgentMark::Working => gpui_kit::component::spinner::Spinner::new()
            .with_size(px(crate::ui::DOT_SIZE + 2.))
            .color(cx.theme().muted_foreground)
            .into_any_element(),
        AgentMark::NeedsYou => {
            crate::controls::status_dot(Some(crate::theme::warning(dark)), cx).into_any_element()
        }
        AgentMark::Done => {
            crate::controls::status_dot(Some(crate::theme::success(dark)), cx).into_any_element()
        }
        AgentMark::Failed => {
            crate::controls::status_dot(Some(crate::theme::error(dark)), cx).into_any_element()
        }
    };
    div()
        .debug_selector(move || format!("terminal-tab-{id}-agent-{mark:?}"))
        .flex()
        .flex_shrink_0()
        .items_center()
        .child(mark_element)
        .into_any_element()
}

/// What Close warns about, if anything: a running command, and other Macs
/// the shell is open on, where it ends too.
fn close_warning(running: bool, elsewhere: &[String]) -> Option<String> {
    let macs = list_names(elsewhere);
    match (running, macs) {
        (false, None) => None,
        (true, None) => {
            Some("A command is still running in it. Closing the terminal ends it.".into())
        }
        (false, Some(macs)) => Some(format!(
            "It is also open on {macs}. Closing the terminal ends it there too."
        )),
        (true, Some(macs)) => Some(format!(
            "A command is still running in it, and it is also open on {macs}. Closing the terminal ends it there too."
        )),
    }
}

/// "Air", "Air and Studio", "Air, Mini and Studio".
fn list_names(names: &[String]) -> Option<String> {
    match names {
        [] => None,
        [only] => Some(only.clone()),
        [rest @ .., last] => Some(format!("{} and {last}", rest.join(", "))),
    }
}

fn custom_name(value: &str) -> Option<String> {
    let name: String = value
        .trim()
        .chars()
        .filter(|c| !c.is_control())
        .take(60)
        .collect();
    (!name.is_empty()).then_some(name)
}

fn tab_destination(index: usize, key: &str, count: usize) -> Option<usize> {
    if count == 0 || index >= count {
        return None;
    }
    match key {
        "left" => Some((index + count - 1) % count),
        "right" => Some((index + 1) % count),
        "home" => Some(0),
        "end" => Some(count - 1),
        "enter" | "space" => Some(index),
        _ => None,
    }
}

fn selected_after_close(active: usize, removed: usize, remaining: usize) -> usize {
    if removed < active {
        active - 1
    } else {
        active.min(remaining.saturating_sub(1))
    }
}

impl Render for Workspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.close_exited_tabs(window, cx);
        self.save(cx);
        if !cx.has_active_drag() {
            self.dragging = None;
            self.drag_pointer = None;
        }
        for (index, tab) in self.tabs.iter().enumerate() {
            if index != self.active {
                tab.view.update(cx, |view, cx| view.deactivate(window, cx));
            }
        }
        let current = self.tabs.get(self.active).map(|tab| tab.view.clone());
        let state = current.as_ref().map(|view| view.read(cx).status.clone());
        let transport_wait = crate::connectivity::for_scope(self.connection.as_ref(), cx)
            .filter(|connection| connection.phase != crate::connectivity::Phase::Connected)
            .filter(|_| {
                state
                    .as_ref()
                    .is_some_and(|state| state.connected() || state.cancel_label().is_some())
            })
            .cloned();
        let error = transport_wait
            .as_ref()
            .map(|connection| connection.error.clone())
            .unwrap_or_else(|| {
                current.as_ref().and_then(|view| {
                    let view = view.read(cx);
                    view.status
                        .error()
                        .map(str::to_owned)
                        .or_else(|| view.input_error.clone())
                })
            });
        let not_held = current
            .as_ref()
            .and_then(|view| view.read(cx).not_held.clone())
            .filter(|_| error.is_none());
        let reveal = cx.entity().downgrade();
        // The most a tab may take: the strip and the row's free space after it.
        let tab_width = self.tab_scroll.bounds().size.width + self.tab_spare.get();
        let tab_spare = self.tab_spare.clone();
        let menu_view = cx.entity().downgrade();
        // Only windowed immersive mode shares this row with the native Mac
        // window controls. Fullscreen hides those controls.
        let inset = self.immersive && cfg!(target_os = "macos") && !window.is_fullscreen();
        let tab_row = div()
            .debug_selector(|| "terminal-tab-row".into())
            .flex()
            .h(px(crate::ui::HEADER_HEIGHT))
            .flex_shrink_0()
            .px_0()
            .when(inset, |bar| bar.pl(px(96.)))
            .items_center()
            .relative()
            .bg(cx.theme().muted)
            .child(crate::ui::header_rule(cx))
            .capture_any_mouse_down(cx.listener(|this, event: &MouseDownEvent, _, _| {
                if event.button == MouseButton::Right {
                    this.context_tab = None;
                }
            }))
            .child(
                div()
                    .id("terminal-tabs")
                    .debug_selector(|| "terminal-tab-bar".into())
                    .role(Role::TabList)
                    .aria_label("Terminal sessions")
                    .flex()
                    .min_w_0()
                    .h_full()
                    .overflow_x_scroll()
                    .track_scroll(&self.tab_scroll)
                    .on_drag_move(cx.listener(
                        |this, event: &DragMoveEvent<crate::project_workspace::DraggedTab<usize>>, _, cx| {
                            if event.drag(cx).strip == cx.entity_id() {
                                this.drag_over_strip(event.event.position.x, event.bounds, cx);
                            }
                        },
                    ))
                    .children(self.tabs.iter().enumerate().map(|(index, tab)| {
                        let id = tab.id;
                        let number = tab.number;
                        if let Some(rename) =
                            self.rename.as_ref().filter(|rename| rename.id == id)
                        {
                            return session_tab(id, index == self.active, cx)
                                .when(tab_width > px(0.), |tab| tab.max_w(tab_width.min(px(260.))))
                                .unfocusable()
                                .when(index == 0 && inset, |tab| tab.border_l_1())
                                .set_position(index + 1, self.tabs.len())
                                .accessibility_label(format!("Rename Terminal {}", number + 1))
                                .debug_selector(move || format!("terminal-tab-{id}"))
                                .child(
                                    Styled::h(crate::controls::input(&rename.input), px(26.))
                                        .min_h(px(26.))
                                        .max_h(px(26.))
                                        .w(px(132.))
                                        .px(px(8.))
                                        .py_0()
                                        .appearance(false)
                                        .bordered(false)
                                        .aria_label(format!("Rename Terminal {}", number + 1)),
                                )
                                .on_action(cx.listener(
                                    |this,
                                     _: &gpui_kit::component::input::Escape,
                                     window,
                                     cx| {
                                        this.finish_rename(false, true, window, cx);
                                    },
                                ));
                        }
                        let remote_title = tab.view.read(cx).title.clone();
                        let title = tab.custom.clone()
                            .unwrap_or_else(|| automatic_tab_title(&remote_title, number));
                        let startup_exit = {
                            let status = &tab.view.read(cx).status;
                            (!tab.exit.should_close(&status.phase) && matches!(status.phase, Phase::Exited(_)))
                                .then(|| status.label())
                        };
                        let closing = tab
                            .view
                            .read(cx)
                            .held()
                            .is_some_and(|terminal| self.closing.contains(&terminal));
                        let elsewhere = list_names(&tab.elsewhere)
                            .map(|macs| format!("Also open on {macs}"));
                        let agent = tab.view.read(cx).agent();
                        let hint = startup_exit.clone().or_else(|| {
                            let hints: Vec<_> = agent
                                .as_ref()
                                .map(|agent| agent.hint.clone())
                                .into_iter()
                                .chain(elsewhere)
                                .collect();
                            (!hints.is_empty()).then(|| hints.join("\n"))
                        }).or_else(|| {
                            (tab.custom.is_none() && title != remote_title && remote_title.contains('@') && remote_title.contains(':'))
                                .then(|| remote_title.clone())
                        });
                        session_tab(id, index == self.active, cx)
                            .when(tab_width > px(0.), |tab| tab.max_w(tab_width.min(px(260.))))
                            // Inset beside the window controls, the first tab
                            // needs its own leading edge.
                            .when(index == 0 && inset, |tab| tab.border_l_1())
                            .set_position(index + 1, self.tabs.len())
                            .accessibility_label(match (&startup_exit, &agent) {
                                (Some(exit), _) => format!("{title}, {exit}"),
                                (None, Some(agent)) => format!("{title}, {}", agent.mark.label()),
                                (None, None) => title.clone(),
                            })
                            .when_some(hint, |tab, hint| tab.hover_hint(hint))
                            .debug_selector(move || format!("terminal-tab-{id}"))
                            .on_mouse_down(
                                MouseButton::Right,
                                cx.listener(move |this, _, _, cx| {
                                    this.context_tab = Some(id);
                                    cx.notify();
                                }),
                            )
                            .focus_handle(tab.focus.clone().tab_stop(index == self.active))
                            .on_drag(
                                crate::project_workspace::DraggedTab {
                                    strip: cx.entity_id(),
                                    id,
                                    title: title.clone().into(),
                                    icon: Some(IconName::Terminal),
                                },
                                {
                                    let owner = cx.entity().downgrade();
                                    move |tab, _, window, cx| {
                                        let _ = owner.update(cx, |this, cx| this.start_drag(tab.id, window, cx));
                                        cx.new(|_| crate::project_workspace::DraggedTabView::new(tab, window))
                                    }
                                },
                            )
                            .when(self.dragging == Some(id), |tab| tab.opacity(0.4))
                            .on_key_down(cx.listener(
                                move |this, event: &KeyDownEvent, window, cx| {
                                    this.tab_key(id, event, window, cx)
                                },
                            ))
                            .on_click(cx.listener(
                                move |this, event: &ClickEvent, window, cx| {
                                    if matches!(event, ClickEvent::Mouse(event) if event.down.click_count == 2) {
                                        this.rename(id, window, cx);
                                        return;
                                    }
                                    if this
                                        .rename
                                        .as_ref()
                                        .is_some_and(|rename| rename.id == id)
                                    {
                                        return;
                                    }
                                    // Commit before replacing the active terminal tree;
                                    // its focus transition can remove the old input's path.
                                    this.finish_rename(true, false, window, cx);
                                    if let Some(index) =
                                        this.tabs.iter().position(|tab| tab.id == id)
                                    {
                                        let tab = &this.tabs[index];
                                        this.active = index;
                                        let focus =
                                            if matches!(event, ClickEvent::Keyboard(_)) {
                                                tab.focus.clone()
                                            } else {
                                                tab.view.read(cx).focus.clone()
                                            };
                                        focus.focus(window, cx);
                                        cx.notify();
                                    }
                                },
                            ))
                            .when_some(agent.map(|agent| agent.mark), |tab, mark| {
                                tab.child(agent_mark(id, mark, cx))
                            })
                            .child(crate::project_workspace::document_tab_title(title.clone()))
                            .when_some(startup_exit, |tab, exit| {
                                tab.child(
                                    div()
                                        .flex_shrink_0()
                                        .text_color(cx.theme().muted_foreground)
                                        .child(exit),
                                )
                            })
                            .when(self.tabs.len() > 1, |tab| {
                                let owner = cx.entity().downgrade();
                                tab.on_close(("close-terminal", id), format!("Close {title}"), true, move |keyboard, window, cx| {
                                    let _ = owner.update(cx, |this, cx| this.close(id, keyboard, window, cx));
                                })
                                .close_pending(closing)
                            })
                    }))
                    .on_prepaint(move |_, window, cx| {
                        let Some(workspace) = reveal.upgrade() else {
                            return;
                        };
                        let state = workspace.read(cx);
                        let Some(selected) = state.tabs.get(state.active) else {
                            return;
                        };
                        let Some(tab) = state.tab_scroll.bounds_for_item(state.active) else {
                            return;
                        };
                        let viewport = state.tab_scroll.bounds();
                        // Track unscrolled geometry so manual scrolling stays put.
                        let geometry = (
                            selected.id,
                            tab.left() - viewport.left(),
                            tab.size.width,
                            viewport.size.width,
                        );
                        if state.revealed_tab != Some(geometry) {
                            window.defer(cx, move |window, cx| {
                                workspace.update(cx, |this, _| {
                                    if this.tabs.get(this.active).is_some_and(|tab| tab.id == geometry.0) {
                                        this.revealed_tab = Some(geometry);
                                        this.tab_scroll.scroll_to_item(this.active);
                                        window.refresh();
                                    }
                                });
                            });
                        }
                    }),
            )
            .child(
                crate::controls::icon_button("new-terminal")
                    .icon(Icon::new(IconName::Plus).size(px(16.)).text_color(cx.theme().muted_foreground))
                    .ghost()
                    .w(px(crate::ui::HEADER_HEIGHT)).min_w(px(crate::ui::HEADER_HEIGHT)).max_w(px(crate::ui::HEADER_HEIGHT))
                    .h(px(crate::ui::HEADER_HEIGHT)).min_h(px(crate::ui::HEADER_HEIGHT)).max_h(px(crate::ui::HEADER_HEIGHT))
                    .rounded_none()
                    .border_b_1().border_color(cx.theme().border)
                    .accessibility_label("New terminal")
                    .tooltip("New terminal")
                    .disabled(self.tabs.len() >= MAX_TERMINALS)
                    .on_click(cx.listener(|this, _, window, cx| this.new_tab(window, cx))),
            )
            .child(
                // Native title-bar hit testing runs before button clicks on
                // Windows. Only empty space may drag the window; marking the
                // whole toolbar also captures the restore and tab controls.
                div()
                    .debug_selector(|| "terminal-drag-region".into())
                    .flex_1()
                    .h_full()
                    .child(
                        canvas(
                            move |bounds, window, _| {
                                if tab_spare.replace(bounds.size.width) != bounds.size.width {
                                    window.request_animation_frame();
                                }
                            },
                            |_, _, _, _| {},
                        )
                        .size_full(),
                    )
                    .when(self.immersive, |spacer| {
                        spacer.window_control_area(WindowControlArea::Drag)
                    }),
            )
            .when_some(state.as_ref(), |bar, state| {
                bar.child(
                    div()
                        .id("terminal-state")
                        .role(Role::Status)
                        .aria_label(transport_wait.as_ref().map(|connection| connection.phase.label().to_owned()).unwrap_or_else(|| state.label()))
                        .text_size(px(crate::ui::FONT_SIZE))
                        .text_color(if error.is_some() {
                            cx.theme().danger
                        } else {
                            cx.theme().muted_foreground
                        })
                        .when(transport_wait.is_none() && (state.steady() || state.cancel_label().is_some()), |status| {
                            status.w_0().h_0().overflow_hidden()
                        })
                        .child(transport_wait.as_ref().map(|connection| connection.phase.label().to_owned()).unwrap_or_else(|| state.label())),
                )
            })
            .when(self.project_root.is_none(), |bar| bar.child(
                crate::controls::icon_button("terminal-immersive")
                    .icon(Icon::new(if self.immersive {
                        gpui_kit::assets::IconName::Minimize2
                    } else {
                        gpui_kit::assets::IconName::Maximize2
                    }).size(px(crate::controls::ICON_SIZE)).text_color(cx.theme().muted_foreground))
                    .ghost()
                    .accessibility_label(if self.immersive {
                        "Exit immersive mode"
                    } else {
                        "Enter immersive mode"
                    })
                    .tooltip(if self.immersive {
                        "Exit immersive mode"
                    } else {
                        "Enter immersive mode"
                    })
                    .on_click(|_, window, cx| {
                        window.dispatch_action(Box::new(ToggleTerminalImmersive), cx);
                    }),
            ))
            .context_menu(move |menu, _, cx| {
                let id = menu_view
                    .upgrade()
                    .and_then(|view| view.read(cx).context_tab);
                if let Some(id) = id {
                    crate::terminal_session_menu::menu_for_tab(
                        menu,
                        menu_view.clone(),
                        id,
                        cx,
                    )
                } else {
                    crate::terminal_session_menu::menu(menu, menu_view.clone(), cx)
                }
            });
        // In immersive mode the tab row is the title bar.
        #[cfg(target_os = "windows")]
        let tab_row = tab_row.children(
            self.immersive
                .then(|| crate::window_controls::caption_buttons(window, cx))
                .flatten(),
        );
        div()
            .size_full()
            .relative()
            .min_w_0()
            .min_h_0()
            .flex()
            .flex_col()
            .on_action(
                cx.listener(|this, _: &crate::project_editor::MoveTabLeft, window, cx| {
                    if !window.has_active_dialog(cx) {
                        this.move_active_tab(false, cx);
                    }
                }),
            )
            .on_action(cx.listener(
                |this, _: &crate::project_editor::MoveTabRight, window, cx| {
                    if !window.has_active_dialog(cx) {
                        this.move_active_tab(true, cx);
                    }
                },
            ))
            .child(tab_row)
            .when_some(error, |root, error| {
                root.child(
                    crate::controls::alert("terminal-error", error, cx)
                        .px(px(12.))
                        .py(px(8.)),
                )
            })
            .when_some(not_held, |root, reason| {
                root.child(crate::controls::warning_banner(
                    "terminal-not-held",
                    NOT_HELD_TITLE,
                    not_held_message(&reason),
                    cx,
                ))
            })
            .when_some(current.clone(), |root, current| {
                root.child(div().relative().flex_1().min_h_0().child(current).when(
                    state.as_ref().is_some_and(|state| !state.started),
                    |frame| {
                        let connecting = state
                            .as_ref()
                            .is_some_and(|state| state.cancel_label().is_some());
                        let failed = state.as_ref().is_some_and(|state| state.error().is_some());
                        let gone = state
                            .as_ref()
                            .filter(|state| matches!(state.phase, Phase::Exited(_) | Phase::Ended));
                        let title: SharedString = if connecting {
                            "Connecting…".into()
                        } else if failed {
                            "Connection failed".into()
                        } else if let Some(state) = gone {
                            state.label().into()
                        } else {
                            "Open a remote shell".into()
                        };
                        frame.child(
                            // The endpoint reads as the description: it is
                            // the shell this tab is about to open.
                            empty::pane(IconName::Terminal, title, self.endpoint.clone(), cx)
                                .absolute()
                                .inset_0()
                                .bg(cx.theme().background),
                        )
                    },
                ))
            })
            .when(current.is_none(), |root| {
                let (title, description) = if self.pending.is_some() {
                    ("Loading terminals", "Open a new one with +.")
                } else {
                    ("No terminals open", "Open a terminal with +.")
                };
                root.child(empty::pane(IconName::Terminal, title, description, cx))
            })
    }
}

const NOT_HELD_TITLE: &str = "This shell won't keep running";

/// Why a tab's ordinary shell ends with the connection, with Needle's reason.
fn not_held_message(reason: &str) -> String {
    format!(
        "It ends when the connection drops or you quit Newport. Needle couldn't keep it on the server: {reason}"
    )
}

#[cfg(test)]
mod tests {
    use super::{
        CommandDraft, HeldClose, HeldShells, MAX_TERMINALS, Workspace, close_warning, custom_name,
        selected_after_close, tab_destination,
    };
    use crate::terminal_view::TerminalView;
    use gpui_kit::{
        self as gpui, App, AppContext, Entity, Modifiers, MouseButton, MouseDownEvent,
        TestAppContext, VisualTestContext, px, size,
    };
    use serde_json::json;

    fn init(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui::component::init(cx);
            crate::theme::install(false, cx);
        });
    }

    fn mac(id: &str, name: &str) -> newport_domain::terminal::Mac {
        newport_domain::terminal::Mac {
            id: id.into(),
            name: name.into(),
        }
    }

    /// A shell another Mac started, attached from the named Macs.
    fn shell(
        terminal: uuid::Uuid,
        name: Option<&str>,
        attached: &[(&str, &str)],
    ) -> newport_domain::terminal::HeldShell {
        newport_domain::terminal::HeldShell {
            terminal,
            name: name.map(str::to_owned),
            title: None,
            agent: None,
            creator: mac("studio", "Studio"),
            attached: attached
                .iter()
                .map(|(id, name)| newport_domain::terminal::AttachedMac {
                    id: (*id).into(),
                    name: (*name).into(),
                    active: false,
                })
                .collect(),
            started: 1,
            exited: false,
        }
    }

    fn listed(shells: Vec<newport_domain::terminal::HeldShell>) -> HeldShells {
        HeldShells {
            desktop: "this".into(),
            holds: true,
            shells: Some(shells),
        }
    }

    #[gpui::test]
    fn a_restored_tab_whose_shell_ended_meanwhile_goes_and_a_live_one_stays(
        cx: &mut TestAppContext,
    ) {
        use newport_gpui::terminal_transport::Event;
        init(cx);
        cx.update(|cx| {
            let (live, gone) = (uuid::Uuid::new_v4(), uuid::Uuid::new_v4());
            let workspace = cx.new(|cx| {
                let mut workspace = Workspace::new(
                    "server".into(),
                    Some(crate::snapshot_fixture::scope("fixture")),
                    "e".into(),
                    cx,
                );
                workspace.tabs[0]
                    .view
                    .update(cx, |view, _| view.attach_to(live));
                workspace.add_held(gone, cx);
                workspace
            });
            workspace.update(cx, |workspace, cx| {
                // The first tab was showing its shell when it ended; the
                // second's shell was closed on another Mac while Newport was
                // closed here.
                workspace.tabs[0].view.update(cx, |view, _| {
                    view.receive_for_test(Event::Persistent);
                    view.receive_for_test(Event::Ready);
                    view.receive_for_test(Event::Ended);
                });
                workspace.tabs[1].view.update(cx, |view, _| {
                    view.receive_for_test(Event::Ended);
                });
                workspace.reconcile(listed(vec![]), cx);
                assert_eq!(workspace.tabs.len(), 1);
                assert_eq!(
                    workspace.tabs[0].view.read(cx).status.label(),
                    "Shell ended on the server",
                    "the live tab says why its shell went"
                );
            });
        });
    }

    /// A server's workspace as it opens: waiting for the server's list,
    /// with the shell this Mac last had selected there.
    fn opening(selected: Option<uuid::Uuid>, cx: &mut App) -> Entity<Workspace> {
        cx.new(|cx| {
            let mut workspace = Workspace::empty(
                "server".into(),
                Some(crate::snapshot_fixture::scope("fixture")),
                "e".into(),
                cx,
            );
            workspace.holdable = true;
            workspace.pending = Some(selected);
            workspace
        })
    }

    #[gpui::test]
    fn restored_tabs_show_their_shells_titles_before_they_are_shown(cx: &mut TestAppContext) {
        init(cx);
        cx.update(|cx| {
            let (older, newer) = (uuid::Uuid::new_v4(), uuid::Uuid::new_v4());
            let titled = |terminal, started, title: &str| {
                let mut shell = shell(terminal, None, &[]);
                shell.started = started;
                shell.title = Some(title.into());
                shell
            };
            let workspace = opening(Some(newer), cx);
            workspace.update(cx, |workspace, cx| {
                workspace.reconcile(
                    listed(vec![
                        titled(older, 1, "dev@host: ~"),
                        titled(newer, 2, "vim"),
                    ]),
                    cx,
                );
                assert_eq!(workspace.active, 1);
                let view = workspace.tabs[0].view.read(cx);
                assert_eq!(view.session_id(), None, "it attaches only when shown");
                assert_eq!(view.title, "dev@host: ~");

                // The server's list follows the shell until the tab attaches.
                workspace.reconcile(
                    listed(vec![
                        titled(older, 1, "cargo test"),
                        titled(newer, 2, "vim"),
                    ]),
                    cx,
                );
                assert_eq!(workspace.tabs[0].view.read(cx).title, "cargo test");

                // Reconnecting keeps it until the shell's replay brings its own.
                workspace.tabs[0]
                    .view
                    .update(cx, |view, cx| view.reconnect(cx));
                assert_eq!(workspace.tabs[0].view.read(cx).title, "cargo test");
            });
        });
    }

    #[gpui::test]
    fn restored_tabs_show_their_agents_status_until_their_shell_replays_its_own(
        cx: &mut TestAppContext,
    ) {
        use crate::terminal_agent::AgentMark;
        use newport_gpui::terminal_transport::Event;
        init(cx);
        cx.update(|cx| {
            let (older, newer) = (uuid::Uuid::new_v4(), uuid::Uuid::new_v4());
            let reporting = |terminal, started, agent: Option<&str>| {
                let mut shell = shell(terminal, None, &[]);
                shell.started = started;
                shell.agent = agent.map(str::to_owned);
                shell
            };
            let workspace = opening(Some(newer), cx);
            workspace.update(cx, |workspace, cx| {
                let mark = |workspace: &Workspace, cx: &App| {
                    workspace.tabs[0]
                        .view
                        .read(cx)
                        .agent()
                        .map(|agent| agent.mark)
                };
                workspace.reconcile(
                    listed(vec![
                        reporting(older, 1, Some("state=done:app=claude-code")),
                        reporting(newer, 2, None),
                    ]),
                    cx,
                );
                assert_eq!(workspace.tabs[0].view.read(cx).session_id(), None);
                assert_eq!(mark(workspace, cx), Some(AgentMark::Done));

                workspace.reconcile(
                    listed(vec![
                        reporting(older, 1, Some("state=bogus")),
                        reporting(newer, 2, None),
                    ]),
                    cx,
                );
                assert_eq!(
                    mark(workspace, cx),
                    None,
                    "an unreadable report shows nothing"
                );

                workspace.reconcile(
                    listed(vec![
                        reporting(older, 1, Some("state=blocked:kind=permission")),
                        reporting(newer, 2, None),
                    ]),
                    cx,
                );
                assert_eq!(mark(workspace, cx), Some(AgentMark::NeedsYou));

                // The shell's own replay, which reports nothing now, wins.
                workspace.tabs[0].view.update(cx, |view, _| {
                    view.receive_for_test(Event::Restore {
                        state: b"$ ".to_vec(),
                        prompted: true,
                    })
                });
                assert_eq!(mark(workspace, cx), None);
            });
        });
    }

    #[gpui::test]
    fn a_finish_seen_before_quitting_shows_as_seen_after_a_restart(cx: &mut TestAppContext) {
        use crate::terminal_agent::AgentMark;
        init(cx);
        cx.update(|cx| {
            let (seen, unseen, rerun) = (
                uuid::Uuid::new_v4(),
                uuid::Uuid::new_v4(),
                uuid::Uuid::new_v4(),
            );
            let done = "state=done:app=claude-code";
            let finished = |terminal, started, agent: &str| {
                let mut shell = shell(terminal, None, &[]);
                shell.started = started;
                shell.agent = Some(agent.to_owned());
                shell
            };
            let shells = || {
                listed(vec![
                    finished(seen, 1, done),
                    finished(unseen, 2, done),
                    finished(rerun, 3, "state=done:app=claude-code:msg=QWdhaW4="),
                ])
            };
            let marks = |workspace: &Entity<Workspace>, cx: &App| -> Vec<_> {
                workspace
                    .read(cx)
                    .tabs
                    .iter()
                    .map(|tab| tab.view.read(cx).agent().map(|agent| agent.mark))
                    .collect()
            };
            // Before quitting, this Mac looked at the first and third tabs.
            let workspace = opening(None, cx);
            workspace.update(cx, |workspace, cx| {
                workspace.reconcile(
                    listed(vec![
                        finished(seen, 1, done),
                        finished(unseen, 2, done),
                        finished(rerun, 3, done),
                    ]),
                    cx,
                );
                for index in [0, 2] {
                    workspace.tabs[index].view.update(cx, |view, cx| {
                        view.see_for_test();
                        cx.notify();
                    });
                }
            });
            let saved = workspace.read(cx).saved_tabs(cx);
            assert_eq!(
                saved.seen,
                [(seen, done.to_owned()), (rerun, done.to_owned())].into()
            );

            // After a restart, the first is still seen; the third's agent
            // has finished again since.
            let workspace = opening(None, cx);
            workspace.update(cx, |workspace, cx| {
                workspace.restored_seen = saved.seen.clone();
                workspace.reconcile(shells(), cx);
            });
            assert_eq!(
                marks(&workspace, cx),
                [None, Some(AgentMark::Done), Some(AgentMark::Done)]
            );
            assert!(workspace.read(cx).restored_seen.is_empty(), "taken once");
        });
    }

    #[gpui::test]
    fn a_server_opens_on_its_own_shells_in_start_order(cx: &mut TestAppContext) {
        init(cx);
        cx.update(|cx| {
            let (older, newer, newest) = (
                uuid::Uuid::new_v4(),
                uuid::Uuid::new_v4(),
                uuid::Uuid::new_v4(),
            );
            let started = |terminal, at| {
                let mut shell = shell(terminal, None, &[]);
                shell.started = at;
                shell
            };
            let shells = || {
                listed(vec![
                    started(newest, 30),
                    started(older, 10),
                    started(newer, 20),
                ])
            };
            let held = |workspace: &Entity<Workspace>, cx: &App| -> Vec<_> {
                let workspace = workspace.read(cx);
                workspace
                    .tabs
                    .iter()
                    .map(|tab| tab.view.read(cx).held())
                    .collect()
            };

            // Without an order of this Mac's own, by when they started, and none is new.
            let workspace = opening(Some(newer), cx);
            assert!(workspace.read(cx).tabs.is_empty(), "it waits for the list");
            workspace.update(cx, |workspace, cx| workspace.reconcile(shells(), cx));
            assert_eq!(
                held(&workspace, cx),
                [Some(older), Some(newer), Some(newest)]
            );
            assert_eq!(workspace.read(cx).active, 1, "what this Mac had selected");
            assert_eq!(
                workspace.read(cx).saved_tabs(cx).selected,
                Some(newer),
                "saved as it was"
            );

            // Without a selection here, the newest.
            let workspace = opening(None, cx);
            workspace.update(cx, |workspace, cx| workspace.reconcile(shells(), cx));
            assert_eq!(workspace.read(cx).active, 2);

            // A server with no shells gets a new one.
            let workspace = opening(Some(newer), cx);
            workspace.update(cx, |workspace, cx| workspace.reconcile(listed(vec![]), cx));
            assert_eq!(held(&workspace, cx), [None]);

            // So does one that can't hold shells, which sends no list.
            let workspace = opening(None, cx);
            let unheld = HeldShells {
                desktop: "this".into(),
                holds: false,
                shells: None,
            };
            workspace.update(cx, |workspace, cx| workspace.reconcile(unheld, cx));
            assert_eq!(held(&workspace, cx), [None]);

            // A server that holds shells but hasn't answered yet is waited for.
            let workspace = opening(None, cx);
            let unanswered = HeldShells {
                desktop: "this".into(),
                holds: true,
                shells: None,
            };
            workspace.update(cx, |workspace, cx| workspace.reconcile(unanswered, cx));
            assert!(workspace.read(cx).tabs.is_empty());
        });
    }

    #[gpui::test]
    fn a_server_opens_on_its_shells_in_the_order_this_mac_left_them(cx: &mut TestAppContext) {
        init(cx);
        cx.update(|cx| {
            let terminals: Vec<_> = (0..4).map(|_| uuid::Uuid::new_v4()).collect();
            let shells = || {
                listed(
                    terminals
                        .iter()
                        .zip(1..)
                        .map(|(terminal, at)| {
                            let mut shell = shell(*terminal, None, &[]);
                            shell.started = at;
                            shell
                        })
                        .collect(),
                )
            };
            let held = |workspace: &Entity<Workspace>, cx: &App| -> Vec<_> {
                let workspace = workspace.read(cx);
                workspace
                    .tabs
                    .iter()
                    .filter_map(|tab| tab.view.read(cx).held())
                    .collect()
            };
            let workspace = opening(Some(terminals[1]), cx);
            workspace.update(cx, |workspace, cx| {
                workspace.reconcile(shells(), cx);
                // The selected second tab is dragged to the end.
                let (dragged, last) = (workspace.tabs[1].id, workspace.tabs.len() - 1);
                workspace.move_tab(dragged, last, cx);
                assert_eq!(
                    workspace.tabs[workspace.active].id, dragged,
                    "still selected"
                );
                // Later lists keep it there.
                workspace.reconcile(shells(), cx);
            });
            let order = vec![terminals[0], terminals[2], terminals[3], terminals[1]];
            assert_eq!(held(&workspace, cx), order);
            let saved = workspace.read(cx).saved_tabs(cx);
            assert_eq!(saved.order, order);

            // The next launch opens them in that order; a shell this Mac
            // hasn't shown, and one that has gone, don't disturb it.
            let gone = order[0];
            let newer = uuid::Uuid::new_v4();
            let workspace = opening(saved.selected, cx);
            workspace.update(cx, |workspace, cx| {
                workspace.restored_order = saved.order.clone();
                let mut list = shells();
                let shells = list.shells.as_mut().unwrap();
                shells.retain(|shell| shell.terminal != gone);
                let mut new = shell(newer, None, &[]);
                new.started = 0;
                shells.push(new);
                workspace.reconcile(list, cx);
            });
            assert_eq!(
                held(&workspace, cx),
                [terminals[2], terminals[3], terminals[1], newer]
            );
            assert_eq!(workspace.read(cx).active, 2, "what this Mac had selected");
        });
    }

    #[test]
    fn a_dragged_tab_takes_a_tabs_place_once_past_its_middle() {
        use crate::project_workspace::dragged_tab_target as target;
        // Tabs 100 wide, the last not laid out.
        let middles = [Some(px(50.)), Some(px(150.)), Some(px(250.)), None];
        // Dragged rightward, past one middle or two.
        assert_eq!(target(0, &middles, px(149.)), None);
        assert_eq!(target(0, &middles, px(151.)), Some(1));
        assert_eq!(target(0, &middles, px(251.)), Some(2));
        // Dragged leftward.
        assert_eq!(target(2, &middles, px(151.)), None);
        assert_eq!(target(2, &middles, px(149.)), Some(1));
        assert_eq!(target(2, &middles, px(10.)), Some(0));
        // Over its own place, or toward a tab not laid out, it stays.
        assert_eq!(target(1, &middles, px(120.)), None);
        assert_eq!(target(2, &middles, px(400.)), None);
    }

    #[test]
    fn a_tab_strip_scrolls_only_while_a_dragged_tab_is_near_its_edges() {
        use crate::project_workspace::drag_scroll_step as step;
        let strip = gpui::Bounds::new(gpui::point(px(100.), px(0.)), size(px(400.), px(36.)));
        assert_eq!(step(px(300.), strip), px(0.));
        assert!(step(px(110.), strip) > px(0.), "toward the start");
        assert!(step(px(490.), strip) < px(0.), "toward the end");
        assert!(
            step(px(101.), strip) > step(px(125.), strip),
            "faster nearer the edge"
        );
        assert_eq!(
            step(px(20.), strip),
            step(px(100.), strip),
            "no faster beyond it"
        );
    }

    #[gpui::test]
    fn dragging_a_tab_selects_it_and_moves_it_along_the_row(cx: &mut TestAppContext) {
        let (workspace, cx) = visual_fixture(cx);
        let ids = |cx: &mut VisualTestContext| -> Vec<usize> {
            workspace.read_with(cx, |workspace, _| {
                workspace.tabs.iter().map(|tab| tab.id).collect()
            })
        };
        assert_eq!(ids(cx), [0, 1, 2]);
        let start = cx.debug_bounds("terminal-tab-0").unwrap().center();
        let last = cx.debug_bounds("terminal-tab-2").unwrap();
        cx.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
        cx.simulate_mouse_move(
            start + gpui::point(px(6.), px(0.)),
            Some(MouseButton::Left),
            Modifiers::default(),
        );
        draw(cx);
        workspace.read_with(cx, |workspace, _| {
            assert_eq!(workspace.dragging, Some(0));
            assert_eq!(
                workspace.tabs[workspace.active].id, 0,
                "the dragged tab is selected"
            );
        });
        // Well below the row, only the pointer's horizontal place counts.
        let end = gpui::point(last.right() - px(4.), last.bottom() + px(80.));
        cx.simulate_mouse_move(end, Some(MouseButton::Left), Modifiers::default());
        draw(cx);
        cx.simulate_mouse_move(
            end + gpui::point(px(1.), px(0.)),
            Some(MouseButton::Left),
            Modifiers::default(),
        );
        draw(cx);
        assert_eq!(ids(cx), [1, 2, 0]);
        cx.simulate_mouse_up(end, MouseButton::Left, Modifiers::default());
        draw(cx);
        workspace.read_with(cx, |workspace, _| {
            assert_eq!(workspace.dragging, None);
            assert!(workspace.rename.is_none(), "a drag is not a click");
            assert_eq!(workspace.tabs[workspace.active].id, 0);
        });
        assert_eq!(ids(cx), [1, 2, 0]);
    }

    #[gpui::test]
    fn the_selected_tab_moves_one_place_from_the_menus(cx: &mut TestAppContext) {
        let (workspace, cx) = visual_fixture(cx);
        let ids = |cx: &mut VisualTestContext| -> Vec<usize> {
            workspace.read_with(cx, |workspace, _| {
                workspace.tabs.iter().map(|tab| tab.id).collect()
            })
        };
        let dispatch = |cx: &mut VisualTestContext, action: Box<dyn gpui::Action>| {
            cx.update(|window, cx| window.dispatch_action(action, cx));
            draw(cx);
        };
        assert_eq!(
            workspace.read_with(cx, |w, _| w.tab_moves(0)),
            (false, true)
        );
        dispatch(cx, Box::new(crate::project_editor::MoveTabRight));
        assert_eq!(ids(cx), [1, 0, 2]);
        assert_eq!(
            workspace.read_with(cx, |w, _| w.tabs[w.active].id),
            0,
            "still selected"
        );
        dispatch(cx, Box::new(crate::project_editor::MoveTabLeft));
        dispatch(cx, Box::new(crate::project_editor::MoveTabLeft));
        assert_eq!(ids(cx), [0, 1, 2], "the first tab stays first");
        // The tab menu moves the tab it was opened on, which needn't be selected.
        assert_eq!(
            workspace.read_with(cx, |w, _| w.tab_moves(2)),
            (true, false)
        );
        workspace.update(cx, |w, cx| w.move_tab_by_one(2, false, cx));
        assert_eq!(ids(cx), [0, 2, 1]);
        assert_eq!(workspace.read_with(cx, |w, _| w.tabs[w.active].id), 0);
    }

    #[gpui::test]
    fn a_tab_held_at_the_strips_edge_scrolls_it_to_reach_tabs_out_of_view(cx: &mut TestAppContext) {
        let (workspace, cx) = visual_fixture(cx);
        workspace.update(cx, |w, cx| {
            for _ in 0..7 {
                w.add(cx);
            }
            w.active = 0;
            for tab in &w.tabs {
                tab.view.update(cx, |view, _| view.close());
            }
        });
        draw(cx);
        let strip = cx.debug_bounds("terminal-tab-bar").unwrap();
        let visible = |cx: &mut VisualTestContext| {
            workspace.read_with(cx, |w, _| {
                (0..w.tabs.len())
                    .filter(|&index| {
                        w.tab_scroll.bounds_for_item(index).is_some_and(|tab| {
                            tab.right() + w.tab_scroll.offset().x <= strip.right()
                        })
                    })
                    .count()
            })
        };
        let shown = visible(cx);
        assert!(shown < 10, "the tabs overflow the strip");
        let start = cx.debug_bounds("terminal-tab-0").unwrap().center();
        cx.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
        cx.simulate_mouse_move(
            start + gpui::point(px(6.), px(0.)),
            Some(MouseButton::Left),
            Modifiers::default(),
        );
        draw(cx);
        let edge = gpui::point(strip.right() - px(4.), start.y);
        cx.simulate_mouse_move(edge, Some(MouseButton::Left), Modifiers::default());
        for _ in 0..200 {
            cx.executor()
                .advance_clock(std::time::Duration::from_millis(16));
            draw(cx);
        }
        workspace.read_with(cx, |w, _| {
            assert!(w.tab_scroll.offset().x < px(0.), "the strip scrolled");
            assert_eq!(w.tabs.last().unwrap().id, 0, "the tab reached the end");
        });
        cx.simulate_mouse_up(edge, MouseButton::Left, Modifiers::default());
        draw(cx);
        let offset = workspace.read_with(cx, |w, _| w.tab_scroll.offset().x);
        cx.executor()
            .advance_clock(std::time::Duration::from_secs(1));
        draw(cx);
        assert_eq!(
            workspace.read_with(cx, |w, _| w.tab_scroll.offset().x),
            offset,
            "scrolling stops with the drag"
        );
    }

    #[gpui::test]
    fn only_a_held_shell_is_saved_as_the_selection(cx: &mut TestAppContext) {
        init(cx);
        cx.update(|cx| {
            let workspace = cx.new(|cx| {
                let mut workspace = Workspace::new(
                    "server".into(),
                    Some(crate::snapshot_fixture::scope("fixture")),
                    "e".into(),
                    cx,
                );
                workspace.add(cx);
                workspace.add(cx);
                workspace
            });
            let held = uuid::Uuid::new_v4();
            workspace.update(cx, |workspace, cx| {
                workspace.tabs[1]
                    .view
                    .update(cx, |view, _| view.attach_to(held));
                workspace.active = 2;
            });
            let saved = workspace.read(cx).saved_tabs(cx);
            assert_eq!(saved.selected, None, "the selected tab isn't held");
            workspace.update(cx, |workspace, _| workspace.active = 1);
            let saved = workspace.read(cx).saved_tabs(cx);
            assert_eq!(saved.selected, Some(held));
        });
    }

    #[gpui::test]
    fn the_servers_list_brings_other_macs_shells_names_and_ends(cx: &mut TestAppContext) {
        init(cx);
        cx.update(|cx| {
            let mine = uuid::Uuid::new_v4();
            let theirs = uuid::Uuid::new_v4();
            let workspace = cx.new(|cx| {
                let workspace = Workspace::new(
                    "server".into(),
                    Some(crate::snapshot_fixture::scope("fixture")),
                    "e".into(),
                    cx,
                );
                workspace.tabs[0]
                    .view
                    .update(cx, |view, _| view.attach_to(mine));
                workspace
            });
            workspace.update(cx, |workspace, cx| {
                workspace.reconcile(
                    listed(vec![
                        shell(mine, Some("build"), &[("this", "Here"), ("air", "Air")]),
                        shell(theirs, None, &[("air", "Air")]),
                    ]),
                    cx,
                );
                assert_eq!(workspace.tabs.len(), 2, "another Mac's shell is a tab");
                assert_eq!(workspace.active, 0, "and doesn't take the selection");
                assert_eq!(workspace.tabs[1].view.read(cx).held(), Some(theirs));
                assert_eq!(workspace.tabs[0].custom.as_deref(), Some("build"));
                assert_eq!(workspace.tabs[0].elsewhere, ["Air"], "not this Mac");

                // An unattached tab whose shell ended elsewhere goes.
                workspace.reconcile(listed(vec![shell(mine, None, &[])]), cx);
                assert_eq!(workspace.tabs.len(), 1);
                assert_eq!(workspace.tabs[0].custom, None, "the name was cleared");
                assert!(workspace.tabs[0].elsewhere.is_empty());

                // An unknown list leaves the tabs as they are.
                workspace.reconcile(
                    HeldShells {
                        desktop: "this".into(),
                        holds: true,
                        shells: None,
                    },
                    cx,
                );
                assert_eq!(workspace.tabs.len(), 1);
            });
        });
    }

    #[gpui::test]
    fn a_shell_closed_here_stays_closed_while_the_server_still_lists_it(cx: &mut TestAppContext) {
        init(cx);
        let cx = cx.add_empty_window();
        let workspace = cx.new(|cx| {
            Workspace::new(
                "server".into(),
                Some(crate::snapshot_fixture::scope("fixture")),
                "e".into(),
                cx,
            )
        });
        let closing = uuid::Uuid::new_v4();
        cx.update(|window, cx| {
            workspace.update(cx, |workspace, cx| {
                workspace.add(cx);
                workspace.tabs[1]
                    .view
                    .update(cx, |view, _| view.attach_to(closing));
                let id = workspace.tabs[1].id;
                workspace.finish_held_close(
                    HeldClose {
                        id,
                        terminal: closing,
                        connection: workspace.connection.clone(),
                        keyboard: false,
                    },
                    Ok(()),
                    window,
                    cx,
                );
                workspace.reconcile(listed(vec![shell(closing, None, &[])]), cx);
                assert_eq!(workspace.tabs.len(), 1, "it doesn't come back");
                // Once gone from the list, the same id would be a new shell.
                workspace.reconcile(listed(vec![]), cx);
                assert!(workspace.ended.is_empty());
            });
        });
    }

    #[gpui::test]
    fn concurrent_acknowledged_held_closes_keep_one_ended_tab_without_restoring_stale_rows(
        cx: &mut TestAppContext,
    ) {
        init(cx);
        let cx = cx.add_empty_window();
        let workspace = cx.new(|cx| {
            Workspace::new(
                "server".into(),
                Some(crate::snapshot_fixture::scope("fixture")),
                "e".into(),
                cx,
            )
        });
        let first = uuid::Uuid::new_v4();
        let second = uuid::Uuid::new_v4();
        cx.update(|window, cx| {
            workspace.update(cx, |workspace, cx| {
                workspace.add(cx);
                workspace.tabs[0]
                    .view
                    .update(cx, |view, _| view.attach_to(first));
                workspace.tabs[1]
                    .view
                    .update(cx, |view, _| view.attach_to(second));
                let first_id = workspace.tabs[0].id;
                let second_id = workspace.tabs[1].id;
                workspace.closing.extend([first, second]);
                for (id, terminal) in [(first_id, first), (second_id, second)] {
                    workspace.finish_held_close(
                        HeldClose {
                            id,
                            terminal,
                            connection: workspace.connection.clone(),
                            keyboard: false,
                        },
                        Ok(()),
                        window,
                        cx,
                    );
                }
                workspace.reconcile(
                    listed(vec![shell(first, None, &[]), shell(second, None, &[])]),
                    cx,
                );
                assert_eq!(workspace.tabs.len(), 1);
                assert_eq!(workspace.tabs[0].view.read(cx).held(), None);
                assert!(workspace.ended.contains(&first));
                assert!(workspace.ended.contains(&second));
                assert!(workspace.closing.is_empty());
            });
        });
    }

    #[gpui::test]
    fn a_refused_or_uncertain_held_close_keeps_the_shell_tab_and_never_suppresses_its_row(
        cx: &mut TestAppContext,
    ) {
        init(cx);
        for (code, message) in [
            (
                Some("reopen_required"),
                "Close the shell in its already-attached window, then reopen it.",
            ),
            (
                Some("holder_outcome_unknown"),
                "The holder did not confirm End. Its outcome is unknown; it was not retried.",
            ),
            (None, "The terminal worker closed early."),
        ] {
            let cx = cx.add_empty_window();
            let workspace = cx.new(|cx| {
                Workspace::new(
                    "server".into(),
                    Some(crate::snapshot_fixture::scope("fixture")),
                    "e".into(),
                    cx,
                )
            });
            let held = uuid::Uuid::new_v4();
            cx.update(|window, cx| {
                workspace.update(cx, |workspace, cx| {
                    workspace.add(cx);
                    workspace.tabs[1]
                        .view
                        .update(cx, |view, _| view.attach_to(held));
                    let id = workspace.tabs[1].id;
                    workspace.closing.push(held);
                    workspace.finish_held_close(
                        HeldClose {
                            id,
                            terminal: held,
                            connection: workspace.connection.clone(),
                            keyboard: false,
                        },
                        Err(newport_client::request_error::RequestError {
                            code: code.map(str::to_owned),
                            message: message.into(),
                        }),
                        window,
                        cx,
                    );
                    workspace.reconcile(listed(vec![shell(held, None, &[])]), cx);
                    assert_eq!(workspace.tabs.len(), 2);
                    assert!(workspace.ended.is_empty());
                    assert!(workspace.closing.is_empty());
                    let tab = workspace.tabs.iter().find(|tab| tab.id == id).unwrap();
                    assert_eq!(tab.view.read(cx).held(), Some(held));
                    assert_eq!(tab.view.read(cx).input_error.as_deref(), Some(message));
                });
            });
        }
    }

    #[gpui::test]
    fn a_name_given_here_outranks_the_servers_until_it_arrives(cx: &mut TestAppContext) {
        init(cx);
        cx.update(|cx| {
            let held = uuid::Uuid::new_v4();
            let workspace = cx.new(|cx| {
                let workspace = Workspace::new(
                    "server".into(),
                    Some(crate::snapshot_fixture::scope("fixture")),
                    "e".into(),
                    cx,
                );
                workspace.tabs[0]
                    .view
                    .update(cx, |view, _| view.attach_to(held));
                workspace
            });
            workspace.update(cx, |workspace, cx| {
                let tab = &mut workspace.tabs[0];
                tab.custom = Some("deploy".into());
                tab.named_at = Some(std::time::Instant::now());
                workspace.reconcile(listed(vec![shell(held, None, &[])]), cx);
                assert_eq!(workspace.tabs[0].custom.as_deref(), Some("deploy"));
                workspace.reconcile(listed(vec![shell(held, Some("deploy"), &[])]), cx);
                assert_eq!(workspace.tabs[0].named_at, None, "settled");
                workspace.reconcile(listed(vec![shell(held, Some("ship"), &[])]), cx);
                assert_eq!(workspace.tabs[0].custom.as_deref(), Some("ship"));
            });
        });
    }

    #[test]
    fn close_warns_about_running_commands_and_other_macs() {
        assert_eq!(close_warning(false, &[]), None);
        assert_eq!(
            close_warning(true, &[]).as_deref(),
            Some("A command is still running in it. Closing the terminal ends it.")
        );
        assert_eq!(
            close_warning(false, &["Air".into(), "Mini".into(), "Studio".into()]).as_deref(),
            Some(
                "It is also open on Air, Mini and Studio. Closing the terminal ends it there too."
            )
        );
        assert!(
            close_warning(true, &["Air".into()])
                .unwrap()
                .starts_with("A command is still running in it, and it is also open on Air.")
        );
    }

    #[gpui::test]
    fn only_a_servers_own_tabs_may_outlive_newport(cx: &mut TestAppContext) {
        init(cx);
        cx.update(|cx| {
            let server = cx.new(|cx| {
                Workspace::new(
                    "server".into(),
                    Some(crate::snapshot_fixture::scope("fixture")),
                    "e".into(),
                    cx,
                )
            });
            let project = cx.new(|cx| {
                Workspace::for_project(
                    "server".into(),
                    Some(crate::snapshot_fixture::scope("fixture")),
                    "/repo".into(),
                    cx,
                )
                .unwrap()
            });
            assert!(server.read(cx).holdable);
            assert!(!project.read(cx).holdable);
            assert!(server.read(cx).tabs[0].view.read(cx).holdable());
            assert!(!project.read(cx).tabs[0].view.read(cx).holdable());
        });
    }

    #[gpui::test]
    fn project_shells_are_lazy_and_keep_their_worktree_scope(cx: &mut TestAppContext) {
        init(cx);
        cx.update(|cx| {
            let workspace = cx.new(|cx| {
                Workspace::for_project(
                    "fixture".into(),
                    Some(crate::snapshot_fixture::scope("scope")),
                    "/repo/worktree".into(),
                    cx,
                )
                .unwrap()
            });
            assert!(workspace.read(cx).session_ids(cx).is_empty());
            workspace.update(cx, |workspace, cx| workspace.add(cx));
            let workspace = workspace.read(cx);
            assert_eq!(workspace.project_root.as_deref(), Some("/repo/worktree"));
            assert_eq!(workspace.tabs.len(), 2);
            assert!(
                workspace
                    .tabs
                    .iter()
                    .all(|t| !t.view.read(cx).status.started)
            );
        });
    }

    #[gpui::test]
    fn project_terminals_take_named_commands_in_a_new_tab(cx: &mut TestAppContext) {
        init(cx);
        cx.update(|cx| {
            let workspace = cx.new(|cx| {
                Workspace::for_project(
                    "fixture".into(),
                    Some(crate::snapshot_fixture::scope("scope")),
                    "/repo/worktree".into(),
                    cx,
                )
                .unwrap()
            });
            let draft = CommandDraft::new("cd -- /repo/worktree && direnv allow".into()).unwrap();
            workspace
                .update(cx, |workspace, cx| {
                    workspace.open_named_command(draft, "Allow .envrc", cx)
                })
                .unwrap();
            let workspace = workspace.read(cx);
            assert_eq!(workspace.tabs.len(), 2);
            assert_eq!(workspace.active, 1);
            assert_eq!(workspace.tabs[1].custom.as_deref(), Some("Allow .envrc"));
            assert!(!workspace.tabs[1].view.read(cx).status.started);
        });
    }

    #[test]
    fn a_new_tab_takes_the_lowest_free_number() {
        assert_eq!(super::free_number([].into_iter()), 0);
        assert_eq!(super::free_number([0, 1, 2].into_iter()), 3);
        assert_eq!(super::free_number([0, 2].into_iter()), 1);
    }

    #[gpui::test]
    fn tab_numbers_start_over_once_the_tabs_close(cx: &mut TestAppContext) {
        init(cx);
        cx.update(|cx| {
            let workspace = cx.new(|cx| {
                Workspace::new(
                    "fixture".into(),
                    Some(crate::snapshot_fixture::scope("fixture")),
                    "fixture@unconnected.invalid".into(),
                    cx,
                )
            });
            workspace.update(cx, |workspace, cx| {
                for i in 0..3 {
                    let draft = CommandDraft::new(format!("true {i}")).unwrap();
                    workspace.open_command(draft, cx).unwrap();
                }
                assert_eq!(workspace.tabs.last().map(|tab| tab.number), Some(3));
                workspace.tabs.clear();
                let draft = CommandDraft::new("true".into()).unwrap();
                workspace.open_command(draft, cx).unwrap();
                assert_eq!(workspace.tabs[0].number, 0);
                assert!(workspace.tabs[0].id > 3, "ids stay unique");
            });
        });
    }

    #[gpui::test]
    fn first_service_command_has_one_dedicated_unstarted_tab(cx: &mut TestAppContext) {
        init(cx);
        cx.update(|cx| {
            let workspace = cx.new(|cx| {
                Workspace::with_command(
                    "fixture".into(),
                    Some(crate::snapshot_fixture::scope("fixture")),
                    "fixture@unconnected.invalid".into(),
                    CommandDraft::new("sudo systemctl restart app.service".into()).unwrap(),
                    cx,
                )
            });
            let workspace = workspace.read(cx);
            assert_eq!(workspace.tabs.len(), 1);
            assert_eq!(workspace.active, 0);
            assert_eq!(workspace.next, 1);
            assert_eq!(workspace.tabs[0].custom.as_deref(), Some("Service command"));
            assert!(workspace.session_ids(cx).is_empty());
            assert!(!workspace.tabs[0].view.read(cx).status.started);
        });
    }

    #[gpui::test]
    fn service_commands_append_without_reusing_tabs_and_capacity_error_is_unchanged(
        cx: &mut TestAppContext,
    ) {
        init(cx);
        cx.update(|cx| {
            let workspace = cx.new(|cx| {
                Workspace::new(
                    "fixture".into(),
                    Some(crate::snapshot_fixture::scope("fixture")),
                    "fixture@unconnected.invalid".into(),
                    cx,
                )
            });
            let shell = workspace.read(cx).tabs[0].view.entity_id();
            workspace.update(cx, |workspace, cx| {
                workspace.tabs[0].custom = Some("Existing shell".into());
                for i in 1..MAX_TERMINALS {
                    let previous: Vec<_> = workspace
                        .tabs
                        .iter()
                        .map(|tab| tab.view.entity_id())
                        .collect();
                    workspace
                        .open_command(
                            CommandDraft::new(format!("sudo systemctl restart app-{i}.service"))
                                .unwrap(),
                            cx,
                        )
                        .unwrap();
                    assert_eq!(workspace.tabs.len(), i + 1);
                    assert_eq!(workspace.active, i);
                    assert_eq!(workspace.tabs[i].custom.as_deref(), Some("Service command"));
                    assert!(!previous.contains(&workspace.tabs[i].view.entity_id()));
                    assert_eq!(
                        workspace.tabs[..i]
                            .iter()
                            .map(|tab| tab.view.entity_id())
                            .collect::<Vec<_>>(),
                        previous,
                    );
                }
                let tabs: Vec<_> = workspace
                    .tabs
                    .iter()
                    .map(|tab| tab.view.entity_id())
                    .collect();
                let before = (workspace.active, workspace.next);
                assert_eq!(
                    workspace.open_command(
                        CommandDraft::new("sudo systemctl stop app.service".into()).unwrap(),
                        cx
                    ),
                    Err("Close a terminal tab before opening this service command.".into()),
                );
                assert_eq!((workspace.active, workspace.next), before);
                assert_eq!(
                    workspace
                        .tabs
                        .iter()
                        .map(|tab| tab.view.entity_id())
                        .collect::<Vec<_>>(),
                    tabs
                );
                assert_eq!(workspace.tabs[0].view.entity_id(), shell);
                assert_eq!(workspace.tabs[0].custom.as_deref(), Some("Existing shell"));
                assert!(workspace.session_ids(cx).is_empty());
            });
        });
    }

    #[test]
    fn prompt_tab_names_keep_directory_and_stable_session_number() {
        assert_eq!(
            super::automatic_tab_title("developer@sample-host:~/Projects/example-api-service", 0),
            "example-api-service"
        );
        assert_eq!(
            super::automatic_tab_title("developer@sample-host:~/Projects/example-api-service", 2),
            "example-api-service 3"
        );
        assert_eq!(super::automatic_tab_title("dev@host:~", 1), "host 2");
        assert_eq!(super::automatic_tab_title("dev@host:/", 0), "host");
        assert_eq!(super::automatic_tab_title("cargo test", 2), "cargo test");
        assert_eq!(super::automatic_tab_title("", 1), "Terminal 2");
    }

    #[test]
    fn custom_names_trim_reset_and_bound_unicode_safely() {
        assert_eq!(custom_name(" \n "), None);
        assert_eq!(custom_name("  Build\nlogs  ").as_deref(), Some("Buildlogs"));
        assert_eq!(custom_name(&"界".repeat(80)).unwrap().chars().count(), 60);
    }

    #[gpui::test]
    fn container_shell_uses_a_new_titled_tab_and_refuses_at_shared_capacity(
        cx: &mut TestAppContext,
    ) {
        init(cx);
        cx.update(|cx| {
            let workspace = cx.new(|cx| {
                Workspace::with_shell(
                    "fixture".into(),
                    Some(crate::snapshot_fixture::scope("fixture")),
                    "fixture@unconnected.invalid".into(),
                    CommandDraft::container_shell("docker exec -it -- abcdef sh".into()).unwrap(),
                    "web · Shell".into(),
                    cx,
                )
            });
            workspace.update(cx, |workspace, cx| {
                assert_eq!(workspace.tabs.len(), 1);
                assert_eq!(workspace.tabs[0].custom.as_deref(), Some("web · Shell"));
                assert!(!workspace.tabs[0].view.read(cx).status.started);
                let initial = workspace.tabs[0].view.entity_id();
                // A running program in an older tab must never receive this launch.
                workspace.tabs[0]
                    .view
                    .update(cx, |view, _| view.status.ready());
                workspace
                    .open_command(
                        CommandDraft::new("sudo usermod -aG docker user".into()).unwrap(),
                        cx,
                    )
                    .unwrap();
                assert_eq!(workspace.tabs[1].custom.as_deref(), Some("Service command"));
                for index in 2..MAX_TERMINALS {
                    let before: Vec<_> = workspace
                        .tabs
                        .iter()
                        .map(|tab| tab.view.entity_id())
                        .collect();
                    workspace
                        .open_shell(
                            CommandDraft::container_shell(format!(
                                "docker exec -it -- abcdef{index} sh"
                            ))
                            .unwrap(),
                            format!("worker-{index} · Shell"),
                            cx,
                        )
                        .unwrap();
                    assert_eq!(workspace.active, index);
                    assert_eq!(workspace.tabs.len(), index + 1);
                    assert_eq!(
                        workspace.tabs[..index]
                            .iter()
                            .map(|tab| tab.view.entity_id())
                            .collect::<Vec<_>>(),
                        before
                    );
                    assert!(!before.contains(&workspace.tabs[index].view.entity_id()));
                    assert!(!workspace.tabs[index].view.read(cx).status.started);
                }
                let before = (workspace.active, workspace.next);
                let tabs: Vec<_> = workspace
                    .tabs
                    .iter()
                    .map(|tab| tab.view.entity_id())
                    .collect();
                assert_eq!(
                    workspace.open_shell(
                        CommandDraft::container_shell("docker exec -it -- abcdef sh".into())
                            .unwrap(),
                        "another · Shell".into(),
                        cx,
                    ),
                    Err("Close a terminal tab before opening this container shell.".into())
                );
                assert_eq!((workspace.active, workspace.next), before);
                assert_eq!(
                    workspace
                        .tabs
                        .iter()
                        .map(|tab| tab.view.entity_id())
                        .collect::<Vec<_>>(),
                    tabs
                );
                assert_eq!(workspace.tabs[0].view.entity_id(), initial);
                assert_eq!(workspace.tabs[0].custom.as_deref(), Some("web · Shell"));
                assert!(workspace.tabs[0].view.read(cx).status.started);
                assert!(workspace.session_ids(cx).is_empty());
            });
        });
    }
    #[test]
    fn tab_keys_wrap_and_ignore_non_navigation_input() {
        assert_eq!(tab_destination(0, "left", 3), Some(2));
        assert_eq!(tab_destination(2, "right", 3), Some(0));
        assert_eq!(tab_destination(1, "home", 3), Some(0));
        assert_eq!(tab_destination(1, "end", 3), Some(2));
        assert_eq!(tab_destination(1, "enter", 3), Some(1));
        assert_eq!(tab_destination(1, "space", 3), Some(1));
        assert_eq!(tab_destination(0, "tab", 3), None);
        assert_eq!(tab_destination(0, "a", 3), None);
        assert_eq!(tab_destination(0, "left", 0), None);
        assert_eq!(tab_destination(3, "right", 3), None);
    }
    #[test]
    fn closing_tabs_preserves_neighbor_selection() {
        assert_eq!(selected_after_close(2, 0, 2), 1);
        assert_eq!(selected_after_close(2, 2, 2), 1);
        assert_eq!(selected_after_close(0, 2, 2), 0);
        assert_eq!(selected_after_close(0, 0, 0), 0);
    }

    fn visual_fixture(cx: &mut TestAppContext) -> (Entity<Workspace>, &mut VisualTestContext) {
        init(cx);
        let mut workspace = None;
        let (_, cx) = cx.add_window_view(|window, cx| {
            let entity = cx.new(|cx| {
                let mut workspace = Workspace::new(
                    "no-transport-fixture".into(),
                    Some(crate::snapshot_fixture::scope("fixture")),
                    "fixture@invalid".into(),
                    cx,
                );
                workspace.add(cx);
                workspace.add(cx);
                workspace.active = 0;
                for tab in &workspace.tabs {
                    tab.view.update(cx, |view, _| view.close());
                }
                workspace
            });
            entity.read(cx).tabs[0].focus.clone().focus(window, cx);
            workspace = Some(entity.clone());
            gpui::base::Root::new(entity, window, cx)
        });
        cx.simulate_resize(size(px(700.), px(400.)));
        draw(cx);
        (workspace.unwrap(), cx)
    }

    #[gpui::test]
    fn the_first_key_after_switching_servers_tells_that_servers_backend(cx: &mut TestAppContext) {
        use crate::terminal_view::input_tests::{Peer, attached, settle, wait};
        use gpui_kit::component::combobox::ComboboxEvent;
        init(cx);
        use crate::snapshot_fixture as fixture;
        let peers = [Peer::new(), Peer::new()];
        let (alpha, beta) = (fixture::SERVER.to_string(), fixture::SECOND.to_string());
        let (alpha, beta) = (alpha.as_str(), beta.as_str());
        let mut parts = None;
        let (_, cx) = cx.add_window_view(|window, cx| {
            let newport = cx.new(|cx| {
                let mut view = crate::page_perf_tests::owner(window, cx);
                let server = |id, name: &str| {
                    let mut server = fixture::server(id, name);
                    server.ssh_host = name.to_lowercase();
                    server.ssh_user = "u".into();
                    server.agent_install_approved = false;
                    server
                };
                view.snapshot = Some(fixture::snapshot(vec![
                    server(fixture::SERVER, "Alpha"),
                    server(fixture::SECOND, "Beta"),
                ]));
                view.server = alpha.into();
                view.page = crate::Page::Terminal;
                view
            });
            let terminals = [alpha, beta].map(|server| {
                let peer = &peers[usize::from(server == beta)];
                cx.new(|cx| attached(peer, server, cx))
            });
            newport.update(cx, |view, cx| {
                for (server, terminal) in [alpha, beta].into_iter().zip(&terminals) {
                    let workspace =
                        cx.new(|cx| Workspace::showing(server.into(), terminal.clone(), cx));
                    view.terminals.insert(server.into(), workspace);
                }
            });
            parts = Some((newport.clone(), terminals));
            gpui::base::Root::new(newport, window, cx)
        });
        let (newport, terminals) = parts.unwrap();
        cx.simulate_resize(size(px(1000.), px(700.)));
        cx.update(|window, _| window.activate_window());
        draw(cx);
        for terminal in &terminals {
            wait(terminal, cx, |view| view.ready());
        }
        // Picking a server in the sidebar, then clicking into its terminal,
        // which focuses it before the click is handled.
        let switch = |server: &str, index: usize, cx: &mut VisualTestContext| {
            let picker = newport.read_with(cx, |view, _| view.server_picker.clone());
            picker.update(cx, |_, cx| {
                cx.emit(ComboboxEvent::Activate(vec![server.into()]))
            });
            draw(cx);
            let focus = terminals[index].read_with(cx, |view, _| view.focus.clone());
            cx.update(|window, cx| focus.focus(window, cx));
            draw(cx);
        };
        // Someone types on each server in turn; each holder makes its tab
        // the active one.
        for (round, (server, index)) in [(alpha, 0), (beta, 1), (alpha, 0)].into_iter().enumerate()
        {
            switch(server, index, cx);
            settle(&terminals[index], cx);
            let before = peers[index].activations();
            assert_eq!(
                before,
                round / 2,
                "switching to {server} alone asks nothing"
            );
            cx.simulate_keystrokes("x");
            wait(&terminals[index], cx, |_| {
                peers[index].activations() == before + 1
            });
            peers[index].event(json!({"type": "active", "data": true}));
            wait(&terminals[index], cx, |view| view.active_here());
        }
    }

    #[gpui::test]
    fn only_a_tab_someone_opens_asks_for_the_display_never_one_focused(cx: &mut TestAppContext) {
        let (workspace, cx) = visual_fixture(cx);
        let asks = |workspace: &Entity<Workspace>, cx: &mut VisualTestContext| {
            workspace.read_with(cx, |w, cx| {
                w.tabs
                    .iter()
                    .map(|tab| tab.view.read(cx).asks_to_activate())
                    .collect::<Vec<_>>()
            })
        };
        // Tabs Newport opened by itself, focused and then another selected
        // and focused: none asks.
        let opened = asks(&workspace, cx).len();
        assert!(opened >= 2);
        assert!(asks(&workspace, cx).iter().all(|asks| !asks));
        workspace.update_in(cx, |w, window, cx| w.select_tab(2, window, cx));
        draw(cx);
        assert!(asks(&workspace, cx).iter().all(|asks| !asks));
        // A held shell's tab, as at launch or from the server's list.
        workspace.update(cx, |w, cx| w.add_held(uuid::Uuid::new_v4(), cx));
        draw(cx);
        assert!(asks(&workspace, cx).iter().all(|asks| !asks));
        // A new tab someone opens asks, whether from the tab bar or another page.
        workspace.update_in(cx, |w, window, cx| w.new_tab(window, cx));
        assert!(asks(&workspace, cx)[opened + 1]);
        workspace.update(cx, |w, cx| {
            w.open_command(CommandDraft::new("true".into()).unwrap(), cx)
                .unwrap()
        });
        assert!(asks(&workspace, cx)[opened + 2]);
    }

    #[gpui::test]
    fn numbers_select_tabs_and_nine_is_the_last(cx: &mut TestAppContext) {
        let (workspace, cx) = visual_fixture(cx);
        let count = workspace.read_with(cx, |w, _| w.tabs.len());
        assert!(count >= 2);
        for (number, expected) in [(2, 1), (9, count - 1), (1, 0), (8, 0)] {
            workspace.update_in(cx, |w, window, cx| w.select_tab(number, window, cx));
            // A number past the last tab leaves the current one.
            assert_eq!(
                workspace.read_with(cx, |w, _| w.active),
                expected,
                "{number}"
            );
        }
    }

    fn draw(cx: &mut VisualTestContext) {
        for _ in 0..3 {
            cx.run_until_parked();
            cx.update(|window, cx| window.draw(cx).clear(cx));
        }
    }

    #[gpui::test]
    fn a_tab_whose_shell_isnt_held_shows_why_above_its_shell(cx: &mut TestAppContext) {
        use newport_gpui::terminal_transport::Event;
        init(cx);
        let mut owner = None;
        let (_, cx) = cx.add_window_view(|window, cx| {
            let entity = cx.new(|cx| {
                let mut owner = crate::page_perf_tests::owner(window, cx);
                owner.page = crate::Page::Terminal;
                owner
            });
            owner = Some(entity.clone());
            gpui::base::Root::new(entity, window, cx)
        });
        let owner = owner.unwrap();
        cx.simulate_resize(size(px(1100.), px(700.)));
        draw(cx);
        let workspace = owner.read_with(cx, |owner, _| owner.terminals[&owner.server].clone());
        let terminal = workspace.read_with(cx, |workspace, _| workspace.tabs[0].view.clone());
        let notice = format!(
            "{}. {}",
            super::NOT_HELD_TITLE,
            super::not_held_message("no private directory")
        );
        let shown = |cx: &mut VisualTestContext| {
            cx.update(|window, _| {
                gpui::base::test_support::snapshots(window)
                    .iter()
                    .any(|node| node.label() == Some(notice.as_str()))
            })
        };
        assert!(!shown(cx));
        terminal.update(cx, |view, _| {
            view.receive_for_test(Event::NotHeld("no private directory".into()));
            view.receive_for_test(Event::Ready);
        });
        draw(cx);
        assert!(shown(cx), "the tab says its shell won't keep running");
        terminal.update(cx, |view, _| view.receive_for_test(Event::Exit(Some(0))));
        draw(cx);
        assert!(!shown(cx), "not once that shell is gone");
    }

    #[gpui::test]
    fn immersive_toolbar_restores_shell_without_replacing_terminal(cx: &mut TestAppContext) {
        init(cx);
        let mut owner = None;
        let (_, cx) = cx.add_window_view(|window, cx| {
            let entity = cx.new(|cx| {
                let mut owner = crate::page_perf_tests::owner(window, cx);
                owner.page = crate::Page::Terminal;
                owner
            });
            entity.read(cx).focus.clone().focus(window, cx);
            owner = Some(entity.clone());
            gpui::base::Root::new(entity, window, cx)
        });
        let owner = owner.unwrap();
        cx.simulate_resize(size(px(1100.), px(700.)));
        draw(cx);
        let workspace = owner.read_with(cx, |owner, _| owner.terminals[&owner.server].clone());
        let terminal = workspace.read_with(cx, |workspace, _| workspace.tabs[0].view.clone());
        for _ in 0..2 {
            for (label, immersive) in [
                ("Enter immersive mode", true),
                ("Exit immersive mode", false),
            ] {
                let button = cx.update(|window, _| {
                    gpui::base::test_support::snapshots(window)
                        .into_iter()
                        .find(|e| e.label() == Some(label))
                        .unwrap()
                        .bounds()
                });
                if !immersive {
                    let drag = cx.debug_bounds("terminal-drag-region").unwrap();
                    let tabs = cx.debug_bounds("terminal-tab-bar").unwrap();
                    assert!(drag.right() <= button.left());
                    assert!(drag.left() >= tabs.right());
                }
                cx.simulate_click(button.center(), Default::default());
                draw(cx);
                assert_eq!(
                    owner.read_with(cx, |owner, _| owner.terminal_immersive),
                    immersive
                );
                assert_eq!(
                    workspace.read_with(cx, |workspace, _| workspace.immersive),
                    immersive
                );
                assert_eq!(
                    workspace.read_with(cx, |workspace, _| workspace.tabs[0].view.clone()),
                    terminal
                );
                let sidebar_visible = cx.update(|window, _| {
                    gpui::base::test_support::snapshots(window)
                        .iter()
                        .any(|e| e.label() == Some("Use icons-only navigation"))
                });
                assert_eq!(sidebar_visible, !immersive);
            }
        }
    }

    #[gpui::test]
    fn a_closing_held_tab_keeps_its_close_slot_and_closes_again_once_the_close_fails(
        cx: &mut TestAppContext,
    ) {
        let (workspace, cx) = visual_fixture(cx);
        let held = uuid::Uuid::new_v4();
        let id = workspace.update(cx, |workspace, cx| {
            workspace.tabs[1]
                .view
                .update(cx, |view, _| view.attach_to(held));
            workspace.tabs[1].id
        });
        draw(cx);
        let close = |cx: &mut VisualTestContext| {
            cx.update(|window, _| {
                gpui::base::test_support::snapshots(window)
                    .into_iter()
                    .find(|e| {
                        e.path()
                            .contains(&gpui::ElementId::from(("close-terminal", id)))
                    })
                    .expect("tab has a close button")
                    .bounds()
            })
        };
        let slot = close(cx);
        let input_error = |cx: &mut VisualTestContext| {
            workspace.read_with(cx, |workspace, cx| {
                workspace.tabs[1].view.read(cx).input_error.clone()
            })
        };

        workspace.update(cx, |workspace, cx| {
            workspace.closing.push(held);
            cx.notify();
        });
        draw(cx);
        assert_eq!(close(cx), slot, "the spinner takes the close button's slot");
        cx.simulate_click(slot.center(), Default::default());
        draw(cx);
        assert_eq!(input_error(cx), None, "a closing tab ignores clicks");

        cx.update(|window, cx| {
            workspace.update(cx, |workspace, cx| {
                workspace.finish_held_close(
                    HeldClose {
                        id,
                        terminal: held,
                        connection: workspace.connection.clone(),
                        keyboard: false,
                    },
                    Err(newport_client::request_error::RequestError {
                        code: None,
                        message: "The terminal worker closed early.".into(),
                    }),
                    window,
                    cx,
                )
            })
        });
        draw(cx);
        assert_eq!(close(cx), slot);
        // The invalid fixture server identity makes an accepted close report an error.
        workspace.update(cx, |workspace, cx| {
            workspace.tabs[1]
                .view
                .update(cx, |view, _| view.input_error = None);
            cx.notify();
        });
        cx.simulate_click(slot.center(), Default::default());
        draw(cx);
        assert_eq!(
            input_error(cx).as_deref(),
            Some("Invalid server identity. Reopen this server.")
        );
    }

    #[gpui::test]
    fn newly_opened_tab_stays_fully_visible_as_its_title_and_viewport_change(
        cx: &mut TestAppContext,
    ) {
        let (workspace, cx) = visual_fixture(cx);
        cx.simulate_resize(size(px(520.), px(400.)));
        workspace.update_in(cx, |workspace, window, cx| workspace.new_tab(window, cx));
        draw(cx);
        let assert_visible = |cx: &mut VisualTestContext, stage: &str| {
            let bar = cx.debug_bounds("terminal-tab-bar").unwrap();
            let tab = cx.debug_bounds("terminal-tab-3").unwrap();
            assert!(tab.left() >= bar.left() - px(1.), "tab start is clipped");
            assert!(
                tab.right() <= bar.right() + px(1.),
                "tab end is clipped {stage}: tab {tab:?}, bar {bar:?}"
            );
            let close = cx.update(|window, _| {
                gpui::base::test_support::snapshots(window)
                    .into_iter()
                    .find(|e| {
                        e.path()
                            .contains(&gpui::ElementId::from(("close-terminal", 3_usize)))
                    })
            });
            let close = close.expect("active tab has a close button").bounds();
            assert!(close.left() >= bar.left());
            assert!(close.right() <= bar.right(), "close button is clipped");
        };
        assert_visible(cx, "opened");
        workspace.update(cx, |workspace, cx| {
            workspace.tabs[3].view.update(cx, |view, cx| {
                view.title = "newport@newport-ubuntu:~/Projects/example-api-service".into();
                cx.notify();
            });
        });
        draw(cx);
        assert_visible(cx, "title changed");
        cx.simulate_resize(size(px(400.), px(400.)));
        draw(cx);
        assert_visible(cx, "resized");
        workspace.update(cx, |workspace, cx| {
            workspace.tab_scroll.set_offset(gpui::point(px(0.), px(0.)));
            cx.notify();
        });
        draw(cx);
        assert_eq!(
            workspace.read_with(cx, |w, _| w.tab_scroll.offset().x),
            px(0.)
        );
    }

    #[gpui::test]
    fn a_tab_grows_when_its_short_title_gets_longer(cx: &mut TestAppContext) {
        let (workspace, cx) = visual_fixture(cx);
        workspace.update(cx, |workspace, cx| {
            workspace.tabs.truncate(1);
            cx.notify();
        });
        cx.simulate_resize(size(px(1100.), px(400.)));
        let title = |workspace: &Entity<Workspace>, cx: &mut VisualTestContext, title: &str| {
            workspace.update(cx, |workspace, cx| {
                for tab in &workspace.tabs {
                    tab.view.update(cx, |view, cx| {
                        view.title = title.into();
                        cx.notify();
                    });
                }
            });
            draw(cx);
            cx.debug_bounds("terminal-tab-0").unwrap().size.width
        };
        let short = title(&workspace, cx, "~");
        let long = title(&workspace, cx, "a title long enough to need room");
        assert!(
            long > short + px(100.),
            "the tab kept its short width: {short:?} → {long:?}"
        );
    }

    #[gpui::test]
    fn terminal_tabs_stay_inside_the_row_above_its_border(cx: &mut TestAppContext) {
        let (workspace, cx) = visual_fixture(cx);
        workspace.update(cx, |w, cx| {
            for tab in &w.tabs {
                tab.view.update(cx, |v, cx| {
                    v.title = "dev@host:~/Projects/example-api-service".into();
                    cx.notify();
                });
            }
        });
        for width in [440., 700., 1100.] {
            cx.simulate_resize(size(px(width), px(400.)));
            for active in 0..3 {
                workspace.update(cx, |workspace, cx| {
                    workspace.active = active;
                    cx.notify();
                });
                draw(cx);
                let row = cx.debug_bounds("terminal-tab-row").unwrap();
                let bar = cx.debug_bounds("terminal-tab-bar").unwrap();
                assert_eq!(row.size.height, px(crate::ui::HEADER_HEIGHT));
                assert!(bar.top() >= row.top(), "tab bar overlaps frame border");
                assert_eq!(
                    bar.bottom(),
                    row.bottom(),
                    "active tab joins the terminal canvas"
                );
                let tab = cx
                    .debug_bounds(["terminal-tab-0", "terminal-tab-1", "terminal-tab-2"][active])
                    .unwrap();
                assert!(tab.top() >= bar.top());
                assert!(tab.bottom() <= bar.bottom());
                assert!(
                    tab.left() >= bar.left() - px(1.),
                    "active tab is revealed after keyboard navigation"
                );
                assert!(
                    tab.right() <= bar.right() + px(1.),
                    "active tab is not clipped"
                );
                let add = cx.update(|window, _| {
                    gpui::base::test_support::snapshots(window)
                        .iter()
                        .find(|e| e.label() == Some("New terminal"))
                        .unwrap()
                        .bounds()
                });
                assert!(
                    (add.left() - bar.right()).abs() <= px(1.),
                    "add stays adjacent to tabs"
                );
                assert_eq!(add.size.height, px(crate::ui::HEADER_HEIGHT));
            }
        }
    }

    #[gpui::test]
    fn workspace_focus_includes_tab_headers_and_renaming(cx: &mut TestAppContext) {
        let (workspace, cx) = visual_fixture(cx);
        workspace.update_in(cx, |w, window, cx| {
            assert!(
                w.contains_focus(window, cx),
                "a tab header belongs to the terminal workspace"
            );
            w.cycle_tab(false, window, cx);
            assert_eq!(w.active, 2);
            assert!(w.contains_focus(window, cx));
            w.cycle_tab(true, window, cx);
            assert_eq!(w.active, 0);
            let id = w.tabs[0].id;
            w.rename(id, window, cx);
        });
        draw(cx);
        workspace.update_in(cx, |w, window, cx| {
            assert!(w.rename.is_some());
            assert!(
                w.contains_focus(window, cx),
                "rename input belongs to the terminal workspace"
            );
        });
    }
    #[gpui::test]
    fn global_connectivity_notifies_every_page_without_polling_or_terminal_overrides(
        cx: &mut TestAppContext,
    ) {
        use crate::connectivity::{self, Phase, ServerState, Snapshot};
        use std::{cell::Cell, rc::Rc};
        let (workspace, cx) = visual_fixture(cx);
        let id = uuid::Uuid::new_v4();
        let instance = uuid::Uuid::new_v4();
        let owner = cx.update(|window, cx| cx.new(|cx| crate::page_perf_tests::owner(window, cx)));
        owner.update(cx, |owner, _| owner.server = id.to_string());
        draw(cx);
        let count = Rc::new(Cell::new(0));
        let observed = count.clone();
        let _subscription =
            cx.update(|_, cx| cx.observe(&owner, move |_, _| observed.set(observed.get() + 1)));
        let pages = [
            crate::Page::Overview,
            crate::Page::Ports,
            crate::Page::Services,
            crate::Page::Containers,
            crate::Page::Files,
            crate::Page::Projects,
            crate::Page::Terminal,
            crate::Page::Integration,
        ];
        for (index, page) in pages.into_iter().enumerate() {
            owner.update(cx, |owner, _| owner.page = page);
            let phase = if index % 2 == 0 {
                Phase::AwaitingApproval
            } else {
                Phase::Failed
            };
            cx.update(|_, cx| {
                connectivity::publish(
                    Snapshot {
                        instance_id: instance,
                        revision: index as u64 + 1,
                        servers: [(
                            id,
                            ServerState {
                                phase,
                                ..Default::default()
                            },
                        )]
                        .into(),
                    },
                    cx,
                )
            });
            draw(cx);
            assert_eq!(
                count.get(),
                index + 1,
                "every page must observe the singleton without owner.tick"
            );
        }
        workspace.update(cx, |workspace, cx| {
            for tab in &workspace.tabs {
                tab.view.update(cx, |view, cx| {
                    view.status.ready();
                    view.title = "live shell".into();
                    cx.notify();
                });
            }
        });
        draw(cx);
        assert_eq!(
            count.get(),
            8,
            "terminal events cannot override server connectivity"
        );
    }

    #[gpui::test]
    fn a_tab_shows_what_its_agent_reports_and_clears_done_once_looked_at(cx: &mut TestAppContext) {
        use newport_gpui::terminal_transport::Event;
        let (workspace, cx) = visual_fixture(cx);
        let hidden = workspace.read_with(cx, |workspace, _| workspace.tabs[1].view.clone());
        let id = workspace.read_with(cx, |workspace, _| workspace.tabs[1].id);
        let report = |state: &str| {
            Event::Data(format!("\x1b]7501;state={state}:app=claude-code\x07").into_bytes())
        };
        let mark = |cx: &mut VisualTestContext, mark: &str| {
            cx.debug_bounds(Box::leak(
                format!("terminal-tab-{id}-agent-{mark}").into_boxed_str(),
            ))
            .is_some()
        };

        hidden.update(cx, |view, _| {
            view.receive_for_test(report("blocked:kind=permission"))
        });
        draw(cx);
        assert!(mark(cx, "NeedsYou"), "a hidden tab shows its agent waiting");

        hidden.update(cx, |view, _| view.receive_for_test(report("working")));
        draw(cx);
        assert!(mark(cx, "Working"));
        assert!(!mark(cx, "NeedsYou"));

        hidden.update(cx, |view, _| view.receive_for_test(report("done")));
        draw(cx);
        assert!(mark(cx, "Done"), "nobody has looked at it yet");

        workspace.update(cx, |workspace, cx| {
            workspace.active = 1;
            cx.notify();
        });
        draw(cx);
        assert!(
            !cx.update(|window, _| window.is_window_active()),
            "the fixture's window starts in the background"
        );
        assert!(
            mark(cx, "Done"),
            "a background window isn't being looked at"
        );

        cx.update(|window, _| window.activate_window());
        draw(cx);
        assert!(!mark(cx, "Done"), "looking at the tab sees the finish");

        hidden.update(cx, |view, _| view.receive_for_test(report("idle")));
        draw(cx);
        for state in ["Working", "NeedsYou", "Done", "Failed"] {
            assert!(!mark(cx, state), "an idle agent shows nothing");
        }
    }

    #[gpui::test]
    fn terminal_redraws_only_notify_workspace_when_tab_presentation_changes(
        cx: &mut TestAppContext,
    ) {
        use std::{cell::Cell, rc::Rc};
        let (workspace, cx) = visual_fixture(cx);
        let notifications = Rc::new(Cell::new(0));
        let observed = notifications.clone();
        let _subscription =
            cx.update(|_, cx| cx.observe(&workspace, move |_, _| observed.set(observed.get() + 1)));
        let view = workspace.read_with(cx, |workspace, _| workspace.tabs[0].view.clone());
        // Output/selection redraws should not reconstruct the tab bar.
        for _ in 0..3 {
            view.update(cx, |_, cx| cx.notify());
            draw(cx);
        }
        assert_eq!(notifications.get(), 0);
        view.update(cx, |view, cx| {
            view.title = "changed remote title".into();
            cx.notify();
        });
        draw(cx);
        assert_eq!(notifications.get(), 1);
        view.update(cx, |view, cx| {
            view.status.failed("connection lost");
            cx.notify();
        });
        draw(cx);
        assert_eq!(notifications.get(), 2);
        view.update(cx, |view, cx| {
            view.input_error = Some("input queue full".into());
            cx.notify();
        });
        draw(cx);
        assert_eq!(notifications.get(), 3);
    }

    #[gpui::test]
    fn inline_rename_commits_on_enter_and_blur_and_cancels_on_escape(cx: &mut TestAppContext) {
        let (workspace, cx) = visual_fixture(cx);
        let position = cx.debug_bounds("terminal-tab-0").unwrap().center();
        cx.simulate_event(MouseDownEvent {
            position,
            button: MouseButton::Left,
            modifiers: Modifiers::default(),
            click_count: 2,
            first_mouse: false,
        });
        cx.simulate_mouse_up(position, MouseButton::Left, Modifiers::default());
        draw(cx);
        assert!(
            cx.debug_bounds("terminal-tab-1").is_some(),
            "other tabs stay visible during rename"
        );
        assert!(workspace.read_with(cx, |workspace, _| workspace.rename.is_some()));
        let editor = cx.update(|window, _| {
            let input = gpui::base::test_support::snapshots(window)
                .into_iter()
                .find(|node| {
                    node.label() == Some("Rename Terminal 1")
                        && node.role() == Some(gpui::Role::TextInput)
                })
                .expect("inline rename input");
            crate::field_frame(window, &input)
        });
        assert_eq!(editor.size, size(px(132.), px(26.)));
        cx.simulate_keystrokes("cmd-a");
        cx.simulate_input("Build logs");
        cx.simulate_keystrokes("enter");
        draw(cx);
        assert_eq!(
            workspace.read_with(cx, |workspace, _| workspace.tabs[0].custom.clone()),
            Some("Build logs".into())
        );

        cx.update(|window, cx| {
            workspace.update(cx, |workspace, cx| workspace.rename(0, window, cx))
        });
        draw(cx);
        cx.simulate_keystrokes("cmd-a");
        cx.simulate_input("discarded");
        cx.simulate_keystrokes("escape");
        draw(cx);
        assert_eq!(
            workspace.read_with(cx, |workspace, _| workspace.tabs[0].custom.clone()),
            Some("Build logs".into())
        );

        cx.update(|window, cx| {
            workspace.update(cx, |workspace, cx| workspace.rename(0, window, cx))
        });
        draw(cx);
        cx.simulate_keystrokes("cmd-a");
        cx.simulate_input("Retained");
        let next = cx.debug_bounds("terminal-tab-1").unwrap().center();
        cx.simulate_mouse_down(next, MouseButton::Left, Modifiers::default());
        cx.simulate_mouse_up(next, MouseButton::Left, Modifiers::default());
        draw(cx);
        workspace.read_with(cx, |workspace, cx| {
            assert_eq!(
                workspace.active, 1,
                "clicking a sibling tab changes the session"
            );
            assert!(workspace.rename.is_none());
            assert_eq!(workspace.tabs[0].custom.as_deref(), Some("Retained"));
            assert_eq!(workspace.active, 1);
            assert!(workspace.session_ids(cx).is_empty());
        });
        cx.update(|window, cx| {
            assert!(
                workspace.read(cx).tabs[1]
                    .view
                    .read(cx)
                    .focus
                    .is_focused(window)
            );
        });
    }

    #[gpui::test]
    fn middle_close_and_keyboard_navigation_use_stable_ids(cx: &mut TestAppContext) {
        let (workspace, cx) = visual_fixture(cx);
        let first = cx.debug_bounds("terminal-tab-0").unwrap().center();
        cx.simulate_mouse_down(first, MouseButton::Middle, Modifiers::default());
        cx.simulate_mouse_up(first, MouseButton::Middle, Modifiers::default());
        draw(cx);
        cx.update(|window, cx| workspace.read(cx).tabs[0].focus.clone().focus(window, cx));
        cx.simulate_keystrokes("right");
        workspace.read_with(cx, |workspace, cx| {
            assert_eq!(workspace.tabs.len(), 2);
            assert_eq!(workspace.tabs[workspace.active].id, 2);
            assert!(workspace.session_ids(cx).is_empty());
        });
        cx.simulate_keystrokes("home");
        assert_eq!(
            workspace.read_with(cx, |workspace, _| workspace.tabs[workspace.active].id),
            1
        );
    }

    #[gpui::test]
    fn context_menu_targets_inactive_tab_without_switching_sessions(cx: &mut TestAppContext) {
        let (workspace, cx) = visual_fixture(cx);
        let second = cx.debug_bounds("terminal-tab-1").unwrap().center();
        cx.simulate_mouse_down(second, MouseButton::Right, Modifiers::default());
        cx.simulate_mouse_up(second, MouseButton::Right, Modifiers::default());
        draw(cx);
        workspace.read_with(cx, |workspace, cx| {
            assert_eq!(workspace.active, 0);
            assert_eq!(workspace.context_tab, Some(1));
            assert_eq!(
                workspace.tab_menu_state(1, cx),
                Some((1, false, false, true))
            );
            assert!(workspace.session_ids(cx).is_empty());
        });
        cx.simulate_keystrokes("escape");
    }

    /// Mark a shell ready long enough ago that its exit is a normal one.
    fn used(
        workspace: &Entity<Workspace>,
        view: &Entity<TerminalView>,
        cx: &mut VisualTestContext,
    ) {
        view.update(cx, |v, cx| {
            v.status.ready();
            cx.notify();
        });
        cx.run_until_parked();
        workspace.update(cx, |w, _| {
            let tab = w.tabs.iter_mut().find(|t| t.view == *view).unwrap();
            tab.exit.elapse_startup();
        });
    }

    #[gpui::test]
    fn shell_exit_closes_only_finished_tabs_including_the_last(cx: &mut TestAppContext) {
        let (workspace, cx) = visual_fixture(cx);
        let original = workspace.read_with(cx, |w, _| {
            w.tabs.iter().map(|t| t.view.clone()).collect::<Vec<_>>()
        });
        for view in &original {
            used(&workspace, view, cx);
        }
        workspace.update_in(cx, |w, window, cx| {
            w.active = 1;
            w.focus_active(window, cx);
            w.tabs[0].view.update(cx, |v, cx| {
                v.status.exited(Some(0));
                cx.notify();
            });
        });
        draw(cx);
        workspace.read_with(cx, |w, cx| {
            assert_eq!(w.tabs.len(), 2);
            assert_eq!(
                w.tabs[w.active].view, original[1],
                "background exit keeps active session"
            );
            assert_eq!(w.next, 3, "exiting must not create a replacement shell");
            assert!(w.session_ids(cx).is_empty());
        });
        original[1].update(cx, |v, cx| {
            v.status.exited(Some(1));
            cx.notify();
        });
        draw(cx);
        workspace.read_with(cx, |w, _| {
            assert_eq!(w.tabs.len(), 1);
            assert_eq!(w.tabs[0].view, original[2]);
        });
        original[2].update(cx, |v, cx| {
            v.status.failed("connection lost");
            cx.notify();
        });
        draw(cx);
        assert_eq!(
            workspace.read_with(cx, |w, _| w.tabs.len()),
            1,
            "failure stays retryable"
        );
        original[2].update(cx, |v, cx| {
            v.status.exited(None);
            cx.notify();
        });
        draw(cx);
        assert!(workspace.read_with(cx, |w, _| w.tabs.is_empty()));
        assert_eq!(workspace.read_with(cx, |w, _| w.next), 3);
        let add = cx.update(|window, _| {
            gpui::base::test_support::snapshots(window)
                .iter()
                .find(|e| e.label() == Some("New terminal"))
                .unwrap()
                .bounds()
                .center()
        });
        cx.simulate_click(add, Default::default());
        draw(cx);
        assert_eq!(
            workspace.read_with(cx, |w, _| w.tabs.len()),
            1,
            "new terminal works after last exit"
        );
    }

    #[gpui::test]
    fn startup_exits_keep_their_tab_and_show_the_exit_status(cx: &mut TestAppContext) {
        let (workspace, cx) = visual_fixture(cx);
        let original = workspace.read_with(cx, |w, _| {
            w.tabs.iter().map(|t| t.view.clone()).collect::<Vec<_>>()
        });
        // A nologin shell becomes ready and exits at once; another never got ready.
        original[0].update(cx, |v, cx| {
            v.status.ready();
            cx.notify();
        });
        cx.run_until_parked();
        original[0].update(cx, |v, cx| {
            v.status.exited(Some(1));
            cx.notify();
        });
        original[1].update(cx, |v, cx| {
            v.status.exited(None);
            cx.notify();
        });
        draw(cx);
        let labels = |cx: &mut VisualTestContext| {
            cx.update(|window, _| {
                gpui::base::test_support::snapshots(window)
                    .iter()
                    .filter_map(|e| e.label().map(str::to_owned))
                    .collect::<Vec<_>>()
            })
        };
        assert_eq!(workspace.read_with(cx, |w, _| w.tabs.len()), 3);
        let shown = labels(cx);
        assert!(shown.iter().any(|l| l == "Terminal, Shell exited · 1"));
        assert!(shown.iter().any(|l| l == "Terminal 2, Shell closed"));
        // The decision is made at exit; time passing does not close the tab later.
        workspace.update(cx, |w, _| {
            for tab in &mut w.tabs {
                tab.exit.elapse_startup();
            }
        });
        draw(cx);
        assert_eq!(workspace.read_with(cx, |w, _| w.tabs.len()), 3);
        // Reconnecting starts over: a later exit after use closes as usual.
        original[0].update(cx, |v, cx| {
            v.status.begin();
            cx.notify();
        });
        cx.run_until_parked();
        used(&workspace, &original[0], cx);
        original[0].update(cx, |v, cx| {
            v.status.exited(Some(0));
            cx.notify();
        });
        draw(cx);
        workspace.read_with(cx, |w, _| {
            assert_eq!(w.tabs.len(), 2);
            assert!(w.tabs.iter().all(|t| t.view != original[0]));
        });
    }

    #[gpui::test]
    fn explicit_background_reconnect_starts_without_selecting_or_stealing_focus(
        cx: &mut TestAppContext,
    ) {
        let (workspace, cx) = visual_fixture(cx);
        cx.update(|window, cx| {
            let active_focus = workspace.read(cx).tabs[0].view.read(cx).focus.clone();
            active_focus.focus(window, cx);
            workspace.update(cx, |workspace, cx| workspace.reconnect(1, window, cx));
            assert!(active_focus.is_focused(window));
            let workspace = workspace.read(cx);
            assert_eq!(workspace.active, 0);
            let background = workspace.tabs[1].view.read(cx);
            // The invalid fixture ID fails before any IPC, proving an attempt
            // happened now, rather than leaving it queued for a later render.
            assert_eq!(
                background.status.error(),
                Some("Invalid terminal server ID")
            );
            assert!(!background.active());
            assert!(workspace.tabs[0].view.read(cx).status.error().is_none());
        });
        draw(cx);
        workspace.read_with(cx, |workspace, cx| {
            assert_eq!(workspace.active, 0);
            assert!(workspace.session_ids(cx).is_empty());
        });
    }

    #[gpui::test]
    fn scope_reset_leaves_unselected_tabs_idle_until_opened(cx: &mut TestAppContext) {
        let (workspace, cx) = visual_fixture(cx);
        workspace.update(cx, |workspace, cx| {
            workspace.tabs[1].custom = Some("Background work".into());
            workspace.sync_connection(
                Some(crate::snapshot_fixture::scope("replacement")),
                "fixture@invalid".into(),
                cx,
            );
            for tab in &workspace.tabs {
                let view = tab.view.read(cx);
                assert_eq!(view.status.label(), "Disconnected");
                assert_eq!(view.status.cancel_label(), None);
                assert!(!view.active());
            }
        });
        draw(cx);
        workspace.read_with(cx, |workspace, cx| {
            assert_eq!(workspace.active, 0);
            assert_eq!(
                workspace.tabs[0].view.read(cx).status.error(),
                Some("Invalid terminal server ID")
            );
            assert_eq!(workspace.tabs[1].custom.as_deref(), Some("Background work"));
            for tab in &workspace.tabs[1..] {
                assert_eq!(tab.view.read(cx).status.label(), "Disconnected");
                assert!(!tab.view.read(cx).active());
            }
        });
    }
}
