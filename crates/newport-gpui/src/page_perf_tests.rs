//! Offline whole-page CPU fixtures. No main-loop timer, startup request, or save hook.
use super::{
    Newport, Page, appearance, file_preview, git, navigation, overview, settings, sidebar_layout,
    theme, ui_state, window_state, workspace_state,
};
use gpui_kit::{
    self as gpui, AppContext, Context, Entity, IntoElement, Render, TestAppContext,
    VisualTestContext, Window, div, prelude::*, px, size,
};
use newport_client::{backend, resources::Resources};
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
        needle_uninstall_confirmations: Default::default(),
        integration_shell: cx.new(|cx| crate::integration::ShellCode::new(window, cx)),
        integration_scroll: gpui::ScrollHandle::new(),
        server_form: None,
        modal_open: false,
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
        snapshot: Value::Null,
        resources: Resources::default(),
        work: work.into(),
        replies,
        last_refresh: Instant::now(),
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
        file_preview_image: None,
        remote_apps: None,
        remote_apps_visit: 0,
        remote_apps_away: false,
        remote_apps_returned: false,
        remote_apps_mutations: Default::default(),
        remote_apps_mutating: Default::default(),
        remote_apps_recheck: None,
        remote_apps_record_tried: None,
        remote_apps_refresh: false,
        file_preview_viewport: file_preview::Viewport::default(),
        file_preview_pdf: None,
        file_preview_pdf_text: None,
        file_transfer: None,
        file_chooser: None,
        file_project_connection: None,
        selection: None,
        project_add: None,
        project_edit: None,
        project_create: None,
        project_creation_save_owner: None,
        project_folder: None,
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

    this.server = "fixture".into();
    this.snapshot = json!({"config":{"servers":[{"id":"fixture","name":"Fixture","setupComplete":true,"agentInstallApproved":true,"sshHost":"localhost","sshUser":"fixture","clipboardEnabled":true,"browserEnabled":true}],"tunnels":[]},"runtime":{"clipboard":{"fixture":{"status":"connected"}}}});
    this.sync_project_library(window, cx);
    this
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
fn put(view: &mut Newport, key: &str, value: Value) {
    let generation = view.resources.begin_refresh(key, false).unwrap();
    view.resources.complete(key, generation, Ok(value));
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
        let entity = cx.new(|cx| owner(window, cx));
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
        let snapshot = v.snapshot.clone();
        put(v, "snapshot", snapshot);
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
    view.update(visual, |v, _| v.snapshot = Value::Null);
    assert!(skeleton(visual));
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
            let mut view = owner(window, cx);
            view.page = Page::Integration;
            view.server = id.to_string();
            view.snapshot["instanceId"] = json!(instance);
            view.snapshot["config"]["servers"][0]["id"] = json!(id);
            view.snapshot["runtime"]["health"][id.to_string()] = json!("reachable");
            view.snapshot["runtime"]["clipboard"][id.to_string()] = json!({"status":"connected"});
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
    // The shell's initial update check is an independent asynchronous effect.
    // Settle the offline error before measuring notifications from our replies.
    let deadline = Instant::now() + std::time::Duration::from_secs(5);
    loop {
        view.update(visual, |view, cx| view.tick(cx));
        frame(visual);
        if view.read_with(visual, |view, _| {
            let keys: &[&str] = if view.page == Page::Settings {
                &["startup", "cache", "update"]
            } else {
                &["update"]
            };
            keys.iter().all(|key| {
                view.settings_state
                    .resource(key)
                    .is_some_and(|resource| !resource.pending)
            })
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
        let entity = cx.new(|cx| owner(window, cx));
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
        v.resources.get("server-delete:fixture").is_none()
    }));
    // A destructive confirmation never approves on a bare Enter.
    visual.simulate_keystrokes("enter");
    frame(visual);
    assert!(visual.update(|window, cx| window.has_active_dialog(cx)));
    assert!(workspace.read_with(visual, |v, _| {
        v.resources.get("server-delete:fixture").is_none()
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
            let mut view = owner(window, cx);
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
        let terminal = workspace.read_with(visual, |view, _| view.terminals["fixture"].clone());
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
                    workspace.read_with(visual, |view, _| view.terminals["fixture"].clone()),
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
fn workspace_shortcuts_follow_the_host_platform_and_respect_modals(cx: &mut TestAppContext) {
    use gpui_kit::component::WindowExt;

    cx.update(|cx| {
        gpui::component::init(cx);
        theme::install(false, cx);
        navigation::install_workspace_shortcuts(cx);
    });
    let mut workspace = None;
    let (_, visual) = cx.add_window_view(|window, cx| {
        let entity = cx.new(|cx| {
            let view = owner(window, cx);
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
    // Views by number, in sidebar order; 8 is Remote apps, absent here.
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
        visual.simulate_keystrokes(&format!("{views}-{number}"));
        frame(visual);
        assert!(
            workspace.read_with(visual, |view, _| view.page) == expected,
            "{number}"
        );
    }
    // Numbers alone select tabs; Overview has none, so nothing changes.
    visual.simulate_keystrokes(&format!("{primary}-2"));
    frame(visual);
    assert!(workspace.read_with(visual, |view, _| view.page) == Page::Overview);
    for visible in [false, true] {
        visual.simulate_keystrokes(&format!("{primary}-shift-l"));
        frame(visual);
        assert_eq!(
            workspace.read_with(visual, |view, _| view.sidebar_visible),
            visible
        );
    }
    visual.simulate_keystrokes(&format!("{primary}-,"));
    frame(visual);
    assert!(workspace.read_with(visual, |view, _| view.page) == Page::Settings);

    visual.update(|window, cx| {
        window.open_dialog(cx, |dialog, _, _| dialog.title("Fixture modal"));
    });
    frame(visual);
    visual.simulate_keystrokes(&format!("{primary}-shift-l {views}-1"));
    frame(visual);
    assert!(workspace.read_with(visual, |view, _| view.sidebar_visible));
    assert!(workspace.read_with(visual, |view, _| view.page) == Page::Settings);
    visual.update(|window, cx| window.close_dialog(cx));
    workspace.update_in(visual, |view, window, cx| view.focus.focus(window, cx));
    frame(visual);
    visual.simulate_keystrokes(&format!("{views}-1"));
    frame(visual);
    assert!(workspace.read_with(visual, |view, _| view.page) == Page::Overview);
    visual.simulate_keystrokes(&format!("{primary}-n"));
    frame(visual);
    assert!(workspace.read_with(visual, |view, _| view.server_form.is_some()));
    visual.simulate_keystrokes(&format!("{views}-2"));
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
            let mut view = owner(window, cx);
            view.page = Page::Integration;
            view
        }))
    });
    for width in [900., 420.] {
        visual.simulate_resize(size(px(width), px(360.)));
        frame(visual);
        visual.simulate_event(gpui::ScrollWheelEvent {
            position: gpui::point(px(8.), px(100.)),
            delta: gpui::ScrollDelta::Pixels(gpui::point(px(0.), px(10000.))),
            ..Default::default()
        });
        frame(visual);
        let before = visual.debug_bounds("needle-status").unwrap();
        visual.simulate_event(gpui::ScrollWheelEvent {
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
            let mut view = owner(window, cx);
            view.page = Page::Integration;
            view.snapshot["runtime"]["agentStatus"]["fixture"] = json!({"installed":true});
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
            .any(|n| n.label() == Some("Reinstall Needle"))
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
    for label in ["Terminal commands", "Needle agent"] {
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
        view.snapshot["runtime"]["clipboard"]["fixture"] =
            json!({"status":"error","errorMessage":"Fixture connection failed"});
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
        "integration:feature:fixture",
        "integration:reinstall:fixture",
    ] {
        let generation = view.update(visual, |view, cx| {
            let generation = view.resources.begin(key).unwrap();
            cx.notify();
            generation
        });
        frame(visual);
        {
            let (label, role) = ("Reinstall Needle", gpui::Role::Button);
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
            view.resources.complete(key, generation, Ok(Value::Null));
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
        assert_eq!(request.method, "set_integration_enabled");
        assert_eq!(
            request.args,
            json!({"id":"fixture", "feature":feature, "enabled":false,
                "expectedConnection":view.read_with(visual, |v, _| crate::terminal_scope::connection(v.selected_server().unwrap(), &v.snapshot))})
        );
        view.update(visual, |view, cx| {
            view.resources
                .complete(&request.key, request.generation, Ok(Value::Null));
            cx.notify();
        });
        frame(visual);
    }
    // Exercise the actual callbacks against an isolated request queue.
    {
        let (label, method, key) = (
            "Reinstall Needle",
            "reinstall_agent",
            "integration:reinstall:fixture",
        );
        let point = nodes(visual)
            .iter()
            .find(|node| node.label() == Some(label))
            .unwrap()
            .bounds()
            .center();
        visual.simulate_click(point, Default::default());
        frame(visual);
        let request = requests.try_recv().expect("enabled action dispatched");
        assert_eq!(request.method, method);
        assert_eq!(request.key, key);
        assert_eq!(request.args["id"], "fixture");
        view.update(visual, |view, cx| {
            view.resources
                .complete(key, request.generation, Ok(Value::Null));
            cx.notify();
        });
        frame(visual);
    }
    view.update(visual, |view, cx| {
        view.snapshot["config"]["servers"][0]["clipboardEnabled"] = json!(false);
        view.snapshot["config"]["servers"][0]["browserEnabled"] = json!(false);
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
            let mut view = owner(window, cx);
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
            update_ready: Some("fixture".into()),
            ..Default::default()
        }));
        assert!(view.notification.is_some());
        put(view, "history", json!([1, 2, 3]));
        put(view, "page", json!({"unrelated":true}));
        assert!(view.apply_settings_effects(Effects {
            metrics_cleared: true,
            ..Default::default()
        }));
        assert!(view.resources.get("history").is_none());
        assert_eq!(
            view.resources.get("page").unwrap().value,
            json!({"unrelated":true})
        );
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
        let view = cx.new(|cx| owner(window, cx));
        owner_view = Some(view.clone());
        PageHost(view)
    });
    let view = owner_view.unwrap();
    let (work, _requests) = std::sync::mpsc::sync_channel(128);
    let (replies, inbox) = std::sync::mpsc::channel();
    view.update(visual, |view, cx| {
        view.work = work.into();
        view.replies = inbox;
        put(view, "snapshot", view.snapshot.clone());
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
        settle_update_check(&view, visual);
        notices.update(visual, |count, _| *count = 0);
        for reading in 1..=3 {
            let (generation, mut snapshot) = view.update(visual, |view, _| {
                view.last_refresh = Instant::now();
                (
                    view.resources.begin_refresh("snapshot", false).unwrap(),
                    view.snapshot.clone(),
                )
            });
            snapshot["revision"] = json!(snapshot["revision"].as_u64().unwrap_or(0) + 1);
            snapshot["runtime"]["agentInstallation"]["fixture"] =
                json!({"stage":"uploading", "sent":reading*100, "total":1000});
            snapshot["runtime"]["agentPermissions"]["fixture"] = json!({"revision":reading});
            replies
                .send(super::Completion {
                    pdf: None,
                    pdf_text: None,
                    image: None,
                    server: "fixture".into(),
                    key: "snapshot".into(),
                    generation,
                    result: Ok(snapshot),
                })
                .unwrap();
            view.update(visual, |view, cx| view.tick(cx));
            frame(visual);
        }
        assert_eq!(
            notices.read_with(visual, |count, _| *count),
            if page == Page::Integration { 3 } else { 0 },
            "{label}"
        );
        assert_eq!(
            view.read_with(visual, |view, _| {
                view.snapshot["runtime"]["agentInstallation"]["fixture"]["sent"].clone()
            }),
            json!(300)
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
            let mut view = owner(window, cx);
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
        let mut second = view.snapshot["config"]["servers"][0].clone();
        second["id"] = json!("second");
        view.snapshot["config"]["servers"]
            .as_array_mut()
            .unwrap()
            .push(second);
        let snapshot = view.snapshot.clone();
        put(view, "snapshot", snapshot.clone());
        view.tick(cx);
        snapshot
    });
    frame(visual);
    settle_update_check(&view, visual);
    let notices = visual.new(|cx: &mut Context<usize>| {
        cx.observe(&view, |count, _, _| *count += 1).detach();
        0
    });
    let deliver = |result, visual: &mut VisualTestContext| {
        let generation = view.update(visual, |view, _| {
            view.resources.begin_refresh("snapshot", false).unwrap()
        });
        replies
            .send(super::Completion {
                pdf: None,
                pdf_text: None,
                image: None,
                server: "fixture".into(),
                key: "snapshot".into(),
                generation,
                result,
            })
            .unwrap();
        view.update(visual, |view, cx| view.tick(cx));
        frame(visual);
    };
    for revision in 1..=3 {
        data["revision"] = json!(revision);
        data["runtime"]["agentInstallation"]["second"] =
            json!({"stage":"uploading", "sentBytes":revision*100});
        data["runtime"]["agentPermissions"]["second"] = json!({"revision":revision});
        data["runtime"]["awaitingAgent"] = json!(["second"]);
        deliver(Ok(data.clone()), visual);
    }
    assert_eq!(
        notices.read_with(visual, |count, _| *count),
        0,
        "another server's runtime must not invalidate the selected page"
    );
    assert_eq!(
        view.read_with(visual, |view, _| view.snapshot.clone()),
        data,
        "quiet snapshots still retain all server state"
    );
    data["runtime"]["agentPermissions"]["fixture"] = json!({"effective":{"browser":false}});
    deliver(Ok(data.clone()), visual);
    assert_eq!(
        notices.read_with(visual, |count, _| *count),
        1,
        "selected-server permission changes must remain immediate"
    );
    data["runtime"]["health"]["fixture"] = json!("error");
    deliver(Ok(data.clone()), visual);
    assert_eq!(notices.read_with(visual, |count, _| *count), 2);
    deliver(Err("Snapshot transport failed".into()), visual);
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
            let mut view = owner(window, cx);
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
        let snapshot = view.snapshot.clone();
        put(view, "snapshot", snapshot.clone());
        view.tick(cx);
        snapshot
    });
    frame(visual);
    settle_update_check(&view, visual);
    let notices = visual.new(|cx: &mut Context<usize>| {
        cx.observe(&view, |count, _, _| *count += 1).detach();
        0
    });
    let deliver = |data: Value, visual: &mut VisualTestContext| {
        let generation = view.update(visual, |view, _| view.resources.begin("snapshot").unwrap());
        replies
            .send(super::Completion {
                pdf: None,
                pdf_text: None,
                image: None,
                server: "fixture".into(),
                key: "snapshot".into(),
                generation,
                result: Ok(data),
            })
            .unwrap();
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
    changed["config"]["servers"][0]["name"] = json!("Renamed fixture");
    deliver(changed, visual);
    assert_eq!(notices.read_with(visual, |count, _| *count), 1);
    assert_eq!(
        view.read_with(visual, |view, _| {
            view.snapshot["config"]["servers"][0]["name"].clone()
        }),
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
            let mut view = owner(window, cx);
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
        view.snapshot.clone()
    });
    let deliver =
        |key: &str, generation, result: Result<Value, String>, visual: &mut VisualTestContext| {
            replies
                .send(super::Completion {
                    pdf: None,
                    pdf_text: None,
                    image: None,
                    server: "fixture".into(),
                    key: key.into(),
                    generation,
                    result: result.map_err(Into::into),
                })
                .unwrap();
            view.update(visual, |view, cx| view.tick(cx));
            frame(visual);
        };
    let (old_snapshot, mutation) = view.update(visual, |view, _| {
        (
            view.resources.begin("snapshot").unwrap(),
            view.resources.begin("integration:feature:fixture").unwrap(),
        )
    });
    // Completion forces a snapshot newer than the preference write.
    deliver(
        "integration:feature:fixture",
        mutation,
        Ok(Value::Null),
        visual,
    );
    let refreshed = requests.try_recv().expect("fresh configuration requested");
    assert_eq!(refreshed.key, "snapshot");
    assert_ne!(refreshed.generation, old_snapshot);
    deliver("snapshot", old_snapshot, Ok(original.clone()), visual);
    assert!(view.read_with(visual, |view, _| {
        view.integration_refresh_pending.contains("fixture")
    }));
    deliver(
        "snapshot",
        refreshed.generation,
        Err("Connection interrupted".into()),
        visual,
    );
    assert!(view.read_with(visual, |view, _| {
        view.integration_refresh_pending.contains("fixture")
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
    assert_eq!(revoke.args["enabled"], false);
    view.update(visual, |view, cx| {
        view.resources
            .complete(&revoke.key, revoke.generation, Ok(Value::Null));
        view.integration_command("fixture", Some(("browser", true)), cx);
    });
    assert!(
        requests.try_recv().is_err(),
        "stale preferences must not enable access"
    );
    let mut unavailable = original.clone();
    unavailable["loadError"] = json!("Vault unavailable");
    deliver("snapshot", retry, Ok(unavailable), visual);
    assert!(view.read_with(visual, |view, _| {
        view.integration_refresh_pending.contains("fixture")
    }));
    let recovered = view.update(visual, |view, _| view.resources.begin("snapshot").unwrap());
    deliver("snapshot", recovered, Ok(original), visual);
    assert!(view.read_with(visual, |view, _| {
        view.integration_refresh_pending.is_empty()
    }));
    visual.simulate_click(label, Default::default());
    frame(visual);
    let mutation = requests
        .try_recv()
        .expect("controls recover after fresh snapshot");
    assert_eq!(mutation.method, "set_integration_enabled");
    // A late operation belongs to its original server, even after navigation.
    view.update(visual, |view, cx| {
        let mut other = view.snapshot["config"]["servers"][0].clone();
        other["id"] = json!("second");
        view.snapshot["config"]["servers"]
            .as_array_mut()
            .unwrap()
            .push(other);
        view.server = "second".into();
        view.resources.clear();
        view.notification = None;
        cx.notify();
    });
    deliver(
        &mutation.key,
        mutation.generation,
        Err("First server failed".into()),
        visual,
    );
    assert!(view.read_with(visual, |view, _| view.notification.is_none()
        && view.integration_refresh_pending.contains("fixture")
        && !view.integration_refresh_pending.contains("second")));
    assert_eq!(requests.try_recv().unwrap().method, "snapshot");
    visual.simulate_click(label, Default::default());
    frame(visual);
    let other = requests
        .try_recv()
        .expect("unrelated server remains usable");
    assert_eq!(other.args["id"], "second");
    assert_eq!(other.key, "integration:disable:browser:second");
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
        let entity = cx.new(|cx| owner(window, cx));
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
                    other["id"] = json!(format!("other-{index}"));
                    other
                })
                .collect();
            servers.push(server);
            view.snapshot["config"]["servers"] = json!(servers);
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
        let entity = cx.new(|cx| owner(window, cx));
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
        visual.update(|window, cx| view.update(cx, |view, cx| {
            view.page = page;
            match page {
                Page::Overview => {
                    put(view,"page",json!({"cpu":20,"cores":8,"memoryUsed":2048,"memoryTotal":8192,"load":[1.0],"network":[],"disks":[],"cpus":[],"processes":(0..count).map(|i|json!({"pid":i,"name":format!("worker-{i}"),"cpu":i%100,"user":"fixture"})).collect::<Vec<_>>()}));
                    put(view,"history",json!([]));
                    view.sync_overview_processes(window,cx);
                }
                Page::Ports => {
                    put(view,"page",Value::Array((0..count).map(|i|json!({"port":8000+i,"pid":i+1,"address":"127.0.0.1","processName":"node","projectRoot":format!("/srv/project-{}",if mode == "single" {0} else {i/10})})).collect()));
                    put(view,"connection-projects",json!([]));
                    view.sync_connections(window,cx);
                    view.connections_state.get_mut("fixture").unwrap().table = mode == "table";
                }
                Page::Projects => {
                    put(view,"page",Value::Array((0..count).map(|i|json!({"id":format!("project-{i}"),"serverId":"fixture","name":format!("Project {i}"),"path":{"display":format!("/srv/project-{i}")}})).collect()));
                }
                _ => {}
            }
        }));
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
        let entity = cx.new(|cx| owner(window, cx));
        view = Some(entity.clone());
        PageHost(entity)
    });
    let view = view.unwrap();
    visual.simulate_resize(size(px(1100.), px(720.)));
    for cores in [32, 256] {
        visual.update(|window, cx| view.update(cx, |view, cx| {
            view.page = Page::Overview;
            put(view, "page", json!({
                "cpu":20,"cores":cores,"memoryUsed":2048,"memoryTotal":8192,"load":[1.0],
                "network":[],"disks":[],"processCount":5000,
                "cpus":(0..cores).map(|id|json!({"id":id,"usage":id%100,"history":(0..60).map(|i|(id+i)%100).collect::<Vec<_>>()})).collect::<Vec<_>>(),
                "processes":(0..5000).map(|pid|json!({"pid":pid,"name":format!("worker-{pid}"),"cpu":pid%100,"user":"fixture"})).collect::<Vec<_>>()
            }));
            put(view, "history", json!((0..300).map(|i|json!({"at":i*3000,"data":{"cpu":i%100,"cores":cores,"load":[1.0],"memoryUsed":2048,"memoryTotal":8192}})).collect::<Vec<_>>()));
            view.sync_overview_processes(window, cx);
        }));
        host.update(visual, |_, cx| cx.notify());
        frame(visual);
        for (position, delta) in [("top", 100000.), ("bottom", -100000.)] {
            visual.simulate_event(gpui::ScrollWheelEvent {
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
            let mut view = owner(window, cx);
            view.page = Page::Ports;
            put(&mut view,"page",Value::Array((0..500).map(|i|json!({"port":8000+i,"pid":i+1,"address":"127.0.0.1","processName":"node","projectRoot":"/srv/project-0"})).collect()));
            view.sync_connections(window,cx);
            view.connections_state.get_mut("fixture").unwrap().table = table;
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
    match index {
        0 => "connection-row-port-8000-1-127.0.0.1-",
        499 => "connection-row-port-8499-500-127.0.0.1-",
        _ => unreachable!(),
    }
}

#[gpui::test]
fn connection_metadata_completion_repaints_only_changed_project_names(cx: &mut TestAppContext) {
    use gpui_kit::base::test_support::snapshots;
    let (view, visual) = connections_fixture(cx, false);
    let (work, requests) = std::sync::mpsc::sync_channel(32);
    let (replies, incoming) = std::sync::mpsc::channel();
    let original =
        json!([{"serverId":"fixture", "name":"Original project", "path":"/srv/project-0"}]);
    view.update(visual, |view, cx| {
        view.work = work.into();
        view.replies = incoming;
        put(view, "connection-projects", original.clone());
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
            json!([{"serverId":"fixture", "name":"Renamed project", "path":"/srv/project-0"}]),
            1,
        ),
    ] {
        requests.try_iter().for_each(drop);
        let generation = view.update(visual, |view, _| {
            view.project_bookmarks_changed("fixture");
            view.poll_connections(Instant::now());
            requests
                .try_iter()
                .find(|work| work.key == "connection-projects")
                .unwrap()
                .generation
        });
        replies
            .send(super::Completion {
                pdf: None,
                pdf_text: None,
                image: None,
                server: "fixture".into(),
                key: "connection-projects".into(),
                generation,
                result: Ok(data),
            })
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
    for count in [100, 1000, 5000] {
        for table in [false, true] {
            view.update(visual, |view, cx| {
                put(
                    view,
                    "page",
                    json!(
                        (0..count)
                            .map(|i| json!({
                                "port":8000+i,"pid":i+1,"address":"127.0.0.1",
                                "processName":format!("service-{}", count-i),
                                "projectRoot":format!("/srv/project-{}", i/10)
                            }))
                            .collect::<Vec<_>>()
                    ),
                );
                let state = view.connections_state.get_mut("fixture").unwrap();
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
        let observation = json!({"data":view.resources.get("page").unwrap().value,
            "fetching":false,"error":null,"stale":false,"updatedAt":1000});
        let generation = view.resources.begin_refresh("page", false).unwrap();
        view.resources
            .complete_query("page", generation, Ok(observation.clone()));
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
                pdf: None,
                pdf_text: None,
                image: None,
                server: "fixture".into(),
                key: "page".into(),
                generation,
                result: Ok(observation.clone()),
            })
            .unwrap();
        view.update(visual, |view, cx| view.tick(cx));
        frame(visual);
    }
    assert_eq!(notices.read_with(visual, |count, _| *count), 0);
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
        view.resources.invalidate("page");
        view.resources.begin("page");
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
        let viewport = view.connections_state["fixture"].viewport.borrow();
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
            view.connections_state["fixture"].viewport.borrow().focus[0].clone()
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
            view.connections_state["fixture"].input.clone().unwrap()
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
        view.connections_state["fixture"]
            .collapsed
            .contains("/srv/project-0")
    }));
    assert!(visual.debug_bounds(row_selector(0)).is_none());
    assert_eq!(
        view.read_with(visual, |view, _| view.connections_state["fixture"]
            .viewport
            .borrow()
            .list
            .item_count()),
        1
    );
    click("project-0", visual);
    assert!(visual.debug_bounds(row_selector(0)).is_some());
    let input = view.read_with(visual, |view, _| {
        view.connections_state["fixture"].input.clone().unwrap()
    });
    visual.update(|window, cx| input.update(cx, |input, cx| input.set_value("8499", window, cx)));
    frame(visual);
    assert!(visual.debug_bounds(row_selector(499)).is_some());
    assert!(visual.debug_bounds(row_selector(0)).is_none());
    view.update(visual, |view, cx| {
        put(view, "page", json!([]));
        cx.notify();
    });
    frame(visual);
    assert!(visual.debug_bounds(row_selector(499)).is_none());
}

#[gpui::test]
fn connections_worktree_disclosure_survives_refresh_and_search(cx: &mut TestAppContext) {
    let (view, visual) = connections_fixture(cx, false);
    let ports = |branch: &str| {
        Value::Array((0..63).map(|i| json!({
        "port":8000+i, "pid":i+1, "address":"127.0.0.1", "processName":"node",
        "projectRoot":"/srv/project-0",
        "worktreePath":if i < 60 { "/srv/project-0/feature-a" } else { "/srv/project-0/feature-b" },
        "worktreeBranch":if i < 60 { branch } else { "feature-b" }, "worktreeMain":false
    })).collect())
    };
    view.update(visual, |view, cx| {
        put(view, "page", ports("feature-a"));
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
    assert!(
        visual
            .debug_bounds("connection-row-port-8060-61-127.0.0.1-")
            .is_some()
    );
    assert_eq!(
        view.read_with(visual, |view, _| view.connections_state["fixture"]
            .viewport
            .borrow()
            .list
            .item_count()),
        1
    );
    // A branch rename changes the label, never the disclosure identity.
    view.update(visual, |view, cx| {
        put(view, "page", ports("renamed-a"));
        cx.notify();
    });
    frame(visual);
    assert!(visual.debug_bounds(row_selector(0)).is_none());
    let input = view.read_with(visual, |view, _| {
        view.connections_state["fixture"].input.clone().unwrap()
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
            let mut view = owner(window, cx);
            view.page = Page::Overview;
            view.snapshot["runtime"]["agentInstallation"] =
                json!({"fixture":{"stage":"error","error":"Old installation failure"}});
            let generation = view.resources.begin("page").unwrap();
            view.resources
                .complete("page", generation, Err("SSH connection refused".into()));
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
        let observation = json!({"data":v.value("page"),"fetching":false,"error":null,"stale":false,"updatedAt":1});
        let late = v.resources.begin("page").unwrap();
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
        assert_eq!(v.value("page")[0]["port"], 8000);
        assert!(!v.resources.complete_query("page", late, Ok(observation)));
    });
    let queued: Vec<_> = requests.try_iter().collect();
    assert!(
        queued.iter().any(
            |request| request.method == "connections_query" && request.args["refresh"] == false
        )
    );
    view.update(visual, |v, cx| v.navigate(Page::Integration, cx));
    frame(visual);
    assert!(visual.debug_bounds(row_selector(0)).is_none());
    view.update(visual, |v, cx| {
        v.snapshot["config"]["servers"][0]["sshHost"] = json!("two");
        v.navigate(Page::Ports, cx);
        assert!(
            v.value("page").is_null(),
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
        v.connections_state["fixture"].input.clone().unwrap()
    });
    let notices = visual.new(|cx: &mut Context<usize>| {
        cx.observe(&view, |count, _, _| *count += 1).detach();
        0
    });
    for (page, server, visible, expected) in [
        (Page::Integration, "fixture", true, 0),
        (Page::Ports, "other", true, 0),
        (Page::Ports, "fixture", false, 0),
        (Page::Ports, "fixture", true, 1),
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
            let mut view = owner(window, cx);
            view.page = Page::Projects;
            view.window_visible = true;
            view.git.repo = json!({"repoId":"repo", "bare":false, "head":{"name":"main"}});
            view.git.page = json!({"snapshot":"one", "entries":[], "nextCursor":null,
                "metadata":{"head":{"name":"main"},"totalEntries":0}});
            let status = view.git.page.clone();
            put(&mut view, "git-sync-status", status);
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
    let observe = |data: Value, visual: &mut VisualTestContext| {
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
                pdf: None,
                pdf_text: None,
                image: None,
                server: "fixture".into(),
                key: "git-sync-status".into(),
                generation,
                result: Ok(data),
            })
            .unwrap();
        view.update(visual, |view, cx| view.tick(cx));
        frame(visual);
    };
    let original = view.read_with(visual, |view, _| view.git.page.clone());
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
    assert_eq!(request.args["serverId"], "fixture");
    assert_eq!(request.args["request"]["params"]["repoId"], "repo");
    replies
        .send(super::Completion {
            pdf: None,
            pdf_text: None,
            image: None,
            server: "fixture".into(),
            key: request.key,
            generation: request.generation,
            result: Ok(original.clone()),
        })
        .unwrap();
    view.update(visual, |view, cx| view.tick(cx));
    frame(visual);
    assert_eq!(notices.read_with(visual, |count, _| *count), 0);

    let mut changed = original.clone();
    changed["snapshot"] = json!("two");
    changed["metadata"]["head"]["name"] = json!("other");
    observe(changed.clone(), visual);
    assert_eq!(notices.read_with(visual, |count, _| *count), 1);
    assert_eq!(
        view.read_with(visual, |view, _| view.git.repo["head"]["name"].clone()),
        "other"
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
        view.git.page = changed.clone();
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
        let entity = cx.new(|cx| owner(window, cx));
        view = Some(entity.clone());
        PageHost(entity)
    });
    let view = view.unwrap();
    for (server, success) in [("other", true), ("fixture", false), ("fixture", true)] {
        let (work, requests) = std::sync::mpsc::sync_channel(64);
        let (replies, inbox) = std::sync::mpsc::channel();
        view.update(visual, |view, cx| {
            view.work = work.into();
            view.replies = inbox;
            view.page = Page::Projects;
            view.selection = Some(json!({"id":"project"}));
            view.git.repo = json!({"repoId":"repo", "bare":false});
            view.last_refresh = Instant::now();
            let key = format!("server-test:{server}");
            let generation = view.resources.begin(&key).unwrap();
            replies
                .send(super::Completion {
                    pdf: None,
                    pdf_text: None,
                    image: None,
                    server: server.into(),
                    key,
                    generation,
                    result: if success {
                        Ok(Value::Null)
                    } else {
                        Err("SSH agent unavailable".to_owned().into())
                    },
                })
                .unwrap();
            view.tick(cx);
        });
        let refresh = requests
            .try_iter()
            .find(|request| request.key == "git-refresh");
        assert_eq!(refresh.is_some(), server == "fixture" && success);
        if let Some(refresh) = refresh {
            assert_eq!(refresh.args, json!({"serverId":"fixture", "repoId":"repo"}));
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
        let entity = cx.new(|cx| owner(window, cx));
        view = Some(entity.clone());
        PageHost(entity)
    });
    let (work, requests) = std::sync::mpsc::sync_channel(64);
    view.unwrap().update(visual, |v, _| {
        v.work = work.into();
        v.page = Page::Projects;
        v.git.repo = json!({"repoId":"first","bare":false});
        v.git.tab = 0;
        put(v, "git-list", json!({"entries":[{"path":"existing"}]}));
        v.git_refresh();
        assert!(
            v.git_mutation_blocks_write(),
            "reviewed writes wait for refreshed facts"
        );
        let barrier = requests.try_recv().unwrap();
        assert_eq!(barrier.method, "git_refresh_repository");
        assert_eq!(barrier.args["repoId"], "first");
        assert!(requests.try_recv().is_err(), "no read before invalidation");
        assert_eq!(v.value("git-list")["entries"][0]["path"], "existing");
        v.resources.complete(
            &barrier.key,
            barrier.generation,
            Ok(json!({"repoId":"first"})),
        );
        v.git_completed(&barrier.key);
        let reads: Vec<_> = requests.try_iter().collect();
        assert!(
            reads
                .iter()
                .any(|r| r.key == "git-list" && r.args["request"]["params"]["repoId"] == "first")
        );

        v.git_refresh();
        let late = requests.try_recv().unwrap();
        v.git.repo = json!({"repoId":"second","bare":false});
        v.resources
            .complete(&late.key, late.generation, Ok(json!({"repoId":"first"})));
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
    let (view, visual) = cx.add_window_view(owner);
    let (work, requests) = std::sync::mpsc::sync_channel(64);
    view.update(visual, |v, _| {
        v.work = work.into();
        v.page = Page::Projects;
        v.git.repo = json!({"repoId":"repo","bare":false});
        let entry = json!({"entryId":"old","path":{"bytesB64":"YQ=="},"unstaged":true});
        let page = json!({"snapshot":"before","entries":[entry],"nextCursor":null,"metadata":{}});
        let diff = json!({"snapshot":"old-diff"});
        v.git.page = page.clone();
        v.git.selected = entry.clone();
        v.git.diff = diff.clone();
        put(v, "git-list", page.clone());
        put(v, "git-diff", diff.clone());
        let old_read = v.resources.begin("git-list").unwrap();
        v.git_refresh_index();
        assert!(v.git_mutation_blocks_write());
        assert_eq!(v.git.page, page);
        assert_eq!(v.git.selected, entry);
        assert_eq!(v.git.diff, diff);
        assert!(!v.resources.complete("git-list", old_read, Ok(page.clone())));
        let barrier = requests.try_recv().unwrap();
        assert_eq!(barrier.method, "git_refresh_repository");
        assert!(requests.try_recv().is_err());
        v.resources.complete(
            &barrier.key,
            barrier.generation,
            Ok(json!({"repoId":"repo"})),
        );
        v.git_completed(&barrier.key);
        let status = requests.try_recv().unwrap();
        assert_eq!(status.args["request"]["method"], "repo.status");
        assert!(requests.try_recv().is_err());
        assert_eq!(v.git.page, page);
        assert_eq!(v.git.diff, diff);
        let staged = json!({"entryId":"staged","path":{"bytesB64":"YQ=="},"staged":true});
        v.resources.complete(
            &status.key,
            status.generation,
            Ok(json!({"snapshot":"after","entries":[staged],"nextCursor":null,"metadata":{}})),
        );
        v.git_completed(&status.key);
        assert!(!v.git_mutation_blocks_write());
        assert_eq!(v.git.selected, staged);
        assert!(v.git.diff_staged);
        let reads: Vec<_> = requests.try_iter().collect();
        assert_eq!(reads.len(), 1);
        assert_eq!(reads[0].args["request"]["method"], "repo.diff_page");
        assert_eq!(reads[0].args["request"]["params"]["snapshot"], "after");
        assert_eq!(reads[0].args["request"]["params"]["side"], "head_to_index");

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
        let view = cx.new(|cx| owner(window, cx));
        owner_view = Some(view.clone());
        PageHost(view)
    });
    let view = owner_view.unwrap();
    let (work, _requests) = std::sync::mpsc::sync_channel(64);
    view.update(visual, |v, cx| {
        v.work = work.into();
        v.page = Page::Overview;
        for (key, value) in [
            ("page", json!({"cpu":42})),
            ("history", json!([{"at":123}])),
        ] {
            v.resources.invalidate(key);
            let generation = v.resources.begin(key).unwrap();
            v.resources.complete(key, generation, Ok(value));
        }
        let stale = v.resources.begin("page").unwrap();
        v.navigate(Page::Integration, cx);
        assert!(v.resources.get("page").is_none());
        assert!(!v.resources.complete("page", stale, Ok(json!({"cpu":99}))));
        v.navigate(Page::Overview, cx);
        assert_eq!(v.resources.get("page").unwrap().value["cpu"], 42);
        assert_eq!(v.resources.get("history").unwrap().value[0]["at"], 123);
        assert!(v.resources.get("page").unwrap().pending);
        assert!(!v.resources.complete("page", stale, Ok(json!({"cpu":99}))));
        v.navigate(Page::Integration, cx);
        v.reset(); // Connection changes must invalidate the retained data.
        assert!(v.overview_state.cached_reads.is_none());
        v.navigate(Page::Overview, cx);
        assert!(v.resources.get("page").unwrap().value.is_null());
    });
}

#[gpui::test]
fn overview_loading_retry_keeps_stale_warning_until_success(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui::component::init(cx);
        theme::install(false, cx);
    });
    let (view, visual) = cx.add_window_view(owner);
    let (work, _requests) = std::sync::mpsc::sync_channel(64);
    view.update(visual, |view, _| {
        view.work = work.into();
        view.page = Page::Overview;
        for key in ["page", "history"] {
            view.resources.invalidate(key);
            put(view, key, json!({"cached":true}));
            let failed = view.resources.begin(key).unwrap();
            view.resources.complete(key, failed, Err("Offline".into()));
        }
        view.refresh();
        for key in ["page", "history"] {
            let resource = view.resources.get(key).unwrap();
            assert!(resource.pending);
            assert_eq!(resource.error.as_deref(), Some("Offline"));
            assert_eq!(resource.value["cached"], true);
        }
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
            let mut view = owner(window, cx);
            view.page = Page::Projects;
            view.resources.begin("page");
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
            let mut view = owner(window, cx);
            view.page = Page::Projects;
            put(&mut view, "page", json!([]));
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
