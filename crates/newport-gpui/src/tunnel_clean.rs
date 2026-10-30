//! Confirmation for deleting saved forwards that nothing listens behind.
use super::*;
use newport_client::tunnel_operations::Action;
use newport_domain::model::Tunnel;

struct Confirmation {
    owner: WeakEntity<Newport>,
    server: uuid::Uuid,
    tunnels: Vec<Tunnel>,
    pending: bool,
    error: Option<String>,
}

impl Newport {
    /// The dead forwards Clean up offers: none unless the server's listeners
    /// are known, and none that another operation is changing.
    pub(super) fn cleanable_forwards(
        &self,
        groups: &[connections_grouped::Project],
        cx: &App,
    ) -> Vec<Tunnel> {
        if !self.listeners_known(cx) {
            return Vec::new();
        }
        connections_grouped::dead_forwards(groups, &self.snapshot["runtime"])
            .into_iter()
            .filter(|value| !self.tunnel_busy(value))
            .filter_map(|value| serde_json::from_value(value).ok())
            .collect()
    }

    pub(super) fn confirm_clean_up_forwards(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if window.has_active_dialog(cx) {
            return;
        }
        let Ok(server) = self.server.parse::<uuid::Uuid>() else {
            return;
        };
        let tunnels = self.cleanable_forwards(&self.connection_projects(), cx);
        if tunnels.is_empty() {
            return;
        }
        let owner = cx.weak_entity();
        let form = cx.new(|_| Confirmation {
            owner,
            server,
            tunnels,
            pending: false,
            error: None,
        });
        window.open_dialog(cx, move |dialog, _, cx| {
            let pending = form.read(cx).pending;
            let cancel_form = form.clone();
            dialog
                .title(crate::controls::dialog_title(title(
                    form.read(cx).tunnels.len(),
                )))
                .w(px(480.))
                .p(px(24.))
                .rounded(px(crate::ui::RADIUS))
                .border_0()
                .centered(true)
                .overlay_closable(false)
                .keyboard(!pending)
                .close_button(!pending)
                .on_cancel(move |_, _, cx| !cancel_form.read(cx).pending)
                // Bare Enter must neither dismiss nor implicitly approve a
                // destructive operation; the explicit button owns submission.
                .on_ok(|_, _, _| false)
                .child(form.clone())
        });
    }
}

fn title(count: usize) -> String {
    if count == 1 {
        "Delete 1 forward?".into()
    } else {
        format!("Delete {count} forwards?")
    }
}

impl Confirmation {
    fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.pending {
            return;
        }
        let action = Action::DeleteMany {
            server: self.server,
            tunnels: self.tunnels.clone(),
        };
        let result = self
            .owner
            .update(cx, |owner, cx| {
                let ticket = owner
                    .tunnel_operations
                    .start(action)
                    .map_err(|e| e.to_string())?;
                cx.notify();
                Ok::<_, String>(ticket)
            })
            .map_err(|_| "The main window is no longer available".to_owned())
            .and_then(|r| r);
        let ticket = match result {
            Ok(ticket) => ticket,
            Err(error) => {
                self.error = Some(error);
                cx.notify();
                return;
            }
        };
        self.pending = true;
        self.error = None;
        cx.notify();
        let count = self.tunnels.len();
        let executor = cx.background_executor().clone();
        cx.spawn_in(window, async move |form, cx| {
            let result = loop {
                if let Some(result) = ticket.result() {
                    break result;
                }
                executor.timer(Duration::from_millis(50)).await;
            };
            let _ = form.update_in(cx, |form, window, cx| {
                form.pending = false;
                match result {
                    Ok(()) => {
                        let _ = form.owner.update(cx, |owner, cx| {
                            owner.notification = Some(Notification::success(format!(
                                "Deleted {count} {}.",
                                if count == 1 { "forward" } else { "forwards" }
                            )));
                            cx.notify();
                        });
                        window.close_dialog(cx);
                    }
                    Err(error) => form.error = Some(error),
                }
                cx.notify();
            });
        })
        .detach();
    }
}

impl Render for Confirmation {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let description = "Nothing is listening on the server for these saved forwards, and they’re switched off. Their configuration will be removed.";
        div()
            .id("clean-up-forwards-confirmation")
            .role(accesskit::Role::AlertDialog)
            .aria_label(title(self.tunnels.len()))
            .aria_description(description)
            .flex()
            .flex_col()
            .pt(px(12.))
            .gap(px(20.))
            .child(
                div()
                    .text_size(px(crate::ui::FONT_SIZE))
                    .line_height(relative(1.65))
                    .child(description),
            )
            .child(
                div()
                    .id("clean-up-forwards-list")
                    .max_h(px(240.))
                    .overflow_y_scroll()
                    .border_1()
                    .border_color(cx.theme().border)
                    .children(self.tunnels.iter().enumerate().map(|(index, t)| {
                        let name = if t.name.is_empty() {
                            format!("Port {}", t.local_port)
                        } else {
                            t.name.clone()
                        };
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap(px(12.))
                            .min_h(px(crate::ui::ROW_HEIGHT + 4.))
                            .px(px(10.))
                            .when(index > 0, |row| {
                                row.border_t_1().border_color(cx.theme().border)
                            })
                            .text_size(px(crate::ui::FONT_SIZE))
                            .child(div().min_w_0().truncate().child(name))
                            .child(
                                div()
                                    .flex_shrink_0()
                                    .font_family(cx.theme().mono_font_family.clone())
                                    .text_color(cx.theme().muted_foreground)
                                    .child(match t.remote_port_end {
                                        Some(end) => {
                                            format!("{}:{}–{end}", t.remote_host, t.remote_port)
                                        }
                                        None => format!("{}:{}", t.remote_host, t.remote_port),
                                    }),
                            )
                    })),
            )
            .when_some(self.error.clone(), |view, error| {
                view.child(crate::controls::alert("clean-up-forwards-error", error, cx))
            })
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap_2()
                    .pt(px(12.))
                    .child(
                        crate::controls::form_button("cancel-clean-up-forwards")
                            .label("Cancel")
                            .min_w(px(76.))
                            .rounded(px(crate::ui::RADIUS))
                            .disabled(self.pending)
                            .on_click(|_, window, cx| window.close_dialog(cx)),
                    )
                    .child(
                        crate::controls::form_button("confirm-clean-up-forwards")
                            .danger()
                            .min_w(px(76.))
                            .rounded(px(crate::ui::RADIUS))
                            .label(if self.pending {
                                "Deleting…"
                            } else {
                                "Delete forwards"
                            })
                            .disabled(self.pending)
                            .on_click(cx.listener(|this, _, window, cx| this.submit(window, cx))),
                    ),
            )
    }
}

#[cfg(test)]
mod tests {
    use crate::page_perf_tests::connections_fixture;
    use gpui::{TestAppContext, VisualTestContext};
    use gpui_kit as gpui;
    use serde_json::json;

    #[gpui::test]
    fn clean_up_offers_only_dead_forwards_to_the_server_itself(cx: &mut TestAppContext) {
        let (view, cx) = connections_fixture(cx, false);
        let server = uuid::Uuid::new_v4();
        let forward = |name: &str, host: &str, port: u16| {
            let mut tunnel =
                newport_domain::forwarding::discovered_tunnel(&[], server, host, port).unwrap();
            tunnel.name = name.into();
            tunnel.local_port = port + 1000;
            serde_json::to_value(tunnel).unwrap()
        };
        let offered = |cx: &mut VisualTestContext| -> Vec<String> {
            view.update(cx, |view, cx| {
                view.cleanable_forwards(&view.connection_projects(), cx)
                    .into_iter()
                    .map(|t| t.name)
                    .collect()
            })
        };
        view.update(cx, |view, _| {
            let state = view.connections_state.remove("fixture").unwrap();
            view.server = server.to_string();
            view.connections_state.insert(view.server.clone(), state);
            view.snapshot["config"]["tunnels"] = json!([
                forward("listening", "127.0.0.1", 8000),
                forward("dead", "127.0.0.1", 4000),
                forward("dead by name", "localhost", 4001),
                forward("database", "db.internal", 5432),
                forward("same server by address", "10.0.0.5", 4002),
            ]);
        });
        assert_eq!(offered(cx), ["dead", "dead by name"]);

        view.update(cx, |view, _| {
            let generation = view.resources.begin_refresh("page", false).unwrap();
            view.resources
                .complete("page", generation, Err("The reading failed".into()));
        });
        assert!(offered(cx).is_empty());
    }
}
