//! UI adapters for the session-owned forwarding client.
use super::*;
use newport_client::tunnel_operations::{Action, Kind};
use newport_domain::model::Tunnel;

impl Newport {
    pub(super) fn open_forward(
        &mut self,
        value: configuration::Tunnel,
        remote_port: Option<u16>,
        cx: &mut Context<Self>,
    ) {
        let tunnel = Tunnel::from(&value);
        let action = match remote_port {
            Some(remote_port) => Action::OpenPort {
                tunnel,
                remote_port,
            },
            None => Action::Open(tunnel),
        };
        let result = self
            .tunnel_operations
            .start(action)
            .map_err(|e| e.to_string());
        if let Err(error) = result {
            self.notification = Some(Notification::error(error));
        }
        cx.notify();
    }
    pub(super) fn tunnel_busy(&self, value: &configuration::Tunnel) -> bool {
        self.tunnel_operations.busy_tunnel(&Tunnel::from(value))
    }
    pub(super) fn port_busy(&self, entry: &connections_grouped::Entry) -> bool {
        entry.forwards.iter().any(|t| self.tunnel_busy(t))
            || self.server.parse().is_ok_and(|server| {
                entry.port.row().is_some_and(|row| {
                    row.members.iter().any(|member| {
                        member.endpoint.destination().is_ok_and(|destination| {
                            self.tunnel_operations.busy_destination(
                                server,
                                &destination.host,
                                destination.port,
                            )
                        })
                    })
                })
            })
    }
    pub(super) fn toggle_port(
        &mut self,
        entry: connections_grouped::Entry,
        project: &str,
        connected: bool,
        cx: &mut Context<Self>,
    ) {
        let action = (|| -> Result<Action, String> {
            let server = self.server.parse().map_err(|_| "Choose a server")?;
            if entry.forwards.is_empty() {
                let plan = self.plan_port_batch(project, &[entry])?;
                if let Some(error) = plan.creation_error {
                    return Err(error);
                }
                Ok(Action::ForwardBatch {
                    server,
                    create: plan.create,
                    connect: plan.connect,
                })
            } else {
                let tunnels = entry.forwards.iter().map(Tunnel::from).collect();
                Ok(Action::SetConnected {
                    server,
                    tunnels,
                    connected,
                })
            }
        })();
        let result = action.and_then(|action| {
            self.tunnel_operations
                .start(action)
                .map_err(|e| e.to_string())
        });
        if let Err(error) = result {
            self.notification = Some(Notification::error(error));
        }
        cx.notify();
    }
    /// Always on is one intent stored as both startup and reconnect policy,
    /// matching the forward editor. Each forward is saved against the value
    /// the menu was opened with, so a concurrent edit fails instead of being
    /// overwritten.
    pub(super) fn set_always_on(
        &mut self,
        forwards: Vec<configuration::Tunnel>,
        enabled: bool,
        cx: &mut Context<Self>,
    ) {
        let mut tickets = Vec::new();
        for value in forwards {
            if value.auto_connect == enabled && value.auto_reconnect == enabled {
                continue;
            }
            let expected = Tunnel::from(&value);
            let tunnel = Tunnel {
                auto_connect: enabled,
                auto_reconnect: enabled,
                ..expected.clone()
            };
            let started = self
                .tunnel_operations
                .start(Action::Update { tunnel, expected })
                .map_err(|e| e.to_string());
            match started {
                Ok(ticket) => tickets.push(ticket),
                Err(error) => self.notification = Some(Notification::error(error)),
            }
        }
        cx.notify();
        if tickets.is_empty() {
            return;
        }
        // Saves report as editor-owned outcomes, which polling leaves silent.
        let executor = cx.background_executor().clone();
        cx.spawn(async move |this, cx| {
            for ticket in tickets {
                let result = loop {
                    if let Some(result) = ticket.result() {
                        break result;
                    }
                    executor.timer(std::time::Duration::from_millis(50)).await;
                };
                if let Err(error) = result {
                    let _ = this.update(cx, |this, cx| {
                        this.notification = Some(Notification::error(error));
                        cx.notify();
                    });
                }
            }
        })
        .detach();
    }
    pub(super) fn poll_tunnel_actions(&mut self) -> bool {
        let outcomes = self.tunnel_operations.poll();
        if outcomes.is_empty() {
            return false;
        }
        let refresh_ports = outcomes
            .iter()
            .any(|o| o.server.to_string() == self.server && o.kind != Kind::Open);
        let refresh_snapshot = outcomes.iter().any(|o| o.kind != Kind::Open);
        for outcome in outcomes {
            // Editors own inline feedback; other actions can finish
            // after navigating away and still surface their result.
            if !matches!(outcome.kind, Kind::Save | Kind::Delete)
                && let Err(error) = outcome.result
            {
                self.notification = Some(Notification::error(error));
            }
        }
        // Both success and failure can leave changed persisted/runtime state.
        // An older in-flight read must not suppress the post-write refresh.
        if refresh_snapshot {
            self.resources.fence("snapshot");
            self.request_snapshot();
        }
        if refresh_ports && self.page == Page::Ports {
            self.refresh();
        }
        true
    }
}
