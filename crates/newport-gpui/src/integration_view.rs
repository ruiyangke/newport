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
        let removal = crate::integration_removal::operation(
            &self.snapshot["runtime"]["agentOperations"][&server_id],
        );
        let busy = self.integration_busy(&server_id);
        let installing = self.integration_installing(&server_id);
        let disabled = busy
            || installing
            || removal.is_some()
            || self.server_form.is_some()
            || !self.configuration_available();
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
        let connection_status = if removal.is_some() {
            "Paused".into()
        } else if enabled {
            transport_wait
                .map(|state| state.phase.label().into())
                .unwrap_or_else(|| connection_label(state))
        } else {
            "Off".into()
        };
        let tone = if !enabled || removal.is_some() {
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
            } else if removal.is_some() {
                "Paused until Needle is restored"
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
                            .accessibility_description(description)
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
        let show_connection = removal.is_some()
            || (enabled
                && (transport_wait.is_some()
                    || status != "connected"
                    || permissions["error"].is_string()));
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
        if removal.is_some() {
            connection = connection.child(description(
                "integration-removal-paused",
                "Integration preferences are saved. Restore Needle to use them again.",
                cx,
            ));
        } else if enabled {
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

        let operations = &self.snapshot["runtime"]["agentOperations"][&server_id];
        let unfinished = unfinished_operation(operations);
        let mut recovery_panel = div();
        if let Some(operation) = unfinished {
            let operation_id = operation["intent"]["operationId"]
                .as_str()
                .unwrap()
                .to_owned();
            let helpers = operation["intent"]["kind"] == "helpers";
            let recovery_title = if helpers {
                "Shell setup unfinished"
            } else {
                "Installation unfinished"
            };
            let mut recovery = div().mt(px(12.)).flex().flex_col().gap_2()
                .child(div().id("needle-recovery-heading").role(Role::Heading).aria_level(3).test_support().aria_label(recovery_title).font_weight(FontWeight::SEMIBOLD).child(recovery_title))
                .child(description("needle-recovery-description",
                    if let Some(reason) = resume_blocker(operation) { reason } else if helpers { "Resume shell setup, or restore its previous files. Needle and its services stay unchanged." } else { "Resume this installation, or restore its previous state. Restoring may stop agent services; edited files are preserved." }, cx));
            if let Some(error) = operation["error"]
                .as_str()
                .filter(|_| resume_blocker(operation).is_none())
            {
                recovery = recovery.child(description("needle-recovery-error", error, cx));
            }
            if self
                .resources
                .get(&format!("integration:recovery:{server_id}"))
                .is_some_and(|r| r.pending)
            {
                recovery = recovery.child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(gpui_kit::component::spinner::Spinner::new().small())
                        .child(if helpers {
                            "Resolving shell setup…"
                        } else {
                            "Resolving installation…"
                        }),
                );
            }
            let mut actions = div().flex().flex_wrap().gap_2();
            if operation["phase"] != "rollingBack" && resume_blocker(operation).is_none() {
                let id = server_id.clone();
                let op = operation_id.clone();
                actions = actions.child(
                    crate::controls::button("needle-resume")
                        .label(if helpers {
                            "Resume shell setup"
                        } else {
                            "Resume installation"
                        })
                        .disabled(disabled)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.integration_recovery(&id, &op, false, cx)
                        })),
                );
            }
            let id = server_id.clone();
            let restore_operation = operation_id.clone();
            actions = actions.child(
                crate::controls::button("needle-restore")
                    .when(operation["phase"] == "rollingBack", |b| {
                        b.accessibility_label("Try restoring again")
                    })
                    .label(if operation["phase"] == "rollingBack" {
                        "Try again"
                    } else if helpers {
                        "Restore shell setup"
                    } else {
                        "Restore previous state"
                    })
                    .disabled(disabled)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.integration_recovery(&id, &restore_operation, true, cx)
                    })),
            );
            if operation["phase"] == "rollingBack" {
                let id = server_id.clone();
                let op = operation_id.clone();
                actions = actions.child(
                    crate::controls::button("needle-review")
                        .label("Review files")
                        .disabled(disabled)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.integration_review(&id, &op, None, cx)
                        })),
                );
                recovery = recovery.child(self.integration_file_review(
                    &server_id,
                    &operation_id,
                    disabled,
                    cx,
                ));
            }
            recovery_panel = recovery_panel.child(recovery.child(actions));
        }
        // A successful helper operation can still require manual shell setup.
        // Later binary-only upgrades must not erase that outstanding guidance.
        let setup_notice = operations["operations"].as_array().and_then(|records| {
            records
                .iter()
                .rev()
                .find(|record| {
                    record["phase"] == "ready" && record["intent"]["integrations"] == true
                })
                .and_then(|record| record["error"].as_str())
        });
        let management_attention = removal.is_some()
            || self.needle_removal_reviews.contains_key(&server_id)
            || operations["error"].is_string();
        let helper_attention = crate::integration_helpers::pending(operations).is_some()
            || self
                .needle_helper_ui
                .get(&server_id)
                .is_some_and(|s| s.has_review());
        let agent_status = &self.snapshot["runtime"]["agentStatus"][&server_id];
        let mut attention = div().flex().flex_col().gap(px(12.)).child(recovery_panel);
        if management_attention {
            attention = attention.child(self.needle_management(cx));
        }
        if helper_attention {
            attention = attention.child(self.standalone_helper_history(cx));
        }
        let terminal_status = terminal_setup_status(operations, removal.is_some());
        let mut shell = section(false, cx).gap(px(8.)).child(
            div()
                .mb(px(8.))
                .flex()
                .flex_wrap()
                .items_center()
                .justify_between()
                .gap(px(12.))
                .child(section_heading(
                    "integration-shell-heading",
                    "Terminal commands",
                ))
                .child(
                    div()
                        .id("terminal-setup-status")
                        .role(Role::Status)
                        .test_support()
                        .aria_label(format!("Terminal commands: {terminal_status}"))
                        .text_size(px(crate::ui::FONT_SIZE))
                        .text_color(muted)
                        .child(terminal_status),
                ),
        );
        if let Some(notice) = setup_notice {
            shell = shell.child(description("integration-shell-notice", notice, cx));
        }
        shell = shell
            .child(description("integration-shell-instructions",
                if terminal_status == "Setup completed" {
                    "For an existing Bash or Zsh session, run this on the server:"
                } else {
                    "Enable clipboard and browser commands in a Bash or Zsh session. Run this on the server:"
                }, cx))
            .child(self.integration_shell.clone())
            .when(!helper_attention, |section| section.child(self.standalone_helper_history(cx)));
        let summary = if removal.is_some_and(|operation| operation["phase"] == "removed") {
            "Removed"
        } else if management_attention || helper_attention || unfinished.is_some() {
            "Needs attention"
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
            let mut details = div()
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
                    needle_status(agent_status),
                    cx,
                ));
            if unfinished.is_some() {
                details = details.child(description(
                    "needle-maintenance-blocked",
                    "Resolve the unfinished operation above before changing Needle.",
                    cx,
                ));
            }
            if !management_attention {
                details = details.child(self.needle_management(cx));
            }
            maintenance = maintenance.child(details);
        }
        let attention_shown = unfinished.is_some() || management_attention || helper_attention;
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
            .when(attention_shown, |page| {
                page.child(section(true, cx).child(attention))
            })
            .child(
                section(!attention_shown, cx)
                    .child(body)
                    .when(show_connection, |section| {
                        section.child(connection.mt(px(16.)))
                    }),
            )
            .child(shell)
            .child(maintenance);
        div()
            .id("integrations-scroll-region")
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
