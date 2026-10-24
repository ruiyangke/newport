//! Kit's base tabs provide vertical-tab semantics without imposing horizontal styling.
use super::*;

/// The selected row's edge marker. Callers place it on the full-width row,
/// never inside a control's padded content.
pub(super) fn selection_marker(cx: &App) -> Div {
    div()
        .absolute()
        .left_0()
        .top_0()
        .bottom_0()
        .w(px(1.))
        .bg(cx.theme().muted_foreground)
}

pub(super) fn icon_color(selected: bool, cx: &App) -> Hsla {
    if selected {
        cx.theme().foreground
    } else {
        cx.theme().muted_foreground
    }
}

/// Settings is a page like the navigation rows, so it shares their surfaces
/// and label colour rather than a dimmer action style.
pub(super) fn sidebar_page_style(
    selected: bool,
    cx: &App,
) -> gpui_kit::component::button::ButtonCustomVariant {
    let selection = theme::sidebar_selection(cx.theme().is_dark());
    gpui_kit::component::button::ButtonCustomVariant::new(cx)
        .foreground(theme::sidebar_foreground(cx.theme().is_dark()))
        .color(if selected {
            selection
        } else {
            cx.theme().transparent
        })
        .hover(if selected {
            selection
        } else {
            theme::row_hover(cx)
        })
        .active(selection)
}
use gpui_kit::base::{Tab, Tabs};

#[derive(Action, Clone, PartialEq, serde::Deserialize)]
#[action(namespace = newport, no_json)]
pub(super) struct OpenView {
    pub page: Page,
}

/// Selects a tab of the current page by number: terminal tabs, or the
/// project editor's.
#[derive(Action, Clone, PartialEq, serde::Deserialize)]
#[action(namespace = newport, no_json)]
pub(super) struct SelectTab {
    pub number: usize,
}

/// Tab `number` among `count`: 1–8 by position, 9 the last, as browsers and
/// terminals number tabs.
pub(super) fn tab_index(number: usize, count: usize) -> Option<usize> {
    match number {
        9 if count > 0 => Some(count - 1),
        1..=8 if number <= count => Some(number - 1),
        _ => None,
    }
}

pub(super) fn install_desktop_commands(cx: &mut App) {
    use gpui_kit::component::input::{Copy, Cut, Paste, Redo, SelectAll, Undo};
    // About is an application command, including when a native panel rather
    // than a GPUI view owns keyboard focus.
    #[cfg(target_os = "macos")]
    cx.on_action(|_: &About, _| about::show());
    #[cfg(target_os = "macos")]
    {
        cx.on_action(|_: &HideApp, cx| cx.hide());
        cx.on_action(|_: &HideOthers, cx| cx.hide_other_apps());
        cx.on_action(|_: &ShowAll, cx| cx.unhide_other_apps());
    }
    install_workspace_shortcuts(cx);
    let mut items = Vec::new();
    for (page, _) in views(cx) {
        items.push(MenuItem::action(page.title(), OpenView { page }));
    }
    items.push(MenuItem::separator());
    items.push(MenuItem::action("Show/hide sidebar", ToggleSidebar));
    items.push(MenuItem::separator());
    items.push(MenuItem::action("Next tab", crate::project_editor::NextTab));
    items.push(MenuItem::action(
        "Previous tab",
        crate::project_editor::PreviousTab,
    ));
    items.push(MenuItem::action(
        "Show/hide project terminal",
        crate::project_editor::ToggleTerminal,
    ));
    let mut app_items = vec![
        MenuItem::action("About Newport", About),
        MenuItem::separator(),
        MenuItem::action("Settings…", OpenSettings),
        MenuItem::separator(),
    ];
    // GPUI's Windows hide-other-apps operations are unimplemented. These are
    // macOS application-menu conventions, not cross-platform commands.
    #[cfg(target_os = "macos")]
    app_items.extend([
        MenuItem::action("Hide Newport", HideApp),
        MenuItem::action("Hide others", HideOthers),
        MenuItem::action("Show all", ShowAll),
        MenuItem::separator(),
    ]);
    app_items.push(MenuItem::action("Quit Newport", Quit));
    cx.set_menus(vec![
        Menu {
            name: "Newport".into(),
            disabled: false,
            items: app_items,
        },
        Menu {
            name: "File".into(),
            disabled: false,
            items: vec![
                MenuItem::action("Open file…", crate::project_editor::QuickOpen),
                MenuItem::action("Focus file explorer", crate::project_editor::FocusExplorer),
                MenuItem::action("Save file", crate::project_editor::SaveFile),
                MenuItem::action("Close tab", crate::project_editor::CloseTab),
                MenuItem::separator(),
                MenuItem::action("Close window", CloseWindow),
            ],
        },
        Menu {
            name: "Server".into(),
            disabled: false,
            items: vec![
                MenuItem::action("Add server…", NewServer),
                MenuItem::action("Edit server…", EditServer),
                MenuItem::action("Test connection", TestServer),
            ],
        },
        Menu {
            name: "Edit".into(),
            disabled: false,
            items: vec![
                MenuItem::os_action("Undo", Undo, OsAction::Undo),
                MenuItem::os_action("Redo", Redo, OsAction::Redo),
                MenuItem::separator(),
                MenuItem::os_action("Cut", Cut, OsAction::Cut),
                MenuItem::os_action("Copy", Copy, OsAction::Copy),
                MenuItem::os_action("Paste", Paste, OsAction::Paste),
                MenuItem::os_action("Select all", SelectAll, OsAction::SelectAll),
            ],
        },
        Menu {
            name: "Workspace".into(),
            disabled: false,
            items,
        },
        Menu {
            name: "View".into(),
            disabled: false,
            // AppKit adds and validates its standard Enter/Exit Full Screen
            // item in this menu. A custom action would duplicate that item.
            items: if cfg!(target_os = "macos") {
                vec![]
            } else {
                vec![MenuItem::action("Toggle full screen", ToggleFullScreen)]
            },
        },
        Menu {
            name: "Window".into(),
            disabled: false,
            items: vec![
                MenuItem::action("Minimize", MinimizeWindow),
                MenuItem::action("Zoom", ZoomWindow),
            ],
        },
    ]);
}

/// No application command shortcuts by default. Terminal Find is the sole
/// exception; copy/paste is handled by the terminal's input surface. Keep
/// ordinary control navigation and text editing scoped to their own widgets.
pub(super) fn install_workspace_shortcuts(cx: &mut App) {
    let primary = if cfg!(target_os = "macos") {
        "cmd"
    } else {
        "ctrl"
    };
    cx.bind_keys([
        KeyBinding::new(
            &format!("{primary}-shift-f"),
            FindInTerminal,
            Some("Terminal"),
        ),
        // Disable inherited Root bindings so raw input reaches the terminal,
        // even if another view retains a text selection.
        KeyBinding::new("tab", gpui::NoAction, Some("Terminal")),
        KeyBinding::new("shift-tab", gpui::NoAction, Some("Terminal")),
        KeyBinding::new(&format!("{primary}-c"), gpui::NoAction, Some("Terminal")),
    ]);
    // Kit installs a global inspector shortcut in debug builds. Newport has
    // no default binding for it, including when a terminal owns focus.
    let inspector = if cfg!(target_os = "macos") {
        "cmd-alt-i"
    } else {
        "ctrl-shift-i"
    };
    cx.bind_keys([KeyBinding::new(inspector, gpui::NoAction, None)]);
}
const VIEWS: [(Page, IconName); 8] = [
    (Page::Overview, IconName::Activity),
    (Page::Ports, IconName::Link),
    (Page::Services, IconName::Layers),
    (Page::Containers, IconName::Box),
    (Page::Terminal, IconName::Terminal),
    (Page::Files, IconName::Folder),
    (Page::Projects, IconName::FolderCode),
    (Page::Integration, IconName::Plug),
];
/// Sidebar views for this build. Remote apps appears only in builds that
/// include it, after the interactive views and before Integrations.
pub(super) fn views(cx: &App) -> Vec<(Page, IconName)> {
    let mut views = VIEWS.to_vec();
    if crate::remote_apps::available(cx) {
        let integration = views.len() - 1;
        views.insert(integration, (Page::RemoteApps, IconName::LayoutGrid));
    }
    views
}
impl Newport {
    /// Tab `number` of the current page, if it has tabs: the terminal's, or
    /// the open project's editor. Other pages have none and ignore it.
    pub(super) fn select_tab(
        &mut self,
        number: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if window.has_active_dialog(cx) || self.server_form.is_some() {
            return;
        }
        match self.page {
            Page::Terminal => {
                if let Some(workspace) = self.terminals.get(&self.server).cloned() {
                    workspace.update(cx, |workspace, cx| workspace.select_tab(number, window, cx));
                }
            }
            Page::Projects => {
                if let Some(editor) = self.code_workspace() {
                    editor.update(cx, |editor, cx| editor.select_tab(number, window, cx));
                }
            }
            _ => {}
        }
    }
    /// One focus handle per sidebar view, including views that become
    /// available after the window opens, such as Remote apps.
    pub(super) fn sync_navigation_focus(&mut self, cx: &mut Context<Self>) {
        let count = views(cx).len();
        while self.navigation_focus.len() < count {
            self.navigation_focus.push(cx.focus_handle());
        }
    }
}
fn destination(index: usize, key: &str, count: usize) -> Option<usize> {
    if count == 0 {
        return None;
    }
    match key {
        "down" => Some((index + 1) % count),
        "up" => Some((index + count - 1) % count),
        "home" => Some(0),
        "end" => Some(count - 1),
        "enter" | "space" => Some(index),
        _ => None,
    }
}
impl Newport {
    pub(super) fn navigation(&self, compact: bool, cx: &mut Context<Self>) -> AnyElement {
        let tunnel_count = rows(&self.snapshot["config"]["tunnels"])
            .iter()
            .filter(|tunnel| text(tunnel, "serverId") == self.server)
            .count();
        let disabled = self.server.is_empty() || self.server_form.is_some() || self.modal_open;
        let views = views(cx);
        let tabs = views.clone();
        let current = views
            .iter()
            .position(|(page, _)| *page == self.page)
            .unwrap_or(0);
        Tabs::new("server-views")
            .aria_label("Server views")
            .aria_orientation(accesskit::Orientation::Vertical)
            .flex()
            .flex_col()
            .flex_shrink_0()
            .w_full()
            .children(tabs.into_iter().enumerate().flat_map(|(i, (page, icon))| {
                let separator = matches!(page, Page::Terminal | Page::Integration).then(|| {
                    div()
                        .id(("navigation-separator", i))
                        .role(accesskit::Role::Splitter)
                        .h(px(1.))
                        .flex_shrink_0()
                        .my(px(8.))
                        .mx(px(if compact { 8. } else { 12. }))
                        .bg(cx.theme().border)
                        .into_any_element()
                });
                let views = views.clone();
                let tab = Tab::new(("nav", i))
                    .accessibility_label(page.title())
                    .selected(page == self.page)
                    .set_position(i + 1, views.len())
                    .disabled(disabled)
                    // Kit blocks activation but does not expose the disabled
                    // flag. Amend the same node, preserving its label, selected
                    // state and position rather than creating a second tab.
                    .a11y_synthetic_children(move |builder| {
                        if disabled {
                            builder.parent_node().set_disabled();
                        }
                    })
                    // A view that appeared after startup gets its handle on the next tick.
                    .when_some(self.navigation_focus.get(i), |tab, focus| {
                        tab.track_focus(focus)
                    })
                    .tab_stop(!disabled && i == current)
                    .w_full()
                    .relative()
                    .h(px(if compact { 40. } else { 36. }))
                    .flex_shrink_0()
                    .px(px(if compact { 0. } else { 17. }))
                    .rounded(px(crate::ui::RADIUS))
                    .text_size(px(crate::ui::FONT_SIZE))
                    .font_weight(FontWeight::NORMAL)
                    .justify_start()
                    .when(page == self.page, |tab| {
                        tab.bg(theme::sidebar_selection(cx.theme().is_dark()))
                            .font_weight(FontWeight::MEDIUM)
                            .child(selection_marker(cx))
                    })
                    .when(!disabled, |tab| {
                        tab.hover(|style| {
                            style.bg(if page == self.page {
                                theme::sidebar_selection(cx.theme().is_dark())
                            } else {
                                theme::row_hover(cx)
                            })
                        })
                    })
                    .when(disabled, |tab| tab.opacity(0.5))
                    .focus_visible(|style| crate::controls::inset_focus_ring(style, cx))
                    .child(
                        div()
                            .w_full()
                            .flex()
                            .items_center()
                            .gap(px(11.))
                            .when(compact, |row| row.justify_center())
                            .child(
                                Icon::new(icon)
                                    .size(px(crate::controls::ICON_SIZE))
                                    .text_color(icon_color(page == self.page, cx)),
                            )
                            .when(!compact, |row| row.child(page.title()))
                            .when(!compact && page == Page::Ports && tunnel_count > 0, |row| {
                                row.child(div().flex_1())
                                    .child(crate::controls::tag(tunnel_count.to_string(), cx))
                            }),
                    )
                    .when(compact, |tab| tab.hover_hint(page.title()))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        if window.has_active_dialog(cx) {
                            return;
                        }
                        this.navigate(page, cx);
                        if let Some(focus) = this.navigation_focus.get(i) {
                            focus.focus(window, cx);
                        }
                    }))
                    .on_key_down(cx.listener(move |this, event: &KeyDownEvent, window, cx| {
                        if this.server.is_empty()
                            || this.server_form.is_some()
                            || window.has_active_dialog(cx)
                        {
                            return;
                        }
                        if event.keystroke.modifiers.control
                            || event.keystroke.modifiers.platform
                            || event.keystroke.modifiers.alt
                        {
                            return;
                        }
                        let Some(next) = destination(i, &event.keystroke.key, views.len()) else {
                            return;
                        };
                        cx.stop_propagation();
                        window.prevent_default();
                        this.navigate(views[next].0, cx);
                        if let Some(focus) = this.navigation_focus.get(next) {
                            focus.focus(window, cx);
                        }
                    }))
                    .into_any_element();
                separator.into_iter().chain(std::iter::once(tab))
            }))
            .into_any_element()
    }
}
#[cfg(test)]
mod tests {
    use super::destination;
    use gpui_kit as gpui;

    #[gpui::test]
    fn terminal_contexts_only_reserve_find_in_the_keymap(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            gpui_kit::component::init(cx);
            super::install_desktop_commands(cx);
            let keymap = cx.key_bindings();
            let keymap = keymap.borrow();
            for names in [
                vec!["Root", "Terminal"],
                vec!["Root", "ProjectEditor", "Terminal"],
            ] {
                let contexts: Vec<_> = names
                    .iter()
                    .map(|name| gpui::KeyContext::parse(name).unwrap())
                    .collect();
                // Include Root's Copy, the debug inspector, project bindings,
                // and both platform families so defaults cannot leak down.
                for key in [
                    "tab",
                    "shift-tab",
                    "ctrl-c",
                    "cmd-c",
                    "ctrl-f",
                    "cmd-f",
                    "ctrl-n",
                    "cmd-n",
                    "ctrl-p",
                    "cmd-p",
                    "ctrl-s",
                    "cmd-s",
                    "ctrl-w",
                    "cmd-w",
                    "ctrl-q",
                    "cmd-q",
                    "ctrl-r",
                    "cmd-r",
                    "ctrl-tab",
                    "ctrl-shift-tab",
                    "ctrl-`",
                    "ctrl-,",
                    "cmd-,",
                    "ctrl-shift-e",
                    "cmd-shift-e",
                    "ctrl-shift-t",
                    "cmd-shift-t",
                    "ctrl-shift-l",
                    "cmd-shift-l",
                    "ctrl-shift-i",
                    "cmd-alt-i",
                    "f5",
                    "f11",
                    "cmd-h",
                    "cmd-alt-h",
                    "cmd-shift-w",
                    "cmd-m",
                    "ctrl-cmd-f",
                    "cmd-alt-left",
                    "cmd-alt-right",
                    "cmd-alt-0",
                    "shift-pageup",
                    "shift-pagedown",
                    "ctrl-shift-pageup",
                    "alt-shift-pagedown",
                    "ctrl-insert",
                    "shift-insert",
                    "ctrl-alt-f",
                    "ctrl-alt-b",
                ]
                .into_iter()
                .map(String::from)
                .chain((1..=9).flat_map(|number| {
                    ["ctrl", "cmd", "ctrl-shift", "ctrl-cmd"]
                        .map(|modifiers| format!("{modifiers}-{number}"))
                })) {
                    let (bindings, pending) = keymap
                        .bindings_for_input(&[gpui::Keystroke::parse(&key).unwrap()], &contexts);
                    assert!(
                        bindings.is_empty() && !pending,
                        "{key} was intercepted in {names:?}"
                    );
                }
                let find = if cfg!(target_os = "macos") {
                    "cmd-shift-f"
                } else {
                    "ctrl-shift-f"
                };
                let (bindings, pending) =
                    keymap.bindings_for_input(&[gpui::Keystroke::parse(find).unwrap()], &contexts);
                assert!(!pending && bindings.len() == 1);
                assert!(bindings[0].action().as_any().is::<crate::FindInTerminal>());
            }
        });
    }

    #[test]
    fn vertical_navigation_wraps_and_supports_boundaries() {
        assert_eq!(destination(0, "up", 6), Some(5));
        assert_eq!(destination(5, "down", 6), Some(0));
        assert_eq!(destination(3, "home", 6), Some(0));
        assert_eq!(destination(3, "end", 6), Some(5));
        assert_eq!(destination(3, "space", 6), Some(3));
        assert_eq!(destination(3, "left", 6), None);
        assert_eq!(destination(0, "down", 0), None);
    }
}
