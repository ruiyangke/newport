//! Window-level commands; page navigation must not discard an open server draft.
use super::*;

pub(super) fn server_menu(menu: PopupMenu, view: WeakEntity<Newport>, cx: &App) -> PopupMenu {
    let Some(entity) = view.upgrade() else {
        return menu;
    };
    let app = entity.read(cx);
    let unavailable = app.server_form.is_some() || !app.configuration_available();
    let missing = app.selected_server().is_none();
    let deleting = app
        .resources
        .get(&format!("server-delete:{}", app.server))
        .is_some_and(|resource| resource.pending);
    let testing = app
        .resources
        .get(&format!("server-test:{}", app.server))
        .is_some_and(|resource| resource.pending);
    menu.item(
        PopupMenuItem::new("Test connection")
            .action(Box::new(TestServer))
            .disabled(unavailable || missing || deleting || testing),
    )
    .item(
        PopupMenuItem::new("Edit server…")
            .action(Box::new(EditServer))
            .disabled(unavailable || missing || deleting),
    )
    .separator()
    .item(
        PopupMenuItem::new("Add server…")
            .action(Box::new(NewServer))
            .disabled(unavailable),
    )
    .item(
        PopupMenuItem::new("Delete server…")
            .disabled(unavailable || missing || deleting)
            .on_click(move |_, window, cx| {
                let _ = view.update(cx, |this, cx| this.confirm_delete_server(window, cx));
            }),
    )
}
impl Newport {
    pub(super) fn configuration_available(&self) -> bool {
        !self.snapshot.is_null()
            && self.snapshot["loadError"]
                .as_str()
                .is_none_or(str::is_empty)
            && self
                .resources
                .get("snapshot")
                .is_none_or(|resource| resource.error.is_none())
    }
    pub(super) fn sidebar_group(
        &self,
        cx: &mut Context<Self>,
    ) -> gpui_kit::component::resizable::ResizablePanelGroup {
        let view = cx.entity().downgrade();
        let focus = self.sidebar_focus.clone();
        let appearance = gpui_kit::component::resizable::resize_handle_appearance();
        self.sidebar_layout
            .group()
            .with_handle_appearance(std::rc::Rc::new(move |handle, window, cx| {
                let keys = view.clone();
                let clicks = view.clone();
                let click_focus = focus.clone();
                let width = view
                    .upgrade()
                    .map(|v| {
                        v.read(cx)
                            .sidebar_layout
                            .width(f32::from(window.viewport_size().width))
                    })
                    .unwrap_or(sidebar_layout::DEFAULT_WIDTH);
                Some(
                    div()
                        .relative()
                        .size_full()
                        .children(appearance(handle, window, cx))
                        .child(
                            div()
                                .id("sidebar-width")
                                .absolute()
                                .left(px(-4.))
                                .top_0()
                                .bottom_0()
                                .w(px(9.))
                                .role(accesskit::Role::Splitter)
                                .aria_label("Sidebar width")
                                .aria_orientation(accesskit::Orientation::Vertical)
                                .aria_value(format!("{width:.0}"))
                                .track_focus(&focus)
                                .tab_stop(true)
                                .focus_visible(|style| style.bg(cx.theme().primary.opacity(0.25)))
                                .on_click(move |event, window, cx| {
                                    click_focus.focus(window, cx);
                                    if event.click_count() == 2 {
                                        let view = clicks.clone();
                                        window.defer(cx, move |window, cx| {
                                            let _ = view.update(cx, |this, cx| {
                                                this.resize_sidebar(0., true, window, cx)
                                            });
                                        });
                                    }
                                })
                                .on_key_down(move |event, window, cx| {
                                    if event.keystroke.modifiers.platform
                                        || event.keystroke.modifiers.control
                                        || event.keystroke.modifiers.alt
                                    {
                                        return;
                                    }
                                    let _ = keys.update(cx, |this, cx| {
                                        let viewport = f32::from(window.viewport_size().width);
                                        let current = this.sidebar_layout.width(viewport);
                                        let delta = match event.keystroke.key.as_str() {
                                            "left" => -10.,
                                            "right" => 10.,
                                            "home" => sidebar_layout::MIN_WIDTH - current,
                                            "end" => sidebar_layout::maximum(viewport) - current,
                                            _ => return,
                                        };
                                        window.prevent_default();
                                        cx.stop_propagation();
                                        this.resize_sidebar(delta, false, window, cx);
                                    });
                                }),
                        )
                        .into_any_element(),
                )
            }))
    }
    pub(super) fn resize_sidebar(
        &mut self,
        delta: f32,
        reset: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // The icon rail has a fixed width; resizing would only change the
        // remembered full width without any visible effect.
        if window.has_active_dialog(cx) || self.sidebar_layout.compact() {
            return;
        }
        let viewport = f32::from(window.viewport_size().width);
        if reset {
            self.sidebar_layout.reset(viewport, cx)
        } else {
            self.sidebar_layout
                .set_width(self.sidebar_layout.width(viewport) + delta, viewport, cx)
        }
        self.save_sidebar();
        cx.notify();
    }
    pub(super) fn selected_server(&self) -> Option<&Value> {
        self.snapshot["config"]["servers"]
            .as_array()
            .and_then(|servers| servers.iter().find(|server| server["id"] == self.server))
    }
    pub(super) fn navigate(&mut self, page: Page, cx: &mut Context<Self>) {
        if self.server_form.is_some() || self.modal_open || self.page == page {
            return;
        }
        if self.server.is_empty() && page != Page::Settings {
            return;
        }
        self.save_workspace_selection();
        self.restore_git = None;
        self.suspend_connections();
        self.suspend_project_library();
        if self.page == Page::Overview {
            self.overview_state.cached_reads =
                Some(self.resources.take_cached(&["page", "history"]));
        }
        self.page = page;
        if page != Page::Settings {
            self.workspace_page = page;
        }
        self.reset_view(true);
        self.restore_connections();
        if page == Page::Overview
            && let Some(reads) = self.overview_state.cached_reads.take()
        {
            self.resources.restore(reads);
        }
        if page == Page::Projects {
            self.restore_git = self.git_destinations.get(&self.server).cloned();
        }
        self.save_workspace_selection();
        self.refresh_navigation();
        cx.notify();
    }
    pub(super) fn test_server(&mut self, cx: &mut Context<Self>) {
        if self.server_form.is_some() || self.modal_open || self.selected_server().is_none() {
            return;
        }
        self.request(
            &format!("server-test:{}", self.server),
            "test_connection",
            json!({"id":self.server}),
        );
        cx.notify();
    }
    pub(super) fn confirm_delete_server(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.server_form.is_some()
            || !self.configuration_available()
            || window.has_active_dialog(cx)
        {
            return;
        }
        let Some(server) = self.selected_server() else {
            return;
        };
        let id = text(server, "id");
        if self
            .resources
            .get(&format!("server-delete:{id}"))
            .is_some_and(|r| r.pending)
        {
            return;
        }
        let name = text(server, "name");
        let title = format!("Delete {name}?");
        let description =
            "Its forwards will disconnect and be removed, along with its saved password.";
        let view = cx.entity().downgrade();
        window.open_dialog(cx, move |dialog, window, _| {
            let id = id.clone();
            let view = view.clone();
            dialog
                .aria_label(title.clone())
                .title(crate::controls::dialog_title(title.clone()))
                .w(px(480.))
                .max_w((window.viewport_size().width - px(32.)).max(px(240.)))
                .p(px(24.))
                .rounded(px(crate::ui::RADIUS))
                .border_0()
                .centered(true)
                .overlay_closable(false)
                // Bare Enter must neither dismiss nor implicitly approve a
                // destructive operation; the explicit button owns submission.
                .on_ok(|_, _, _| false)
                .child(
                    div()
                        .id("delete-server-confirmation")
                        .role(accesskit::Role::AlertDialog)
                        .aria_label(title.clone())
                        .aria_description(description)
                        .flex()
                        .flex_col()
                        .pt(px(12.))
                        .gap(px(32.))
                        .child(
                            div()
                                .text_size(px(crate::ui::FONT_SIZE))
                                .line_height(relative(1.65))
                                .child(description),
                        )
                        .child(
                            div()
                                .flex()
                                .justify_end()
                                .gap_2()
                                .child(
                                    crate::controls::form_button("cancel-delete-server")
                                        .label("Cancel")
                                        .min_w(px(76.))
                                        .rounded(px(crate::ui::RADIUS))
                                        .on_click(|_, window, cx| window.close_dialog(cx)),
                                )
                                .child(
                                    crate::controls::form_button("confirm-delete-server")
                                        .danger()
                                        .min_w(px(76.))
                                        .rounded(px(crate::ui::RADIUS))
                                        .label("Delete server")
                                        .on_click(move |_, window, cx| {
                                            let _ = view.update(cx, |this, cx| {
                                                this.request(
                                                    &format!("server-delete:{id}"),
                                                    "delete_server",
                                                    json!({"id":id}),
                                                );
                                                cx.notify();
                                            });
                                            window.close_dialog(cx);
                                        }),
                                ),
                        ),
                )
        });
    }
    pub(super) fn shell_actions(&self, cx: &mut Context<Self>) -> AnyElement {
        let disabled = self.server_form.is_some()
            || self.modal_open
            || self
                .resources
                .get(&format!("server-delete:{}", self.server))
                .is_some_and(|r| r.pending);
        div()
            .flex()
            .flex_shrink_0()
            .items_center()
            .gap(px(2.))
            .when(
                self.selected_server().is_some() && self.page != Page::Settings,
                |row| {
                    row.child(
                        crate::controls::icon_button("test-server")
                            .icon(IconName::Cable)
                            .accessibility_label("Test connection")
                            .tooltip("Test connection")
                            .loading(
                                self.resources
                                    .get(&format!("server-test:{}", self.server))
                                    .is_some_and(|r| r.pending),
                            )
                            .ghost()
                            .text_color(cx.theme().muted_foreground)
                            .disabled(
                                disabled
                                    || self
                                        .resources
                                        .get(&format!("server-test:{}", self.server))
                                        .is_some_and(|r| r.pending),
                            )
                            .on_click(cx.listener(|this, _, _, cx| this.test_server(cx))),
                    )
                },
            )
            .into_any_element()
    }
}
