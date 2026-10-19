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
    pub number: usize,
}

// Match the desktop.rs accelerators; Terminal is primary+5 and Files primary+6.
pub(super) fn shortcut_page(number: usize) -> Option<Page> {
    match number {
        1 => Some(Page::Overview),
        2 => Some(Page::Ports),
        3 => Some(Page::Services),
        4 => Some(Page::Containers),
        5 => Some(Page::Terminal),
        6 => Some(Page::Files),
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
    #[cfg(target_os = "macos")]
    cx.bind_keys([
        KeyBinding::new("cmd-h", HideApp, None),
        KeyBinding::new("cmd-alt-h", HideOthers, None),
        KeyBinding::new("cmd-w", crate::project_editor::CloseTab, None),
        KeyBinding::new("cmd-shift-w", CloseWindow, None),
        KeyBinding::new("cmd-m", MinimizeWindow, None),
        KeyBinding::new("ctrl-cmd-f", ToggleFullScreen, None),
    ]);
    #[cfg(not(target_os = "macos"))]
    cx.bind_keys([KeyBinding::new("f11", ToggleFullScreen, None)]);
    let mut items = Vec::new();
    for number in [1, 2, 3, 4, 5, 6] {
        let page = shortcut_page(number).expect("supported desktop view");
        let action = OpenView { number };
        items.push(MenuItem::action(page.title(), action));
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

/// Keep menu accelerators and keyboard dispatch on the same platform mapping.
/// GPUI's `cmd` means the Windows key on Windows, not Control.
pub(super) fn install_workspace_shortcuts(cx: &mut App) {
    let primary = if cfg!(target_os = "macos") {
        "cmd"
    } else {
        "ctrl"
    };
    cx.bind_keys([
        KeyBinding::new(&format!("{primary}-f"), FindInView, None),
        KeyBinding::new(&format!("{primary}-,"), OpenSettings, None),
        KeyBinding::new(&format!("{primary}-n"), NewServer, None),
        KeyBinding::new(
            &format!("{primary}-shift-e"),
            EditServer,
            Some("!ProjectEditor"),
        ),
        KeyBinding::new(&format!("{primary}-shift-t"), TestServer, None),
        KeyBinding::new(&format!("{primary}-shift-l"), ToggleSidebar, None),
    ]);
    // Ctrl+Q and Ctrl+R are terminal flow-control/history keys on Windows.
    // Refresh uses F5 there; quitting remains available through the app menu.
    #[cfg(not(target_os = "macos"))]
    cx.bind_keys([KeyBinding::new("f5", Refresh, Some("!Terminal"))]);
    // Control+Alt is AltGr on Windows layouts. Keep these macOS-only;
    // the focused divider supports keyboard resizing on both platforms.
    #[cfg(target_os = "macos")]
    cx.bind_keys([
        KeyBinding::new("cmd-q", Quit, None),
        KeyBinding::new("cmd-r", Refresh, None),
        KeyBinding::new("cmd-alt-right", SidebarWider, None),
        KeyBinding::new("cmd-alt-left", SidebarNarrower, None),
        KeyBinding::new("cmd-alt-0", ResetSidebar, None),
    ]);
    for number in 1..=6 {
        cx.bind_keys([KeyBinding::new(
            &format!("{primary}-{number}"),
            OpenView { number },
            None,
        )]);
    }
}
pub(super) const VIEWS: [(Page, IconName); 8] = [
    (Page::Overview, IconName::Activity),
    (Page::Ports, IconName::Link),
    (Page::Services, IconName::Layers),
    (Page::Containers, IconName::Box),
    (Page::Terminal, IconName::Terminal),
    (Page::Files, IconName::Folder),
    (Page::Projects, IconName::FolderCode),
    (Page::Integration, IconName::LayoutGrid),
];
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
        let current = VIEWS
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
            .children(VIEWS.into_iter().enumerate().flat_map(|(i, (page, icon))| {
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
                let tab = Tab::new(("nav", i))
                    .accessibility_label(page.title())
                    .selected(page == self.page)
                    .set_position(i + 1, VIEWS.len())
                    .disabled(disabled)
                    // Kit blocks activation but does not expose the disabled
                    // flag. Amend the same node, preserving its label, selected
                    // state and position rather than creating a second tab.
                    .a11y_synthetic_children(move |builder| {
                        if disabled {
                            builder.parent_node().set_disabled();
                        }
                    })
                    .track_focus(&self.navigation_focus[i])
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
                        this.navigation_focus[i].focus(window, cx);
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
                        let Some(next) = destination(i, &event.keystroke.key, VIEWS.len()) else {
                            return;
                        };
                        cx.stop_propagation();
                        window.prevent_default();
                        this.navigate(VIEWS[next].0, cx);
                        this.navigation_focus[next].focus(window, cx);
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
