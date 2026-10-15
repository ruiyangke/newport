mod grammars;
extern crate self as newport_gpui;

pub use newport_client::backend;
pub mod git_log_text;
pub use newport_domain::git_operation;
pub mod terminal_focus;
pub mod terminal_input;
pub mod terminal_mouse;
pub use newport_client::terminal_transport;

pub mod forwarding;
pub mod sticky_header;
pub mod sticky_list_headers;
pub mod terminal_status;

mod about;
mod appearance;
use appearance::ResolveAppearance;
mod assets;
mod connection_batches;
mod connections;
mod connections_grouped;
mod connections_rows;
mod connections_selector;
mod containers;
mod containers_host;
mod controls;
mod display_preferences;
mod file_icon;
mod file_icon_assets;
mod file_image;
mod file_pdf;
mod file_preview;
mod file_transfer;
mod file_tree;
mod files;
mod git;
mod git_blob_text;
mod git_branch_manage;
mod git_branch_picker;
mod git_branch_search;
mod git_changes;
mod git_checkout_picker;
mod git_clean;
mod git_commit;
mod git_commit_header;
mod git_conflict;
mod git_diff;
mod git_diff_actions;
mod git_diff_runs;
mod git_diff_search;
mod git_diff_settings;
mod git_diff_split;
mod git_diff_syntax;
mod git_diff_text;
mod git_discard;
mod git_file_row;
mod git_history;
mod git_history_files;
mod git_integration;
mod git_layout;
mod git_listing;
mod git_log;
mod git_log_view;
mod git_mutation;
mod git_open;
mod git_page_demand;
mod git_picker_keys;
mod git_project_picker;
mod git_pull;
mod git_remote_actions;
mod git_remote_edit;
mod git_remote_picker;
mod git_remote_refs;
mod git_remotes;
mod git_reset;
mod git_settings;
mod git_split;
mod git_stash;
mod git_sync;
mod git_tag_push;
mod git_tags;
mod git_watch;
mod git_worktree_create;
mod git_worktree_picker;
mod git_worktree_view;
mod git_worktrees;
mod integration;
mod integration_helpers;
mod integration_removal;
mod request_dispatch;
mod server_setup;
mod setup_preview;
pub use setup_preview::run as run_setup_preview;
mod empty;
mod loading;
mod localized_time;
mod log_line;
mod material;
mod modal;
mod navigation;
mod overview;
mod overview_process;
#[cfg(test)]
mod page_perf_tests;
mod presentation_time;
mod project_add;
mod project_checks;
mod project_create;
mod project_dialog;
mod project_folder;
mod project_library;
mod project_library_rows;
mod project_library_state;
mod project_loading;
mod project_preferences;
mod quiet_action;
mod screens;
mod selectable_log;
mod server_form;

/// Exercise the platform's real editing bindings in shared UI fixtures.
#[cfg(test)]
fn test_shortcuts(cx: &mut gpui_kit::VisualTestContext, mac_keys: &str) {
    if cfg!(target_os = "macos") {
        cx.simulate_keystrokes(mac_keys);
    } else {
        cx.simulate_keystrokes(&mac_keys.replace("cmd-", "ctrl-"));
    }
}
mod server_picker;
mod services;
mod services_host;
mod settings;
mod shell;
mod sidebar_layout;
mod snapshot_presentation;
mod state;
mod terminal_color;
mod terminal_paint;
mod terminal_scope;
mod terminal_search_paint;
mod terminal_session_menu;
mod terminal_view;
mod terminal_workspace;
mod theme;
mod tunnel_actions;
mod tunnel_delete;
mod tunnel_form;
mod window_state;
mod workspace_state;
use gpui_kit::assets::IconName;
use gpui_kit::base::Selectable as _;
use gpui_kit::component::menu::{ContextMenuExt, DropdownMenu, PopupMenu, PopupMenuItem};
use gpui_kit::component::{ActiveTheme, Icon, WindowExt, notification::Notification};
use gpui_kit::component::{
    Disableable, Sizable,
    button::{Button, ButtonVariants},
    input::{Input, InputState},
};
use gpui_kit::{prelude::*, *};
use serde_json::{Value, json};
use state::Resources;
use std::{
    sync::mpsc,
    time::{Duration, Instant},
};

#[derive(Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
enum Page {
    Overview,
    Ports,
    Projects,
    Services,
    Containers,
    Terminal,
    Files,
    Integration,
    Settings,
}
impl Page {
    const ALL: [(Self, &'static str); 9] = [
        (Self::Overview, "Overview"),
        (Self::Ports, "Connections"),
        (Self::Projects, "Projects"),
        (Self::Services, "Services"),
        (Self::Containers, "Containers"),
        (Self::Terminal, "Terminal"),
        (Self::Files, "Files"),
        (Self::Integration, "Integrations"),
        (Self::Settings, "Settings"),
    ];
    fn title(self) -> &'static str {
        Self::ALL.iter().find(|(p, _)| *p == self).unwrap().1
    }
}
struct Completion {
    pdf: Option<file_pdf::PdfBytes>,
    pdf_text: Option<std::sync::Arc<file_pdf::TextLayer>>,
    image: Option<std::sync::Arc<RenderImage>>,
    server: String,
    key: String,
    generation: u64,
    result: Result<Value, newport_client::request_error::RequestError>,
}
struct Work {
    pdf: Option<file_pdf::PdfBytes>,
    key: String,
    generation: u64,
    method: String,
    args: Value,
}
struct Newport {
    terminal_immersive: bool,
    window_state: window_state::WindowState,
    terminals: std::collections::HashMap<String, Entity<terminal_workspace::Workspace>>,
    integration_refresh_pending: std::collections::HashSet<String>,
    needle_removal_reviews: std::collections::HashMap<String, integration_removal::Review>,
    needle_helper_ui: std::collections::HashMap<String, integration_helpers::State>,
    integration_shell: Entity<integration::ShellCode>,
    server_form: Option<Entity<server_form::ServerForm>>,
    modal_open: bool,
    server_picker: Entity<server_picker::Picker>,
    picker_items: Vec<server_picker::ServerItem>,
    picker_selection: String,
    page: Page,
    workspace_page: Page,
    restore_git: Option<workspace_state::GitDestination>,
    git_destinations: std::collections::BTreeMap<String, workspace_state::GitDestination>,
    appearance: appearance::Appearance,
    appearance_store: appearance::AppearanceStore,
    settings_state: newport_client::settings::Settings,
    settings_ui: settings::SettingsUi,
    services_panels: std::collections::HashMap<String, services_host::HostedServices>,
    services_preferences: std::collections::HashMap<String, services::Preferences>,
    retired_services: Vec<Entity<services::ServicesPanel>>,
    services_refresh_requested: bool,
    display_preferences: display_preferences::DisplayPreferences,
    connections_state: std::collections::HashMap<String, connections::State>,
    tunnel_operations: newport_client::tunnel_operations::Operations,
    setup_flow: Option<server_setup::Flow>,
    overview_state: overview::OverviewState,
    containers_panels: std::collections::HashMap<String, containers_host::HostedContainers>,
    containers_preferences: std::collections::HashMap<String, containers::Preferences>,
    retired_containers: Vec<Entity<containers::ContainersPanel>>,
    containers_refresh_requested: bool,
    sidebar_visible: bool,
    sidebar_layout: sidebar_layout::SidebarLayout,
    content_width: f32,
    server: String,
    saved_server_selection: Option<String>,
    snapshot: Value,
    resources: Resources,
    work: request_dispatch::Dispatcher,
    replies: mpsc::Receiver<Completion>,
    last_refresh: Instant,
    window_visible: bool,
    focus: FocusHandle,
    navigation_focus: Vec<FocusHandle>,
    sidebar_focus: FocusHandle,
    file_browsers: std::collections::HashMap<String, files::Browser>,
    files_refresh_requested: bool,
    file_preview_path: Option<String>,
    file_preview_image: Option<std::sync::Arc<RenderImage>>,
    file_preview_viewport: file_preview::Viewport,
    file_preview_pdf: Option<file_pdf::PdfBytes>,
    file_preview_pdf_text: Option<std::sync::Arc<file_pdf::TextLayer>>,
    file_transfer: Option<file_transfer::Transfer>,
    file_chooser: Option<file_transfer::FileChooser>,
    file_project_connection: Option<Value>,
    selection: Option<Value>,
    project_add: Option<project_add::Add>,
    project_edit: Option<project_library::Edit>,
    project_create: Option<project_create::Create>,
    project_creation_save_owner: Option<(uuid::Uuid, String)>,
    project_folder: Option<project_folder::Chooser>,
    project_dialog_open: bool,
    project_dialog_feedback: project_dialog::FeedbackScope,
    project_dialog_rendering: bool,
    project_checks: Option<project_checks::Batch>,
    project_libraries: std::collections::BTreeMap<String, project_library_state::State>,
    project_preferences: std::collections::BTreeMap<String, project_preferences::Preferences>,
    git: git::Workspace,
    git_diff_layouts: std::collections::HashMap<String, git_diff_split::Layout>,
    git_mutation: Option<git_mutation::Mutation>,
    git_notice_clear: bool,
    git_log: git_log::Log,
    git_drafts: std::collections::HashMap<String, git_commit::Draft>,
    notice: Option<String>,
    notification: Option<Notification>,
}
gpui_kit::actions!(
    newport,
    [
        Quit,
        About,
        Refresh,
        ToggleSidebar,
        ToggleTerminalImmersive,
        FindInView,
        OpenSettings,
        NewServer,
        EditServer,
        TestServer,
        SidebarWider,
        SidebarNarrower,
        ResetSidebar,
        HideApp,
        HideOthers,
        ShowAll,
        CloseWindow,
        MinimizeWindow,
        ZoomWindow,
        ToggleFullScreen
    ]
);
fn text(value: &Value, field: &str) -> String {
    let v = &value[field];
    v.as_str().map(str::to_owned).unwrap_or_else(|| {
        if v.is_null() {
            "—".into()
        } else {
            v.to_string()
        }
    })
}
fn data_server_exists(snapshot: &Value) -> bool {
    snapshot["config"]["servers"]
        .as_array()
        .is_some_and(|a| !a.is_empty())
}
fn rows(value: &Value) -> Vec<Value> {
    value.as_array().cloned().unwrap_or_default()
}
fn label(value: impl Into<SharedString>) -> Div {
    div().text_sm().opacity(0.7).child(value.into())
}
fn cell(value: impl Into<SharedString>, width: f32) -> Div {
    div()
        .w(px(width))
        .flex_shrink_0()
        .overflow_hidden()
        .child(value.into())
}
fn bytes(value: &Value) -> String {
    let n = value.as_f64().unwrap_or(0.);
    if n < 1024. {
        format!("{n:.0} B")
    } else if n < 1048576. {
        format!("{:.1} KiB", n / 1024.)
    } else if n < 1073741824. {
        format!("{:.1} MiB", n / 1048576.)
    } else if n < 1099511627776. {
        format!("{:.1} GiB", n / 1073741824.)
    } else {
        format!("{:.1} TiB", n / 1099511627776.)
    }
}
impl Newport {
    fn new(
        window: &mut Window,
        window_state: window_state::WindowState,
        cx: &mut Context<Self>,
    ) -> Self {
        Self::observe_modal_visibility(window, cx);
        cx.observe_window_bounds(window, |this, window, cx| {
            this.window_state.capture(window, cx)
        })
        .detach();
        cx.on_app_quit(|this, cx| {
            this.save_workspace_selection();
            if let Err(error) = this.window_state.save() {
                eprintln!("Could not save native window state: {error}");
            }
            let ids: Vec<_> = this
                .terminals
                .values()
                .flat_map(|workspace| workspace.read(cx).session_ids(cx))
                .collect();
            let tasks: Vec<_> = ids
                .into_iter()
                .map(|id| {
                    cx.background_spawn(async move {
                        let _ = backend::Backend::discover().call_with_timeout(
                            "terminal_close",
                            json!({"session":id}),
                            Duration::from_secs(2),
                        );
                    })
                })
                .collect();
            async move {
                for task in tasks {
                    task.await;
                }
            }
        })
        .detach();
        let (sidebar_layout, sidebar_error) = sidebar_layout::SidebarLayout::new(cx);
        let appearance_store = appearance::AppearanceStore::from_env();
        let (appearance, appearance_error) = match appearance_store.load() {
            Ok(value) => (value, None),
            Err(error) => (
                appearance::Appearance::System,
                Some(format!("Could not load appearance: {error}")),
            ),
        };
        theme::install(appearance.is_dark(window.appearance()), cx);
        material::sync(window, appearance.is_dark(window.appearance()));
        let material_dark = appearance.is_dark(window.appearance());
        window.defer(cx, move |window, _| material::sync(window, material_dark));
        cx.observe_window_appearance(window, |this, window, cx| {
            theme::install(this.appearance.is_dark(window.appearance()), cx);
            material::sync(window, this.appearance.is_dark(window.appearance()));
            cx.notify();
        })
        .detach();
        // Tauri's query focus manager follows document visibility, not keyboard
        // focus. A visible but inactive native window should keep polling too.
        cx.observe_window_visibility(window, |this, visibility, _, cx| {
            this.window_visibility_changed(visibility.is_visible(), cx);
        })
        .detach();
        let (work, replies) = request_dispatch::start();
        cx.spawn(async move |view, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(50))
                    .await;
                if view.update(cx, |this, cx| this.tick(cx)).is_err() {
                    break;
                }
            }
        })
        .detach();
        let restored = workspace_state::load();
        let chart_focus = cx.focus_handle();
        cx.on_blur(&chart_focus, window, |this, _, cx| {
            this.overview_state.inspection = None;
            this.overview_state.keyboard_inspection = false;
            cx.notify();
        })
        .detach();
        let mut this = Self {
            window_state,
            terminals: Default::default(),
            terminal_immersive: false,
            integration_refresh_pending: Default::default(),
            needle_removal_reviews: Default::default(),
            needle_helper_ui: Default::default(),
            integration_shell: cx.new(|cx| integration::ShellCode::new(window, cx)),
            server_form: None,
            modal_open: false,
            server_picker: Self::init_picker(window, cx),
            picker_items: Vec::new(),
            picker_selection: String::new(),
            page: restored.page,
            workspace_page: restored.workspace_page,
            restore_git: restored.git,
            git_destinations: restored.git_destinations,
            appearance,
            appearance_store,
            settings_state: newport_client::settings::Settings::new(backend::Backend::discover()),
            settings_ui: settings::SettingsUi::new(appearance, window, cx),
            services_panels: Default::default(),
            services_preferences: Default::default(),
            retired_services: Vec::new(),
            services_refresh_requested: false,
            display_preferences: display_preferences::DisplayPreferences::new(cx),
            setup_flow: None,
            overview_state: overview::OverviewState {
                chart_focus: Some(chart_focus),
                interfaces: restored.network_interfaces,
                preferences: restored.overview_preferences,
                ..Default::default()
            },
            connections_state: Default::default(),
            tunnel_operations: newport_client::tunnel_operations::Operations::new(
                backend::Backend::discover(),
            ),
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
            work,
            replies,
            last_refresh: Instant::now(),
            window_visible: window.is_visible(),
            focus: cx.focus_handle(),
            sidebar_focus: cx.focus_handle(),
            navigation_focus: navigation::VIEWS
                .iter()
                .map(|_| cx.focus_handle())
                .collect(),
            file_browsers: Default::default(),
            files_refresh_requested: true,
            file_preview_path: None,
            file_preview_image: None,
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
            git_diff_layouts: Default::default(),
            git_mutation: None,
            git_notice_clear: false,
            git_log: Default::default(),
            git_drafts: Default::default(),
            notice: None,
            notification: {
                let errors: Vec<_> = appearance_error
                    .into_iter()
                    .chain(
                        sidebar_error
                            .map(|error| format!("Could not load sidebar preferences: {error}")),
                    )
                    .collect();
                (!errors.is_empty()).then(|| Notification::error(errors.join("\n")))
            },
        };
        this.focus.focus(window, cx);
        this.request("snapshot", "snapshot", json!({}));
        this
    }
    fn request(&mut self, key: &str, method: &str, args: Value) {
        let generation = if key == "snapshot" {
            // A background retry must not temporarily make unavailable
            // configuration look healthy before a fresh reply arrives.
            self.resources.begin_refresh(key, false)
        } else {
            self.resources.begin(key)
        };
        self.dispatch_request(key, method, args, generation);
    }
    fn request_refresh(&mut self, key: &str, method: &str, args: Value) {
        let generation = self.resources.begin_refresh(key, false);
        self.dispatch_request(key, method, args, generation);
    }
    fn dispatch_request(&mut self, key: &str, method: &str, args: Value, generation: Option<u64>) {
        let Some(generation) = generation else {
            return;
        };
        if self
            .work
            .try_send(Work {
                pdf: (method == "native.pdf_page")
                    .then(|| self.file_preview_pdf.clone())
                    .flatten(),
                key: key.into(),
                generation,
                method: method.into(),
                args,
            })
            .is_err()
        {
            self.resources.complete(
                key,
                generation,
                Err("Too many requests. Try again when an operation finishes.".into()),
            );
        }
    }
    fn window_visibility_changed(&mut self, visible: bool, cx: &mut Context<Self>) {
        let resumed = visible && !self.window_visible;
        self.window_visible = visible;
        if resumed {
            // Paint the newest retained state even when background data has
            // stopped changing since the window was hidden.
            cx.notify();
        }
    }
    fn tick(&mut self, cx: &mut Context<Self>) {
        self.poll_project_library();
        self.poll_project_checks(cx);
        self.sync_git_log(cx);
        self.poll_conflict_preview(cx);
        let branch_search_changed = self.poll_git_branch_search();
        let changes_search_changed = self.poll_git_changes();
        let remote_refs_changed = self.poll_remote_refs();
        let remote_search_changed = self.poll_git_remote_search();
        self.window_state.settle();
        let mut changed = self.display_preferences.refresh();
        changed |= branch_search_changed;
        changed |= self.poll_checkout_search();
        changed |= self.poll_checkout_scroll();
        changed |= changes_search_changed;
        changed |= remote_refs_changed;
        changed |= remote_search_changed;
        changed |= self.poll_git_diff_scroll();
        changed |= self.poll_git_mutation(cx);
        changed |= self.poll_tunnel_actions();
        let settings = self.settings_state.poll(self.page == Page::Settings);
        changed |= self.apply_settings_effects(settings);
        let mut completed_file_project = None;
        while let Ok(reply) = self.replies.try_recv() {
            let snapshot_status = |resource: Option<&state::Resource>| {
                resource.map(|resource| (resource.display_pending(), resource.error.clone()))
            };
            let previous_snapshot_status = if reply.key == "snapshot" {
                snapshot_status(self.resources.get("snapshot"))
            } else {
                None
            };
            let previous_presentation = self.resources.get(&reply.key).and_then(|resource| {
                (resource.updated_at.is_some() || !resource.value.is_null())
                    .then_some(resource.presentation_revision)
            });
            let opened = (reply.key == "repository")
                .then(|| reply.result.as_ref().ok().cloned())
                .flatten();
            let accepted = if reply.key == "page" && self.page == Page::Ports {
                self.resources
                    .complete_query(&reply.key, reply.generation, reply.result)
            } else {
                self.resources
                    .complete_detailed(&reply.key, reply.generation, reply.result)
            };
            if !accepted {
                if let Some(repo) = opened {
                    std::thread::spawn(move || {
                        backend::Backend::discover().release_repository(&reply.server, &repo)
                    });
                }
                continue;
            }
            if reply.key == "git-sync-status" {
                let presentation_changed = previous_presentation.is_none()
                    || previous_presentation
                        != self
                            .resources
                            .get(&reply.key)
                            .map(|r| r.presentation_revision);
                // An identical observation may still invalidate a displayed
                // older page or restore its HEAD. Reconcile before deciding
                // whether the window needs a redraw.
                let recovered = self.git_recover_missing_checkout(&reply.key);
                let reconciled = !recovered
                    && self
                        .resources
                        .get(&reply.key)
                        .is_some_and(|r| r.error.is_none())
                    && self.git_watch_completed();
                changed |= presentation_changed || recovered || reconciled;
                continue;
            }
            if reply.key == "page" && self.page == Page::Projects {
                self.restore_git_destination();
            }
            let background_read = reply.key == "snapshot"
                || reply.key == "connection-projects" && self.page == Page::Ports
                || reply.key == "page" && matches!(self.page, Page::Ports | Page::Projects)
                || reply.key.starts_with("project-status:");
            let configuration_acknowledgement = reply.key == "snapshot"
                && (self.saved_server_selection.is_some()
                    || !self.integration_refresh_pending.is_empty());
            if background_read
                && !configuration_acknowledgement
                && previous_presentation.is_some()
                && previous_presentation
                    == self
                        .resources
                        .get(&reply.key)
                        .map(|r| r.presentation_revision)
            {
                // The request still settled and advanced polling state. Skip
                // rebuilding shell scopes and notifying the entire window for
                // another observation of the same displayed state.
                continue;
            }
            if reply.key.starts_with("project-status:") {
                // Retain the query result above, but only repaint a library
                // that currently displays this repository's status.
                changed |= self.project_status_visible(&reply.key);
                continue;
            }
            changed |= reply.key != "snapshot"
                || configuration_acknowledgement
                || previous_snapshot_status != snapshot_status(self.resources.get("snapshot"))
                || snapshot_presentation::changed(
                    &self.snapshot,
                    self.resources
                        .get("snapshot")
                        .map(|resource| &resource.value)
                        .unwrap_or(&Value::Null),
                    &self.server,
                    match self.page {
                        Page::Integration => snapshot_presentation::Surface::Integration,
                        Page::Overview if self.setup_visible() => {
                            snapshot_presentation::Surface::Setup
                        }
                        _ => snapshot_presentation::Surface::General,
                    },
                );
            // Even quiet snapshots must reconcile retained connection owners.
            if reply.key == "preview"
                && self
                    .resources
                    .get("preview")
                    .is_some_and(|resource| resource.error.is_none())
            {
                self.file_preview_image = reply.image;
                self.file_preview_pdf = reply.pdf;
                self.file_preview_pdf_text = reply.pdf_text;
            }
            if reply.key == "snapshot" {
                let data = self.value("snapshot");
                if !data.is_null() {
                    let old_server = self.snapshot["config"]["servers"]
                        .as_array()
                        .and_then(|a| a.iter().find(|s| s["id"] == self.server))
                        .cloned();
                    let old_connection = old_server
                        .as_ref()
                        .map(|server| terminal_scope::connection(server, &self.snapshot));
                    self.sync_connections_snapshot(&data);
                    self.snapshot = data;
                    // A failed profile load is not evidence that a server was
                    // deleted. Preserve the destination until a valid snapshot.
                    if !self.configuration_available() {
                        continue;
                    }
                    self.prune_connections();
                    self.prune_project_libraries();
                    // A failed refresh (or a vault load error) does not confirm
                    // the preferences written by an integration operation.
                    // Keep its controls locked through retries until fresh
                    // configuration has actually been accepted.
                    self.integration_refresh_pending.clear();
                    let server_ids: std::collections::HashSet<String> =
                        rows(&self.snapshot["config"]["servers"])
                            .iter()
                            .filter_map(|server| server["id"].as_str().map(str::to_owned))
                            .collect();
                    self.terminals.retain(|id, _| server_ids.contains(id));
                    self.file_browsers.retain(|id, _| server_ids.contains(id));
                    for server in rows(&self.snapshot["config"]["servers"]) {
                        if let Some(id) = server["id"].as_str() {
                            let scope = terminal_scope::connection(&server, &self.snapshot);
                            if self
                                .file_browsers
                                .get(id)
                                .is_some_and(|browser| browser.connection != scope)
                            {
                                self.file_connection_changed(id, scope);
                            }
                        }
                        if let Some(workspace) =
                            server["id"].as_str().and_then(|id| self.terminals.get(id))
                        {
                            let scope = terminal_scope::connection(&server, &self.snapshot);
                            let endpoint = format!(
                                "{}@{}",
                                text(&server, "sshUser"),
                                text(&server, "sshHost")
                            );
                            workspace.update(cx, |workspace, cx| {
                                workspace.sync_connection(scope, endpoint, cx)
                            });
                        }
                    }
                    if let Some(id) = self.saved_server_selection.clone()
                        && rows(&self.snapshot["config"]["servers"])
                            .iter()
                            .any(|server| server["id"] == id)
                    {
                        self.reset();
                        self.server = id;
                        self.saved_server_selection = None;
                        self.refresh();
                    }
                    if self.server.is_empty() && data_server_exists(&self.snapshot) {
                        self.server = self.snapshot["config"]["servers"][0]["id"]
                            .as_str()
                            .unwrap_or_default()
                            .into();
                        self.refresh();
                    } else {
                        let current = self.snapshot["config"]["servers"]
                            .as_array()
                            .and_then(|a| a.iter().find(|s| s["id"] == self.server))
                            .cloned();
                        let connection_changed = current
                            .as_ref()
                            .map(|server| terminal_scope::connection(server, &self.snapshot))
                            != old_connection;
                        if connection_changed || (!self.server.is_empty() && current.is_none()) {
                            // A backend restart or connection reset invalidates reads,
                            // not the user's repository/worktree destination. Reopen
                            // it only after reloading the same server's bookmarks.
                            let destination = (current == old_server)
                                .then(|| self.git_destination())
                                .flatten();
                            if current.is_none() {
                                self.server = rows(&self.snapshot["config"]["servers"])
                                    .first()
                                    .and_then(|server| server["id"].as_str())
                                    .unwrap_or_default()
                                    .to_owned();
                            }
                            self.reset();
                            self.restore_git = destination;
                            self.refresh_navigation();
                        }
                    }
                }
            } else if reply.key == "server-save" {
                if self
                    .resources
                    .get("server-save")
                    .is_some_and(|r| r.error.is_none())
                {
                    self.saved_server_selection = Some(reply.server.clone());
                    self.notification = Some(Notification::success("Server saved."));
                    self.request("snapshot", "snapshot", json!({}));
                }
            } else if reply.key.starts_with("server-delete:") {
                let result = self.resources.get(&reply.key).unwrap();
                self.notification = Some(match &result.error {
                    Some(error) => Notification::error(error.clone()),
                    None => Notification::success("Server deleted."),
                });
                self.request("snapshot", "snapshot", json!({}));
            } else if reply.key.starts_with("server-test:") {
                if reply.server == self.server {
                    let result = self.resources.get(&reply.key).unwrap();
                    self.notification = Some(match &result.error {
                        Some(error) => Notification::error(error.clone()),
                        None => Notification::success("SSH connection verified."),
                    });
                    if result.error.is_none() && !self.git_write_pending() {
                        self.refresh();
                    }
                    self.request("snapshot", "snapshot", json!({}));
                }
            } else if reply.key.starts_with("integration:") {
                self.integration_completed(&reply.key, &reply.server);
            } else if reply.key == "file-project" {
                let resource = self.resources.get("file-project").unwrap();
                completed_file_project = Some((
                    reply.server,
                    self.file_project_connection.take(),
                    resource
                        .error
                        .clone()
                        .map_or_else(|| Ok(resource.value.clone()), Err),
                ));
            } else if reply.key == "project-created-save" {
                let saved = self.value("project-created-save");
                if self
                    .resources
                    .get("project-created-save")
                    .is_some_and(|r| r.error.is_none())
                {
                    if self.project_create.as_ref().is_some_and(|form| {
                        self.project_creation_save_owner
                            .as_ref()
                            .is_some_and(|(id, server)| *id == form.id && server == &self.server)
                    }) {
                        self.project_create = None;
                    }
                    self.project_creation_save_owner = None;
                    self.project_bookmarks_changed(&reply.server);
                    self.notification = Some(Notification::success(
                        "Repository created and saved in Projects.",
                    ));
                    if saved["serverId"].as_str() == Some(&self.server)
                        && self.page == Page::Projects
                        && self.selection.is_none()
                    {
                        self.refresh();
                    }
                }
            } else if reply.key == "project-edit" {
                self.project_edit_completed();
            } else if reply.key == "project-save" {
                if self
                    .resources
                    .get("project-save")
                    .is_some_and(|r| r.error.is_none())
                {
                    self.project_bookmarks_changed(&reply.server);
                    self.project_add = None;
                    self.notification = Some(Notification::success("Project added."));
                    if self.page == Page::Projects {
                        self.refresh();
                    }
                }
            } else if reply.key.starts_with("git-") || reply.key == "repository" {
                if reply.key == "git-log-events" {
                    self.git_log_completed(cx);
                }
                self.git_sync_completed(&reply.key, cx);
                self.git_completed(&reply.key);
            } else if reply.key == "action" {
                let r = self.resources.get("action").unwrap();
                self.notice = Some(r.error.clone().unwrap_or_else(|| {
                    if r.value["needsPassword"] == true {
                        "This action requires a password. Use Newport’s terminal to authorize it."
                            .into()
                    } else {
                        "Operation completed.".into()
                    }
                }));
                self.request("snapshot", "snapshot", json!({}));
                self.refresh();
            }
        }
        // Apply configuration replies before draining retained panel actions so
        // an obsolete controller cannot refresh a newly configured endpoint.
        changed |= self.poll_file_browsers(cx);
        changed |= self.poll_file_transfer(cx);
        if let Some((server, connection, result)) = completed_file_project {
            self.file_project_completed(&server, connection, result);
        }
        self.git_branch_picker_poll(cx);
        changed |= self.poll_upstream_search();
        changed |= self.poll_git_status();
        self.poll_services(cx);
        self.poll_containers(cx);
        changed |= self.poll_setup();
        let refresh_interval = Duration::from_secs(if self.page == Page::Overview { 3 } else { 5 });
        if self.last_refresh.elapsed() >= refresh_interval {
            self.last_refresh = Instant::now();
            self.request("snapshot", "snapshot", json!({}));
            if self.page == Page::Overview {
                self.refresh();
            }
        }
        changed |= self.poll_connections(Instant::now());
        self.git_listing_scroll();
        self.git_history_files_scroll();
        if changed {
            cx.notify();
        }
    }
    fn reset(&mut self) {
        self.reset_view(false);
    }
    fn reset_view(&mut self, navigation: bool) {
        if !navigation {
            self.setup_flow = None;
            self.overview_state.cached_reads = None;
        }
        self.project_checks = None;
        self.close_project_folder();
        self.close_file_preview();
        for browser in self.file_browsers.values_mut() {
            browser.reads.suspend();
            browser.active = false;
        }
        self.git_release();
        if navigation {
            self.resources.clear_preserving("setup-");
        } else {
            self.resources.clear();
        }
        self.git = git::Workspace::default();
        self.selection = None;
        self.project_add = None;
        self.project_edit = None;
        self.project_create = None;
        self.notice = None;
    }
    fn invalidate_metric_readings(&mut self) {
        self.overview_state.cached_reads = None;
        // Generations reject replies that started before cache clearing finished.
        // Page resources are shared by route; never erase another page's data.
        self.resources.invalidate("history");
        self.overview_state.charts.borrow_mut().take();
        self.overview_state.inspection = None;
        self.overview_state.keyboard_inspection = false;
        if self.page == Page::Overview {
            self.resources.invalidate("page");
            self.refresh();
        }
    }
    fn value(&self, key: &str) -> Value {
        self.resources
            .get(key)
            .map(|r| r.value.clone())
            .unwrap_or_default()
    }
    fn refresh(&mut self) {
        self.project_checks = None;
        if self.server.is_empty() && self.page != Page::Settings {
            return;
        }
        match self.page {
            Page::Overview => {
                self.poll_setup();
                if self.setup_visible() || self.setup_flow.as_ref().is_some_and(|f| f.skipped) {
                    return;
                }
                self.request_refresh(
                    "page",
                    "cockpit_collect",
                    json!({"id":self.server,"section":"overview","refresh":false}),
                );
                self.request_refresh("history","cockpit_history",json!({"id":self.server,"minutes":self.overview_state.prefs(&self.server).minutes}));
            }
            Page::Ports => {
                if !self.configuration_available() {
                    return;
                }
                self.restore_connections();
                self.discover_connections(false);
                self.refresh_connection_projects(Instant::now());
            }
            Page::Projects => {
                if self.selection.is_some() {
                    self.git_refresh();
                } else {
                    self.refresh_project_library(true);
                }
            }
            Page::Services => self.services_refresh_requested = true,
            Page::Containers => self.containers_refresh_requested = true,
            Page::Files => self.files_refresh_requested = true,
            Page::Terminal => {}
            Page::Integration => self.request("snapshot", "snapshot", json!({})),
            Page::Settings => self.settings_state.refresh(),
        }
    }
    fn sidebar(&self, compact: bool, cx: &mut Context<Self>) -> AnyElement {
        let menu_view = cx.entity().downgrade();
        let connected = rows(&self.snapshot["config"]["tunnels"])
            .iter()
            .filter(|t| self.snapshot["runtime"]["tunnels"][text(t, "id")]["status"] == "connected")
            .count();
        div()
            .id("shell-sidebar")
            .debug_selector(|| "shell-sidebar".into())
            .w_full()
            .h_full()
            .min_h_0()
            .overflow_y_scroll()
            .flex_shrink_0()
            .bg(cx
                .theme()
                .sidebar
                .opacity(if self.display_preferences.opaque {
                    1.0
                } else {
                    0.30
                }))
            .text_color(theme::sidebar_foreground(cx.theme().is_dark()))
            // The resizable panel handle supplies the single sidebar divider.
            .px(px(10.))
            .flex()
            .flex_col()
            .child(
                div()
                    .h(px(52.))
                    .flex_shrink_0()
                    .mx(px(-10.))
                    .window_control_area(WindowControlArea::Drag),
            )
            .child(
                div()
                    .h(px(30.))
                    .flex_shrink_0()
                    .mb(px(6.))
                    .px(px(9.))
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .window_control_area(WindowControlArea::Drag)
                    .text_size(px(14.))
                    .child(img("newport.png").size(px(22.)).rounded(px(5.)))
                    .child("Newport"),
            )
            .child(self.server_selector(compact, cx))
            .child(
                div()
                    .id("sidebar-navigation-scroll")
                    .min_h_0()
                    .overflow_y_scroll()
                    .child(self.navigation(cx)),
            )
            .child(div().flex_1())
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_shrink_0()
                    .gap(px(3.))
                    .pt(px(if compact { 4. } else { 16. }))
                    .pb(px(8.))
                    .child(
                        Button::new("settings")
                            .accessibility_label("Settings")
                            .disabled(self.server_form.is_some() || self.modal_open)
                            .selected(self.page == Page::Settings)
                            .child(
                                div()
                                    .w_full()
                                    .flex()
                                    .items_center()
                                    .gap(px(9.))
                                    .text_size(px(13.))
                                    .child(Icon::new(IconName::Settings2).size(px(16.)))
                                    .child("Settings"),
                            )
                            .custom(navigation::sidebar_action_style(
                                self.page == Page::Settings,
                                cx,
                            ))
                            .h(px(32.))
                            .rounded(px(6.))
                            .px(px(8.))
                            .text_color(theme::sidebar_action(cx.theme().is_dark()))
                            .font_weight(if cx.theme().is_dark() {
                                FontWeight::NORMAL
                            } else {
                                FontWeight::MEDIUM
                            })
                            .justify_start()
                            .text_size(px(13.))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.navigate(Page::Settings, cx);
                            })),
                    )
                    .child(
                        Button::new("add-server")
                            .accessibility_label("Add Server")
                            .disabled(self.server_form.is_some() || !self.configuration_available())
                            .child(
                                div()
                                    .w_full()
                                    .flex()
                                    .items_center()
                                    .gap(px(9.))
                                    .text_size(px(13.))
                                    .child(Icon::new(IconName::Plus).size(px(16.)))
                                    .child("Add Server"),
                            )
                            .custom(navigation::sidebar_action_style(false, cx))
                            .h(px(32.))
                            .rounded(px(6.))
                            .px(px(8.))
                            .text_color(theme::sidebar_action(cx.theme().is_dark()))
                            .font_weight(if cx.theme().is_dark() {
                                FontWeight::NORMAL
                            } else {
                                FontWeight::MEDIUM
                            })
                            .justify_start()
                            .text_size(px(13.))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.edit_server(None, window, cx)
                            })),
                    ),
            )
            .child(
                div()
                    .border_t_1()
                    .flex_shrink_0()
                    .border_color(cx.theme().border)
                    .px(px(8.))
                    .pt(px(10.))
                    .pb(px(14.))
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(div().size(px(5.)).rounded_full().bg(if connected > 0 {
                        theme::success(cx.theme().is_dark())
                    } else {
                        theme::sidebar_muted(cx.theme().is_dark())
                    }))
                    .child(
                        div()
                            .child(if connected > 0 {
                                format!(
                                    "{connected} active tunnel{}",
                                    if connected == 1 { "" } else { "s" }
                                )
                            } else {
                                "No active tunnels".into()
                            })
                            .text_size(px(11.)),
                    ),
            )
            .context_menu(move |menu, _, cx| shell::server_menu(menu, menu_view.clone(), cx))
            .into_any_element()
    }
    fn status(&self, key: &str, cx: &App) -> AnyElement {
        let Some(r) = self.resources.get(key) else {
            return div().into_any_element();
        };
        if let Some(error) = &r.error {
            return div()
                .id(SharedString::from(format!("{key}-error")))
                .role(accesskit::Role::Alert)
                .aria_label(error.clone())
                .p_3()
                .bg(cx.theme().danger.opacity(0.12))
                .text_color(cx.theme().danger)
                .child(error.clone())
                .into_any_element();
        }
        if r.pending && (r.value.is_null() || key.ends_with("-more")) {
            if self.page == Page::Projects
                && (key.starts_with("git-") || key.starts_with("project-") || key == "repository")
            {
                return project_loading::resource(key, self.git.tab);
            }
            return loading::lines(key, 3);
        }
        div().into_any_element()
    }
}
impl Render for Newport {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.sync_services(window, cx);
        self.sync_containers(window, cx);
        self.sync_settings_ui(window, cx);
        self.sync_overview_processes(window, cx);
        self.sync_connections(window, cx);
        self.sync_git_drafts(window, cx);
        self.sync_git_changes(window, cx);
        self.sync_git_listing_filter(window, cx);
        self.sync_repository_settings(window, cx);
        self.sync_tag_push(window, cx);
        self.sync_project_dialog(window, cx);
        self.sync_worktree_creation(window, cx);
        self.sync_git_branch_search(window, cx);
        self.sync_branch_manage(window, cx);
        self.sync_file_browser(window, cx);
        self.sync_picker(window, cx);
        if std::mem::take(&mut self.git_notice_clear) {
            window.remove_notification::<git_mutation::GitNotice>(cx);
        }
        if let Some(notification) = self.notification.take() {
            cx.defer_in(window, move |_, window, cx| {
                window.push_notification(notification, cx)
            });
        }
        if self.page == Page::Terminal
            && self.configuration_available()
            && self.selected_server().is_some()
            && !self.terminals.contains_key(&self.server)
        {
            let server = self.server.clone();
            let config = self.selected_server().expect("validated server");
            let endpoint = format!("{}@{}", text(&config, "sshUser"), text(&config, "sshHost"));
            let scope = terminal_scope::connection(
                &self.selected_server().expect("validated server"),
                &self.snapshot,
            );
            let workspace = cx
                .new(|cx| terminal_workspace::Workspace::new(server.clone(), scope, endpoint, cx));
            self.terminals.insert(server, workspace);
        }
        let viewport = f32::from(window.viewport_size().width);
        for (server, workspace) in &self.terminals {
            if self.page != Page::Terminal || server != &self.server || self.server_form.is_some() {
                workspace.update(cx, |workspace, cx| workspace.deactivate(window, cx));
            }
        }
        if self.page != Page::Terminal
            || self.selected_server().is_none()
            || self.server_form.is_some()
            || !self.configuration_available()
        {
            self.terminal_immersive = false;
        }
        let immersive = self.terminal_immersive;
        if let Some(workspace) = self.terminals.get(&self.server) {
            workspace.update(cx, |workspace, cx| workspace.set_immersive(immersive, cx));
        }
        let show_sidebar = self.sidebar_visible && !immersive;
        self.sidebar_layout.prepare(viewport, cx);
        self.content_width = viewport
            - if show_sidebar {
                self.sidebar_layout.width(viewport)
            } else {
                0.
            };
        let title = self.snapshot["config"]["servers"]
            .as_array()
            .and_then(|a| a.iter().find(|s| s["id"] == self.server))
            .map(|s| {
                format!(
                    "{} · {}@{}",
                    server_picker::display_name(s),
                    text(s, "sshUser"),
                    text(s, "sshHost")
                )
            })
            .unwrap_or_else(|| "Connect to Newport".into());
        let awaiting_agent = self.snapshot["runtime"]["awaitingAgent"]
            .as_array()
            .is_some_and(|servers| servers.iter().any(|server| server == &self.server));
        let health = if awaiting_agent {
            "awaiting-agent"
        } else {
            self.snapshot["runtime"]["health"][&self.server]
                .as_str()
                .unwrap_or("unknown")
        };
        let health_label = match health {
            "awaiting-agent" => "Waiting for SSH approval",
            "reachable" => "Reachable",
            "unreachable" => "Unreachable",
            "error" => "Connection failed",
            "checking" => "Checking",
            _ => "Connection unknown",
        };
        let health_color = match health {
            "reachable" => theme::success(cx.theme().is_dark()),
            "unreachable" | "error" => theme::error(cx.theme().is_dark()),
            _ => cx.theme().muted_foreground,
        };
        let title = if self.page == Page::Settings {
            "Application preferences".to_owned()
        } else {
            title
        };
        let page_heading = if self.server.is_empty() && self.page != Page::Settings {
            "Servers"
        } else {
            self.page.title()
        };
        let content = div()
            .bg(cx.theme().background)
            .w_full()
            .flex_1()
            .min_w_0()
            .h_full()
            .overflow_hidden()
            .flex()
            .flex_col()
            .child(
                div()
                    .id("workspace-header")
                    .h(px(52.))
                    .flex_shrink_0()
                    .pl(px(
                        if !self.sidebar_visible
                            && cfg!(target_os = "macos")
                            && !window.is_fullscreen()
                        {
                            96.
                        } else {
                            12.
                        },
                    ))
                    .pr(px(if viewport <= 700. { 10. } else { 16. }))
                    .bg(rgb(if cx.theme().is_dark() {
                        0x2a292d
                    } else {
                        0xf5f4f6
                    }))
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(px(if viewport <= 700. { 7. } else { 10. }))
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(if viewport <= 700. { 7. } else { 10. }))
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .child(
                                crate::controls::icon_button("toggle-sidebar")
                                    .icon(Icon::new(IconName::PanelLeft).size(px(18.)))
                                    .accessibility_label(if self.sidebar_visible {
                                        "Hide sidebar"
                                    } else {
                                        "Show sidebar"
                                    })
                                    .ghost()
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.sidebar_visible = !this.sidebar_visible;
                                        cx.notify();
                                    })),
                            )
                            .child(
                                div()
                                    .id("workspace-header-drag-region")
                                    .debug_selector(|| "workspace-header-drag-region".into())
                                    // Native hit testing must never mark the sidebar
                                    // toggle or the trailing action buttons as caption.
                                    .window_control_area(WindowControlArea::Drag)
                                    .flex()
                                    .flex_col()
                                    .h_full()
                                    .justify_center()
                                    .gap(px(2.))
                                    .min_w_0()
                                    .flex_1()
                                    .child(
                                        div()
                                            .flex()
                                            .flex_wrap()
                                            .items_center()
                                            .gap_x(px(10.))
                                            .gap_y(px(5.))
                                            .child(
                                                div()
                                                    .id("workspace-page-heading")
                                                    .aria_label(page_heading)
                                                    .role(accesskit::Role::Heading)
                                                    .aria_level(1)
                                                    .font_weight(FontWeight::SEMIBOLD)
                                                    .child(page_heading),
                                            )
                                            .when(
                                                !self.server.is_empty()
                                                    && self.page != Page::Settings,
                                                |row| {
                                                    row.child(
                                                        div()
                                                            .id("server-connection-status")
                                                            .role(accesskit::Role::Status)
                                                            .aria_label(health_label)
                                                            .flex()
                                                            .flex_shrink_0()
                                                            .items_center()
                                                            .gap(px(6.))
                                                            .text_size(px(10.))
                                                            .text_color(health_color)
                                                            .child(
                                                                div()
                                                                    .size(px(5.))
                                                                    .rounded_full()
                                                                    .bg(health_color),
                                                            )
                                                            .child(health_label),
                                                    )
                                                },
                                            ),
                                    )
                                    .child(
                                        div()
                                            .child(title)
                                            .text_color(cx.theme().muted_foreground)
                                            .text_size(px(11.))
                                            .truncate(),
                                    ),
                            ),
                    )
                    .child(self.shell_actions(cx))
                    .when(immersive, |header| header.hidden()),
            )
            .when(!self.initial_loading("snapshot"), |view| {
                view.child(self.status("snapshot", cx))
            })
            .when_some(
                self.snapshot["loadError"]
                    .as_str()
                    .filter(|error| !error.is_empty()),
                |content, error| {
                    content.child(
                        div()
                            .p_3()
                            .bg(cx.theme().danger.opacity(0.12))
                            .child(format!("Could not load your configuration: {error}")),
                    )
                },
            )
            .when(
                !matches!(
                    self.page,
                    Page::Overview
                        | Page::Ports
                        | Page::Settings
                        | Page::Services
                        | Page::Containers
                        | Page::Files
                ) && !self.initial_loading("page"),
                |view| view.child(self.status("page", cx)),
            )
            .when_some(self.notice.clone(), |s, n| {
                s.child(div().px_4().py_2().bg(cx.theme().muted).child(n))
            })
            .child(self.screen(window, cx));
        div()
            .id("application-workspace")
            .role(accesskit::Role::Pane)
            .a11y_synthetic_children({
                let modal_open = self.modal_open;
                move |builder| {
                    if modal_open {
                        builder.parent_node().set_hidden();
                    }
                }
            })
            .size_full()
            .flex()
            .text_color(cx.theme().foreground)
            .font_family(crate::theme::UI_FONT)
            .text_size(px(13.))
            .track_focus(&self.focus)
            .when(cfg!(not(target_os = "macos")), |shell| {
                shell.on_action(cx.listener(|_, _: &About, window, cx| {
                    #[cfg(not(target_os = "macos"))]
                    about::show(window, cx);
                    #[cfg(target_os = "macos")]
                    let _ = (window, cx);
                }))
            })
            .on_action(
                cx.listener(|_, _: &CloseWindow, window, cx| window_activation::hide(window, cx)),
            )
            .on_action(cx.listener(|_, _: &MinimizeWindow, window, _| window.minimize_window()))
            .on_action(cx.listener(|_, _: &ZoomWindow, window, _| window.zoom_window()))
            .on_action(cx.listener(|_, _: &ToggleFullScreen, window, _| window.toggle_fullscreen()))
            .on_action(cx.listener(|this, _: &SidebarWider, window, cx| {
                this.resize_sidebar(10., false, window, cx)
            }))
            .on_action(cx.listener(|this, _: &SidebarNarrower, window, cx| {
                this.resize_sidebar(-10., false, window, cx)
            }))
            .on_action(cx.listener(|this, _: &ResetSidebar, window, cx| {
                this.resize_sidebar(0., true, window, cx)
            }))
            .on_action(cx.listener(|this, _: &ToggleSidebar, window, cx| {
                if window.has_active_dialog(cx) {
                    return;
                }
                this.sidebar_visible = !this.sidebar_visible;
                cx.notify();
            }))
            .on_action(
                cx.listener(|this, _: &ToggleTerminalImmersive, window, cx| {
                    if this.page == Page::Terminal
                        && this.selected_server().is_some()
                        && this.server_form.is_none()
                        && this.configuration_available()
                        && !window.has_active_dialog(cx)
                    {
                        this.terminal_immersive = !this.terminal_immersive;
                        this.focus.focus(window, cx);
                        cx.notify();
                    }
                }),
            )
            .on_action(cx.listener(|this, _: &OpenSettings, window, cx| {
                if !window.has_active_dialog(cx) && this.server_form.is_none() {
                    this.navigate(Page::Settings, cx);
                    this.focus.focus(window, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &FindInView, window, cx| {
                if this.page == Page::Projects
                    && this.server_form.is_none()
                    && !window.has_active_dialog(cx)
                {
                    this.open_git_find(window, cx);
                    return;
                }
                if this.page == Page::Terminal
                    && this.server_form.is_none()
                    && !window.has_active_dialog(cx)
                    && let Some(workspace) = this.terminals.get(&this.server)
                {
                    workspace.update(cx, |workspace, cx| workspace.open_find(window, cx));
                }
            }))
            .on_action(
                cx.listener(|this, action: &navigation::OpenView, window, cx| {
                    if !window.has_active_dialog(cx)
                        && this.server_form.is_none()
                        && let Some(page) = navigation::shortcut_page(action.number)
                    {
                        this.navigate(page, cx);
                        // The previous page may own the focused terminal/input.
                        // Do not leave global shortcuts on an unmounted focus path.
                        this.focus.focus(window, cx);
                    }
                }),
            )
            .on_action(cx.listener(|this, _: &NewServer, window, cx| {
                if this.configuration_available() {
                    this.edit_server(None, window, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &EditServer, window, cx| {
                if let Some(server) = this.selected_server() {
                    this.edit_server(Some(server), window, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &TestServer, window, cx| {
                if !window.has_active_dialog(cx) {
                    this.test_server(cx);
                }
            }))
            .on_action(cx.listener(|this, _: &Refresh, window, cx| {
                if window.has_active_dialog(cx) {
                    return;
                }
                this.request("snapshot", "snapshot", json!({}));
                if this.page == Page::Ports {
                    this.discover_connections(true);
                } else {
                    this.refresh();
                }
                cx.notify();
            }))
            .child(if show_sidebar {
                self.sidebar_group(cx)
                    .child(
                        self.sidebar_layout.sidebar_panel(viewport).child(
                            self.sidebar(f32::from(window.viewport_size().height) <= 560., cx),
                        ),
                    )
                    .child(self.sidebar_layout.content_panel().child(content))
                    .on_resize(cx.listener(|this, _, window, cx| {
                        if let Err(error) = this
                            .sidebar_layout
                            .capture_resize(f32::from(window.viewport_size().width), cx)
                        {
                            this.notification = Some(Notification::error(format!(
                                "Could not save sidebar width: {error}"
                            )));
                        }
                        cx.notify();
                    }))
                    .into_any_element()
            } else {
                content.into_any_element()
            })
    }
}
/// Run the UI against the configured backend, as a separate client.
pub fn run() -> Result<(), String> {
    run_with(|_| {})
}

/// Reveal the existing workspace. Used by native reopen and the application host.
pub fn activate(cx: &mut App) -> Result<(), String> {
    let windows = cx.windows();
    if windows.is_empty() {
        return Err("No Newport window is available".into());
    }
    cx.activate(true);
    for handle in windows {
        handle
            .update(cx, |_, window, _| {
                window_activation::restore(window)?;
                window.activate_window();
                Ok::<_, String>(())
            })
            .map_err(|error| error.to_string())??;
    }
    Ok(())
}
mod window_activation;

/// Application-owned entry points (for example, the tray) use the same Settings
/// action as the app menu, preserving draft and navigation behavior.
pub fn open_settings(cx: &mut App) -> Result<(), String> {
    activate(cx)?;
    for handle in cx.windows() {
        handle
            .update(cx, |_, window, cx| {
                window.dispatch_action(Box::new(OpenSettings), cx);
            })
            .map_err(|error| error.to_string())?;
    }
    Ok(())
}

/// Install application-owned desktop dispatch before opening the UI. Backend
/// services and their runtime remain outside this crate.
pub fn run_with(setup: impl FnOnce(&mut App) + 'static) -> Result<(), String> {
    run_with_options(LaunchOptions::default(), setup)
}

/// Presentation options supplied by the application host. OS login registration
/// and command-line policy remain outside the UI crate.
#[derive(Clone, Copy, Default)]
pub struct LaunchOptions {
    pub start_hidden: bool,
}

pub fn run_with_options(
    options: LaunchOptions,
    setup: impl FnOnce(&mut App) + 'static,
) -> Result<(), String> {
    run_with_options_and_ready(options, setup, || {})
}

/// Notify the host after the root window has been constructed and the event
/// loop resumes, including hidden login startup. This is not a paint guarantee.
pub fn run_with_options_and_ready(
    options: LaunchOptions,
    setup: impl FnOnce(&mut App) + 'static,
    ready: impl FnOnce() + 'static,
) -> Result<(), String> {
    let _agent_relay = match newport_client::agent_relay::start(&backend::Backend::discover()) {
        Ok(relay) => relay,
        Err(error) => return Err(format!("Could not start native SSH agent relay: {error}")),
    };
    if let Err(error) = grammars::init() {
        eprintln!("Syntax highlighting unavailable: {error}");
    }
    let app = gpui_kit::application().with_assets(assets::Assets);
    app.on_reopen(|cx| {
        let _ = activate(cx);
    });
    let startup_error = std::rc::Rc::new(std::cell::RefCell::new(None));
    let failed_startup = startup_error.clone();
    app.run(move |cx| {
        setup(cx);
        gpui_kit::init(cx);
        theme::install(
            matches!(
                cx.window_appearance(),
                WindowAppearance::Dark | WindowAppearance::VibrantDark
            ),
            cx,
        );
        cx.on_action(|_: &Quit, cx| cx.quit());
        navigation::install_desktop_commands(cx);
        cx.on_window_closed(|cx, _| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();
        let window_state = window_state::WindowState::load(cx);
        let bounds = window_state.bounds();
        let opened = cx.open_window(
            WindowOptions {
                show: !options.start_hidden,
                focus: !options.start_hidden,
                window_bounds: Some(bounds),
                display_id: window_state.display_id(),
                window_min_size: Some(size(px(640.), px(480.))),
                window_background: WindowBackgroundAppearance::Blurred,
                titlebar: Some(TitlebarOptions {
                    title: Some("Newport — Native".into()),
                    // On Windows transparency removes the system caption and
                    // its controls. Keep native minimize/maximize/close until
                    // the shell explicitly provides a replacement title bar.
                    appears_transparent: !cfg!(target_os = "windows"),
                    traffic_light_position: cfg!(target_os = "macos")
                        .then(|| point(px(9.), px(9.))),
                }),
                ..Default::default()
            },
            move |window, cx| {
                // Closing this client preserves its workspace and drafts,
                // like the Tauri shell. Explicit Quit still exits normally.
                window.on_window_should_close(cx, |window, cx| {
                    window_activation::hide(window, cx);
                    false
                });
                window.set_rem_size(px(14.));
                let view = cx.new(|cx| Newport::new(window, window_state, cx));
                cx.new(|cx| {
                    gpui_kit::base::Root::new(view, window, cx).bg(gpui_kit::transparent_black())
                })
            },
        );
        if let Err(error) = opened {
            *failed_startup.borrow_mut() = Some(format!("Could not open Newport: {error}"));
            cx.quit();
            return;
        }
        cx.defer(move |_| ready());
        if !options.start_hidden {
            cx.activate(true);
        }
    });
    startup_error.borrow_mut().take().map_or(Ok(()), Err)
}

/// Validate all bundled grammar libraries without opening a window or backend.
pub fn check_grammars() -> Result<(), String> {
    grammars::validate_all()
}
