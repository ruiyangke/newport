//! The Needle agent on a server: install or update it, or uninstall it.
//! Uninstalling is final and asks first.
use crate::integration::description;
use crate::*;

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
            self.request(
                &key(&self.server),
                "uninstall_agent",
                json!({ "expectedConnection": confirmation.server }),
            );
            // Uninstalling starts setup over, even one finished in this window.
            self.setup_flow = None;
            self.resources.invalidate_prefix("setup-");
        }
        cx.notify();
    }

    pub(super) fn needle_management(&self, cx: &mut Context<Self>) -> AnyElement {
        let status = &self.snapshot["runtime"]["agentStatus"][&self.server];
        let request = self.resources.get(&key(&self.server));
        let busy = self.integration_busy(&self.server)
            || self.integration_installing(&self.server)
            || !self.configuration_available()
            || self.server_form.is_some();
        let mut content = div()
            .id("needle-management")
            .flex()
            .flex_col()
            .gap_2()
            .mt_3()
            .min_w_0();
        if request.is_some_and(|r| r.pending) {
            return content
                .child(status_line("Uninstalling Needle…", cx))
                .into_any_element();
        }
        if let Some(error) = request.and_then(|r| r.error.as_deref()) {
            content = content.child(
                crate::controls::alert("needle-uninstall-error", error.to_owned(), cx)
                    .test_support()
                    .line_height(relative(1.6)),
            );
        }
        if self
            .needle_uninstall_confirmations
            .contains_key(&self.server)
        {
            return content
                .child(description(
                    "needle-uninstall-warning",
                    "Uninstalling stops Needle and deletes it from this server, with its command links, its shell setup and everything it keeps. This can’t be undone; you can install it again later. SSH terminals and port forwarding keep working.",
                    cx,
                ))
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .gap_2()
                        .child(
                            crate::controls::button("needle-uninstall-confirm")
                                .label("Uninstall")
                                .danger()
                                .disabled(busy)
                                .on_click(
                                    cx.listener(|this, _, _, cx| this.uninstall_needle(cx)),
                                ),
                        )
                        .child(
                            crate::controls::button("needle-uninstall-cancel")
                                .label("Cancel")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    let server = this.server.clone();
                                    this.needle_uninstall_confirmations.remove(&server);
                                    cx.notify();
                                })),
                        ),
                )
                .into_any_element();
        }
        // Without Needle, the one way in is server setup, as on every page
        // that needs it.
        if self.needle_missing() {
            return content
                .child(
                    crate::controls::button("set-up-needle")
                        .label("Set up Needle")
                        .disabled(busy)
                        .on_click(cx.listener(|this, _, _, cx| this.set_up_needle(cx))),
                )
                .into_any_element();
        }
        let id = self.server.clone();
        let actions = div()
            .flex()
            .flex_wrap()
            .gap_2()
            .child(
                crate::controls::button("integration-reinstall")
                    .label(if status["updateAvailable"] == true {
                        "Update Needle"
                    } else {
                        "Reinstall Needle"
                    })
                    .disabled(busy)
                    .on_click(
                        cx.listener(move |this, _, _, cx| this.integration_command(&id, None, cx)),
                    ),
            )
            .child(
                crate::controls::button("needle-uninstall")
                    .label("Uninstall Needle…")
                    .disabled(busy)
                    .on_click(cx.listener(|this, _, _, cx| this.ask_to_uninstall_needle(cx))),
            );
        content.child(actions).into_any_element()
    }
}

fn status_line(title: &str, cx: &App) -> AnyElement {
    div()
        .id("needle-operation-state")
        .role(Role::Status)
        .test_support()
        .aria_label(title.to_owned())
        .flex()
        .items_center()
        .gap_2()
        .text_size(px(crate::ui::FONT_SIZE))
        .font_weight(FontWeight::SEMIBOLD)
        .child(gpui_kit::component::spinner::Spinner::new().small())
        .text_color(cx.theme().foreground)
        .child(title.to_owned())
        .into_any_element()
}
