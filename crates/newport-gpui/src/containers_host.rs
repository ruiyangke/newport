//! Retain each Containers screen across navigation, scoped to its connection.
//! Only session presentation choices cross a connection replacement, keyed by
//! stable server ID; they never enter the on-disk workspace selection.
use super::*;
use newport_protocol::monitor::compose;

pub(super) struct HostedContainers {
    connection: Option<configuration::ConnectionScope>,
    panel: Entity<containers::ContainersPanel>,
    _subscription: Subscription,
}

#[derive(Clone)]
struct ComposeOutputTarget {
    server: String,
    connection: Option<configuration::ConnectionScope>,
    project: String,
    job: String,
    panel: WeakEntity<containers::ContainersPanel>,
}

impl ComposeOutputTarget {
    /// A persistent toast must not route an old job into a replacement server
    /// or a newly created panel that happens to have the same server ID.
    fn validated_panel(
        &self,
        hosted: &std::collections::HashMap<String, HostedContainers>,
        configuration: Option<&configuration::Snapshot>,
    ) -> Result<Entity<containers::ContainersPanel>, &'static str> {
        let snapshot = configuration
            .ok_or("Command output is unavailable until server configuration is restored.")?;
        let current = snapshot
            .config
            .servers
            .iter()
            .find(|server| server.id.to_string() == self.server);
        if current
            .map(|server| Some(snapshot.connection_scope(server)))
            .as_ref()
            != Some(&self.connection)
        {
            return Err(
                "This server connection changed. Open Containers to view its current activity.",
            );
        }
        let current = hosted.get(&self.server).ok_or(
            "This activity session has closed. Open Containers to view its current activity.",
        )?;
        if current.connection != self.connection
            || self
                .panel
                .upgrade()
                .is_none_or(|original| original.entity_id() != current.panel.entity_id())
        {
            return Err(
                "This activity session has changed. Open Containers to view its current activity.",
            );
        }
        Ok(current.panel.clone())
    }
}

fn compose_notification(job: &compose::Job, server_name: Option<&str>) -> Option<Notification> {
    use compose::State;
    use newport_domain::compose::job_status;
    let notification = match job.state {
        State::Running => return None,
        State::Succeeded => Notification::success(job.command.clone()),
        State::Failed => Notification::error(job.command.clone()),
        State::Cancelled => Notification::info(job.command.clone()),
    };
    let subject = server_name.map_or_else(
        || job.project.clone(),
        |server| format!("{server} · {}", job.project),
    );
    Some(notification.title(format!(
        "{subject}: {}",
        job_status(job, chrono::Local::now().timestamp_millis()).label
    )))
}

fn retire_obsolete_containers(
    hosted: &mut std::collections::HashMap<String, HostedContainers>,
    retired: &mut Vec<Entity<containers::ContainersPanel>>,
    preferences: &mut std::collections::HashMap<String, containers::Preferences>,
    snapshot: Option<&configuration::Snapshot>,
    available: bool,
    cx: &mut App,
) {
    let Some(snapshot) = snapshot.filter(|_| available) else {
        return;
    };
    let servers = &snapshot.config.servers;
    let saved = |id: &str| servers.iter().find(|server| server.id.to_string() == id);
    preferences.retain(|id, _| saved(id).is_some());
    let obsolete: Vec<_> = hosted
        .iter()
        .filter(|(id, panel)| {
            saved(id)
                .map(|server| Some(snapshot.connection_scope(server)))
                .as_ref()
                != Some(&panel.connection)
        })
        .map(|(id, _)| id.clone())
        .collect();
    for id in obsolete {
        let panel = hosted.remove(&id).expect("collected entry");
        if saved(&id).is_some() {
            preferences.insert(id, panel.panel.read(cx).preferences(cx));
        }
        panel.panel.update(cx, |panel, cx| panel.retire(cx));
        retired.push(panel.panel);
    }
}

fn poll_containers(
    hosted: &mut std::collections::HashMap<String, HostedContainers>,
    retired: &mut Vec<Entity<containers::ContainersPanel>>,
    preferences: &mut std::collections::HashMap<String, containers::Preferences>,
    configuration: Option<&configuration::Snapshot>,
    visible_server: Option<&str>,
    cx: &mut App,
) {
    // Rendering can stop while hidden. Retire obsolete connections before
    // their completion can schedule a read against a reused server ID.
    if configuration.is_some() {
        retire_obsolete_containers(hosted, retired, preferences, configuration, true, cx);
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

fn open_container_shell(
    terminals: &mut std::collections::HashMap<String, Entity<terminal_workspace::Workspace>>,
    server: &str,
    connection: Option<configuration::ConnectionScope>,
    endpoint: String,
    command: String,
    title: String,
    cx: &mut App,
) -> Result<(), String> {
    let draft = newport_client::terminal_command::CommandDraft::container_shell(command)?;
    match terminals.entry(server.to_owned()) {
        std::collections::hash_map::Entry::Occupied(entry) => entry
            .get()
            .update(cx, |workspace, cx| workspace.open_shell(draft, title, cx)),
        std::collections::hash_map::Entry::Vacant(entry) => {
            entry.insert(cx.new(|cx| {
                terminal_workspace::Workspace::with_shell(
                    server.to_owned(),
                    connection,
                    endpoint,
                    draft,
                    title,
                    cx,
                )
            }));
            Ok(())
        }
    }
}

impl Newport {
    fn open_compose_notification(
        &mut self,
        target: &ComposeOutputTarget,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let result = (|| {
            if self.server_form.is_some() || self.modal_open || window.has_active_dialog(cx) {
                return Err("Close the current dialog before opening command output.");
            }
            let panel = target.validated_panel(
                &self.containers_panels,
                self.snapshot
                    .as_ref()
                    .filter(|_| self.configuration_available()),
            )?;
            if !panel.update(cx, |panel, cx| {
                panel.open_compose_activity(&target.project, &target.job, window, cx)
            }) {
                return Err(
                    "This command's output is no longer available. Open Activity to view recent commands.",
                );
            }
            // Validation and navigation share one synchronous UI turn. Save the
            // previous destination before changing server, including when the
            // current page is already Containers and navigate() would be a no-op.
            if self.server != target.server || self.page != Page::Containers {
                self.save_workspace_selection();
                self.reset();
                self.server = target.server.clone();
                self.restore_git = None;
                self.page = Page::Containers;
                self.workspace_page = Page::Containers;
                self.save_workspace_selection();
                self.refresh();
            }
            cx.notify();
            Ok(())
        })();
        if let Err(message) = result {
            window.push_notification(Notification::info(message), cx);
        }
    }

    fn open_container_shell(
        &mut self,
        server: &str,
        command: String,
        title: String,
        cx: &mut Context<Self>,
    ) {
        let Some(config) = self.saved_server(server) else {
            return;
        };
        let connection = self.connection(config);
        let endpoint = format!("{}@{}", config.ssh_user, config.ssh_host);
        match open_container_shell(
            &mut self.terminals,
            server,
            connection,
            endpoint,
            command,
            title,
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

    fn retire_obsolete_containers(&mut self, cx: &mut Context<Self>) {
        let available = self.configuration_available();
        retire_obsolete_containers(
            &mut self.containers_panels,
            &mut self.retired_containers,
            &mut self.containers_preferences,
            self.snapshot.as_ref(),
            available,
            cx,
        );
    }

    pub(super) fn sync_containers(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.retire_obsolete_containers(cx);
        if !self.configuration_available() {
            return;
        }
        if self.page != Page::Containers || self.selected_server().is_none() {
            return;
        }
        if !self.containers_panels.contains_key(&self.server) {
            let server = self.server.clone();
            let connection = self.selected_connection();
            let preferences = self.containers_preferences.remove(&server);
            let panel = cx.new(|cx| {
                let mut panel = containers::ContainersPanel::new(
                    server.clone(),
                    connection.clone(),
                    window,
                    cx,
                );
                if let Some(preferences) = preferences {
                    panel.restore_preferences(preferences, window, cx);
                }
                panel
            });
            let event_connection = connection.clone();
            let subscription = cx.subscribe_in(
                &panel,
                window,
                move |this, panel, event: &containers::ContainersEvent, window, cx| {
                    if !this.configuration_available()
                        || this
                            .connection_of(&server)
                            .is_none_or(|current| Some(current) != event_connection)
                    {
                        return;
                    }
                    let name = this.saved_server(&server).map(|row| row.name.clone());
                    match event {
                        containers::ContainersEvent::ComposeCompleted(job) => {
                            let Some(mut notification) = compose_notification(
                                job,
                                (this.server != server).then_some(name.as_deref()).flatten(),
                            ) else {
                                return;
                            };
                            if job.state == compose::State::Failed {
                                let owner = cx.entity().downgrade();
                                let target = ComposeOutputTarget {
                                    server: server.clone(),
                                    connection: event_connection.clone(),
                                    project: job.project.clone(),
                                    job: job.job.clone(),
                                    panel: panel.downgrade(),
                                };
                                notification = notification.action(move |_, _, cx| {
                                    let owner = owner.clone();
                                    let target = target.clone();
                                    crate::controls::compact_button("compose-view-output")
                                        .label("View output")
                                        .on_click(cx.listener(
                                            move |notification, _, window, cx| {
                                                cx.stop_propagation();
                                                notification.dismiss(window, cx);
                                                let _ = owner.update(cx, |this, cx| {
                                                    this.open_compose_notification(
                                                        &target, window, cx,
                                                    );
                                                });
                                            },
                                        ))
                                });
                            }
                            window.push_notification(notification, cx);
                        }
                        containers::ContainersEvent::OpenPort(target) => {
                            panel.update(cx, |panel, cx| {
                                panel.open_port_link(
                                    &server,
                                    event_connection.clone(),
                                    target,
                                    &mut this.tunnel_operations,
                                    cx,
                                );
                            });
                        }
                        containers::ContainersEvent::OpenShell { command, title } => {
                            this.open_container_shell(&server, command.clone(), title.clone(), cx);
                        }
                        containers::ContainersEvent::OpenTerminal { command } => {
                            this.open_service_terminal(&server, command.clone(), cx);
                        }
                        containers::ContainersEvent::Notification { message } => {
                            let message = if this.server == server {
                                message.clone()
                            } else {
                                format!(
                                    "{}: {message}",
                                    name.clone().unwrap_or_else(|| server.clone())
                                )
                            };
                            this.notification = Some(Notification::info(message));
                            cx.notify();
                        }
                    }
                },
            );
            self.containers_panels.insert(
                self.server.clone(),
                HostedContainers {
                    connection,
                    panel,
                    _subscription: subscription,
                },
            );
            self.containers_refresh_requested = false;
        }
        if self.containers_refresh_requested {
            self.containers_refresh_requested = false;
            if let Some(hosted) = self.containers_panels.get(&self.server) {
                hosted.panel.update(cx, |panel, cx| panel.refresh(cx));
            }
        }
    }

    pub(super) fn poll_containers(&mut self, cx: &mut Context<Self>) {
        let available = self.configuration_available();
        // This page's confirmation dialog still needs current container state
        // and operation receipts. Suspending it as an unrelated overlay would
        // invalidate the very review the user just opened.
        let own_dialog = self
            .containers_panels
            .get(&self.server)
            .is_some_and(|hosted| hosted.panel.read(cx).has_review_dialog());
        let visible_server = (self.window_visible
            && (!self.modal_open || own_dialog)
            && self.page == Page::Containers
            && !self.needle_missing())
        .then_some(self.server.as_str());
        poll_containers(
            &mut self.containers_panels,
            &mut self.retired_containers,
            &mut self.containers_preferences,
            self.snapshot.as_ref().filter(|_| available),
            visible_server,
            cx,
        );
        if let Some((server, hosted)) =
            visible_server.and_then(|server| self.containers_panels.get_key_value(server))
        {
            hosted.panel.update(cx, |panel, cx| {
                panel.sync_port_links(
                    server,
                    self.snapshot.as_ref().filter(|_| available),
                    &self.tunnel_operations,
                    cx,
                );
            });
        }
    }

    /// The selected server's Containers have a reading to show.
    pub(super) fn containers_loaded(&self, cx: &App) -> bool {
        self.containers_panels
            .get(&self.server)
            .is_some_and(|hosted| hosted.panel.read(cx).loaded())
    }
    pub(super) fn containers_screen(&self) -> AnyElement {
        if !self.configuration_available() {
            return div()
                .p_4()
                .child("Containers aren’t available until server configuration is restored.")
                .into_any_element();
        }
        self.containers_panels
            .get(&self.server)
            .map(|hosted| hosted.panel.clone().into_any_element())
            .unwrap_or_else(|| loading::page(Page::Containers, self.content_width))
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ComposeOutputTarget, HostedContainers, compose_notification, open_container_shell,
        poll_containers, retire_obsolete_containers,
    };
    use crate::containers::{
        ContainersPanel,
        tests::{container_done, draw, fixture, restart_web, settle},
    };
    use crate::snapshot_fixture as config_fixture;
    use gpui_kit as gpui;
    use gpui_kit::component::WindowExt;
    use gpui_kit::{AppContext, Subscription, TestAppContext};
    use newport_protocol::desktop::configuration;
    use newport_protocol::monitor::compose::{Job, State};
    use std::{cell::Cell, collections::HashMap, rc::Rc, thread, time::Duration};

    #[gpui::test]
    fn invalid_shell_handover_cannot_create_a_workspace(cx: &mut TestAppContext) {
        cx.update(|cx| {
            let mut terminals = HashMap::new();
            for command in [
                "".into(),
                "echo a\necho b".into(),
                "x".repeat(16 * 1024 + 1),
            ] {
                assert!(
                    open_container_shell(
                        &mut terminals,
                        "fixture",
                        Some(crate::snapshot_fixture::scope("fixture")),
                        "fixture@unconnected.invalid".into(),
                        command,
                        "web · Shell".into(),
                        cx,
                    )
                    .is_err()
                );
                assert!(terminals.is_empty());
            }
        });
    }

    #[gpui::test]
    fn shell_handover_routes_to_requested_server_and_appends_until_capacity(
        cx: &mut TestAppContext,
    ) {
        cx.update(|cx| {
            gpui::component::init(cx);
            crate::theme::install(false, cx);
            let mut terminals = HashMap::new();
            open_container_shell(
                &mut terminals,
                "other",
                Some(crate::snapshot_fixture::scope("other")),
                "other@unconnected.invalid".into(),
                "docker exec -it -- 012345 sh".into(),
                "other · Shell".into(),
                cx,
            )
            .unwrap();
            let other_id = terminals["other"].entity_id();
            let other_state = terminals["other"].read(cx).menu_state(cx);
            for index in 0..newport_domain::terminal::MAX_TERMINALS {
                open_container_shell(
                    &mut terminals,
                    "fixture",
                    Some(crate::snapshot_fixture::scope("fixture")),
                    "fixture@unconnected.invalid".into(),
                    "docker exec -it -- abcdef sh".into(),
                    format!("web-{index} · Shell"),
                    cx,
                )
                .unwrap();
                assert_eq!(terminals.len(), 2);
                let workspace = terminals["fixture"].read(cx);
                assert_eq!(
                    workspace.menu_state(cx),
                    Some((index, true, false, index > 0))
                );
                // Handover queues the first open until layout supplies its size.
                assert!(workspace.session_ids(cx).is_empty());
            }
            let before = terminals["fixture"].read(cx).menu_state(cx);
            assert!(
                open_container_shell(
                    &mut terminals,
                    "fixture",
                    Some(crate::snapshot_fixture::scope("fixture")),
                    "fixture@unconnected.invalid".into(),
                    "docker exec -it -- abcdef sh".into(),
                    "over capacity · Shell".into(),
                    cx,
                )
                .is_err()
            );
            assert_eq!(terminals["fixture"].read(cx).menu_state(cx), before);
            assert_eq!(terminals["other"].entity_id(), other_id);
            assert_eq!(terminals["other"].read(cx).menu_state(cx), other_state);
            assert!(terminals["other"].read(cx).session_ids(cx).is_empty());
        });
    }

    const ID: &str = "00000000-0000-4000-8000-000000000001";
    fn configured() -> configuration::Snapshot {
        let mut server = config_fixture::server(config_fixture::SERVER, "Test server");
        server.ssh_host = "original.example".into();
        server.ssh_user = "user".into();
        config_fixture::snapshot(vec![server])
    }

    #[gpui::test]
    fn container_session_preferences_follow_server_id_and_expire_on_deletion(
        cx: &mut TestAppContext,
    ) {
        let (panel, cx, backend) = fixture(cx);
        for label in ["Inspect", "Fold stack"] {
            let position = cx.update(|window, _| {
                gpui::base::test_support::snapshots(window)
                    .into_iter()
                    .find(|element| element.label() == Some(label))
                    .unwrap_or_else(|| panic!("missing {label}"))
                    .bounds()
                    .center()
            });
            cx.simulate_click(position, Default::default());
            draw(cx);
        }
        let expected = panel.read_with(cx, |panel, cx| panel.preferences(cx));
        assert_ne!(expected, Default::default());
        let mut configuration = configured();
        let mut hosted = HashMap::from([(
            ID.into(),
            HostedContainers {
                connection: Some(configuration.connection_scope(&configuration.config.servers[0])),
                panel,
                _subscription: Subscription::new(|| {}),
            },
        )]);
        let mut retired = Vec::new();
        let mut preferences = HashMap::new();
        config_fixture::set(
            &mut configuration.runtime.connection_revisions,
            config_fixture::SERVER,
            2,
        );
        cx.update(|_, cx| {
            retire_obsolete_containers(
                &mut hosted,
                &mut retired,
                &mut preferences,
                Some(&configuration),
                true,
                cx,
            )
        });
        assert!(hosted.is_empty());
        assert_eq!(preferences.get(ID), Some(&expected));
        let replacement = cx.update(|window, cx| {
            cx.new(|cx| {
                let mut panel = ContainersPanel::with_client(backend.client(), window, cx);
                panel.restore_preferences(preferences.remove(ID).unwrap(), window, cx);
                panel
            })
        });
        settle(&replacement, cx);
        assert_eq!(
            replacement.read_with(cx, |panel, cx| panel.preferences(cx)),
            expected
        );
        hosted.insert(
            ID.into(),
            HostedContainers {
                connection: Some(configuration.connection_scope(&configuration.config.servers[0])),
                panel: replacement,
                _subscription: Subscription::new(|| {}),
            },
        );
        preferences.insert("already-closed-server".into(), expected);
        configuration.config.servers.clear();
        cx.update(|_, cx| {
            retire_obsolete_containers(
                &mut hosted,
                &mut retired,
                &mut preferences,
                Some(&configuration),
                true,
                cx,
            )
        });
        assert!(hosted.is_empty());
        assert!(preferences.is_empty());
        assert_eq!(backend.writes(), 0);
    }

    #[gpui::test]
    fn compose_notification_target_rejects_replaced_connections_and_panels(
        cx: &mut TestAppContext,
    ) {
        let (panel, cx, backend) = fixture(cx);
        let snapshot = configured();
        let connection = Some(snapshot.connection_scope(&snapshot.config.servers[0]));
        let target = ComposeOutputTarget {
            server: ID.into(),
            connection: connection.clone(),
            project: "app".into(),
            job: "failed-command".into(),
            panel: panel.downgrade(),
        };
        let mut hosted = HashMap::from([(
            ID.into(),
            HostedContainers {
                connection: connection.clone(),
                panel: panel.clone(),
                _subscription: Subscription::new(|| {}),
            },
        )]);
        assert_eq!(
            target
                .validated_panel(&hosted, Some(&snapshot))
                .unwrap()
                .entity_id(),
            panel.entity_id()
        );
        assert!(target.validated_panel(&hosted, None).is_err());
        for change in ["backend", "credentials", "endpoint", "delete"] {
            let mut changed = snapshot.clone();
            match change {
                "backend" => changed.instance_id = config_fixture::NEXT_INSTANCE,
                "credentials" => config_fixture::set(
                    &mut changed.runtime.connection_revisions,
                    config_fixture::SERVER,
                    2,
                ),
                "endpoint" => changed.config.servers[0].ssh_host = "different.invalid".into(),
                _ => changed.config.servers.clear(),
            }
            assert!(
                target.validated_panel(&hosted, Some(&changed)).is_err(),
                "{change}"
            );
        }
        let mut renamed = snapshot.clone();
        renamed.config.servers[0].name = "Renamed server".into();
        assert!(target.validated_panel(&hosted, Some(&renamed)).is_ok());

        let replacement = cx.update(|window, cx| {
            cx.new(|cx| ContainersPanel::with_client(backend.client(), window, cx))
        });
        hosted.get_mut(ID).unwrap().panel = replacement;
        assert!(target.validated_panel(&hosted, Some(&snapshot)).is_err());
        hosted.get_mut(ID).unwrap().panel = panel;
        hosted.get_mut(ID).unwrap().connection = Some(crate::snapshot_fixture::scope("replaced"));
        assert!(target.validated_panel(&hosted, Some(&snapshot)).is_err());
        hosted.clear();
        assert!(target.validated_panel(&hosted, Some(&snapshot)).is_err());
    }

    #[gpui::test]
    fn compose_completions_pushed_in_one_frame_remain_separate(cx: &mut TestAppContext) {
        let (_, cx, _backend) = fixture(cx);
        cx.update(|window, cx| {
            for (index, state) in [State::Succeeded, State::Failed, State::Cancelled]
                .into_iter()
                .enumerate()
            {
                let job = Job {
                    job: format!("job-{index}"),
                    project: "app".into(),
                    command: "docker compose up -d".into(),
                    state,
                    started_at: 1000,
                    finished_at: Some(3000),
                    exit_code: (state == State::Failed).then_some(17),
                    ..Default::default()
                };
                window.push_notification(
                    compose_notification(&job, Some("Test server")).unwrap(),
                    cx,
                );
            }
            assert_eq!(window.notifications(cx).len(), 3);
            assert!(compose_notification(&Job::default(), None).is_none());
        });
    }

    fn hidden_retirement(
        cx: &mut TestAppContext,
        change: impl FnOnce(&mut configuration::Snapshot),
    ) {
        let (panel, cx, backend) = fixture(cx);
        let original = configured();
        let disconnected = Rc::new(Cell::new(false));
        let on_drop = disconnected.clone();
        let mut hosted = HashMap::from([(
            ID.into(),
            HostedContainers {
                connection: Some(original.connection_scope(&original.config.servers[0])),
                panel: panel.clone(),
                _subscription: Subscription::new(move || on_drop.set(true)),
            },
        )]);
        let mut retired = Vec::new();
        let mut preferences = HashMap::new();
        let expected_preferences = panel.read_with(cx, |panel, cx| panel.preferences(cx));
        backend.hold_actions();
        restart_web(&panel, cx);
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
        let snapshots = backend.count("containers_snapshot");
        let mut current = original;
        change(&mut current);
        // A wrongly dispatched read would now return different endpoint data.
        backend.set_snapshot(newport_protocol::monitor::containers::ContainersSnapshot::default());

        // No draw or sync_containers() runs after configuration changes. Exercise
        // the same polling helper used by the real host while its window hides.
        cx.update(|_, cx| {
            poll_containers(
                &mut hosted,
                &mut retired,
                &mut preferences,
                Some(&current),
                None,
                cx,
            )
        });
        assert!(hosted.is_empty());
        if current.config.servers.is_empty() {
            assert!(
                preferences.is_empty(),
                "deleted servers lose presentation choices"
            );
        } else {
            assert_eq!(preferences.get(ID), Some(&expected_preferences));
        }
        assert_eq!(
            retired.len(),
            1,
            "physical action is retained until drained"
        );
        assert!(disconnected.get(), "obsolete event subscription is removed");
        backend.release_action(container_done());
        for _ in 0..300 {
            cx.update(|_, cx| {
                poll_containers(
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
        assert_eq!(backend.count("container_action"), 1);
        assert_eq!(
            backend.count("containers_snapshot"),
            snapshots,
            "old completion cannot read the replacement endpoint"
        );
    }

    #[gpui::test]
    fn hidden_backend_replacement_retires_before_action_completion(cx: &mut TestAppContext) {
        hidden_retirement(cx, |snapshot| {
            snapshot.instance_id = config_fixture::NEXT_INSTANCE
        });
    }

    #[gpui::test]
    fn hidden_credential_change_retires_before_action_completion(cx: &mut TestAppContext) {
        hidden_retirement(cx, |snapshot| {
            config_fixture::set(
                &mut snapshot.runtime.connection_revisions,
                config_fixture::SERVER,
                2,
            )
        });
    }

    #[gpui::test]
    fn hidden_server_deletion_retires_before_action_completion(cx: &mut TestAppContext) {
        hidden_retirement(cx, |snapshot| snapshot.config.servers.clear());
    }

    #[gpui::test]
    fn configuration_outage_holds_completions_until_connection_is_revalidated(
        cx: &mut TestAppContext,
    ) {
        let (panel, cx, backend) = fixture(cx);
        let mut snapshot = configured();
        let mut hosted = HashMap::from([(
            ID.into(),
            HostedContainers {
                connection: Some(snapshot.connection_scope(&snapshot.config.servers[0])),
                panel: panel.clone(),
                _subscription: Subscription::new(|| {}),
            },
        )]);
        let mut retired = Vec::new();
        let mut preferences = HashMap::new();
        backend.hold_actions();
        restart_web(&panel, cx);
        for _ in 0..300 {
            if backend.held_actions() == 1 {
                break;
            }
            thread::sleep(Duration::from_millis(2));
        }
        assert_eq!(backend.held_actions(), 1);
        let snapshots = backend.count("containers_snapshot");
        let logs = backend.count("container_logs");
        backend.release_action(container_done());
        for _ in 0..20 {
            cx.update(|_, cx| {
                poll_containers(&mut hosted, &mut retired, &mut preferences, None, None, cx)
            });
            thread::sleep(Duration::from_millis(2));
        }
        assert!(hosted.contains_key(ID));
        assert_eq!(backend.count("containers_snapshot"), snapshots);
        assert_eq!(backend.count("container_logs"), logs);
        assert!(
            panel.read_with(cx, |panel, _| panel.has_pending()),
            "completion waits for validated configuration"
        );
        snapshot.instance_id = config_fixture::NEXT_INSTANCE;
        for _ in 0..300 {
            cx.update(|_, cx| {
                poll_containers(
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
            backend.count("containers_snapshot"),
            snapshots,
            "retired completion cannot refresh replacement connection"
        );
        assert_eq!(backend.count("container_logs"), logs);
    }

    #[gpui::test]
    fn temporary_configuration_failure_and_rename_preserve_hosted_panel(cx: &mut TestAppContext) {
        let (panel, cx, backend) = fixture(cx);
        let mut snapshot = configured();
        let mut hosted = HashMap::from([(
            ID.into(),
            HostedContainers {
                connection: Some(snapshot.connection_scope(&snapshot.config.servers[0])),
                panel: panel.clone(),
                _subscription: Subscription::new(|| {}),
            },
        )]);
        let mut retired = Vec::new();
        let expected_preferences = panel.read_with(cx, |panel, cx| panel.preferences(cx));
        let mut preferences = HashMap::from([
            (ID.into(), expected_preferences.clone()),
            ("removed-server".into(), expected_preferences.clone()),
        ]);
        let snapshots = backend.count("containers_snapshot");
        cx.update(|_, cx| {
            poll_containers(&mut hosted, &mut retired, &mut preferences, None, None, cx)
        });
        assert!(hosted.contains_key(ID));
        assert_eq!(preferences.get(ID), Some(&expected_preferences));
        assert!(
            preferences.contains_key("removed-server"),
            "unavailable configuration must not discard cached choices"
        );
        snapshot.config.servers[0].name = "Renamed server".into();
        cx.update(|_, cx| {
            poll_containers(
                &mut hosted,
                &mut retired,
                &mut preferences,
                Some(&snapshot),
                None,
                cx,
            )
        });
        assert!(hosted.contains_key(ID));
        assert!(
            !preferences.contains_key("removed-server"),
            "a confirmed deletion prunes cached choices even without a hosted view"
        );
        assert!(retired.is_empty());
        assert_eq!(backend.count("containers_snapshot"), snapshots);
    }
}
