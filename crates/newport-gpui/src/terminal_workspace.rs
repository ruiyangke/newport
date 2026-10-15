use crate::{terminal_view::TerminalView, *};
use gpui_kit::component::tab::Tab;
use newport_client::terminal_command::CommandDraft;

const MAX_TERMINALS: usize = 8;

struct TerminalTab {
    id: usize,
    focus: FocusHandle,
    custom: Option<String>,
    view: Entity<TerminalView>,
    _observer: Subscription,
    _links: Subscription,
}

struct Rename {
    id: usize,
    input: Entity<InputState>,
    _subscription: Subscription,
    _blur: Subscription,
}

pub struct Workspace {
    endpoint: String,
    immersive: bool,
    server: String,
    connection: Value,
    tabs: Vec<TerminalTab>,
    active: usize,
    next: usize,
    rename: Option<Rename>,
    context_tab: Option<usize>,
}

impl Workspace {
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
        let mut workspace = Self::empty(server, connection, endpoint);
        workspace.add(cx);
        workspace
    }

    /// A first handover is the initial tab, not an extra tab beside a blank shell.
    pub(super) fn with_command(
        server: String,
        connection: Value,
        endpoint: String,
        draft: CommandDraft,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut workspace = Self::empty(server, connection, endpoint);
        workspace.add_command(draft, cx);
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
        let mut workspace = Self::empty(server, connection, endpoint);
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

    fn empty(server: String, connection: Value, endpoint: String) -> Self {
        Self {
            endpoint,
            immersive: false,
            server,
            connection,
            tabs: vec![],
            active: 0,
            next: 0,
            rename: None,
            context_tab: None,
        }
    }

    fn add(&mut self, cx: &mut Context<Self>) {
        if self.tabs.len() >= MAX_TERMINALS {
            return;
        }
        let view = cx.new(|cx| TerminalView::new(self.server.clone(), cx));
        // Output, selection and pointer changes repaint the terminal itself.
        // Only presentation changes need to rebuild the surrounding tab bar.
        let mut previous = view.read(cx).tab_presentation();
        let observer = cx.observe(&view, move |_, view, cx| {
            let next = view.read(cx).tab_presentation();
            if next != previous {
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
            id: self.next,
            focus: cx.focus_handle(),
            custom: None,
            view,
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
        self.add_command(draft, cx);
        Ok(())
    }

    fn add_command(&mut self, draft: CommandDraft, cx: &mut Context<Self>) {
        self.add(cx);
        let tab = &mut self.tabs[self.active];
        tab.custom = Some("Service command".into());
        assert!(tab.view.update(cx, |view, _| view.prepare_command(draft)));
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
        // Match the reference: the last session remains available to reconnect.
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
        for (index, tab) in self.tabs.iter().enumerate() {
            if index != self.active {
                tab.view.update(cx, |view, cx| view.deactivate(window, cx));
            }
        }
        let current = self.tabs.get(self.active).map(|tab| tab.view.clone());
        let state = current.as_ref().map(|view| view.read(cx).status.clone());
        let error = current.as_ref().and_then(|view| {
            let view = view.read(cx);
            view.status
                .error()
                .map(str::to_owned)
                .or_else(|| view.input_error.clone())
        });
        let menu_view = cx.entity().downgrade();
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
                    .h(px(36.))
                    .flex_shrink_0()
                    .px(px(8.))
                    // Only windowed immersive mode shares this row with the
                    // native Mac window controls. Fullscreen hides those controls.
                    .when(self.immersive && cfg!(target_os = "macos") && !window.is_fullscreen(), |bar| {
                        bar.pl(px(96.))
                    })
                    .items_center()
                    .border_b_1()
                    .border_color(cx.theme().border.opacity(0.65))
                    .capture_any_mouse_down(cx.listener(|this, event: &MouseDownEvent, _, _| {
                        if event.button == MouseButton::Right {
                            this.context_tab = None;
                        }
                    }))
                    .child(
                        div()
                            .debug_selector(|| "terminal-tab-bar".into())
                            .min_w_0()
                            .child(crate::controls::tabs("terminal-tabs")
                            .subtle()
                            // Kit's subtle bar defaults to 38px. Leave the
                            // row's bottom border outside its painted bounds.
                            .h(px(35.))
                            .min_h(px(35.))
                            .max_h(px(35.))
                            .accessibility_label("Terminal sessions")
                            .bg(cx.theme().background)
                            .min_w_0()
                            .max_width(px(180.))
                            .selected_index(self.active)
                            .children(self.tabs.iter().enumerate().map(|(index, tab)| {
                                let id = tab.id;
                                if let Some(rename) =
                                    self.rename.as_ref().filter(|rename| rename.id == id)
                                {
                                    return Tab::new().quiet()
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
                                let title = tab
                                    .custom
                                    .clone()
                                    .unwrap_or_else(|| tab.view.read(cx).title.clone());
                                let group = SharedString::from(format!("terminal-tab-{id}"));
                                Tab::new().quiet()
                                    .group(group.clone())
                                    .debug_selector(move || format!("terminal-tab-{id}"))
                                    .on_mouse_down(
                                        MouseButton::Right,
                                        cx.listener(move |this, _, _, cx| {
                                            this.context_tab = Some(id);
                                            cx.notify();
                                        }),
                                    )
                                    .on_mouse_up(
                                        MouseButton::Middle,
                                        cx.listener(move |this, _, window, cx| {
                                            this.close(id, false, window, cx);
                                            cx.stop_propagation();
                                        }),
                                    )
                                    .track_focus(&tab.focus.clone().tab_stop(index == self.active))
                                    .focus_visible(|style| {
                                        style.border_1().border_color(cx.theme().ring)
                                    })
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
                                    .label(if title.is_empty() {
                                        format!("Terminal {}", id + 1)
                                    } else {
                                        title
                                    })
                                    .when(self.tabs.len() > 1, |tab| {
                                        tab.suffix(
                                            crate::controls::compact_icon_button((
                                                "close-terminal",
                                                id,
                                            ))
                                            .icon(Icon::new(IconName::X).size(px(12.)))
                                            .w(px(20.)).min_w(px(20.)).max_w(px(20.))
                                            .h(px(20.)).min_h(px(20.)).max_h(px(20.))
                                            .ghost()
                                            .opacity(0.65)
                                            .group_hover(group.clone(), |style| style.opacity(1.))
                                            .focus(|style| style.opacity(1.))
                                            .accessibility_label("Close terminal")
                                            .on_click(cx.listener(
                                                move |this, event: &ClickEvent, window, cx| {
                                                    cx.stop_propagation();
                                                    this.close(
                                                        id,
                                                        matches!(event, ClickEvent::Keyboard(_)),
                                                        window,
                                                        cx,
                                                    );
                                                },
                                            )),
                                        )
                                    })
                            }))),
                    )
                    .child(
                        crate::controls::icon_button("new-terminal")
                            .icon(Icon::new(IconName::Plus).size(px(13.)).text_color(cx.theme().muted_foreground))
                            .ghost()
                            .accessibility_label("New terminal")
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
                                .aria_label(state.label())
                                .text_size(px(11.))
                                .text_color(if state.error().is_some() {
                                    cx.theme().danger
                                } else {
                                    cx.theme().muted_foreground
                                })
                                .when(state.steady() || state.cancel_label().is_some(), |status| {
                                    status.w_0().h_0().overflow_hidden()
                                })
                                .child(state.label()),
                        )
                    })
                    .child(
                        crate::controls::icon_button("terminal-immersive")
                            .icon(Icon::new(if self.immersive {
                                gpui_kit::assets::IconName::Minimize2
                            } else {
                                gpui_kit::assets::IconName::Maximize2
                            }).size(px(13.)).text_color(cx.theme().muted_foreground))
                            .ghost()
                            .accessibility_label(if self.immersive {
                                "Exit immersive mode"
                            } else {
                                "Enter immersive mode"
                            })
                            .on_click(|_, window, cx| {
                                window.dispatch_action(Box::new(ToggleTerminalImmersive), cx);
                            }),
                    )
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
                    div()
                        .id("terminal-error")
                        .role(Role::Alert)
                        .aria_label(error.clone())
                        .px(px(12.))
                        .py(px(9.))
                        .text_size(px(12.))
                        .text_color(cx.theme().danger)
                        .bg(cx.theme().danger.opacity(0.06))
                        .border_b_1()
                        .border_color(cx.theme().border)
                        .child(error),
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
                                let title = if connecting {
                                    "Connecting…"
                                } else if failed {
                                    "Connection failed"
                                } else {
                                    "Open a remote shell"
                                };
                                frame.child(
                                    div()
                                        .absolute()
                                        .inset_0()
                                        .bg(cx.theme().background)
                                        .flex()
                                        .flex_col()
                                        .items_center()
                                        .justify_center()
                                        .gap(px(6.))
                                        .p(px(24.))
                                        .child(
                                            Icon::new(IconName::Terminal)
                                                .size(px(28.))
                                                .text_color(cx.theme().muted_foreground),
                                        )
                                        .child(
                                            div()
                                                .id("terminal-welcome")
                                                .role(Role::Status)
                                                .aria_label(title)
                                                .text_size(px(15.))
                                                .font_weight(FontWeight::SEMIBOLD)
                                                .child(title),
                                        )
                                        .child(
                                            div()
                                                .font_family(cx.theme().mono_font_family.clone())
                                                .text_size(px(12.))
                                                .text_color(cx.theme().muted_foreground)
                                                .child(self.endpoint.clone()),
                                        ),
                                )
                            },
                        ),
                )
            })
            .when(current.is_none(), |root| {
                root.child(
                    div()
                        .flex_1()
                        .flex()
                        .items_center()
                        .justify_center()
                        .child("Open a terminal with +"),
                )
            })
    }
}

#[cfg(test)]
mod tests {
    use super::{
        CommandDraft, MAX_TERMINALS, Workspace, custom_name, selected_after_close, tab_destination,
    };
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
                        .any(|e| e.label() == Some("Hide sidebar"))
                });
                assert_eq!(sidebar_visible, !immersive);
            }
        }
    }

    #[gpui::test]
    fn terminal_tabs_stay_inside_the_row_above_its_border(cx: &mut TestAppContext) {
        let (workspace, cx) = visual_fixture(cx);
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
                assert_eq!(row.size.height, px(36.));
                assert!(bar.top() >= row.top(), "tab bar overlaps frame border");
                assert!(bar.bottom() <= row.bottom() - px(1.));
                let tab = cx
                    .debug_bounds(["terminal-tab-0", "terminal-tab-1", "terminal-tab-2"][active])
                    .unwrap();
                assert!(tab.top() >= bar.top());
                assert!(tab.bottom() <= bar.bottom());
            }
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
            gpui::base::test_support::snapshots(window)
                .into_iter()
                .find(|node| node.label() == Some("Rename Terminal 1"))
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
