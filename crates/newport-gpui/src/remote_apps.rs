//! Remote apps: launch a server's Linux GUI applications as native windows.
//!
//! Newport's server queries report the installed agent and the server's
//! applications; [`crate::remote_apps_sessions`] runs the sessions. Streaming
//! is part of full builds only (see `build.rs`); elsewhere the page is not
//! shown.
use super::*;
use crate::remote_apps_api as api;
use crate::remote_apps_install as install;
use crate::remote_apps_sessions::{Launch, Sessions};
use gpui_kit::component::{button::ButtonGroup, input::InputEvent};
use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
    rc::Rc,
    sync::Arc,
};

const INSTALL_UNAVAILABLE: &str = "Installing from Newport isn’t part of this build. Install the agent from a newport-remote checkout with `cargo xtask deploy-agent USER@HOST`, then check again.";

/// Setting up the agent on one server, started from this page.
enum Install {
    /// Get started was pressed: the server is being checked.
    Diagnosing(Diagnosis),
    Running(Activation, install::Progress),
    /// Installed; waiting for a status reading newer than this revision of
    /// the server query to report an agent.
    Finished(Activation, u64),
    Failed(String),
}

/// A shell on the server, floating over the page, with a command typed in
/// that installs a missing program. The user presses Return to run it.
struct InstallTerminal {
    /// Distinguishes this shell from one opened later, for its re-checks.
    id: u64,
    /// What it installs, for example "Install Xvfb".
    title: String,
    /// The command it was opened with; the button that opened it shows so.
    command: String,
    view: Entity<crate::terminal_view::TerminalView>,
}

/// The floating terminal's height.
const TERMINAL_HEIGHT: f32 = 260.;
/// How often the server is checked again while an install shell is open, so
/// the checklist follows the install as it happens.
const TERMINAL_RECHECK: Duration = Duration::from_secs(3);

/// The shortest time the activation screen shows, so a fast install still
/// reads as a deliberate step rather than a flash.
const ACTIVATION_MIN: Duration = Duration::from_secs(2);

/// When Activate was pressed, and whether it was a first install (shown on
/// the wall) rather than an update (shown as an alert).
#[derive(Clone, Copy)]
struct Activation {
    started: Instant,
    fresh: bool,
}

impl Activation {
    /// How far the screen shows the activation: the real progress, but never
    /// faster than [`ACTIVATION_MIN`].
    fn shown(&self, progress: f32) -> f32 {
        let clock = self.started.elapsed().as_secs_f32() / ACTIVATION_MIN.as_secs_f32();
        progress.min(clock).clamp(0., 1.)
    }

    /// A first install keeps its screen until the minimum time has passed.
    fn held(&self) -> bool {
        self.fresh && self.started.elapsed() < ACTIVATION_MIN
    }
}

/// A fresh check of the server, revealed one row at a time.
struct Diagnosis {
    /// When Get started was pressed; a reading finished after it, with no
    /// read in flight, is the check's result.
    started: Instant,
    arrived: bool,
    /// A check in this diagnosis succeeded; a later failed re-check keeps
    /// its results rather than replacing them with the error.
    confirmed: bool,
    /// Rows shown so far.
    shown: usize,
}

/// What each diagnosis row looked for, in order.
const CHECKS: [&str; 5] = ["Connection", "System", "D-Bus", "Xvfb", "Sound"];
/// How long each row takes to reveal its result.
const CHECK_STEP: Duration = Duration::from_millis(350);

/// One row of the diagnosis.
struct Check {
    label: String,
    detail: String,
    outcome: CheckOutcome,
    /// A command installing what's missing, run in the server's terminal.
    fix: Option<String>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum CheckOutcome {
    Passed,
    /// Activation works; some apps or features won't.
    Gap,
    /// Activation can't succeed until this is fixed.
    Blocked,
}

/// The welcome wall: icons standing for the apps a server can open, each with
/// its hue in [`theme::launch_tile`].
const WALL: [(IconName, usize); 24] = [
    (IconName::Globe, 0),
    (IconName::Code, 3),
    (IconName::SquareTerminal, 1),
    (IconName::Folder, 4),
    (IconName::Image, 2),
    (IconName::Calculator, 6),
    (IconName::Palette, 5),
    (IconName::Settings, 7),
    (IconName::FileText, 1),
    (IconName::Brush, 2),
    (IconName::Mail, 0),
    (IconName::Music, 5),
    (IconName::Database, 3),
    (IconName::ChartBar, 4),
    (IconName::Calendar, 6),
    (IconName::GitBranch, 0),
    (IconName::Archive, 7),
    (IconName::Video, 5),
    (IconName::Map, 1),
    (IconName::Notebook, 4),
    (IconName::Pencil, 2),
    (IconName::Camera, 3),
    (IconName::Gamepad2, 6),
    (IconName::MessageCircle, 0),
];
/// When each tile lights up during an install, as a rank: scattered, not in
/// reading order.
const LIGHT_RANK: [usize; 24] = [
    9, 2, 17, 5, 22, 11, 0, 14, 7, 19, 3, 12, 21, 6, 16, 1, 23, 10, 4, 18, 13, 8, 20, 15,
];
const WALL_TILE: f32 = 52.;
const WALL_GAP: f32 = 14.;

/// Everything Remote apps keeps outside the page.
struct State {
    sessions: Rc<Sessions>,
    /// Servers that allow clipboard sharing, from the accepted snapshot.
    clipboard: RefCell<HashSet<String>>,
}
impl Global for State {}

/// Set up Remote apps before the main window opens.
pub(crate) fn start(cx: &mut App) {
    if !cx.has_global::<State>() {
        cx.set_global(State::load(Sessions::new()));
    }
}

/// Sessions that stream nothing, for page tests.
#[cfg(test)]
pub(crate) fn start_scripted(cx: &mut App) {
    cx.set_global(State::load(Sessions::scripted()));
}

/// Whether this build shows Remote apps.
pub(crate) fn available(cx: &App) -> bool {
    cx.try_global::<State>()
        .is_some_and(|state| state.sessions.available())
}

fn sessions(cx: &App) -> Option<Rc<Sessions>> {
    cx.try_global::<State>().map(|state| state.sessions.clone())
}

/// Stop sessions and close their windows before quitting.
pub(crate) fn shutdown(cx: &mut App) {
    if let Some(sessions) = sessions(cx) {
        sessions.shutdown(cx);
    }
}

/// Remote apps whose windows are open or opening.
pub(crate) fn active_sessions(cx: &App) -> usize {
    sessions(cx).map_or(0, |sessions| sessions.active())
}

/// True once after session state changed.
pub(crate) fn take_changed(cx: &App) -> bool {
    sessions(cx).is_some_and(|sessions| {
        sessions.poll();
        sessions.take_changed()
    })
}

/// Mirror per-server permissions from the accepted snapshot.
pub(crate) fn sync_permissions(snapshot: &Value, cx: &App) {
    let Some(state) = cx.try_global::<State>() else {
        return;
    };
    *state.clipboard.borrow_mut() = rows(&snapshot["config"]["servers"])
        .iter()
        .filter(|server| server["clipboardEnabled"] == true)
        .map(|server| text(server, "id"))
        .collect();
}

fn clipboard(server: &str, cx: &App) -> bool {
    cx.try_global::<State>()
        .is_some_and(|state| state.clipboard.borrow().contains(server))
}

/// A command the user added to a server's applications.
#[derive(Clone, Debug)]
struct SavedCommand {
    id: String,
    name: String,
    /// As typed: parsed again on every launch.
    command: String,
    display: api::DisplayBackend,
}

/// The application id of a saved command's tile and sessions.
fn command_app_id(id: &str) -> String {
    format!("command:{id}")
}

/// Environment variables and the program with its arguments.
type ParsedCommand = (Vec<(String, String)>, Vec<String>);

/// Split a command line like a shell would, without running one. Leading
/// `NAME=value` words become environment variables.
fn parse_command(text: &str) -> Result<ParsedCommand, String> {
    let words = shlex::split(text).ok_or("Check the quotes in the command.")?;
    let mut env = Vec::new();
    let mut words = words.into_iter().peekable();
    while let Some((name, value)) = words.peek().and_then(|word| word.split_once('=')) {
        let valid = name.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_')
            && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
        if !valid {
            break;
        }
        env.push((name.to_owned(), value.to_owned()));
        words.next();
    }
    let argv: Vec<String> = words.collect();
    if argv.is_empty() {
        return Err("Enter a program to run.".into());
    }
    Ok((env, argv))
}

/// How to install missing software, naming the server's distribution.
fn install_guidance(
    server: &str,
    os: Option<&api::ServerOs>,
    name: &str,
    required: bool,
) -> String {
    let need = if required {
        format!("Remote apps need {name} on {server}")
    } else {
        format!("{server} doesn’t have {name}")
    };
    match os {
        Some(os) if os.package_manager() == Some(api::PackageManager::Nix) => format!(
            "{need} ({}). Install it for your user, or add it to environment.systemPackages and rebuild. Then check again.",
            os.name
        ),
        Some(os) if os.package_manager().is_some() => {
            format!("{need} ({}). Install it, then check again.", os.name)
        }
        Some(os) if !os.name.is_empty() => format!(
            "{need} ({}). Install it with your package manager, then check again.",
            os.name
        ),
        _ => format!("{need}. Install it with the server’s package manager, then check again."),
    }
}

/// "/opt/tool/bin/app" is shown as "app".
fn program_name(program: &str) -> String {
    program.rsplit('/').next().unwrap_or(program).to_owned()
}

impl State {
    fn load(sessions: Sessions) -> Self {
        Self {
            sessions: Rc::new(sessions),
            clipboard: Default::default(),
        }
    }
}

/// Preferences live in the backend's preferences store. These keys hold the
/// page's readings of them, kept while the page is away so a return draws them
/// at once.
const SETTINGS_KEY: &str = "remote-apps-settings";
/// The servers last seen with the agent, from the backend's preferences.
const INSTALLED_KEY: &str = "remote-apps-installed";
fn commands_key(server: &str) -> String {
    format!("remote-apps-commands:{server}")
}

fn decode_settings(value: &Value) -> api::StreamingSettings {
    let defaults = api::StreamingSettings::default();
    api::StreamingSettings {
        audio: value["audio"].as_bool().unwrap_or(defaults.audio),
        codec: match value["codec"].as_str() {
            Some("lossless") => api::CodecPreference::Lossless,
            Some("fast") => api::CodecPreference::Fast,
            _ => api::CodecPreference::Auto,
        },
        fps_goal: value["fpsGoal"]
            .as_u64()
            .filter(|fps| [15, 30, 60].contains(fps))
            .map_or(defaults.fps_goal, |fps| fps as u32),
        hud: value["hud"].as_bool().unwrap_or(defaults.hud),
    }
}

fn encode_settings(settings: api::StreamingSettings) -> Value {
    json!({
        "audio": settings.audio,
        "codec": match settings.codec {
            api::CodecPreference::Auto => "auto",
            api::CodecPreference::Lossless => "lossless",
            api::CodecPreference::Fast => "fast",
        },
        "fpsGoal": settings.fps_goal,
        "hud": settings.hud,
    })
}

fn decode_commands(value: &Value) -> Vec<SavedCommand> {
    value
        .as_array()
        .into_iter()
        .flatten()
        .map(|command| SavedCommand {
            id: text(command, "id"),
            name: text(command, "name"),
            command: text(command, "command"),
            display: match command["display"].as_str() {
                Some("x11") => api::DisplayBackend::X11,
                Some("wayland") => api::DisplayBackend::Wayland,
                _ => api::DisplayBackend::Auto,
            },
        })
        .filter(|command| !command.id.is_empty())
        .collect()
}

fn encode_command(server: &str, command: &SavedCommand) -> Value {
    json!({
        "id": command.id,
        "serverId": server,
        "name": command.name,
        "command": command.command,
        "display": match command.display {
            api::DisplayBackend::Auto => "auto",
            api::DisplayBackend::X11 => "x11",
            api::DisplayBackend::Wayland => "wayland",
        },
    })
}

const TILE_WIDTH: f32 = 112.;
const TILE_HEIGHT: f32 = 128.;
const ICON: f32 = 64.;
const GAP_X: f32 = 8.;
const GAP_Y: f32 = 12.;
/// Height reserved under the grid for the page indicator.
const PAGER_HEIGHT: f32 = 40.;
/// Trackpad scroll distance that turns a page. One gesture turns at most one
/// page; its momentum is ignored. Devices without gesture phases start a new
/// gesture after a pause.
const SWIPE: f32 = 60.;
const GESTURE_GAP: Duration = Duration::from_millis(300);

/// The command invoker's form.
struct Runner {
    /// The saved command being edited.
    editing: Option<String>,
    command: Entity<InputState>,
    name: Entity<InputState>,
    display: api::DisplayBackend,
    save: bool,
    error: Option<String>,
    _subscription: Subscription,
}

pub(crate) enum LaunchpadEvent {
    /// Read the server's queries again, bypassing their freshness.
    Refresh,
    /// Read only the server's status again, bypassing its freshness. With
    /// `replace`, a read already in flight gives way to this one.
    Recheck {
        replace: bool,
    },
    /// Save streaming settings in the backend's preferences.
    SaveSettings(Value),
    /// Save one of the server's commands.
    SaveCommand {
        server: String,
        command: Value,
    },
    RemoveCommand {
        server: String,
        id: String,
    },
}

/// A preference change waiting to be sent, under the key it updates.
pub(crate) struct Mutation {
    key: String,
    method: &'static str,
    args: Value,
}

/// Whether the installed-servers record lists this server.
fn known_installed(record: &Value, server: &str) -> bool {
    record
        .as_array()
        .is_some_and(|servers| servers.iter().any(|id| id == server))
}

/// "one fix", "two gaps", "7 fixes".
fn count(n: usize, one: &str, many: &str) -> String {
    let word = match n {
        1 => "one",
        2 => "two",
        3 => "three",
        4 => "four",
        _ => return format!("{n} {many}"),
    };
    format!("{word} {}", if n == 1 { one } else { many })
}

/// How often the page asks the backend for each query while visible. The
/// backend answers from its cache until the query itself is stale.
const STATUS_POLL: Duration = Duration::from_secs(15);
const CATALOGUE_POLL: Duration = Duration::from_secs(60);

fn status_key(server: &str) -> String {
    format!("remote-apps-status:{server}")
}
fn catalogue_key(server: &str) -> String {
    format!("remote-apps-catalogue:{server}")
}

/// One backend query as the page last saw it.
#[derive(Clone, Default)]
pub(crate) struct QueryRead {
    value: Value,
    error: Option<String>,
    pending: bool,
    /// Whether a reading ever succeeded; a failed refresh keeps the old value.
    loaded: bool,
    revision: u64,
    /// When the last read finished, successfully or not.
    finished_at: Option<Instant>,
    /// The value holds an edit whose save hasn't been confirmed.
    edited: bool,
}

impl QueryRead {
    /// Copies the value only when a newer reading replaced it.
    fn observe(&mut self, resource: Option<&state::Resource>) {
        let Some(resource) = resource else {
            *self = Self::default();
            return;
        };
        // A newer reading replaces the value. So does a settled save that
        // failed: the edit it carried was not kept.
        let rejected = self.edited && !resource.pending;
        if resource.revision != self.revision
            || !self.loaded && resource.updated_at.is_some()
            || rejected
        {
            self.value = resource.value.clone();
            self.revision = resource.revision;
            self.edited = false;
        }
        self.loaded = resource.updated_at.is_some();
        self.error = resource.error.clone();
        self.pending = resource.pending;
        self.finished_at = resource.finished_at;
    }
}

/// Server programs the check found missing, with their package names.
fn missing_packages(status: &Value) -> Vec<api::MissingPackage> {
    status["missing"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|package| {
            let mut packages = api::PackageNames::new();
            for (manager, key) in [
                (api::PackageManager::Apt, "apt"),
                (api::PackageManager::Dnf, "dnf"),
                (api::PackageManager::Yum, "yum"),
                (api::PackageManager::Zypper, "zypper"),
                (api::PackageManager::Pacman, "pacman"),
                (api::PackageManager::Apk, "apk"),
                (api::PackageManager::Xbps, "xbps"),
                (api::PackageManager::Emerge, "emerge"),
                (api::PackageManager::Nix, "nix"),
            ] {
                if let Some(name) = package["packages"][key].as_str() {
                    packages = packages.with(manager, name);
                }
            }
            api::MissingPackage {
                id: text(package, "id"),
                name: text(package, "name"),
                purpose: text(package, "purpose"),
                required: package["required"] == true,
                needed_to_install: package["neededToInstall"] == true,
                packages,
            }
        })
        .filter(|package| !package.name.is_empty())
        .collect()
}

/// The agent's catalogue entries, as the agent prints them.
fn decode_catalogue(value: &Value) -> Vec<api::RemoteApp> {
    use base64::Engine;
    let text = |value: &Value, key: &str| value[key].as_str().unwrap_or_default().to_owned();
    value["apps"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|app| api::RemoteApp {
            id: text(app, "id"),
            name: text(app, "name"),
            description: Some(text(app, "description")).filter(|d| !d.is_empty()),
            icon_png: app["icon"].as_str().and_then(|icon| {
                base64::engine::general_purpose::STANDARD
                    .decode(icon)
                    .ok()
                    .map(Into::into)
            }),
            actions: app["actions"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|action| api::AppAction {
                    id: text(action, "id"),
                    name: text(action, "name"),
                })
                .collect(),
        })
        .filter(|app| !app.id.is_empty())
        .collect()
}

/// The Remote apps launchpad for the selected server.
pub(crate) struct Launchpad {
    server: String,
    server_name: String,
    search: Entity<InputState>,
    _search: Subscription,
    icons: HashMap<(String, usize), Arc<Image>>,
    error: Option<String>,
    /// Alerts the user closed, as `server/alert id`, kept across visits.
    dismissed: HashSet<String>,
    preferences_open: bool,
    runner: Option<Runner>,
    /// The page visit this view last showed.
    visit: u64,
    /// Installs by server; they go on while the page shows another server.
    installs: HashMap<String, Install>,
    /// Servers whose install finished while another one was shown.
    installed: HashSet<String>,
    /// The floating terminal installing a missing program, if open.
    terminal: Option<InstallTerminal>,
    /// Numbers each shell opened.
    terminals_opened: u64,
    status: QueryRead,
    settings: QueryRead,
    /// The servers last seen with the agent.
    installed_record: QueryRead,
    commands: QueryRead,
    catalogue: QueryRead,
    /// The decoded catalogue and the revision it came from.
    decoded: RefCell<Option<(u64, Vec<api::RemoteApp>)>>,
    /// Commands run from the panel in this session, newest first.
    recent: Vec<String>,
    page: usize,
    /// The measured grid area; zero until the first layout.
    area: Size<Pixels>,
    swipe: f32,
    /// Whether the current scroll gesture may still turn a page.
    armed: bool,
    scrolled_at: Option<Instant>,
}

impl Launchpad {
    pub(crate) fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let search = cx.new(|cx| InputState::new(window, cx).placeholder("Search apps"));
        let subscription = cx.subscribe(&search, |this, _, event, cx| match event {
            InputEvent::Change => {
                this.page = 0;
                cx.notify();
            }
            // Return opens the first match, as in macOS Launchpad.
            InputEvent::PressEnter { .. } => {
                if let Some(app) = this.matches(cx).into_iter().next() {
                    this.launch(&app.id, None, cx);
                }
            }
            _ => {}
        });
        Self {
            server: String::new(),
            server_name: String::new(),
            search,
            _search: subscription,
            icons: HashMap::new(),
            error: None,
            dismissed: HashSet::new(),
            preferences_open: false,
            runner: None,
            visit: 0,
            installs: HashMap::new(),
            installed: HashSet::new(),
            terminal: None,
            terminals_opened: 0,
            status: QueryRead::default(),
            settings: QueryRead::default(),
            installed_record: QueryRead::default(),
            commands: QueryRead::default(),
            catalogue: QueryRead::default(),
            decoded: Default::default(),
            recent: Vec::new(),
            page: 0,
            area: Size::default(),
            swipe: 0.,
            armed: true,
            scrolled_at: None,
        }
    }

    /// Columns and rows of tiles that fit the measured grid area.
    fn layout(&self) -> (usize, usize) {
        let fit = |space: Pixels, tile: f32, gap: f32| {
            (((f32::from(space) + gap) / (tile + gap)).floor() as usize).max(1)
        };
        if self.area.width <= px(0.) {
            return (6, 3);
        }
        (
            fit(self.area.width, TILE_WIDTH, GAP_X),
            fit(self.area.height - px(PAGER_HEIGHT), TILE_HEIGHT, GAP_Y),
        )
    }

    fn turn(&mut self, page: usize, pages: usize, cx: &mut Context<Self>) {
        let page = page.min(pages.saturating_sub(1));
        if page != self.page {
            self.page = page;
            cx.notify();
        }
    }

    /// Turns pages with horizontal or vertical scrolling.
    fn scrolled(&mut self, event: &ScrollWheelEvent, pages: usize, cx: &mut Context<Self>) {
        if pages < 2 {
            return;
        }
        // A wheel notch turns a page; a trackpad needs some distance.
        let (x, y, threshold) = match event.delta {
            ScrollDelta::Lines(lines) => (lines.x, lines.y, 1.),
            ScrollDelta::Pixels(pixels) => (f32::from(pixels.x), f32::from(pixels.y), SWIPE),
        };
        let delta = if x.abs() > y.abs() { x } else { y };
        let now = Instant::now();
        let paused = self
            .scrolled_at
            .is_none_or(|at| now.duration_since(at) > GESTURE_GAP);
        self.scrolled_at = Some(now);
        if event.touch_phase == TouchPhase::Started || paused {
            self.armed = true;
            self.swipe = 0.;
        }
        if !self.armed || delta == 0. {
            return;
        }
        if self.swipe.signum() != delta.signum() {
            self.swipe = 0.;
        }
        self.swipe += delta;
        if self.swipe.abs() >= threshold {
            let page = if self.swipe < 0. {
                self.page + 1
            } else {
                self.page.saturating_sub(1)
            };
            self.armed = false;
            self.swipe = 0.;
            self.turn(page, pages, cx);
        }
    }

    fn pager(&self, pages: usize, cx: &mut Context<Self>) -> Div {
        let mut dots = div().flex().items_center().gap(px(2.));
        for page in 0..pages {
            let current = page == self.page;
            dots = dots.child(
                div()
                    .id(("remote-apps-page", page))
                    .debug_selector(move || format!("remote-apps-page-{page}"))
                    .role(Role::Button)
                    .aria_label(format!("Page {}", page + 1))
                    .size(px(20.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .child(div().size(px(7.)).rounded_full().bg(if current {
                        cx.theme().foreground
                    } else {
                        cx.theme().muted_foreground.opacity(0.35)
                    }))
                    .on_click(cx.listener(move |this, _, _, cx| this.turn(page, pages, cx))),
            );
        }
        let last = pages - 1;
        div()
            .h(px(PAGER_HEIGHT))
            .flex_shrink_0()
            .flex()
            .items_center()
            .justify_center()
            .gap(px(6.))
            .child(
                crate::controls::compact_icon_button("remote-apps-previous")
                    .icon(IconName::ChevronLeft)
                    .tooltip("Previous page")
                    .disabled(self.page == 0)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.turn(this.page.saturating_sub(1), pages, cx)
                    })),
            )
            .child(dots)
            .child(
                crate::controls::compact_icon_button("remote-apps-next")
                    .icon(IconName::ChevronRight)
                    .tooltip("Next page")
                    .disabled(self.page == last)
                    .on_click(
                        cx.listener(move |this, _, _, cx| this.turn(this.page + 1, pages, cx)),
                    ),
            )
    }

    /// Show a server. A new server or a return to the page starts the view
    /// afresh; the last readings stay on screen until newer ones arrive.
    pub(crate) fn show(
        &mut self,
        server: &str,
        name: &str,
        visit: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.server != server {
            // A shell, which may still be installing, belongs to its server;
            // it stays open while other pages are shown.
            if let Some(terminal) = self.terminal.take() {
                terminal.view.update(cx, |view, _| view.close());
            }
            // A check left unfinished starts again with Get started.
            self.installs
                .retain(|_, install| !matches!(install, Install::Diagnosing(_)));
        }
        if self.server != server || self.visit != visit {
            self.server = server.to_owned();
            self.visit = visit;
            self.preferences_open = false;
            self.error = None;
            self.runner = None;
            self.recent.clear();
            self.page = 0;
            self.search
                .update(cx, |search, cx| search.set_value("", window, cx));
        }
        self.server_name = name.to_owned();
        if self.installed.remove(server) {
            cx.emit(LaunchpadEvent::Refresh);
        }
    }

    /// Open a shell on the server, floating over the page, with `command`
    /// typed in. It replaces any shell already open.
    fn open_terminal(
        &mut self,
        title: String,
        command: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let draft = match newport_client::terminal_command::CommandDraft::new(command.clone()) {
            Ok(draft) => draft,
            Err(error) => {
                self.error = Some(error);
                cx.notify();
                return;
            }
        };
        if let Some(terminal) = self.terminal.take() {
            terminal.view.update(cx, |view, _| view.close());
        }
        let server = self.server.clone();
        let view = cx.new(|cx| crate::terminal_view::TerminalView::new(server, cx));
        view.update(cx, |view, _| view.prepare_command(draft));
        // Typing goes straight to the shell once it's drawn.
        let focus = view.read(cx).focus.clone();
        window.defer(cx, move |window, cx| focus.focus(window, cx));
        self.terminals_opened += 1;
        let id = self.terminals_opened;
        self.terminal = Some(InstallTerminal {
            id,
            title,
            command,
            view,
        });
        cx.notify();
        // Check the server again every few seconds while this shell is open;
        // the checklist updates in place as programs appear.
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(TERMINAL_RECHECK).await;
                let open = this.update(cx, |this, cx| {
                    let open = this.terminal.as_ref().is_some_and(|t| t.id == id);
                    if open {
                        this.recheck(cx);
                    }
                    open
                });
                if !open.unwrap_or(false) {
                    return;
                }
            }
        })
        .detach();
    }

    /// Read the server's status again without replaying the checklist: rows
    /// keep their place and only what changed is redrawn.
    fn recheck(&mut self, cx: &mut Context<Self>) {
        // One check at a time; a newer one would only cancel this one.
        if self.status.pending {
            return;
        }
        cx.emit(LaunchpadEvent::Recheck { replace: false });
    }

    /// Close the shell and check the server again, so a program it installed
    /// shows as present.
    fn close_terminal(&mut self, cx: &mut Context<Self>) {
        let Some(terminal) = self.terminal.take() else {
            return;
        };
        terminal.view.update(cx, |view, _| view.close());
        // A check in flight may have started before the install finished.
        cx.emit(LaunchpadEvent::Recheck { replace: true });
    }

    /// The floating shell, over the page's lower part.
    fn terminal_panel(&self, cx: &mut Context<Self>) -> Option<Div> {
        let terminal = self.terminal.as_ref()?;
        Some(
            div()
                .debug_selector(|| "remote-apps-terminal".into())
                .absolute()
                .left(px(crate::controls::PAGE_PADDING))
                .right(px(crate::controls::PAGE_PADDING))
                .bottom(px(crate::controls::PAGE_PADDING))
                .h(px(TERMINAL_HEIGHT))
                .flex()
                .flex_col()
                .overflow_hidden()
                .rounded(px(12.))
                .border_1()
                .border_color(cx.theme().border)
                .bg(cx.theme().background)
                .shadow_lg()
                .child(
                    div()
                        .flex_shrink_0()
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .px(px(12.))
                        .py(px(6.))
                        .border_b_1()
                        .border_color(cx.theme().border)
                        .child(
                            Icon::new(IconName::SquareTerminal)
                                .size(px(15.))
                                .text_color(cx.theme().muted_foreground),
                        )
                        .child(
                            div()
                                .text_size(px(12.))
                                .font_weight(FontWeight::SEMIBOLD)
                                .child(terminal.title.clone()),
                        )
                        .child(
                            div()
                                .text_size(px(12.))
                                .text_color(cx.theme().muted_foreground)
                                .child(format!("· {}", self.server_name)),
                        )
                        .child(div().flex_1())
                        .child(
                            div()
                                .text_size(px(12.))
                                .text_color(cx.theme().muted_foreground)
                                .child("Close it when it’s done to check again."),
                        )
                        .child(
                            div()
                                .debug_selector(|| "remote-apps-terminal-close".into())
                                .child(
                                    crate::controls::compact_icon_button(
                                        "remote-apps-terminal-close",
                                    )
                                    .icon(IconName::X)
                                    .tooltip("Close and check again")
                                    .on_click(
                                        cx.listener(|this, _, _, cx| this.close_terminal(cx)),
                                    ),
                                ),
                        ),
                )
                .child(div().flex_1().min_h_0().child(terminal.view.clone())),
        )
    }

    /// The page with the floating shell over it, leaving room for it below.
    fn with_terminal(&self, root: Stateful<Div>, cx: &mut Context<Self>) -> AnyElement {
        let panel = self.terminal_panel(cx);
        root.relative()
            .when(panel.is_some(), |root| {
                root.pb(px(TERMINAL_HEIGHT + 2. * crate::controls::PAGE_PADDING))
            })
            .children(panel)
            .into_any_element()
    }

    /// Check the server afresh and reveal what it has, one row at a time.
    fn diagnose(&mut self, cx: &mut Context<Self>) {
        let server = self.server.clone();
        self.installs.insert(
            server.clone(),
            Install::Diagnosing(Diagnosis {
                started: Instant::now(),
                arrived: false,
                confirmed: false,
                shown: 0,
            }),
        );
        self.error = None;
        cx.emit(LaunchpadEvent::Refresh);
        cx.notify();
        let reduce_motion = cx.reduce_motion();
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(CHECK_STEP).await;
                let more = this.update(cx, |this, cx| {
                    if this.server != server {
                        return false;
                    }
                    let Some(Install::Diagnosing(diagnosis)) = this.installs.get_mut(&server)
                    else {
                        return false;
                    };
                    if diagnosis.arrived {
                        diagnosis.shown = if reduce_motion {
                            CHECKS.len()
                        } else {
                            (diagnosis.shown + 1).min(CHECKS.len())
                        };
                        cx.notify();
                    }
                    diagnosis.shown < CHECKS.len()
                });
                if !more.unwrap_or(false) {
                    return;
                }
            }
        })
        .detach();
    }

    /// What the server has for the agent, from its latest status reading.
    fn checks(&self) -> Vec<Check> {
        let value = &self.status.value;
        let name = &self.server_name;
        let missing = missing_packages(value);
        let os = self.os();
        let text = |key: &str| value[key].as_str().unwrap_or_default().to_owned();
        let (system, architecture) = (text("system"), text("architecture"));
        let program = |id: &str, label: &str, present: &str, absent: &str, outcome| match missing
            .iter()
            .find(|package| package.id == id)
        {
            None => Check {
                label: label.to_owned(),
                detail: present.to_owned(),
                outcome: CheckOutcome::Passed,
                fix: None,
            },
            Some(package) => {
                let fix = api::install_command(os.as_ref(), &package.packages);
                Check {
                    label: format!("{label} missing"),
                    detail: if fix.is_some() {
                        absent.to_owned()
                    } else {
                        format!("{absent}. Install it with the server’s package manager.")
                    },
                    outcome,
                    fix,
                }
            }
        };
        vec![
            Check {
                label: "Connected".into(),
                detail: format!("{name} over SSH"),
                outcome: CheckOutcome::Passed,
                fix: None,
            },
            if !system.is_empty() && system != "Linux" {
                Check {
                    label: "Not Linux".into(),
                    detail: format!("Remote apps need a Linux server, and {name} runs {system}."),
                    outcome: CheckOutcome::Blocked,
                    fix: None,
                }
            } else if matches!(architecture.as_str(), "x86_64" | "aarch64") {
                Check {
                    label: format!("Linux on {architecture}"),
                    detail: "An agent is ready for it".into(),
                    outcome: CheckOutcome::Passed,
                    fix: None,
                }
            } else {
                Check {
                    label: format!("Linux on {architecture}"),
                    detail: "There’s no agent for this processor yet.".into(),
                    outcome: CheckOutcome::Blocked,
                    fix: None,
                }
            },
            program(
                "dbus",
                "D-Bus",
                "Needed to start apps",
                "Required to activate",
                CheckOutcome::Blocked,
            ),
            program(
                "xvfb",
                "Xvfb",
                "Opens X11 apps",
                "Most app icons won’t open without it",
                CheckOutcome::Gap,
            ),
            match missing.iter().find(|package| package.id == "audio") {
                None => Check {
                    label: "Sound".into(),
                    detail: "Sound from remote apps".into(),
                    outcome: CheckOutcome::Passed,
                    fix: None,
                },
                Some(package) => Check {
                    label: "No sound".into(),
                    detail: "Apps will be silent".into(),
                    outcome: CheckOutcome::Gap,
                    fix: api::install_command(os.as_ref(), &package.packages),
                },
            },
        ]
    }

    /// Install this build's agent on the server, or replace an older one.
    fn install(&mut self, cx: &mut Context<Self>) {
        if matches!(
            self.installs.get(&self.server),
            Some(Install::Running(..) | Install::Finished(..))
        ) {
            return;
        }
        if !install::available() {
            self.error = Some(INSTALL_UNAVAILABLE.into());
            cx.notify();
            return;
        }
        let server = self.server.clone();
        let architecture = self.status.value["architecture"]
            .as_str()
            .unwrap_or_default()
            .to_owned();
        let (progress, finished) = match install::start(&server, &architecture) {
            Ok(started) => started,
            Err(error) => {
                self.installs.insert(server, Install::Failed(error));
                cx.notify();
                return;
            }
        };
        let activation = Activation {
            started: Instant::now(),
            fresh: matches!(self.status(), api::ServerStatus::NeedsInstall),
        };
        self.installs
            .insert(server.clone(), Install::Running(activation, progress));
        cx.spawn(async move |this, cx| {
            // Redraw the progress until the install ends.
            let result = loop {
                match finished.try_recv() {
                    Ok(result) => break result,
                    Err(async_channel::TryRecvError::Empty) => {
                        cx.background_executor()
                            .timer(Duration::from_millis(150))
                            .await;
                        let shown = this.update(cx, |this, cx| {
                            if this.server == server {
                                cx.notify();
                            }
                        });
                        if shown.is_err() {
                            return;
                        }
                    }
                    Err(async_channel::TryRecvError::Closed) => {
                        break Err("The install stopped unexpectedly.".into());
                    }
                }
            };
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok(_) => {
                        let revision = this.status.revision;
                        this.installs
                            .insert(server.clone(), Install::Finished(activation, revision));
                        if this.server == server {
                            this.refresh(cx);
                        } else {
                            this.installed.insert(server);
                        }
                    }
                    Err(error) => {
                        this.installs.insert(server.clone(), Install::Failed(error));
                    }
                }
                cx.notify();
            });
            // Keep drawing until the minimum time has passed.
            while activation.held() {
                cx.background_executor()
                    .timer(Duration::from_millis(150))
                    .await;
                if this.update(cx, |_, cx| cx.notify()).is_err() {
                    return;
                }
            }
            let _ = this.update(cx, |this, cx| {
                this.settle_activation();
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// Forget a finished activation once the server reports its release and
    /// the activation screen has shown long enough.
    /// The release reported may differ from this build's when the server
    /// reused one it already had.
    fn settle_activation(&mut self) {
        if let Some(Install::Finished(activation, revision)) = self.installs.get(&self.server) {
            let reported =
                self.status.revision != *revision && self.status.value["agent"].is_string();
            if reported && !activation.held() {
                self.installs.remove(&self.server);
            }
        }
    }

    /// Whether this server had the agent when last checked, before this
    /// visit's check answers.
    fn known_installed(&self) -> bool {
        known_installed(&self.installed_record.value, &self.server)
    }

    /// The installed agent's release and the one this build carries for the
    /// server, when they differ.
    fn outdated(&self) -> Option<(&str, &'static str)> {
        let value = &self.status.value;
        let installed = value["revision"].as_str()?;
        let bundled = value["architecture"].as_str().and_then(install::revision)?;
        (bundled != installed).then_some((installed, bundled))
    }

    pub(crate) fn refresh(&mut self, cx: &mut Context<Self>) {
        cx.emit(LaunchpadEvent::Refresh);
        self.error = None;
        cx.notify();
    }

    /// The key that remembers an alert's dismissal on this server.
    fn dismissal(&self, id: &str) -> String {
        format!("{}/{id}", self.server)
    }

    /// The newest readings of this server's queries, from Newport's resources.
    fn observe(&mut self, resources: &state::Resources) {
        self.settings.observe(resources.get(SETTINGS_KEY));
        self.installed_record.observe(resources.get(INSTALLED_KEY));
        self.commands
            .observe(resources.get(&commands_key(&self.server)));
        self.status
            .observe(resources.get(&status_key(&self.server)));
        let read = &self.status;
        if let Some(Install::Diagnosing(diagnosis)) = self.installs.get_mut(&self.server) {
            // Only a read finished after the check began counts, not an
            // older reading the page hadn't drawn yet.
            let fresh = read
                .finished_at
                .is_some_and(|at| at >= diagnosis.started);
            diagnosis.arrived |= fresh && !read.pending;
            diagnosis.confirmed |= diagnosis.arrived && !read.pending && read.error.is_none();
        }
        self.settle_activation();
        self.catalogue
            .observe(resources.get(&catalogue_key(&self.server)));
    }

    /// Show an edit at once; the backend's reply confirms or reverts it.
    fn edit(read: &mut QueryRead, value: Value) {
        read.value = value;
        read.edited = true;
    }

    /// Streaming settings from the backend's preferences.
    fn settings(&self) -> api::StreamingSettings {
        decode_settings(&self.settings.value)
    }

    /// The server's saved commands from the backend's preferences.
    fn saved(&self) -> Vec<SavedCommand> {
        decode_commands(&self.commands.value)
    }

    /// Whether this server can run apps, from the server query.
    fn status(&self) -> api::ServerStatus {
        let read = &self.status;
        if !read.loaded {
            return match &read.error {
                Some(error) if !read.pending => api::ServerStatus::Failed(error.clone()),
                _ => api::ServerStatus::Checking,
            };
        }
        let missing = missing_packages(&read.value);
        if !read.value["agent"].is_string() {
            api::ServerStatus::NeedsInstall
        } else if !missing.is_empty() {
            api::ServerStatus::MissingPackages(missing)
        } else {
            api::ServerStatus::Ready
        }
    }

    /// The installed agent the server query found.
    fn agent(&self) -> Option<String> {
        self.status.value["agent"].as_str().map(str::to_owned)
    }

    fn os(&self) -> Option<api::ServerOs> {
        let release = self.status.value["osRelease"].as_str()?;
        Some(api::ServerOs::parse(release)).filter(|os| !os.id.is_empty())
    }

    /// The server's applications, decoded once per reading.
    fn catalogue(&self) -> api::Catalogue {
        let read = &self.catalogue;
        if !read.loaded {
            return match &read.error {
                Some(error) if !read.pending => api::Catalogue::Failed(error.clone()),
                _ => api::Catalogue::Loading,
            };
        }
        let mut decoded = self.decoded.borrow_mut();
        if decoded
            .as_ref()
            .is_none_or(|(revision, _)| *revision != read.revision)
        {
            *decoded = Some((read.revision, decode_catalogue(&read.value)));
        }
        api::Catalogue::Ready(
            decoded
                .as_ref()
                .map(|(_, apps)| apps.clone())
                .unwrap_or_default(),
        )
    }

    fn apps(&self, _cx: &App) -> Option<Vec<api::RemoteApp>> {
        let api::Catalogue::Ready(mut apps) = self.catalogue() else {
            return None;
        };
        apps.extend(self.saved().into_iter().map(|saved| api::RemoteApp {
            id: command_app_id(&saved.id),
            name: saved.name,
            description: Some(saved.command),
            icon_png: None,
            actions: Vec::new(),
        }));
        Some(apps)
    }

    fn matches(&self, cx: &App) -> Vec<api::RemoteApp> {
        let query = self.search.read(cx).value().trim().to_lowercase();
        let mut apps = self.apps(cx).unwrap_or_default();
        apps.sort_by_key(|app| app.name.to_lowercase());
        if query.is_empty() {
            return apps;
        }
        apps.into_iter()
            .filter(|app| {
                app.name.to_lowercase().contains(&query)
                    || app.id.to_lowercase().contains(&query)
                    || app
                        .description
                        .as_deref()
                        .is_some_and(|d| d.to_lowercase().contains(&query))
            })
            .collect()
    }

    fn launch(&mut self, app: &str, action: Option<String>, cx: &mut Context<Self>) {
        if let Some(id) = app.strip_prefix("command:") {
            return self.run_saved(id, cx);
        }
        let Some(sessions) = sessions(cx) else {
            return;
        };
        // One click opens one session; ignore clicks while it is starting.
        if sessions.list().iter().any(|s| {
            s.server == self.server && s.app_id == app && s.state == api::SessionState::Starting
        }) {
            return;
        }
        let name = self
            .apps(cx)
            .and_then(|apps| apps.into_iter().find(|candidate| candidate.id == app))
            .map(|app| app.name)
            .unwrap_or_else(|| app.to_owned());
        let request = api::LaunchRequest {
            app_id: app.to_owned(),
            name,
            action,
        };
        let agent = self.agent().unwrap_or_default();
        let launch = self.launch_options(&agent, cx);
        self.error = sessions.launch(launch, request, cx).err();
        cx.notify();
    }

    fn launch_options<'a>(&'a self, agent: &'a str, cx: &App) -> Launch<'a> {
        Launch {
            server: &self.server,
            server_name: &self.server_name,
            agent,
            x11: !missing_packages(&self.status.value)
                .iter()
                .any(|package| package.name == "Xvfb"),
            settings: self.settings(),
            clipboard: clipboard(&self.server, cx),
        }
    }

    fn icon(&mut self, app: &api::RemoteApp) -> Option<Arc<Image>> {
        let png = app.icon_png.as_ref()?;
        Some(
            self.icons
                .entry((app.id.clone(), png.len()))
                .or_insert_with(|| Arc::new(Image::from_bytes(ImageFormat::Png, png.to_vec())))
                .clone(),
        )
    }

    fn toolbar(&self, apps: usize, cx: &mut Context<Self>) -> Div {
        let summary = match apps {
            0 => String::new(),
            1 => "1 app".into(),
            n => format!("{n} apps"),
        };
        div()
            .flex_shrink_0()
            .flex()
            .items_center()
            .gap(px(12.))
            .px(px(crate::controls::PAGE_PADDING))
            .py(px(10.))
            .border_b_1()
            .border_color(cx.theme().border)
            .child(
                div().w(px(280.)).min_w(px(160.)).child(
                    crate::controls::input(&self.search)
                        .prefix(Icon::new(IconName::Search).size(px(14.))),
                ),
            )
            .child(div().flex_1())
            .child(
                div()
                    .text_size(px(crate::ui::FONT_SIZE))
                    .text_color(cx.theme().muted_foreground)
                    .child(summary),
            )
            .child(
                div().debug_selector(|| "remote-apps-run".into()).child(
                    crate::controls::button("remote-apps-run")
                        .icon(IconName::SquareTerminal)
                        .label("Run command")
                        .selected(self.runner.is_some())
                        .on_click(cx.listener(|this, _, window, cx| {
                            if this.runner.take().is_none() {
                                this.open_runner(None, window, cx);
                            }
                            cx.notify();
                        })),
                ),
            )
            .child(
                crate::controls::icon_button("remote-apps-refresh")
                    .icon(IconName::RefreshCw)
                    .tooltip("Refresh")
                    .on_click(cx.listener(|this, _, _, cx| this.refresh(cx))),
            )
            .child(
                div()
                    .debug_selector(|| "remote-apps-preferences".into())
                    .child(
                        crate::controls::icon_button("remote-apps-preferences")
                            .icon(IconName::Settings2)
                            .tooltip("Preferences")
                            .selected(self.preferences_open)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.preferences_open = !this.preferences_open;
                                this.runner = None;
                                cx.notify();
                            })),
                    ),
            )
    }

    /// Centered message for states without applications to show.
    fn hero(
        &self,
        icon: IconName,
        title: impl Into<SharedString>,
        description: impl Into<SharedString>,
        cx: &App,
    ) -> Div {
        div()
            .flex_1()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .p(px(crate::controls::PAGE_PADDING))
            .child(
                div()
                    .max_w(px(420.))
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap(px(12.))
                    .child(
                        div()
                            .size(px(72.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .bg(cx.theme().muted)
                            .child(
                                Icon::new(icon)
                                    .size(px(32.))
                                    .text_color(cx.theme().muted_foreground),
                            ),
                    )
                    .child(
                        div()
                            .id("remote-apps-hero-title")
                            .role(Role::Heading)
                            .aria_level(2)
                            .text_size(px(17.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_center()
                            .child(title.into()),
                    )
                    .child(
                        div()
                            .text_size(px(crate::ui::FONT_SIZE))
                            .line_height(px(20.))
                            .text_center()
                            .text_color(cx.theme().muted_foreground)
                            .child(description.into()),
                    ),
            )
    }

    /// The first screen: a wall of app icons, lit as far as an install has
    /// come, above the heading and one sentence.
    fn welcome(&self, lit: usize, sentence: impl Into<SharedString>, cx: &App) -> Div {
        let dark = cx.theme().is_dark();
        let reduce_motion = cx.reduce_motion();
        let tiles = WALL
            .iter()
            .zip(LIGHT_RANK)
            .enumerate()
            .map(|(index, (&(icon, hue), rank))| {
                let on = rank < lit;
                let (fill, ink) = if on {
                    theme::launch_tile(hue, dark)
                } else {
                    (cx.theme().muted, cx.theme().muted_foreground)
                };
                let tile = div()
                    // A new ID when a tile lights restarts its fade.
                    .id(("remote-apps-wall", index * 2 + usize::from(on)))
                    .size(px(WALL_TILE))
                    .flex_shrink_0()
                    .rounded(px(14.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .bg(fill)
                    .child(Icon::new(icon).size(px(26.)).text_color(ink));
                if !on {
                    tile.opacity(0.55).into_any_element()
                } else if reduce_motion {
                    tile.into_any_element()
                } else {
                    tile.with_animation(
                        ("remote-apps-wall-light", index),
                        Animation::new(Duration::from_millis(450)),
                        |tile, phase| tile.opacity(0.55 + 0.45 * phase),
                    )
                    .into_any_element()
                }
            });
        div()
            .flex_1()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .p(px(crate::controls::PAGE_PADDING))
            .child(
                div()
                    .max_w(px(8. * WALL_TILE + 7. * WALL_GAP))
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap(px(8.))
                    .child(
                        div()
                            .debug_selector(|| "remote-apps-wall".into())
                            .flex()
                            .flex_wrap()
                            .justify_center()
                            .gap(px(WALL_GAP))
                            .pb(px(20.))
                            .children(tiles),
                    )
                    .child(
                        div()
                            .id("remote-apps-hero-title")
                            .role(Role::Heading)
                            .aria_level(2)
                            .text_size(px(22.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_center()
                            .child("Open remote apps directly on your desktop"),
                    )
                    .child(
                        div()
                            .pb(px(12.))
                            .text_size(px(14.))
                            .line_height(px(20.))
                            .text_center()
                            .text_color(cx.theme().muted_foreground)
                            .child(sentence.into()),
                    ),
            )
    }

    fn status_hero(&self, status: &api::ServerStatus, cx: &mut Context<Self>) -> Option<Div> {
        let name = self.server_name.clone();
        const SENTENCE: &str = "Each app opens in its own window, right next to your Mac apps.";
        let activating = matches!(
            self.installs.get(&self.server),
            Some(Install::Running(activation, _) | Install::Finished(activation, _))
                if activation.fresh
        );
        Some(match status {
            // A first install's screen stays until it has finished.
            api::ServerStatus::Ready | api::ServerStatus::MissingPackages(_) if activating => {
                self.activation_screen(cx)
            }
            api::ServerStatus::Ready | api::ServerStatus::MissingPackages(_) => return None,
            // A server that had the agent shows its apps loading; others keep
            // the wall while the first check decides what follows.
            api::ServerStatus::Checking if self.known_installed() => return None,
            api::ServerStatus::Checking => self.welcome(0, SENTENCE, cx),
            api::ServerStatus::NeedsInstall => match self.installs.get(&self.server) {
                Some(Install::Diagnosing(diagnosis)) => self.diagnosis(diagnosis, cx),
                Some(Install::Running(..) | Install::Finished(..)) => self.activation_screen(cx),
                Some(Install::Failed(_)) | None => self.welcome(0, SENTENCE, cx).child(
                    div()
                        .debug_selector(|| "remote-apps-get-started".into())
                        .child(
                            crate::controls::button("remote-apps-get-started")
                                .primary()
                                .label("Get started")
                                .on_click(cx.listener(|this, _, _, cx| this.diagnose(cx))),
                        ),
                ),
            },
            api::ServerStatus::Failed(error) => self
                .hero(
                    IconName::TriangleAlert,
                    format!("Couldn’t check {name}"),
                    error.clone(),
                    cx,
                )
                .child(
                    div().pt(px(8.)).child(
                        crate::controls::button("remote-apps-retry")
                            .label("Try again")
                            .on_click(cx.listener(|this, _, _, cx| this.refresh(cx))),
                    ),
                ),
        })
    }

    /// The wall lighting up after Activate, never faster than
    /// [`ACTIVATION_MIN`].
    fn activation_screen(&self, cx: &mut Context<Self>) -> Div {
        const SENTENCE: &str = "Each app opens in its own window, right next to your Mac apps.";
        let (shown, label) = match self.installs.get(&self.server) {
            Some(Install::Running(activation, progress)) => {
                let shown = activation.shown(progress.fraction());
                let label = if shown < 1. {
                    format!("Activating… {}%", (shown * 100.) as u32)
                } else {
                    "Setting up…".to_owned()
                };
                (shown, label)
            }
            Some(Install::Finished(activation, _)) => {
                let shown = activation.shown(1.);
                let label = if shown < 1. {
                    format!("Activating… {}%", (shown * 100.) as u32)
                } else {
                    "Finding apps…".to_owned()
                };
                (shown, label)
            }
            _ => (1., "Finding apps…".to_owned()),
        };
        let lit = (shown * WALL.len() as f32).round() as usize;
        let bar = (label != "Finding apps…").then_some(shown);
        self.welcome(lit, SENTENCE, cx).child(
            div()
                .flex()
                .flex_col()
                .items_center()
                .gap(px(10.))
                .child(
                    div()
                        .debug_selector(|| "remote-apps-activating".into())
                        .child(
                            crate::controls::button("remote-apps-activating")
                                .primary()
                                .label(label)
                                .loading(true),
                        ),
                )
                .when_some(bar, |column, fraction| {
                    column.child(
                        div()
                            .w(px(220.))
                            .h(px(4.))
                            .debug_selector(|| "remote-apps-install-progress".into())
                            .child(
                                gpui_kit::component::progress::Progress::new(
                                    "remote-apps-install-bar",
                                )
                                .accessibility_label("Activating Newport Remote")
                                .color(cx.theme().primary)
                                .with_size(px(4.))
                                .value(fraction * 100.),
                            ),
                    )
                }),
        )
    }

    /// The check after Get started: each row shows what the server has, and
    /// the button below activates the agent or asks to check again.
    fn diagnosis(&self, diagnosis: &Diagnosis, cx: &mut Context<Self>) -> Div {
        let name = &self.server_name;
        let dark = cx.theme().is_dark();
        let failed = diagnosis.arrived && !self.status.pending && !diagnosis.confirmed;
        let error = self.status.error.clone().filter(|_| failed);
        let mut checks = self.checks();
        if let Some(error) = &error {
            // The server couldn't be reached; nothing below can be known.
            checks.truncate(1);
            checks[0] = Check {
                label: "Couldn’t connect".into(),
                detail: error.clone(),
                outcome: CheckOutcome::Blocked,
                fix: None,
            };
        }
        let shown = if diagnosis.arrived {
            diagnosis.shown.min(checks.len())
        } else {
            0
        };
        let done = diagnosis.arrived && shown >= checks.len();
        let blocked = checks
            .iter()
            .filter(|c| c.outcome == CheckOutcome::Blocked)
            .count();
        let gaps = checks
            .iter()
            .filter(|c| c.outcome == CheckOutcome::Gap)
            .count();
        let (title, subtitle) = match (done, blocked, gaps) {
            (false, ..) => (
                format!("Checking {name}"),
                "Making sure your apps can run there.".to_owned(),
            ),
            (true, 1.., _) => (
                format!("{name} needs {}", count(blocked, "fix", "fixes")),
                "Install what’s missing, then check again.".to_owned(),
            ),
            (true, 0, 1..) => (
                format!("Ready, with {}", count(gaps, "gap", "gaps")),
                "You can activate now and add these any time.".to_owned(),
            ),
            (true, 0, 0) => (
                format!("{name} is ready"),
                "Everything your apps need is there.".to_owned(),
            ),
        };
        let reduce_motion = cx.reduce_motion();
        let rows = checks.into_iter().enumerate().map(|(index, check)| {
            // A row's identity follows its result, so a changed result fades
            // in while unchanged rows stay as they are.
            let state = if index < shown {
                match check.outcome {
                    CheckOutcome::Passed => 1,
                    CheckOutcome::Gap => 2,
                    CheckOutcome::Blocked => 3,
                }
            } else {
                0
            };
            let revealed = state > 0;
            let row = div()
                .id(("remote-apps-check", index * 4 + state))
                .debug_selector(move || format!("remote-apps-check-{index}"))
                .w_full()
                .flex()
                .items_center()
                .gap(px(10.))
                .py(px(9.))
                .when(index > 0, |row| {
                    row.border_t_1().border_color(cx.theme().border)
                });
            let (mark, label, detail): (AnyElement, String, String) = if index < shown {
                let (icon, color) = match check.outcome {
                    CheckOutcome::Passed => (IconName::CircleCheck, theme::success(dark)),
                    CheckOutcome::Gap => (IconName::TriangleAlert, theme::warning(dark)),
                    CheckOutcome::Blocked => (IconName::CircleX, theme::error(dark)),
                };
                (
                    Icon::new(icon)
                        .size(px(17.))
                        .text_color(color)
                        .into_any_element(),
                    check.label.clone(),
                    check.detail.clone(),
                )
            } else if index == shown && !done {
                (
                    gpui_kit::component::spinner::Spinner::new()
                        .with_size(px(16.))
                        .color(cx.theme().muted_foreground)
                        .into_any_element(),
                    CHECKS[index].to_owned(),
                    "Checking…".to_owned(),
                )
            } else {
                (
                    Icon::new(IconName::Circle)
                        .size(px(16.))
                        .text_color(cx.theme().muted_foreground)
                        .into_any_element(),
                    CHECKS[index].to_owned(),
                    "Waiting".to_owned(),
                )
            };
            let row = row
                .when(index > shown, |row| row.opacity(0.45))
                .child(div().w(px(20.)).flex().justify_center().child(mark))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .child(div().text_size(px(13.)).child(label))
                        .child(
                            div()
                                .text_size(px(12.))
                                .text_color(cx.theme().muted_foreground)
                                .child(detail),
                        ),
                )
                .when_some(check.fix.filter(|_| index < shown), |row, command| {
                    let open = self
                        .terminal
                        .as_ref()
                        .is_some_and(|terminal| terminal.command == command);
                    let run = command.clone();
                    let title = match CHECKS[index] {
                        "Sound" => "Install sound tools".to_owned(),
                        program => format!("Install {program}"),
                    };
                    row.child(
                        div()
                            .debug_selector(move || format!("remote-apps-check-fix-{index}"))
                            .child(
                                crate::controls::compact_button(("remote-apps-check-fix", index))
                                    .icon(IconName::SquareTerminal)
                                    .label(if open { "Installing…" } else { "Install" })
                                    .tooltip(command)
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.open_terminal(title.clone(), run.clone(), window, cx)
                                    })),
                            ),
                    )
                });
            if revealed && !reduce_motion {
                row.with_animation(
                    ("remote-apps-check-result", index * 4 + state),
                    Animation::new(Duration::from_millis(350)),
                    |row, phase| row.opacity(0.35 + 0.65 * phase),
                )
                .into_any_element()
            } else {
                row.into_any_element()
            }
        });
        let action = if !done {
            crate::controls::button("remote-apps-checking")
                .label("Checking…")
                .loading(true)
                .into_any_element()
        } else if blocked > 0 {
            div()
                .debug_selector(|| "remote-apps-check-again".into())
                .child(
                    crate::controls::button("remote-apps-check-again")
                        .label(if self.status.pending {
                            "Checking…"
                        } else {
                            "Check again"
                        })
                        .loading(self.status.pending)
                        .on_click(cx.listener(|this, _, _, cx| this.recheck(cx))),
                )
                .into_any_element()
        } else {
            div()
                .debug_selector(|| "remote-apps-activate".into())
                .child(
                    crate::controls::button("remote-apps-activate")
                        .primary()
                        .label("Activate")
                        .on_click(cx.listener(|this, _, _, cx| this.install(cx))),
                )
                .into_any_element()
        };
        div()
            .flex_1()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .p(px(crate::controls::PAGE_PADDING))
            .child(
                div()
                    .debug_selector(|| "remote-apps-diagnosis".into())
                    .w_full()
                    .max_w(px(470.))
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap(px(6.))
                    .child(
                        div()
                            .id("remote-apps-hero-title")
                            .role(Role::Heading)
                            .aria_level(2)
                            .text_size(px(20.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_center()
                            .child(title),
                    )
                    .child(
                        div()
                            .pb(px(14.))
                            .text_size(px(13.))
                            .text_center()
                            .text_color(cx.theme().muted_foreground)
                            .child(subtitle),
                    )
                    .child(div().w_full().flex().flex_col().children(rows))
                    .child(div().pt(px(18.)).child(action)),
            )
    }

    fn open_runner(
        &mut self,
        saved: Option<SavedCommand>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (command_text, name_text) = saved
            .as_ref()
            .map(|saved| (saved.command.clone(), saved.name.clone()))
            .unwrap_or_default();
        let command = cx.new(|cx| {
            let mut input =
                InputState::new(window, cx).placeholder("/opt/tool/bin/app --flag value");
            input.set_value(command_text, window, cx);
            input
        });
        let name = cx.new(|cx| {
            let mut input = InputState::new(window, cx).placeholder("Name in Remote apps");
            input.set_value(name_text, window, cx);
            input
        });
        let subscription = cx.subscribe(&command, |this, _, event, cx| match event {
            InputEvent::PressEnter { .. } => this.submit_runner(true, cx),
            InputEvent::Change => {
                if let Some(runner) = &mut this.runner {
                    runner.error = None;
                }
                cx.notify();
            }
            _ => {}
        });
        command.update(cx, |input, cx| input.focus(window, cx));
        self.preferences_open = false;
        self.runner = Some(Runner {
            editing: saved.as_ref().map(|saved| saved.id.clone()),
            save: saved.is_some(),
            display: saved.map(|saved| saved.display).unwrap_or_default(),
            command,
            name,
            error: None,
            _subscription: subscription,
        });
        cx.notify();
    }

    /// Run the panel's command, saving it first when asked. Without `run`,
    /// only saves an edited command.
    fn submit_runner(&mut self, run: bool, cx: &mut Context<Self>) {
        let Some(runner) = &mut self.runner else {
            return;
        };
        let text = runner.command.read(cx).value().trim().to_owned();
        let parsed = parse_command(&text);
        let (env, argv) = match parsed {
            Ok(parsed) => parsed,
            Err(error) => {
                runner.error = Some(error);
                cx.notify();
                return;
            }
        };
        let typed = runner.name.read(cx).value().trim().to_owned();
        let name = if typed.is_empty() {
            program_name(&argv[0])
        } else {
            typed
        };
        let id = runner
            .editing
            .clone()
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        let saved = SavedCommand {
            id: id.clone(),
            name: name.clone(),
            command: text.clone(),
            display: runner.display,
        };
        if runner.save {
            // A retry after a failed start updates this command, not a copy.
            runner.editing = Some(id.clone());
            let mut commands = decode_commands(&self.commands.value);
            match commands.iter_mut().find(|command| command.id == id) {
                Some(existing) => *existing = saved.clone(),
                None => commands.push(saved.clone()),
            }
            let value = Value::Array(
                commands
                    .iter()
                    .map(|command| encode_command(&self.server, command))
                    .collect(),
            );
            Self::edit(&mut self.commands, value);
            cx.emit(LaunchpadEvent::SaveCommand {
                server: self.server.clone(),
                command: encode_command(&self.server, &saved),
            });
        }
        if run {
            let request = api::CommandRequest {
                id: command_app_id(&id),
                name,
                argv,
                env,
                display: runner.display,
            };
            let Some(sessions) = sessions(cx) else {
                return;
            };
            let agent = self.agent().unwrap_or_default();
            let launch = self.launch_options(&agent, cx);
            if let Err(error) = sessions.run(launch, request, cx) {
                if let Some(runner) = &mut self.runner {
                    runner.error = Some(error);
                }
                cx.notify();
                return;
            }
            self.recent.retain(|recent| *recent != text);
            self.recent.insert(0, text);
            self.recent.truncate(5);
        }
        self.runner = None;
        cx.notify();
    }

    fn run_saved(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(saved) = self.saved().into_iter().find(|saved| saved.id == id) else {
            return;
        };
        let Some(sessions) = sessions(cx) else {
            return;
        };
        let agent = self.agent().unwrap_or_default();
        self.error = parse_command(&saved.command)
            .and_then(|(env, argv)| {
                let request = api::CommandRequest {
                    id: command_app_id(&saved.id),
                    name: saved.name.clone(),
                    argv,
                    env,
                    display: saved.display,
                };
                sessions.run(self.launch_options(&agent, cx), request, cx)
            })
            .err();
        cx.notify();
    }

    fn remove_saved(&mut self, id: &str, cx: &mut Context<Self>) {
        let mut value = self.commands.value.clone();
        if let Some(rows) = value.as_array_mut() {
            rows.retain(|row| row["id"] != id);
        }
        Self::edit(&mut self.commands, value);
        cx.emit(LaunchpadEvent::RemoveCommand {
            server: self.server.clone(),
            id: id.to_owned(),
        });
        cx.notify();
    }

    /// The command invoker, beside the applications.
    fn runner_panel(&self, runner: &Runner, cx: &mut Context<Self>) -> Stateful<Div> {
        let muted = cx.theme().muted_foreground;
        let label = |text: &'static str| {
            div()
                .text_size(px(crate::ui::FONT_SIZE))
                .font_weight(FontWeight::SEMIBOLD)
                .child(text)
        };
        let note = |text: String| {
            div()
                .text_size(px(12.))
                .line_height(px(16.))
                .text_color(muted)
                .child(text)
        };
        let display = ButtonGroup::new("remote-apps-run-display").children(
            [
                (api::DisplayBackend::Auto, "Automatic"),
                (api::DisplayBackend::X11, "X11"),
                (api::DisplayBackend::Wayland, "Wayland"),
            ]
            .into_iter()
            .enumerate()
            .map(|(index, (display, name))| {
                crate::controls::compact_button(("remote-apps-run-display", index))
                    .label(name)
                    .selected(runner.display == display)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Some(runner) = &mut this.runner {
                            runner.display = display;
                        }
                        cx.notify();
                    }))
            }),
        );
        let editing = runner.editing.is_some();
        let mut panel = div()
            .id("remote-apps-run-panel")
            .debug_selector(|| "remote-apps-run-panel".into())
            .w(px(320.))
            .flex_shrink_0()
            .h_full()
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .gap(px(18.))
            .p(px(crate::controls::PAGE_PADDING))
            .border_l_1()
            .border_color(cx.theme().border)
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .id("remote-apps-run-title")
                            .role(Role::Heading)
                            .aria_level(2)
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(if editing {
                                "Edit command"
                            } else {
                                "Run a command"
                            }),
                    )
                    .child(
                        crate::controls::compact_icon_button("remote-apps-run-close")
                            .icon(IconName::X)
                            .tooltip("Close")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.runner = None;
                                cx.notify();
                            })),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(8.))
                    .child(label("Command"))
                    .child(
                        div()
                            .debug_selector(|| "remote-apps-run-command".into())
                            .font_family(cx.theme().mono_font_family.clone())
                            .child(crate::controls::input(&runner.command)),
                    )
                    .child(note(format!(
                        "Runs on {} without a shell. Quote arguments with spaces. Put NAME=value before the program to set environment variables.",
                        self.server_name
                    )))
                    .when_some(runner.error.clone(), |field, error| {
                        field.child(
                            div()
                                .text_size(px(12.))
                                .text_color(cx.theme().danger)
                                .child(error),
                        )
                    }),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(8.))
                    .child(label("Display"))
                    .child(display)
                    .child(note(
                        "Automatic uses X11 when the server has it, and Wayland otherwise. Choose Wayland for apps that only support it."
                            .into(),
                    )),
            );
        if !editing {
            panel = panel.child(
                div().flex().child(
                    div()
                        .debug_selector(|| "remote-apps-run-save".into())
                        .child(
                            crate::controls::checkbox("remote-apps-run-save")
                                .checked(runner.save)
                                .label("Add to Remote apps")
                                .on_click(cx.listener(|this, value: &bool, _, cx| {
                                    if let Some(runner) = &mut this.runner {
                                        runner.save = *value;
                                    }
                                    cx.notify();
                                })),
                        ),
                ),
            );
        }
        if runner.save {
            panel = panel.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(8.))
                    .child(label("Name"))
                    .child(crate::controls::input(&runner.name))
                    .child(note("Leave empty to use the program’s name.".into())),
            );
        }
        let mut buttons = div().flex().items_center().gap(px(8.)).child(
            div()
                .debug_selector(|| "remote-apps-run-submit".into())
                .child(
                    crate::controls::button("remote-apps-run-submit")
                        .primary()
                        .icon(IconName::Play)
                        .label("Run")
                        .on_click(cx.listener(|this, _, _, cx| this.submit_runner(true, cx))),
                ),
        );
        if editing {
            buttons = buttons.child(
                crate::controls::button("remote-apps-run-update")
                    .label("Save")
                    .on_click(cx.listener(|this, _, _, cx| this.submit_runner(false, cx))),
            );
        }
        panel = panel.child(buttons);
        if !editing && !self.recent.is_empty() {
            let mut recent = div().flex().flex_col().gap(px(2.)).child(label("Recent"));
            for (index, command) in self.recent.iter().enumerate() {
                let fill = command.clone();
                recent = recent.child(
                    div()
                        .id(("remote-apps-run-recent", index))
                        .px(px(6.))
                        .py(px(4.))
                        .truncate()
                        .cursor_pointer()
                        .font_family(cx.theme().mono_font_family.clone())
                        .text_size(px(12.))
                        .hover(|style| style.bg(theme::row_hover(cx)))
                        .child(command.clone())
                        .on_click(cx.listener(move |this, _, window, cx| {
                            if let Some(runner) = &this.runner {
                                runner.command.update(cx, |input, cx| {
                                    input.set_value(fill.clone(), window, cx)
                                });
                            }
                        })),
                );
            }
            panel = panel.child(recent);
        }
        panel
    }

    /// Everything wrong on this server, most severe first, with whether the
    /// user may dismiss it.
    fn alerts(
        &self,
        status: &api::ServerStatus,
        sessions: &[api::SessionSummary],
    ) -> Vec<(api::Alert, bool)> {
        let server = self.server_name.as_str();
        let mut alerts = Vec::new();
        let os = self.os();
        if let api::ServerStatus::MissingPackages(packages) = status {
            for package in packages {
                let (severity, title) = if package.required {
                    (api::Severity::Error, format!("{} is missing", package.name))
                } else {
                    (
                        api::Severity::Warning,
                        format!("{} is unavailable", package.purpose),
                    )
                };
                let detail = install_guidance(server, os.as_ref(), &package.name, package.required);
                // Required software cannot be dismissed: nothing launches without it.
                alerts.push((
                    api::Alert {
                        id: format!("missing:{}", package.name),
                        severity,
                        title,
                        detail,
                        fix: Some(api::Fix::Install(package.packages.clone())),
                    },
                    !package.required,
                ));
            }
        }
        for (read, what) in [
            (&self.settings, "preferences"),
            (&self.commands, "saved commands"),
        ] {
            if let Some(error) = read.error.as_ref().filter(|_| !read.pending) {
                alerts.push((
                    api::Alert {
                        id: format!("preferences:{what}:{error}"),
                        severity: api::Severity::Error,
                        title: format!("Couldn’t save or read Remote apps {what}"),
                        detail: error.clone(),
                        fix: None,
                    },
                    true,
                ));
            }
        }
        match self.installs.get(&self.server) {
            Some(Install::Failed(error)) => alerts.push((
                api::Alert {
                    id: format!("install:{error}"),
                    severity: api::Severity::Error,
                    title: format!("Couldn’t install Newport Remote on {server}"),
                    detail: error.clone(),
                    fix: None,
                },
                true,
            )),
            Some(Install::Running(activation, progress)) if !activation.fresh => alerts.push((
                api::Alert {
                    id: "update:running".into(),
                    severity: api::Severity::Warning,
                    title: if progress.uploaded() {
                        "Setting up the new Newport Remote…".into()
                    } else {
                        format!(
                            "Updating Newport Remote… {}%",
                            (progress.fraction() * 100.) as u32
                        )
                    },
                    detail: "Apps that are open keep running.".into(),
                    fix: None,
                },
                false,
            )),
            Some(Install::Finished(..) | Install::Running(..)) => {}
            _ => {
                if matches!(
                    status,
                    api::ServerStatus::Ready | api::ServerStatus::MissingPackages(_)
                ) && let Some((installed, bundled)) = self.outdated()
                {
                    alerts.push((
                        api::Alert {
                            // A later build's agent asks again.
                            id: format!("update:{installed}:{bundled}"),
                            severity: api::Severity::Warning,
                            title: "Another version of Newport Remote is available".into(),
                            detail: format!(
                                "Update installs this build’s version on {server}. Apps that are open keep running."
                            ),
                            fix: Some(api::Fix::UpdateAgent),
                        },
                        true,
                    ));
                }
            }
        }
        if let Some(error) = &self.error {
            alerts.push((
                api::Alert {
                    id: format!("error:{error}"),
                    severity: api::Severity::Error,
                    title: "Something went wrong".into(),
                    detail: error.clone(),
                    fix: None,
                },
                true,
            ));
        }
        for session in sessions {
            if let api::SessionState::Ended { error: Some(error) } = &session.state {
                // Tray apps and daemons start fine but never show a window.
                let windowless = error.contains("did not open a window");
                alerts.push((
                    api::Alert {
                        id: format!("session:{}", session.id.0),
                        severity: if windowless {
                            api::Severity::Warning
                        } else {
                            api::Severity::Error
                        },
                        title: if windowless {
                            format!("{} didn’t open a window", session.app_name)
                        } else {
                            format!("{} closed unexpectedly", session.app_name)
                        },
                        detail: if windowless {
                            "It may run in the background, such as in the system tray, which Remote apps can’t show.".into()
                        } else {
                            error.clone()
                        },
                        fix: None,
                    },
                    true,
                ));
            }
        }
        alerts.retain(|(alert, _)| !self.dismissed.contains(&self.dismissal(&alert.id)));
        alerts.sort_by_key(|(alert, _)| std::cmp::Reverse(alert.severity));
        alerts
    }

    /// One alert: what is wrong, and the actions that fix it.
    fn alert(
        &self,
        index: usize,
        alert: api::Alert,
        dismissible: bool,
        cx: &mut Context<Self>,
    ) -> Div {
        let (icon, color) = match alert.severity {
            api::Severity::Error => (IconName::CircleAlert, cx.theme().danger),
            api::Severity::Warning => (IconName::TriangleAlert, cx.theme().warning),
        };
        let check = |index| {
            crate::controls::compact_button(("remote-apps-alert-check", index))
                .label("Check again")
                .on_click(cx.listener(|this, _, _, cx| this.refresh(cx)))
        };
        let mut actions = div().flex_shrink_0().flex().items_center().gap(px(6.));
        let mut command_line = None;
        let os = self.os();
        let fix = match alert.fix.clone() {
            // Without a command for this OS the user installs it their own way.
            Some(api::Fix::Install(packages)) => Some(
                api::install_command(os.as_ref(), &packages)
                    .map_or(api::Fix::Refresh, api::Fix::Command),
            ),
            fix => fix,
        };
        match fix {
            Some(api::Fix::Command(command)) => {
                let (run, copy) = (command.clone(), command.clone());
                // "missing:Xvfb" installs Xvfb.
                let title = match alert.id.strip_prefix("missing:") {
                    Some(program) => format!("Install {program}"),
                    None => alert.title.clone(),
                };
                command_line = Some(command);
                actions = actions
                    .child(
                        div()
                            .debug_selector(move || format!("remote-apps-alert-run-{index}"))
                            .child(
                                crate::controls::compact_button(("remote-apps-alert-run", index))
                                    .icon(IconName::SquareTerminal)
                                    .label("Install")
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.open_terminal(title.clone(), run.clone(), window, cx)
                                    })),
                            ),
                    )
                    .child(
                        crate::controls::compact_icon_button(("remote-apps-alert-copy", index))
                            .icon(IconName::Copy)
                            .tooltip("Copy command")
                            .on_click(move |_, _, cx| {
                                cx.write_to_clipboard(ClipboardItem::new_string(copy.clone()))
                            }),
                    )
                    .child(check(index));
            }
            Some(api::Fix::Refresh | api::Fix::Install(_)) => actions = actions.child(check(index)),
            Some(api::Fix::UpdateAgent) => {
                actions = actions.child(
                    div()
                        .debug_selector(move || format!("remote-apps-alert-update-{index}"))
                        .child(
                            crate::controls::compact_button(("remote-apps-alert-update", index))
                                .label("Update")
                                .on_click(cx.listener(|this, _, _, cx| this.install(cx))),
                        ),
                );
            }
            None => {}
        }
        if dismissible {
            let id = alert.id.clone();
            actions = actions.child(
                div()
                    .debug_selector(move || format!("remote-apps-alert-dismiss-{index}"))
                    .child(
                        crate::controls::compact_icon_button(("remote-apps-alert-dismiss", index))
                            .icon(IconName::X)
                            .tooltip("Dismiss")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if id.starts_with("error:") {
                                    this.error = None;
                                } else if id.starts_with("install:") {
                                    this.installs.remove(&this.server);
                                } else if let Some(session) = id
                                    .strip_prefix("session:")
                                    .and_then(|session| session.parse().ok())
                                    && let Some(sessions) = sessions(cx)
                                {
                                    // A failed session has nothing left to show.
                                    sessions.forget(api::SessionId(session));
                                } else {
                                    let dismissal = this.dismissal(&id);
                                    this.dismissed.insert(dismissal);
                                }
                                cx.notify();
                            })),
                    ),
            );
        }
        div()
            .debug_selector(move || format!("remote-apps-alert-{index}"))
            .flex_shrink_0()
            .flex()
            .items_center()
            .gap(px(10.))
            .px(px(crate::controls::PAGE_PADDING))
            .py(px(10.))
            .border_b_1()
            .border_color(cx.theme().border)
            .bg(color.opacity(0.1))
            .child(Icon::new(icon).size(px(16.)).text_color(color))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .text_size(px(crate::ui::FONT_SIZE))
                    .child(div().font_weight(FontWeight::SEMIBOLD).child(alert.title))
                    .child(
                        div()
                            .text_color(cx.theme().muted_foreground)
                            .child(alert.detail),
                    )
                    .when_some(command_line, |text, command| {
                        text.child(
                            div()
                                .mt(px(2.))
                                .truncate()
                                .font_family(cx.theme().mono_font_family.clone())
                                .text_size(px(12.))
                                .child(command),
                        )
                    }),
            )
            .child(actions)
    }

    fn save(&mut self, settings: api::StreamingSettings, cx: &mut Context<Self>) {
        Self::edit(&mut self.settings, encode_settings(settings));
        cx.emit(LaunchpadEvent::SaveSettings(self.settings.value.clone()));
        cx.notify();
    }

    /// Streaming preferences, beside the applications.
    fn preferences(&self, cx: &mut Context<Self>) -> Stateful<Div> {
        let current = self.settings();
        let muted = cx.theme().muted_foreground;
        let section = |title: &'static str, note: &'static str, control: AnyElement| {
            div()
                .flex()
                .flex_col()
                .gap(px(8.))
                .child(
                    div()
                        .text_size(px(crate::ui::FONT_SIZE))
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(title),
                )
                .child(control)
                .child(
                    div()
                        .text_size(px(12.))
                        .line_height(px(16.))
                        .text_color(muted)
                        .child(note),
                )
        };
        let quality = ButtonGroup::new("remote-apps-quality").children(
            [
                (api::CodecPreference::Auto, "Balanced"),
                (api::CodecPreference::Lossless, "Lossless"),
                (api::CodecPreference::Fast, "Low latency"),
            ]
            .into_iter()
            .enumerate()
            .map(|(index, (codec, name))| {
                crate::controls::compact_button(("remote-apps-quality", index))
                    .label(name)
                    .selected(current.codec == codec)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let settings = api::StreamingSettings {
                            codec,
                            ..this.settings()
                        };
                        this.save(settings, cx)
                    }))
            }),
        );
        let quality_note = match current.codec {
            api::CodecPreference::Auto => {
                "Sharp text with modest bandwidth. Video adapts to your connection."
            }
            api::CodecPreference::Lossless => "Exact pixels. Uses the most bandwidth.",
            api::CodecPreference::Fast => {
                "Least processing on the server, for the lowest delay. Uses more bandwidth."
            }
        };
        let rate =
            ButtonGroup::new("remote-apps-fps").children([30u32, 60].into_iter().map(|fps| {
                crate::controls::compact_button(("remote-apps-fps", fps as usize))
                    .label(format!("{fps} fps"))
                    .selected(current.fps_goal == fps)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let settings = api::StreamingSettings {
                            fps_goal: fps,
                            ..this.settings()
                        };
                        this.save(settings, cx)
                    }))
            }));
        div()
            .id("remote-apps-preferences-panel")
            .debug_selector(|| "remote-apps-preferences-panel".into())
            .w(px(300.))
            .flex_shrink_0()
            .h_full()
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .gap(px(22.))
            .p(px(crate::controls::PAGE_PADDING))
            .border_l_1()
            .border_color(cx.theme().border)
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .id("remote-apps-preferences-title")
                            .role(Role::Heading)
                            .aria_level(2)
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("Preferences"),
                    )
                    .child(
                        crate::controls::compact_icon_button("remote-apps-preferences-close")
                            .icon(IconName::X)
                            .tooltip("Close")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.preferences_open = false;
                                cx.notify();
                            })),
                    ),
            )
            .child(section(
                "Sound",
                "Hear remote apps on this Mac.",
                div()
                    .flex()
                    .child(
                        div().debug_selector(|| "remote-apps-audio".into()).child(
                            crate::controls::switch("remote-apps-audio")
                                .checked(current.audio)
                                .label("Play sound")
                                .on_click(cx.listener(|this, value: &bool, _, cx| {
                                    let settings = api::StreamingSettings {
                                        audio: *value,
                                        ..this.settings()
                                    };
                                    this.save(settings, cx)
                                })),
                        ),
                    )
                    .into_any_element(),
            ))
            .child(section("Quality", quality_note, quality.into_any_element()))
            .child(section(
                "Frame rate",
                "60 fps is smoother. 30 fps uses less bandwidth and battery.",
                rate.into_any_element(),
            ))
            .child(section(
                "Statistics",
                "Show frame rate, delay and bandwidth over remote windows.",
                crate::controls::switch("remote-apps-hud")
                    .checked(current.hud)
                    .label("Show statistics")
                    .on_click(cx.listener(|this, value: &bool, _, cx| {
                        let settings = api::StreamingSettings {
                            hud: *value,
                            ..this.settings()
                        };
                        this.save(settings, cx)
                    }))
                    .into_any_element(),
            ))
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(muted)
                    .child("Changes apply to apps you open next."),
            )
    }

    fn tile(
        &mut self,
        index: usize,
        app: api::RemoteApp,
        running: Vec<api::SessionId>,
        starting: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let icon = self.icon(&app);
        let id = app.id.clone();
        let menu_owner = cx.entity().downgrade();
        let menu_app = app.clone();
        // Only apps with something to offer get a menu.
        let has_menu =
            app.id.starts_with("command:") || !app.actions.is_empty() || !running.is_empty();
        let tile = div()
            .id(("remote-app", index))
            .debug_selector(move || format!("remote-app-{index}"))
            .role(Role::Button)
            .aria_label(app.name.clone())
            .w(px(TILE_WIDTH))
            .h(px(TILE_HEIGHT))
            .flex_shrink_0()
            .flex()
            .flex_col()
            .items_center()
            .pt(px(12.))
            .px(px(6.))
            .gap(px(8.))
            .cursor_pointer()
            .hover(|style| style.bg(theme::row_hover(cx)))
            .active(|style| style.bg(cx.theme().muted))
            // While opening, the icon dims under a spinner until windows show.
            .child(
                div()
                    .relative()
                    .size(px(ICON))
                    .flex_shrink_0()
                    .child(
                        div()
                            .size_full()
                            .when(starting, |icon| icon.opacity(0.35))
                            .child(match icon {
                                Some(icon) => img(icon).size(px(ICON)).into_any_element(),
                                None => monogram(&app.name, cx),
                            }),
                    )
                    .when(starting, |icon| {
                        icon.child(
                            div()
                                .debug_selector(move || format!("remote-app-starting-{index}"))
                                .absolute()
                                .inset_0()
                                .flex()
                                .items_center()
                                .justify_center()
                                .child(
                                    gpui_kit::component::spinner::Spinner::new().with_size(px(24.)),
                                ),
                        )
                    }),
            )
            .child(
                div()
                    .w_full()
                    .h(px(32.))
                    .overflow_hidden()
                    .text_size(px(13.))
                    .line_height(px(16.))
                    .text_center()
                    .text_ellipsis()
                    .line_clamp(2)
                    .when(starting, |label| {
                        label.text_color(cx.theme().muted_foreground)
                    })
                    .child(if starting {
                        "Opening…".to_owned()
                    } else {
                        app.name.clone()
                    }),
            )
            .on_click(cx.listener(move |this, _, _, cx| this.launch(&id, None, cx)));
        if !has_menu {
            return tile.into_any_element();
        }
        tile.context_menu(move |mut menu, _, _| {
            // Groups are separated; the first needs no separator above it.
            let mut grouped = false;
            if let Some(id) = menu_app.id.strip_prefix("command:") {
                grouped = true;
                let (edit, remove) = (
                    (menu_owner.clone(), id.to_owned()),
                    (menu_owner.clone(), id.to_owned()),
                );
                menu = menu
                    .item(PopupMenuItem::new("Edit…").on_click(move |_, window, cx| {
                        let _ = edit.0.update(cx, |this, cx| {
                            let saved = this.saved().into_iter().find(|saved| saved.id == edit.1);
                            if saved.is_some() {
                                this.open_runner(saved, window, cx);
                            }
                        });
                    }))
                    .item(PopupMenuItem::new("Remove").on_click(move |_, _, cx| {
                        let _ = remove
                            .0
                            .update(cx, |this, cx| this.remove_saved(&remove.1, cx));
                    }));
            }
            if !menu_app.actions.is_empty() {
                if grouped {
                    menu = menu.separator();
                }
                grouped = true;
                for action in &menu_app.actions {
                    let (owner, app, action_id) =
                        (menu_owner.clone(), menu_app.id.clone(), action.id.clone());
                    menu = menu.item(PopupMenuItem::new(action.name.clone()).on_click(
                        move |_, _, cx| {
                            let _ = owner.update(cx, |this, cx| {
                                this.launch(&app, Some(action_id.clone()), cx)
                            });
                        },
                    ));
                }
            }
            if !running.is_empty() {
                let running = running.clone();
                if grouped {
                    menu = menu.separator();
                }
                menu = menu.item(PopupMenuItem::new("Quit").on_click(move |_, _, cx| {
                    if let Some(engine) = sessions(cx) {
                        for session in &running {
                            engine.stop(*session);
                        }
                    }
                }));
            }
            menu
        })
        .into_any_element()
    }
}

/// Fallback icon: the first letter on a colour derived from the name.
fn monogram(name: &str, cx: &App) -> AnyElement {
    let hash = name.bytes().fold(2166136261u32, |h, b| {
        (h ^ u32::from(b)).wrapping_mul(16777619)
    });
    let hue = (hash % 360) as f32 / 360.;
    let dark = cx.theme().is_dark();
    // Initials of the first two words: "Visual Studio Code" is "VS".
    let letters: String = name
        .split_whitespace()
        .filter_map(|word| word.chars().find(|c| c.is_alphanumeric()))
        .take(2)
        .flat_map(char::to_uppercase)
        .collect();
    let lightness = if dark { 0.36 } else { 0.52 };
    div()
        .size(px(ICON))
        .flex_shrink_0()
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(14.))
        .bg(linear_gradient(
            180.,
            linear_color_stop(hsla(hue, 0.5, lightness + 0.08, 1.), 0.),
            linear_color_stop(hsla(hue, 0.5, lightness - 0.04, 1.), 1.),
        ))
        .text_color(white())
        .text_size(px(if letters.chars().count() > 1 {
            22.
        } else {
            26.
        }))
        .font_weight(FontWeight::SEMIBOLD)
        .child(if letters.is_empty() {
            "?".into()
        } else {
            letters
        })
        .into_any_element()
}

/// Placeholder tiles while the server is checked or apps load.
fn skeleton(cx: &App) -> Div {
    let mut grid = div().flex().flex_wrap().gap_x(px(4.)).gap_y(px(8.));
    for _ in 0..12 {
        grid = grid.child(
            div()
                .w(px(TILE_WIDTH))
                .h(px(TILE_HEIGHT))
                .flex()
                .flex_col()
                .items_center()
                .pt(px(12.))
                .gap(px(10.))
                .child(div().size(px(ICON)).rounded(px(14.)).bg(cx.theme().muted))
                .child(div().w(px(72.)).h(px(10.)).bg(cx.theme().muted)),
        );
    }
    grid
}

impl EventEmitter<LaunchpadEvent> for Launchpad {}

impl Render for Launchpad {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(engine) = sessions(cx) else {
            return div().into_any_element();
        };
        let status = self.status();
        let sessions: Vec<_> = engine
            .list()
            .into_iter()
            .filter(|s| s.server == self.server)
            .collect();
        let mut root = div()
            .id("remote-apps")
            .size_full()
            .min_h_0()
            .flex()
            .flex_col();
        // Alerts sit above everything, including the setup screens.
        for (index, (alert, dismissible)) in self.alerts(&status, &sessions).into_iter().enumerate()
        {
            root = root.child(self.alert(index, alert, dismissible, cx));
        }
        if let Some(hero) = self.status_hero(&status, cx) {
            return self.with_terminal(root.child(hero), cx);
        }
        let catalogue = self.catalogue();
        let total = match &catalogue {
            api::Catalogue::Ready(apps) => apps.len(),
            _ => 0,
        };
        root = root.child(self.toolbar(total, cx));
        let mut pages = 1;
        let content: AnyElement = match catalogue {
            api::Catalogue::Loading => skeleton(cx).into_any_element(),
            api::Catalogue::Failed(error) => self
                .hero(
                    IconName::TriangleAlert,
                    "Couldn’t list applications",
                    error,
                    cx,
                )
                .into_any_element(),
            api::Catalogue::Ready(apps) if apps.is_empty() => self
                .hero(
                    IconName::LayoutGrid,
                    "No apps found",
                    "Install graphical applications on this server, then refresh.",
                    cx,
                )
                .into_any_element(),
            api::Catalogue::Ready(_) => {
                let matches = self.matches(cx);
                if matches.is_empty() {
                    let query = self.search.read(cx).value().trim().to_owned();
                    empty::compact(format!("No apps match “{query}”."), cx).into_any_element()
                } else {
                    let (columns, rows) = self.layout();
                    let per_page = columns * rows;
                    pages = matches.len().div_ceil(per_page);
                    self.page = self.page.min(pages - 1);
                    let start = self.page * per_page;
                    let width = columns as f32 * (TILE_WIDTH + GAP_X) - GAP_X;
                    let mut grid = div()
                        .w(px(width))
                        .mx_auto()
                        .flex()
                        .flex_wrap()
                        .content_start()
                        .gap_x(px(GAP_X))
                        .gap_y(px(GAP_Y));
                    for (index, app) in matches.into_iter().enumerate().skip(start).take(per_page) {
                        let running = sessions
                            .iter()
                            .filter(|s| s.app_id == app.id && s.state.is_active())
                            .map(|s| s.id)
                            .collect();
                        let starting = sessions
                            .iter()
                            .any(|s| s.app_id == app.id && s.state == api::SessionState::Starting);
                        grid = grid.child(self.tile(index, app, running, starting, cx));
                    }
                    div()
                        .size_full()
                        .flex()
                        .flex_col()
                        .child(div().flex_1().min_h_0().child(grid))
                        .when(pages > 1, |area| area.child(self.pager(pages, cx)))
                        .into_any_element()
                }
            }
        };
        let owner = cx.entity().downgrade();
        let grid = div()
            .id("remote-apps-grid")
            .debug_selector(|| "remote-apps-grid".into())
            .flex_1()
            .min_w_0()
            .h_full()
            .relative()
            .overflow_hidden()
            .p(px(crate::controls::PAGE_PADDING))
            .pb(px(8.))
            .on_scroll_wheel(cx.listener(move |this, event: &ScrollWheelEvent, _, cx| {
                this.scrolled(event, pages, cx)
            }))
            .child(
                canvas(
                    move |bounds, window, cx| {
                        let padding = px(crate::controls::PAGE_PADDING);
                        let area = size(
                            bounds.size.width - padding * 2.,
                            bounds.size.height - padding - px(8.),
                        );
                        let _ = owner.update(cx, |this, cx| {
                            if this.area != area {
                                this.area = area;
                                window.refresh();
                                cx.notify();
                            }
                        });
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .size_full()
                .left_0()
                .top_0(),
            )
            .child(content);
        let runner = self.runner.take();
        let panel = match &runner {
            Some(runner) => Some(self.runner_panel(runner, cx)),
            None => self.preferences_open.then(|| self.preferences(cx)),
        };
        self.runner = runner;
        root = root.child(div().flex_1().min_h_0().flex().child(grid).children(panel));
        self.with_terminal(root, cx)
    }
}

impl Newport {
    pub(crate) fn remote_apps(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if !available(cx) {
            // A restored page from a full build.
            self.page = Page::Overview;
            return self.overview(cx);
        }
        let Some(server) = self.selected_server() else {
            return empty::pane(
                IconName::Server,
                "Choose a server",
                "Select a server to see its applications.",
                cx,
            )
            .into_any_element();
        };
        let name = text(server, "name");
        let id = self.server.clone();
        let view = match &self.remote_apps {
            Some(view) => view.clone(),
            None => {
                let view = cx.new(|cx| Launchpad::new(window, cx));
                cx.subscribe(&view, |this, _, event, cx| match event {
                    LaunchpadEvent::Recheck { replace } => {
                        this.remote_apps_recheck = Some(*replace);
                        this.poll_remote_apps(cx);
                    }
                    LaunchpadEvent::Refresh => {
                        this.remote_apps_refresh = true;
                        this.poll_remote_apps(cx);
                    }
                    LaunchpadEvent::SaveSettings(settings) => {
                        this.queue_remote_mutation(Mutation {
                            key: SETTINGS_KEY.to_owned(),
                            method: "remote_apps_settings_save",
                            args: json!({"settings": settings}),
                        })
                    }
                    LaunchpadEvent::SaveCommand { server, command } => {
                        this.queue_remote_mutation(Mutation {
                            key: commands_key(server),
                            method: "remote_apps_command_save",
                            args: json!({"command": command}),
                        })
                    }
                    LaunchpadEvent::RemoveCommand { server, id } => {
                        this.queue_remote_mutation(Mutation {
                            key: commands_key(server),
                            method: "remote_apps_command_remove",
                            args: json!({"serverId": server, "id": id}),
                        })
                    }
                })
                .detach();
                self.remote_apps = Some(view.clone());
                view
            }
        };
        if std::mem::take(&mut self.remote_apps_away) {
            self.remote_apps_visit += 1;
            self.remote_apps_returned = true;
        }
        let visit = self.remote_apps_visit;
        let resources = &self.resources;
        view.update(cx, |view, cx| {
            view.show(&id, &name, visit, window, cx);
            view.observe(resources);
        });
        view.into_any_element()
    }

    /// Ask the backend for the selected server's Remote apps queries while
    /// the page is visible. Replies arrive through the shared resources.
    /// Send a preference change. Changes to one key go one at a time, in
    /// order, and a read in flight for that key gives way to them.
    fn queue_remote_mutation(&mut self, mutation: Mutation) {
        // Settings are saved whole; only the newest unsent change matters.
        if mutation.method == "remote_apps_settings_save" {
            self.remote_apps_mutations
                .retain(|queued| queued.method != mutation.method);
        }
        self.remote_apps_mutations.push_back(mutation);
        self.send_remote_mutations();
    }

    pub(crate) fn send_remote_mutations(&mut self) {
        let resources = &self.resources;
        self.remote_apps_mutating
            .retain(|key| resources.get(key).is_some_and(|read| read.pending));
        let mut waiting = std::collections::VecDeque::new();
        while let Some(mutation) = self.remote_apps_mutations.pop_front() {
            if self.remote_apps_mutating.contains(&mutation.key)
                || waiting
                    .iter()
                    .any(|queued: &Mutation| queued.key == mutation.key)
            {
                waiting.push_back(mutation);
                continue;
            }
            if self
                .resources
                .get(&mutation.key)
                .is_some_and(|read| read.pending)
            {
                // A read can be repeated; the change can't be dropped.
                self.resources.fence(&mutation.key);
            }
            let generation = self.resources.begin_refresh(&mutation.key, true);
            self.dispatch_request(&mutation.key, mutation.method, mutation.args, generation);
            self.remote_apps_mutating.insert(mutation.key);
        }
        self.remote_apps_mutations = waiting;
    }

    pub(crate) fn poll_remote_apps(&mut self, cx: &mut Context<Self>) {
        self.send_remote_mutations();
        if self.page != Page::RemoteApps {
            // Keep the last readings, so returning draws them at once.
            self.remote_apps_away |= self.remote_apps.is_some();
            return;
        }
        if !self.window_visible || self.server.is_empty() || !available(cx) {
            return;
        }
        // A refresh or recheck asked for while the page wasn't shown waits
        // until it is.
        let manual = std::mem::take(&mut self.remote_apps_refresh);
        if let Some(replace) = self.remote_apps_recheck.take() {
            let key = status_key(&self.server);
            if replace || !self.resources.get(&key).is_some_and(|read| read.pending) {
                let server = self.server.clone();
                self.poll_remote_query(
                    &key,
                    "remote_apps_status",
                    &server,
                    STATUS_POLL,
                    true,
                    false,
                );
            }
        }
        // A return reads everything again; the view reset itself when drawn.
        let returned = std::mem::take(&mut self.remote_apps_returned);
        let server = self.server.clone();
        if (returned || self.resources.get(SETTINGS_KEY).is_none())
            && !self.remote_apps_mutating.contains(SETTINGS_KEY)
        {
            let generation = self.resources.begin_refresh(SETTINGS_KEY, false);
            self.dispatch_request(SETTINGS_KEY, "remote_apps_settings", json!({}), generation);
        }
        if (returned || self.resources.get(INSTALLED_KEY).is_none())
            && !self.remote_apps_mutating.contains(INSTALLED_KEY)
        {
            let generation = self.resources.begin_refresh(INSTALLED_KEY, false);
            self.dispatch_request(
                INSTALLED_KEY,
                "remote_apps_installed",
                json!({}),
                generation,
            );
        }
        let commands = commands_key(&server);
        if (returned || self.resources.get(&commands).is_none())
            && !self.remote_apps_mutating.contains(&commands)
        {
            let generation = self.resources.begin_refresh(&commands, false);
            self.dispatch_request(
                &commands,
                "remote_apps_commands",
                json!({"serverId": server}),
                generation,
            );
        }
        self.poll_remote_query(
            &status_key(&server),
            "remote_apps_status",
            &server,
            STATUS_POLL,
            manual,
            returned,
        );
        // The status's agent once a reading arrived, and whether it settled.
        let status = self
            .resources
            .get(&status_key(&server))
            .filter(|status| status.updated_at.is_some())
            .map(|status| {
                let settled = !status.pending && status.error.is_none();
                (status.value["agent"].is_string(), settled, status.revision)
            });
        let record = self
            .resources
            .get(INSTALLED_KEY)
            .filter(|record| record.updated_at.is_some())
            .map(|record| known_installed(&record.value, &server));
        let known = record == Some(true);
        // Remember what this check found, for the next first visit.
        // Once per reading: a save that failed waits for the next one.
        if let (Some((installed, true, revision)), Some(known)) = (status, record)
            && installed != known
            && !self.remote_apps_mutating.contains(INSTALLED_KEY)
            && self.remote_apps_record_tried != Some((server.clone(), revision))
        {
            self.remote_apps_record_tried = Some((server.clone(), revision));
            self.queue_remote_mutation(Mutation {
                key: INSTALLED_KEY.to_owned(),
                method: "remote_apps_installed_save",
                args: json!({"serverId": server, "installed": installed}),
            });
        }
        // The catalogue is read beside the status when the server is known
        // to have the agent, rather than after the status answers.
        let installed = match status {
            Some((installed, ..)) => installed,
            None => known,
        };
        if installed {
            self.poll_remote_query(
                &catalogue_key(&server),
                "remote_apps_catalogue",
                &server,
                CATALOGUE_POLL,
                manual,
                returned,
            );
        }
    }

    fn poll_remote_query(
        &mut self,
        key: &str,
        method: &str,
        server: &str,
        every: Duration,
        manual: bool,
        returned: bool,
    ) {
        // A return asks again, but the backend answers from its cache while fresh.
        let due = manual
            || returned
            || self.resources.get(key).is_none_or(|read| {
                !read.pending && read.finished_at.is_none_or(|at| at.elapsed() >= every)
            });
        if !due {
            return;
        }
        if manual && self.resources.get(key).is_some_and(|read| read.pending) {
            // A background read in flight would answer from the backend's
            // cache; Refresh replaces it with one that bypasses it.
            self.resources.fence(key);
        }
        let generation = self.resources.begin_refresh(key, manual);
        self.dispatch_request(
            key,
            method,
            json!({"id": server, "refresh": manual}),
            generation,
        );
    }
}

#[cfg(test)]
mod tests {
    use crate::page_perf_tests::{PageHost, frame, owner};
    use crate::remote_apps_api as api;
    use gpui_kit::{self as gpui, AppContext, Modifiers, TestAppContext, VisualTestContext};
    use serde_json::{Value, json};

    const AGENT: &str = "/home/u/.cache/newport-remote/releases/system/a1b2/current/run-agent";

    /// A server with the agent installed, as the backend's status query reports it.
    fn ready() -> Value {
        json!({"agent": AGENT, "osRelease": "ID=ubuntu\nPRETTY_NAME=\"Ubuntu 24.04 LTS\""})
    }

    /// Eleven applications; sorted by name, Files comes first.
    fn catalogue() -> Value {
        let apps = [
            ("google-chrome.desktop", "Google Chrome"),
            ("firefox.desktop", "Firefox"),
            ("code.desktop", "Visual Studio Code"),
            ("org.gnome.TextEditor.desktop", "Text Editor"),
            ("org.kde.kate.desktop", "Kate"),
            ("org.gnome.Terminal.desktop", "Terminal"),
            ("org.gnome.Nautilus.desktop", "Files"),
            ("gimp.desktop", "GNU Image Manipulation Program"),
            ("org.inkscape.Inkscape.desktop", "Inkscape"),
            ("libreoffice-writer.desktop", "LibreOffice Writer"),
            ("org.gnome.Settings.desktop", "Settings"),
        ];
        json!({"apps": apps.iter().map(|(id, name)| json!({
            "id": id, "name": name, "description": "", "keywords": [], "icon": null,
            "actions": if *id == "google-chrome.desktop" {
                json!([{"id": "new-private-window", "name": "New Incognito Window"}])
            } else {
                json!([])
            },
        })).collect::<Vec<_>>()})
    }

    /// Complete the page's server queries, as backend replies would.
    fn feed(
        view: &gpui::Entity<crate::Newport>,
        visual: &mut VisualTestContext,
        status: Value,
        catalogue: Option<Value>,
    ) {
        view.update(visual, |v, _| {
            let server = v.server.clone();
            let reads = [
                (super::status_key(&server), Some(status)),
                (super::catalogue_key(&server), catalogue),
            ];
            for (key, value) in reads {
                let Some(value) = value else { continue };
                let generation = v.resources.begin_refresh(&key, false).unwrap();
                v.resources.complete(&key, generation, Ok(value));
            }
        });
        frame(visual);
    }

    /// A first-install activation that started `seconds` ago.
    fn activation(seconds: u64) -> super::Activation {
        super::Activation {
            started: std::time::Instant::now() - std::time::Duration::from_secs(seconds),
            fresh: true,
        }
    }

    /// Answer the status read the diagnosis asked for, once the page has
    /// drawn it in flight.
    fn answer(view: &gpui::Entity<crate::Newport>, visual: &mut VisualTestContext, status: Value) {
        frame(visual);
        view.update(visual, |v, _| {
            let key = super::status_key(&v.server);
            v.resources.fence(&key);
            let generation = v.resources.begin_refresh(&key, true).unwrap();
            v.resources.complete(&key, generation, Ok(status));
        });
        frame(visual);
    }

    /// Let the diagnosis reveal every row.
    fn reveal(visual: &mut VisualTestContext) {
        visual
            .executor()
            .advance_clock(super::CHECK_STEP * (super::CHECKS.len() as u32 + 1));
        visual.run_until_parked();
        frame(visual);
    }

    /// The Remote apps page with scripted sessions, at an optional size.
    fn page(
        cx: &mut TestAppContext,
        size: Option<(f32, f32)>,
    ) -> (gpui::Entity<crate::Newport>, &mut VisualTestContext) {
        cx.update(|cx| {
            gpui::component::init(cx);
            crate::theme::install(false, cx);
            super::start_scripted(cx);
        });
        let mut view = None;
        // Inside a Root, as the app's windows are; the terminal needs it.
        let (_, visual) = cx.add_window_view(|window, cx| {
            let entity = cx.new(|cx| owner(window, cx));
            view = Some(entity.clone());
            gpui::base::Root::new(cx.new(|_| PageHost(entity)), window, cx)
        });
        if let Some((width, height)) = size {
            visual.simulate_resize(gpui::size(gpui::px(width), gpui::px(height)));
        }
        let view = view.unwrap();
        view.update(visual, |v, _| v.page = crate::Page::RemoteApps);
        frame(visual);
        (view, visual)
    }

    fn sessions(visual: &mut VisualTestContext) -> Vec<super::api::SessionSummary> {
        visual.update(|_, cx| super::sessions(cx).unwrap().list())
    }

    fn click(visual: &mut VisualTestContext, selector: &'static str) {
        frame(visual);
        let bounds = visual
            .debug_bounds(selector)
            .unwrap_or_else(|| panic!("{selector} is not on screen"));
        visual.simulate_click(bounds.center(), Modifiers::none());
        frame(visual);
    }

    #[gpui::test]
    fn install_open_and_quit(cx: &mut TestAppContext) {
        let (view, visual) = page(cx, None);
        visual.update(|_, cx| {
            assert!(super::available(cx));
            let pages: Vec<_> = crate::navigation::views(cx).iter().map(|v| v.0).collect();
            let at = pages
                .iter()
                .position(|p| *p == crate::Page::RemoteApps)
                .unwrap();
            assert!(pages[at + 1] == crate::Page::Integration);
        });
        assert!(visual.debug_bounds("remote-app-0").is_none());
        // No agent yet: Get started checks the server first.
        feed(&view, visual, json!({"agent": null, "osRelease": ""}), None);
        click(visual, "remote-apps-get-started");
        assert!(visual.debug_bounds("remote-apps-diagnosis").is_some());
        feed(&view, visual, ready(), Some(catalogue()));
        assert!(visual.debug_bounds("remote-apps-install").is_none());
        click(visual, "remote-app-0");
        let opened = sessions(visual);
        assert_eq!(opened.len(), 1);
        assert_eq!(opened[0].app_name, "Files");
        visual.update(|_, cx| super::sessions(cx).unwrap().stop(opened[0].id));
        assert!(sessions(visual).is_empty());
    }

    #[gpui::test]
    fn get_started_diagnoses_then_activates(cx: &mut TestAppContext) {
        let (view, visual) = page(cx, None);
        let linux = |missing: Value| {
            json!({"agent": null, "system": "Linux", "architecture": "aarch64",
                "missing": missing, "osRelease": "ID=debian"})
        };
        let dbus = json!({"id": "dbus", "name": "D-Bus", "purpose": "Applications",
            "required": true, "neededToInstall": true, "packages": {"apt": "dbus"}});
        let xvfb = json!({"id": "xvfb", "name": "Xvfb", "purpose": "Applications",
            "required": true, "packages": {"apt": "xvfb"}});
        // The welcome screen: the wall and one button, nothing else.
        feed(
            &view,
            visual,
            linux(json!([dbus.clone(), xvfb.clone()])),
            None,
        );
        assert!(visual.debug_bounds("remote-apps-wall").is_some());
        assert!(visual.debug_bounds("remote-apps-get-started").is_some());
        assert!(visual.debug_bounds("remote-apps-alert-0").is_none());
        // Get started checks the server afresh, then reveals each row.
        click(visual, "remote-apps-get-started");
        assert!(visual.debug_bounds("remote-apps-diagnosis").is_some());
        assert!(visual.debug_bounds("remote-apps-check-again").is_none());
        answer(&view, visual, linux(json!([dbus.clone(), xvfb.clone()])));
        reveal(visual);
        // D-Bus blocks activation; Xvfb is a gap. Both offer an install.
        assert!(visual.debug_bounds("remote-apps-check-fix-2").is_some());
        assert!(visual.debug_bounds("remote-apps-check-fix-3").is_some());
        assert!(visual.debug_bounds("remote-apps-activate").is_none());
        assert!(visual.debug_bounds("remote-apps-check-again").is_some());
        // With D-Bus installed, gaps don't hold up activation.
        click(visual, "remote-apps-check-again");
        answer(&view, visual, linux(json!([xvfb.clone()])));
        reveal(visual);
        assert!(visual.debug_bounds("remote-apps-activate").is_some());
        assert!(visual.debug_bounds("remote-apps-check-fix-2").is_none());
        // Activation lights the wall as it goes, then settles until the
        // server query reports the new agent.
        view.update(visual, |v, cx| {
            v.remote_apps.as_ref().unwrap().update(cx, |launchpad, _| {
                let server = launchpad.server.clone();
                launchpad.installs.insert(
                    server,
                    super::Install::Running(activation(0), super::install::Progress::at(100, 50)),
                );
            });
        });
        frame(visual);
        assert!(visual.debug_bounds("remote-apps-wall").is_some());
        assert!(
            visual
                .debug_bounds("remote-apps-install-progress")
                .is_some()
        );
        view.update(visual, |v, cx| {
            v.remote_apps.as_ref().unwrap().update(cx, |launchpad, _| {
                let server = launchpad.server.clone();
                launchpad
                    .installs
                    .insert(server, super::Install::Finished(activation(5), 0));
            });
        });
        frame(visual);
        assert!(visual.debug_bounds("remote-apps-activating").is_some());
        assert!(
            visual
                .debug_bounds("remote-apps-install-progress")
                .is_none()
        );
        feed(
            &view,
            visual,
            json!({"agent": "/c/releases/system/new/current/run-agent", "revision": "new",
                "system": "Linux", "architecture": "aarch64", "osRelease": "ID=debian"}),
            Some(catalogue()),
        );
        view.update(visual, |v, cx| {
            assert!(v.remote_apps.as_ref().unwrap().read(cx).installs.is_empty());
        });
        assert!(visual.debug_bounds("remote-apps-wall").is_none());
        assert!(visual.debug_bounds("remote-app-0").is_some());
    }

    #[gpui::test]
    fn a_fast_activation_still_shows_for_two_seconds(cx: &mut TestAppContext) {
        let (view, visual) = page(cx, None);
        feed(
            &view,
            visual,
            json!({"agent": null, "system": "Linux", "architecture": "aarch64",
                "osRelease": "ID=debian"}),
            None,
        );
        // The install already finished and the server reports the agent, but
        // the activation began only a moment ago.
        view.update(visual, |v, cx| {
            v.remote_apps.as_ref().unwrap().update(cx, |launchpad, _| {
                let server = launchpad.server.clone();
                launchpad
                    .installs
                    .insert(server, super::Install::Finished(activation(1), 0));
            });
        });
        let installed = json!({"agent": "/c/releases/system/new/current/run-agent",
            "revision": "new", "system": "Linux", "architecture": "aarch64",
            "osRelease": "ID=debian"});
        feed(&view, visual, installed.clone(), Some(catalogue()));
        assert!(visual.debug_bounds("remote-apps-wall").is_some());
        assert!(
            visual
                .debug_bounds("remote-apps-install-progress")
                .is_some()
        );
        assert!(visual.debug_bounds("remote-app-0").is_none());
        // Once the minimum time has passed, the apps take over.
        view.update(visual, |v, cx| {
            v.remote_apps.as_ref().unwrap().update(cx, |launchpad, _| {
                let server = launchpad.server.clone();
                launchpad
                    .installs
                    .insert(server, super::Install::Finished(activation(3), 0));
            });
        });
        feed(&view, visual, installed, Some(catalogue()));
        assert!(visual.debug_bounds("remote-apps-wall").is_none());
        assert!(visual.debug_bounds("remote-app-0").is_some());
    }

    #[gpui::test]
    fn install_opens_a_shell_on_the_page_and_the_checklist_follows_it(cx: &mut TestAppContext) {
        let (view, visual) = page(cx, None);
        let xvfb = json!({"id": "xvfb", "name": "Xvfb", "purpose": "Applications",
            "required": true, "packages": {"apt": "xvfb"}});
        let missing = json!({"agent": null, "system": "Linux", "architecture": "aarch64",
            "missing": [xvfb], "osRelease": "ID=debian"});
        let present = json!({"agent": null, "system": "Linux", "architecture": "aarch64",
            "missing": [], "osRelease": "ID=debian"});
        feed(&view, visual, missing.clone(), None);
        click(visual, "remote-apps-get-started");
        answer(&view, visual, missing.clone());
        reveal(visual);
        assert!(visual.debug_bounds("remote-apps-terminal").is_none());
        // Install opens the shell over the page, with the command typed in.
        click(visual, "remote-apps-check-fix-3");
        assert!(visual.debug_bounds("remote-apps-terminal").is_some());
        assert!(visual.debug_bounds("remote-apps-diagnosis").is_some());
        view.update(visual, |v, cx| {
            let launchpad = v.remote_apps.as_ref().unwrap().read(cx);
            let terminal = launchpad.terminal.as_ref().unwrap();
            assert_eq!(terminal.title, "Install Xvfb");
            assert_eq!(terminal.command, "sudo apt update && sudo apt install xvfb");
        });
        let shown = |view: &gpui::Entity<crate::Newport>, visual: &mut VisualTestContext| {
            view.update(visual, |v, cx| {
                let launchpad = v.remote_apps.as_ref().unwrap().read(cx);
                match launchpad.installs.get(&launchpad.server) {
                    Some(super::Install::Diagnosing(diagnosis)) => diagnosis.shown,
                    _ => usize::MAX,
                }
            })
        };
        // When the status was last read, to see a new read happen.
        let read_at = |view: &gpui::Entity<crate::Newport>, visual: &mut VisualTestContext| {
            view.update(visual, |v, _| {
                v.resources
                    .get(&super::status_key(&v.server))
                    .and_then(|read| read.finished_at)
            })
        };
        // While the shell is open, the server is checked again every few
        // seconds; the rows stay in place, even if a check fails.
        let before = read_at(&view, visual);
        visual
            .executor()
            .advance_clock(super::TERMINAL_RECHECK + std::time::Duration::from_millis(10));
        visual.run_until_parked();
        frame(visual);
        assert_ne!(read_at(&view, visual), before);
        assert_eq!(shown(&view, visual), super::CHECKS.len());
        assert!(visual.debug_bounds("remote-apps-check-4").is_some());
        // Once Xvfb is there, its row turns green without replaying the list.
        answer(&view, visual, present.clone());
        assert!(visual.debug_bounds("remote-apps-check-fix-3").is_none());
        assert_eq!(shown(&view, visual), super::CHECKS.len());
        // Closing the shell checks once more, quietly.
        let before = read_at(&view, visual);
        click(visual, "remote-apps-terminal-close");
        assert!(visual.debug_bounds("remote-apps-terminal").is_none());
        assert_ne!(read_at(&view, visual), before);
        assert_eq!(shown(&view, visual), super::CHECKS.len());
        answer(&view, visual, present);
        // A shell still installing stays open while another page is shown.
        answer(&view, visual, missing);
        click(visual, "remote-apps-check-fix-3");
        view.update(visual, |v, cx| {
            v.navigate(crate::Page::Overview, cx);
            v.poll_remote_apps(cx);
        });
        frame(visual);
        view.update(visual, |v, cx| v.navigate(crate::Page::RemoteApps, cx));
        frame(visual);
        assert!(visual.debug_bounds("remote-apps-terminal").is_some());
    }

    #[gpui::test]
    fn check_again_keeps_the_checklist_in_place(cx: &mut TestAppContext) {
        let (view, visual) = page(cx, None);
        let dbus = json!({"id": "dbus", "name": "D-Bus", "purpose": "Applications",
            "required": true, "neededToInstall": true, "packages": {"apt": "dbus"}});
        let status = json!({"agent": null, "system": "Linux", "architecture": "aarch64",
            "missing": [dbus], "osRelease": "ID=debian"});
        feed(&view, visual, status.clone(), None);
        click(visual, "remote-apps-get-started");
        answer(&view, visual, status);
        reveal(visual);
        click(visual, "remote-apps-check-again");
        view.update(visual, |v, cx| {
            let launchpad = v.remote_apps.as_ref().unwrap().read(cx);
            assert!(matches!(
                launchpad.installs.get(&launchpad.server),
                Some(super::Install::Diagnosing(d)) if d.shown == super::CHECKS.len()
            ));
        });
        // Every row is still drawn while the check runs.
        assert!(visual.debug_bounds("remote-apps-check-4").is_some());
        assert!(visual.debug_bounds("remote-apps-check-fix-2").is_some());
    }

    #[gpui::test]
    fn a_failed_activation_returns_to_get_started(cx: &mut TestAppContext) {
        let (view, visual) = page(cx, None);
        feed(
            &view,
            visual,
            json!({"agent": null, "system": "Linux", "architecture": "x86_64",
                "osRelease": "ID=debian"}),
            None,
        );
        view.update(visual, |v, cx| {
            v.remote_apps.as_ref().unwrap().update(cx, |launchpad, _| {
                let server = launchpad.server.clone();
                launchpad
                    .installs
                    .insert(server, super::Install::Failed("Missing programs".into()));
            });
        });
        frame(visual);
        assert!(visual.debug_bounds("remote-apps-get-started").is_some());
        click(visual, "remote-apps-alert-dismiss-0");
        view.update(visual, |v, cx| {
            assert!(v.remote_apps.as_ref().unwrap().read(cx).installs.is_empty());
        });
    }

    #[gpui::test]
    fn other_systems_and_processors_are_blocked_in_the_diagnosis(cx: &mut TestAppContext) {
        let (view, visual) = page(cx, None);
        for (system, architecture) in [("FreeBSD", "amd64"), ("Linux", "riscv64")] {
            let status = json!({"agent": null, "system": system,
                "architecture": architecture, "osRelease": ""});
            feed(&view, visual, status.clone(), None);
            click(visual, "remote-apps-get-started");
            answer(&view, visual, status);
            reveal(visual);
            assert!(visual.debug_bounds("remote-apps-check-again").is_some());
            assert!(visual.debug_bounds("remote-apps-activate").is_none());
            view.update(visual, |v, cx| {
                v.remote_apps.as_ref().unwrap().update(cx, |launchpad, _| {
                    launchpad.installs.clear();
                });
            });
        }
    }

    #[gpui::test]
    fn an_app_shows_progress_while_it_opens(cx: &mut TestAppContext) {
        let (view, visual) = page(cx, None);
        feed(&view, visual, ready(), Some(catalogue()));
        assert!(visual.debug_bounds("remote-app-starting-0").is_none());
        // Scripted sessions start at once; record one still starting.
        view.update(visual, |v, cx| {
            let launchpad = v.remote_apps.as_ref().unwrap().read(cx);
            let server = launchpad.server.clone();
            super::sessions(cx)
                .unwrap()
                .record_starting(&server, "org.gnome.Nautilus.desktop");
        });
        frame(visual);
        assert!(visual.debug_bounds("remote-app-starting-0").is_some());
        // Clicking again while it opens starts nothing new.
        click(visual, "remote-app-0");
        assert_eq!(sessions(visual).len(), 1);
    }

    #[gpui::test]
    fn apps_that_do_not_fit_are_paged(cx: &mut TestAppContext) {
        let (view, visual) = page(cx, Some((640., 440.)));
        feed(&view, visual, ready(), Some(catalogue()));
        // The first frame measures the grid; the next lays it out.
        frame(visual);
        assert!(visual.debug_bounds("remote-app-0").is_some());
        assert!(visual.debug_bounds("remote-app-10").is_none());
        assert!(visual.debug_bounds("remote-apps-page-1").is_some());
        let last = (0..4)
            .rev()
            .find(|page| {
                visual
                    .debug_bounds(Box::leak(
                        format!("remote-apps-page-{page}").into_boxed_str(),
                    ))
                    .is_some()
            })
            .unwrap();
        click(
            visual,
            Box::leak(format!("remote-apps-page-{last}").into_boxed_str()),
        );
        assert!(visual.debug_bounds("remote-app-0").is_none());
        assert!(visual.debug_bounds("remote-app-10").is_some());
    }

    #[gpui::test]
    fn each_swipe_turns_one_page_and_momentum_is_ignored(cx: &mut TestAppContext) {
        use gpui::{ScrollDelta, ScrollWheelEvent, TouchPhase, point, px};
        let (view, visual) = page(cx, Some((560., 400.)));
        feed(&view, visual, ready(), Some(catalogue()));
        // The first frame measures the grid; the next lays it out.
        frame(visual);
        assert!(visual.debug_bounds("remote-apps-page-1").is_some());
        let at = visual.debug_bounds("remote-apps-grid").unwrap().center();
        let scroll = |visual: &mut VisualTestContext, x: f32, phase: TouchPhase| {
            visual.simulate_event(ScrollWheelEvent {
                position: at,
                delta: ScrollDelta::Pixels(point(px(x), px(0.))),
                touch_phase: phase,
                ..Default::default()
            });
            frame(visual);
        };
        let page = |visual: &mut VisualTestContext| {
            view.update(visual, |v, cx| {
                v.remote_apps.as_ref().unwrap().read(cx).page
            })
        };
        // Swipe forward; momentum continues past the threshold again.
        scroll(visual, -40., TouchPhase::Started);
        scroll(visual, -40., TouchPhase::Moved);
        scroll(visual, -40., TouchPhase::Ended);
        for _ in 0..6 {
            scroll(visual, -30., TouchPhase::Moved);
        }
        assert_eq!(page(visual), 1);
        // Swiping back at once, while momentum was still arriving, turns back.
        scroll(visual, 40., TouchPhase::Started);
        scroll(visual, 40., TouchPhase::Moved);
        assert_eq!(page(visual), 0);
        // After a pause, a mouse wheel notch turns a page.
        std::thread::sleep(super::GESTURE_GAP + std::time::Duration::from_millis(50));
        visual.simulate_event(ScrollWheelEvent {
            position: at,
            delta: ScrollDelta::Lines(point(0., -1.)),
            ..Default::default()
        });
        frame(visual);
        assert_eq!(page(visual), 1);
    }

    /// A ready server whose status report adds these fields.
    fn launchpad_with(
        extra: Value,
        cx: &mut TestAppContext,
    ) -> (gpui::Entity<crate::Newport>, &mut VisualTestContext) {
        let (view, visual) = page(cx, None);
        let mut status = ready();
        if let (Some(status), Some(extra)) = (status.as_object_mut(), extra.as_object()) {
            status.extend(extra.clone());
        }
        feed(&view, visual, status, Some(catalogue()));
        (view, visual)
    }

    #[gpui::test]
    fn returning_draws_the_last_readings_at_once_and_reads_them_again(cx: &mut TestAppContext) {
        let (view, visual) = launchpad_with(json!({}), cx);
        click(visual, "remote-apps-preferences");
        assert!(
            visual
                .debug_bounds("remote-apps-preferences-panel")
                .is_some()
        );
        // Leave and return through the shell, which resets other pages.
        view.update(visual, |v, cx| {
            v.navigate(crate::Page::Overview, cx);
            v.poll_remote_apps(cx);
        });
        frame(visual);
        view.update(visual, |v, cx| v.navigate(crate::Page::RemoteApps, cx));
        frame(visual);
        // No welcome wall, skeleton or loading state: the grid is there in
        // the first frame, and the view itself starts afresh.
        assert!(visual.debug_bounds("remote-apps-wall").is_none());
        assert!(visual.debug_bounds("remote-app-0").is_some());
        assert!(
            visual
                .debug_bounds("remote-apps-preferences-panel")
                .is_none()
        );
        // Every reading is requested again from the backend.
        view.update(visual, |v, cx| {
            v.window_visible = true;
            v.poll_remote_apps(cx);
            for key in [
                super::SETTINGS_KEY.to_owned(),
                super::commands_key(&v.server),
                super::status_key(&v.server),
                super::catalogue_key(&v.server),
            ] {
                let read = v.resources.get(&key);
                assert!(
                    read.is_some_and(|r| r.pending || r.finished_at.is_some()),
                    "{key}"
                );
                // A return lets the backend answer from its cache; only
                // Refresh rescans the server.
                assert!(read.is_some_and(|r| !r.manual_pending), "{key}");
            }
        });
        // A later tick does not ask again before the polling interval.
        view.update(visual, |v, cx| {
            let before = v
                .resources
                .get(&super::status_key(&v.server))
                .map(|r| r.finished_at);
            v.poll_remote_apps(cx);
            let after = v
                .resources
                .get(&super::status_key(&v.server))
                .map(|r| r.finished_at);
            assert_eq!(before, after);
        });
    }

    #[gpui::test]
    fn a_server_known_to_have_the_agent_loads_its_apps_on_a_first_visit(cx: &mut TestAppContext) {
        let (view, visual) = page(cx, None);
        // Only the record of installed servers has arrived: no status yet.
        view.update(visual, |v, _| {
            let server = v.server.clone();
            let generation = v
                .resources
                .begin_refresh(super::INSTALLED_KEY, false)
                .unwrap();
            v.resources
                .complete(super::INSTALLED_KEY, generation, Ok(json!([server])));
        });
        frame(visual);
        assert!(visual.debug_bounds("remote-apps-wall").is_none());
        assert!(visual.debug_bounds("remote-apps-grid").is_some());
        // The catalogue is asked for beside the status, not after it.
        view.update(visual, |v, cx| {
            v.window_visible = true;
            v.poll_remote_apps(cx);
            let server = v.server.clone();
            assert!(v.resources.get(&super::status_key(&server)).is_some());
            assert!(v.resources.get(&super::catalogue_key(&server)).is_some());
        });
    }

    #[gpui::test]
    fn each_check_updates_the_record_of_installed_servers(cx: &mut TestAppContext) {
        let (view, visual) = launchpad_with(json!({}), cx);
        view.update(visual, |v, cx| {
            let generation = v
                .resources
                .begin_refresh(super::INSTALLED_KEY, false)
                .unwrap();
            v.resources
                .complete(super::INSTALLED_KEY, generation, Ok(json!([])));
            v.window_visible = true;
            v.poll_remote_apps(cx);
            // The check found the agent; the record is saved with it.
            assert!(v.remote_apps_mutating.contains(super::INSTALLED_KEY));
            // If the save fails, it isn't sent again until a new reading.
            let generation = v.resources.begin_refresh(super::INSTALLED_KEY, true);
            v.resources.fence(super::INSTALLED_KEY);
            let generation = generation
                .or_else(|| v.resources.begin_refresh(super::INSTALLED_KEY, true))
                .unwrap();
            v.resources
                .complete(super::INSTALLED_KEY, generation, Err("disk full".into()));
            v.poll_remote_apps(cx);
            v.poll_remote_apps(cx);
            assert!(!v.remote_apps_mutating.contains(super::INSTALLED_KEY));
        });
    }

    #[gpui::test]
    fn remote_apps_starting_after_the_window_opens_keeps_the_sidebar_working(
        cx: &mut TestAppContext,
    ) {
        cx.update(|cx| {
            gpui::component::init(cx);
            crate::theme::install(false, cx);
        });
        let mut view = None;
        let (_, visual) = cx.add_window_view(|window, cx| {
            let entity = cx.new(|cx| owner(window, cx));
            view = Some(entity.clone());
            PageHost(entity)
        });
        let view = view.unwrap();
        frame(visual);
        let before = view.update(visual, |v, _| v.navigation_focus.len());
        // Full builds start Remote apps once the window exists.
        visual.update(|_, cx| super::start_scripted(cx));
        frame(visual);
        view.update(visual, |v, cx| {
            assert_eq!(crate::navigation::views(cx).len(), before + 1);
            v.sync_navigation_focus(cx);
            assert_eq!(v.navigation_focus.len(), before + 1);
        });
        frame(visual);
    }

    /// Answer a key as the backend would after a successful save.
    fn confirm(
        view: &gpui::Entity<crate::Newport>,
        visual: &mut VisualTestContext,
        key: &str,
        value: Value,
    ) {
        view.update(visual, |v, _| {
            let generation = v.resources.begin_refresh(key, false).unwrap();
            v.resources.complete(key, generation, Ok(value));
        });
        frame(visual);
    }

    fn package(required: bool) -> Value {
        json!({"missing": [{
            "name": "PipeWire tools",
            "purpose": "Sound from remote apps",
            "required": required,
            "packages": {"apt": "pipewire-bin", "nix": "pipewire"},
        }]})
    }

    #[gpui::test]
    fn missing_software_is_an_alert_with_a_fix(cx: &mut TestAppContext) {
        let mut extra = package(false);
        extra["osRelease"] = json!("ID=nixos\nPRETTY_NAME=\"NixOS 26.05 (Yarara)\"");
        let (_, visual) = launchpad_with(extra, cx);
        assert!(visual.debug_bounds("remote-apps-alert-0").is_some());
        assert!(visual.debug_bounds("remote-apps-alert-run-0").is_some());
        // Optional software only limits some features; the user may dismiss it.
        click(visual, "remote-apps-alert-dismiss-0");
        assert!(visual.debug_bounds("remote-apps-alert-0").is_none());
        assert!(visual.debug_bounds("remote-app-0").is_some());
    }

    #[gpui::test]
    fn a_dismissed_alert_stays_dismissed_after_returning(cx: &mut TestAppContext) {
        let (view, visual) = launchpad_with(package(false), cx);
        click(visual, "remote-apps-alert-dismiss-0");
        view.update(visual, |v, cx| {
            v.page = crate::Page::Overview;
            v.poll_remote_apps(cx);
        });
        frame(visual);
        view.update(visual, |v, _| v.page = crate::Page::RemoteApps);
        frame(visual);
        assert!(visual.debug_bounds("remote-apps-alert-0").is_none());
    }

    #[gpui::test]
    fn a_change_is_sent_even_while_a_read_is_in_flight(cx: &mut TestAppContext) {
        let (view, visual) = launchpad_with(json!({}), cx);
        view.update(visual, |v, _| {
            let key = super::commands_key(&v.server);
            let remove = |id: &str, server: &str| super::Mutation {
                key: super::commands_key(server),
                method: "remote_apps_command_remove",
                args: json!({"serverId": server, "id": id}),
            };
            assert!(v.resources.begin_refresh(&key, false).is_some());
            let server = v.server.clone();
            v.queue_remote_mutation(remove("a", &server));
            // The read gave way and the change went out; the test backend
            // refuses it at once.
            assert!(
                v.resources
                    .get(&key)
                    .is_some_and(|read| read.error.is_some())
            );
            // While a change to the key is in flight, the next one waits.
            let generation = v.resources.begin_refresh(&key, true).unwrap();
            v.remote_apps_mutating.insert(key.clone());
            v.queue_remote_mutation(remove("b", &server));
            assert_eq!(v.remote_apps_mutations.len(), 1);
            v.resources.complete(&key, generation, Ok(json!([])));
            v.send_remote_mutations();
            assert!(v.remote_apps_mutations.is_empty());
        });
    }

    #[gpui::test]
    fn refresh_replaces_a_background_read_in_flight(cx: &mut TestAppContext) {
        let (view, visual) = launchpad_with(json!({}), cx);
        view.update(visual, |v, cx| {
            v.window_visible = true;
            let key = super::status_key(&v.server);
            assert!(v.resources.begin_refresh(&key, false).is_some());
            v.remote_apps_refresh = true;
            v.poll_remote_apps(cx);
            // A new request was made instead of waiting on the cached read.
            assert!(
                v.resources
                    .get(&key)
                    .is_some_and(|read| !read.pending || read.manual_pending)
            );
            assert!(
                v.resources
                    .get(&key)
                    .is_some_and(|read| read.error.is_some())
            );
        });
    }

    #[gpui::test]
    fn without_a_known_os_the_alert_asks_to_check_again(cx: &mut TestAppContext) {
        let mut extra = package(false);
        extra["osRelease"] = json!("ID=gentoo\nPRETTY_NAME=Gentoo");
        let (_, visual) = launchpad_with(extra, cx);
        assert!(visual.debug_bounds("remote-apps-alert-0").is_some());
        assert!(visual.debug_bounds("remote-apps-alert-run-0").is_none());
    }

    #[test]
    fn guidance_names_the_distribution() {
        let nixos = super::api::ServerOs::parse("ID=nixos\nPRETTY_NAME=\"NixOS 26.05\"");
        let text = super::install_guidance("Azure", Some(&nixos), "PipeWire tools", false);
        assert!(text.starts_with("Azure doesn’t have PipeWire tools (NixOS 26.05)."));
        assert!(text.contains("environment.systemPackages"));
        let ubuntu = super::api::ServerOs::parse("ID=ubuntu\nPRETTY_NAME=\"Ubuntu 24.04 LTS\"");
        assert_eq!(
            super::install_guidance("Box", Some(&ubuntu), "Xvfb", true),
            "Remote apps need Xvfb on Box (Ubuntu 24.04 LTS). Install it, then check again."
        );
        assert!(
            super::install_guidance("Box", None, "Xvfb", true).contains("server’s package manager")
        );
    }

    #[gpui::test]
    fn required_software_cannot_be_dismissed(cx: &mut TestAppContext) {
        let (_, visual) = launchpad_with(package(true), cx);
        assert!(visual.debug_bounds("remote-apps-alert-0").is_some());
        assert!(visual.debug_bounds("remote-apps-alert-dismiss-0").is_none());
    }

    #[gpui::test]
    fn preferences_change_what_new_sessions_use(cx: &mut TestAppContext) {
        let (view, visual) = launchpad_with(json!({}), cx);
        assert!(
            visual
                .debug_bounds("remote-apps-preferences-panel")
                .is_none()
        );
        click(visual, "remote-apps-preferences");
        assert!(
            visual
                .debug_bounds("remote-apps-preferences-panel")
                .is_some()
        );
        let audio = |visual: &mut VisualTestContext| {
            view.update(visual, |v, cx| {
                v.remote_apps.as_ref().unwrap().read(cx).settings().audio
            })
        };
        assert!(audio(visual));
        // The test backend refuses every request: the change shows an alert
        // and goes back to what the backend has.
        click(visual, "remote-apps-audio");
        assert!(audio(visual));
        assert!(visual.debug_bounds("remote-apps-alert-0").is_some());
        // A confirmed save shows the saved value.
        let saved = super::encode_settings(api::StreamingSettings {
            audio: false,
            ..Default::default()
        });
        confirm(&view, visual, super::SETTINGS_KEY, saved);
        assert!(!audio(visual));
        click(visual, "remote-apps-preferences");
        assert!(
            visual
                .debug_bounds("remote-apps-preferences-panel")
                .is_none()
        );
    }

    #[test]
    fn commands_split_like_a_shell_without_running_one() {
        let (env, argv) = super::parse_command(
            "GDK_SCALE=2 MOZ_ENABLE_WAYLAND=1 firefox -P 'Clean profile' --new-instance",
        )
        .unwrap();
        assert_eq!(
            env,
            [
                ("GDK_SCALE".into(), "2".into()),
                ("MOZ_ENABLE_WAYLAND".into(), "1".into())
            ]
        );
        assert_eq!(argv, ["firefox", "-P", "Clean profile", "--new-instance"]);
        // Shell syntax is passed through as plain arguments.
        let (_, argv) = super::parse_command("/opt/app/bin/app $HOME '|' x=1").unwrap();
        assert_eq!(argv, ["/opt/app/bin/app", "$HOME", "|", "x=1"]);
        assert!(super::parse_command("app 'unclosed").is_err());
        assert!(super::parse_command("FOO=bar").is_err());
        assert_eq!(super::program_name("/opt/tool/bin/viewer"), "viewer");
    }

    #[gpui::test]
    fn a_command_runs_and_can_be_added_to_the_apps(cx: &mut TestAppContext) {
        let (view, visual) = launchpad_with(json!({}), cx);
        click(visual, "remote-apps-run");
        assert!(visual.debug_bounds("remote-apps-run-panel").is_some());
        visual.simulate_input("/opt/viewer/bin/viewer --safe-mode");
        click(visual, "remote-apps-run-save");
        click(visual, "remote-apps-run-submit");
        assert!(visual.debug_bounds("remote-apps-run-panel").is_none());
        let (sessions, saved) = visual.update(|_, cx| {
            (
                super::sessions(cx).unwrap().list(),
                view.read(cx).remote_apps.as_ref().unwrap().read(cx).saved(),
            )
        });
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].app_name, "viewer");
        // Refused by the test backend: nothing is shown as saved.
        assert!(saved.is_empty());
        // Once the backend confirms, the command is a tile with the session's id.
        let id = sessions[0]
            .app_id
            .strip_prefix("command:")
            .unwrap()
            .to_owned();
        let server = view.read_with(visual, |v, _| v.server.clone());
        let command = super::SavedCommand {
            id: id.clone(),
            name: "viewer".into(),
            command: "/opt/viewer/bin/viewer --safe-mode".into(),
            display: api::DisplayBackend::Auto,
        };
        confirm(
            &view,
            visual,
            &super::commands_key(&server),
            json!([super::encode_command(&server, &command)]),
        );
        let saved = view.update(visual, |v, cx| {
            v.remote_apps.as_ref().unwrap().read(cx).saved()
        });
        assert_eq!(saved.len(), 1);
        assert_eq!(saved[0].id, id);
        // The saved command is now an application tile.
        let tile = view.update(visual, |v, cx| {
            v.remote_apps
                .as_ref()
                .unwrap()
                .read(cx)
                .matches(cx)
                .iter()
                .position(|app| app.name == "viewer")
        });
        assert!(tile.is_some());
    }

    #[gpui::test]
    fn a_bad_command_keeps_the_panel_open_with_the_reason(cx: &mut TestAppContext) {
        let (_, visual) = launchpad_with(json!({}), cx);
        click(visual, "remote-apps-run");
        visual.simulate_input("viewer 'unclosed");
        click(visual, "remote-apps-run-submit");
        assert!(visual.debug_bounds("remote-apps-run-panel").is_some());
        assert!(sessions(visual).is_empty());
    }

    #[test]
    fn the_agent_catalogue_decodes_with_icons_and_actions() {
        let apps = super::decode_catalogue(&json!({"apps": [
            {"id": "google-chrome.desktop", "name": "Google Chrome", "description": "",
             "keywords": [], "icon": "iVBORw0KGgo=",
             "actions": [{"id": "new-private-window", "name": "New Incognito Window"}]},
            {"id": "", "name": "Broken"}
        ]}));
        assert_eq!(apps.len(), 1);
        assert_eq!(apps[0].name, "Google Chrome");
        assert_eq!(apps[0].description, None);
        assert_eq!(apps[0].icon_png.as_deref(), Some(&b"\x89PNG\r\n\x1a\n"[..]));
        assert_eq!(apps[0].actions[0].id, "new-private-window");
    }

    #[test]
    fn a_failed_refresh_keeps_the_last_reading() {
        let mut resources = crate::state::Resources::default();
        let mut read = super::QueryRead::default();
        let generation = resources.begin_refresh("q", false).unwrap();
        resources.complete("q", generation, Ok(json!({"agent": "/a/run-agent"})));
        read.observe(resources.get("q"));
        assert!(read.loaded);
        assert_eq!(read.value["agent"], "/a/run-agent");
        let generation = resources.begin_refresh("q", true).unwrap();
        resources.complete("q", generation, Err("offline".into()));
        read.observe(resources.get("q"));
        assert!(read.loaded);
        assert_eq!(read.value["agent"], "/a/run-agent");
        assert_eq!(read.error.as_deref(), Some("offline"));
    }

    #[gpui::test]
    fn without_streaming_the_page_is_absent(cx: &mut TestAppContext) {
        cx.update(|cx| {
            assert!(!super::available(cx));
            assert!(
                crate::navigation::views(cx)
                    .iter()
                    .all(|(page, _)| *page != crate::Page::RemoteApps)
            );
        });
    }
}
