//! Shared agent setup surface for Overview and Integrations.
use crate::*;

impl Newport {
    pub(super) fn agent_setup(&self, overview: bool, cx: &mut Context<Self>) -> AnyElement {
        let state = &self.snapshot["runtime"]["agentInstallation"][&self.server];
        let retry_key = format!("agent-install:{}", self.server);
        let pending = self.resources.get(&retry_key).is_some_and(|r| r.pending);
        let error = if overview {
            self.resources.get("page").and_then(|r| r.error.clone())
        } else {
            None
        };
        let destination = self
            .selected_server()
            .map(|server| format!("{}@{}", text(&server, "sshUser"), text(&server, "sshHost")))
            .unwrap_or_default();
        let owner = cx.entity().downgrade();
        let server = self.server.clone();
        render_setup(
            Setup {
                state: state.clone(),
                overview,
                destination,
                pending,
                error,
                can_retry: self.configuration_available(),
                retry: std::rc::Rc::new(move |_, cx| {
                    let _ = owner.update(cx, |this, cx| {
                        if this.server != server || !this.configuration_available() {
                            return;
                        }
                        this.request(
                            &format!("agent-install:{server}"),
                            "cockpit_collect",
                            json!({"id":server,"section":"overview","refresh":true}),
                        );
                        cx.notify();
                    });
                }),
            },
            cx,
        )
    }
}

type RetrySetup = std::rc::Rc<dyn Fn(&mut Window, &mut App)>;

pub(super) struct Setup {
    pub state: Value,
    pub overview: bool,
    pub destination: String,
    pub pending: bool,
    pub error: Option<String>,
    pub can_retry: bool,
    pub retry: RetrySetup,
}

pub(super) fn render_setup(setup: Setup, cx: &App) -> AnyElement {
    let Setup {
        state,
        overview,
        destination,
        pending,
        error,
        can_retry,
        retry,
    } = setup;
    let state = &state;
    let stage = state["stage"]
        .as_str()
        .unwrap_or(if overview { "preparing" } else { "unknown" });
    let error = state["error"].as_str().or(error.as_deref());
    let active = matches!(
        stage,
        "preparing" | "uploading" | "installing" | "verifying" | "activating"
    ) && !state.is_null();
    let failed = !active && error.is_some();
    // Prefer structured progress. Older backends prefix installation errors
    // with their stage; ordinary SSH/RPC errors must not imply an upload.
    let failed_stage = state["failedStage"]
        .as_str()
        .or_else(|| {
            error
                .and_then(|error| error.split_once(':').map(|(stage, _)| stage))
                .filter(|stage| {
                    matches!(
                        *stage,
                        "preparing"
                            | "uploading"
                            | "installing"
                            | "verifying"
                            | "activating"
                            | "ready"
                    )
                })
        })
        .unwrap_or(stage);
    let (failure_title, recovery, retry_label) = match failed_stage {
        "preparing" => (
            "Couldn’t check this server",
            "Check the server address and your SSH credentials, then retry setup.",
            "Retry setup",
        ),
        "uploading" => (
            "Agent upload failed",
            "Check your SSH connection, then retry to send the agent again.",
            "Retry upload",
        ),
        "installing" | "verifying" | "activating" => (
            "Agent installation failed",
            "Review the error details, then retry installation.",
            "Retry installation",
        ),
        "ready" => (
            "Couldn’t load server metrics",
            "Retry to request a new reading.",
            "Retry metrics",
        ),
        _ => (
            "Server setup needs attention",
            "Review the error details, then retry setup.",
            "Retry setup",
        ),
    };
    let title = if failed {
        failure_title
    } else {
        match stage {
            "uploading" => "Uploading Newport agent",
            "installing" => "Staging Newport agent",
            "verifying" => "Checking Newport agent",
            "activating" => "Activating Newport agent",
            "ready" if overview => "Loading server metrics",
            "ready" => "Newport agent is installed",
            "unknown" => "Agent status not checked yet",
            _ => "Preparing Newport agent",
        }
    };
    let detail = match stage {
        "uploading" => "Keep Newport open while the agent transfers over SSH.",
        "installing" => "Storing the verified upload on your server.",
        "verifying" => "Checking that the new runtime can start before activating it.",
        "activating" => "Switching to the verified build and retaining the previous version.",
        "ready" if overview => "Waiting for the first reading. Metrics will appear automatically.",
        "ready" => "The server agent is available for monitoring and integrations.",
        "unknown" => "Open Overview to check the agent and set up server monitoring.",
        _ => "Checking the server and its installed agent before loading live metrics.",
    };
    let sent = state["sent"].as_u64().unwrap_or(0);
    let total = state["total"].as_u64().unwrap_or(0);
    let mut body = div()
        .id("agent-setup")
        .debug_selector(|| "agent-setup".into())
        .role(Role::Region)
        .aria_label("Newport agent setup")
        .w_full()
        .min_w_0()
        .flex()
        .flex_col()
        .gap(px(12.))
        .child(
            div()
                .id("agent-setup-status")
                .role(Role::Status)
                .test_support()
                .aria_label(title)
                .text_size(px(if overview { 22. } else { 14. }))
                .font_weight(FontWeight::SEMIBOLD)
                .child(title),
        )
        .when(!failed, |body| {
            body.child(
                div()
                    .text_size(px(12.))
                    .text_color(cx.theme().muted_foreground)
                    .child(detail),
            )
        });
    if total > 0 && !failed && stage == "uploading" {
        let fraction = (sent as f32 / total as f32).clamp(0., 1.);
        body = body
            .child(
                div()
                    .id("agent-upload-progress")
                    .role(Role::ProgressIndicator)
                    .aria_label(format!("Agent upload: {sent} of {total} bytes sent"))
                    .w_full()
                    .h(px(5.))
                    .rounded_full()
                    .bg(cx.theme().muted)
                    .child(
                        div()
                            .h_full()
                            .w(relative(fraction))
                            .rounded_full()
                            .bg(cx.theme().primary),
                    ),
            )
            .child(
                div()
                    .text_size(px(11.))
                    .text_color(cx.theme().muted_foreground)
                    .child(format!(
                        "{:.0}% · {:.1} / {:.1} MiB sent{}",
                        fraction * 100.,
                        sent as f64 / 1048576.,
                        total as f64 / 1048576.,
                        if stage == "uploading" {
                            format!(" · {}", upload_speed(state["bytesPerSecond"].as_f64()))
                        } else {
                            String::new()
                        }
                    )),
            );
    }
    if failed {
        body = body.child(
            div()
                .id("agent-setup-error")
                .role(Role::Alert)
                .test_support()
                .aria_label(error.unwrap_or_default().to_owned())
                .text_size(px(12.))
                .text_color(cx.theme().danger)
                .child(error.unwrap_or_default().to_owned()),
        );
        body = body.child(
            div()
                .text_size(px(12.))
                .text_color(cx.theme().muted_foreground)
                .child(recovery),
        );
        body = body.child(
            div().flex().child(
                crate::controls::button("retry-agent-installation")
                    .label(retry_label)
                    .loading(pending)
                    .disabled(pending || !can_retry)
                    .on_click(move |_, window, cx| retry(window, cx)),
            ),
        );
    }
    if overview {
        let current = setup_step(if failed { failed_stage } else { stage });
        let (note_title, note) = match current {
            0 => (
                "Checking compatibility",
                "Checking your SSH connection, server platform, and existing installation. A compatible agent will be reused automatically.",
            ),
            1 => (
                "What happens next",
                "Once the transfer finishes, Newport verifies the file and starts monitoring automatically. You don’t need to advance to the next step.",
            ),
            2 => (
                "What changes on the server",
                "The agent is installed at ~/.local/bin/newport-agent. Your shell configuration stays unchanged.",
            ),
            _ => (
                "Almost ready",
                "Setup finishes when the first CPU, memory, and network readings arrive. Overview opens automatically.",
            ),
        };
        let wizard = div()
            .w_full()
            .max_w(px(640.))
            .min_w_0()
            .flex()
            .flex_col()
            .gap(px(28.))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .justify_between()
                    .gap(px(8.))
                    .child(
                        div()
                            .text_size(px(13.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("Server setup"),
                    )
                    .child(
                        div()
                            .id("server-setup-step")
                            .role(Role::Status)
                            .test_support()
                            .aria_label(format!("Step {} of 4", current + 1))
                            .text_size(px(12.))
                            .text_color(cx.theme().muted_foreground)
                            .child(format!("Step {} of 4", current + 1)),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(16.))
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(cx.theme().muted_foreground)
                            .child(destination),
                    )
                    .child(body),
            )
            .when(!failed, |wizard| {
                wizard.child(
                    div()
                        .debug_selector(|| "agent-setup-next-step".into())
                        .border_t_1()
                        .border_color(cx.theme().border)
                        .pt(px(16.))
                        .flex()
                        .flex_col()
                        .gap(px(8.))
                        .child(
                            div()
                                .text_size(px(12.))
                                .font_weight(FontWeight::SEMIBOLD)
                                .child(note_title),
                        )
                        .child(
                            div()
                                .text_size(px(12.))
                                .line_height(relative(1.5))
                                .text_color(cx.theme().muted_foreground)
                                .child(note),
                        ),
                )
            });

        div()
            .id("agent-setup-page")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .p(px(crate::controls::PAGE_PADDING))
            .child(wizard)
            .into_any_element()
    } else {
        body.into_any_element()
    }
}

fn setup_step(stage: &str) -> usize {
    match stage {
        "uploading" => 1,
        "installing" | "verifying" | "activating" => 2,
        "ready" => 3,
        _ => 0,
    }
}
fn upload_speed(bytes: Option<f64>) -> String {
    match bytes.filter(|n| n.is_finite() && *n >= 0.) {
        Some(n) if n >= 1048576. => format!("{:.1} MiB/s", n / 1048576.),
        Some(n) if n >= 1024. => format!("{:.1} KiB/s", n / 1024.),
        Some(n) => format!("{n:.0} B/s"),
        None => "Measuring speed…".into(),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn upload_speed_uses_readable_units_and_distinguishes_zero_from_unknown() {
        assert_eq!(super::upload_speed(None), "Measuring speed…");
        assert_eq!(super::upload_speed(Some(0.)), "0 B/s");
        assert_eq!(super::upload_speed(Some(1536.)), "1.5 KiB/s");
        assert_eq!(super::upload_speed(Some(2097152.)), "2.0 MiB/s");
    }
}
