//! Production onboarding: inspection, explicit consent, then optional integrations.
use crate::*;
use gpui_kit::component::spinner::Spinner;

pub(super) struct Flow {
    connection: Value,
    pub phase: u8,
    specs: Value,
    browser: bool,
    clipboard: bool,
    pub dismissed: bool,
    error: Option<String>,
    /// The code of an error reaching the server, which names the cause.
    error_code: Option<String>,
    pending: bool,
    /// Failures of the current step, and when the latest one happened.
    attempts: u32,
    failed_at: Option<i64>,
}
use newport_domain::connection_scope::settings as connection_settings;
impl Newport {
    pub(super) fn setup_visible(&self) -> bool {
        self.setup_flow.as_ref().is_some_and(|f| !f.dismissed)
            || self.selected_server().is_some_and(|s| self.needs_setup(s))
                && self.setup_flow.as_ref().is_none_or(|f| !f.dismissed)
    }
    /// Setup is due until it is finished or skipped on this connection, and
    /// again once an approved Needle is found missing. Both come from the last
    /// snapshot: the backend checks Needle in the background, never while the
    /// window draws. An unchecked server (`installed` null) is not treated as
    /// missing.
    pub(super) fn needs_setup(&self, server: &Value) -> bool {
        server["setupComplete"] != true
            || server["agentInstallApproved"] == true
                && self.snapshot["runtime"]["agentStatus"][text(server, "id")]["installed"] == false
    }
    /// A server whose setup is unfinished shows only its setup, whichever
    /// page was open when it was added or selected, Settings included.
    pub(super) fn sync_setup_page(&mut self, cx: &mut Context<Self>) {
        if self.page != Page::Overview && self.setup_visible() {
            self.navigate(Page::Overview, cx);
        }
    }
    pub(super) fn poll_setup(&mut self) -> bool {
        if self.page != Page::Overview {
            return false;
        }
        let Some(server) = self.selected_server().cloned() else {
            return false;
        };
        let connection = connection_settings(&server);
        if self
            .setup_flow
            .as_ref()
            .is_some_and(|f| f.connection != connection)
        {
            self.setup_flow = None;
        }
        if self.setup_flow.is_none() && self.needs_setup(&server) {
            self.start_setup(server);
            return true;
        }
        let Some(flow) = self.setup_flow.as_mut() else {
            return false;
        };
        if !flow.pending {
            return false;
        }
        let operation = match flow.phase {
            0 => "inspect",
            1 => "install",
            2 => "finish",
            _ => return false,
        };
        let key = format!("setup-{operation}:{}", self.server);
        let Some(resource) = self.resources.get(&key) else {
            return false;
        };
        if resource.pending {
            return false;
        }
        flow.pending = false;
        flow.error = resource.error.clone();
        flow.error_code = resource.error_code.clone();
        if flow.error.is_some() {
            flow.attempts += 1;
            flow.failed_at = Some(chrono::Local::now().timestamp_millis());
        } else {
            flow.attempts = 0;
            flow.failed_at = None;
        }
        if flow.error.is_none() {
            if flow.phase == 0 {
                flow.specs = resource.value.clone();
            }
            flow.phase += 1;
            let finished = flow.phase == 3;
            self.request("snapshot", "snapshot", json!({}));
            if finished {
                self.finish_setup();
            }
        }
        true
    }
    /// Starts setup on `server` from its inspection.
    pub(super) fn start_setup(&mut self, server: Value) {
        self.setup_flow = Some(Flow {
            connection: connection_settings(&server),
            phase: 0,
            specs: Value::Null,
            browser: true,
            clipboard: true,
            dismissed: false,
            error: None,
            error_code: None,
            pending: true,
            attempts: 0,
            failed_at: None,
        });
        self.request(
            &format!("setup-inspect:{}", self.server),
            "setup_inspect",
            json!({"expectedConnection":server}),
        );
    }
    /// Choosing integrations is the last step: close the setup and show the
    /// server's Overview, keeping any note the helper left about the shell.
    fn finish_setup(&mut self) {
        if let Some(flow) = &mut self.setup_flow {
            flow.dismissed = true;
        }
        let note = self
            .resources
            .get(&format!("setup-finish:{}", self.server))
            .and_then(|r| r.value.as_str())
            .filter(|note| !note.is_empty())
            .map(str::to_owned);
        self.notification = Some(match note {
            Some(note) => Notification::warning(note),
            None => Notification::success("Server is set up."),
        });
        self.resources.invalidate("page");
        self.request(
            "page",
            "cockpit_collect",
            json!({"id":self.server,"section":"overview","refresh":true}),
        );
        self.refresh();
    }
    /// Skipping is remembered for this connection; pages that need Needle
    /// still offer setup.
    fn skip_setup(&mut self) {
        let Some(flow) = &mut self.setup_flow else {
            return;
        };
        flow.dismissed = true;
        if let Some(server) = self.selected_server().cloned() {
            self.request(
                &format!("setup-skip:{}", self.server),
                "setup_skip",
                json!({"expectedConnection":server}),
            );
        }
    }
    fn setup_request(&mut self, cx: &mut Context<Self>) {
        let Some(server) = self.selected_server().cloned() else {
            return;
        };
        let Some(flow) = self.setup_flow.as_mut() else {
            return;
        };
        if flow.pending || flow.connection != connection_settings(&server) || flow.dismissed {
            return;
        }
        let operation = match flow.phase {
            0 => "inspect",
            1 => "install",
            2 => "finish",
            _ => return,
        };
        let args =
            json!({"expectedConnection":server,"browser":flow.browser,"clipboard":flow.clipboard});
        flow.pending = true;
        // A retry keeps the failed step on screen, with the button busy, so
        // the page doesn't swap to a spinner and back. Installation shows
        // its own progress instead.
        if flow.phase == 1 {
            flow.error = None;
            flow.error_code = None;
        }
        self.request(
            &format!("setup-{operation}:{}", self.server),
            &format!("setup_{operation}"),
            args,
        );
        cx.notify();
    }
    pub(super) fn render_server_setup(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(flow) = &self.setup_flow else {
            return div()
                .p(px(16.))
                .child(Spinner::new().small())
                .into_any_element();
        };
        let phase = flow.phase;
        let specs = &flow.specs;
        let spec_text = format!(
            "{} · {}\n{} CPU cores · {} memory · {} free",
            specs["os"]
                .as_str()
                .filter(|s| !s.is_empty())
                .unwrap_or("Linux"),
            specs["arch"].as_str().unwrap_or("Unknown architecture"),
            specs["cores"]
                .as_u64()
                .map(|n| n.to_string())
                .unwrap_or("—".into()),
            gib(&specs["memoryKiB"]),
            gib(&specs["freeKiB"])
        );
        let server = self.selected_server().cloned().unwrap_or_default();
        let name = server["name"].as_str().unwrap_or("this server").to_owned();
        let address = format!(
            "{}@{}{}",
            server["sshUser"].as_str().unwrap_or_default(),
            server["sshHost"].as_str().unwrap_or_default(),
            match server["sshPort"].as_u64() {
                Some(22) | None => String::new(),
                Some(port) => format!(":{port}"),
            }
        );
        let mut timeline = div().w_full().max_w(px(480.)).flex().flex_col();
        let mut before = div().flex_1().flex().flex_col().justify_end().child(
            div()
                .flex_shrink_0()
                .ml(px(39.))
                .mb(px(28.))
                .child(crate::controls::page_title(
                    format!("Set up {name}"),
                    None,
                    cx,
                ))
                .child(
                    div()
                        .id("setup-server-address")
                        .mt(px(6.))
                        .font_family(cx.theme().mono_font_family.clone())
                        .text_size(px(crate::ui::FONT_SIZE))
                        .text_color(cx.theme().muted_foreground)
                        .child(address),
                ),
        );
        // Joins the current step to the ones below it; the last step has none.
        let mut after = div().flex_1().flex().flex_col().when(phase < 2, |after| {
            after.child(
                div()
                    .h(px(24.))
                    .flex_shrink_0()
                    .pl(px(11.))
                    .child(div().h_full().w(px(1.)).bg(cx.theme().border)),
            )
        });
        let mut current_step = None;
        for (index, label) in [
            "Inspect server",
            "Install the helper",
            "Choose integrations",
        ]
        .into_iter()
        .enumerate()
        {
            let index = index as u8;
            let done = phase > index;
            let active = phase == index;
            let failed = active && flow.error.is_some();
            let retrying = failed && flow.pending;
            let mut entry = div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(8.))
                .child(
                    div()
                        .text_size(px(crate::ui::FONT_SIZE))
                        .font_weight(if active {
                            FontWeight::SEMIBOLD
                        } else {
                            FontWeight::MEDIUM
                        })
                        .text_color(if active || done {
                            cx.theme().foreground
                        } else {
                            cx.theme().muted_foreground
                        })
                        .child(label),
                );
            if done {
                entry = entry.child(
                    div()
                        .text_size(px(crate::ui::FONT_SIZE))
                        .line_height(relative(1.7))
                        .text_color(cx.theme().muted_foreground)
                        .child(match index {
                            0 => spec_text.clone(),
                            1 => "Helper installed".into(),
                            _ => format!(
                                "Browser {} · Clipboard {}",
                                if flow.browser { "enabled" } else { "off" },
                                if flow.clipboard { "enabled" } else { "off" }
                            ),
                        }),
                );
            }
            if active {
                let mut body = div().mt(px(8.)).min_w_0().flex().flex_col().gap(px(16.));
                if flow.pending && !retrying {
                    let progress = &self.snapshot["runtime"]["agentInstallation"][&self.server];
                    let title = match phase {
                        0 => "Inspecting your server…",
                        1 => {
                            if progress["stage"] == "uploading" {
                                "Copying the helper…"
                            } else if progress["stage"] == "verifying" {
                                "Checking the helper…"
                            } else {
                                "Installing the helper…"
                            }
                        }
                        _ => "Applying integrations…",
                    };
                    body = body.child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(12.))
                            .child(Spinner::new().small())
                            .child(div().text_size(px(crate::ui::FONT_SIZE)).child(title)),
                    );
                    if phase == 1 && progress["stage"] == "uploading" {
                        let sent = progress["sent"].as_f64().unwrap_or(0.) / 1048576.;
                        let total = progress["total"]
                            .as_f64()
                            .filter(|n| *n > 0.)
                            .or_else(|| specs["total"].as_f64())
                            .unwrap_or(0.)
                            / 1048576.;
                        let speed = progress["bytesPerSecond"]
                            .as_f64()
                            .map(|n| format!("{:.1} KiB/s", n / 1024.))
                            .unwrap_or("Measuring speed…".into());
                        body = body.child(
                            div()
                                .text_size(px(crate::ui::FONT_SIZE))
                                .text_color(cx.theme().muted_foreground)
                                .child(format!("{sent:.1} of {total:.1} MiB · {speed}")),
                        );
                    }
                } else if flow.error.is_none() && phase == 1 {
                    body=body.child(crate::controls::dialog_title("Install the helper?"))
                        .child(div().text_size(px(crate::ui::FONT_SIZE)).text_color(cx.theme().muted_foreground).child(if crate::remote_apps_install::available() {
                            "Needle, a small tool for server metrics, Git, remote apps, and optional browser and clipboard integration."
                        } else {
                            "Needle, a small tool for server metrics, Git, and optional browser and clipboard integration."
                        }))
                        .child(div().text_size(px(crate::ui::FONT_SIZE)).text_color(cx.theme().muted_foreground).child(format!("{:.1} MiB · copied over SSH",specs["total"].as_f64().unwrap_or(0.)/1048576.)))
                        .child(div().text_size(px(crate::ui::FONT_SIZE)).text_color(cx.theme().muted_foreground).child("Browser and clipboard access stay off until you choose them below."));
                } else if flow.error.is_none() && phase == 2 {
                    for (browser, label, checked) in [
                        (true, "Open server links in your browser", flow.browser),
                        (
                            false,
                            "Share your clipboard with this server",
                            flow.clipboard,
                        ),
                    ] {
                        body = body.child(
                            div()
                                .flex()
                                .items_center()
                                .justify_between()
                                .gap(px(16.))
                                .py(px(8.))
                                .border_t_1()
                                .border_color(cx.theme().border)
                                .child(
                                    div()
                                        .flex_1()
                                        .text_size(px(crate::ui::FONT_SIZE))
                                        .child(label),
                                )
                                .child(
                                    crate::controls::switch(if browser {
                                        "setup-browser"
                                    } else {
                                        "setup-clipboard"
                                    })
                                    .accessibility_label(label)
                                    .checked(checked)
                                    .on_click(cx.listener(
                                        move |this, value, _, cx| {
                                            if let Some(flow) = &mut this.setup_flow
                                                && !flow.pending
                                            {
                                                if browser {
                                                    flow.browser = *value;
                                                } else {
                                                    flow.clipboard = *value;
                                                }
                                            }
                                            cx.notify();
                                        },
                                    )),
                                ),
                        );
                    }
                }
                let copy = flow
                    .error
                    .as_ref()
                    .map(|_| error_copy(phase, flow.error_code.as_deref()));
                if let (Some(error), Some(copy)) = (&flow.error, copy) {
                    // The cause usually follows a summary line, so show the
                    // whole reply; only a very long one is cut short.
                    let lines: Vec<&str> = error
                        .lines()
                        .map(str::trim_end)
                        .filter(|l| !l.trim().is_empty())
                        .collect();
                    let mut shown = lines.iter().take(8).copied().collect::<Vec<_>>().join("\n");
                    if lines.len() > 8 {
                        shown.push_str("\n…");
                    }
                    body = body.child(
                        div()
                            .id("setup-error")
                            .role(Role::Alert)
                            .aria_label(format!("{} {}", copy.title, copy.guidance))
                            .flex()
                            .flex_col()
                            .gap(px(8.))
                            .child(
                                div()
                                    .text_size(px(crate::ui::FONT_SIZE))
                                    .line_height(relative(1.6))
                                    .child(div().font_weight(FontWeight::MEDIUM).child(copy.title))
                                    .child(
                                        div()
                                            .text_color(cx.theme().muted_foreground)
                                            .child(copy.guidance),
                                    ),
                            )
                            .child(
                                div()
                                    .id("setup-technical-error")
                                    .debug_selector(|| "setup-technical-error".into())
                                    .border_l_2()
                                    .border_color(crate::theme::error(cx.theme().is_dark()))
                                    .bg(crate::theme::error(cx.theme().is_dark()).opacity(0.06))
                                    .px(px(12.))
                                    .py(px(8.))
                                    .font_family(cx.theme().mono_font_family.clone())
                                    .text_size(px(crate::ui::FONT_SIZE))
                                    .line_height(relative(1.5))
                                    .text_color(crate::theme::error(cx.theme().is_dark()))
                                    .whitespace_normal()
                                    .child(shown),
                            ),
                    );
                }
                if !flow.pending || retrying {
                    let fix = copy.and_then(|copy| copy.connection).filter(|_| phase == 0);
                    let fix_connection = fix.is_some();
                    body = body.child(
                        div()
                            .flex()
                            .flex_wrap()
                            .items_center()
                            .gap(px(6.))
                            .when(fix_connection, |actions| {
                                actions.child(self.edit_connection_button(fix, retrying, cx))
                            })
                            .child(
                                crate::controls::button("setup-action")
                                    .when(!fix_connection, |b| b.primary())
                                    .when(flow.error.is_some(), |b| b.icon(IconName::RefreshCw))
                                    .label(if flow.error.is_some() {
                                        "Try again"
                                    } else if phase == 1 {
                                        "Allow"
                                    } else {
                                        "Save and finish"
                                    })
                                    .loading(retrying)
                                    .disabled(retrying)
                                    .on_click(cx.listener(|this, _, _, cx| this.setup_request(cx))),
                            )
                            .when(
                                phase == 0 && flow.error.is_some() && !fix_connection,
                                |actions| {
                                    actions.child(self.edit_connection_button(None, retrying, cx))
                                },
                            )
                            // Declining the helper is always possible, and so is leaving a
                            // failed integrations step; the sidebar is hidden meanwhile.
                            .when(
                                phase == 1 || (phase == 2 && flow.error.is_some()),
                                |actions| {
                                    actions.child(
                                        crate::controls::button("setup-skip")
                                            .ghost()
                                            .label("Skip")
                                            .disabled(retrying)
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.skip_setup();
                                                cx.notify();
                                            })),
                                    )
                                },
                            )
                            .when_some(flow.error.clone(), |actions, error| {
                                actions.child(
                                    crate::controls::button("setup-copy-details")
                                        .ghost()
                                        .icon(IconName::Copy)
                                        .label("Copy details")
                                        .on_click(move |_, _, cx| {
                                            cx.write_to_clipboard(ClipboardItem::new_string(
                                                error.clone(),
                                            ))
                                        }),
                                )
                            }),
                    );
                    if flow.attempts > 1
                        && let Some(at) = flow.failed_at
                    {
                        body = body.child(
                            div()
                                .id("setup-attempts")
                                .debug_selector(|| "setup-attempts".into())
                                .text_size(px(crate::ui::FONT_SIZE))
                                .text_color(cx.theme().muted_foreground)
                                .child(format!(
                                    "Tried {} times · last at {}",
                                    flow.attempts,
                                    crate::localized_time::file_modified(at, at)
                                )),
                        );
                    }
                }
                entry = entry.child(body);
            }
            let marker = div()
                .w(px(23.))
                .h(px(23.))
                .flex_shrink_0()
                .rounded_full()
                .flex()
                .items_center()
                .justify_center()
                .text_size(px(crate::ui::FONT_SIZE))
                .bg(if failed {
                    crate::theme::error(cx.theme().is_dark())
                } else if active {
                    cx.theme().foreground
                } else {
                    cx.theme().muted
                })
                .text_color(if failed || active {
                    cx.theme().background
                } else {
                    cx.theme().muted_foreground
                })
                .when(done, |v| v.child(Icon::new(IconName::Check).size(px(12.))))
                .when(failed, |v| v.child("!"))
                .when(!done && !failed, |v| v.child((index + 1).to_string()));
            let row = div()
                .flex_shrink_0()
                .flex()
                .gap(px(16.))
                .child(
                    div()
                        .w(px(23.))
                        .flex_shrink_0()
                        .flex()
                        .flex_col()
                        .items_center()
                        .child(marker)
                        .when(index < 2, |v| {
                            v.child(
                                div()
                                    .w(px(1.))
                                    .flex_1()
                                    .min_h(px(16.))
                                    .mt(px(6.))
                                    .bg(cx.theme().border),
                            )
                        }),
                )
                .child(entry.when(!active, |entry| entry.pb(px(24.))));
            if active {
                current_step = Some(row.debug_selector(|| "setup-current-step".into()));
            } else if done {
                before = before.child(row);
            } else {
                after = after.child(row);
            }
        }
        if let Some(current_step) = current_step {
            timeline = timeline
                .flex_1()
                .child(before)
                .child(current_step)
                .child(after);
        }
        div()
            .id("server-setup-timeline")
            .debug_selector(|| "server-setup-viewport".into())
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .child(
                div()
                    .w_full()
                    .min_h_full()
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .p(px(16.))
                    .child(timeline.flex_shrink_0()),
            )
            .into_any_element()
    }
    /// Opens the connection settings; with a likely `fix`, it leads the
    /// actions and focuses that field.
    fn edit_connection_button(
        &self,
        fix: Option<server_form::Fix>,
        disabled: bool,
        cx: &mut Context<Self>,
    ) -> Button {
        crate::controls::button("setup-edit-connection")
            .when(fix.is_some(), |b| b.primary())
            .disabled(disabled)
            .label("Edit connection")
            .on_click(cx.listener(move |this, _, window, cx| {
                if let Some(server) = this.selected_server() {
                    this.edit_server_at(Some(server.clone()), fix, window, cx);
                }
            }))
    }
}
#[derive(Clone, Copy)]
struct ErrorCopy {
    title: &'static str,
    guidance: &'static str,
    /// The fix is most likely in the server's connection settings, there.
    connection: Option<server_form::Fix>,
}
fn error_copy(phase: u8, code: Option<&str>) -> ErrorCopy {
    let copy = |title, guidance, connection| ErrorCopy {
        title,
        guidance,
        connection,
    };
    if phase == 1 {
        return copy(
            "The helper couldn’t be installed.",
            "Check the connection and try again.",
            None,
        );
    }
    if phase == 2 {
        return copy(
            "Integrations couldn’t be enabled.",
            "The helper is installed. Try again to save your integration choices.",
            None,
        );
    }
    match code {
        Some("SSH_HOST_KEY_CHANGED") => copy(
            "This server’s identity changed.",
            "Its SSH host key doesn’t match the one saved on this Mac. Make sure it’s the server you expect before you trust it:",
            None,
        ),
        Some("SSH_HOST_KEY_REJECTED") => copy(
            "Newport can’t trust this server’s identity.",
            "Its SSH host key was refused:",
            None,
        ),
        Some("SSH_UNREACHABLE") => copy(
            "The server couldn’t be reached.",
            "Check the server address and SSH port, and that SSH is running.",
            Some(server_form::Fix::Address),
        ),
        Some("SSH_TIMED_OUT") => copy(
            "The server isn’t responding.",
            "Check that it’s online and reachable from your network, then try again.",
            None,
        ),
        Some("SSH_AUTH_REFUSED") => copy(
            "Newport couldn’t sign in.",
            "Check the SSH username and key or password.",
            Some(server_form::Fix::SignIn),
        ),
        Some("SSH_CREDENTIALS") => copy(
            "Newport couldn’t use this server’s SSH credentials.",
            "Check the key file or saved password for this server.",
            Some(server_form::Fix::SignIn),
        ),
        Some("SSH_APPROVAL_REQUIRED") => copy(
            "The SSH key wasn’t approved.",
            "Try again and approve the request in your SSH agent.",
            None,
        ),
        Some("SSH_AGENT_UNAVAILABLE") => copy(
            "The SSH agent isn’t available.",
            "Unlock or start your SSH agent, then try again.",
            Some(server_form::Fix::SignIn),
        ),
        Some("SSH_PROXY_FAILED") => copy(
            "Newport couldn’t connect through the proxy command.",
            "Check the proxy command in the connection settings. It reported:",
            Some(server_form::Fix::Proxy),
        ),
        Some("SSH_INVALID_PROXY") => copy(
            "Newport couldn’t run the proxy command.",
            "Check the proxy command in the connection settings:",
            Some(server_form::Fix::Proxy),
        ),
        Some("SSH_INVALID_JUMP") => copy(
            "Newport couldn’t connect through the jump host.",
            "Check which server this one connects through in the connection settings:",
            Some(server_form::Fix::Proxy),
        ),
        Some("SSH_LOST") => copy(
            "The connection to the server was lost.",
            "Check your network, then try again.",
            None,
        ),
        Some("SSH_SESSIONS_DISABLED") => copy(
            "The server doesn’t allow SSH sessions.",
            "Allow sessions in its SSH settings (MaxSessions), then try again.",
            None,
        ),
        Some("SSH_CHANNEL_REFUSED") => copy(
            "The server refused a request from Newport.",
            "Its SSH settings may not allow it. Check them, then try again.",
            None,
        ),
        Some("SSH_REQUEST_DENIED") => copy(
            "The server refused to forward a port.",
            "Another connection may still hold the port. Try again in a moment.",
            None,
        ),
        Some("SSH_BUSY") => copy(
            "Too many requests are waiting for this server.",
            "Wait a moment, then try again.",
            None,
        ),
        Some("SSH_CLOSED") => copy(
            "The connection closed before setup finished.",
            "Try again.",
            None,
        ),
        _ => copy(
            "Newport couldn’t read this server’s details.",
            "The server replied:",
            None,
        ),
    }
}

fn gib(value: &Value) -> String {
    value
        .as_f64()
        .map(|n| format!("{:.1} GiB", n / 1048576.))
        .unwrap_or("Unavailable".into())
}

#[cfg(test)]
mod tests {
    use crate::{Newport, Page, page_perf_tests, theme};
    use gpui_kit::{
        self as gpui, AppContext, Context, Entity, IntoElement, Render, TestAppContext, Window,
        div, prelude::*, px, size,
    };
    use serde_json::json;
    struct Host(Entity<Newport>);
    impl Render for Host {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            self.0.update(cx, |this, cx| {
                div().size_full().flex().flex_col().child(this.overview(cx))
            })
        }
    }
    #[gpui::test]
    fn changing_how_the_server_is_reached_starts_setup_over(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_kit::component::init(cx);
            theme::install(false, cx);
            cx.set_reduce_motion(true);
        });
        let mut owner = None;
        let (_, visual) = cx.add_window_view(|window, cx| {
            let view = cx.new(|cx| {
                let mut view = page_perf_tests::owner(window, cx);
                view.page = Page::Overview;
                view.setup_flow = Some(super::Flow {
                    connection: super::connection_settings(view.selected_server().unwrap()),
                    phase: 1,
                    specs: json!({"cores":8,"total":1000}),
                    browser: true,
                    clipboard: true,
                    dismissed: false,
                    error: None,
                    error_code: None,
                    pending: false,
                    attempts: 0,
                    failed_at: None,
                });
                view
            });
            owner = Some(view.clone());
            Host(view)
        });
        let view = owner.unwrap();
        view.update(visual, |view, _| {
            assert!(!view.poll_setup() || view.setup_flow.as_ref().is_some_and(|f| f.phase == 1));
            let id = view.server.clone();
            let servers = view.snapshot["config"]["servers"].as_array_mut().unwrap();
            let server = servers.iter_mut().find(|s| s["id"] == id.as_str()).unwrap();
            server["proxy"] = json!({"command": "nc %h %p"});
            view.poll_setup();
            assert!(
                view.setup_flow.as_ref().is_none_or(|flow| flow.phase == 0),
                "the flow for the old route goes"
            );
        });
    }

    #[gpui::test]
    fn visible_setup_receives_installation_progress_without_restarting_inspection(
        cx: &mut TestAppContext,
    ) {
        cx.update(|cx| {
            gpui_kit::component::init(cx);
            theme::install(false, cx);
            cx.set_reduce_motion(true);
        });
        let mut owner = None;
        let (_, visual) = cx.add_window_view(|window, cx| {
            let view = cx.new(|cx| {
                let mut view = page_perf_tests::owner(window, cx);
                view.page = Page::Overview;
                view.setup_flow = Some(super::Flow {
                    connection: super::connection_settings(view.selected_server().unwrap()),
                    phase: 1,
                    specs: json!({"cores":8,"total":1000}),
                    browser: true,
                    clipboard: true,
                    dismissed: false,
                    error: None,
                    error_code: None,
                    pending: true,
                    attempts: 0,
                    failed_at: None,
                });
                view
            });
            owner = Some(view.clone());
            Host(view)
        });
        let view = owner.unwrap();
        let (work, _requests) = std::sync::mpsc::sync_channel(16);
        let (replies, inbox) = std::sync::mpsc::channel();
        view.update(visual, |view, _| {
            view.work = work.into();
            view.replies = inbox;
            let generation = view.resources.begin("snapshot").unwrap();
            view.resources
                .complete("snapshot", generation, Ok(view.snapshot.clone()));
        });
        page_perf_tests::settle_update_check(&view, visual);
        let notices = visual.new(|cx: &mut Context<usize>| {
            cx.observe(&view, |count, _, _| *count += 1).detach();
            0
        });
        for sent in [100, 200, 300] {
            let (generation, mut snapshot) = view.update(visual, |view, _| {
                (
                    view.resources.begin_refresh("snapshot", false).unwrap(),
                    view.snapshot.clone(),
                )
            });
            snapshot["runtime"]["agentInstallation"]["fixture"] =
                json!({"stage":"uploading","sent":sent,"total":1000,"bytesPerSecond":100});
            replies
                .send(crate::Completion {
                    pdf: None,
                    pdf_text: None,
                    image: None,
                    server: "fixture".into(),
                    key: "snapshot".into(),
                    generation,
                    result: Ok(snapshot),
                })
                .unwrap();
            view.update(visual, |view, cx| view.tick(cx));
            page_perf_tests::frame(visual);
        }
        assert_eq!(notices.read_with(visual, |count, _| *count), 3);
        view.read_with(visual, |view, _| {
            let flow = view.setup_flow.as_ref().unwrap();
            assert_eq!(flow.phase, 1);
            assert_eq!(flow.specs["cores"], 8);
            assert!(flow.pending);
            assert_eq!(
                view.snapshot["runtime"]["agentInstallation"]["fixture"]["sent"],
                300
            );
        });
    }

    #[gpui::test]
    fn connection_error_shows_the_reply_and_leads_with_the_connection_fix(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_kit::component::init(cx);
            theme::install(false, cx);
        });
        let (_, visual) = cx.add_window_view(|window, cx| {
            let view = cx.new(|cx| {
                let mut v = page_perf_tests::owner(window, cx);
                v.page = Page::Overview;
                v.setup_flow = Some(super::Flow {
                    connection: super::connection_settings(v.selected_server().unwrap()),
                    phase: 0,
                    specs: serde_json::Value::Null,
                    browser: true,
                    clipboard: true,
                    dismissed: false,
                    error: Some(
                        "The server couldn't be reached: Connection refused (os error 61)".into(),
                    ),
                    error_code: Some("SSH_UNREACHABLE".into()),
                    pending: false,
                    attempts: 2,
                    failed_at: Some(0),
                });
                v
            });
            Host(view)
        });
        for dark in [false, true] {
            visual.update(|_, cx| theme::install(dark, cx));
            for width in [420., 1000.] {
                visual.simulate_resize(size(px(width), px(900.)));
                visual.update(|window, cx| window.draw(cx).clear(cx));
                let active = visual.debug_bounds("setup-current-step").unwrap();
                assert!(f32::from(active.size.width) <= 480.);
                assert!(active.left() >= px(16.));
                assert!(active.right() <= px(width - 16.));
                // The server's own reply is visible without another click.
                assert!(visual.debug_bounds("setup-technical-error").is_some());
                visual.update(|window, _| {
                    let nodes = gpui_kit::base::test_support::snapshots(window);
                    for label in ["Edit connection", "Try again", "Copy details"] {
                        assert!(
                            nodes.iter().any(|n| n.label() == Some(label)),
                            "missing {label}"
                        );
                    }
                    assert!(!nodes.iter().any(|n| n.label() == Some("Show details")));
                });
                assert!(visual.debug_bounds("setup-attempts").is_some());
            }
        }
        // A refused connection is fixed in its settings, which come first.
        let (edit, retry) = visual.update(|window, _| {
            let nodes = gpui_kit::base::test_support::snapshots(window);
            let at = |label| {
                nodes
                    .iter()
                    .find(|n| n.label() == Some(label))
                    .unwrap()
                    .bounds()
            };
            (at("Edit connection"), at("Try again"))
        });
        assert!(edit.left() < retry.left());
        // Retrying keeps the failed step in place instead of swapping it out.
        let before = visual.debug_bounds("setup-current-step").unwrap();
        visual.simulate_click(retry.center(), Default::default());
        visual.update(|window, cx| window.draw(cx).clear(cx));
        assert!(visual.debug_bounds("setup-technical-error").is_some());
        assert_eq!(
            visual.debug_bounds("setup-current-step").unwrap().top(),
            before.top()
        );
    }

    #[test]
    fn a_changed_host_key_reads_as_a_changed_identity_not_a_bad_reply() {
        let copy = super::error_copy(0, Some("SSH_HOST_KEY_CHANGED"));
        assert_eq!(copy.title, "This server’s identity changed.");
        assert!(copy.connection.is_none());
    }

    #[test]
    fn every_ssh_error_code_has_its_own_wording() {
        // Every code of `newport_ssh::Error::code`.
        let codes = [
            "SSH_UNREACHABLE",
            "SSH_PROXY_FAILED",
            "SSH_INVALID_PROXY",
            "SSH_INVALID_JUMP",
            "SSH_TIMED_OUT",
            "SSH_LOST",
            "SSH_AGENT_UNAVAILABLE",
            "SSH_APPROVAL_REQUIRED",
            "SSH_AUTH_REFUSED",
            "SSH_CREDENTIALS",
            "SSH_HOST_KEY_CHANGED",
            "SSH_HOST_KEY_REJECTED",
            "SSH_SESSIONS_DISABLED",
            "SSH_CHANNEL_REFUSED",
            "SSH_REQUEST_DENIED",
            "SSH_BUSY",
            "SSH_CLOSED",
        ];
        let reply = super::error_copy(0, None).title;
        let mut titles = std::collections::HashSet::new();
        for code in codes {
            let copy = super::error_copy(0, Some(code));
            assert_ne!(copy.title, reply, "{code} has no wording");
            assert!(titles.insert(copy.title), "{code} repeats a title");
            for text in [copy.title, copy.guidance] {
                assert!(!text.contains('\''), "{code}: use a curly apostrophe");
                assert!(
                    text.starts_with(char::is_uppercase),
                    "{code}: sentence case"
                );
            }
        }
    }

    #[test]
    fn errors_reaching_the_server_are_worded_by_their_code() {
        for (code, title, fix) in [
            (
                "SSH_UNREACHABLE",
                "The server couldn’t be reached.",
                Some(crate::server_form::Fix::Address),
            ),
            (
                "SSH_AUTH_REFUSED",
                "Newport couldn’t sign in.",
                Some(crate::server_form::Fix::SignIn),
            ),
            (
                "SSH_PROXY_FAILED",
                "Newport couldn’t connect through the proxy command.",
                Some(crate::server_form::Fix::Proxy),
            ),
            (
                "SSH_INVALID_JUMP",
                "Newport couldn’t connect through the jump host.",
                Some(crate::server_form::Fix::Proxy),
            ),
            ("SSH_TIMED_OUT", "The server isn’t responding.", None),
        ] {
            let copy = super::error_copy(0, Some(code));
            assert_eq!((copy.title, copy.connection), (title, fix), "{code}");
        }
        // A reply mentioning a cause is the server's output, not that cause.
        let reply = super::error_copy(0, None);
        assert_eq!(reply.title, "Newport couldn’t read this server’s details.");
        assert!(reply.connection.is_none());
    }

    #[gpui::test]
    fn an_unfinished_server_opens_its_setup_from_any_page(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_kit::component::init(cx);
            theme::install(false, cx);
        });
        let mut owner = None;
        let (_, visual) = cx.add_window_view(|window, cx| {
            let view = cx.new(|cx| {
                let mut v = page_perf_tests::owner(window, cx);
                // A server just added while another server's Ports page was open.
                v.page = Page::Ports;
                v.snapshot["config"]["servers"][0]["setupComplete"] = json!(false);
                v
            });
            owner = Some(view.clone());
            Host(view)
        });
        let view = owner.unwrap();
        let (work, requests) = std::sync::mpsc::sync_channel(16);
        view.update(visual, |v, cx| {
            v.work = work.into();
            v.sync_setup_page(cx);
            assert!(v.page == Page::Overview);
            assert!(v.setup_visible());
        });
        assert!(requests.try_iter().any(|r| r.method == "setup_inspect"));
        view.update(visual, |v, cx| {
            // Not even Settings opens before setup is finished.
            v.page = Page::Settings;
            v.sync_setup_page(cx);
            assert!(v.page == Page::Overview);
            // A finished server keeps whatever page was open.
            v.page = Page::Ports;
            v.setup_flow = None;
            v.snapshot["config"]["servers"][0]["setupComplete"] = json!(true);
            v.sync_setup_page(cx);
            assert!(v.page == Page::Ports);
            // Not yet checked: no reason to interrupt.
            v.snapshot["runtime"]["agentStatus"]["fixture"] = json!({"installed": null});
            v.sync_setup_page(cx);
            assert!(v.page == Page::Ports);
            // Needle found missing on a finished server: setup again.
            v.snapshot["runtime"]["agentStatus"]["fixture"] = json!({"installed": false});
            v.sync_setup_page(cx);
            assert!(v.page == Page::Overview);
            assert!(v.setup_visible());
        });
    }

    #[gpui::test]
    fn a_skipped_setup_is_remembered_until_set_up_is_chosen(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_kit::component::init(cx);
            theme::install(false, cx);
        });
        let mut owner = None;
        let (_, visual) = cx.add_window_view(|window, cx| {
            let view = cx.new(|cx| {
                let mut v = page_perf_tests::owner(window, cx);
                v.page = Page::Overview;
                v.snapshot["config"]["servers"][0]["setupComplete"] = json!(false);
                v.snapshot["config"]["servers"][0]["agentInstallApproved"] = json!(false);
                v.snapshot["runtime"]["agentStatus"]["fixture"] = json!({"installed": false});
                v
            });
            owner = Some(view.clone());
            Host(view)
        });
        let view = owner.unwrap();
        let (work, requests) = std::sync::mpsc::sync_channel(16);
        view.update(visual, |v, _| {
            v.work = work.into();
            v.poll_setup();
            v.skip_setup();
        });
        let skip = requests
            .try_iter()
            .find(|r| r.method == "setup_skip")
            .unwrap();
        assert_eq!(skip.args["expectedConnection"]["id"], "fixture");
        view.update(visual, |v, cx| {
            // Saved: a later window, or a return to this server, has no
            // setup to show, and its pages say Needle isn't set up.
            v.setup_flow = None;
            v.snapshot["config"]["servers"][0]["setupComplete"] = json!(true);
            v.sync_setup_page(cx);
            assert!(!v.poll_setup());
            assert!(!v.setup_visible());
            assert!(v.needle_missing());
            v.set_up_needle(cx);
            assert!(v.setup_visible());
        });
        assert!(requests.try_iter().any(|r| r.method == "setup_inspect"));
    }

    #[gpui::test]
    fn timeline_waits_for_consent_and_keeps_errors_on_the_current_step(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_kit::component::init(cx);
            theme::install(false, cx);
        });
        let mut owner = None;
        let (_, visual) = cx.add_window_view(|window, cx| {
            let view = cx.new(|cx| {
                let mut v = page_perf_tests::owner(window, cx);
                v.page = Page::Overview;
                v.snapshot["config"]["servers"][0]["setupComplete"] = json!(false);
                v.snapshot["config"]["servers"][0]["agentInstallApproved"] = json!(false);
                v
            });
            owner = Some(view.clone());
            Host(view)
        });
        let view = owner.unwrap();
        let (work, requests) = std::sync::mpsc::sync_channel(64);
        let (replies, completions) = std::sync::mpsc::channel();
        view.update(visual, |v, _| {
            v.work = work.into();
            v.replies = completions;
            v.refresh();
            assert_eq!(v.setup_flow.as_ref().unwrap().phase, 0);
        });
        let inspect = requests.try_recv().unwrap();
        assert_eq!(inspect.method, "setup_inspect");
        view.update(visual,|v,_| {
            v.resources.complete(&inspect.key,inspect.generation,Ok(json!({"os":"Ubuntu","arch":"x86_64","cores":4,"memoryKiB":8388608,"freeKiB":44040192,"total":5242880})));
            v.poll_setup();assert_eq!(v.setup_flow.as_ref().unwrap().phase,1);
        });
        assert!(requests.try_iter().all(|r| r.method != "setup_install"));
        visual.simulate_resize(size(px(1000.), px(1000.)));
        visual.update(|window, cx| {
            window.draw(cx).clear(cx);
        });
        let viewport = visual.debug_bounds("server-setup-viewport").unwrap();
        let active = visual.debug_bounds("setup-current-step").unwrap();
        assert!(
            (f32::from(active.center().y - viewport.center().y)).abs() < 2.,
            "active step must be vertically centered: {active:?} in {viewport:?}"
        );
        assert!(
            (f32::from(active.center().x - viewport.center().x)).abs() < 2.,
            "active step must be horizontally centered"
        );
        for dark in [false, true] {
            visual.update(|_, cx| theme::install(dark, cx));
            visual.simulate_resize(size(px(420.), px(680.)));
            visual.update(|window, cx| {
                window.draw(cx).clear(cx);
                let nodes = gpui_kit::base::test_support::snapshots(window);
                assert!(nodes.iter().any(|n| n.label() == Some("Allow")));
                assert!(!nodes.iter().any(|n| n.label() == Some("Back")));
            });
        }
        view.update(visual, |v, cx| v.setup_request(cx));
        let install = requests.try_recv().unwrap();
        assert_eq!(install.method, "setup_install");
        view.update(visual, |v, cx| {
            v.navigate(Page::Integration, cx);
            assert!(v.resources.get(&install.key).unwrap().pending);
            v.navigate(Page::Overview, cx);
            let flow = v.setup_flow.as_ref().unwrap();
            assert_eq!(flow.phase, 1);
            assert!(flow.pending);
            assert_eq!(flow.specs["total"], 5242880);
        });
        assert!(
            requests
                .try_iter()
                .all(|r| r.method != "setup_inspect" && r.method != "setup_install")
        );

        // Saving consent emits a snapshot before the installation reply arrives.
        // It must not reset the timeline or discard the pending operation/specs.
        view.update(visual, |v, cx| {
            let mut snapshot = v.snapshot.clone();
            snapshot["config"]["servers"][0]["agentInstallApproved"] = json!(true);
            v.resources.invalidate("snapshot");
            let generation = v.resources.begin("snapshot").unwrap();
            replies
                .send(crate::Completion {
                    pdf: None,
                    pdf_text: None,
                    image: None,
                    server: v.server.clone(),
                    key: "snapshot".into(),
                    generation,
                    result: Ok(snapshot),
                })
                .unwrap();
            v.tick(cx);
            let flow = v.setup_flow.as_ref().unwrap();
            assert_eq!(flow.phase, 1);
            assert!(flow.pending);
            assert_eq!(flow.specs["total"], 5242880);
            assert!(v.resources.get(&install.key).unwrap().pending);
        });
        requests.try_iter().for_each(drop);

        view.update(visual, |v, _| {
            v.resources.complete(
                &install.key,
                install.generation,
                Err("Transfer interrupted".into()),
            );
            v.poll_setup();
            let f = v.setup_flow.as_ref().unwrap();
            assert_eq!(f.phase, 1);
            assert!(f.error.is_some());
        });
        view.update(visual, |v, cx| v.setup_request(cx));
        let retry = requests.try_recv().unwrap();
        view.update(visual, |v, _| {
            v.resources
                .complete(&retry.key, retry.generation, Ok(json!(null)));
            v.poll_setup();
            let f = v.setup_flow.as_ref().unwrap();
            assert_eq!(f.phase, 2);
            assert!(f.browser && f.clipboard);
        });
        requests.try_iter().for_each(drop);
        view.update(visual, |v, cx| {
            v.setup_flow.as_mut().unwrap().clipboard = false;
            v.setup_request(cx);
        });
        let finish = requests.try_recv().unwrap();
        assert_eq!(finish.method, "setup_finish");
        assert_eq!(finish.args["clipboard"], false);
        requests.try_iter().for_each(drop);
        view.update(visual, |v, cx| {
            let mut snapshot = v.snapshot.clone();
            snapshot["config"]["servers"][0]["setupComplete"] = json!(true);
            snapshot["config"]["servers"][0]["browserEnabled"] = json!(true);
            v.resources.invalidate("snapshot");
            let generation = v.resources.begin("snapshot").unwrap();
            replies
                .send(crate::Completion {
                    pdf: None,
                    pdf_text: None,
                    image: None,
                    server: v.server.clone(),
                    key: "snapshot".into(),
                    generation,
                    result: Ok(snapshot),
                })
                .unwrap();
            v.tick(cx);
            assert_eq!(v.setup_flow.as_ref().unwrap().phase, 2);
            assert!(v.resources.get(&finish.key).unwrap().pending);
        });

        view.update(visual, |v, _| {
            v.resources.complete(
                &finish.key,
                finish.generation,
                Ok(json!(
                    "Manual shell setup needed. /home/fixture/.bashrc is a symlink."
                )),
            );
            v.poll_setup();
            // Choosing integrations closes the setup straight away.
            assert_eq!(v.setup_flow.as_ref().unwrap().phase, 3);
            assert!(!v.setup_visible());
            assert!(v.page == Page::Overview);
            assert!(v.notification.is_some(), "the shell note is not dropped");
        });
        visual.update(|window, cx| {
            window.draw(cx).clear(cx);
            assert!(
                !gpui_kit::base::test_support::snapshots(window)
                    .iter()
                    .any(|n| n.label() == Some("Done"))
            );
        });
        let reading = requests
            .try_iter()
            .find(|r| r.method == "cockpit_collect")
            .unwrap();
        assert_eq!(reading.args["refresh"], true);
    }
}
