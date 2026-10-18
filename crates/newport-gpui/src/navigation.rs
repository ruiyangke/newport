//! Kit's base tabs provide vertical-tab semantics without imposing horizontal styling.
use super::*;

/// Footer actions share the navigation rows' hover and selected surfaces.
pub(super) fn sidebar_action_style(
    selected: bool,
    cx: &App,
) -> gpui_kit::component::button::ButtonCustomVariant {
    let selection = theme::sidebar_selection(cx.theme().is_dark());
    gpui_kit::component::button::ButtonCustomVariant::new(cx)
        .foreground(theme::sidebar_action(cx.theme().is_dark()))
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
    items.push(MenuItem::action("Show/Hide Sidebar", ToggleSidebar));
    items.push(MenuItem::separator());
    items.push(MenuItem::action("Next Tab", crate::project_editor::NextTab));
    items.push(MenuItem::action(
        "Previous Tab",
        crate::project_editor::PreviousTab,
    ));
    items.push(MenuItem::action(
        "Show/Hide Project Terminal",
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
        MenuItem::action("Hide Others", HideOthers),
        MenuItem::action("Show All", ShowAll),
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
                MenuItem::action("Open File…", crate::project_editor::QuickOpen),
                MenuItem::action("Focus File Explorer", crate::project_editor::FocusExplorer),
                MenuItem::action("Save File", crate::project_editor::SaveFile),
                MenuItem::action("Close Tab", crate::project_editor::CloseTab),
                MenuItem::separator(),
                MenuItem::action("Close Window", CloseWindow),
            ],
        },
        Menu {
            name: "Server".into(),
            disabled: false,
            items: vec![
                MenuItem::action("Add Server…", NewServer),
                MenuItem::action("Edit Server…", EditServer),
                MenuItem::action("Test Connection", TestServer),
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
                MenuItem::os_action("Select All", SelectAll, OsAction::SelectAll),
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
                vec![MenuItem::action("Toggle Full Screen", ToggleFullScreen)]
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
    (Page::Ports, IconName::Cable),
    (Page::Services, IconName::Layers),
    (Page::Containers, IconName::Boxes),
    (Page::Terminal, IconName::Terminal),
    (Page::Files, IconName::FolderOpen),
    (Page::Projects, IconName::FolderGit2),
    (Page::Integration, IconName::Plug),
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
    pub(super) fn navigation(&self, cx: &mut Context<Self>) -> AnyElement {
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
            .children(VIEWS.into_iter().enumerate().map(|(i, (page, icon))| {
                Tab::new(("nav", i))
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
                    .h(px(32.))
                    .when(i + 1 < VIEWS.len(), |tab| tab.mb(px(2.)))
                    .px(px(8.))
                    .border_1()
                    .border_color(gpui_kit::transparent_black())
                    .rounded(px(6.))
                    .text_size(px(13.))
                    .when(!cx.theme().is_dark(), |tab| {
                        tab.font_weight(FontWeight::MEDIUM)
                    })
                    .justify_start()
                    .when(page == self.page, |tab| {
                        tab.bg(theme::sidebar_selection(cx.theme().is_dark()))
                            .font_weight(FontWeight::MEDIUM)
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
                    .focus_visible(|style| style.border_color(cx.theme().ring))
                    .child(
                        div()
                            .w_full()
                            .flex()
                            .items_center()
                            .gap(px(9.))
                            .child(
                                Icon::new(icon)
                                    .size(px(16.))
                                    .text_color(if page == self.page {
                                        cx.theme().primary
                                    } else {
                                        theme::sidebar_muted(cx.theme().is_dark())
                                    }),
                            )
                            .child(page.title())
                            .when(page == Page::Ports && tunnel_count > 0, |row| {
                                row.child(div().flex_1()).child(
                                    div()
                                        .text_size(px(11.))
                                        .text_color(if page == self.page {
                                            theme::sidebar_foreground(cx.theme().is_dark())
                                        } else {
                                            theme::sidebar_muted(cx.theme().is_dark())
                                        })
                                        .child(tunnel_count.to_string()),
                                )
                            }),
                    )
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
