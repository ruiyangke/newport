//! UI adapters for the session-owned forwarding client.
use super::*;
use newport_client::tunnel_operations::{Action, Kind};
use newport_domain::model::Tunnel;

impl Newport {
    pub(super) fn open_forward(
        &mut self,
        value: Value,
        remote_port: Option<u16>,
        cx: &mut Context<Self>,
    ) {
        let result = serde_json::from_value::<Tunnel>(value)
            .map_err(|e| e.to_string())
            .and_then(|tunnel| {
                let action = match remote_port {
                    Some(remote_port) => Action::OpenPort {
                        tunnel,
                        remote_port,
                    },
                    None => Action::Open(tunnel),
                };
                self.tunnel_operations
                    .start(action)
                    .map_err(|e| e.to_string())
            });
        if let Err(error) = result {
            self.notification = Some(Notification::error(error));
        }
        cx.notify();
    }
    pub(super) fn tunnel_busy(&self, value: &Value) -> bool {
        serde_json::from_value::<Tunnel>(value.clone())
            .is_ok_and(|t| self.tunnel_operations.busy_tunnel(&t))
    }
    pub(super) fn port_busy(&self, entry: &connections_grouped::Entry) -> bool {
        entry.forwards.iter().any(|t| self.tunnel_busy(t))
            || (!entry.saved
                && self.server.parse().is_ok_and(|server| {
                    entry.port["port"]
                        .as_u64()
                        .and_then(|p| u16::try_from(p).ok())
                        .is_some_and(|port| {
                            self.tunnel_operations.busy_destination(
                                server,
                                connections_grouped::string(&entry.port, "address"),
                                port,
                            )
                        })
                }))
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
                let tunnels = entry
                    .forwards
                    .into_iter()
                    .map(serde_json::from_value)
                    .collect::<Result<Vec<Tunnel>, _>>()
                    .map_err(|e| e.to_string())?;
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
        forwards: Vec<Value>,
        enabled: bool,
        cx: &mut Context<Self>,
    ) {
        let mut tickets = Vec::new();
        for value in forwards {
            if value["autoConnect"] == enabled && value["autoReconnect"] == enabled {
                continue;
            }
            let started = (|| -> Result<_, String> {
                let expected: Tunnel =
                    serde_json::from_value(value.clone()).map_err(|e| e.to_string())?;
                let mut next = value;
                next["autoConnect"] = json!(enabled);
                next["autoReconnect"] = json!(enabled);
                let tunnel: Tunnel = serde_json::from_value(next).map_err(|e| e.to_string())?;
                self.tunnel_operations
                    .start(Action::Update { tunnel, expected })
                    .map_err(|e| e.to_string())
            })();
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
            self.request("snapshot", "snapshot", json!({}));
        }
        if refresh_ports && self.page == Page::Ports {
            self.refresh();
        }
        true
    }
}
