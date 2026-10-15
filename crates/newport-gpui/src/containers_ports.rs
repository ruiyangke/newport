//! Compact port links; forwarding plans and operation ownership are shared.
use super::*;
use newport_client::{
    container_ports::{ForwardedPort, PortForwards},
    tunnel_operations::{Action as TunnelAction, Operations, Ticket},
};
use newport_domain::{containers::ContainerPort, model::Status};

#[derive(Clone, PartialEq)]
pub(super) struct PortRow {
    port: ContainerPort,
    pub(super) forward: Option<ForwardedPort>,
    busy: bool,
}
pub(super) struct Attempt {
    port: ContainerPort,
    ticket: Option<Ticket>,
    pub(super) result: Option<Result<(), String>>,
}
#[derive(Default)]
pub(super) struct PortsView {
    container: Option<String>,
    available: bool,
    forwards: PortForwards,
    pub(super) rows: Vec<PortRow>,
    pub(super) attempts: Vec<Attempt>,
}

impl ContainersPanel {
    pub(crate) fn sync_port_links(
        &mut self,
        server: &str,
        snapshot: Option<&Value>,
        operations: &Operations,
        cx: &mut Context<Self>,
    ) {
        self.ports.forwards.update(snapshot, server);
        let container = self.client.selected_container();
        let id = container.map(|c| c.id.clone());
        let rows = container
            .map(|c| {
                domain::published_ports(c)
                    .into_iter()
                    .map(|port| PortRow {
                        port: port.clone(),
                        forward: self.ports.forwards.get(port),
                        busy: server.parse().is_ok_and(|server| {
                            port.host_port.is_some_and(|number| {
                                operations.busy_destination(
                                    server,
                                    port.ip
                                        .as_deref()
                                        .filter(|ip| !ip.is_empty())
                                        .unwrap_or("0.0.0.0"),
                                    number,
                                )
                            })
                        }),
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let available = snapshot.is_some() && !self.retired;
        let mut changed = self.ports.container != id
            || self.ports.rows != rows
            || self.ports.available != available;
        if self.ports.container != id {
            // The session operation owner retains every submitted write. Page
            // feedback can disappear without cancelling or replaying it.
            self.ports.attempts.clear();
        }
        for attempt in &mut self.ports.attempts {
            if attempt.result.is_none()
                && let Some(result) = attempt.ticket.as_ref().and_then(Ticket::result)
            {
                attempt.result = Some(result);
                changed = true;
            }
        }
        self.ports.container = id;
        self.ports.rows = rows;
        self.ports.available = available;
        if changed {
            cx.notify();
        }
    }

    pub(crate) fn open_port_link(
        &mut self,
        server: &str,
        connection: Value,
        target: &PortTarget,
        operations: &mut Operations,
        cx: &mut Context<Self>,
    ) {
        let port = &target.port;
        if !self.selected_container_is(&target.container, &target.name) || !self.ports.available {
            return;
        }
        // A duplicate callback must retain the original pending receipt.
        if self.ports.attempts.iter().any(|attempt| {
            attempt.port == *port && attempt.ticket.is_some() && attempt.result.is_none()
        }) {
            return;
        }
        let result = self
            .client
            .port_destination(&target.container, &target.name, port)
            .map_err(str::to_owned)
            .and_then(|destination| {
                let server = server.parse().map_err(|_| "Choose a server".to_owned())?;
                operations
                    .start(TunnelAction::ForwardAndOpen {
                        server,
                        address: destination.address,
                        port: destination.port,
                        name: destination.name,
                        connection,
                    })
                    .map_err(|error| error.to_string())
            });
        self.ports.attempts.retain(|attempt| attempt.port != *port);
        self.ports.attempts.push(match result {
            Ok(ticket) => Attempt {
                port: port.clone(),
                ticket: Some(ticket),
                result: None,
            },
            Err(error) => Attempt {
                port: port.clone(),
                ticket: None,
                result: Some(Err(error)),
            },
        });
        cx.notify();
    }

    pub(super) fn port_links(&self, container: &Container, cx: &mut Context<Self>) -> AnyElement {
        let published = domain::published_ports(container);
        let internal = domain::exposed_ports(container);
        if published.is_empty() && internal.is_empty() {
            return div().into_any_element();
        }
        let current = self.ports.container.as_deref() == Some(container.id.as_str());
        div()
            .id("container-port-links")
            .debug_selector(|| "container-port-links".into())
            .flex()
            .flex_col()
            .gap(px(6.))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(6.))
                    .child(quiet("Ports", cx).text_size(px(11.)).mr(px(2.)))
                    .children(published.iter().enumerate().map(|(index, port)| {
                        let label = domain::port_label(port);
                        if !domain::forwardable(port) {
                            let reason = if port.protocol != domain::PortProtocol::Tcp {
                                "SSH forwards support TCP ports.".to_owned()
                            } else {
                                format!(
                                    "Bound to {}; this binding cannot be forwarded here.",
                                    port.ip.as_deref().unwrap_or("an unavailable address")
                                )
                            };
                            return div()
                                .id(("container-port-info", index))
                                .aria_label(format!("Port {label}. {reason}"))
                                .tooltip(move |window, cx| {
                                    gpui_kit::component::tooltip::Tooltip::new(reason.clone())
                                        .build(window, cx)
                                })
                                .h(px(24.))
                                .px(px(8.))
                                .flex()
                                .items_center()
                                .border_1()
                                .border_color(cx.theme().border)
                                .rounded(px(6.))
                                .font_family(cx.theme().mono_font_family.clone())
                                .text_size(px(11.5))
                                .text_color(cx.theme().muted_foreground)
                                .child(label)
                                .into_any_element();
                        }
                        let row = current
                            .then(|| self.ports.rows.iter().find(|r| r.port == **port))
                            .flatten();
                        let forward = row.and_then(|r| r.forward.as_ref());
                        let connected = forward.is_some_and(|f| f.status == Status::Connected);
                        let attempt = current
                            .then(|| self.ports.attempts.iter().find(|a| a.port == **port))
                            .flatten();
                        let pending = row.is_some_and(|r| r.busy)
                            || attempt.is_some_and(|a| a.ticket.is_some() && a.result.is_none());
                        let transitioning = forward.is_some_and(|f| {
                            matches!(f.status, Status::Connecting | Status::Reconnecting)
                        });
                        let busy = pending || transitioning;
                        let blocked = self
                            .client
                            .port_destination(&container.id, &container.name, port)
                            .err();
                        let unavailable = !current || !self.ports.available;
                        let action = if connected {
                            "Open"
                        } else if forward.is_some() {
                            "Connect and open"
                        } else {
                            "Forward and open"
                        };
                        let tooltip = if let Some(reason) = blocked {
                            reason.to_owned()
                        } else if unavailable {
                            "Waiting for server configuration.".into()
                        } else if let Some(forward) = forward.filter(|_| connected) {
                            format!("Open http://localhost:{}", forward.local_port)
                        } else if let Some(error) = forward.and_then(|f| f.error.as_ref()) {
                            error.clone()
                        } else if busy {
                            "Waiting for the forward to connect…".into()
                        } else if forward.is_some() {
                            "Connect the saved forward and open it".into()
                        } else {
                            "Forward to this computer and open it".into()
                        };
                        let id = container.id.clone();
                        let name = container.name.clone();
                        let port = (*port).clone();
                        crate::controls::compact_button(("container-port", index))
                            .debug_selector(move || format!("container-port-{index}"))
                            .rounded(px(6.))
                            .px(px(8.))
                            .when(connected, |button| {
                                button.ghost().bg(cx.theme().primary.opacity(0.08))
                            })
                            .when(busy, |button| button.icon(IconName::RefreshCw))
                            .loading(busy)
                            .disabled(busy || blocked.is_some() || unavailable)
                            .accessibility_label(format!(
                                "{action} port {label}{}",
                                if busy { ", connecting" } else { "" }
                            ))
                            .tooltip(tooltip)
                            .child(
                                div()
                                    .font_family(cx.theme().mono_font_family.clone())
                                    .text_size(px(11.5))
                                    .when(connected, |v| v.text_color(cx.theme().primary))
                                    .child(if let Some(forward) = forward.filter(|_| connected) {
                                        format!("localhost:{} → {label}", forward.local_port)
                                    } else {
                                        label
                                    }),
                            )
                            .when(!connected, |button| {
                                button.child(
                                    quiet(
                                        if busy {
                                            "Forwarding…"
                                        } else if forward.is_some() {
                                            "Connect & open"
                                        } else {
                                            "Forward & open"
                                        },
                                        cx,
                                    )
                                    .text_size(px(11.)),
                                )
                            })
                            .when(connected, |button| {
                                button.child(
                                    Icon::new(IconName::ExternalLink)
                                        .size(px(11.))
                                        .text_color(cx.theme().primary),
                                )
                            })
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if this.selected_container_is(&id, &name) {
                                    cx.emit(ContainersEvent::OpenPort(PortTarget {
                                        container: id.clone(),
                                        name: name.clone(),
                                        port: port.clone(),
                                    }));
                                }
                            }))
                            .into_any_element()
                    }))
                    .when(!internal.is_empty(), |v| {
                        v.child(
                            div()
                                .flex()
                                .gap(px(4.))
                                .text_size(px(11.))
                                .text_color(cx.theme().muted_foreground)
                                .child(if published.is_empty() {
                                    "internal"
                                } else {
                                    "· internal"
                                })
                                .child(
                                    div()
                                        .font_family(cx.theme().mono_font_family.clone())
                                        .child(
                                            internal
                                                .into_iter()
                                                .map(domain::port_label)
                                                .collect::<Vec<_>>()
                                                .join(", "),
                                        ),
                                ),
                        )
                    }),
            )
            .children(
                self.ports
                    .attempts
                    .iter()
                    .enumerate()
                    .filter(|_| current)
                    .filter_map(|(index, attempt)| {
                        attempt
                            .result
                            .as_ref()
                            .and_then(|r| r.as_ref().err())
                            .map(|error| {
                                alert(
                                    format!("container-port-error-{index}"),
                                    format!("Port {}: {error}", domain::port_label(&attempt.port)),
                                    cx,
                                )
                                .debug_selector(move || format!("container-port-error-{index}"))
                            })
                    }),
            )
            .into_any_element()
    }
}
