use crate::{terminal_status::Phase, terminal_view::TerminalView, *};
use newport_client::terminal_command::CommandDraft;
use std::time::{Duration, Instant};

use newport_domain::terminal::MAX_TERMINALS;

struct TerminalTab {
    id: usize,
    focus: FocusHandle,
    custom: Option<String>,
    view: Entity<TerminalView>,
    exit: ExitPolicy,
    _observer: Subscription,
    _links: Subscription,
}

/// A shell that ends this soon after becoming ready failed to start.
const STARTUP_EXIT: Duration = Duration::from_secs(2);

/// Exits after use close their tab. A startup exit (nologin, a profile that
/// exits) keeps it, so the output and exit status explain what happened.
#[derive(Default)]
struct ExitPolicy {
    ready_at: Option<Instant>,
    keep: bool,
}
impl ExitPolicy {
    fn observe(&mut self, phase: &Phase) {
        match phase {
            Phase::Connecting => *self = Self::default(),
            Phase::Connected => {
                *self = Self {
                    ready_at: Some(Instant::now()),
                    keep: false,
                }
            }
            Phase::Exited(_) => {
                self.keep = self.ready_at.is_none_or(|at| at.elapsed() < STARTUP_EXIT);
            }
            _ => {}
        }
    }
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
    connection: Value,
    tabs: Vec<TerminalTab>,
    active: usize,
    next: usize,
    rename: Option<Rename>,
    context_tab: Option<usize>,
    tab_scroll: ScrollHandle,
    revealed_tab: Option<(usize, usize)>,
}

impl Workspace {
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
    pub fn sync_connection(&mut self, connection: Value, endpoint: String, cx: &mut Context<Self>) {
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
                tab.custom = custom_name(rename.input.read(cx).value().as_ref());
            }
            if restore_focus {
                tab.focus.focus(window, cx);
            }
        }
        cx.notify();
    }
    pub(super) fn reset_name(&mut self, id: usize, cx: &mut Context<Self>) {
        if let Some(tab) = self.tabs.iter_mut().find(|tab| tab.id == id) {
            tab.custom = None;
            cx.notify();
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
        connection: Value,
        endpoint: String,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut workspace = Self::empty(server, connection, endpoint, cx);
        workspace.add(cx);
        workspace
    }

    pub(super) fn for_project(
        server: String,
        connection: Value,
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
        connection: Value,
        endpoint: String,
        draft: CommandDraft,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut workspace = Self::empty(server, connection, endpoint, cx);
        workspace.add_command(draft, "Service command", cx);
        workspace
    }

    pub(super) fn with_shell(
        server: String,
        connection: Value,
        endpoint: String,
        draft: CommandDraft,
        title: String,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut workspace = Self::empty(server, connection, endpoint, cx);
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

    fn add_shell(&mut self, draft: CommandDraft, title: String, cx: &mut Context<Self>) {
        self.add(cx);
        let tab = &mut self.tabs[self.active];
        tab.custom = Some(custom_name(&title).unwrap_or_else(|| "Container shell".into()));
        assert!(tab.view.update(cx, |view, _| view.prepare_command(draft)));
        cx.notify();
    }

    fn empty(server: String, connection: Value, endpoint: String, cx: &mut Context<Self>) -> Self {
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
            tab_scroll: ScrollHandle::new(),
            revealed_tab: None,
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
        if let Some(draft) = draft {
            // A fresh view always takes its one draft.
            assert!(view.update(cx, |view, _| view.prepare_command(draft)));
        }
        // Output, selection and pointer changes repaint the terminal itself.
        // Only presentation changes need to rebuild the surrounding tab bar.
        let id = self.next;
        let mut previous = view.read(cx).tab_presentation();
        let observer = cx.observe(&view, move |this, view, cx| {
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
        self.tabs.push(TerminalTab {
            id,
            focus: cx.focus_handle(),
            custom: None,
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
        self.tabs[self.active].custom = Some(title.into());
        cx.notify();
    }

    pub fn session_ids(&self, cx: &App) -> Vec<uuid::Uuid> {
        self.tabs
            .iter()
            .filter_map(|tab| tab.view.read(cx).session_id())
            .collect()
    }

    pub(super) fn close(
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
        if let Some(index) = self.tabs.iter().position(|tab| tab.id == id) {
            if self.rename.as_ref().is_some_and(|rename| rename.id == id) {
                self.rename = None;
            }
            self.tabs[index].view.update(cx, |view, _| view.close());
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
            if tab.exit.keep || !matches!(tab.view.read(cx).status.phase, Phase::Exited(_)) {
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
fn automatic_tab_title(title: &str, id: usize) -> String {
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
    if id == 0 {
        base.to_owned()
    } else {
        format!("{base} {}", id + 1)
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
        for (index, tab) in self.tabs.iter().enumerate() {
            if index != self.active {
                tab.view.update(cx, |view, cx| view.deactivate(window, cx));
            }
        }
        let current = self.tabs.get(self.active).map(|tab| tab.view.clone());
        let state = current.as_ref().map(|view| view.read(cx).status.clone());
        let transport_wait = crate::connectivity::for_scope(&self.connection, cx)
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
        let selected = (self.active, self.tabs.len());
        if self.revealed_tab != Some(selected) {
            self.revealed_tab = Some(selected);
            self.tab_scroll.scroll_to_item(self.active);
        }
        let menu_view = cx.entity().downgrade();
        // Only windowed immersive mode shares this row with the native Mac
        // window controls. Fullscreen hides those controls.
        let inset = self.immersive && cfg!(target_os = "macos") && !window.is_fullscreen();
        div()
            .size_full()
            .relative()
            .min_w_0()
            .min_h_0()
            .flex()
            .flex_col()
            .child(
                div()
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
                            .children(self.tabs.iter().enumerate().map(|(index, tab)| {
                                let id = tab.id;
                                if let Some(rename) =
                                    self.rename.as_ref().filter(|rename| rename.id == id)
                                {
                                    return session_tab(id, index == self.active, cx)
                                        .unfocusable()
                                        .when(index == 0 && inset, |tab| tab.border_l_1())
                                        .set_position(index + 1, self.tabs.len())
                                        .accessibility_label(format!("Rename Terminal {}", id + 1))
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
                                                .aria_label(format!("Rename Terminal {}", id + 1)),
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
                                    .unwrap_or_else(|| automatic_tab_title(&remote_title, id));
                                let startup_exit = {
                                    let status = &tab.view.read(cx).status;
                                    (tab.exit.keep && matches!(status.phase, Phase::Exited(_)))
                                        .then(|| status.label())
                                };
                                let hint = startup_exit.clone().or_else(|| {
                                    (tab.custom.is_none() && title != remote_title && remote_title.contains('@') && remote_title.contains(':'))
                                        .then(|| remote_title.clone())
                                });
                                session_tab(id, index == self.active, cx)
                                    // Inset beside the window controls, the first tab
                                    // needs its own leading edge.
                                    .when(index == 0 && inset, |tab| tab.border_l_1())
                                    .set_position(index + 1, self.tabs.len())
                                    .accessibility_label(match &startup_exit {
                                        Some(exit) => format!("{title}, {exit}"),
                                        None => title.clone(),
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
                                    })
                            })),
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
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.finish_rename(true, false, window, cx);
                                this.add(cx);
                                if let Some(tab) = this.tabs.get(this.active) {
                                    let focus = tab.view.read(cx).focus.clone();
                                    focus.focus(window, cx);
                                }
                            })),
                    )
                    .child(
                        // Native title-bar hit testing runs before button clicks on
                        // Windows. Only empty space may drag the window; marking the
                        // whole toolbar also captures the restore and tab controls.
                        div()
                            .debug_selector(|| "terminal-drag-region".into())
                            .flex_1()
                            .h_full()
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
                    }),
            )
            .when_some(error, |root, error| {
                root.child(
                    crate::controls::alert("terminal-error", error, cx)
                        .px(px(12.))
                        .py(px(8.)),
                )
            })
            .when_some(current.clone(), |root, current| {
                root.child(
                    div()
                        .relative()
                        .flex_1()
                        .min_h_0()
                        .px(px(10.))
                        .py(px(8.))
                        .child(current)
                        .when(
                            state.as_ref().is_some_and(|state| !state.started),
                            |frame| {
                                let connecting = state
                                    .as_ref()
                                    .is_some_and(|state| state.cancel_label().is_some());
                                let failed =
                                    state.as_ref().is_some_and(|state| state.error().is_some());
                                let exited = state
                                    .as_ref()
                                    .filter(|state| matches!(state.phase, Phase::Exited(_)));
                                let title: SharedString = if connecting {
                                    "Connecting…".into()
                                } else if failed {
                                    "Connection failed".into()
                                } else if let Some(state) = exited {
                                    state.label().into()
                                } else {
                                    "Open a remote shell".into()
                                };
                                frame.child(
                                    // The endpoint reads as the description: it is
                                    // the shell this tab is about to open.
                                    empty::pane(
                                        IconName::Terminal,
                                        title,
                                        self.endpoint.clone(),
                                        cx,
                                    )
                                    .absolute()
                                    .inset_0()
                                    .bg(cx.theme().background),
                                )
                            },
                        ),
                )
            })
            .when(current.is_none(), |root| {
                root.child(empty::pane(
                    IconName::Terminal,
                    "No terminals open",
                    "Open a terminal with +.",
                    cx,
                ))
            })
    }
}

#[cfg(test)]
mod tests {
    use super::{
        CommandDraft, MAX_TERMINALS, STARTUP_EXIT, Workspace, custom_name, selected_after_close,
        tab_destination,
    };
    use crate::terminal_view::TerminalView;
    use gpui_kit::{
        self as gpui, AppContext, Entity, Modifiers, MouseButton, MouseDownEvent, TestAppContext,
        VisualTestContext, px, size,
    };
    use serde_json::json;

    fn init(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui::component::init(cx);
            crate::theme::install(false, cx);
        });
    }

    #[gpui::test]
    fn project_shells_are_lazy_and_keep_their_worktree_scope(cx: &mut TestAppContext) {
        init(cx);
        cx.update(|cx| {
            let workspace = cx.new(|cx| {
                Workspace::for_project(
                    "fixture".into(),
                    json!(["scope"]),
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
                    json!(["scope"]),
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

    #[gpui::test]
    fn first_service_command_has_one_dedicated_unstarted_tab(cx: &mut TestAppContext) {
        init(cx);
        cx.update(|cx| {
            let workspace = cx.new(|cx| {
                Workspace::with_command(
                    "fixture".into(),
                    json!({"scope":"fixture"}),
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
                    json!({"scope":"fixture"}),
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
            super::automatic_tab_title("ruiyang@azure:~/Projects/marketplace-service", 0),
            "marketplace-service"
        );
        assert_eq!(
            super::automatic_tab_title("ruiyang@azure:~/Projects/marketplace-service", 2),
            "marketplace-service 3"
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
                    json!({"scope":"fixture"}),
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
                    json!({}),
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

    fn draw(cx: &mut VisualTestContext) {
        for _ in 0..3 {
            cx.run_until_parked();
            cx.update(|window, cx| window.draw(cx).clear(cx));
        }
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
        let workspace = owner.read_with(cx, |owner, _| owner.terminals["fixture"].clone());
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
    fn terminal_tabs_stay_inside_the_row_above_its_border(cx: &mut TestAppContext) {
        let (workspace, cx) = visual_fixture(cx);
        workspace.update(cx, |w, cx| {
            for tab in &w.tabs {
                tab.view.update(cx, |v, cx| {
                    v.title = "dev@host:~/Projects/marketplace-service".into();
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
            gpui::base::test_support::snapshots(window)
                .into_iter()
                .find(|node| {
                    node.label() == Some("Rename Terminal 1")
                        && node.role() == Some(gpui::Role::TextInput)
                })
                .expect("inline rename input")
                .bounds()
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
            let ready = tab.exit.ready_at.expect("observed ready shell");
            tab.exit.ready_at = Some(ready - STARTUP_EXIT * 2);
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
                tab.exit.ready_at = tab.exit.ready_at.map(|at| at - STARTUP_EXIT * 2);
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
            workspace.sync_connection(json!({"scope":"replacement"}), "fixture@invalid".into(), cx);
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
