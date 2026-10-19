//! Retain each Services screen across navigation, scoped to its connection.
use super::*;

pub(super) struct HostedServices {
    connection: Value,
    panel: Entity<services::ServicesPanel>,
    _subscription: Subscription,
}

fn retire_obsolete_services(
    hosted: &mut std::collections::HashMap<String, HostedServices>,
    retired: &mut Vec<Entity<services::ServicesPanel>>,
    preferences: &mut std::collections::HashMap<String, services::Preferences>,
    snapshot: &Value,
    available: bool,
    cx: &mut App,
) {
    if !available {
        return;
    }
    let servers = snapshot["config"]["servers"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default();
    preferences.retain(|id, _| servers.iter().any(|server| server["id"] == id.as_str()));
    let obsolete: Vec<_> = hosted
        .iter()
        .filter_map(|(id, panel)| {
            let current = servers.iter().find(|server| server["id"] == id.as_str());
            (current
                .map(|server| terminal_scope::connection(server, snapshot))
                .as_ref()
                != Some(&panel.connection))
            .then(|| id.clone())
        })
        .collect();
    for id in obsolete {
        let panel = hosted.remove(&id).expect("collected entry");
        if servers.iter().any(|server| server["id"] == id.as_str()) {
            preferences.insert(id, panel.panel.read(cx).preferences(cx));
        }
        panel.panel.update(cx, |panel, cx| panel.retire(cx));
        retired.push(panel.panel);
    }
}

fn poll_services(
    hosted: &mut std::collections::HashMap<String, HostedServices>,
    retired: &mut Vec<Entity<services::ServicesPanel>>,
    preferences: &mut std::collections::HashMap<String, services::Preferences>,
    configuration: Option<&Value>,
    visible_server: Option<&str>,
    cx: &mut App,
) {
    // Rendering can stop while hidden. Retire obsolete connections before
    // their completion can schedule a read against a reused server ID.
    if let Some(snapshot) = configuration {
        retire_obsolete_services(hosted, retired, preferences, snapshot, true, cx);
    }
    // Keep bounded completion receipts queued during a configuration outage.
    // `poll(false)` only suspends periodic refreshes: accepting a completed
    // action or selection can still dispatch follow-up reads. Revalidate the
    // connection above before allowing the controller to consume those replies.
    if configuration.is_some() {
        for (id, hosted) in hosted {
            let visible = visible_server == Some(id.as_str());
            hosted.panel.update(cx, |panel, cx| panel.poll(visible, cx));
        }
    }
    for panel in retired.iter() {
        panel.update(cx, |panel, cx| panel.poll(false, cx));
    }
    retired.retain(|panel| panel.read(cx).has_pending());
}

fn open_service_command(
    terminals: &mut std::collections::HashMap<String, Entity<terminal_workspace::Workspace>>,
    server: &str,
    connection: Value,
    endpoint: String,
    command: String,
    cx: &mut App,
) -> Result<(), String> {
    // Validate before creating any presentation state, including the first tab.
    let draft = newport_client::terminal_command::CommandDraft::new(command)?;
    match terminals.entry(server.to_owned()) {
        std::collections::hash_map::Entry::Occupied(entry) => entry
            .get()
            .update(cx, |workspace, cx| workspace.open_command(draft, cx)),
        std::collections::hash_map::Entry::Vacant(entry) => {
            entry.insert(cx.new(|cx| {
                terminal_workspace::Workspace::with_command(
                    server.to_owned(),
                    connection,
                    endpoint,
                    draft,
                    cx,
                )
            }));
            Ok(())
        }
    }
}

impl Newport {
    fn retire_obsolete_services(&mut self, cx: &mut Context<Self>) {
        let available = self.configuration_available();
        retire_obsolete_services(
            &mut self.services_panels,
            &mut self.retired_services,
            &mut self.services_preferences,
            &self.snapshot,
            available,
            cx,
        );
    }

    pub(super) fn sync_services(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.retire_obsolete_services(cx);
        if !self.configuration_available() {
            return;
        }
        if self.page != Page::Services || self.selected_server().is_none() {
            return;
        }
        if !self.services_panels.contains_key(&self.server) {
            let server = self.server.clone();
            let connection =
                terminal_scope::connection(self.selected_server().unwrap(), &self.snapshot);
            let preferences = self.services_preferences.remove(&server);
            let panel = cx.new(|cx| {
                let mut panel =
                    services::ServicesPanel::new(server.clone(), connection.clone(), window, cx);
                if let Some(preferences) = preferences {
                    panel.restore_preferences(preferences, window, cx);
                }
                panel
            });
            let event_connection = connection.clone();
            let subscription = cx.subscribe_in(
                &panel,
                window,
                move |this, _, event: &services::ServicesEvent, _, cx| {
                    let current = rows(&this.snapshot["config"]["servers"])
                        .into_iter()
                        .find(|row| row["id"] == server);
                    if !this.configuration_available()
                        || current
                            .as_ref()
                            .map(|row| terminal_scope::connection(row, &this.snapshot))
                            .as_ref()
                            != Some(&event_connection)
                    {
                        return;
                    }
                    match event {
                        services::ServicesEvent::OpenTerminal { command } => {
                            this.open_service_terminal(&server, command.clone(), cx);
                        }
                        services::ServicesEvent::Notification { message } => {
                            let message = if this.server == server {
                                message.clone()
                            } else {
                                format!(
                                    "{}: {message}",
                                    current
                                        .as_ref()
                                        .map(|row| text(row, "name"))
                                        .unwrap_or_else(|| server.clone())
                                )
                            };
                            this.notification = Some(Notification::info(message));
                            cx.notify();
                        }
                    }
                },
            );
            self.services_panels.insert(
                self.server.clone(),
                HostedServices {
                    connection,
                    panel,
                    _subscription: subscription,
                },
            );
            self.services_refresh_requested = false;
        }
        if self.services_refresh_requested {
            self.services_refresh_requested = false;
            if let Some(hosted) = self.services_panels.get(&self.server) {
                hosted.panel.update(cx, |panel, cx| panel.refresh(cx));
            }
        }
    }

    pub(super) fn poll_services(&mut self, cx: &mut Context<Self>) {
        let available = self.configuration_available();
        let visible_server =
            (self.window_visible && !self.modal_open && self.page == Page::Services)
                .then_some(self.server.as_str());
        poll_services(
            &mut self.services_panels,
            &mut self.retired_services,
            &mut self.services_preferences,
            available.then_some(&self.snapshot),
            visible_server,
            cx,
        );
    }

    pub(super) fn services_screen(&self) -> AnyElement {
        if !self.configuration_available() {
            return div()
                .p_4()
                .child("Services aren’t available until server configuration is restored.")
                .into_any_element();
        }
        self.services_panels
            .get(&self.server)
            .map(|hosted| hosted.panel.clone().into_any_element())
            .unwrap_or_else(|| loading::page(Page::Services, self.content_width))
    }

    pub(super) fn open_service_terminal(
        &mut self,
        server: &str,
        command: String,
        cx: &mut Context<Self>,
    ) {
        let Some(config) = rows(&self.snapshot["config"]["servers"])
            .into_iter()
            .find(|row| row["id"] == server)
        else {
            return;
        };
        let connection = terminal_scope::connection(&config, &self.snapshot);
        let endpoint = format!("{}@{}", text(&config, "sshUser"), text(&config, "sshHost"));
        match open_service_command(
            &mut self.terminals,
            server,
            connection,
            endpoint,
            command,
            cx,
        ) {
            Ok(()) => {
                self.server = server.to_owned();
                self.navigate(Page::Terminal, cx);
            }
            Err(error) => self.notification = Some(Notification::error(error)),
        }
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::{HostedServices, open_service_command, poll_services};
    use crate::{services::tests::fixture, terminal_scope};
    use gpui_kit as gpui;
    use gpui_kit::{Subscription, TestAppContext};
    use newport_domain::services::{Scope, ServiceVerb};
    use serde_json::{Value, json};
    use std::{cell::Cell, collections::HashMap, rc::Rc, thread, time::Duration};

    #[gpui::test]
    fn command_handover_rejects_invalid_input_before_creating_workspace(cx: &mut TestAppContext) {
        cx.update(|cx| {
            let mut terminals = HashMap::new();
            for command in [
                "".into(),
                " \t".into(),
                "echo a\necho b".into(),
                "x".repeat(16 * 1024 + 1),
            ] {
                let result = open_service_command(
                    &mut terminals,
                    "fixture",
                    json!({"scope":"fixture"}),
                    "fixture@unconnected.invalid".into(),
                    command,
                    cx,
                );
                assert!(result.is_err());
                assert!(terminals.is_empty());
            }
        });
    }

    #[gpui::test]
    fn command_handover_creates_one_tab_then_appends_until_capacity(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui::component::init(cx);
            crate::theme::install(false, cx);
            let mut terminals = HashMap::new();
            for index in 0..newport_domain::terminal::MAX_TERMINALS {
                open_service_command(
                    &mut terminals,
                    "fixture",
                    json!({"scope":"fixture"}),
                    "fixture@unconnected.invalid".into(),
                    format!("sudo systemctl restart app-{index}.service"),
                    cx,
                )
                .unwrap();
                assert_eq!(terminals.len(), 1);
                let workspace = terminals["fixture"].read(cx);
                assert_eq!(
                    workspace.menu_state(cx),
                    Some((index, true, false, index > 0))
                );
                // Handover queues the first open until layout supplies its size.
                assert!(workspace.session_ids(cx).is_empty());
            }
            let workspace_id = terminals["fixture"].entity_id();
            let before = terminals["fixture"].read(cx).menu_state(cx);
            for command in ["sudo systemctl stop app.service", "echo invalid\n"] {
                assert!(
                    open_service_command(
                        &mut terminals,
                        "fixture",
                        json!({"scope":"fixture"}),
                        "fixture@unconnected.invalid".into(),
                        command.into(),
                        cx,
                    )
                    .is_err()
                );
                assert_eq!(terminals["fixture"].entity_id(), workspace_id);
                assert_eq!(terminals["fixture"].read(cx).menu_state(cx), before);
                assert!(terminals["fixture"].read(cx).session_ids(cx).is_empty());
            }
        });
    }

    fn configured() -> Value {
        json!({"instanceId":"original-backend","config":{"servers":[{
            "id":"fixture","name":"Test server","sshHost":"original.example",
            "sshUser":"user","sshPort":22,"authMethod":"sshAgent"
        }]},"runtime":{"connectionRevisions":{"fixture":1}}})
    }

    fn hidden_retirement(cx: &mut TestAppContext, change: impl FnOnce(&mut Value)) {
        let (panel, cx, backend) = fixture(cx);
        let original = configured();
        let disconnected = Rc::new(Cell::new(false));
        let on_drop = disconnected.clone();
        let mut hosted = HashMap::from([(
            "fixture".into(),
            HostedServices {
                connection: terminal_scope::connection(
                    &original["config"]["servers"][0],
                    &original,
                ),
                panel: panel.clone(),
                _subscription: Subscription::new(move || on_drop.set(true)),
            },
        )]);
        let mut retired = Vec::new();
        let mut preferences = HashMap::new();
        let expected_preferences = panel.read_with(cx, |panel, cx| panel.preferences(cx));
        backend.hold_actions();
        panel.update(cx, |panel, cx| {
            panel.test_action("app.service", Scope::System, ServiceVerb::Restart, cx)
        });
        for _ in 0..300 {
            if backend.held_actions() == 1 {
                break;
            }
            thread::sleep(Duration::from_millis(2));
        }
        assert_eq!(
            backend.held_actions(),
            1,
            "action reached isolated RPC fixture"
        );
        assert!(panel.read_with(cx, |panel, _| panel.has_pending()));
        let snapshots = backend.count("services_snapshot");
        let mut current = original;
        change(&mut current);
        // A wrongly dispatched read would now return different endpoint data.
        backend.set_snapshot(newport_domain::services::ServicesSnapshot::default());

        // No draw or sync_services() runs after configuration changes. Exercise
        // the same polling helper used by the real host while its window hides.
        cx.update(|_, cx| {
            poll_services(
                &mut hosted,
                &mut retired,
                &mut preferences,
                Some(&current),
                None,
                cx,
            )
        });
        assert!(hosted.is_empty());
        if current["config"]["servers"].as_array().unwrap().is_empty() {
            assert!(
                preferences.is_empty(),
                "deleted servers lose presentation choices"
            );
        } else {
            assert_eq!(preferences.get("fixture"), Some(&expected_preferences));
        }
        assert_eq!(
            retired.len(),
            1,
            "physical action is retained until drained"
        );
        assert!(disconnected.get(), "obsolete event subscription is removed");
        backend.release_action(json!({"ok":true}));
        for _ in 0..300 {
            cx.update(|_, cx| {
                poll_services(
                    &mut hosted,
                    &mut retired,
                    &mut preferences,
                    Some(&current),
                    None,
                    cx,
                )
            });
            if retired.is_empty() {
                break;
            }
            thread::sleep(Duration::from_millis(2));
        }
        assert!(retired.is_empty());
        assert!(!panel.read_with(cx, |panel, _| panel.has_pending()));
        assert_eq!(backend.count("service_action"), 1);
        assert_eq!(
            backend.count("services_snapshot"),
            snapshots,
            "old completion cannot read the replacement endpoint"
        );
    }

    #[gpui::test]
    fn hidden_backend_replacement_retires_before_action_completion(cx: &mut TestAppContext) {
        hidden_retirement(cx, |snapshot| {
            snapshot["instanceId"] = json!("replacement-backend")
        });
    }

    #[gpui::test]
    fn hidden_credential_change_retires_before_action_completion(cx: &mut TestAppContext) {
        hidden_retirement(cx, |snapshot| {
            snapshot["runtime"]["connectionRevisions"]["fixture"] = json!(2)
        });
    }

    #[gpui::test]
    fn hidden_server_deletion_retires_before_action_completion(cx: &mut TestAppContext) {
        hidden_retirement(cx, |snapshot| snapshot["config"]["servers"] = json!([]));
    }

    #[gpui::test]
    fn configuration_outage_holds_completions_until_connection_is_revalidated(
        cx: &mut TestAppContext,
    ) {
        let (panel, cx, backend) = fixture(cx);
        let mut snapshot = configured();
        let mut hosted = HashMap::from([(
            "fixture".into(),
            HostedServices {
                connection: terminal_scope::connection(
                    &snapshot["config"]["servers"][0],
                    &snapshot,
                ),
                panel: panel.clone(),
                _subscription: Subscription::new(|| {}),
            },
        )]);
        let mut retired = Vec::new();
        let mut preferences = HashMap::new();
        backend.hold_actions();
        panel.update(cx, |panel, cx| {
            panel.test_action("app.service", Scope::System, ServiceVerb::Restart, cx)
        });
        for _ in 0..300 {
            if backend.held_actions() == 1 {
                break;
            }
            thread::sleep(Duration::from_millis(2));
        }
        assert_eq!(backend.held_actions(), 1);
        let snapshots = backend.count("services_snapshot");
        let logs = backend.count("service_logs");
        backend.release_action(json!({"ok":true}));
        for _ in 0..20 {
            cx.update(|_, cx| {
                poll_services(&mut hosted, &mut retired, &mut preferences, None, None, cx)
            });
            thread::sleep(Duration::from_millis(2));
        }
        assert!(hosted.contains_key("fixture"));
        assert_eq!(backend.count("services_snapshot"), snapshots);
        assert_eq!(backend.count("service_logs"), logs);
        assert!(
            panel.read_with(cx, |panel, _| panel.has_pending()),
            "completion waits for validated configuration"
        );
        snapshot["instanceId"] = json!("replacement-backend");
        for _ in 0..300 {
            cx.update(|_, cx| {
                poll_services(
                    &mut hosted,
                    &mut retired,
                    &mut preferences,
                    Some(&snapshot),
                    None,
                    cx,
                )
            });
            if retired.is_empty() {
                break;
            }
            thread::sleep(Duration::from_millis(2));
        }
        assert!(hosted.is_empty());
        assert!(retired.is_empty());
        assert_eq!(
            backend.count("services_snapshot"),
            snapshots,
            "retired completion cannot refresh replacement connection"
        );
        assert_eq!(backend.count("service_logs"), logs);
    }

    #[gpui::test]
    fn temporary_configuration_failure_and_rename_preserve_hosted_panel(cx: &mut TestAppContext) {
        let (panel, cx, backend) = fixture(cx);
        let mut snapshot = configured();
        let mut hosted = HashMap::from([(
            "fixture".into(),
            HostedServices {
                connection: terminal_scope::connection(
                    &snapshot["config"]["servers"][0],
                    &snapshot,
                ),
                panel: panel.clone(),
                _subscription: Subscription::new(|| {}),
            },
        )]);
        let mut retired = Vec::new();
        let expected_preferences = panel.read_with(cx, |panel, cx| panel.preferences(cx));
        let mut preferences = HashMap::from([
            ("fixture".into(), expected_preferences.clone()),
            ("removed-server".into(), expected_preferences.clone()),
        ]);
        let snapshots = backend.count("services_snapshot");
        cx.update(|_, cx| {
            poll_services(&mut hosted, &mut retired, &mut preferences, None, None, cx)
        });
        assert!(hosted.contains_key("fixture"));
        assert_eq!(preferences.get("fixture"), Some(&expected_preferences));
        assert!(
            preferences.contains_key("removed-server"),
            "unavailable configuration must not discard cached choices"
        );
        snapshot["config"]["servers"][0]["name"] = json!("Renamed server");
        cx.update(|_, cx| {
            poll_services(
                &mut hosted,
                &mut retired,
                &mut preferences,
                Some(&snapshot),
                None,
                cx,
            )
        });
        assert!(hosted.contains_key("fixture"));
        assert!(
            !preferences.contains_key("removed-server"),
            "a confirmed deletion prunes cached choices even without a hosted view"
        );
        assert!(retired.is_empty());
        assert_eq!(backend.count("services_snapshot"), snapshots);
    }
}
