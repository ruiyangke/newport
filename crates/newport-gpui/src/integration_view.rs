//! Integrations: sharing controls and always-visible terminal and agent settings.
use super::*;
use gpui_kit::component::scroll::ScrollableElement as _;

fn section_heading(id: &'static str, label: &'static str) -> impl IntoElement {
    div()
        .id(id)
        .role(Role::Heading)
        .aria_level(2)
        .test_support()
        .aria_label(label)
        .text_size(px(14.))
        .font_weight(FontWeight::SEMIBOLD)
        .child(label)
}

/// A page section spaced like Settings: 22px above and below, a rule after.
fn section(first: bool, cx: &App) -> Div {
    div()
        .flex()
        .flex_col()
        .border_b_1()
        .border_color(cx.theme().border)
        .pt(px(if first { 4. } else { 22. }))
        .pb(px(22.))
}

const NEEDS_NEEDLE: &str = "Needs Needle";

impl Newport {
    pub(crate) fn integration(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(server) = self.selected_server() else {
            return empty::pane(
                IconName::Server,
                "Choose a server",
                "Select a server to manage integrations.",
                cx,
            )
            .into_any_element();
        };
        let server_id = self.server.clone();
        let busy = self.integration_busy(&server_id);
        let installing = self.integration_installing(&server_id);
        let disabled =
            busy || installing || self.server_form.is_some() || !self.configuration_available();
        let clipboard = server["clipboardEnabled"] == true;
        let browser = server["browserEnabled"] == true;
        let enabled = clipboard || browser;
        let state = &self.snapshot["runtime"]["clipboard"][&server_id];
        let permissions = &self.snapshot["runtime"]["agentPermissions"][&server_id];
        let transport = crate::connectivity::scoped(&server_id, &self.snapshot, cx);
        let transport_wait =
            transport.filter(|state| state.phase != crate::connectivity::Phase::Connected);
        let status = state["status"].as_str().unwrap_or("disconnected");
        let muted = cx.theme().muted_foreground;
        // Sharing runs on Needle; a server that skipped its setup has none.
        let unset = self.needle_missing();
        let connection_status = if enabled {
            transport_wait
                .map(|state| state.phase.label().into())
                .unwrap_or_else(|| connection_label(state))
        } else {
            "Off".into()
        };
        let tone = if !enabled {
            muted
        } else if let Some(transport) = transport_wait {
            if transport.phase == crate::connectivity::Phase::Failed {
                theme::error(cx.theme().is_dark())
            } else {
                muted
            }
        } else {
            match status {
                "connected" => theme::success(cx.theme().is_dark()),
                "error" => theme::error(cx.theme().is_dark()),
                "connecting" | "reconnecting" => theme::warning(cx.theme().is_dark()),
                _ => muted,
            }
        };
        let mut body = div().w_full().min_w_0().flex().flex_col().gap(px(16.));
        for (feature, title, description, checked) in [
            (
                "clipboard",
                "Clipboard access",
                "Let apps on this server read your clipboard.",
                clipboard,
            ),
            (
                "browser",
                "Browser links",
                "Open links from this server in your browser.",
                browser,
            ),
        ] {
            let id = server_id.clone();
            let label_id = server_id.clone();
            let expected = crate::terminal_scope::connection(server, &self.snapshot);
            let label_expected = expected.clone();
            let feature_disabled = if checked {
                self.server_form.is_some()
                    || !self.configuration_available()
                    || self.integration_disabling(&server_id, feature)
            } else {
                disabled
            };
            let feature_state = if self.integration_disabling(&server_id, feature) {
                "Turning off…"
            } else if let Some(error) = self
                .resources
                .get(&disable_key(&server_id, feature))
                .and_then(|r| r.error.as_deref())
            {
                error
            } else if !checked {
                "Off"
            } else if unset {
                NEEDS_NEEDLE
            } else if let Some(transport) = transport_wait {
                transport.phase.label()
            } else {
                feature_status(feature, checked, permissions, status == "connected")
            }
            .to_owned();
            body = body.child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(px(16.))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(
                                div()
                                    .flex()
                                    .flex_wrap()
                                    .items_center()
                                    .gap(px(8.))
                                    .child(
                                        div()
                                            .id(SharedString::from(format!(
                                                "integration-feature-label-{feature}"
                                            )))
                                            .role(Role::Label)
                                            .test_support()
                                            .aria_label(title)
                                            .text_size(px(crate::ui::FONT_SIZE))
                                            .line_height(relative(1.5))
                                            .when(!feature_disabled, |label| {
                                                label.cursor_pointer().on_click(cx.listener(
                                                    move |this, _, _, cx| {
                                                        this.integration_scoped_command(
                                                            &label_id,
                                                            &label_expected,
                                                            (feature, !checked),
                                                            cx,
                                                        );
                                                    },
                                                ))
                                            })
                                            .child(title),
                                    )
                                    .when(
                                        feature_state != "Active" && feature_state != "Off",
                                        |row| {
                                            row.child(
                                                div()
                                                    .id(SharedString::from(format!(
                                                        "integration-effective-{feature}"
                                                    )))
                                                    .role(Role::Status)
                                                    .test_support()
                                                    .aria_label(format!("{title}: {feature_state}"))
                                                    .text_size(px(crate::ui::FONT_SIZE))
                                                    .text_color(muted)
                                                    .child(feature_state),
                                            )
                                        },
                                    ),
                            )
                            .child(
                                div()
                                    .id(SharedString::from(format!(
                                        "integration-description-{feature}"
                                    )))
                                    .role(Role::Paragraph)
                                    .test_support()
                                    .aria_label(description)
                                    .mt(px(4.))
                                    .text_size(px(crate::ui::FONT_SIZE))
                                    .line_height(relative(1.6))
                                    .text_color(muted)
                                    .child(description),
                            ),
                    )
                    .child(
                        crate::controls::switch(feature)
                            .checked(checked)
                            .disabled(feature_disabled)
                            .accessibility_label(title)
                            .on_click(cx.listener(move |this, checked: &bool, _, cx| {
                                this.integration_scoped_command(
                                    &id,
                                    &expected,
                                    (feature, *checked),
                                    cx,
                                );
                            })),
                    ),
            );
        }
        // A matching switch already communicates the healthy on/off state.
        // Reserve the connection row for progress and problems requiring action.
        let show_connection = enabled
            && !unset
            && (transport_wait.is_some()
                || status != "connected"
                || permissions["error"].is_string());
        let mut connection = div()
            .id("integration-connection")
            .role(Role::Region)
            .test_support()
            .aria_label("Sharing connection")
            .flex()
            .flex_col()
            .gap(px(8.))
            .child(
                div()
                    .id("integration-connection-status")
                    .role(Role::Status)
                    .test_support()
                    .aria_label(format!("Sharing connection: {connection_status}"))
                    .text_size(px(crate::ui::FONT_SIZE))
                    .text_color(tone)
                    .child(format!("Sharing connection: {connection_status}")),
            );
        if enabled {
            if let Some(error) = transport_wait
                .map(|state| state.error.as_deref())
                .unwrap_or_else(|| state["errorMessage"].as_str())
                .filter(|error| !error.is_empty())
            {
                connection = connection.child(
                    div()
                        .id("integration-error")
                        .role(Role::Alert)
                        .test_support()
                        .aria_label(error.to_owned())
                        .text_size(px(crate::ui::FONT_SIZE))
                        .line_height(relative(1.6))
                        .text_color(theme::error(cx.theme().is_dark()))
                        .child(error.to_owned()),
                );
            }
            if permissions["error"].is_string() {
                let id = server_id.clone();
                let expected = crate::terminal_scope::connection(server, &self.snapshot);
                connection = connection.child(
                    div().flex().child(
                        crate::controls::button("integration-apply-permissions")
                            .label("Apply permissions")
                            .loading(self.integration_pending(&server_id, false))
                            .disabled(disabled)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.integration_scoped_command(
                                    &id,
                                    &expected,
                                    (if clipboard { "clipboard" } else { "browser" }, true),
                                    cx,
                                );
                            })),
                    ),
                );
            }
        }

        let agent_status = &self.snapshot["runtime"]["agentStatus"][&server_id];
        let shell = section(false, cx)
            .gap(px(8.))
            .child(div().mb(px(8.)).child(section_heading(
                "integration-shell-heading",
                "Terminal commands",
            )))
            .child(description(
                "integration-shell-instructions",
                "Clipboard and browser commands are set up for new shells when Needle is installed with sharing on. For an existing Bash or Zsh session, run this on the server:",
                cx,
            ))
            .child(self.integration_shell.clone());
        let summary = if self.integration_installing(&server_id) {
            "Working…"
        } else if unset {
            "Not set up"
        } else if agent_status["updateAvailable"] == true {
            "Update available"
        } else if agent_status["error"].is_string() {
            "Check failed"
        } else if agent_status["installed"] == true {
            "Installed"
        } else if agent_status["installed"] == false {
            "Not installed"
        } else {
            "Not checked"
        };
        let mut maintenance = section(false, cx)
            .id("needle-status")
            .debug_selector(|| "needle-status".into())
            .gap(px(8.))
            .child(
                div()
                    .mb(px(8.))
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .justify_between()
                    .gap(px(12.))
                    .child(section_heading("needle-heading", "Needle agent"))
                    .child(
                        div()
                            .id("needle-summary")
                            .role(Role::Status)
                            .test_support()
                            .aria_label(format!("Needle agent: {summary}"))
                            .text_size(px(crate::ui::FONT_SIZE))
                            .text_color(muted)
                            .child(summary),
                    ),
            );
        {
            let details = div()
                .flex()
                .flex_col()
                .gap(px(8.))
                .child(description(
                    "needle-purpose",
                    "Runs monitoring, Git, and sharing on this server.",
                    cx,
                ))
                .child(description(
                    "needle-version-status",
                    if unset {
                        "Needle isn’t set up on this server."
                    } else {
                        needle_status(agent_status)
                    },
                    cx,
                ))
                .child(self.needle_management(cx));
            maintenance = maintenance.child(details);
        }
        let content = div()
            .id("integrations-content")
            .debug_selector(|| "integrations-content".into())
            .w_full()
            .max_w(px(720.))
            .mx_auto()
            .min_w_0()
            .flex()
            .flex_col()
            .child(crate::controls::page_title("Integrations", None, cx).mb(px(16.)))
            .child(
                section(true, cx)
                    .child(body)
                    .when(show_connection, |section| {
                        section.child(connection.mt(px(16.)))
                    }),
            )
            .child(shell)
            .child(maintenance);
        div()
            .id("integrations-scroll-region")
            .debug_selector(|| "integrations-scroll-region".into())
            .flex_1()
            .min_h_0()
            .min_w_0()
            .flex()
            .flex_col()
            .overflow_hidden()
            .relative()
            .child(
                div()
                    .id("integrations-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .track_scroll(&self.integration_scroll)
                    .child(
                        div()
                            .w_full()
                            .p(px(crate::controls::PAGE_PADDING))
                            .child(content),
                    ),
            )
            .vertical_scrollbar(&self.integration_scroll)
            .into_any_element()
    }
}
