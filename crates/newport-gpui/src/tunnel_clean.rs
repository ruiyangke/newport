//! Confirmation for deleting saved forwards that nothing listens behind.
use super::*;
use gpui_kit::TestSupportExt as _;
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
    /// The forwards Clean up offers: every saved forward whose destination
    /// the scan didn't observe, on or off. None until the server's listeners
    /// are known, and none that another operation is changing.
    pub(super) fn cleanable_forwards(
        &self,
        groups: &[connections_grouped::Project],
        cx: &App,
    ) -> Vec<Tunnel> {
        if !self.listeners_known(cx) {
            return Vec::new();
        }
        connections_grouped::unobserved_forwards(groups)
            .into_iter()
            .filter(|value| !self.tunnel_busy(value))
            .map(|value| Tunnel::from(&value))
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
                .centered()
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
                if owner.server != self.server.to_string() || owner.page != Page::Ports {
                    return Err("The selected server changed. Review cleanup again.".into());
                }
                let eligible = owner.cleanable_forwards(&owner.connection_projects(), cx);
                if self.tunnels.iter().any(|tunnel| !eligible.contains(tunnel)) {
                    return Err(
                        "Discovery or a saved forward changed. Review cleanup again.".into(),
                    );
                }
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
        let description = "Newport didn’t observe a destination for these saved forwards. Deleting them removes them and stops any that are on.";
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
                        // Named as the row names it, at the address it leads to.
                        let ports = match t.remote_port_end.filter(|end| *end != t.remote_port) {
                            Some(end) => format!("{}–{end}", t.remote_port),
                            None => t.remote_port.to_string(),
                        };
                        let name = if t.name.is_empty() {
                            format!("Port {ports}")
                        } else {
                            t.name.clone()
                        };
                        let host = if t.remote_host.contains(':') {
                            format!("[{}]", t.remote_host)
                        } else {
                            t.remote_host.clone()
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
                                    .child(format!("{host}:{ports}")),
                            )
                    })),
            )
            .when_some(self.error.clone(), |view, error| {
                view.child(
                    crate::controls::alert("clean-up-forwards-error", error, cx).test_support(),
                )
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
    use crate::connections::fixtures::{self as discovery_fixture, Listener};
    use crate::page_perf_tests::{connections_fixture, frame};
    use gpui::{TestAppContext, VisualTestContext};
    use gpui_kit as gpui;

    #[gpui::test]
    fn clean_up_offers_every_saved_forward_without_an_observed_destination(
        cx: &mut TestAppContext,
    ) {
        let (view, cx) = connections_fixture(cx, false);
        let server = view.read_with(cx, |view, _| view.server.parse::<uuid::Uuid>().unwrap());
        let forward = |name: &str, host: &str, port: u16| {
            let mut tunnel =
                newport_domain::forwarding::discovered_tunnel(&[], server, host, port).unwrap();
            tunnel.name = name.into();
            tunnel.remote_host = host.into();
            tunnel.local_port = port + 1000;
            tunnel
        };
        let offered = |cx: &mut VisualTestContext| -> Vec<String> {
            let mut names: Vec<String> = view.update(cx, |view, cx| {
                view.cleanable_forwards(&view.connection_projects(), cx)
                    .into_iter()
                    .map(|t| t.name)
                    .collect()
            });
            names.sort();
            names
        };
        let mut reverse = forward("reverse", "127.0.0.1", 4004);
        reverse.direction = newport_domain::model::Direction::Remote;
        view.update(cx, |view, _| {
            view.snapshot_mut().config.tunnels = [
                forward("listening", "127.0.0.1", 8000),
                forward("dead", "127.0.0.1", 4000),
                forward("dead by name", "localhost", 4001),
                forward("database", "db.internal", 5432),
                forward("same server by address", "10.0.0.5", 4002),
                forward("never listened", "127.0.0.1", 4003),
                reverse,
            ]
            .iter()
            .map(newport_protocol::desktop::configuration::Tunnel::from)
            .collect();
            discovery_fixture::put(
                view,
                discovery_fixture::observation([
                    Listener::new(8000),
                    Listener::new(4000),
                    Listener::new(4001),
                ]),
            );
        });
        // On or off, to the server or through it: every row that says
        // "Destination not observed". Reverse forwards lead to this computer.
        assert_eq!(
            offered(cx),
            ["database", "never listened", "same server by address"]
        );

        view.update(cx, |view, _| {
            let mut reading = discovery_fixture::observation([Listener::new(8000)]);
            reading.data.as_mut().unwrap().current.generation = 2;
            discovery_fixture::put(view, reading);
        });
        assert_eq!(
            offered(cx),
            [
                "database",
                "dead",
                "dead by name",
                "never listened",
                "same server by address"
            ]
        );

        view.update(cx, |view, _| {
            let generation = view.resources.begin_refresh("ports", false).unwrap();
            view.resources
                .complete("ports", generation, Err("The reading failed".into()));
        });
        assert!(offered(cx).is_empty());
    }

    #[gpui::test]
    fn a_scan_restored_on_return_offers_nothing_until_a_new_scan(cx: &mut TestAppContext) {
        let (view, cx) = connections_fixture(cx, false);
        let offered = |cx: &mut VisualTestContext| {
            view.update(cx, |view, cx| {
                view.cleanable_forwards(&view.connection_projects(), cx)
                    .len()
            })
        };
        view.update(cx, |view, _| {
            let tunnel = newport_domain::forwarding::discovered_tunnel(
                &[],
                view.server.parse().unwrap(),
                "127.0.0.1",
                4000,
            )
            .unwrap();
            view.snapshot_mut().config.tunnels =
                vec![newport_protocol::desktop::configuration::Tunnel::from(
                    &tunnel,
                )];
            discovery_fixture::put(view, discovery_fixture::observation([]));
        });
        assert_eq!(offered(cx), 1);

        view.update(cx, |view, _| {
            view.suspend_connections();
            view.restore_connections();
        });
        assert_eq!(offered(cx), 0, "the restored scan may be out of date");

        view.update(cx, |view, _| {
            let mut next = discovery_fixture::observation([]);
            next.data.as_mut().unwrap().current.generation = 2;
            discovery_fixture::put(view, next);
        });
        assert_eq!(offered(cx), 1);
    }

    #[gpui::test]
    fn cleanup_confirmation_rechecks_evidence_and_saved_destination(cx: &mut TestAppContext) {
        for change in ["listener returned", "stale reading", "edited destination"] {
            let (view, visual) = connections_fixture(cx, false);
            let tunnel = view.update(visual, |view, _| {
                let tunnel = newport_domain::forwarding::discovered_tunnel(
                    &[],
                    view.server.parse().unwrap(),
                    "127.0.0.1",
                    8000,
                )
                .unwrap();
                view.snapshot_mut().config.tunnels =
                    vec![newport_protocol::desktop::configuration::Tunnel::from(
                        &tunnel,
                    )];
                discovery_fixture::put(view, discovery_fixture::observation([Listener::new(8000)]));
                let mut absent = discovery_fixture::observation([]);
                absent.data.as_mut().unwrap().current.generation = 2;
                discovery_fixture::put(view, absent);
                tunnel
            });
            visual.update(|window, cx| {
                view.update(cx, |view, cx| view.confirm_clean_up_forwards(window, cx));
            });
            frame(visual);
            let confirm = visual.update(|window, _| {
                gpui_kit::base::test_support::snapshots(window)
                    .into_iter()
                    .find(|element| element.label() == Some("Delete forwards"))
                    .expect("rendered cleanup confirmation")
                    .bounds()
                    .center()
            });
            view.update(visual, |view, _| match change {
                "edited destination" => {
                    let mut next = view.snapshot.clone().unwrap();
                    next.config.tunnels[0].remote_port = 8001;
                    view.sync_connections_snapshot(&next);
                    view.snapshot = Some(next);
                }
                _ => {
                    let mut next = discovery_fixture::observation(
                        (change == "listener returned").then(|| Listener::new(8000)),
                    );
                    next.data.as_mut().unwrap().current.generation = 3;
                    next.stale = change == "stale reading";
                    discovery_fixture::put(view, next);
                }
            });
            visual.simulate_click(confirm, Default::default());
            frame(visual);
            visual.update(|window, cx| {
                use gpui_kit::component::WindowExt;
                assert!(window.has_active_dialog(cx), "{change}");
                assert!(
                    gpui_kit::base::test_support::snapshots(window)
                        .iter()
                        .any(|element| element.label().is_some_and(|label| {
                            label.contains("Discovery or a saved forward changed")
                        })),
                    "{change} must require a new review"
                );
            });
            view.read_with(visual, |view, _| {
                assert!(!view.tunnel_operations.busy_tunnel(&tunnel), "{change}");
                assert_eq!(view.tunnels().len(), 1);
            });
        }
    }
}
