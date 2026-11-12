//! The Needle agent on a server: install or update it, or uninstall it.
//! Uninstalling is final and asks first.
use crate::integration::description;
use crate::*;
#[path = "integration_needle_status.rs"]
mod status;
use status::{Action, Tone};

pub(super) fn inspect_key(server: &str) -> String {
    format!("integration:inspect:{server}")
}

#[cfg(test)]
#[path = "integration_needle_tests.rs"]
mod tests;

/// The uninstall request of a server.
pub(super) fn key(server: &str) -> String {
    format!("integration:uninstall:{server}")
}

/// A confirmation to uninstall, tied to the connection it was asked for.
#[derive(Clone)]
pub(super) struct Confirmation {
    server: Value,
    scope: Value,
}

impl Newport {
    fn ask_to_uninstall_needle(&mut self, cx: &mut Context<Self>) {
        if !self.configuration_available()
            || self.server_form.is_some()
            || self.integration_busy(&self.server)
        {
            return;
        }
        let Some(server) = self.selected_server().cloned() else {
            return;
        };
        self.needle_uninstall_confirmations.insert(
            self.server.clone(),
            Confirmation {
                scope: newport_domain::connection_scope::connection(&server, &self.snapshot),
                server,
            },
        );
        // Needle's section ends the page; its confirmation opens below it.
        self.integration_scroll.scroll_to_bottom();
        cx.notify();
    }

    fn uninstall_needle(&mut self, cx: &mut Context<Self>) {
        let Some(confirmation) = self.needle_uninstall_confirmations.remove(&self.server) else {
            return;
        };
        // The connection changed since it was asked for: ask again.
        let current = self.selected_server().is_some_and(|server| {
            server["id"] == confirmation.server["id"]
                && newport_domain::connection_scope::connection(server, &self.snapshot)
                    == confirmation.scope
        });
        if current && !self.integration_busy(&self.server) {
            self.request_result(
                &key(&self.server),
                desktop_server_record(&confirmation.server).map(|expected_server| {
                    desktop::Operation::Connection(Box::new(
                        desktop::connection::Operation::UninstallAgent(Box::new(
                            desktop::connection::UninstallAgentArgs { expected_server },
                        )),
                    ))
                }),
            );
            // Uninstalling starts setup over, even one finished in this window.
            self.setup_flow = None;
            self.resources.invalidate_prefix("setup-");
        }
        cx.notify();
    }

    fn needle_action(
        &mut self,
        server: &str,
        expected: &Value,
        action: Action,
        cx: &mut Context<Self>,
    ) {
        if self.server != server
            || self.server_form.is_some()
            || !self.configuration_available()
            || self.integration_busy(server)
            || self.integration_installing(server)
            || self.selected_server().is_none_or(|current| {
                crate::terminal_scope::connection(current, &self.snapshot) != *expected
            })
        {
            return;
        }
        match action {
            Action::Setup => self.set_up_needle(cx),
            Action::Update => self.integration_command(server, None, cx),
            Action::Uninstall => self.ask_to_uninstall_needle(cx),
            Action::Inspect => {
                let Some(current) = self.selected_server() else {
                    return;
                };
                let request = desktop_server_record(current).map(|expected_server| {
                    desktop::Operation::Connection(Box::new(
                        desktop::connection::Operation::InspectAgent(Box::new(
                            desktop::connection::InspectAgentArgs { expected_server },
                        )),
                    ))
                });
                self.request_result(&inspect_key(server), request);
                cx.notify();
            }
        }
    }

    fn needle_details_open(&self) -> bool {
        self.selected_server().is_some_and(|server| {
            self.needle_details.get(&self.server)
                == Some(&crate::terminal_scope::connection(server, &self.snapshot))
        })
    }

    fn toggle_needle_details(&mut self, cx: &mut Context<Self>) {
        if self.needle_details_open() {
            self.needle_details.remove(&self.server);
        } else if let Some(server) = self.selected_server() {
            self.needle_details.insert(
                self.server.clone(),
                crate::terminal_scope::connection(server, &self.snapshot),
            );
        }
        cx.notify();
    }

    pub(super) fn needle_management(&self, cx: &mut Context<Self>) -> AnyElement {
        let server = self.server.clone();
        let installed = &self.snapshot["runtime"]["agentStatus"][&server];
        let runtime = &self.snapshot["runtime"]["agentRuntime"][&server];
        let transport = crate::connectivity::scoped(&server, &self.snapshot, cx)
            .map(|state| match state.phase {
                crate::connectivity::Phase::Connected => "connected",
                crate::connectivity::Phase::Connecting => "connecting",
                crate::connectivity::Phase::AwaitingApproval => "awaitingApproval",
                crate::connectivity::Phase::Reconnecting => "recovering",
                crate::connectivity::Phase::Failed => "failed",
                crate::connectivity::Phase::Disconnected => "disconnected",
            })
            .or_else(|| self.snapshot["runtime"]["agentTransport"][&server]["status"].as_str());
        let missing = self.needle_missing();
        let mut presentation = status::present(installed, runtime, transport, missing);
        let request = self.resources.get(&key(&server));
        let inspecting = self
            .resources
            .get(&inspect_key(&server))
            .is_some_and(|r| r.pending);
        let installing =
            self.integration_installing(&server) || self.integration_pending(&server, true);
        let uninstalling = request.is_some_and(|r| r.pending);
        let busy = self.integration_busy(&server)
            || installing
            || !self.configuration_available()
            || self.server_form.is_some();
        if uninstalling || installing || inspecting {
            presentation.label = if uninstalling {
                "Uninstalling…"
            } else if installing {
                "Updating…"
            } else {
                "Checking…"
            };
            presentation.note = None;
            presentation.action = None;
            presentation.tone = Tone::Quiet;
        }
        let operation_error = request
            .and_then(|r| r.error.as_deref())
            .or_else(|| {
                self.resources
                    .get(&crate::integration::action_key(&server, true))
                    .and_then(|r| r.error.as_deref())
            })
            .or_else(|| {
                self.resources
                    .get(&inspect_key(&server))
                    .and_then(|r| r.error.as_deref())
            });
        if !busy && operation_error.is_some() {
            presentation.note = Some("The last action failed. See Details.");
        }
        let actions = self.needle_controls(presentation.action, missing, busy, cx);
        let summary = div()
            .id("needle-summary")
            .role(Role::Status)
            .test_support()
            .aria_label(format!("Needle: {}", presentation.label))
            .flex()
            .items_center()
            .gap(px(5.))
            .text_size(px(crate::ui::FONT_SIZE))
            .text_color(cx.theme().muted_foreground)
            .when(presentation.tone != Tone::Quiet, |v| {
                v.child(
                    Icon::new(if presentation.tone == Tone::Ready {
                        IconName::Check
                    } else {
                        IconName::TriangleAlert
                    })
                    .size(px(14.))
                    .text_color(if presentation.tone == Tone::Ready {
                        theme::success(cx.theme().is_dark())
                    } else {
                        theme::warning(cx.theme().is_dark())
                    }),
                )
            })
            .child(presentation.label);
        let mut content = div()
            .id("needle-management")
            .flex()
            .flex_col()
            .gap(px(8.))
            .min_w_0()
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .justify_between()
                    .gap(px(12.))
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .items_center()
                            .gap(px(15.))
                            .child(
                                div()
                                    .id("needle-heading")
                                    .role(Role::Heading)
                                    .aria_level(2)
                                    .test_support()
                                    .aria_label("Needle")
                                    .text_size(px(crate::ui::FONT_SIZE))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child("Needle"),
                            )
                            .when_some(
                                presentation
                                    .active
                                    .then(|| status::running_version(installed, runtime))
                                    .flatten(),
                                |row, version| {
                                    row.child(
                                        div()
                                            .text_color(cx.theme().muted_foreground)
                                            .child(version.to_owned()),
                                    )
                                },
                            )
                            .child(summary),
                    )
                    .child(actions),
            );
        if let Some(note) = presentation.note {
            content = content.child(description("needle-note", note, cx));
        }
        if self.needle_details_open() {
            content = content.child(self.needle_diagnostics(&presentation, operation_error, cx));
        }
        if self
            .needle_uninstall_confirmations
            .contains_key(&self.server)
        {
            content = content
                .child(description("needle-uninstall-warning", "Stops Needle, ends its saved terminal sessions, and removes its state and shell setup. This can’t be undone.", cx))
                .child(div().flex().flex_wrap().gap(px(8.))
                    .child(crate::controls::button("needle-uninstall-confirm").label("Uninstall").danger().disabled(busy)
                        .on_click(cx.listener(|this, _, _, cx| this.uninstall_needle(cx))))
                    .child(crate::controls::button("needle-uninstall-cancel").label("Cancel")
                        .on_click(cx.listener(|this, _, _, cx| { this.needle_uninstall_confirmations.remove(&this.server); cx.notify(); }))));
        }
        content.into_any_element()
    }

    fn needle_controls(
        &self,
        action: Option<(Action, &'static str)>,
        missing: bool,
        busy: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let server = self.server.clone();
        let expected = self
            .selected_server()
            .map(|s| crate::terminal_scope::connection(s, &self.snapshot))
            .unwrap_or_default();
        let mut actions = div().flex().flex_wrap().items_center().gap(px(6.));
        if let Some((action, label)) = action {
            let server = server.clone();
            let expected = expected.clone();
            actions = actions.child(
                crate::controls::button("needle-primary-action")
                    .label(label)
                    .primary()
                    .disabled(busy)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.needle_action(&server, &expected, action, cx)
                    })),
            );
        }
        actions = actions.child(
            crate::controls::button("needle-details-toggle")
                .label(if self.needle_details_open() {
                    "Hide details"
                } else {
                    "Details"
                })
                .ghost()
                .on_click(cx.listener(|this, _, _, cx| this.toggle_needle_details(cx))),
        );
        let view = cx.entity().downgrade();
        actions = actions.child(
            crate::controls::icon_button("needle-more")
                .icon(IconName::Ellipsis)
                .ghost()
                .accessibility_label("More Needle actions")
                .disabled(busy)
                .dropdown_menu_with_anchor(gpui_kit::Anchor::TopRight, move |mut menu, _, _| {
                    for (action, label) in [
                        (Action::Inspect, "Check again"),
                        (Action::Update, "Reinstall"),
                        (Action::Uninstall, "Uninstall…"),
                    ] {
                        if missing && action != Action::Inspect {
                            continue;
                        }
                        if action == Action::Uninstall {
                            menu = menu.separator();
                        }
                        let view = view.clone();
                        let server = server.clone();
                        let expected = expected.clone();
                        menu = menu.item(PopupMenuItem::new(label).disabled(busy).on_click(
                            move |_, _, cx| {
                                let _ = view.update(cx, |this, cx| {
                                    this.needle_action(&server, &expected, action, cx)
                                });
                            },
                        ));
                    }
                    menu
                }),
        );
        actions.into_any_element()
    }

    fn needle_diagnostics(
        &self,
        presentation: &status::Status,
        operation_error: Option<&str>,
        cx: &App,
    ) -> AnyElement {
        let installed = &self.snapshot["runtime"]["agentStatus"][&self.server];
        let runtime = &self.snapshot["runtime"]["agentRuntime"][&self.server];
        let builds = status::Builds::of(installed, runtime);
        let mut details = div()
            .id("needle-details")
            .debug_selector(|| "needle-details".into())
            .flex()
            .flex_col()
            .gap(px(8.))
            .mt(px(8.))
            .pt(px(16.))
            .border_t_1()
            .border_color(cx.theme().border);
        for (label, value) in [
            ("Newport", crate::about::version().to_owned()),
            (
                if presentation.active {
                    "Used by this connection"
                } else {
                    "Last observed runtime"
                },
                builds.running(),
            ),
            ("Included with Newport", builds.bundled()),
            (
                "Server default",
                if installed["installed"] == false {
                    "Not installed".into()
                } else {
                    builds.installed()
                },
            ),
            (
                "Updates",
                if presentation.label == "Update required" {
                    "Manual update required"
                } else if installed["buildRelation"] == "newer" {
                    "Update Newport first"
                } else if installed["error"].is_string() {
                    "Server default cannot be updated automatically"
                } else {
                    "Automatic when compatible"
                }
                .to_owned(),
            ),
        ] {
            details = details.child(detail_row(label, value, cx));
        }
        if let Some(error) = operation_error
            .or_else(|| runtime["error"].as_str())
            .or_else(|| installed["error"].as_str())
        {
            details = details.child(description("needle-diagnostic", error, cx));
        }
        details.into_any_element()
    }
}

fn detail_row(label: &'static str, value: String, cx: &App) -> AnyElement {
    div()
        .id(ElementId::Name(format!("needle-detail-{label}").into()))
        .role(Role::Paragraph)
        .test_support()
        .aria_label(format!("{label}: {value}"))
        .flex()
        .flex_wrap()
        .gap(px(8.))
        .text_size(px(crate::ui::FONT_SIZE))
        .child(
            div()
                .w(px(180.))
                .text_color(cx.theme().muted_foreground)
                .child(label),
        )
        .child(div().flex_1().min_w(px(140.)).child(value))
        .into_any_element()
}
