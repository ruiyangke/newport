use newport_protocol::desktop::{self, configuration};
#[cfg(test)]
mod account_fixture;
mod grammars;
pub mod remote_app_catalog;
pub mod remote_app_focus;
pub mod remote_app_tile;
mod remote_apps;
mod remote_apps_api;
#[cfg(test)]
mod remote_apps_fixture;
mod remote_apps_install;
mod remote_apps_sessions;
pub mod separate_apps;
pub mod start_menu;
mod terminal_gui;
mod terminal_setup;
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
mod clipboard_files;
pub use clipboard_files::{ClipboardFilesNotice, ClipboardFilesQuestion};
mod appearance;
use appearance::ResolveAppearance;
mod assets;
mod connection_batches;
mod connections;
mod connections_grouped;
mod connections_hover;
mod connections_process_icon;
mod connections_rows;
mod connections_selector;
pub mod connectivity;
mod containers;
mod containers_host;
mod controls;
#[cfg(test)]
mod desktop_rpc_fixture;
mod hover;
mod ui;
use hover::HoverExt as _;
mod connection_wait;
mod file_icon;
mod file_icon_assets;
mod file_image;
mod file_preview;
#[cfg(test)]
mod file_test_backend;
mod file_transfer;
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
mod git_index_refresh;
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
mod integration_needle;
mod needle_required;
mod request_dispatch;
mod server_setup;
mod setup_preview;
mod welcome;
pub use setup_preview::run as run_setup_preview;
mod empty;
mod loading;
mod localized_time;
mod log_line;
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
mod project_documents;
mod project_editor;
mod project_folder;
mod project_library;
mod project_library_rows;
mod project_library_state;
mod project_loading;
mod project_preferences;
mod project_quick_open;
mod project_workspace;
mod quiet_action;
mod screens;
mod selectable_log;
mod server_form;

/// A text input's whole field. Kit's accessibility snapshot of a text input is
/// its inner text area; the field is the input element around it.
#[cfg(test)]
fn field_frame(
    window: &gpui_kit::Window,
    input: &gpui_kit::base::test_support::ElementSnapshot,
) -> gpui_kit::Bounds<gpui_kit::Pixels> {
    let path = input.path();
    let at = path
        .iter()
        .rposition(|id| matches!(id, gpui_kit::ElementId::NamedInteger(name, _) if name == "input"))
        .expect("text input inside an input field");
    gpui_kit::base::test_support::find(window, &path[..at], &path[at])
        .expect("rendered input field")
        .bounds()
}

/// Exercise the platform's real editing bindings in shared UI fixtures.
#[cfg(test)]
fn test_shortcuts(cx: &mut gpui_kit::VisualTestContext, mac_keys: &str) {
    if cfg!(target_os = "macos") {
        cx.simulate_keystrokes(mac_keys);
    } else {
        cx.simulate_keystrokes(&mac_keys.replace("cmd-", "ctrl-"));
    }
}
#[cfg(test)]
mod git_fixture;
mod server_picker;
mod services;
mod services_host;
mod settings;
mod shell;
mod sidebar_account;
mod sidebar_layout;
#[cfg(test)]
mod snapshot_fixture;
mod snapshot_presentation;
mod state;
mod terminal_agent;
mod terminal_color;
mod terminal_paint;
mod terminal_search_paint;
mod terminal_session_menu;
mod terminal_view;
mod terminal_workspace;
mod theme;
mod tunnel_actions;
mod tunnel_clean;
mod tunnel_delete;
mod tunnel_form;
mod ui_state;
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
    RemoteApps,
}
impl Page {
    const ALL: [(Self, &'static str); 10] = [
        (Self::Overview, "Overview"),
        (Self::Ports, "Connections"),
        (Self::Projects, "Projects"),
        (Self::Services, "Services"),
        (Self::Containers, "Containers"),
        (Self::Terminal, "Terminal"),
        (Self::Files, "Files"),
        (Self::Integration, "Integrations"),
        (Self::Settings, "Settings"),
        (Self::RemoteApps, "Remote apps"),
    ];
    fn title(self) -> &'static str {
        Self::ALL.iter().find(|(p, _)| *p == self).unwrap().1
    }
}
struct Completion {
    server: String,
    key: String,
    generation: u64,
    result: Outcome,
}
/// A finished request's result. A request sent as
/// [`request_dispatch::Request::Typed`] hands its decoded reply to the key's
/// owner, a Git read its checked reply, a Git log poll its rendered commands,
/// a file preview its decoded image, an added project its saved bookmark and
/// a port discovery its prepared reading.
enum Outcome {
    Discovery(connections::DiscoveryCompletion),
    Typed(Result<desktop::Reply, newport_client::request_error::RequestError>),
    Git(Result<newport_protocol::git::Reply, newport_client::request_error::RequestError>),
    Project(Result<desktop::preferences::Project, newport_client::request_error::RequestError>),
    GitLog(Result<git_log_text::LogBatch, newport_client::request_error::RequestError>),
    Preview(Result<file_image::Prepared, newport_client::request_error::RequestError>),
}
struct Work {
    key: String,
    generation: u64,
    request: request_dispatch::Request,
}
struct Newport {
    /// Saves where the window and workspace were left, off the UI thread.
    ui_state: ui_state::Writer,
    terminal_immersive: bool,
    terminal_setup: terminal_setup::State,
    window_state: window_state::WindowState,
    terminals: std::collections::HashMap<String, Entity<terminal_workspace::Workspace>>,
    integration_refresh_pending: std::collections::HashSet<String>,
    needle_details: std::collections::HashMap<String, configuration::ConnectionScope>,
    needle_uninstall_confirmations:
        std::collections::HashMap<String, integration_needle::Confirmation>,
    integration_shell: Entity<integration::ShellCode>,
    integration_scroll: ScrollHandle,
    server_form: Option<Entity<server_form::ServerForm>>,
    modal_open: bool,
    /// The quit confirmation is showing, so a second Quit adds no other.
    quit_prompt: std::rc::Rc<std::cell::Cell<bool>>,
    server_picker: Entity<server_picker::Picker>,
    picker_items: Vec<server_picker::ServerItem>,
    picker_selection: String,
    page: Page,
    workspace_page: Page,
    remote_apps: Option<Entity<remote_apps::Launchpad>>,
    remote_apps_refresh: bool,
    /// Counts returns to Remote apps; each return resets the page's view state.
    remote_apps_visit: u64,
    remote_apps_away: bool,
    /// The page was just returned to; its readings are requested again.
    remote_apps_returned: bool,
    /// The selected server's connected transport generation when last polled.
    remote_apps_transport: Option<(String, Option<u64>)>,
    /// Preference changes not yet sent, and keys with one in flight.
    remote_apps_mutations: std::collections::VecDeque<remote_apps::Mutation>,
    /// The typed replies of the Remote apps reads in `resources`.
    remote_apps_readings: remote_apps::Readings,
    remote_apps_mutating: std::collections::HashSet<String>,
    /// A status-only read asked for, and whether it replaces one in flight.
    remote_apps_recheck: Option<bool>,
    /// The server and status reading the installed-servers record was last
    /// saved for, so a failed save isn't retried every tick.
    remote_apps_record_tried: Option<(String, u64)>,
    restore_git: Option<workspace_state::GitDestination>,
    git_destinations: std::collections::BTreeMap<String, workspace_state::GitDestination>,
    appearance: appearance::Appearance,
    settings_state: newport_client::settings::Settings,
    settings_ui: settings::SettingsUi,
    account: newport_client::account::Account,
    services_panels: std::collections::HashMap<String, services_host::HostedServices>,
    services_preferences: std::collections::HashMap<String, services::Preferences>,
    retired_services: Vec<Entity<services::ServicesPanel>>,
    services_refresh_requested: bool,
    connections_state: std::collections::HashMap<String, connections::State>,
    tunnel_operations: newport_client::tunnel_operations::Operations,
    setup_flow: Option<server_setup::Flow>,
    welcome: welcome::Welcome,
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
    /// The backend's configuration and runtime state; `None` until the first
    /// snapshot arrives.
    snapshot: Option<configuration::Snapshot>,
    /// When the snapshot's data last changed, in milliseconds since the Unix
    /// epoch. An identical read keeps it, so it adds no new presentation.
    snapshot_updated_at: Option<u64>,
    resources: Resources,
    work: request_dispatch::Dispatcher,
    replies: mpsc::Receiver<Completion>,
    last_refresh: Instant,
    /// The tick's last check for loading placeholders that fell due.
    placeholders_checked_at: Instant,
    window_visible: bool,
    focus: FocusHandle,
    navigation_focus: Vec<FocusHandle>,
    sidebar_focus: FocusHandle,
    file_browsers: std::collections::HashMap<String, files::Browser>,
    files_refresh_requested: bool,
    file_preview_path: Option<String>,
    /// The selected file's prepared preview; read it through
    /// [`Self::file_preview_data`], which checks its read is still shown.
    file_preview: Option<file_image::Prepared>,
    /// The places panel's saved projects; read them through [`Self::owned`]
    /// with the `file-places` key.
    file_places: Option<desktop::preferences::Projects>,
    file_preview_viewport: file_preview::Viewport,
    file_transfer: Option<file_transfer::Transfer>,
    file_chooser: Option<file_transfer::FileChooser>,
    file_project_connection: Option<configuration::ConnectionScope>,
    /// Decoded connection replies by request key; read them through
    /// [`Self::connection_reply`].
    connection_replies: std::collections::HashMap<String, desktop::connection::Reply>,
    /// The saved project whose checkout Projects shows.
    selection: Option<desktop::preferences::Project>,
    project_add: Option<project_add::Add>,
    project_edit: Option<project_library::Edit>,
    project_create: Option<project_create::Create>,
    project_creation_save_owner: Option<(uuid::Uuid, String)>,
    project_folder: Option<project_folder::Chooser>,
    /// The folder chooser's listing; read it through [`Self::owned`] with the
    /// `project-folder` key.
    project_folder_listing: Option<desktop::files::Listing>,
    project_dialog_open: bool,
    project_dialog_feedback: project_dialog::FeedbackScope,
    project_dialog_rendering: bool,
    project_checks: Option<project_checks::Batch>,
    project_libraries: std::collections::BTreeMap<String, project_library_state::State>,
    project_preferences: std::collections::BTreeMap<String, project_preferences::Preferences>,
    git: git::Workspace,
    code_workspaces: Vec<project_editor::Retained>,
    git_diff_layouts: std::collections::HashMap<String, git_diff_split::Layout>,
    git_mutation: Option<git_mutation::Mutation>,
    git_notice_clear: bool,
    git_log: git_log::Log,
    git_drafts: std::collections::HashMap<git_commit::DraftKey, git_commit::Draft>,
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
        FindInTerminal,
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
/// Now, in milliseconds since the Unix epoch, for a query's update time.
fn now_millis() -> Option<u64> {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .and_then(|elapsed| u64::try_from(elapsed.as_millis()).ok())
}
fn label(value: impl Into<SharedString>) -> Div {
    div()
        .text_size(px(crate::ui::FONT_SIZE))
        .opacity(0.7)
        .child(value.into())
}
fn cell(value: impl Into<SharedString>, width: f32) -> Div {
    div()
        .w(px(width))
        .flex_shrink_0()
        .overflow_hidden()
        .child(value.into())
}
fn bytes(value: u64) -> String {
    byte_size(value as f64)
}
/// A byte count in binary units.
fn byte_size(n: f64) -> String {
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
fn desktop_server(value: &str) -> Result<uuid::Uuid, newport_client::request_error::RequestError> {
    newport_client::desktop_inputs::identity(value, "server identity")
        .map_err(|error| error.to_string().into())
}
impl Newport {
    /// Saves window state and closes this window's terminals.
    fn quitting(&mut self, cx: &mut Context<Self>) -> impl Future<Output = ()> + use<> {
        self.save_workspace_selection();
        self.ui_state.window(self.window_state.placement());
        self.ui_state.flush();
        let ids: Vec<_> = self
            .terminals
            .values()
            .flat_map(|workspace| workspace.read(cx).session_ids(cx))
            .chain(self.project_terminal_sessions(cx))
            .collect();
        let tasks: Vec<_> = ids
            .into_iter()
            .map(|id| {
                cx.background_spawn(async move {
                    let _ = backend::Backend::discover().execute_with_timeout(
                        desktop::Operation::Terminal(Box::new(
                            desktop::terminal::Operation::Close(Box::new(
                                desktop::terminal::CloseArgs { session: id },
                            )),
                        )),
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
    }
    fn new(
        window: &mut Window,
        window_state: window_state::WindowState,
        saved: ui_state::UiState,
        cx: &mut Context<Self>,
    ) -> Self {
        Self::observe_connectivity(cx);
        connectivity::observe_latency(cx, |this: &Self| &this.server).detach();
        Self::observe_modal_visibility(window, cx);
        cx.observe_window_bounds(window, |this, window, cx| {
            this.window_state.capture(window, cx)
        })
        .detach();
        cx.on_app_quit(Self::quitting).detach();
        // Read once, before the first frame; saved through the backend.
        let (preferences, preferences_error) = newport_client::window_preferences::load(
            newport_client::profile::directory().as_deref(),
        );
        let sidebar_layout = sidebar_layout::SidebarLayout::new(
            preferences.sidebar_width,
            preferences.sidebar_compact,
            cx,
        );
        let appearance = preferences.appearance;
        theme::install(appearance.is_dark(window.appearance()), cx);
        cx.set_global(terminal_view::OptionAsMeta(
            preferences.terminal_option_as_meta,
        ));
        cx.observe_window_appearance(window, |this, window, cx| {
            theme::install(this.appearance.is_dark(window.appearance()), cx);
            cx.notify();
        })
        .detach();
        // Polling follows visibility, not keyboard focus. A visible but
        // inactive window keeps polling.
        cx.observe_window_visibility(window, |this, visibility, _, cx| {
            this.window_visibility_changed(visibility.is_visible(), cx);
        })
        .detach();
        // Returning from the browser (after buying Pro, say) shows the current plan.
        cx.observe_window_activation(window, |this, window, _| {
            if window.is_window_active() {
                this.account.refresh();
            }
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
        let restored = saved.workspace.clone();
        cx.set_global(terminal_workspace::RestoredTabs(
            saved.terminals.clone().into_iter().collect(),
        ));
        cx.set_global(project_editor::RestoredFiles(saved.files.clone()));
        let ui_state = ui_state::Writer::start(saved);
        cx.set_global(terminal_workspace::TabsSaver(ui_state.clone()));
        cx.set_global(terminal_workspace::HeldShellsSource(
            newport_client::terminal_transport::held_shells,
        ));
        let chart_focus = cx.focus_handle();
        cx.on_blur(&chart_focus, window, |this, _, cx| {
            this.overview_state.inspection = None;
            this.overview_state.keyboard_inspection = false;
            cx.notify();
        })
        .detach();
        let mut this = Self {
            ui_state,
            window_state,
            terminals: Default::default(),
            terminal_immersive: false,
            terminal_setup: Default::default(),
            integration_refresh_pending: Default::default(),
            needle_details: Default::default(),
            needle_uninstall_confirmations: Default::default(),
            integration_shell: cx.new(|cx| integration::ShellCode::new(window, cx)),
            integration_scroll: ScrollHandle::new(),
            server_form: None,
            modal_open: false,
            quit_prompt: Default::default(),
            server_picker: Self::init_picker(window, cx),
            picker_items: Vec::new(),
            picker_selection: String::new(),
            page: restored.page,
            workspace_page: restored.workspace_page,
            restore_git: restored.git,
            git_destinations: restored.git_destinations,
            appearance,
            settings_state: newport_client::settings::Settings::new(backend::Backend::discover()),
            settings_ui: settings::SettingsUi::new(appearance, window, cx),
            account: newport_client::account::Account::new(backend::Backend::discover()),
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
            snapshot: None,
            snapshot_updated_at: None,
            resources: Resources::default(),
            work,
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
            remote_apps_refresh: false,
            remote_apps_visit: 0,
            remote_apps_away: false,
            remote_apps_returned: false,
            remote_apps_transport: None,
            remote_apps_mutations: Default::default(),
            remote_apps_readings: Default::default(),
            remote_apps_mutating: Default::default(),
            remote_apps_recheck: None,
            remote_apps_record_tried: None,
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
            notification: preferences_error.map(Notification::error),
        };
        this.focus.focus(window, cx);
        this.request_snapshot();
        this
    }
    fn request_snapshot(&mut self) {
        self.request_work(
            "snapshot",
            request_dispatch::Request::Typed(Box::new(desktop::Operation::Configuration(
                Box::new(configuration::Operation::Snapshot(Box::new(
                    newport_protocol::common::Present {},
                ))),
            ))),
        );
    }
    /// Settles a configuration read. Returns whether its generation was still
    /// current, and the snapshot it delivered.
    fn complete_snapshot(
        &mut self,
        generation: u64,
        result: Result<desktop::Reply, newport_client::request_error::RequestError>,
    ) -> (bool, Option<configuration::Snapshot>) {
        let snapshot = result.and_then(newport_client::backend::snapshot_reply);
        let changed = snapshot
            .as_ref()
            .is_ok_and(|next| self.snapshot.as_ref() != Some(next));
        let updated_at = match &snapshot {
            Ok(_) if changed || self.snapshot_updated_at.is_none() => now_millis(),
            _ => self.snapshot_updated_at,
        };
        let status = newport_client::resources::QueryStatus {
            error: snapshot.as_ref().err().cloned(),
            updated_at,
            ..Default::default()
        };
        let has_data = snapshot.is_ok() || self.snapshot.is_some();
        if !self
            .resources
            .complete_query("snapshot", generation, &status, has_data, changed)
        {
            return (false, None);
        }
        self.snapshot_updated_at = updated_at;
        (true, snapshot.ok())
    }
    /// Settles a read whose typed data the page keeps in `held`, with whether
    /// it differs from the data shown. A failed read keeps that data.
    fn complete_owned<T>(
        &mut self,
        key: &str,
        generation: u64,
        result: Result<(T, bool), newport_client::request_error::RequestError>,
        held: fn(&mut Self) -> &mut Option<T>,
    ) -> bool {
        let (data, result) = match result {
            Ok((data, changed)) => (Some(data), Ok(changed)),
            Err(error) => (None, Err(error)),
        };
        let accepted = self.resources.complete_owned(key, generation, result);
        if accepted && let Some(data) = data {
            *held(self) = Some(data);
        }
        accepted
    }
    /// The data `held` keeps for `key`, while that key's read still shows it.
    fn owned<'a, T>(&'a self, key: &str, held: &'a Option<T>) -> Option<&'a T> {
        self.resources
            .get(key)
            .is_some_and(newport_client::resources::Resource::has_value)
            .then_some(held.as_ref())
            .flatten()
    }
    /// The decoded connection reply `key`'s read still shows.
    fn connection_reply(&self, key: &str) -> Option<&desktop::connection::Reply> {
        self.resources
            .get(key)
            .is_some_and(newport_client::resources::Resource::has_value)
            .then(|| self.connection_replies.get(key))
            .flatten()
    }
    /// Settles a connection or configuration request. A reply that carries
    /// data is kept for [`Self::connection_reply`]; an acknowledgement settles
    /// its key without data, as a failure does.
    fn complete_connection(
        &mut self,
        key: &str,
        generation: u64,
        result: Result<desktop::Reply, newport_client::request_error::RequestError>,
    ) -> bool {
        use desktop::connection::Reply as Connection;
        let reply = match result {
            Ok(desktop::Reply::Connection(reply)) => match *reply {
                Connection::SetupInstall(_)
                | Connection::SetupSkip(_)
                | Connection::TestConnection(_)
                | Connection::ReinstallAgent(_)
                | Connection::InspectAgent(_) => None,
                Connection::ConnectionsQuery(_) => {
                    return self.resources.complete_detailed(
                        key,
                        generation,
                        Err("Port discovery replies are read by the Ports page.".into()),
                    );
                }
                reply => Some(reply),
            },
            Ok(desktop::Reply::Configuration(reply))
                if !matches!(
                    *reply,
                    configuration::Reply::Snapshot(_) | configuration::Reply::ForwardTunnelBatch(_)
                ) =>
            {
                None
            }
            Ok(_) => {
                return self.resources.complete_detailed(
                    key,
                    generation,
                    Err("The backend answered with an unexpected reply.".into()),
                );
            }
            Err(error) => {
                return self
                    .resources
                    .complete_detailed(key, generation, Err(error));
            }
        };
        let Some(reply) = reply else {
            return self.resources.complete_detailed(key, generation, Ok(()));
        };
        let accepted = self.resources.complete_owned(key, generation, Ok(true));
        if accepted {
            self.connection_replies.insert(key.to_owned(), reply);
        }
        accepted
    }
    /// Refreshes a key whose owner reads the decoded reply, keeping its data
    /// on screen meanwhile.
    fn request_typed_refresh(
        &mut self,
        key: &str,
        operation: Result<desktop::Operation, newport_client::request_error::RequestError>,
    ) {
        let generation = self.resources.begin_refresh(key, false);
        self.dispatch_typed(key, operation, generation);
    }
    /// Starts a read whose owner reads the decoded reply.
    fn request_typed(
        &mut self,
        key: &str,
        operation: Result<desktop::Operation, newport_client::request_error::RequestError>,
    ) {
        let generation = self.resources.begin(key);
        self.dispatch_typed(key, operation, generation);
    }
    /// Sends a request whose owner reads the decoded reply, under a generation
    /// the caller began.
    fn dispatch_typed(
        &mut self,
        key: &str,
        operation: Result<desktop::Operation, newport_client::request_error::RequestError>,
        generation: Option<u64>,
    ) {
        match operation {
            Ok(operation) => self.dispatch_request(
                key,
                request_dispatch::Request::Typed(Box::new(operation)),
                generation,
            ),
            Err(error) => {
                if let Some(generation) = generation {
                    self.resources
                        .complete_detailed(key, generation, Err(error));
                }
            }
        }
    }
    fn request_work(&mut self, key: &str, request: request_dispatch::Request) {
        let generation = if key == "snapshot" {
            self.resources.begin_refresh(key, false)
        } else {
            self.resources.begin(key)
        };
        self.dispatch_request(key, request, generation);
    }
    /// Settles a read that could not be sent, as if it had failed.
    fn request_failed(&mut self, key: &str, error: newport_client::request_error::RequestError) {
        if let Some(generation) = self.resources.begin(key) {
            self.resources
                .complete_detailed(key, generation, Err(error));
        }
    }
    /// Starts a Git read whose owner reads the checked reply from
    /// [`Self::git_reply`].
    fn request_git_typed(&mut self, key: &str, request: newport_protocol::git::Request) {
        let generation = self.resources.begin(key);
        self.dispatch_git_typed(key, request, generation);
    }
    /// Sends a Git read under a generation the caller began.
    fn dispatch_git_typed(
        &mut self,
        key: &str,
        request: newport_protocol::git::Request,
        generation: Option<u64>,
    ) {
        match uuid::Uuid::parse_str(&self.server) {
            Ok(server) => self.dispatch_request(
                key,
                request_dispatch::Request::Git(Box::new(
                    newport_protocol::desktop::git::RequestArgs {
                        server,
                        request,
                        retry_connection: false,
                    },
                )),
                generation,
            ),
            Err(error) => {
                if let Some(generation) = generation {
                    self.resources.complete_detailed(
                        key,
                        generation,
                        Err(error.to_string().into()),
                    );
                }
            }
        }
    }
    /// The checked Git reply `key`'s read still shows.
    fn git_reply(&self, key: &str) -> Option<&newport_protocol::git::Reply> {
        self.resources
            .get(key)
            .and_then(newport_client::resources::Resource::git)
    }
    fn request_repository_settings(&mut self, key: &str, repo_id: String) {
        match uuid::Uuid::parse_str(&self.server) {
            Ok(server) => self.request_work(
                key,
                request_dispatch::Request::RepositorySettings { server, repo_id },
            ),
            Err(error) => self.request_failed(key, error.to_string().into()),
        }
    }
    /// Asks the backend to drop its cached reads of the open repository. The
    /// reported repository lands in [`git::Workspace::refreshed`].
    fn request_repository_refresh(&mut self, key: &str, server: uuid::Uuid) {
        let generation = self.resources.begin_refresh(key, false);
        self.dispatch_request(
            key,
            request_dispatch::Request::Typed(Box::new(desktop::Operation::Git(Box::new(
                desktop::git::Operation::RefreshRepository(Box::new(
                    desktop::git::RefreshRepositoryArgs {
                        server,
                        repo_id: self.git_repository_id(),
                    },
                )),
            )))),
            generation,
        );
    }
    fn complete_repository_refresh(
        &mut self,
        key: &str,
        generation: u64,
        result: Result<desktop::Reply, newport_client::request_error::RequestError>,
    ) -> bool {
        let result = result.and_then(|reply| match reply {
            desktop::Reply::Git(reply) => match *reply {
                desktop::git::Reply::RefreshRepository(refreshed) => Ok(refreshed.repo_id),
                _ => Err("Invalid Git response operation".into()),
            },
            _ => Err("Invalid Git response family".into()),
        });
        let changed = result
            .as_ref()
            .map(|repo| self.git.refreshed.get(key) != Some(repo))
            .map_err(Clone::clone);
        let accepted = self.resources.complete_owned(key, generation, changed);
        if accepted && let Ok(repo) = result {
            self.git.refreshed.insert(key.to_owned(), repo);
        }
        accepted
    }
    fn dispatch_request(
        &mut self,
        key: &str,
        request: request_dispatch::Request,
        generation: Option<u64>,
    ) {
        let Some(generation) = generation else {
            return;
        };
        if self
            .work
            .try_send(Work {
                key: key.into(),
                generation,
                request,
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
    fn observe_connectivity(cx: &mut Context<Self>) {
        connectivity::observe(
            cx,
            |this: &Self| &this.server,
            |this, cx| {
                this.request_snapshot();
                if connectivity::scoped(&this.server, this.snapshot.as_ref(), cx)
                    .is_some_and(|state| state.phase == connectivity::Phase::Connected)
                    && this.configuration_available()
                {
                    this.refresh();
                }
                cx.notify();
            },
        )
        .detach();
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
        if let Some(placement) = self.window_state.settle() {
            self.ui_state.window(placement);
        }
        let mut changed = branch_search_changed;
        self.sync_navigation_focus(cx);
        changed |= remote_apps::take_changed(cx);
        // A running remote app that failed: its windows just vanished, so say
        // why wherever the person is.
        if let Some(message) = remote_apps::take_notices(cx).pop() {
            self.notification = Some(Notification::error(message));
            changed = true;
        }
        self.poll_remote_apps(cx);
        changed |= self.poll_checkout_search();
        changed |= self.poll_checkout_scroll();
        changed |= changes_search_changed;
        changed |= remote_refs_changed;
        changed |= remote_search_changed;
        if !self.project_editor_active(cx) {
            changed |= self.poll_git_diff_scroll();
        }
        changed |= self.poll_git_mutation(cx);
        changed |= self.poll_tunnel_actions();
        let settings = self.settings_state.poll(self.page == Page::Settings);
        changed |= self.apply_settings_effects(settings);
        let account = self.account.poll();
        changed |= self.apply_account_effects(account, cx);
        changed |= self.poll_terminal_setup(cx);
        changed |= clipboard_files::changed(cx);
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
                (resource.updated_at.is_some() || resource.has_value())
                    .then_some(resource.presentation_revision)
            });
            let opened = match &reply.result {
                Outcome::Git(Ok(newport_protocol::git::Reply::Open(repo)))
                    if reply.key == "repository" =>
                {
                    Some(repo.repo_id.clone())
                }
                _ => None,
            };
            let mut received = None;
            let mut git_log = None;
            let mut saved_project = None;
            let accepted = match reply.result {
                Outcome::Discovery(discovery) => {
                    reply.key == "ports"
                        && self.complete_connections_reading(
                            &reply.server,
                            reply.generation,
                            discovery,
                        )
                }
                Outcome::Typed(result) if reply.key == "snapshot" => {
                    let (accepted, snapshot) = self.complete_snapshot(reply.generation, result);
                    received = snapshot;
                    accepted
                }
                Outcome::Typed(result) if reply.key == settings::WINDOW_PREFERENCES => {
                    self.complete_window_preferences(reply.generation, result)
                }
                Outcome::Typed(result) if matches!(reply.key.as_str(), "overview" | "history") => {
                    self.complete_metrics(&reply.key, reply.generation, result)
                }
                Outcome::Typed(result) if reply.key == "file-places" => {
                    self.complete_file_places(reply.generation, result)
                }
                Outcome::Typed(result) if reply.key == "connection-projects" => {
                    self.complete_connection_projects(reply.generation, result)
                }
                Outcome::Typed(result) if reply.key == "projects" => {
                    self.complete_projects(reply.generation, result)
                }
                Outcome::Typed(result) if reply.key == "project-folder" => {
                    self.complete_project_folder(reply.generation, result)
                }
                Outcome::Typed(result) if reply.key == "project-edit" => {
                    self.complete_project_edit(reply.generation, result)
                }
                Outcome::Typed(result)
                    if matches!(reply.key.as_str(), "git-refresh" | "git-index-refresh") =>
                {
                    self.complete_repository_refresh(&reply.key, reply.generation, result)
                }
                Outcome::Typed(result) if reply.key.starts_with("remote-apps-") => self
                    .remote_apps_readings
                    .complete(&mut self.resources, &reply.key, reply.generation, result),
                Outcome::Typed(result) => {
                    self.complete_connection(&reply.key, reply.generation, result)
                }
                Outcome::Git(result) => {
                    self.resources
                        .complete_git(&reply.key, reply.generation, result)
                }
                Outcome::Project(result) => {
                    let (accepted, project) =
                        self.complete_added_project(&reply.key, reply.generation, result);
                    saved_project = project;
                    accepted
                }
                Outcome::GitLog(result) if reply.key == "git-log-events" => {
                    let (accepted, batch) = self.complete_git_log(reply.generation, result);
                    git_log = batch;
                    accepted
                }
                Outcome::GitLog(_) => false,
                Outcome::Preview(result) if reply.key == "preview" => self.complete_owned(
                    "preview",
                    reply.generation,
                    result.map(|prepared| (prepared, true)),
                    |this| &mut this.file_preview,
                ),
                Outcome::Preview(_) => false,
            };
            if !accepted {
                if let Some(repo_id) = opened {
                    git::release_repository(reply.server, repo_id);
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
            if reply.key == "projects" && self.page == Page::Projects {
                self.restore_git_destination();
            }
            if reply.key.starts_with("remote-apps-") {
                // A change queued behind this reply goes out before the page
                // draws, so its value never flicks back in between.
                self.send_remote_mutations();
            }
            let background_read = reply.key == "snapshot"
                || reply.key == "connection-projects" && self.page == Page::Ports
                || matches!(
                    (reply.key.as_str(), self.page),
                    ("ports", Page::Ports) | ("projects", Page::Projects)
                )
                || reply.key.starts_with("project-status:")
                || reply.key.starts_with("remote-apps-");
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
                || received.as_ref().is_some_and(|next| {
                    snapshot_presentation::changed(
                        self.snapshot.as_ref(),
                        next,
                        self.server_id(),
                        match self.page {
                            Page::Integration => snapshot_presentation::Surface::Integration,
                            Page::Overview if self.setup_visible() => {
                                snapshot_presentation::Surface::Setup
                            }
                            _ => snapshot_presentation::Surface::General,
                        },
                    )
                });
            // Even quiet snapshots must reconcile retained connection owners.
            if reply.key == "snapshot" {
                if let Some(data) = received {
                    let old_server = self.selected_server().cloned();
                    let old_connection = self.selected_connection();
                    self.sync_connections_snapshot(&data);
                    remote_apps::sync_permissions(&data, cx);
                    self.snapshot = Some(data);
                    // A failed profile load is not evidence that a server was
                    // deleted. Preserve the destination until a valid snapshot.
                    if !self.configuration_available() {
                        continue;
                    }
                    self.prune_connections();
                    self.prune_project_libraries();
                    // A failed refresh (or a profile load error) does not confirm
                    // the preferences written by an integration operation.
                    // Keep its controls locked through retries until fresh
                    // configuration has actually been accepted.
                    self.integration_refresh_pending.clear();
                    let server_ids: std::collections::HashSet<String> = self
                        .servers()
                        .iter()
                        .map(|server| server.id.to_string())
                        .collect();
                    // A removed server's saved tabs go with it.
                    let removed: std::collections::BTreeSet<String> = self
                        .terminals
                        .keys()
                        .chain(self.code_workspaces.iter().map(|retained| &retained.server))
                        .filter(|id| !server_ids.contains(*id))
                        .cloned()
                        .collect();
                    for removed in removed {
                        self.ui_state.forget(removed);
                    }
                    self.terminals.retain(|id, _| server_ids.contains(id));
                    self.file_browsers.retain(|id, _| server_ids.contains(id));
                    let connections: Vec<_> = self
                        .servers()
                        .iter()
                        .map(|server| {
                            (
                                server.id.to_string(),
                                self.connection(server),
                                format!("{}@{}", server.ssh_user, server.ssh_host),
                            )
                        })
                        .collect();
                    for (id, scope, endpoint) in connections {
                        if self
                            .file_browsers
                            .get(&id)
                            .is_some_and(|browser| browser.connection != scope)
                        {
                            self.file_connection_changed(&id, scope.clone());
                        }
                        if let Some(workspace) = self.terminals.get(&id) {
                            workspace.update(cx, |workspace, cx| {
                                workspace.sync_connection(scope, endpoint, cx)
                            });
                        }
                    }
                    if let Some(id) = self.saved_server_selection.clone()
                        && self
                            .servers()
                            .iter()
                            .any(|server| server.id.to_string() == id)
                    {
                        self.saved_server_selection = None;
                        // Re-saving the current server keeps its pages; the
                        // connection check below resets them when the
                        // connection itself changed.
                        if id != self.server {
                            self.reset();
                            self.server = id;
                            self.refresh();
                        }
                    }
                    if self.server.is_empty() && !self.servers().is_empty() {
                        self.server = self.servers()[0].id.to_string();
                        self.refresh();
                    } else {
                        let current = self.selected_server().cloned();
                        let connection_changed = self.selected_connection() != old_connection;
                        if connection_changed || (!self.server.is_empty() && current.is_none()) {
                            // A backend restart or connection reset invalidates reads,
                            // not the user's repository/worktree destination. Reopen
                            // it only after reloading the same server's bookmarks.
                            let destination = (current == old_server)
                                .then(|| self.git_destination())
                                .flatten();
                            if current.is_none() {
                                self.server = self
                                    .servers()
                                    .first()
                                    .map(|server| server.id.to_string())
                                    .unwrap_or_default();
                            }
                            self.reset();
                            self.restore_git = destination;
                            self.refresh_navigation();
                        }
                    }
                }
            } else if reply.key == settings::WINDOW_PREFERENCES {
                self.window_preferences_saved();
            } else if reply.key == "server-save" {
                if self
                    .resources
                    .get("server-save")
                    .is_some_and(|r| r.error.is_none())
                {
                    self.saved_server_selection = Some(reply.server.clone());
                    self.notification = Some(Notification::success("Server saved."));
                    self.request_snapshot();
                }
            } else if reply.key.starts_with("server-delete:") {
                let result = self.resources.get(&reply.key).unwrap();
                let deleted = result.error.is_none();
                self.notification = Some(match &result.error {
                    Some(error) => Notification::error(error.clone()),
                    None => Notification::success("Server deleted."),
                });
                if deleted && let Some(id) = reply.key.strip_prefix("server-delete:") {
                    self.forget_server_state(&id.to_owned());
                }
                self.request_snapshot();
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
                    self.request_snapshot();
                }
            } else if reply.key.starts_with("integration:") {
                self.integration_completed(&reply.key, &reply.server);
            } else if reply.key == "file-project" {
                let resource = self.resources.get("file-project").unwrap();
                let result = match (saved_project.take(), &resource.error) {
                    (Some(project), None) => Ok(project),
                    (_, error) => Err(error.clone().unwrap_or_default()),
                };
                completed_file_project =
                    Some((reply.server, self.file_project_connection.take(), result));
            } else if reply.key == "project-created-save" {
                if let Some(saved) = saved_project.take() {
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
                    if saved.server.to_string() == self.server
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
                if let Some(batch) = git_log.take() {
                    self.git_log_completed(batch, cx);
                }
                self.git_sync_completed(&reply.key, cx);
                self.git_completed(&reply.key);
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
        changed |= self.poll_welcome();
        let refresh_interval = Duration::from_secs(if self.page == Page::Overview { 3 } else { 5 });
        if self.last_refresh.elapsed() >= refresh_interval {
            self.last_refresh = Instant::now();
            self.request_snapshot();
            if self.page == Page::Overview {
                self.refresh();
            }
        }
        let now = Instant::now();
        changed |= self.poll_connections(now);
        changed |= self
            .resources
            .placeholder_became_due(self.placeholders_checked_at, now);
        self.placeholders_checked_at = now;
        let editor = self.code_workspace();
        if editor.as_ref().is_none_or(|e| {
            let e = e.read(cx);
            !e.files || e.reviewing
        }) {
            self.git_listing_scroll();
        }
        if !self.project_editor_active(cx) {
            self.git_history_files_scroll();
        }
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
            for library in self.project_libraries.values_mut() {
                library.drop_checkout();
            }
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
            // Remote apps keeps its readings so a return draws them at once;
            // its own polling reads them again.
            self.resources.clear_preserving(&["setup-", "remote-apps-"]);
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
        self.resources.invalidate("history");
        self.overview_state.charts.borrow_mut().take();
        self.overview_state.inspection = None;
        self.overview_state.keyboard_inspection = false;
        if self.page == Page::Overview {
            self.resources.invalidate("overview");
            self.refresh();
        }
    }
    fn refresh(&mut self) {
        self.project_checks = None;
        if self.server.is_empty() && self.page != Page::Settings {
            return;
        }
        match self.page {
            Page::Overview => {
                self.poll_setup();
                if self.setup_visible() || self.needle_missing() {
                    return;
                }
                self.request_typed_refresh(
                    "overview",
                    desktop_server(&self.server).map(|server| {
                        desktop::Operation::Metrics(Box::new(desktop::metrics::Operation::Collect(
                            Box::new(desktop::metrics::CollectArgs {
                                server,
                                refresh: false,
                                section: desktop::metrics::Section::Overview,
                            }),
                        )))
                    }),
                );
                self.request_typed_refresh(
                    "history",
                    desktop_server(&self.server).and_then(|server| {
                        let minutes =
                            u32::try_from(self.overview_state.prefs(&self.server).minutes)
                                .map_err(|_| {
                                    newport_client::request_error::RequestError::from(
                                        "Invalid metrics history interval.",
                                    )
                                })?;
                        Ok(desktop::Operation::Metrics(Box::new(
                            desktop::metrics::Operation::History(Box::new(
                                desktop::metrics::HistoryArgs {
                                    server,
                                    minutes: Some(minutes),
                                },
                            )),
                        )))
                    }),
                );
            }
            Page::Ports => {
                if !self.configuration_available() {
                    return;
                }
                self.restore_connections();
                if !self.needle_missing() {
                    self.discover_connections(false);
                }
                self.refresh_connection_projects(Instant::now());
            }
            Page::Projects => {
                if self.needle_missing() {
                    return;
                }
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
            Page::Integration => self.request_snapshot(),
            // Read again, letting the backend answer from its cache while
            // fresh; only the page's Refresh bypasses it.
            Page::RemoteApps => self.remote_apps_returned = true,
            Page::Settings => self.settings_state.refresh(),
        }
    }
    fn sidebar(&self, compact: bool, cx: &mut Context<Self>) -> AnyElement {
        let menu_view = cx.entity().downgrade();
        div()
            .id("shell-sidebar")
            .debug_selector(|| "shell-sidebar".into())
            .w_full()
            .h_full()
            .min_h_0()
            .flex_shrink_0()
            .bg(cx.theme().sidebar)
            .text_color(theme::sidebar_foreground(cx.theme().is_dark()))
            .flex()
            .flex_col()
            .pt(px(6.))
            .child(
                div()
                    .id("sidebar-navigation-scroll")
                    .min_h_0()
                    .overflow_y_scroll()
                    .child(self.navigation(compact, cx)),
            )
            .child(div().flex_1())
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_shrink_0()
                    .py(px(6.))
                    .border_t_1()
                    .border_color(cx.theme().border)
                    .child({
                        let selected = self.page == Page::Settings;
                        div()
                            .relative()
                            .w_full()
                            .child(
                                crate::controls::button("settings")
                                    .accessibility_label("Settings")
                                    .disabled(self.server_form.is_some() || self.modal_open)
                                    .selected(selected)
                                    .child(
                                        div()
                                            .w_full()
                                            .flex()
                                            .items_center()
                                            .gap(px(11.))
                                            .when(selected, |row| {
                                                row.font_weight(FontWeight::MEDIUM)
                                            })
                                            .when(compact, |row| row.justify_center())
                                            .child(
                                                Icon::new(IconName::SlidersHorizontal)
                                                    .size(px(crate::controls::ICON_SIZE))
                                                    .text_color(navigation::icon_color(
                                                        selected, cx,
                                                    )),
                                            )
                                            .when(!compact, |row| row.child("Settings")),
                                    )
                                    .when(compact, |button| button.tooltip("Settings"))
                                    .custom(navigation::sidebar_page_style(selected, cx))
                                    .h(px(if compact { 40. } else { 36. }))
                                    .min_h(px(if compact { 40. } else { 36. }))
                                    .max_h(px(if compact { 40. } else { 36. }))
                                    .w_full()
                                    .rounded_none()
                                    // Match the navigation tabs' content inset.
                                    .border_0()
                                    .px(px(if compact { 0. } else { 17. }))
                                    .when(!compact, |button| button.justify_start())
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.navigate(Page::Settings, cx)
                                    })),
                            )
                            .when(selected, |row| row.child(navigation::selection_marker(cx)))
                    })
                    .child(self.account_row(compact, cx)),
            )
            .context_menu(move |menu, _, cx| shell::server_menu(menu, menu_view.clone(), cx))
            .into_any_element()
    }
    fn status(&self, key: &str, cx: &App) -> AnyElement {
        let Some(r) = self.resources.get(key) else {
            return div().into_any_element();
        };
        if let Some(error) = &r.error {
            return error_banner(format!("{key}-error"), error.clone(), cx).into_any_element();
        }
        if r.pending && (!r.has_value() || key.ends_with("-more")) && r.placeholder_due() {
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
/// A page-wide error above the content: error text on a faint error tint,
/// announced as an alert.
fn error_banner(
    id: impl Into<ElementId>,
    message: impl Into<SharedString>,
    cx: &App,
) -> gpui_kit::Stateful<Div> {
    crate::controls::alert(id, message, cx)
        .flex_shrink_0()
        .p_3()
        .bg(theme::error(cx.theme().is_dark()).opacity(0.12))
}
impl Render for Newport {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.sync_setup_page(cx);
        self.sync_terminal_setup(window, cx);
        self.sync_clipboard_files(window, cx);
        self.sync_services(window, cx);
        self.sync_containers(window, cx);
        self.sync_settings_ui(window, cx);
        self.sync_overview_processes(window, cx);
        self.sync_connections(window, cx);
        self.sync_code_workspace(window, cx);
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
            let endpoint = format!("{}@{}", config.ssh_user, config.ssh_host);
            let scope = self.selected_connection();
            let workspace = cx
                .new(|cx| terminal_workspace::Workspace::new(server.clone(), scope, endpoint, cx));
            self.terminals.insert(server, workspace);
        }
        self.deactivate_project_terminals(window, cx);
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
        let welcome = self.welcome_visible();
        // First run and an unfinished setup are single-purpose screens.
        let focused = welcome || (self.page == Page::Overview && self.setup_visible());
        let show_sidebar = self.sidebar_visible && !immersive && !focused;
        self.sidebar_layout.prepare(viewport, cx);
        self.content_width = viewport
            - if show_sidebar {
                if self.sidebar_layout.compact() {
                    crate::ui::ICON_RAIL_WIDTH
                } else {
                    self.sidebar_layout.width(viewport)
                }
            } else {
                0.
            };
        let connection = connectivity::scoped(&self.server, self.snapshot.as_ref(), cx);
        let health = connection.map(|state| state.phase);
        let health_label = health
            .map(connectivity::Phase::label)
            .unwrap_or("Connection unknown");
        let health_color = match health {
            Some(connectivity::Phase::Connected) => theme::success(cx.theme().is_dark()),
            Some(connectivity::Phase::Failed) => theme::error(cx.theme().is_dark()),
            _ => cx.theme().muted_foreground,
        };
        let latency = connectivity::latency(&self.server, cx)
            .filter(|_| health == Some(connectivity::Phase::Connected));
        let sidebar_toggle_label = if !self.sidebar_visible {
            "Show sidebar"
        } else if self.sidebar_layout.compact() {
            "Use full-name navigation"
        } else {
            "Use icons-only navigation"
        };
        let header = div()
            .id("workspace-header")
            .debug_selector(|| "workspace-header".into())
            .h(px(crate::ui::TITLEBAR_HEIGHT))
            .flex_shrink_0()
            .pl(px(
                if cfg!(target_os = "macos") && !window.is_fullscreen() {
                    80.
                } else {
                    8.
                },
            ))
            .pr(px(8.))
            .bg(cx.theme().sidebar)
            .flex()
            .items_center()
            .gap(px(6.))
            .border_b_1()
            .border_color(cx.theme().border)
            .when(!focused, |header| {
                header.child(
                    crate::controls::icon_button("toggle-sidebar")
                        .icon(IconName::PanelLeft)
                        .accessibility_label(sidebar_toggle_label)
                        .tooltip(sidebar_toggle_label)
                        .ghost()
                        .on_click(cx.listener(|this, _, window, cx| {
                            if window.has_active_dialog(cx) {
                                return;
                            }
                            if !this.sidebar_visible {
                                this.sidebar_visible = true;
                            } else {
                                this.sidebar_layout
                                    .set_compact(!this.sidebar_layout.compact());
                                this.save_sidebar();
                            }
                            cx.notify();
                        })),
                )
            })
            .when(!welcome, |header| header.child(self.server_selector(cx)))
            .child(
                div()
                    .id("workspace-header-drag-region")
                    .debug_selector(|| "workspace-header-drag-region".into())
                    .window_control_area(WindowControlArea::Drag)
                    .flex_1()
                    .min_w(px(8.))
                    .h_full(),
            )
            .when(
                !self.server.is_empty() && self.page != Page::Settings,
                |header| {
                    header
                        // Apart from the status, so screen readers don't announce
                        // every new reading.
                        .when_some(latency, |header, latency| {
                            let label = connectivity::latency_label(latency.round_trip);
                            let (color, hint) = match latency.speed {
                                connectivity::Speed::Normal => {
                                    (cx.theme().muted_foreground, "SSH round trip")
                                }
                                connectivity::Speed::Slow => (
                                    theme::warning(cx.theme().is_dark()),
                                    "SSH round trip, slow connection",
                                ),
                                connectivity::Speed::VerySlow => (
                                    theme::error(cx.theme().is_dark()),
                                    "SSH round trip, very slow connection",
                                ),
                            };
                            header.child(
                                div()
                                    .id("server-latency")
                                    .debug_selector(|| "server-latency".into())
                                    .aria_label(format!("{hint}: {label}"))
                                    .hover_hint(hint)
                                    .flex_shrink_0()
                                    .mr(px(2.))
                                    .text_size(px(crate::ui::FONT_SIZE))
                                    .font_features(crate::ui::tabular_figures())
                                    .text_color(color)
                                    .child(label),
                            )
                        })
                        .child(
                            div()
                                .id("server-connection-status")
                                .role(accesskit::Role::Status)
                                .test_support()
                                .aria_label(health_label)
                                .flex()
                                .flex_shrink_0()
                                .items_center()
                                .gap(px(6.))
                                .text_size(px(crate::ui::FONT_SIZE))
                                .text_color(health_color)
                                .child(crate::controls::status_dot(Some(health_color), cx))
                                .child(health_label),
                        )
                },
            )
            .child(self.shell_actions(cx));
        // Newport's header is the title bar: caption buttons sit flush right.
        #[cfg(target_os = "windows")]
        let header = header.pr_0().children(
            window_controls::caption_buttons(window, cx)
                .map(|buttons| div().h_full().ml(px(6.)).child(buttons)),
        );
        let content = div()
            .bg(cx.theme().background)
            .w_full()
            .flex_1()
            .min_w_0()
            .h_full()
            .min_h_0()
            .overflow_hidden()
            .flex()
            .flex_col()
            .when(!self.initial_loading("snapshot"), |view| {
                view.child(self.status("snapshot", cx))
            })
            .when_some(
                self.snapshot
                    .as_ref()
                    .and_then(|snapshot| snapshot.load_error.as_deref())
                    .filter(|error| !error.is_empty()),
                |content, error| {
                    content.child(error_banner(
                        "configuration-error",
                        format!("Couldn’t load your configuration: {error}"),
                        cx,
                    ))
                },
            )
            .when(
                self.page == Page::Projects && !self.initial_loading("projects"),
                |view| view.child(self.status("projects", cx)),
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
            .flex_col()
            .text_color(cx.theme().foreground)
            .font_family(crate::theme::UI_FONT)
            .text_size(px(crate::ui::FONT_SIZE))
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
                cx.listener(|this, _: &project_editor::FocusExplorer, window, cx| {
                    if this.page == Page::Projects
                        && !window.has_active_dialog(cx)
                        && let Some(editor) = this.code_workspace()
                    {
                        editor.update(cx, |e, cx| e.navigate(0, window, cx));
                    }
                }),
            )
            .on_action(
                cx.listener(|this, _: &project_editor::QuickOpen, window, cx| {
                    if this.page == Page::Projects
                        && let Some(editor) = this.code_workspace()
                    {
                        editor.update(cx, |e, cx| e.open_quick(window, cx));
                    }
                }),
            )
            .on_action(
                cx.listener(|this, _: &project_editor::CloseTab, window, cx| {
                    if window.has_active_dialog(cx) {
                        return;
                    }
                    if this.page == Page::Projects
                        && let Some(editor) = this.code_workspace()
                    {
                        editor.update(cx, |e, cx| e.close_active_tab(window, cx));
                    } else {
                        window_activation::hide(window, cx);
                    }
                }),
            )
            .on_action(
                cx.listener(|_, _: &CloseWindow, window, cx| window_activation::hide(window, cx)),
            )
            .on_action(cx.listener(|this, _: &Quit, window, cx| this.editor_quit(window, cx)))
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
                // An unfinished setup is the only screen until it is done.
                if !window.has_active_dialog(cx)
                    && this.server_form.is_none()
                    && !this.setup_visible()
                {
                    this.navigate(Page::Settings, cx);
                    this.focus.focus(window, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &FindInView, window, cx| {
                if this.page == Page::Projects
                    && !this.project_editor_active(cx)
                    && this.server_form.is_none()
                    && !window.has_active_dialog(cx)
                {
                    this.open_git_find(window, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &FindInTerminal, window, cx| {
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
                    let page = action.page;
                    if !window.has_active_dialog(cx)
                        && this.server_form.is_none()
                        && navigation::views(cx).iter().any(|(view, _)| *view == page)
                    {
                        this.navigate(page, cx);
                        // The previous page may own the focused terminal/input.
                        // Do not leave global shortcuts on an unmounted focus path.
                        this.focus.focus(window, cx);
                    }
                }),
            )
            .on_action(
                cx.listener(|this, action: &navigation::SelectTab, window, cx| {
                    this.select_tab(action.number, window, cx)
                }),
            )
            .on_action(
                cx.listener(|this, _: &navigation::NewTerminal, window, cx| {
                    this.new_terminal(window, cx)
                }),
            )
            .on_action(cx.listener(|this, _: &NewServer, window, cx| {
                if this.configuration_available() {
                    this.edit_server(None, window, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &EditServer, window, cx| {
                if let Some(server) = this.selected_server() {
                    this.edit_server(Some(server.clone()), window, cx);
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
                this.request_snapshot();
                if this.page == Page::Ports {
                    this.discover_connections(true);
                } else {
                    this.refresh();
                }
                cx.notify();
            }))
            .when(!immersive, |root| root.child(header))
            .child(if show_sidebar && !self.sidebar_layout.compact() {
                div()
                    .flex_1()
                    .min_h_0()
                    .overflow_hidden()
                    .child(
                        self.sidebar_group(cx)
                            .child(
                                self.sidebar_layout
                                    .sidebar_panel(viewport)
                                    .child(self.sidebar(false, cx)),
                            )
                            .child(self.sidebar_layout.content_panel().child(content))
                            .on_resize(cx.listener(|this, _, window, cx| {
                                if this
                                    .sidebar_layout
                                    .capture_resize(f32::from(window.viewport_size().width), cx)
                                {
                                    this.save_sidebar();
                                }
                                cx.notify();
                            })),
                    )
                    .into_any_element()
            } else if show_sidebar {
                div()
                    .flex()
                    .flex_1()
                    .min_h_0()
                    .min_w_0()
                    .child(
                        div()
                            .w(px(crate::ui::ICON_RAIL_WIDTH))
                            .flex_shrink_0()
                            .h_full()
                            .border_r_1()
                            .border_color(cx.theme().border)
                            .child(self.sidebar(true, cx)),
                    )
                    .child(content)
                    .into_any_element()
            } else {
                content.into_any_element()
            })
    }
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
                // Remote app windows have no Root and keep their minimized
                // or hidden state when Newport is activated.
                if window.root::<gpui_kit::base::Root>().flatten().is_some() {
                    window_activation::restore(window)?;
                    window.activate_window();
                }
                Ok::<_, String>(())
            })
            .map_err(|error| error.to_string())??;
    }
    Ok(())
}
/// Host-driven exits must honor the same unsaved-buffer guard as the app menu.
pub fn request_quit(cx: &mut App) -> Result<(), String> {
    for handle in cx.windows() {
        let handled = handle
            .update(cx, |_, window, cx| {
                let Some(root) = window.root::<gpui_kit::base::Root>().flatten() else {
                    return false;
                };
                let Ok(owner) = root.read(cx).view().clone().downcast::<Newport>() else {
                    return false;
                };
                let _ = window_activation::restore(window);
                window.activate_window();
                owner.update(cx, |this, cx| this.editor_quit(window, cx));
                true
            })
            .map_err(|error| error.to_string())?;
        if handled {
            return Ok(());
        }
    }
    cx.quit();
    Ok(())
}

/// An installer cannot silently discard an editor buffer or interrupt a save.
/// Refusal happens before accepting the installation handoff, so retry stays available.
pub fn ensure_editors_saved(cx: &mut App) -> Result<(), String> {
    for handle in cx.windows() {
        let dirty = handle
            .update(cx, |_, window, cx| {
                let Some(root) = window.root::<gpui_kit::base::Root>().flatten() else {
                    return false;
                };
                let Ok(owner) = root.read(cx).view().clone().downcast::<Newport>() else {
                    return false;
                };
                owner
                    .read(cx)
                    .code_workspaces
                    .iter()
                    .any(|workspace| workspace.editor.read(cx).dirty(cx))
            })
            .map_err(|error| error.to_string())?;
        if dirty {
            return Err("Save or close unsaved code files, and wait for saves to finish before installing the update.".into());
        }
    }
    Ok(())
}

mod window_activation;
#[cfg(target_os = "windows")]
mod window_controls;

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

/// Presentation options supplied by the application host. OS login registration
/// and command-line policy remain outside the UI crate.
#[derive(Clone, Default)]
pub struct LaunchOptions {
    pub start_hidden: bool,
    /// What a taskbar pin of the window runs, when Newport alone wouldn't
    /// open this profile again (Windows).
    pub relaunch: Option<Relaunch>,
}

/// A taskbar pin's command, under the window's AppUserModelID.
#[derive(Clone)]
pub struct Relaunch {
    pub app_id: String,
    pub command: String,
    /// `<file>,<index>`.
    pub icon: String,
}

/// Notify the host after the root window has been constructed and the event
/// loop resumes, including hidden login startup. This is not a paint guarantee.
pub fn run_with_options_and_ready(
    options: LaunchOptions,
    setup: impl FnOnce(&mut App) + 'static,
    ready: impl FnOnce() + 'static,
) -> Result<(), String> {
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
        // The login-item mark is only readable while the app finishes launching.
        let start_hidden = options.start_hidden || newport_platform::startup::launched_at_login();
        setup(cx);
        gpui_kit::init(cx);
        theme::install(
            matches!(
                cx.window_appearance(),
                WindowAppearance::Dark | WindowAppearance::VibrantDark
            ),
            cx,
        );
        cx.on_action(|_: &Quit, cx| {
            let _ = request_quit(cx);
        });
        cx.on_window_closed(|cx, _| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();
        // Before the window's first frame, so a restored Remote apps page
        // and its sidebar entry are there from the start; and before the
        // menus, so the View menu lists it too.
        remote_apps::start(cx);
        navigation::install_desktop_commands(cx);
        let saved = ui_state::load();
        let window_state = window_state::WindowState::load(saved.window, cx);
        let bounds = window_state.bounds();
        let opened = cx.open_window(
            WindowOptions {
                show: !start_hidden,
                focus: !start_hidden,
                window_bounds: Some(bounds),
                display_id: window_state.display_id(),
                window_min_size: Some(size(px(640.), px(480.))),
                window_background: WindowBackgroundAppearance::Opaque,
                titlebar: Some(TitlebarOptions {
                    title: Some("Newport".into()),
                    // Newport draws the title bar; on Windows its header
                    // carries the caption buttons (`window_controls`).
                    appears_transparent: true,
                    traffic_light_position: cfg!(target_os = "macos")
                        .then(|| point(px(12.), px(12.))),
                }),
                ..Default::default()
            },
            move |window, cx| {
                #[cfg(windows)]
                if let Some(relaunch) = &options.relaunch {
                    use raw_window_handle::HasWindowHandle;
                    let set = HasWindowHandle::window_handle(window)
                        .map_err(|error| error.to_string())
                        .and_then(|handle| {
                            newport_platform::window::set_relaunch(
                                handle,
                                &relaunch.app_id,
                                &relaunch.command,
                                "Newport",
                                &relaunch.icon,
                            )
                        });
                    if let Err(error) = set {
                        eprintln!("{error}");
                    }
                }
                // Closing the window hides it and preserves its workspace and
                // drafts. Explicit Quit still exits normally.
                window.on_window_should_close(cx, |window, cx| {
                    window_activation::hide(window, cx);
                    false
                });
                window.set_rem_size(px(14.));
                let view = cx.new(|cx| Newport::new(window, window_state, saved, cx));
                cx.new(|cx| {
                    gpui_kit::base::Root::new(view, window, cx).bg(gpui_kit::transparent_black())
                })
            },
        );
        if let Err(error) = opened {
            *failed_startup.borrow_mut() = Some(format!("Couldn’t open Newport: {error}"));
            cx.quit();
            return;
        }
        cx.defer(move |_| ready());
        if !start_hidden {
            cx.activate(true);
        }
    });
    startup_error.borrow_mut().take().map_or(Ok(()), Err)
}

/// Validate all bundled grammar libraries without opening a window or backend.
pub fn check_grammars() -> Result<(), String> {
    grammars::validate_all()
}

/// Whether this build can guide GUI launch attempts from a terminal.
pub fn supports_terminal_gui_setup() -> bool {
    cfg!(newport_remote_apps)
}

/// Opens the remote app a taskbar pin or shortcut names, by its registry
/// entry (`None` when Newport no longer knows it), or says in Newport's
/// window why it can't.
pub fn open_remote_app(
    entry: Option<newport_domain::remote_app_registry::Entry>,
    action: Option<String>,
    cx: &mut App,
) {
    remote_apps::open_from_shortcut(entry, action, cx);
}

/// Called only after a remote process requests GUI display services.
pub fn request_terminal_gui_setup(server: &str, cx: &mut App) {
    terminal_setup::request(server, cx);
}

/// Ask whether to download the files of a server copy. `cancelled` closes the
/// question; closing it unanswered replies no.
pub fn request_clipboard_files(
    question: ClipboardFilesQuestion,
    reply: Box<dyn FnOnce(bool) + Send>,
    cancelled: std::sync::Arc<std::sync::atomic::AtomicBool>,
    cx: &mut App,
) {
    clipboard_files::request(question, reply, cancelled, cx);
}

/// Shows how the download of a server copy's files is going.
pub fn clipboard_files_notice(server: String, notice: ClipboardFilesNotice, cx: &mut App) {
    clipboard_files::notice(server, notice, cx);
}
