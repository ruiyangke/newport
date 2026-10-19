//! Confirmation presentation; the session-owned client performs deletion.
use super::*;
use newport_client::tunnel_operations::Action;
use newport_domain::model::Tunnel;

struct Confirmation {
    owner: WeakEntity<Newport>,
    tunnel: Tunnel,
    pending: bool,
    error: Option<String>,
}

pub(super) fn name(value: &Value) -> String {
    value["name"]
        .as_str()
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| format!("Port {}", value["localPort"]))
}

impl Newport {
    pub(super) fn confirm_delete_tunnel(
        &mut self,
        value: Value,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if window.has_active_dialog(cx) || self.tunnel_busy(&value) {
            return;
        }
        let tunnel = match serde_json::from_value::<Tunnel>(value) {
            Ok(t) if t.server_id.to_string() == self.server => t,
            _ => return,
        };
        let owner = cx.weak_entity();
        let form = cx.new(|_| Confirmation {
            owner,
            tunnel,
            pending: false,
            error: None,
        });
        window.open_dialog(cx, move |dialog, _, cx| {
            let pending = form.read(cx).pending;
            let cancel_form = form.clone();
            dialog
                .aria_label("Delete tunnel?")
                .title(
                    div()
                        .text_size(px(17.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .child("Delete tunnel?"),
                )
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

impl Confirmation {
    fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.pending {
            return;
        }
        let result = self
            .owner
            .update(cx, |owner, cx| {
                let ticket = owner
                    .tunnel_operations
                    .start(Action::Delete(self.tunnel.clone()))
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
                            owner.notification = Some(Notification::success("Forward deleted."));
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
        let title = if self.tunnel.name.is_empty() {
            format!("Port {}", self.tunnel.local_port)
        } else {
            self.tunnel.name.clone()
        };
        let description = format!(
            "Delete {title}? The tunnel will disconnect and its configuration will be removed."
        );
        div()
            .id("delete-forward-confirmation")
            .role(accesskit::Role::AlertDialog)
            .aria_label("Delete tunnel?")
            .aria_description(description.clone())
            .flex()
            .flex_col()
            .pt(px(12.))
            .gap(px(32.))
            .child(
                div()
                    .text_size(px(crate::ui::FONT_SIZE))
                    .line_height(relative(1.65))
                    .child(StyledText::new(description).with_highlights([(
                        7..7 + title.len(),
                        HighlightStyle {
                            font_weight: Some(FontWeight::SEMIBOLD),
                            ..Default::default()
                        },
                    )])),
            )
            .when_some(self.error.clone(), |view, error| {
                view.child(
                    div()
                        .id("delete-forward-error")
                        .role(accesskit::Role::Alert)
                        .text_color(cx.theme().danger)
                        .child(error),
                )
            })
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap_2()
                    .child(
                        crate::controls::form_button("cancel-delete-forward")
                            .label("Cancel")
                            .min_w(px(76.))
                            .rounded(px(crate::ui::RADIUS))
                            .disabled(self.pending)
                            .on_click(|_, window, cx| window.close_dialog(cx)),
                    )
                    .child(
                        crate::controls::form_button("confirm-delete-forward")
                            .danger()
                            .min_w(px(76.))
                            .rounded(px(crate::ui::RADIUS))
                            .label(if self.pending {
                                "Deleting…"
                            } else {
                                "Delete tunnel"
                            })
                            .disabled(self.pending)
                            .on_click(cx.listener(|this, _, window, cx| this.submit(window, cx))),
                    ),
            )
    }
}
