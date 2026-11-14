//! Offline whole-page CPU fixtures. No main-loop timer, startup request, or save hook.
use super::{
    Newport, Page, appearance, file_preview, git, navigation, overview, settings, sidebar_layout,
    theme, ui_state, window_state, workspace_state,
};
use crate::connections::fixtures::{self as discovery_fixture, Listener};
use crate::snapshot_fixture as fixture;
use gpui_kit::{
    self as gpui, AppContext, Context, Entity, IntoElement, Render, TestAppContext,
    VisualTestContext, Window, div, prelude::*, px, size,
};
use newport_client::{backend, resources::Resources};
use newport_protocol::desktop::configuration;
use serde_json::{Value, json};
use std::time::Instant;

pub(super) fn owner(window: &mut Window, cx: &mut Context<Newport>) -> Newport {
    Newport::observe_connectivity(cx);
    let restored = workspace_state::Selection::default();
    let window_state = window_state::WindowState::load(None, cx);
    let sidebar_layout = sidebar_layout::SidebarLayout::isolated(cx);
    let appearance = appearance::Appearance::Light;
    // A private nonexistent socket also isolates any accidental request from the host.
    let backend = backend::Backend::at_path(
        std::env::temp_dir().join(format!("np-page-bench-{}.sock", uuid::Uuid::new_v4())),
    );
    let (work, _) = std::sync::mpsc::sync_channel(1);
    let (_, replies) = std::sync::mpsc::channel();
    let chart_focus = cx.focus_handle();
    let mut this = Newport {
        ui_state: ui_state::Writer::disabled(),
        window_state,
        terminals: Default::default(),
        terminal_immersive: false,
        terminal_setup: Default::default(),
        integration_refresh_pending: Default::default(),
        needle_details: Default::default(),
        needle_uninstall_confirmations: Default::default(),
        integration_shell: cx.new(|cx| crate::integration::ShellCode::new(window, cx)),
        integration_scroll: gpui::ScrollHandle::new(),
        server_form: None,
        modal_open: false,
        quit_prompt: Default::default(),
        server_picker: Newport::init_picker(window, cx),
        picker_items: Vec::new(),
        picker_selection: String::new(),
        page: restored.page,
        workspace_page: restored.workspace_page,
        restore_git: restored.git,
        git_destinations: restored.git_destinations,
        appearance,
        settings_state: newport_client::settings::Settings::new(backend.clone()),
        settings_ui: settings::SettingsUi::new(appearance, window, cx),
        account: newport_client::account::Account::new(backend.clone()),
        services_panels: Default::default(),
        services_preferences: Default::default(),
        retired_services: Vec::new(),
        services_refresh_requested: false,
        setup_flow: None,
        welcome: Default::default(),
        overview_state: overview::OverviewState {
            chart_focus: Some(chart_focus),
            interfaces: restored.network_interfaces,
            preferences: restored.overview_preferences,
            ..Default::default()
        },
        connections_state: Default::default(),
        tunnel_operations: newport_client::tunnel_operations::Operations::new(backend.clone()),
        containers_panels: Default::default(),
        containers_preferences: Default::default(),
        retired_containers: Vec::new(),
        containers_refresh_requested: false,
        sidebar_visible: true,
        sidebar_layout,
        content_width: 854.,
        server: restored.server,
        saved_server_selection: None,
        snapshot: None,
        snapshot_updated_at: None,
        resources: Resources::default(),
        work: work.into(),
        replies,
        last_refresh: Instant::now(),
        placeholders_checked_at: Instant::now(),
        window_visible: window.is_visible(),
        focus: cx.focus_handle(),
        sidebar_focus: cx.focus_handle(),
        navigation_focus: navigation::views(cx)
            .iter()
            .map(|_| cx.focus_handle())
            .collect(),
        file_browsers: Default::default(),
        files_refresh_requested: true,
        file_preview_path: None,
        file_preview: None,
        file_places: None,
        remote_apps: None,
        remote_apps_visit: 0,
        remote_apps_away: false,
        remote_apps_returned: false,
        remote_apps_transport: None,
        remote_apps_mutations: Default::default(),
        remote_apps_readings: Default::default(),
        remote_apps_mutating: Default::default(),
        remote_apps_recheck: None,
        remote_apps_record_tried: None,
        remote_apps_refresh: false,
        file_preview_viewport: file_preview::Viewport::default(),
        file_transfer: None,
        file_chooser: None,
        file_project_connection: None,
        selection: None,
        project_add: None,
        project_edit: None,
        project_create: None,
        project_creation_save_owner: None,
        project_folder: None,
        project_folder_listing: None,
        connection_replies: Default::default(),
        project_dialog_open: false,
        project_dialog_feedback: Default::default(),
        project_dialog_rendering: false,
        project_checks: None,
        project_libraries: Default::default(),
        project_preferences: restored.project_preferences,
        git: git::Workspace::default(),
        code_workspaces: Vec::new(),
        git_diff_layouts: Default::default(),
        git_mutation: None,
        git_notice_clear: false,
        git_log: Default::default(),
        git_drafts: Default::default(),
        notice: None,
        notification: None,
    };

    this.server = fixture::SERVER.to_string();
    this.snapshot = Some(owner_snapshot());
    this.sync_project_library(window, cx);
    this
}
/// The owner's snapshot: one set-up server with both integrations on and a
/// connected clipboard.
pub(super) fn owner_snapshot() -> configuration::Snapshot {
    let mut server = fixture::server(fixture::SERVER, "Fixture");
    server.clipboard_enabled = true;
    server.browser_enabled = true;
    let mut snapshot = fixture::snapshot(vec![server]);
    fixture::set(
        &mut snapshot.runtime.clipboard,
        fixture::SERVER,
        fixture::connection(configuration::ConnectionStatus::Connected),
    );
    snapshot
}
fn typed_owner(window: &mut Window, cx: &mut Context<Newport>) -> Newport {
    owner(window, cx)
}
/// The owner with its server saved as `server`, from backend `instance`.
pub(super) fn owner_at(
    window: &mut Window,
    cx: &mut Context<Newport>,
    server: uuid::Uuid,
    instance: uuid::Uuid,
) -> Newport {
    let mut view = owner(window, cx);
    view.server = server.to_string();
    let snapshot = view.snapshot_mut();
    snapshot.instance_id = instance;
    snapshot.config.servers[0].id = server;
    for row in &mut snapshot.runtime.clipboard {
        row.id = server;
    }
    for row in &mut snapshot.runtime.connection_revisions {
        row.id = server;
    }
    view
}
/// The owner's snapshot, as the view holds it.
fn snapshot_of(view: &Newport) -> configuration::Snapshot {
    view.snapshot.clone().expect("the owner has a snapshot")
}
// One-way observations keep UI assertions readable; these maps are never sent.
fn observed(work: &super::Work) -> (&'static str, Value) {
    use newport_protocol::desktop::{self, configuration as config};
    if let super::request_dispatch::Request::Discovery {
        server, refresh, ..
    } = &work.request
    {
        return ("connections_query", json!({"id":server,"refresh":refresh}));
    }
    if let super::request_dispatch::Request::Git(args) = &work.request {
        return (
            "git_request",
            json!({"serverId":args.server,"request":args.request}),
        );
    }
    let Some(operation) = work.request.operation() else {
        return ("local", Value::Null);
    };
    match operation {
        desktop::Operation::Configuration(value) => match value.as_ref() {
            config::Operation::Snapshot(_) => ("snapshot", json!({})),
            config::Operation::SetIntegrationEnabled(v) => (
                "set_integration_enabled",
                json!({"id":v.server,"enabled":v.enabled,"feature":match v.feature {config::Feature::Clipboard=>"clipboard",config::Feature::Browser=>"browser"},"expectedConnection":scope_projection(&v.expected)}),
            ),
            _ => ("configuration", Value::Null),
        },
        desktop::Operation::Connection(value) => match value.as_ref() {
            desktop::connection::Operation::ConnectionsQuery(v) => (
                "connections_query",
                json!({"id":v.server,"refresh":v.refresh}),
            ),
            desktop::connection::Operation::ReinstallAgent(v) => {
                ("reinstall_agent", json!({"id":v.server}))
            }
            _ => ("connection", Value::Null),
        },
        desktop::Operation::Git(value) => match value.as_ref() {
            desktop::git::Operation::Request(v) => (
                "git_request",
                json!({"serverId":v.server,"request":v.request}),
            ),
            desktop::git::Operation::RefreshRepository(v) => (
                "git_refresh_repository",
                json!({"serverId":v.server,"repoId":v.repo_id}),
            ),
            _ => ("git", Value::Null),
        },
        _ => ("other", Value::Null),
    }
}
fn scope_projection(v: &newport_protocol::desktop::configuration::ConnectionScope) -> Value {
    use newport_protocol::desktop::configuration::{AuthMethod, Proxy};
    let settings = &v.settings;
    let proxy = settings.proxy.as_ref().map(|v| match v {
        Proxy::Command(command) => json!({"command":command}),
        Proxy::Jump(jump) => json!({"jump":jump}),
    });
    json!([
        settings.server_id,
        settings.ssh_host,
        settings.ssh_user,
        settings.ssh_port,
        match settings.auth_method {
            AuthMethod::PublicKey => "publicKey",
            AuthMethod::Password => "password",
        },
        settings.identity_file,
        settings.agent_source,
        settings.agent_key_fingerprint,
        proxy,
        v.connection_revision,
        v.backend_instance
    ])
}
pub(super) struct PageHost(pub(super) Entity<Newport>);
impl Render for PageHost {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let content = self.0.update(cx, |view, cx| {
            view.content_width = f32::from(window.viewport_size().width);
            match view.page {
                Page::Overview => view.overview(cx),
                Page::Ports => view.ports(cx),
                Page::Integration => view.integration(cx),
                Page::Projects => view.projects(window, cx),
                _ => view.screen(window, cx),
            }
        });
        div()
            .size_full()
            .flex()
            .flex_col()
            .font_family(crate::theme::UI_FONT)
            .text_size(px(crate::ui::FONT_SIZE))
            .child(content)
    }
}
/// A reading of `cores` cores with `processes` processes, for Overview layouts.
fn overview_sample(
    cores: u32,
    processes: u32,
    cpus: &[newport_protocol::monitor::metrics::Core],
) -> newport_protocol::monitor::metrics::Snapshot {
    newport_protocol::monitor::metrics::Snapshot {
        cpu: 20.,
        cores,
        memory_used: 2048,
        memory_total: 8192,
        load: [1., 0., 0.],
        cpus: cpus.to_vec(),
        processes: (0..processes)
            .map(|pid| newport_protocol::monitor::metrics::Process {
                pid,
                name: format!("worker-{pid}"),
                cpu: f64::from(pid % 100),
                user: "00000000-0000-4000-8000-000000000001".into(),
                ..Default::default()
            })
            .collect(),
        ..Default::default()
    }
}
/// The reply to a repository refresh of `repo`.
fn refreshed(
    repo: &str,
) -> Result<newport_protocol::desktop::Reply, newport_client::request_error::RequestError> {
    use newport_protocol::desktop::{self, git};
    Ok(desktop::Reply::Git(Box::new(
        git::Reply::RefreshRepository(Box::new(git::Refreshed {
            repo_id: repo.into(),
        })),
    )))
}
/// A status read's HEAD on the branch `name`.
fn typed_branch_head(name: &str) -> newport_protocol::git::Head {
    newport_protocol::git::Head {
        oid: None,
        name: Some(newport_protocol::git::Path::new(name.as_bytes())),
        detached: false,
        unborn: false,
    }
}
/// Settles `key` as a read whose data its owner keeps.
fn put(view: &mut Newport, key: &str) {
    let generation = view.resources.begin_refresh(key, false).unwrap();
    view.resources.complete_owned(key, generation, Ok(true));
}
/// Settles a read of the saved projects that name the ports' projects.
fn put_bookmarks(
    view: &mut Newport,
    projects: Vec<newport_protocol::desktop::preferences::Project>,
) {
    view.resources.invalidate("connection-projects");
    let generation = view.resources.begin("connection-projects").unwrap();
    assert!(view.complete_connection_projects(
        generation,
        Ok(crate::project_library_state::typed_fixture::projects_reply(
            projects
        ))
    ));
}
pub(super) fn frame(cx: &mut VisualTestContext) {
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));
}

#[gpui::test]
fn refreshing_an_accepted_snapshot_never_blanks_the_page(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui::component::init(cx);
        theme::install(false, cx);
    });
    struct ScreenHost(Entity<Newport>);
    impl Render for ScreenHost {
        fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let content = self.0.update(cx, |view, cx| view.screen(window, cx));
            div().size_full().flex().flex_col().child(content)
        }
    }
    let mut view = None;
    let (_, visual) = cx.add_window_view(|window, cx| {
        let entity = cx.new(|cx| typed_owner(window, cx));
        view = Some(entity.clone());
        ScreenHost(entity)
    });
    let view = view.unwrap();
    let skeleton = |visual: &mut VisualTestContext| {
        frame(visual);
        visual.debug_bounds("loading-page").is_some()
    };
    view.update(visual, |v, _| {
        v.page = Page::Integration;
        let snapshot = snapshot_of(v);
        fixture::accept(v, snapshot);
    });
    assert!(!skeleton(visual));
    // The refresh after a forward toggle or integration change.
    view.update(visual, |v, _| {
        v.resources.fence("snapshot");
        v.resources.begin_refresh("snapshot", false).unwrap();
    });
    assert!(!skeleton(visual));
    // Even a dropped row keeps the accepted snapshot on screen.
    view.update(visual, |v, _| {
        v.resources.invalidate("snapshot");
        v.resources.begin("snapshot").unwrap();
    });
    assert!(!skeleton(visual));
    // Only a first read with nothing to show uses the skeleton.
    view.update(visual, |v, _| v.snapshot = None);
    assert!(skeleton(visual));
}

#[gpui::test]
fn a_configuration_error_stays_shown_while_the_next_read_is_pending(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui::component::init(cx);
        theme::install(false, cx);
    });
    let (view, visual) = cx.add_window_view(typed_owner);
    view.update(visual, |view, _| {
        fixture::accept(view, snapshot_of(view));
        let generation = view.resources.begin_refresh("snapshot", false).unwrap();
        let (accepted, received) =
            view.complete_snapshot(generation, Err("Backend unavailable".into()));
        assert!(accepted && received.is_none());
        view.resources.begin_refresh("snapshot", false).unwrap();
        assert!(!view.initial_loading("snapshot"));
        let resource = view.resources.get("snapshot").unwrap();
        assert_eq!(resource.error.as_deref(), Some("Backend unavailable"));
        assert!(!resource.display_pending());
        assert!(view.snapshot.is_some(), "the last snapshot stays shown");
    });
}

#[gpui::test]
fn header_and_sharing_follow_global_failure_even_with_a_stale_connected_page_snapshot(
    cx: &mut TestAppContext,
) {
    use crate::connectivity::{self, Phase, ServerState, Snapshot};
    let id = uuid::Uuid::new_v4();
    let instance = uuid::Uuid::new_v4();
    cx.update(|cx| {
        gpui::component::init(cx);
        theme::install(false, cx);
    });
    let (_, visual) = cx.add_window_view(|window, cx| {
        let view = cx.new(|cx| {
            let mut view = typed_owner(window, cx);
            view.page = Page::Integration;
            view.server = id.to_string();
            let mut server = fixture::server(id, "Fixture");
            server.clipboard_enabled = true;
            server.browser_enabled = true;
            let mut snapshot = fixture::snapshot(vec![server]);
            snapshot.instance_id = instance;
            fixture::clear(&mut snapshot.runtime.connection_revisions, id);
            fixture::set(
                &mut snapshot.runtime.agent_transport,
                id,
                fixture::transport(configuration::TransportStatus::Connected),
            );
            fixture::set(
                &mut snapshot.runtime.clipboard,
                id,
                fixture::connection(configuration::ConnectionStatus::Connected),
            );
            view.snapshot = Some(snapshot);
            view
        });
        gpui::base::Root::new(view, window, cx)
    });
    frame(visual);
    visual.update(|_, cx| {
        connectivity::publish(
            Snapshot {
                instance_id: instance,
                revision: 1,
                servers: [(
                    id,
                    ServerState {
                        phase: Phase::Failed,
                        error: Some("Approval was not completed".into()),
                        ..Default::default()
                    },
                )]
                .into(),
            },
            cx,
        )
    });
    frame(visual);
    let labels = visual.update(|window, _| {
        gpui::base::test_support::snapshots(window)
            .into_iter()
            .filter_map(|node| node.label().map(str::to_owned))
            .collect::<Vec<_>>()
    });
    for label in [
        "Connection failed",
        "Sharing connection: Connection failed",
        "Clipboard access: Connection failed",
        "Browser links: Connection failed",
        "Approval was not completed",
    ] {
        assert!(
            labels.iter().any(|actual| actual == label),
            "missing {label}: {labels:?}"
        );
    }
    // Keep the page snapshot unchanged: retry and recovery must arrive through
    // the shared subscription, including clearing the previous SSH error.
    for (revision, phase) in [(2, Phase::AwaitingApproval), (3, Phase::Connected)] {
        visual.update(|_, cx| {
            connectivity::publish(
                Snapshot {
                    instance_id: instance,
                    revision,
                    servers: [(
                        id,
                        ServerState {
                            phase,
                            ..Default::default()
                        },
                    )]
                    .into(),
                },
                cx,
            )
        });
        frame(visual);
        let labels = visual.update(|window, _| {
            gpui::base::test_support::snapshots(window)
                .into_iter()
                .filter_map(|node| node.label().map(str::to_owned))
                .collect::<Vec<_>>()
        });
        assert!(labels.iter().any(|label| label == phase.label()));
        assert!(
            !labels
                .iter()
                .any(|label| label == "Approval was not completed")
        );
        assert!(
            !labels
                .iter()
                .any(|label| label.contains("Connection failed"))
        );
    }
}

pub(super) fn settle_update_check(view: &Entity<Newport>, visual: &mut VisualTestContext) {
    // Settings and placeholders run on a real clock: the updater polls every
    // few seconds, and a request still pending draws its placeholder after a
    // delay. Read Settings again, settle the offline errors and let every
    // placeholder come due, so nothing changes on its own while a caller
    // counts notifications from its own replies.
    view.update(visual, |view, _| view.settings_state.refresh());
    let deadline = Instant::now() + std::time::Duration::from_secs(5);
    loop {
        view.update(visual, |view, cx| view.tick(cx));
        frame(visual);
        if view.read_with(visual, |view, _| {
            ["startup", "cache", "update"].iter().all(|key| {
                view.settings_state
                    .resource(key)
                    .is_some_and(|resource| !resource.pending)
            }) && !view.resources.placeholder_became_due(
                view.placeholders_checked_at,
                Instant::now() + std::time::Duration::from_secs(3600),
            )
        }) {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "offline update fixture did not settle"
        );
        std::thread::yield_now();
    }
}

#[gpui::test]
fn server_dropdown_actions_preserve_edit_and_delete_guards(cx: &mut TestAppContext) {
    use gpui::component::WindowExt;
    cx.update(|cx| {
        gpui::component::init(cx);
        theme::install(false, cx);
    });
    let mut workspace = None;
    let (_, visual) = cx.add_window_view(|window, cx| {
        let entity = cx.new(|cx| typed_owner(window, cx));
        workspace = Some(entity.clone());
        gpui::base::Root::new(entity, window, cx)
    });
    let workspace = workspace.unwrap();
    visual.simulate_resize(size(px(900.), px(700.)));
    frame(visual);
    let click = |cx: &mut VisualTestContext, label: &str| {
        frame(cx);
        let at = cx.update(|window, _| {
            gpui::base::test_support::snapshots(window)
                .iter()
                .find(|e| e.label() == Some(label))
                .unwrap_or_else(|| panic!("missing {label}"))
                .bounds()
                .center()
        });
        cx.simulate_click(at, Default::default());
        frame(cx);
    };
    let open_picker = |cx: &mut VisualTestContext| {
        frame(cx);
        let at = cx.update(|window, _| {
            gpui::base::test_support::snapshots(window)
                .iter()
                .find(|e| e.role() == Some(gpui::Role::ComboBox))
                .expect("server picker")
                .bounds()
                .center()
        });
        cx.simulate_click(at, Default::default());
        frame(cx);
    };
    let controls = visual.update(|window, _| gpui::base::test_support::snapshots(window));
    assert!(
        !controls
            .iter()
            .any(|e| matches!(e.label(), Some("Edit server" | "Server actions")))
    );
    open_picker(visual);
    click(visual, "Edit server");
    assert!(workspace.read_with(visual, |v, _| v.server_form.is_some()));
    click(visual, "Cancel");
    assert!(workspace.read_with(visual, |v, _| v.server_form.is_none()));
    // Delete lives in the server's edit dialog, not in the switcher.
    open_picker(visual);
    let controls = visual.update(|window, _| gpui::base::test_support::snapshots(window));
    assert!(!controls.iter().any(|e| e.label() == Some("Delete server")));
    click(visual, "Edit server");
    click(visual, "Delete server");
    assert!(workspace.read_with(visual, |v, _| v.server_form.is_none()));
    assert!(visual.update(|window, cx| window.has_active_dialog(cx)));
    assert!(workspace.read_with(visual, |v, _| {
        v.resources
            .get("server-delete:00000000-0000-4000-8000-000000000001")
            .is_none()
    }));
    // A destructive confirmation never approves on a bare Enter.
    visual.simulate_keystrokes("enter");
    frame(visual);
    assert!(visual.update(|window, cx| window.has_active_dialog(cx)));
    assert!(workspace.read_with(visual, |v, _| {
        v.resources
            .get("server-delete:00000000-0000-4000-8000-000000000001")
            .is_none()
    }));
    click(visual, "Cancel");
    assert!(workspace.read_with(visual, |v, _| v.selected_server().is_some()));
    open_picker(visual);
    click(visual, "Add server…");
    assert!(workspace.read_with(visual, |v, _| v.server_form.is_some()));
    click(visual, "Cancel");
}

#[gpui::test]
fn sidebar_modes_preserve_terminal_and_stay_outside_caption_drag_region(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui::component::init(cx);
        theme::install(false, cx);
    });
    let mut workspace = None;
    let (_, visual) = cx.add_window_view(|window, cx| {
        let entity = cx.new(|cx| {
            let mut view = typed_owner(window, cx);
            view.page = Page::Terminal;
            view
        });
        workspace = Some(entity.clone());
        gpui::base::Root::new(entity, window, cx)
    });
    let workspace = workspace.unwrap();
    for width in [640., 1100.] {
        visual.simulate_resize(size(px(width), px(700.)));
        frame(visual);
        let terminal = workspace.read_with(visual, |view, _| {
            view.terminals["00000000-0000-4000-8000-000000000001"].clone()
        });
        for _ in 0..2 {
            for (label, compact) in [
                ("Use icons-only navigation", true),
                ("Use full-name navigation", false),
            ] {
                let controls =
                    visual.update(|window, _| gpui::base::test_support::snapshots(window));
                let toggle = controls
                    .iter()
                    .find(|e| e.label() == Some(label))
                    .unwrap()
                    .bounds();
                let drag = visual.debug_bounds("workspace-header-drag-region").unwrap();
                assert!(drag.left() >= toggle.right());
                {
                    let label = "Test connection";
                    let action = controls
                        .iter()
                        .find(|e| e.label() == Some(label))
                        .unwrap()
                        .bounds();
                    assert!(
                        drag.right() <= action.left(),
                        "drag region overlaps {label}"
                    );
                }
                visual.simulate_click(toggle.center(), Default::default());
                frame(visual);
                assert_eq!(
                    workspace.read_with(visual, |view, _| view.sidebar_layout.compact()),
                    compact
                );
                assert_eq!(
                    workspace.read_with(visual, |view, _| {
                        view.terminals["00000000-0000-4000-8000-000000000001"].clone()
                    }),
                    terminal
                );
                let sidebar = visual.debug_bounds("shell-sidebar").unwrap();
                assert!(workspace.read_with(visual, |view, _| view.sidebar_visible));
                assert_eq!(sidebar.top(), px(crate::ui::TITLEBAR_HEIGHT));
                assert!(sidebar.bottom() <= px(700.));
                if compact {
                    assert_eq!(sidebar.size.width + px(1.), px(crate::ui::ICON_RAIL_WIDTH));
                }
            }
        }
    }
}

#[gpui::test]
fn workspace_commands_use_actions_without_default_shortcuts_and_respect_modals(
    cx: &mut TestAppContext,
) {
    use gpui_kit::component::WindowExt;

    cx.update(|cx| {
        gpui::component::init(cx);
        theme::install(false, cx);
        navigation::install_workspace_shortcuts(cx);
    });
    let mut workspace = None;
    let (_, visual) = cx.add_window_view(|window, cx| {
        let entity = cx.new(|cx| {
            let view = typed_owner(window, cx);
            view.focus.focus(window, cx);
            view
        });
        workspace = Some(entity.clone());
        gpui::base::Root::new(entity, window, cx)
    });
    let workspace = workspace.unwrap();
    // Pages ask the backend for reads; nothing answers in this fixture.
    let (work, _requests) = std::sync::mpsc::sync_channel(256);
    workspace.update(visual, |view, _| view.work = work.into());
    frame(visual);
    let (primary, views) = if cfg!(target_os = "macos") {
        ("cmd", "ctrl-cmd")
    } else {
        ("ctrl", "ctrl-shift")
    };
    // Former shortcuts do nothing; the same commands remain available to menus.
    for (number, expected) in [
        (2, Page::Ports),
        (3, Page::Services),
        (4, Page::Containers),
        (5, Page::Terminal),
        (6, Page::Files),
        (7, Page::Projects),
        (8, Page::Projects),
        (9, Page::Integration),
        (1, Page::Overview),
    ] {
        let before = workspace.read_with(visual, |view, _| view.page);
        visual.simulate_keystrokes(&format!("{views}-{number}"));
        frame(visual);
        assert!(workspace.read_with(visual, |view, _| view.page) == before);
        let page = if number == 8 {
            Page::RemoteApps
        } else {
            expected
        };
        visual.update(|window, cx| {
            window.dispatch_action(Box::new(navigation::OpenView { page }), cx)
        });
        frame(visual);
        assert!(
            workspace.read_with(visual, |view, _| view.page) == expected,
            "{number}"
        );
    }
    // Other former workspace accelerators must also leave the page alone.
    visual.simulate_keystrokes(&format!("{primary}-2"));
    frame(visual);
    assert!(workspace.read_with(visual, |view, _| view.page) == Page::Overview);
    for visible in [false, true] {
        visual.simulate_keystrokes(&format!("{primary}-shift-l"));
        frame(visual);
        assert_eq!(
            workspace.read_with(visual, |view, _| view.sidebar_visible),
            !visible
        );
        visual.update(|window, cx| window.dispatch_action(Box::new(crate::ToggleSidebar), cx));
        frame(visual);
        assert_eq!(
            workspace.read_with(visual, |view, _| view.sidebar_visible),
            visible
        );
    }
    visual.simulate_keystrokes(&format!("{primary}-,"));
    frame(visual);
    assert!(workspace.read_with(visual, |view, _| view.page) == Page::Overview);
    visual.update(|window, cx| window.dispatch_action(Box::new(crate::OpenSettings), cx));
    frame(visual);
    assert!(workspace.read_with(visual, |view, _| view.page) == Page::Settings);

    visual.update(|window, cx| {
        window.open_dialog(cx, |dialog, _, _| dialog.title("Fixture modal"));
    });
    frame(visual);
    visual.update(|window, cx| {
        window.dispatch_action(Box::new(crate::ToggleSidebar), cx);
        window.dispatch_action(
            Box::new(navigation::OpenView {
                page: Page::Overview,
            }),
            cx,
        );
    });
    frame(visual);
    assert!(workspace.read_with(visual, |view, _| view.sidebar_visible));
    assert!(workspace.read_with(visual, |view, _| view.page) == Page::Settings);
    visual.update(|window, cx| window.close_dialog(cx));
    workspace.update_in(visual, |view, window, cx| view.focus.focus(window, cx));
    frame(visual);
    visual.update(|window, cx| {
        window.dispatch_action(
            Box::new(navigation::OpenView {
                page: Page::Overview,
            }),
            cx,
        )
    });
    frame(visual);
    assert!(workspace.read_with(visual, |view, _| view.page) == Page::Overview);
    visual.simulate_keystrokes(&format!("{primary}-n"));
    frame(visual);
    assert!(workspace.read_with(visual, |view, _| view.server_form.is_none()));
    visual.update(|window, cx| window.dispatch_action(Box::new(crate::NewServer), cx));
    frame(visual);
    assert!(workspace.read_with(visual, |view, _| view.server_form.is_some()));
    visual.update(|window, cx| {
        window.dispatch_action(Box::new(navigation::OpenView { page: Page::Ports }), cx)
    });
    frame(visual);
    assert!(workspace.read_with(visual, |view, _| view.page) == Page::Overview);
}

#[gpui::test]
fn integration_scrolls_to_the_bottom_in_short_windows(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::component::init(cx);
        theme::install(false, cx);
        cx.set_reduce_motion(true);
    });
    let (_, visual) = cx.add_window_view(|window, cx| {
        PageHost(cx.new(|cx| {
            let mut view = typed_owner(window, cx);
            view.page = Page::Integration;
            view
        }))
    });
    for width in [900., 420.] {
        visual.simulate_resize(size(px(width), px(360.)));
        frame(visual);
        visual.simulate_event(gpui::ScrollEvent {
            position: gpui::point(px(8.), px(100.)),
            delta: gpui::ScrollDelta::Pixels(gpui::point(px(0.), px(10000.))),
            ..Default::default()
        });
        frame(visual);
        let before = visual.debug_bounds("needle-status").unwrap();
        visual.simulate_event(gpui::ScrollEvent {
            position: gpui::point(px(8.), px(100.)),
            delta: gpui::ScrollDelta::Pixels(gpui::point(px(0.), px(-10000.))),
            ..Default::default()
        });
        frame(visual);
        let after = visual.debug_bounds("needle-status").unwrap();
        assert!(
            after.top() < before.top(),
            "content must scroll at width {width}: before={before:?}, after={after:?}, content={:?}",
            visual.debug_bounds("integrations-content")
        );
        let content = visual.debug_bounds("integrations-content").unwrap();
        assert!(
            content.bottom() <= px(360.),
            "bottom must be reachable: {content:?}"
        );
    }
}

#[gpui::test]
fn integration_exposes_connection_states_and_locks_controls_during_updates(
    cx: &mut TestAppContext,
) {
    cx.update(|cx| {
        gpui_kit::component::init(cx);
        theme::install(false, cx);
        cx.set_reduce_motion(true);
    });
    let mut view = None;
    let (_, visual) = cx.add_window_view(|window, cx| {
        let entity = cx.new(|cx| {
            let mut view = typed_owner(window, cx);
            view.page = Page::Integration;
            fixture::set(
                &mut view.snapshot_mut().runtime.agent_status,
                fixture::SERVER,
                fixture::installed(true),
            );
            view
        });
        view = Some(entity.clone());
        PageHost(entity)
    });
    let view = view.unwrap();
    let (work, requests) = std::sync::mpsc::sync_channel(8);
    view.update(visual, |view, _| view.work = work.into());
    visual.simulate_resize(size(px(900.), px(850.)));
    frame(visual);
    let nodes = |visual: &mut VisualTestContext| {
        visual.update(|window, _| gpui_kit::base::test_support::snapshots(window))
    };
    assert!(
        nodes(visual)
            .iter()
            .any(|n| n.label() == Some("Shell configuration"))
    );
    assert!(
        nodes(visual)
            .iter()
            .any(|n| n.label() == Some("More Needle actions"))
    );
    for label in [
        "Sharing",
        "Sharing connection: Connected",
        "Clipboard access: Active",
        "Browser links: Active",
    ] {
        assert!(
            !nodes(visual).iter().any(|node| node.label() == Some(label)),
            "redundant healthy state: {label}"
        );
    }
    for label in ["Terminal commands", "Needle"] {
        assert!(
            nodes(visual)
                .iter()
                .any(|n| n.label() == Some(label) && n.role() == Some(gpui::Role::Heading))
        );
        assert!(
            !nodes(visual)
                .iter()
                .any(|n| n.label() == Some(label) && n.role() == Some(gpui::Role::Button))
        );
    }
    for label in [
        "Let apps on this server read your clipboard.",
        "Open links from this server in your browser.",
        "Shell configuration",
    ] {
        assert!(
            nodes(visual).iter().any(|node| node.label() == Some(label)),
            "missing {label}"
        );
    }
    assert_eq!(
        visual.debug_bounds("integrations-content").unwrap().top(),
        px(16.)
    );
    view.update(visual, |view, cx| {
        fixture::set(
            &mut view.snapshot_mut().runtime.clipboard,
            fixture::SERVER,
            configuration::ConnectionState {
                error_message: Some("Fixture connection failed".into()),
                ..fixture::connection(configuration::ConnectionStatus::Error)
            },
        );
        cx.notify();
    });
    frame(visual);
    assert!(
        nodes(visual)
            .iter()
            .any(|node| node.label() == Some("Fixture connection failed"))
    );
    assert!(
        !nodes(visual)
            .iter()
            .any(|node| matches!(node.label(), Some("Retry" | "Try again")))
    );
    for key in [
        "integration:feature:00000000-0000-4000-8000-000000000001",
        "integration:reinstall:00000000-0000-4000-8000-000000000001",
    ] {
        let generation = view.update(visual, |view, cx| {
            let generation = view.resources.begin(key).unwrap();
            cx.notify();
            generation
        });
        frame(visual);
        {
            let (label, role) = ("More Needle actions", gpui::Role::Button);
            let found = nodes(visual);
            let control = found
                .iter()
                .find(|node| node.label() == Some(label) && node.role() == Some(role))
                .unwrap();
            visual.simulate_click(control.bounds().center(), Default::default());
            frame(visual);
            assert!(
                requests.try_recv().is_err(),
                "{label} dispatched during {key}"
            );
        }
        view.update(visual, |view, cx| {
            view.resources.complete(key, generation, Ok(()));
            cx.notify();
        });
        frame(visual);
    }
    for (label, feature) in [
        ("Clipboard access", "clipboard"),
        ("Browser links", "browser"),
    ] {
        let point = nodes(visual)
            .iter()
            .find(|node| node.label() == Some(label) && node.role() == Some(gpui::Role::Label))
            .unwrap()
            .bounds()
            .center();
        visual.simulate_click(point, Default::default());
        frame(visual);
        let request = requests.try_recv().expect("label toggles its feature");
        assert_eq!(observed(&request).0, "set_integration_enabled");
        assert_eq!(
            observed(&request).1,
            json!({"id":"00000000-0000-4000-8000-000000000001", "feature":feature, "enabled":false,
                "expectedConnection":view.read_with(visual, |v, _| scope_projection(&v.selected_connection().unwrap()))})
        );
        view.update(visual, |view, cx| {
            view.resources
                .complete(&request.key, request.generation, Ok(()));
            cx.notify();
        });
        frame(visual);
    }
    // Exercise the actual callbacks against an isolated request queue.
    {
        let (label, method, key) = (
            "Reinstall",
            "reinstall_agent",
            "integration:reinstall:00000000-0000-4000-8000-000000000001",
        );
        let more = nodes(visual)
            .iter()
            .find(|node| node.label() == Some("More Needle actions"))
            .unwrap()
            .bounds()
            .center();
        visual.simulate_click(more, Default::default());
        frame(visual);
        let point = nodes(visual)
            .iter()
            .find(|node| node.label() == Some(label))
            .unwrap()
            .bounds()
            .center();
        visual.simulate_click(point, Default::default());
        frame(visual);
        let request = requests.try_recv().expect("enabled action dispatched");
        assert_eq!(observed(&request).0, method);
        assert_eq!(request.key, key);
        assert_eq!(
            observed(&request).1["id"],
            "00000000-0000-4000-8000-000000000001"
        );
        view.update(visual, |view, cx| {
            view.resources.complete(key, request.generation, Ok(()));
            cx.notify();
        });
        frame(visual);
    }
    view.update(visual, |view, cx| {
        let server = &mut view.snapshot_mut().config.servers[0];
        server.clipboard_enabled = false;
        server.browser_enabled = false;
        cx.notify();
    });
    visual.simulate_resize(size(px(550.), px(600.)));
    frame(visual);
    assert!(
        !nodes(visual)
            .iter()
            .any(|node| node.label() == Some("Sharing connection: Off"))
    );
    assert!(
        !nodes(visual)
            .iter()
            .any(|node| matches!(node.label(), Some("Retry" | "Try again")))
    );
    let heading = nodes(visual)
        .iter()
        .find(|node| node.label() == Some("Let apps on this server read your clipboard."))
        .unwrap()
        .bounds();
    assert!(heading.top() > px(crate::controls::PAGE_PADDING));
    assert_eq!(
        visual.debug_bounds("integrations-content").unwrap().top(),
        px(crate::controls::PAGE_PADDING)
    );
    assert_eq!(heading.left(), px(crate::controls::PAGE_PADDING));
}

#[gpui::test]
fn hidden_settings_reads_do_not_redraw_other_pages_but_global_effects_still_apply(
    cx: &mut TestAppContext,
) {
    cx.update(|cx| {
        gpui_kit::component::init(cx);
        theme::install(false, cx);
    });
    let mut owner_view = None;
    let (_, visual) = cx.add_window_view(|window, cx| {
        let view = cx.new(|cx| {
            let mut view = typed_owner(window, cx);
            view.page = Page::Integration;
            view
        });
        owner_view = Some(view.clone());
        PageHost(view)
    });
    owner_view.unwrap().update(visual, |view, _| {
        use newport_client::settings::Effects;
        for page in [
            Page::Overview,
            Page::Ports,
            Page::Projects,
            Page::Services,
            Page::Containers,
            Page::Files,
            Page::Terminal,
            Page::Integration,
        ] {
            view.page = page;
            assert!(!view.apply_settings_effects(Effects {
                changed: true,
                ..Default::default()
            }));
        }
        view.page = Page::Settings;
        assert!(view.apply_settings_effects(Effects {
            changed: true,
            ..Default::default()
        }));
        view.page = Page::Integration;
        assert!(view.apply_settings_effects(Effects {
            update_ready: Some("00000000-0000-4000-8000-000000000001".into()),
            ..Default::default()
        }));
        assert!(view.notification.is_some());
        put(view, "history");
        put(view, "ports");
        assert!(view.apply_settings_effects(Effects {
            metrics_cleared: true,
            ..Default::default()
        }));
        assert!(view.resources.get("history").is_none());
        assert!(view.resources.get("ports").unwrap().has_value());
    });
}

#[gpui::test]
fn background_agent_activity_only_invalidates_its_visible_feature(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::component::init(cx);
        theme::install(false, cx);
        cx.set_reduce_motion(true);
    });
    let mut owner_view = None;
    let (_, visual) = cx.add_window_view(|window, cx| {
        let view = cx.new(|cx| typed_owner(window, cx));
        owner_view = Some(view.clone());
        PageHost(view)
    });
    let view = owner_view.unwrap();
    let (work, _requests) = std::sync::mpsc::sync_channel(128);
    let (replies, inbox) = std::sync::mpsc::channel();
    view.update(visual, |view, cx| {
        view.work = work.into();
        view.replies = inbox;
        fixture::accept(view, snapshot_of(view));
        view.tick(cx);
    });
    frame(visual);
    settle_update_check(&view, visual);
    let notices = visual.new(|cx: &mut Context<usize>| {
        cx.observe(&view, |count, _, _| *count += 1).detach();
        0
    });
    for (page, label) in Page::ALL {
        view.update(visual, |view, cx| {
            view.page = page;
            cx.notify();
        });
        frame(visual);
        let mut redraws = 0;
        for reading in 1..=3 {
            // Count only what the reading changes: the page's own timers, such
            // as placeholders and the updater's poll, redraw while settling.
            view.update(visual, |view, _| view.last_refresh = Instant::now());
            settle_update_check(&view, visual);
            notices.update(visual, |count, _| *count = 0);
            let (generation, mut snapshot) = view.update(visual, |view, _| {
                (
                    view.resources.begin_refresh("snapshot", false).unwrap(),
                    snapshot_of(view),
                )
            });
            snapshot.revision += 1;
            fixture::set(
                &mut snapshot.runtime.agent_installation,
                fixture::SERVER,
                fixture::install(configuration::InstallStage::Uploading, reading * 100, 1000),
            );
            let mut permissions = fixture::permissions(false, false);
            permissions.desired.revision = reading;
            fixture::set(
                &mut snapshot.runtime.agent_permissions,
                fixture::SERVER,
                permissions,
            );
            replies
                .send(fixture::completion(generation, snapshot))
                .unwrap();
            view.update(visual, |view, cx| view.tick(cx));
            frame(visual);
            redraws += notices.read_with(visual, |count, _| *count);
        }
        assert_eq!(
            redraws,
            if page == Page::Integration { 3 } else { 0 },
            "{label}"
        );
        assert_eq!(
            view.read_with(visual, |view, _| {
                view.runtime()
                    .and_then(|runtime| runtime.agent_installation(fixture::SERVER))
                    .map(|progress| progress.sent)
            }),
            Some(300)
        );
    }
}

#[gpui::test]
fn other_server_runtime_changes_are_retained_without_window_notifications(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::component::init(cx);
        theme::install(false, cx);
        cx.set_reduce_motion(true);
    });
    let mut owner_view = None;
    let (_, visual) = cx.add_window_view(|window, cx| {
        let view = cx.new(|cx| {
            let mut view = typed_owner(window, cx);
            view.page = Page::Integration;
            view
        });
        owner_view = Some(view.clone());
        PageHost(view)
    });
    let view = owner_view.unwrap();
    let (work, _requests) = std::sync::mpsc::sync_channel(32);
    let (replies, inbox) = std::sync::mpsc::channel();
    let mut data = view.update(visual, |view, cx| {
        view.work = work.into();
        view.replies = inbox;
        let mut snapshot = snapshot_of(view);
        let mut second = snapshot.config.servers[0].clone();
        second.id = fixture::SECOND;
        snapshot.config.servers.push(second);
        fixture::accept(view, snapshot.clone());
        view.tick(cx);
        snapshot
    });
    frame(visual);
    settle_update_check(&view, visual);
    let notices = visual.new(|cx: &mut Context<usize>| {
        cx.observe(&view, |count, _, _| *count += 1).detach();
        0
    });
    let deliver = |result: Result<configuration::Snapshot, &str>,
                   visual: &mut VisualTestContext| {
        let generation = view.update(visual, |view, _| {
            view.resources.begin_refresh("snapshot", false).unwrap()
        });
        replies
            .send(match result {
                Ok(snapshot) => fixture::completion(generation, snapshot),
                Err(error) => fixture::failure(generation, error),
            })
            .unwrap();
        view.update(visual, |view, cx| view.tick(cx));
        frame(visual);
    };
    for revision in 1..=3 {
        data.revision = revision;
        fixture::set(
            &mut data.runtime.agent_installation,
            fixture::SECOND,
            fixture::install(configuration::InstallStage::Uploading, revision * 100, 1000),
        );
        let mut permissions = fixture::permissions(false, false);
        permissions.desired.revision = revision;
        fixture::set(
            &mut data.runtime.agent_permissions,
            fixture::SECOND,
            permissions,
        );
        data.runtime.awaiting_agent = vec![fixture::SECOND];
        deliver(Ok(data.clone()), visual);
    }
    assert_eq!(
        notices.read_with(visual, |count, _| *count),
        0,
        "another server's runtime must not invalidate the selected page"
    );
    assert_eq!(
        view.read_with(visual, |view, _| view.snapshot.clone()),
        Some(data.clone()),
        "quiet snapshots still retain all server state"
    );
    fixture::set(
        &mut data.runtime.agent_permissions,
        fixture::SERVER,
        fixture::permissions(false, false),
    );
    deliver(Ok(data.clone()), visual);
    assert_eq!(
        notices.read_with(visual, |count, _| *count),
        1,
        "selected-server permission changes must remain immediate"
    );
    fixture::set(
        &mut data.runtime.agent_transport,
        fixture::SERVER,
        fixture::transport(configuration::TransportStatus::Recovering),
    );
    deliver(Ok(data.clone()), visual);
    assert_eq!(notices.read_with(visual, |count, _| *count), 2);
    deliver(Err("Snapshot transport failed"), visual);
    assert_eq!(notices.read_with(visual, |count, _| *count), 3);
    assert!(!view.read_with(visual, |view, _| view.configuration_available()));
    deliver(Ok(data), visual);
    assert_eq!(
        notices.read_with(visual, |count, _| *count),
        4,
        "recovery publishes even when the snapshot data is identical"
    );
    assert!(view.read_with(visual, |view, _| view.configuration_available()));
}

#[gpui::test]
fn identical_background_snapshots_do_not_notify_the_window(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::component::init(cx);
        theme::install(false, cx);
        cx.set_reduce_motion(true);
    });
    let mut owner_view = None;
    let (_, visual) = cx.add_window_view(|window, cx| {
        let view = cx.new(|cx| {
            let mut view = typed_owner(window, cx);
            view.page = Page::Integration;
            view
        });
        owner_view = Some(view.clone());
        PageHost(view)
    });
    let view = owner_view.unwrap();
    let (work, _requests) = std::sync::mpsc::sync_channel(16);
    let (replies, inbox) = std::sync::mpsc::channel();
    let original = view.update(visual, |view, cx| {
        view.work = work.into();
        view.replies = inbox;
        let snapshot = snapshot_of(view);
        fixture::accept(view, snapshot.clone());
        view.tick(cx);
        snapshot
    });
    frame(visual);
    settle_update_check(&view, visual);
    let notices = visual.new(|cx: &mut Context<usize>| {
        cx.observe(&view, |count, _, _| *count += 1).detach();
        0
    });
    let deliver = |data: configuration::Snapshot, visual: &mut VisualTestContext| {
        let generation = view.update(visual, |view, _| view.resources.begin("snapshot").unwrap());
        replies.send(fixture::completion(generation, data)).unwrap();
        view.update(visual, |view, cx| view.tick(cx));
        frame(visual);
    };
    for _ in 0..3 {
        deliver(original.clone(), visual);
    }
    let quiet = notices.read_with(visual, |count, _| *count);
    eprintln!("Identical snapshot notifications: {quiet} for 3 background reads");
    assert_eq!(
        quiet, 0,
        "identical shell readings must not invalidate the entire window"
    );
    let mut changed = original;
    changed.config.servers[0].name = "Renamed fixture".into();
    deliver(changed, visual);
    assert_eq!(notices.read_with(visual, |count, _| *count), 1);
    assert_eq!(
        view.read_with(visual, |view, _| view.servers()[0].name.clone()),
        "Renamed fixture"
    );
}

#[gpui::test]
fn integration_waits_for_fresh_configuration_after_failed_refresh(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::component::init(cx);
        theme::install(false, cx);
        cx.set_reduce_motion(true);
    });
    let mut owner_view = None;
    let (_, visual) = cx.add_window_view(|window, cx| {
        let view = cx.new(|cx| {
            let mut view = typed_owner(window, cx);
            view.page = Page::Integration;
            view
        });
        owner_view = Some(view.clone());
        PageHost(view)
    });
    let view = owner_view.unwrap();
    let (work, requests) = std::sync::mpsc::sync_channel(16);
    let (replies, inbox) = std::sync::mpsc::channel();
    let original = view.update(visual, |view, _| {
        view.work = work.into();
        view.replies = inbox;
        snapshot_of(view)
    });
    let deliver =
        |key: &str, generation, result: super::Outcome, visual: &mut VisualTestContext| {
            replies
                .send(super::Completion {
                    server: "00000000-0000-4000-8000-000000000001".into(),
                    key: key.into(),
                    generation,
                    result,
                })
                .unwrap();
            view.update(visual, |view, cx| view.tick(cx));
            frame(visual);
        };
    let (old_snapshot, mutation) = view.update(visual, |view, _| {
        (
            view.resources.begin("snapshot").unwrap(),
            view.resources
                .begin("integration:feature:00000000-0000-4000-8000-000000000001")
                .unwrap(),
        )
    });
    // Completion forces a snapshot newer than the preference write.
    deliver(
        "integration:feature:00000000-0000-4000-8000-000000000001",
        mutation,
        super::Outcome::Typed(Ok(newport_protocol::desktop::Reply::Configuration(
            Box::new(
                newport_protocol::desktop::configuration::Reply::SetIntegrationEnabled(Box::new(
                    newport_protocol::common::Present {},
                )),
            ),
        ))),
        visual,
    );
    let refreshed = requests.try_recv().expect("fresh configuration requested");
    assert_eq!(refreshed.key, "snapshot");
    assert_ne!(refreshed.generation, old_snapshot);
    deliver(
        "snapshot",
        old_snapshot,
        fixture::outcome(Ok(original.clone())),
        visual,
    );
    assert!(view.read_with(visual, |view, _| {
        view.integration_refresh_pending
            .contains("00000000-0000-4000-8000-000000000001")
    }));
    deliver(
        "snapshot",
        refreshed.generation,
        fixture::outcome(Err("Connection interrupted")),
        visual,
    );
    assert!(view.read_with(visual, |view, _| {
        view.integration_refresh_pending
            .contains("00000000-0000-4000-8000-000000000001")
    }));
    // A stale snapshot cannot enable access. An explicit revocation remains
    // safe and available even before a fresh snapshot arrives.
    let retry = view.update(visual, |view, cx| {
        let generation = view.resources.begin("snapshot").unwrap();
        cx.notify();
        generation
    });
    frame(visual);
    let label = visual.update(|window, _| {
        gpui_kit::base::test_support::snapshots(window)
            .into_iter()
            .find(|node| {
                node.label() == Some("Browser links") && node.role() == Some(gpui::Role::Label)
            })
            .unwrap()
            .bounds()
            .center()
    });
    visual.simulate_click(label, Default::default());
    frame(visual);
    let revoke = requests
        .try_recv()
        .expect("revocation remains available while refreshing");
    assert_eq!(observed(&revoke).1["enabled"], false);
    view.update(visual, |view, cx| {
        view.resources
            .complete(&revoke.key, revoke.generation, Ok(()));
        view.integration_command(
            "00000000-0000-4000-8000-000000000001",
            Some(("browser", true)),
            cx,
        );
    });
    assert!(
        requests.try_recv().is_err(),
        "stale preferences must not enable access"
    );
    let mut unavailable = original.clone();
    unavailable.load_error = Some("Vault unavailable".into());
    deliver("snapshot", retry, fixture::outcome(Ok(unavailable)), visual);
    assert!(view.read_with(visual, |view, _| {
        view.integration_refresh_pending
            .contains("00000000-0000-4000-8000-000000000001")
    }));
    let recovered = view.update(visual, |view, _| view.resources.begin("snapshot").unwrap());
    deliver(
        "snapshot",
        recovered,
        fixture::outcome(Ok(original)),
        visual,
    );
    assert!(view.read_with(visual, |view, _| {
        view.integration_refresh_pending.is_empty()
    }));
    visual.simulate_click(label, Default::default());
    frame(visual);
    let mutation = requests
        .try_recv()
        .expect("controls recover after fresh snapshot");
    assert_eq!(observed(&mutation).0, "set_integration_enabled");
    // A late operation belongs to its original server, even after navigation.
    view.update(visual, |view, cx| {
        let servers = &mut view.snapshot_mut().config.servers;
        let mut other = servers[0].clone();
        other.id = fixture::SECOND;
        servers.push(other);
        view.server = "00000000-0000-4000-8000-000000000002".into();
        view.resources.clear();
        view.notification = None;
        cx.notify();
    });
    deliver(
        &mutation.key,
        mutation.generation,
        super::Outcome::Typed(Err("First server failed".into())),
        visual,
    );
    assert!(view.read_with(visual, |view, _| {
        view.notification.is_none()
            && view
                .integration_refresh_pending
                .contains("00000000-0000-4000-8000-000000000001")
            && !view
                .integration_refresh_pending
                .contains("00000000-0000-4000-8000-000000000002")
    }));
    assert_eq!(observed(&requests.try_recv().unwrap()).0, "snapshot");
    visual.simulate_click(label, Default::default());
    frame(visual);
    let other = requests
        .try_recv()
        .expect("unrelated server remains usable");
    assert_eq!(
        observed(&other).1["id"],
        "00000000-0000-4000-8000-000000000002"
    );
    assert_eq!(
        other.key,
        "integration:disable:browser:00000000-0000-4000-8000-000000000002"
    );
}

#[gpui::test]
#[ignore = "manual server lookup/ownership CPU benchmark; excludes rendering and remote work"]
fn benchmark_server_lookup_and_idle_panel_ownership(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::component::init(cx);
        theme::install(false, cx);
    });
    let mut view = None;
    let (_, visual) = cx.add_window_view(|window, cx| {
        let entity = cx.new(|cx| typed_owner(window, cx));
        view = Some(entity.clone());
        PageHost(entity)
    });
    let view = view.unwrap();
    for count in [1, 100, 1000] {
        view.update(visual, |view, cx| {
            let server = view.selected_server().unwrap().clone();
            let mut servers: Vec<_> = (1..count)
                .map(|index| {
                    let mut other = server.clone();
                    other.id = uuid::Uuid::from_u128(0xf000 + index as u128);
                    other
                })
                .collect();
            servers.push(server);
            view.snapshot_mut().config.servers = servers;
            let start = Instant::now();
            for _ in 0..200 {
                std::hint::black_box(view.selected_server());
            }
            eprintln!(
                "Selected server, {count} servers: {:.3} ms/200 lookups",
                start.elapsed().as_secs_f64() * 1000.
            );
            let start = Instant::now();
            for _ in 0..200 {
                view.poll_services(cx);
                view.poll_containers(cx);
            }
            eprintln!(
                "Idle panel ownership, {count} servers: {:.3} ms/200 polls",
                start.elapsed().as_secs_f64() * 1000.
            );
        });
    }
}

#[gpui::test]
#[ignore = "manual whole-page CPU frame benchmark; isolated from backend and GPU presentation"]
fn benchmark_whole_pages(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::component::init(cx);
        theme::install(false, cx);
        cx.set_reduce_motion(true);
    });
    let mut view = None;
    let (host, visual) = cx.add_window_view(|window, cx| {
        let entity = cx.new(|cx| typed_owner(window, cx));
        view = Some(entity.clone());
        PageHost(entity)
    });
    let view = view.unwrap();
    visual.simulate_resize(size(px(1100.), px(720.)));
    for (page, count, mode) in [
        (Page::Overview, 50, "groups"),
        (Page::Overview, 5000, "groups"),
        (Page::Ports, 50, "groups"),
        (Page::Ports, 500, "groups"),
        (Page::Integration, 0, "groups"),
        (Page::Projects, 5000, "groups"),
        (Page::Ports, 500, "single"),
        (Page::Ports, 500, "table"),
    ] {
        visual.update(|window, cx| {
            view.update(cx, |view, cx| {
                view.page = page;
                match page {
                    Page::Overview => {
                        view.put_reading(overview_sample(8, count as u32, &[]));
                        view.put_history(Vec::new());
                        view.sync_overview_processes(window, cx);
                    }
                    Page::Ports => {
                        discovery_fixture::put(
                            view,
                            discovery_fixture::observation((0..count).map(|i| {
                                let group = if mode == "single" { 0 } else { i / 10 };
                                Listener::new(8000 + u16::try_from(i).unwrap())
                                    .pid(u32::try_from(group + 1).unwrap())
                                    .process("node", "node", &[], None)
                                    .project(&format!("/srv/project-{group}"))
                            })),
                        );
                        put_bookmarks(view, Vec::new());
                        view.sync_connections(window, cx);
                        view.connections_state
                            .get_mut("00000000-0000-4000-8000-000000000001")
                            .unwrap()
                            .table = mode == "table";
                    }
                    Page::Projects => {
                        use crate::project_library_state::typed_fixture::{bookmark, put_projects};
                        let projects = (0..count)
                            .map(|i| {
                                bookmark(
                                    i as u128,
                                    "00000000-0000-4000-8000-000000000001",
                                    &format!("Project {i}"),
                                    &format!("/srv/project-{i}"),
                                )
                            })
                            .collect();
                        put_projects(view, projects);
                    }
                    _ => {}
                }
            })
        });
        host.update(visual, |_, cx| cx.notify());
        frame(visual);
        let start = Instant::now();
        for _ in 0..10 {
            host.update(visual, |_, cx| cx.notify());
            frame(visual);
        }
        let name = page.title();
        eprintln!(
            "Whole page {name} size={count} mode={mode}: {:.3} ms/CPU frame",
            start.elapsed().as_secs_f64() * 1000. / 10.
        );
    }
}

#[gpui::test]
#[ignore = "manual populated Overview CPU benchmark; excludes native GPU presentation"]
fn benchmark_overview_core_history_frames(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::component::init(cx);
        theme::install(false, cx);
        cx.set_reduce_motion(true);
    });
    let mut view = None;
    let (host, visual) = cx.add_window_view(|window, cx| {
        let entity = cx.new(|cx| typed_owner(window, cx));
        view = Some(entity.clone());
        PageHost(entity)
    });
    let view = view.unwrap();
    visual.simulate_resize(size(px(1100.), px(720.)));
    for cores in [32, 256] {
        visual.update(|window, cx| {
            view.update(cx, |view, cx| {
                view.page = Page::Overview;
                let cpus: Vec<_> = (0..cores)
                    .map(|id| newport_protocol::monitor::metrics::Core {
                        id,
                        usage: f64::from(id % 100),
                        history: (0..60).map(|i| f64::from((id + i) % 100)).collect(),
                    })
                    .collect();
                view.put_reading(newport_protocol::monitor::metrics::Snapshot {
                    process_count: 5000,
                    ..overview_sample(cores, 5000, &cpus)
                });
                view.put_history(
                    (0..300)
                        .map(|i| crate::overview::cpu_sample(i * 3000, (i % 100) as f64))
                        .collect(),
                );
                view.sync_overview_processes(window, cx);
            })
        });
        host.update(visual, |_, cx| cx.notify());
        frame(visual);
        for (position, delta) in [("top", 100000.), ("bottom", -100000.)] {
            visual.simulate_event(gpui::ScrollEvent {
                position: gpui::point(px(1000.), px(600.)),
                delta: gpui::ScrollDelta::Pixels(gpui::point(px(0.), px(delta))),
                ..Default::default()
            });
            frame(visual);
            if position == "top" {
                assert!(visual.debug_bounds("cpu-core-0").is_some());
            } else {
                assert!(visual.debug_bounds("cpu-core-0").is_none());
                assert!(visual.debug_bounds("overview-process-1999").is_some());
            }
            let start = Instant::now();
            for _ in 0..10 {
                host.update(visual, |_, cx| cx.notify());
                frame(visual);
            }
            eprintln!(
                "Populated Overview cores={cores} at={position}: {:.3} ms/CPU frame",
                start.elapsed().as_secs_f64() * 1000. / 10.
            );
        }
    }
}

pub(super) fn connections_fixture(
    cx: &mut TestAppContext,
    table: bool,
) -> (Entity<Newport>, &mut VisualTestContext) {
    cx.update(|cx| {
        gpui_kit::component::init(cx);
        theme::install(false, cx);
        cx.set_reduce_motion(true);
    });
    let mut view = None;
    let (_, visual) = cx.add_window_view(|window, cx| {
        let entity = cx.new(|cx| {
            let mut view = typed_owner(window, cx);
            view.page = Page::Ports;
            discovery_fixture::put(
                &mut view,
                discovery_fixture::observation((0..500).map(|i| {
                    Listener::new(8000 + i)
                        .pid(1)
                        .process("node", "node", &[], None)
                        .project("/srv/project-0")
                })),
            );
            view.sync_connections(window, cx);
            // Settle the first project read; left pending, its placeholder
            // delay would redraw the page partway through a test.
            view.resources.invalidate("connection-projects");
            put_bookmarks(&mut view, Vec::new());
            view.connections_state
                .get_mut("00000000-0000-4000-8000-000000000001")
                .unwrap()
                .table = table;
            view
        });
        view = Some(entity.clone());
        let host = cx.new(|cx| {
            cx.observe(&entity, |_, _, cx| cx.notify()).detach();
            PageHost(entity)
        });
        gpui_kit::base::Root::new(host, window, cx)
    });
    visual.simulate_resize(size(px(1100.), px(720.)));
    frame(visual);
    (view.unwrap(), visual)
}
fn row_selector(index: usize) -> &'static str {
    listener_selector(8000 + u16::try_from(index).unwrap(), 1)
}
pub(super) fn listener_selector(port: u16, pid: u32) -> &'static str {
    static SELECTORS: std::sync::OnceLock<std::collections::HashMap<(u16, u32), String>> =
        std::sync::OnceLock::new();
    SELECTORS
        .get_or_init(|| {
            [(8000, 1), (8001, 1), (8060, 61), (8499, 1)]
                .into_iter()
                .map(|identity @ (port, pid)| {
                    (
                        identity,
                        format!(
                            "connection-row-{}",
                            Listener::new(port).pid(pid).port().key()
                        ),
                    )
                })
                .collect()
        })
        .get(&(port, pid))
        .expect("fixture row selector")
        .as_str()
}

#[gpui::test]
fn connection_metadata_completion_repaints_only_changed_project_names(cx: &mut TestAppContext) {
    use gpui_kit::base::test_support::snapshots;
    let (view, visual) = connections_fixture(cx, false);
    let (work, requests) = std::sync::mpsc::sync_channel(32);
    let (replies, incoming) = std::sync::mpsc::channel();
    use crate::project_library_state::typed_fixture::{
        TYPED_SERVER, bookmark, projects_completion,
    };
    let original = vec![bookmark(
        1,
        TYPED_SERVER,
        "Original project",
        "/srv/project-0",
    )];
    view.update(visual, |view, cx| {
        view.work = work.into();
        view.replies = incoming;
        put_bookmarks(view, original.clone());
        cx.notify();
    });
    frame(visual);
    settle_update_check(&view, visual);
    let notices = visual.new(|cx: &mut Context<usize>| {
        cx.observe(&view, |count, _, _| *count += 1).detach();
        0
    });
    let rendered_names = |visual: &mut VisualTestContext| {
        visual.update(|window, _| {
            snapshots(window)
                .iter()
                .filter_map(|node| node.label().map(str::to_owned))
                .collect::<Vec<_>>()
        })
    };
    assert!(
        rendered_names(visual)
            .iter()
            .any(|label| label.contains("Original project"))
    );
    for (data, expected_notices) in [
        (original.clone(), 0),
        (
            vec![bookmark(
                1,
                TYPED_SERVER,
                "Renamed project",
                "/srv/project-0",
            )],
            1,
        ),
    ] {
        requests.try_iter().for_each(drop);
        let generation = view.update(visual, |view, _| {
            view.project_bookmarks_changed("00000000-0000-4000-8000-000000000001");
            view.poll_connections(Instant::now());
            requests
                .try_iter()
                .find(|work| work.key == "connection-projects")
                .unwrap()
                .generation
        });
        replies
            .send(projects_completion("connection-projects", generation, data))
            .unwrap();
        view.update(visual, |view, cx| view.tick(cx));
        frame(visual);
        assert_eq!(
            notices.read_with(visual, |count, _| *count),
            expected_notices
        );
    }
    let labels = rendered_names(visual);
    assert!(labels.iter().any(|label| label.contains("Renamed project")));
    assert!(
        !labels
            .iter()
            .any(|label| label.contains("Original project"))
    );
}

#[gpui::test]
#[ignore = "manual Connections CPU frame benchmark; excludes native GPU presentation"]
fn benchmark_connections_cached_frames(cx: &mut TestAppContext) {
    let (view, visual) = connections_fixture(cx, false);
    let mut measurements = Vec::new();
    for count in [100, 1000, newport_protocol::monitor::ports::MAX_LISTENERS] {
        for table in [false, true] {
            view.update(visual, |view, cx| {
                discovery_fixture::put(
                    view,
                    discovery_fixture::observation((0..count).map(|i| {
                        let group = i / 20;
                        let name = format!("service-{}", count - group);
                        Listener::new(8000 + u16::try_from(i).unwrap())
                            .pid(u32::try_from(group + 1).unwrap())
                            .process(&name, &name, &[], None)
                            .project(&format!("/srv/project-{group}"))
                    })),
                );
                let state = view
                    .connections_state
                    .get_mut("00000000-0000-4000-8000-000000000001")
                    .unwrap();
                state.table = table;
                state.sort = 1;
                cx.notify();
            });
            frame(visual);
            let start = Instant::now();
            for _ in 0..10 {
                view.update(visual, |_, cx| cx.notify());
                frame(visual);
            }
            measurements.push(format!(
                "Connections rows={count} table={table}: {:.3} ms/CPU frame",
                start.elapsed().as_secs_f64() * 1000. / 10.
            ));
        }
    }
    eprintln!("{}", measurements.join("\n"));
}

#[gpui::test]
fn cached_connection_observations_and_poll_admission_do_not_notify_the_window(
    cx: &mut TestAppContext,
) {
    let (view, visual) = connections_fixture(cx, false);
    let (work, requests) = std::sync::mpsc::sync_channel(16);
    let (replies, inbox) = std::sync::mpsc::channel();
    let observation = view.update(visual, |view, cx| {
        view.work = work.into();
        view.replies = inbox;
        let observation = crate::connections::DiscoveryCompletion {
            scope: view.selected_connection().unwrap(),
            reading: view.connections_state[&view.server]
                .reading
                .clone()
                .unwrap(),
        };
        let generation = view.resources.begin_refresh("ports", false).unwrap();
        assert!(view.complete_connections_reading(
            &view.server.clone(),
            generation,
            observation.clone()
        ));
        view.tick(cx);
        observation
    });
    frame(visual);
    settle_update_check(&view, visual);
    let notices = visual.new(|cx: &mut Context<usize>| {
        cx.observe(&view, |count, _, _| *count += 1).detach();
        0
    });
    for _ in 0..3 {
        let generation = view.update(visual, |view, _| {
            // Drive the real scheduled polling branch without changing the data.
            view.last_refresh = Instant::now();
            assert!(!view.poll_connections(Instant::now() + std::time::Duration::from_secs(2)));
            requests
                .try_recv()
                .expect("scheduled cached observation")
                .generation
        });
        frame(visual);
        assert_eq!(
            notices.read_with(visual, |count, _| *count),
            0,
            "background poll admission is not a visual loading state"
        );
        replies
            .send(super::Completion {
                server: "00000000-0000-4000-8000-000000000001".into(),
                key: "ports".into(),
                generation,
                result: super::Outcome::Discovery(observation.clone()),
            })
            .unwrap();
        view.update(visual, |view, cx| view.tick(cx));
        frame(visual);
    }
    assert_eq!(notices.read_with(visual, |count, _| *count), 0);
}

#[gpui::test]
fn ports_table_preserves_readable_service_cells_when_resized(cx: &mut TestAppContext) {
    static SERVICE_SELECTOR: std::sync::LazyLock<String> = std::sync::LazyLock::new(|| {
        format!(
            "connection-identity-{}",
            Listener::new(8000).pid(1).port().key()
        )
    });
    let (_, visual) = connections_fixture(cx, true);
    for width in [680., 820., 900., 920., 1100., 440.] {
        visual.simulate_resize(size(px(width), px(720.)));
        frame(visual);
        let service = visual.debug_bounds(&SERVICE_SELECTOR).unwrap();
        assert!(
            service.size.width >= px(180.),
            "service column collapsed at {width}: {service:?}"
        );
        visual.update(|window, _| {
            let nodes = gpui_kit::base::test_support::snapshots(window);
            let details = nodes
                .iter()
                .find(|node| {
                    node.label().is_some_and(|label| {
                        label.starts_with("Connection details for port 8000: ")
                    })
                })
                .unwrap();
            assert!(details.bounds().left() >= service.left());
            assert!(details.bounds().right() <= service.right() + px(1.));
            let sort = nodes
                .iter()
                .find(|node| node.label() == Some("Sort by Service"))
                .unwrap()
                .bounds();
            assert!(sort.right() <= service.right() + px(1.));
            let actions = nodes
                .iter()
                .find(|node| node.label() == Some("More actions for port 8000"))
                .unwrap()
                .bounds();
            assert!(actions.left() >= service.right());
            assert!(actions.right() <= px(width));
        });
    }
}

#[gpui::test]
fn connections_toolbar_wraps_and_loading_replaces_the_list_in_place(cx: &mut TestAppContext) {
    let (view, visual) = connections_fixture(cx, false);
    for width in [1100., 440.] {
        visual.simulate_resize(size(px(width), px(720.)));
        frame(visual);
        let toolbar = visual.debug_bounds("connections-toolbar").unwrap();
        let footer = visual.debug_bounds("connections-footer").unwrap();
        let first = visual.debug_bounds(row_selector(0)).unwrap();
        assert!(first.top() >= toolbar.bottom());
        assert!(first.bottom() < footer.top());
        if width == 1100. {
            assert_eq!(
                toolbar.size.height,
                px(crate::controls::TOOLBAR_HEIGHT),
                "wide toolbar should use one control row: {toolbar:?}"
            );
        }
        visual.update(|window, _| {
            let nodes = gpui_kit::base::test_support::snapshots(window);
            for name in [
                "All (500)",
                "Saved (0)",
                "Projects view",
                "Ports view",
                "Refresh ports",
                "New forward",
            ] {
                let node = nodes
                    .iter()
                    .find(|n| n.label() == Some(name))
                    .unwrap_or_else(|| panic!("missing {name}"));
                assert!(
                    node.bounds().left() >= toolbar.left()
                        && node.bounds().right() <= toolbar.right() + px(1.),
                    "{name} overflows at {width}"
                );
                assert!(
                    node.bounds().top() >= toolbar.top()
                        && node.bounds().bottom() <= toolbar.bottom(),
                    "{name} leaves the toolbar at {width}"
                );
            }
        });
    }
    let before = visual.debug_bounds("connections-toolbar").unwrap();
    view.update(visual, |view, cx| {
        view.resources.invalidate("ports");
        view.resources.begin("ports");
        cx.notify();
    });
    frame(visual);
    assert_eq!(visual.debug_bounds("connections-toolbar").unwrap(), before);
    let skeleton = visual.debug_bounds("connections-loading").unwrap();
    let footer = visual.debug_bounds("connections-footer").unwrap();
    assert!(skeleton.top() >= before.bottom() && skeleton.bottom() <= footer.top());
    assert!(skeleton.size.height > px(400.));
    assert!(
        visual.debug_bounds(row_selector(0)).is_none(),
        "loading replaces rows rather than adding another block"
    );
}
fn scroll_connections(view: &Entity<Newport>, cx: &mut VisualTestContext, last: bool) {
    view.update(cx, |view, cx| {
        let viewport = view.connections_state["00000000-0000-4000-8000-000000000001"]
            .viewport
            .borrow();
        if last {
            viewport.list.scroll_to_end();
        } else {
            viewport.list.scroll_to(gpui::ListOffset {
                item_ix: 0,
                offset_in_item: px(0.),
            });
        }
        cx.notify();
    });
    frame(cx);
}
#[gpui::test]
fn connections_virtualizes_single_projects_and_tables_and_preserves_focus(cx: &mut TestAppContext) {
    for table in [false, true] {
        let (view, visual) = connections_fixture(cx, table);
        assert!(visual.debug_bounds(row_selector(0)).is_some());
        assert!(visual.debug_bounds(row_selector(499)).is_none());
        let mounted = visual.update(|window, _| {
            gpui_kit::base::test_support::snapshots(window)
                .iter()
                .filter(|node| {
                    node.label()
                        .is_some_and(|label| label.starts_with("Forward port "))
                })
                .count()
        });
        assert!(
            mounted > 0 && mounted < 40,
            "mounted {mounted} rows, table={table}"
        );
        scroll_connections(&view, visual, true);
        assert!(
            visual.debug_bounds(row_selector(499)).is_some(),
            "last row unreachable, table={table}"
        );
        assert!(visual.debug_bounds(row_selector(0)).is_none());
        scroll_connections(&view, visual, false);
        let focus = view.read_with(visual, |view, _| {
            view.connections_state["00000000-0000-4000-8000-000000000001"]
                .viewport
                .borrow()
                .focus[0]
                .clone()
        });
        visual.update(|window, cx| focus.focus(window, cx));
        scroll_connections(&view, visual, true);
        assert!(
            visual.update(|window, _| focus.is_focused(window)),
            "scroll dropped keyboard focus"
        );
        assert!(
            visual.debug_bounds(row_selector(0)).is_some(),
            "focused block was unmounted"
        );
        let input = view.read_with(visual, |view, _| {
            view.connections_state["00000000-0000-4000-8000-000000000001"]
                .input
                .clone()
                .unwrap()
        });
        visual.update(|window, cx| {
            use gpui::Focusable;
            input.read(cx).focus_handle(cx).focus(window, cx);
        });
        visual.simulate_resize(size(px(550.), px(500.)));
        frame(visual);
        scroll_connections(&view, visual, true);
        let viewport = visual.debug_bounds("connections-scroll").unwrap();
        let row = visual.debug_bounds(row_selector(499)).unwrap();
        assert!(row.right() <= viewport.right() && row.left() >= viewport.left());
    }
}
#[gpui::test]
fn connections_collapse_search_and_refresh_update_virtual_ranges(cx: &mut TestAppContext) {
    let (view, visual) = connections_fixture(cx, false);
    // Project headings are shared group headings: "name, count".
    let click = |name: &str, visual: &mut VisualTestContext| {
        let prefix = format!("{name}, ");
        let point = visual.update(|window, _| {
            gpui_kit::base::test_support::snapshots(window)
                .into_iter()
                .find(|node| node.label().is_some_and(|label| label.starts_with(&prefix)))
                .unwrap_or_else(|| panic!("missing {name} heading"))
                .bounds()
                .center()
        });
        visual.simulate_click(point, Default::default());
        frame(visual);
    };
    click("project-0", visual);
    assert!(view.read_with(visual, |view, _| {
        view.connections_state["00000000-0000-4000-8000-000000000001"]
            .collapsed
            .contains("/srv/project-0")
    }));
    assert!(visual.debug_bounds(row_selector(0)).is_none());
    assert_eq!(
        view.read_with(visual, |view, _| {
            view.connections_state["00000000-0000-4000-8000-000000000001"]
                .viewport
                .borrow()
                .list
                .item_count()
        }),
        1
    );
    click("project-0", visual);
    assert!(visual.debug_bounds(row_selector(0)).is_some());
    let input = view.read_with(visual, |view, _| {
        view.connections_state["00000000-0000-4000-8000-000000000001"]
            .input
            .clone()
            .unwrap()
    });
    visual.update(|window, cx| input.update(cx, |input, cx| input.set_value("8499", window, cx)));
    frame(visual);
    assert!(visual.debug_bounds(row_selector(499)).is_some());
    assert!(visual.debug_bounds(row_selector(0)).is_none());
    view.update(visual, |view, cx| {
        discovery_fixture::put(view, discovery_fixture::observation([]));
        cx.notify();
    });
    frame(visual);
    assert!(visual.debug_bounds(row_selector(499)).is_none());
}

#[gpui::test]
fn connections_worktree_disclosure_survives_refresh_and_search(cx: &mut TestAppContext) {
    let (view, visual) = connections_fixture(cx, false);
    let ports = |branch: &str| {
        discovery_fixture::observation((0..63).map(|i| {
            let (pid, path, branch) = if i < 60 {
                (1, "/srv/project-0/feature-a", branch)
            } else {
                (61, "/srv/project-0/feature-b", "feature-b")
            };
            Listener::new(8000 + i)
                .pid(pid)
                .process("node", "node", &[], None)
                .project("/srv/project-0")
                .checkout(path, Some(branch), Some(false))
        }))
    };
    view.update(visual, |view, cx| {
        discovery_fixture::put(view, ports("feature-a"));
        cx.notify();
    });
    frame(visual);
    let point = visual.update(|window, _| {
        gpui_kit::base::test_support::snapshots(window)
            .into_iter()
            .find(|node| node.label() == Some("feature-a, 60"))
            .unwrap()
            .bounds()
            .center()
    });
    visual.simulate_click(point, Default::default());
    frame(visual);
    assert!(visual.debug_bounds(row_selector(0)).is_none());
    assert!(visual.debug_bounds(listener_selector(8060, 61)).is_some());
    assert_eq!(
        view.read_with(visual, |view, _| {
            view.connections_state["00000000-0000-4000-8000-000000000001"]
                .viewport
                .borrow()
                .list
                .item_count()
        }),
        1
    );
    // A branch rename changes the label, never the disclosure identity.
    view.update(visual, |view, cx| {
        discovery_fixture::put(view, ports("renamed-a"));
        cx.notify();
    });
    frame(visual);
    assert!(visual.debug_bounds(row_selector(0)).is_none());
    let input = view.read_with(visual, |view, _| {
        view.connections_state["00000000-0000-4000-8000-000000000001"]
            .input
            .clone()
            .unwrap()
    });
    visual.update(|window, cx| input.update(cx, |input, cx| input.set_value("8000", window, cx)));
    frame(visual);
    assert!(
        visual.debug_bounds(row_selector(0)).is_some(),
        "search reveals matches in a folded checkout"
    );
    visual.update(|window, cx| input.update(cx, |input, cx| input.set_value("", window, cx)));
    frame(visual);
    assert!(
        visual.debug_bounds(row_selector(0)).is_none(),
        "clearing search restores disclosure choice"
    );
    let point = visual.update(|window, _| {
        gpui_kit::base::test_support::snapshots(window)
            .into_iter()
            .find(|node| node.label() == Some("renamed-a, 60"))
            .unwrap()
            .bounds()
            .center()
    });
    visual.simulate_click(point, Default::default());
    frame(visual);
    assert!(visual.debug_bounds(row_selector(0)).is_some());
    visual.simulate_resize(size(px(550.), px(500.)));
    frame(visual);
    let viewport = visual.debug_bounds("connections-scroll").unwrap();
    let row = visual.debug_bounds(row_selector(0)).unwrap();
    assert!(row.right() <= viewport.right() && row.left() >= viewport.left());
}

#[gpui::test]
fn finished_setup_does_not_reenter_agent_setup_when_metrics_fail(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::component::init(cx);
        theme::install(false, cx);
    });
    let (_, visual) = cx.add_window_view(|window, cx| {
        let view = cx.new(|cx| {
            let mut view = typed_owner(window, cx);
            view.page = Page::Overview;
            fixture::set(
                &mut view.snapshot_mut().runtime.agent_installation,
                fixture::SERVER,
                configuration::InstallProgress {
                    error: Some("Old installation failure".into()),
                    ..fixture::install(configuration::InstallStage::Error, 0, 0)
                },
            );
            let generation = view.resources.begin("overview").unwrap();
            view.resources
                .complete("overview", generation, Err("SSH connection refused".into()));
            view
        });
        PageHost(view)
    });
    for dark in [false, true] {
        visual.update(|_, cx| theme::install(dark, cx));
        visual.simulate_resize(size(px(900.), px(850.)));
        frame(visual);
        assert!(visual.debug_bounds("overview-scroll").is_some());
        assert!(visual.debug_bounds("agent-setup").is_none());
        visual.update(|window, _| {
            let nodes = gpui_kit::base::test_support::snapshots(window);
            assert!(!nodes.iter().any(|n| n.label() == Some("Step 4 of 4")));
        });
    }
}

#[gpui::test]
fn connections_query_navigation_retains_cache_and_fences_connection_changes(
    cx: &mut TestAppContext,
) {
    let (view, visual) = connections_fixture(cx, false);
    let (work, requests) = std::sync::mpsc::sync_channel(64);
    let (late, observation) = view.update(visual, |v, _| {
        v.work = work.into();
        let observation = crate::connections::DiscoveryCompletion {
            scope: v.selected_connection().unwrap(),
            reading: v.connections_state[&v.server].reading.clone().unwrap(),
        };
        let late = v.resources.begin("ports").unwrap();
        (late, observation)
    });
    // Render and inspect each destination separately; batching navigation in a
    // single owner update would never exercise the intermediate screen.
    assert!(visual.debug_bounds(row_selector(0)).is_some());
    view.update(visual, |v, cx| v.navigate(Page::Integration, cx));
    frame(visual);
    assert!(visual.debug_bounds(row_selector(0)).is_none());
    view.update(visual, |v, cx| v.navigate(Page::Ports, cx));
    frame(visual);
    assert!(visual.debug_bounds(row_selector(0)).is_some());
    view.update(visual, |v, _| {
        assert_eq!(
            v.connections_state[&v.server]
                .reading
                .as_ref()
                .unwrap()
                .view()
                .unwrap()
                .members()[0]
                .endpoint
                .port,
            8000
        );
        assert!(!v.complete_connections_reading(&v.server.clone(), late, observation));
    });
    let queued: Vec<_> = requests.try_iter().collect();
    assert!(
        queued
            .iter()
            .any(|request| observed(request).0 == "connections_query"
                && observed(request).1["refresh"] == false)
    );
    view.update(visual, |v, cx| v.navigate(Page::Integration, cx));
    frame(visual);
    assert!(visual.debug_bounds(row_selector(0)).is_none());
    view.update(visual, |v, cx| {
        v.snapshot_mut().config.servers[0].ssh_host = "two".into();
        v.navigate(Page::Ports, cx);
        assert!(
            v.connections_state[&v.server].reading.is_none(),
            "old connection data must not be restored"
        );
    });
    frame(visual);
    assert!(visual.debug_bounds(row_selector(0)).is_none());
    assert!(visual.debug_bounds("connections-loading").is_some());
}

#[gpui::test]
fn connections_filter_notifies_only_its_visible_server_page(cx: &mut TestAppContext) {
    let (view, visual) = connections_fixture(cx, false);
    let input = view.read_with(visual, |v, _| {
        v.connections_state["00000000-0000-4000-8000-000000000001"]
            .input
            .clone()
            .unwrap()
    });
    let notices = visual.new(|cx: &mut Context<usize>| {
        cx.observe(&view, |count, _, _| *count += 1).detach();
        0
    });
    for (page, server, visible, expected) in [
        (
            Page::Integration,
            "00000000-0000-4000-8000-000000000001",
            true,
            0,
        ),
        (Page::Ports, "other", true, 0),
        (
            Page::Ports,
            "00000000-0000-4000-8000-000000000001",
            false,
            0,
        ),
        (Page::Ports, "00000000-0000-4000-8000-000000000001", true, 1),
    ] {
        view.update(visual, |v, _| {
            v.page = page;
            v.server = server.into();
            v.window_visible = visible;
        });
        visual.update(|window, cx| {
            input.update(cx, |input, cx| {
                input.insert(format!("{}-{server}-{visible}", page.title()), window, cx);
            })
        });
        frame(visual);
        assert_eq!(notices.read_with(visual, |count, _| *count), expected);
    }
}

#[gpui::test]
fn git_status_observations_are_quiet_but_reconcile_changed_and_older_pages(
    cx: &mut TestAppContext,
) {
    cx.update(|cx| {
        gpui::component::init(cx);
        theme::install(false, cx);
    });
    let mut owner_view = None;
    let (_, visual) = cx.add_window_view(|window, cx| {
        let view = cx.new(|cx| {
            let mut view = typed_owner(window, cx);
            view.page = Page::Projects;
            view.window_visible = true;
            let mut repo = crate::git_fixture::read_only_repository("repo");
            repo.head = typed_branch_head("main");
            view.git.repo = Some(repo);
            let mut status = crate::git_fixture::status_page("one", vec![], None);
            status.metadata.head = typed_branch_head("main");
            view.git.page = Some(crate::git_listing::Listing::Status(Box::new(
                status.clone(),
            )));
            let generation = view
                .resources
                .begin_refresh("git-sync-status", false)
                .unwrap();
            view.resources.complete_git(
                "git-sync-status",
                generation,
                Ok(newport_protocol::git::Reply::Status(Box::new(status))),
            );
            view
        });
        owner_view = Some(view.clone());
        PageHost(view)
    });
    let view = owner_view.unwrap();
    let (work, requests) = std::sync::mpsc::sync_channel(64);
    let (replies, inbox) = std::sync::mpsc::channel();
    view.update(visual, |view, _| {
        view.work = work.into();
        view.replies = inbox;
    });
    settle_update_check(&view, visual);
    let notices = visual.new(|cx: &mut Context<usize>| {
        cx.observe(&view, |count, _, _| *count += 1).detach();
        0
    });
    let observe = |data: crate::git_listing::StatusPage, visual: &mut VisualTestContext| {
        let generation = view.update(visual, |view, _| {
            view.last_refresh = Instant::now();
            let generation = view
                .resources
                .begin_refresh("git-sync-status", false)
                .unwrap();
            assert!(
                !view
                    .resources
                    .get("git-sync-status")
                    .unwrap()
                    .display_pending()
            );
            generation
        });
        replies
            .send(super::Completion {
                server: "00000000-0000-4000-8000-000000000001".into(),
                key: "git-sync-status".into(),
                generation,
                result: super::Outcome::Git(Ok(newport_protocol::git::Reply::Status(Box::new(
                    data,
                )))),
            })
            .unwrap();
        view.update(visual, |view, cx| view.tick(cx));
        frame(visual);
    };
    let original = view.read_with(visual, |view, _| view.git.status_page().unwrap().clone());
    for _ in 0..3 {
        observe(original.clone(), visual);
    }
    assert_eq!(notices.read_with(visual, |count, _| *count), 0);
    assert!(!requests.try_iter().any(|request| request.key == "git-list"));

    // The timer itself must not invalidate a cached control or window.
    view.update(visual, |view, _| {
        assert!(!view.poll_git_status_at(Instant::now() + std::time::Duration::from_secs(16)));
    });
    let request = requests.try_recv().expect("due repository observation");
    assert_eq!(request.key, "git-sync-status");
    assert_eq!(
        observed(&request).1["serverId"],
        "00000000-0000-4000-8000-000000000001"
    );
    assert_eq!(observed(&request).1["request"]["params"]["repoId"], "repo");
    replies
        .send(super::Completion {
            server: "00000000-0000-4000-8000-000000000001".into(),
            key: request.key,
            generation: request.generation,
            result: super::Outcome::Git(Ok(newport_protocol::git::Reply::Status(Box::new(
                original.clone(),
            )))),
        })
        .unwrap();
    view.update(visual, |view, cx| view.tick(cx));
    frame(visual);
    assert_eq!(notices.read_with(visual, |count, _| *count), 0);

    let mut changed = original.clone();
    changed.snapshot = "two".into();
    changed.metadata.head = typed_branch_head("other");
    observe(changed.clone(), visual);
    assert_eq!(notices.read_with(visual, |count, _| *count), 1);
    assert_eq!(
        view.read_with(visual, |view, _| view
            .git
            .repo
            .as_ref()
            .and_then(|repo| repo.head.name.clone())),
        Some(newport_protocol::git::Path::new(b"other"))
    );
    assert_eq!(requests.try_recv().unwrap().key, "git-list");
    // Repeated observations must not abandon an already-running first-page read.
    observe(changed.clone(), visual);
    assert_eq!(notices.read_with(visual, |count, _| *count), 1);
    assert!(!requests.try_iter().any(|request| request.key == "git-list"));
    // A stale displayed page still needs reconciliation even if the observed
    // resource itself did not change (for example after restoring a tab).
    view.update(visual, |view, _| view.resources.invalidate("git-list"));
    observe(changed.clone(), visual);
    assert_eq!(notices.read_with(visual, |count, _| *count), 2);
    assert_eq!(requests.try_recv().unwrap().key, "git-list");

    // Approval/reconnection may succeed without changing the repository.
    // A failed list must still recover from an identical successful poll.
    view.update(visual, |view, _| {
        view.git.page = Some(crate::git_listing::Listing::Status(Box::new(
            changed.clone(),
        )));
        view.resources.invalidate("git-list");
        let generation = view.resources.begin("git-list").unwrap();
        view.resources.complete(
            "git-list",
            generation,
            Err("SSH agent unavailable: early eof".into()),
        );
    });
    observe(changed.clone(), visual);
    assert_eq!(requests.try_recv().unwrap().key, "git-list");
    observe(changed, visual);
    assert!(
        !requests.try_iter().any(|request| request.key == "git-list"),
        "recovery joins the pending read"
    );
}

#[gpui::test]
fn successful_connection_test_refreshes_only_the_selected_servers_page(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui::component::init(cx);
        theme::install(false, cx);
    });
    let mut view = None;
    let (_, visual) = cx.add_window_view(|window, cx| {
        let entity = cx.new(|cx| typed_owner(window, cx));
        view = Some(entity.clone());
        PageHost(entity)
    });
    let view = view.unwrap();
    for (server, success) in [
        ("other", true),
        ("00000000-0000-4000-8000-000000000001", false),
        ("00000000-0000-4000-8000-000000000001", true),
    ] {
        let (work, requests) = std::sync::mpsc::sync_channel(64);
        let (replies, inbox) = std::sync::mpsc::channel();
        view.update(visual, |view, cx| {
            view.work = work.into();
            view.replies = inbox;
            view.page = Page::Projects;
            view.selection = Some(crate::project_library_state::typed_fixture::bookmark(
                1,
                "00000000-0000-4000-8000-000000000001",
                "Project",
                "/project",
            ));
            view.git.repo = Some(crate::git_fixture::read_only_repository("repo"));
            view.last_refresh = Instant::now();
            let key = format!("server-test:{server}");
            let generation = view.resources.begin(&key).unwrap();
            replies
                .send(super::Completion {
                    server: server.into(),
                    key,
                    generation,
                    result: super::Outcome::Typed(if success {
                        Ok(newport_protocol::desktop::Reply::Connection(Box::new(
                            newport_protocol::desktop::connection::Reply::TestConnection(Box::new(
                                newport_protocol::common::Present {},
                            )),
                        )))
                    } else {
                        Err("SSH agent unavailable".to_owned().into())
                    }),
                })
                .unwrap();
            view.tick(cx);
        });
        let refresh = requests
            .try_iter()
            .find(|request| request.key == "git-refresh");
        assert_eq!(
            refresh.is_some(),
            server == "00000000-0000-4000-8000-000000000001" && success
        );
        if let Some(refresh) = refresh {
            assert_eq!(
                observed(&refresh).1,
                json!({"serverId":"00000000-0000-4000-8000-000000000001", "repoId":"repo"})
            );
        }
    }
}

#[gpui::test]
fn git_refresh_waits_for_cache_invalidation_and_cannot_reload_a_replacement_checkout(
    cx: &mut TestAppContext,
) {
    cx.update(|cx| {
        gpui_kit::component::init(cx);
        theme::install(false, cx);
    });
    let mut view = None;
    let (_, visual) = cx.add_window_view(|window, cx| {
        let entity = cx.new(|cx| typed_owner(window, cx));
        view = Some(entity.clone());
        PageHost(entity)
    });
    let (work, requests) = std::sync::mpsc::sync_channel(64);
    view.unwrap().update(visual, |v, _| {
        v.work = work.into();
        v.page = Page::Projects;
        v.git.repo = Some(crate::git_fixture::read_only_repository("first"));
        v.git.tab = 0;
        let existing = crate::git_fixture::status_page(
            "existing",
            vec![crate::git_fixture::status_entry("existing", "existing")],
            None,
        );
        let generation = v.resources.begin_refresh("git-list", false).unwrap();
        v.resources.complete_git(
            "git-list",
            generation,
            Ok(newport_protocol::git::Reply::Status(Box::new(
                existing.clone(),
            ))),
        );
        v.git_refresh();
        assert!(
            v.git_mutation_blocks_write(),
            "reviewed writes wait for refreshed facts"
        );
        let barrier = requests.try_recv().unwrap();
        assert_eq!(observed(&barrier).0, "git_refresh_repository");
        assert_eq!(observed(&barrier).1["repoId"], "first");
        assert!(requests.try_recv().is_err(), "no read before invalidation");
        assert_eq!(v.git_status("git-list"), Some(&existing));
        assert!(v.complete_repository_refresh(
            &barrier.key,
            barrier.generation,
            refreshed("first")
        ));
        v.git_completed(&barrier.key);
        let reads: Vec<_> = requests.try_iter().collect();
        assert!(reads.iter().any(
            |r| r.key == "git-list" && observed(r).1["request"]["params"]["repoId"] == "first"
        ));

        v.git_refresh();
        let late = requests.try_recv().unwrap();
        v.git.repo = Some(crate::git_fixture::read_only_repository(
            "00000000-0000-4000-8000-000000000002",
        ));
        assert!(v.complete_repository_refresh(&late.key, late.generation, refreshed("first")));
        v.git_completed(&late.key);
        assert!(
            requests.try_recv().is_err(),
            "old refresh must not reload new checkout"
        );
    });
}

#[gpui::test]
fn git_index_refresh_keeps_the_view_and_only_reloads_current_status_and_diff(
    cx: &mut TestAppContext,
) {
    cx.update(|cx| {
        gpui_kit::component::init(cx);
        theme::install(false, cx);
    });
    let (view, visual) = cx.add_window_view(typed_owner);
    let (work, requests) = std::sync::mpsc::sync_channel(64);
    view.update(visual, |v, _| {
        v.work = work.into();
        v.page = Page::Projects;
        v.git.repo = Some(crate::git_fixture::read_only_repository("repo"));
        let entry = crate::git_fixture::status_entry("old", "a");
        let status = crate::git_fixture::status_page("before", vec![entry.clone()], None);
        let page = Some(crate::git_listing::Listing::Status(Box::new(
            status.clone(),
        )));
        let entry = Some(crate::git_listing::Entry::Change(entry));
        let diff = Some(crate::git_fixture::diff_page(
            "old-diff",
            crate::git_fixture::working_metadata("before", "old"),
            vec![],
            None,
        ));
        v.git.page = page.clone();
        v.git.selected = entry.clone();
        v.git.diff = diff.clone();
        let generation = v.resources.begin_refresh("git-list", false).unwrap();
        v.resources.complete_git(
            "git-list",
            generation,
            Ok(newport_protocol::git::Reply::Status(Box::new(
                status.clone(),
            ))),
        );
        let generation = v.resources.begin_refresh("git-diff", false).unwrap();
        v.resources.complete_git(
            "git-diff",
            generation,
            Ok(newport_protocol::git::Reply::DiffPage(Box::new(
                diff.clone().unwrap(),
            ))),
        );
        let old_read = v.resources.begin("git-list").unwrap();
        v.git_refresh_index();
        assert!(v.git_mutation_blocks_write());
        assert_eq!(v.git.page, page);
        assert_eq!(v.git.selected, entry);
        assert_eq!(v.git.diff, diff);
        assert!(!v.resources.complete_git(
            "git-list",
            old_read,
            Ok(newport_protocol::git::Reply::Status(Box::new(
                status.clone()
            )))
        ));
        let barrier = requests.try_recv().unwrap();
        assert_eq!(observed(&barrier).0, "git_refresh_repository");
        assert!(requests.try_recv().is_err());
        assert!(v.complete_repository_refresh(&barrier.key, barrier.generation, refreshed("repo")));
        v.git_completed(&barrier.key);
        let status = requests.try_recv().unwrap();
        assert_eq!(observed(&status).1["request"]["method"], "repo.status");
        assert!(requests.try_recv().is_err());
        assert_eq!(v.git.page, page);
        assert_eq!(v.git.diff, diff);
        let mut staged = crate::git_fixture::status_entry("staged", "a");
        staged.staged = true;
        staged.unstaged = false;
        v.resources.complete_git(
            &status.key,
            status.generation,
            Ok(newport_protocol::git::Reply::Status(Box::new(
                crate::git_fixture::status_page("after", vec![staged.clone()], None),
            ))),
        );
        v.git_completed(&status.key);
        assert!(!v.git_mutation_blocks_write());
        assert_eq!(v.git.entry(), Some(&staged));
        assert!(v.git.diff_staged);
        let reads: Vec<_> = requests.try_iter().collect();
        assert_eq!(reads.len(), 1);
        assert_eq!(observed(&reads[0]).1["request"]["method"], "repo.diff_page");
        assert_eq!(
            observed(&reads[0]).1["request"]["params"]["snapshot"],
            "after"
        );
        assert_eq!(
            observed(&reads[0]).1["request"]["params"]["side"],
            "head_to_index"
        );

        let visible = v.git.page.clone();
        v.git_refresh_index();
        let failed = requests.try_recv().unwrap();
        v.resources
            .complete(&failed.key, failed.generation, Err("Offline".into()));
        v.git_completed(&failed.key);
        assert_eq!(v.git.page, visible);
        assert_eq!(v.git.error.as_deref(), Some("Offline"));
        assert!(!v.git_can_change_index(false));
        assert!(requests.try_recv().is_err());
    });
}

#[gpui::test]
fn overview_navigation_retains_readings_and_rejects_detached_refresh(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::component::init(cx);
        theme::install(false, cx);
    });
    let mut owner_view = None;
    let (_, visual) = cx.add_window_view(|window, cx| {
        let view = cx.new(|cx| typed_owner(window, cx));
        owner_view = Some(view.clone());
        PageHost(view)
    });
    let view = owner_view.unwrap();
    let (work, _requests) = std::sync::mpsc::sync_channel(64);
    view.update(visual, |v, cx| {
        v.work = work.into();
        v.page = Page::Overview;
        let cpu = |cpu| newport_protocol::monitor::metrics::Snapshot {
            cpu,
            ..Default::default()
        };
        let late = |cpu| {
            Ok(newport_protocol::desktop::Reply::Metrics(Box::new(
                newport_protocol::desktop::metrics::Reply::Collect(Box::new(
                    crate::overview::reading(newport_protocol::monitor::metrics::Snapshot {
                        cpu,
                        ..Default::default()
                    }),
                )),
            )))
        };
        v.resources.invalidate("overview");
        v.resources.invalidate("history");
        v.put_reading(cpu(42.));
        v.put_history(vec![crate::overview::cpu_sample(123, 1.)]);
        let stale = v.resources.begin("overview").unwrap();
        v.navigate(Page::Integration, cx);
        assert!(v.resources.get("overview").is_none());
        assert!(!v.complete_metrics("overview", stale, late(99.)));
        v.navigate(Page::Overview, cx);
        assert_eq!(v.overview_reading().unwrap().sample.cpu, 42.);
        assert_eq!(v.overview_history()[0].at, 123);
        assert!(v.resources.get("overview").unwrap().pending);
        assert!(!v.complete_metrics("overview", stale, late(99.)));
        v.navigate(Page::Integration, cx);
        v.reset(); // Connection changes must invalidate the retained data.
        assert!(v.overview_state.cached_reads.is_none());
        v.navigate(Page::Overview, cx);
        assert!(v.overview_reading().is_none());
    });
}

#[gpui::test]
fn overview_loading_retry_keeps_stale_warning_until_success(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui::component::init(cx);
        theme::install(false, cx);
    });
    let (view, visual) = cx.add_window_view(typed_owner);
    let (work, _requests) = std::sync::mpsc::sync_channel(64);
    view.update(visual, |view, _| {
        view.work = work.into();
        view.page = Page::Overview;
        view.resources.invalidate("overview");
        view.resources.invalidate("history");
        view.put_reading(newport_protocol::monitor::metrics::Snapshot {
            cpu: 7.,
            ..Default::default()
        });
        view.put_history(vec![crate::overview::cpu_sample(1, 7.)]);
        for key in ["overview", "history"] {
            let failed = view.resources.begin(key).unwrap();
            view.resources.complete(key, failed, Err("Offline".into()));
        }
        view.refresh();
        for key in ["overview", "history"] {
            let resource = view.resources.get(key).unwrap();
            assert!(resource.pending);
            assert_eq!(resource.error.as_deref(), Some("Offline"));
            assert!(resource.has_value());
        }
        assert_eq!(view.overview_reading().unwrap().sample.cpu, 7.);
        assert_eq!(view.overview_history().len(), 1);
    });
}

#[gpui::test]
fn projects_loading_routes_to_library_with_toolbar(cx: &mut TestAppContext) {
    struct Routed(Entity<Newport>);
    impl Render for Routed {
        fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            self.0.update(cx, |view, cx| view.screen(window, cx))
        }
    }
    cx.update(|cx| {
        gpui::component::init(cx);
        theme::install(false, cx);
    });
    let (_, visual) = cx.add_window_view(|window, cx| {
        Routed(cx.new(|cx| {
            let mut view = typed_owner(window, cx);
            view.page = Page::Projects;
            view.resources.begin("projects");
            view
        }))
    });
    visual.simulate_resize(size(px(1000.), px(700.)));
    frame(visual);
    assert!(visual.debug_bounds("project-library-loading").is_some());
}

#[gpui::test]
fn empty_project_library_offers_the_add_choices_in_place(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui::component::init(cx);
        theme::install(false, cx);
    });
    let mut owner_view = None;
    let (_, visual) = cx.add_window_view(|window, cx| {
        let view = cx.new(|cx| {
            let mut view = typed_owner(window, cx);
            view.page = Page::Projects;
            crate::project_library_state::typed_fixture::put_projects(&mut view, Vec::new());
            view
        });
        owner_view = Some(view.clone());
        PageHost(view)
    });
    let view = owner_view.unwrap();
    let labels = |visual: &mut VisualTestContext| {
        visual.update(|window, _| {
            gpui_kit::base::test_support::snapshots(window)
                .iter()
                .filter_map(|n| n.label().map(str::to_owned))
                .collect::<Vec<_>>()
        })
    };
    visual.simulate_resize(size(px(1000.), px(700.)));
    frame(visual);
    assert!(visual.debug_bounds("projects-empty").is_some());
    let shown = labels(visual);
    for label in [
        "Add a repository",
        "Clone a repository",
        "Create a repository",
    ] {
        assert!(shown.iter().any(|l| l == label), "missing {label}");
    }
    assert!(
        !shown.iter().any(|l| l == "Find a project"),
        "nothing to filter in an empty library"
    );
    let (add, clone) = (
        visual.debug_bounds("projects-empty-add").unwrap(),
        visual.debug_bounds("projects-empty-clone").unwrap(),
    );
    assert_eq!(add.top(), clone.top(), "cards share a row when wide");
    visual.simulate_resize(size(px(520.), px(700.)));
    frame(visual);
    let (add, clone) = (
        visual.debug_bounds("projects-empty-add").unwrap(),
        visual.debug_bounds("projects-empty-clone").unwrap(),
    );
    assert!(clone.top() > add.top(), "cards stack when narrow");
    visual.simulate_click(add.center(), Default::default());
    frame(visual);
    assert!(view.read_with(visual, |view, _| view.project_add.is_some()));
}
