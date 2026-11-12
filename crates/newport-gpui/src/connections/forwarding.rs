use super::*;
use newport_domain::{
    port_discovery::{DestinationError, Endpoint},
    port_grouping::{Entry, ListenerSelection, Port},
};
use std::collections::BTreeSet;

fn endpoints(entry: &Entry) -> Vec<(Endpoint, Entry)> {
    let Some(row) = entry.port.row() else {
        return Vec::new();
    };
    row.members
        .iter()
        .map(|member| member.endpoint)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .map(|endpoint| {
            let mut selected = entry.clone();
            if let Port::Observed(row) = &mut selected.port {
                row.members.retain(|member| member.endpoint == endpoint);
            }
            selected.addresses = selected.port.addresses();
            let members = selected
                .port
                .row()
                .map(|row| row.members.clone())
                .unwrap_or_default();
            selected.forwards.retain(|forward| {
                members.iter().any(|member| {
                    newport_domain::forwarding::reaches_member(
                        forward,
                        connections_grouped::string(forward, "serverId"),
                        member,
                    )
                })
            });
            (endpoint, selected)
        })
        .collect()
}

impl Newport {
    pub(crate) fn forward_discovered(
        &mut self,
        entry: Entry,
        project: &str,
        options: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.page != Page::Ports {
            return;
        }
        let selection = ListenerSelection::capture(&[entry]);
        let mut latest = selection.resolve(
            self.connection_projects()
                .into_iter()
                .flat_map(|project| project.entries)
                .collect(),
        );
        if latest.len() != 1 {
            self.notification = Some(Notification::error(DestinationError::Changed.to_string()));
            cx.notify();
            return;
        }
        let entry = latest.remove(0);
        match entry.port.destination(None) {
            Err(DestinationError::ChooseMember) => {
                if window.has_active_dialog(cx) {
                    return;
                }
                let choices = endpoints(&entry);
                let owner = cx.weak_entity();
                let server = self.server.clone();
                let scope = self
                    .selected_server()
                    .map(|server| terminal_scope::connection(server, &self.snapshot));
                let project = project.to_owned();
                window.open_dialog(cx, move |dialog, _, _| {
                    let owner = owner.clone();
                    let server = server.clone();
                    let scope = scope.clone();
                    let project = project.clone();
                    dialog
                        .title(crate::controls::dialog_title("Choose server address"))
                        .w(px(480.))
                        .p(px(24.))
                        .rounded(px(crate::ui::RADIUS))
                        .on_ok(|_, _, _| false)
                        .child(
                            div().flex().flex_col().gap_2().children(
                                choices
                                    .iter()
                                    .enumerate()
                                    .map(|(index, (endpoint, entry))| {
                                        let destination = entry.port.destination(None);
                                        let address = std::net::SocketAddr::new(
                                            endpoint.address,
                                            endpoint.port,
                                        );
                                        let label = match &destination {
                                            Ok(destination) => {
                                                format!("{address} → {}", destination.host)
                                            }
                                            Err(error) => format!("{address} · {error}"),
                                        };
                                        let entry = entry.clone();
                                        let owner = owner.clone();
                                        let server = server.clone();
                                        let scope = scope.clone();
                                        let project = project.clone();
                                        crate::controls::button(("discovery-address", index))
                                            .label(label.clone())
                                            .accessibility_label(label)
                                            .disabled(destination.is_err())
                                            .on_click(move |_, window, cx| {
                                                window.close_dialog(cx);
                                                let _ = owner.update(cx, |this, cx| {
                                                    if this.page != Page::Ports
                                                        || this.server != server
                                                        || this.selected_server().map(|server| {
                                                            terminal_scope::connection(
                                                                server,
                                                                &this.snapshot,
                                                            )
                                                        }) != scope
                                                    {
                                                        return;
                                                    }
                                                    this.forward_discovered(
                                                        entry.clone(),
                                                        &project,
                                                        options,
                                                        window,
                                                        cx,
                                                    );
                                                });
                                            })
                                    }),
                            ),
                        )
                });
            }
            Err(error) => {
                self.notification = Some(Notification::error(error.to_string()));
                cx.notify();
            }
            Ok(destination) if options => {
                let draft = (|| -> Result<(Value, bool), String> {
                    let server = self.server.parse().map_err(|_| "Invalid server")?;
                    let tunnels = serde_json::from_value::<Vec<newport_domain::model::Tunnel>>(
                        self.snapshot["config"]["tunnels"].clone(),
                    )
                    .map_err(|error| error.to_string())?;
                    connections_rows::editor_draft(
                        &tunnels,
                        server,
                        &destination.host,
                        destination.port,
                        &entry.port.forward_name(),
                        project,
                    )
                })();
                match draft {
                    Ok((value, existing)) => {
                        self.open_tunnel_form(Some(value), existing, window, cx)
                    }
                    Err(error) => {
                        self.notification = Some(Notification::error(error));
                        cx.notify();
                    }
                }
            }
            Ok(_) => self.toggle_port(entry, project, true, cx),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{ListenerSelection, Port, endpoints};
    use crate::connections::fixtures::Listener;
    use serde_json::json;

    #[gpui_kit::test]
    fn dual_stack_forward_options_open_directly_with_an_editable_default(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        use crate::connections::fixtures as fixture;
        use crate::page_perf_tests::{connections_fixture, frame};
        let (view, visual) = connections_fixture(cx, false);
        let entry = view.update(visual, |view, _| {
            fixture::put(
                view,
                fixture::observation([
                    Listener::new(8000).pid(1).address("0.0.0.0"),
                    Listener::new(8000).pid(1).address("::"),
                ]),
            );
            view.connection_projects()
                .into_iter()
                .flat_map(|project| project.entries)
                .next()
                .unwrap()
        });
        view.update(visual, |view, _| {
            let plan = view
                .plan_port_batch("", std::slice::from_ref(&entry))
                .unwrap();
            assert!(plan.creation_error.is_none());
            assert_eq!(plan.create.len(), 1);
            assert_eq!(plan.create[0].remote_host, "localhost");
            assert_eq!(plan.create[0].remote_port, 8000);
        });
        visual.update(|window, cx| {
            view.update(cx, |view, cx| {
                view.forward_discovered(entry, "", true, window, cx)
            });
        });
        frame(visual);
        visual.update(|window, _| {
            let elements = gpui_kit::base::test_support::snapshots(window);
            let host = elements
                .iter()
                .find(|element| {
                    element.label() == Some("Remote host")
                        && element.role() == Some(gpui_kit::Role::TextInput)
                })
                .expect("dual-stack forwarding opens the editor without an address dialog");
            assert_eq!(host.value(), Some("localhost"));
            assert!(
                !elements
                    .iter()
                    .any(|element| element.label() == Some("[::]:8000 → ::1"))
            );
        });
    }

    #[test]
    fn address_choices_keep_family_and_only_the_selected_forward() {
        let mut entry = Listener::new(8000).address("0.0.0.0").entry();
        let Port::Observed(row) = &mut entry.port else {
            unreachable!()
        };
        // Beside an IPv4 listener on the same port, `::` is IPv6-only.
        let mut ipv6 = Listener::new(8000).address("::").member();
        if let newport_domain::port_discovery::Kind::Socket(socket) = &mut ipv6.kind {
            socket.ipv6_only = Some(true);
        }
        row.members.push(ipv6);
        entry.forwards = vec![
            json!({"id":"v4", "serverId":"s", "direction":"local", "remoteHost":"127.0.0.1", "remotePort":8000}),
            json!({"id":"v6", "serverId":"s", "direction":"local", "remoteHost":"::1", "remotePort":8000}),
        ];
        assert_eq!(entry.port.destination(None).unwrap().host, "localhost");
        let choices = endpoints(&entry);
        assert_eq!(choices.len(), 2);
        assert_eq!(
            choices[0].1.port.destination(None).unwrap().host,
            "127.0.0.1"
        );
        assert_eq!(choices[1].1.port.destination(None).unwrap().host, "::1");
        assert_eq!(choices[0].1.forwards, vec![entry.forwards[0].clone()]);
        assert_eq!(choices[1].1.forwards, vec![entry.forwards[1].clone()]);
        let selected = ListenerSelection::capture(&[choices[0].1.clone()]);
        assert_eq!(
            selected.resolve(vec![entry])[0]
                .port
                .destination(None)
                .unwrap()
                .host,
            "127.0.0.1"
        );
    }

    #[gpui_kit::test]
    fn address_dialog_preserves_the_choice_and_rejects_changed_evidence(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        use crate::connections::fixtures as fixture;
        use crate::page_perf_tests::{connections_fixture, frame};
        use gpui_kit::component::WindowExt;

        for (host, change) in [
            ("10.0.0.4", "fresh"),
            ("fd00::4", "fresh"),
            ("fd00::4", "stale"),
            ("fd00::4", "replaced"),
        ] {
            let (view, visual) = connections_fixture(cx, false);
            let observation = |pid| {
                fixture::observation([
                    Listener::new(8000).pid(pid).address("10.0.0.4"),
                    Listener::new(8000).pid(pid).address("fd00::4"),
                ])
            };
            let entry = view.update(visual, |view, _| {
                fixture::put(view, observation(1));
                let mut entries = view
                    .connection_projects()
                    .into_iter()
                    .flat_map(|project| project.entries);
                let entry = entries.next().unwrap();
                assert!(entries.next().is_none());
                assert_eq!(entry.port.row().unwrap().members.len(), 2);
                entry
            });
            visual.update(|window, cx| {
                view.update(cx, |view, cx| {
                    view.forward_discovered(entry, "", true, window, cx)
                });
            });
            frame(visual);
            let label = if host == "fd00::4" {
                "[fd00::4]:8000 → fd00::4"
            } else {
                "10.0.0.4:8000 → 10.0.0.4"
            };
            let choice = visual.update(|window, _| {
                gpui_kit::base::test_support::snapshots(window)
                    .into_iter()
                    .find(|element| element.label() == Some(label))
                    .expect("explicit address choice")
                    .bounds()
                    .center()
            });
            if change != "fresh" {
                view.update(visual, |view, _| {
                    let mut next = observation(if change == "replaced" { 2 } else { 1 });
                    next.data.as_mut().unwrap().current.generation = 2;
                    next.stale = change == "stale";
                    fixture::put(view, next);
                });
            }
            visual.simulate_click(choice, Default::default());
            frame(visual);
            visual.update(|window, cx| {
                if change == "fresh" {
                    let elements = gpui_kit::base::test_support::snapshots(window);
                    let destination = elements
                        .iter()
                        .find(|element| {
                            element.label() == Some("Remote host")
                                && element.role() == Some(gpui_kit::Role::TextInput)
                        })
                        .expect("forward options with selected address");
                    assert_eq!(destination.value(), Some(host));
                } else {
                    assert!(!window.has_active_dialog(cx), "{change}");
                    assert!(view.read(cx).notification.is_some(), "{change}");
                }
                assert!(
                    view.read(cx).snapshot["config"]["tunnels"]
                        .as_array()
                        .unwrap()
                        .is_empty()
                );
            });
        }
    }
}
