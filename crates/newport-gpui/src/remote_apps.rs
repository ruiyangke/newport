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
use crate::terminal_status::ExitPolicy;
use gpui_kit::component::{button::ButtonGroup, input::InputEvent};
use std::{
    cell::{Cell, RefCell},
    collections::{HashMap, HashSet},
    rc::Rc,
    sync::Arc,
};

const SOMETHING_WENT_WRONG: &str = "Something went wrong";
const INSTALL_UNAVAILABLE: &str =
    "This Newport build does not include Remote Apps. Use a Newport build with the full agent.";

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
    /// What it installs, for example "Install XWayland".
    title: String,
    /// The command it was opened with; the button that opened it shows so.
    command: String,
    view: Entity<crate::terminal_view::TerminalView>,
    exit: ExitPolicy,
    _observer: Subscription,
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

/// How long each row takes to reveal its result.
const CHECK_STEP: Duration = Duration::from_millis(350);

/// One row of the diagnosis.
struct Check {
    label: String,
    detail: String,
    outcome: CheckOutcome,
    /// What the row checked, when it is a requirement.
    requirement: Option<api::Requirement>,
    /// A command or NixOS settings fixing what's missing.
    fix: Option<api::Remedy>,
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
    connections: RefCell<HashMap<String, Value>>,
    /// Whether this computer held Pro at the last account observation.
    account: Cell<Option<bool>>,
    /// The end of access already warned about while sessions were open, so a
    /// Pro plan set to end is announced once.
    warned_end: RefCell<Option<String>>,
}
impl Global for State {}

/// Set up Remote apps before the main window opens.
pub(crate) fn start(cx: &mut App) {
    if !cx.has_global::<State>() {
        cx.set_global(State::load(Sessions::new()));
        // Application observation continues while the main window is hidden.
        // The backend retains displays independently of this page.
        let sessions = Rc::downgrade(&cx.global::<State>().sessions);
        cx.spawn(async move |cx| {
            loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(250))
                    .await;
                let Some(sessions) = sessions.upgrade() else {
                    break;
                };
                cx.update(|_| sessions.poll());
            }
        })
        .detach();
    }
}

/// Sessions that stream nothing, for page tests.
#[cfg(test)]
pub(crate) fn start_scripted(cx: &mut App) {
    cx.set_global(State::load(Sessions::scripted()));
    cx.set_global(crate::terminal_gui::TestProbe(Ok(false)));
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

/// Follows account changes for launcher notices and sign-out. The backend
/// renews the shared graphical session and enforces its license.
pub(crate) fn follow_account(
    status: &newport_client::account::Status,
    cx: &mut App,
) -> Option<String> {
    use newport_client::account::Phase;
    let state = cx.try_global::<State>()?;
    let held = state.account.get();
    if status.phase == Phase::Loading {
        return None;
    }
    let holds = match status.phase {
        Phase::SignedIn => status.license.as_ref().map(|license| license.holds_pro()),
        _ => None,
    };
    state.account.set(holds);
    // Signed out, here or on the account site: a signed-out computer runs
    // nothing on the account.
    if held.is_some() && status.phase == Phase::SignedOut {
        stop_all(cx);
        return None;
    }
    let open = active_sessions(cx) > 0;
    let license = status.license.as_ref();
    if held == Some(true) && holds == Some(false) && open {
        let field = |key: &str| {
            license.and_then(|license| match key {
                "state" => Some(license.state.as_str()),
                _ => license.seat.as_deref(),
            })
        };
        // Refunded, or this computer's seat moved to another one: what the
        // account site and its portal promise is that Remote stops here now.
        let stopped_now = match (field("state"), field("seat")) {
            (Some("revoked"), _) => Some("Newport Pro was refunded, so open remote apps closed."),
            (Some("active" | "grace"), Some("free")) => Some(
                "This computer’s Pro seat moved to another computer, so its open remote apps closed.",
            ),
            _ => None,
        };
        if let Some(message) = stopped_now {
            stop_all(cx);
            return Some(message.to_owned());
        }
        return Some(
            "Newport Pro has ended. Open remote apps close when this computer’s license runs out."
                .to_owned(),
        );
    }
    // Pro is set to end (cancelled, no renewal): say when, once, while
    // remote apps are open, before they close.
    let ending = license
        .filter(|license| license.holds_pro() && license.renews_at.is_none())
        .and_then(|license| license.ends_at.clone());
    let state = cx.try_global::<State>()?;
    let mut warned = state.warned_end.borrow_mut();
    match ending {
        Some(ends) if open && warned.as_deref() != Some(ends.as_str()) => {
            *warned = Some(ends.clone());
            let date = chrono::DateTime::parse_from_rfc3339(&ends).ok()?;
            Some(format!(
                "Newport Pro ends on {}. Open remote apps close then.",
                date.with_timezone(&chrono::Local).format("%-d %b")
            ))
        }
        Some(_) => None,
        None => {
            *warned = None;
            None
        }
    }
}

/// Stop every session, before signing out.
pub(crate) fn stop_all(cx: &App) {
    if let Some(sessions) = sessions(cx) {
        sessions.stop_all();
    }
}

/// Remote apps whose windows are open or opening.
pub(crate) fn active_sessions(cx: &App) -> usize {
    sessions(cx).map_or(0, |sessions| sessions.active())
}

/// The backend has verified the installed full product and supported service.
/// Its actual executable is selected by runtime admission, never by this marker.
pub(crate) fn launch_agent(status: &Value) -> Result<String, String> {
    if let Some(error) = status["agentError"].as_str() {
        return Err(error.to_owned());
    }
    if status["installedVariant"] == "full"
        && let Some(agent) = status["agent"].as_str().filter(|agent| !agent.is_empty())
    {
        return Ok(agent.to_owned());
    }
    Err("Install the full Newport agent on this server first.".into())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Update {
    Newer,
    Older,
    Unordered,
}

/// Use the shared lifecycle's verified release ordering. UI code never starts
/// an automatic replacement; that policy belongs to the server session.
fn update_offer(status: &Value) -> Option<(String, String, Update)> {
    if status["bundledVariant"] != "full"
        || status["installedVariant"] != "full"
        || status["agentError"].is_string()
    {
        return None;
    }
    let installed = status["revision"].as_str()?;
    let bundled = status["bundledRevision"].as_str()?;
    if installed == bundled {
        return None;
    }
    let update = match status["buildRelation"].as_str()? {
        "older" => Update::Newer,
        "newer" => Update::Older,
        // Another build of the same release moves to this Newport's on its
        // own once idle; only a failed attempt needs the person.
        "different" | "sameRelease" if failed_update(status).is_some() => Update::Unordered,
        _ => return None,
    };
    Some((installed.into(), bundled.into(), update))
}

/// Why the last automatic update to this Newport's agent build failed, when
/// the server still runs another build.
fn failed_update(status: &Value) -> Option<String> {
    let outcome = &status["automaticUpdate"];
    let error = outcome["error"].as_str()?;
    (outcome["to"].as_str()? == status["bundledRevision"].as_str()?
        && status["revision"].as_str()? != status["bundledRevision"].as_str()?)
    .then(|| error.trim_end_matches('.').to_owned())
}

/// Running Remote apps that failed since last asked, worded for a toast.
pub(crate) fn take_notices(cx: &App) -> Vec<String> {
    sessions(cx).map_or_else(Vec::new, |sessions| sessions.take_notices())
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
    *state.connections.borrow_mut() = rows(&snapshot["config"]["servers"])
        .iter()
        .map(|server| {
            (
                text(server, "id"),
                newport_domain::connection_scope::connection(server, snapshot),
            )
        })
        .collect();
}

impl State {
    fn load(sessions: Sessions) -> Self {
        Self {
            sessions: Rc::new(sessions),
            connections: Default::default(),
            account: Default::default(),
            warned_end: Default::default(),
        }
    }
}

/// Preferences live in the backend's preferences store. These keys hold the
/// page's readings of them, kept while the page is away so a return draws them
/// at once.
const SETTINGS_KEY: &str = "remote-apps-settings";
/// The servers last seen with the agent, from the backend's preferences.
const INSTALLED_KEY: &str = "remote-apps-installed";

pub(crate) fn decode_settings(value: &Value) -> api::StreamingSettings {
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

const TILE_WIDTH: f32 = 112.;
const TILE_HEIGHT: f32 = 128.;
const ICON: f32 = 64.;
const GAP_X: f32 = 4.;
const GAP_Y: f32 = 8.;
/// Two lines of 13px names.
const LABEL_HEIGHT: f32 = 32.;
/// Height reserved under the grid for the page indicator.
const PAGER_HEIGHT: f32 = 40.;
/// Trackpad scroll distance that turns a page. One gesture turns at most one
/// page; its momentum is ignored. Devices without gesture phases start a new
/// gesture after a pause.
const SWIPE: f32 = 60.;
const GESTURE_GAP: Duration = Duration::from_millis(300);

pub(crate) enum LaunchpadEvent {
    /// Read the server's queries again, bypassing their freshness.
    Refresh,
    /// One of the upsell's buttons: sign in, start the trial, open Billing or
    /// the account page.
    Account(newport_client::account::Action),
    /// Read only the server's status again, bypassing its freshness. With
    /// `replace`, a read already in flight gives way to this one.
    Recheck { replace: bool },
    /// Save streaming settings in the backend's preferences.
    SaveSettings(Value),
}

/// What this computer may do with Remote apps, decided from the account
/// status, offline too: the saved license holds until the next refresh.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Entitlement {
    /// The account status hasn't loaded: show no upsell yet, so the page never
    /// flashes the wrong one.
    #[default]
    Unknown,
    SignedOut,
    /// Sign-in waits for the browser; its page can be opened again.
    Waiting,
    /// A free account that can still start its trial here.
    TrialAvailable,
    /// This computer already used a free trial, under any account.
    TrialUsedHere,
    /// The trial was used, or Pro ended.
    Ended {
        trial_used: bool,
    },
    /// A Pro account whose Pro seats are on other computers.
    FreeSeat,
    /// The account's trial holds its seat on another computer.
    TrialElsewhere,
    Pro,
}

impl Entitlement {
    pub(crate) fn of(status: &newport_client::account::Status) -> Self {
        use newport_client::account::Phase;
        match status.phase {
            Phase::Loading => Self::Unknown,
            Phase::SignedOut => Self::SignedOut,
            Phase::Waiting => Self::Waiting,
            Phase::SignedIn => {
                let Some(license) = &status.license else {
                    return Self::Unknown;
                };
                let free_seat = license.seat.as_deref() == Some("free");
                match license.state.as_str() {
                    _ if license.holds_pro() => Self::Pro,
                    "trial" if free_seat => Self::TrialElsewhere,
                    "active" | "grace" if free_seat => Self::FreeSeat,
                    "none" if license.trial_used_here && !license.trial_used => Self::TrialUsedHere,
                    "none" if !license.trial_used => Self::TrialAvailable,
                    _ => Self::Ended {
                        trial_used: license.trial_used,
                    },
                }
            }
        }
    }
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
/// Why a reading may be due before its poll: a Refresh, a return to the
/// page, or the server's connection coming back after it failed.
#[derive(Clone, Copy)]
struct Asked {
    manual: bool,
    returned: bool,
    reconnected: bool,
}
const CATALOGUE_POLL: Duration = Duration::from_secs(60);

pub(super) fn status_key(server: &str) -> String {
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

/// What the server falls short of, as the backend reported it.
fn gaps(status: &Value) -> Vec<api::Gap> {
    serde_json::from_value(status["gaps"].clone()).unwrap_or_default()
}

/// What the server's account can do about its gaps.
fn host(status: &Value) -> api::Host {
    serde_json::from_value(status["host"].clone()).unwrap_or_default()
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
    /// What last failed here: its alert's title and detail.
    error: Option<(String, String)>,
    /// Alerts the user closed, as `server/alert id`, kept across visits.
    dismissed: HashSet<String>,
    preferences_open: bool,
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
    catalogue: QueryRead,
    /// The decoded catalogue and the revision it came from.
    decoded: RefCell<Option<(u64, Vec<api::RemoteApp>)>>,
    page: usize,
    /// The measured grid area; zero until the first layout.
    area: Size<Pixels>,
    swipe: f32,
    /// Whether the current scroll gesture may still turn a page.
    armed: bool,
    scrolled_at: Option<Instant>,
    /// Whether this computer holds Newport Pro, from the account status.
    entitlement: Entitlement,
    /// An account step the upsell started is still running.
    account_busy: bool,
    /// Why this computer is on a free seat, from the account site.
    seat_message: Option<String>,
    /// The next automatic check after a failed one.
    status_retry: Option<StatusRetry>,
}

/// A check scheduled after the server check failed, for one failure.
struct StatusRetry {
    server: String,
    attempt: u32,
    /// When the failed status and app-list reads finished.
    failed_at: (Option<Instant>, Option<Instant>),
    _task: Task<()>,
}

/// The first automatic check after a failure; each later one waits twice as
/// long, up to [`STATUS_RETRY_MAX`].
const STATUS_RETRY: Duration = Duration::from_secs(5);
const STATUS_RETRY_MAX: Duration = Duration::from_secs(60);

impl Launchpad {
    pub(crate) fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let search = cx.new(|cx| InputState::new(window, cx).placeholder("Find an app"));
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
            visit: 0,
            installs: HashMap::new(),
            installed: HashSet::new(),
            terminal: None,
            terminals_opened: 0,
            status: QueryRead::default(),
            settings: QueryRead::default(),
            installed_record: QueryRead::default(),
            catalogue: QueryRead::default(),
            decoded: Default::default(),
            page: 0,
            area: Size::default(),
            swipe: 0.,
            armed: true,
            scrolled_at: None,
            entitlement: Entitlement::Unknown,
            account_busy: false,
            seat_message: None,
            status_retry: None,
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

    /// What the account status says this computer may do, and whether an
    /// account step from the upsell is still running.
    /// `seat_message` is the account site's reason this computer is on a
    /// free seat, when it gave one.
    pub(crate) fn set_account(
        &mut self,
        entitlement: Entitlement,
        seat_message: Option<String>,
        busy: bool,
    ) {
        self.entitlement = entitlement;
        self.seat_message = seat_message;
        self.account_busy = busy;
    }

    /// Without Newport Pro on this computer the page is the welcome wall with
    /// one sentence and one button for the situation. `None` with Pro.
    fn upsell(&self, cx: &mut Context<Self>) -> Option<Div> {
        use newport_client::account::Action;
        const PRO: &str = "Remote apps are part of Newport Pro.";
        const UPGRADE: (&str, Action) = ("Upgrade to Pro", Action::OpenBilling);
        let (sentence, (label, action)): (String, (&str, Action)) = match self.entitlement {
            Entitlement::Pro => return None,
            Entitlement::Unknown => {
                return Some(self.welcome(
                    0,
                    "Each app opens in its own window, right next to your Mac apps.",
                    cx,
                ));
            }
            Entitlement::SignedOut => (
                format!("{PRO} Sign in to try them free for 14 days."),
                ("Sign in", Action::SignIn),
            ),
            Entitlement::Waiting => (
                "Finish signing in in your browser. If you closed that page, open it again.".into(),
                ("Open sign-in page", Action::SignIn),
            ),
            Entitlement::TrialUsedHere => (
                format!("{PRO} This computer has already used a free trial."),
                UPGRADE,
            ),
            Entitlement::TrialAvailable => (
                format!("{PRO} Try it free for 14 days on this computer, no card needed."),
                ("Start free trial", Action::StartTrial),
            ),
            Entitlement::Ended { trial_used: true } => {
                (format!("{PRO} Your free trial has ended."), UPGRADE)
            }
            Entitlement::Ended { trial_used: false } => (PRO.to_owned(), UPGRADE),
            Entitlement::FreeSeat => (
                // The account site knows why: seats open but not chosen here,
                // or all in use.
                self.seat_message.clone().unwrap_or_else(|| {
                    "Newport Pro is in use on your other computers. Move a Pro seat here to open remote apps on this one."
                        .to_owned()
                }),
                ("Use Pro on this computer", Action::OpenPortal),
            ),
            Entitlement::TrialElsewhere => (
                "Your free trial runs on the computer that started it.".to_owned(),
                UPGRADE,
            ),
        };
        let trial = self.entitlement == Entitlement::TrialAvailable;
        Some(
            self.welcome(0, sentence, cx).child(
                div()
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap(px(6.))
                    .child(
                        div()
                            .debug_selector(move || format!("remote-apps-upsell: {label}"))
                            .child(
                                crate::controls::button("remote-apps-upsell")
                                    .primary()
                                    .label(label)
                                    .loading(self.account_busy)
                                    .disabled(self.account_busy)
                                    .on_click(cx.listener(move |_, _, _, cx| {
                                        cx.emit(LaunchpadEvent::Account(action))
                                    })),
                            ),
                    )
                    .when(trial, |column| {
                        column.child(
                            div().debug_selector(|| "remote-apps-upgrade".into()).child(
                                crate::controls::compact_button("remote-apps-upgrade")
                                    .ghost()
                                    .label(UPGRADE.0)
                                    .on_click(cx.listener(|_, _, _, cx| {
                                        cx.emit(LaunchpadEvent::Account(UPGRADE.1))
                                    })),
                            ),
                        )
                    }),
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
                self.error = Some((SOMETHING_WENT_WRONG.into(), error));
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
        let mut previous = view.read(cx).status.phase.clone();
        let observer = cx.observe(&view, move |this, view, cx| {
            let phase = view.read(cx).status.phase.clone();
            if phase == previous {
                return;
            }
            previous = phase.clone();
            let Some(terminal) = this.terminal.as_mut().filter(|terminal| terminal.id == id) else {
                return;
            };
            terminal.exit.observe(&phase);
            if terminal.exit.should_close(&phase) {
                this.close_terminal(cx);
            }
        });
        self.terminal = Some(InstallTerminal {
            id,
            title,
            command,
            view,
            exit: ExitPolicy::default(),
            _observer: observer,
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
        cx.notify();
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
                                .child("Type exit when finished to close and check again."),
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
                    let rows = this.checks().len();
                    let Some(Install::Diagnosing(diagnosis)) = this.installs.get_mut(&server)
                    else {
                        return false;
                    };
                    if diagnosis.arrived {
                        diagnosis.shown = if reduce_motion {
                            rows
                        } else {
                            (diagnosis.shown + 1).min(rows)
                        };
                        cx.notify();
                    }
                    diagnosis.shown < rows
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
        use api::Requirement;
        let value = &self.status.value;
        let name = &self.server_name;
        let gaps = gaps(value);
        let os = self.os();
        let host = host(value);
        let text = |key: &str| value[key].as_str().unwrap_or_default().to_owned();
        let (system, architecture) = (text("system"), text("architecture"));
        let passed = |label: &str, detail: &str, requirement| Check {
            label: label.to_owned(),
            detail: detail.to_owned(),
            outcome: CheckOutcome::Passed,
            requirement,
            fix: None,
        };
        let row = |gap: &api::Gap| Check {
            label: gap.title(),
            detail: gap.detail(name, os.as_ref(), &host),
            outcome: CheckOutcome::Gap,
            requirement: Some(gap.requirement),
            fix: Some(gap.remedy(os.as_ref(), &host)).filter(|fix| *fix != api::Remedy::Manual),
        };
        let requirement = |requirement: Requirement, detail: &str| {
            gaps.iter()
                .find(|gap| gap.requirement == requirement)
                .map_or_else(
                    || passed(requirement.name(), detail, Some(requirement)),
                    row,
                )
        };
        let mut checks = vec![
            passed("Connected", &format!("{name} over SSH"), None),
            if !system.is_empty() && system != "Linux" {
                Check {
                    label: "Not Linux".into(),
                    detail: format!("Remote apps need a Linux server, and {name} runs {system}."),
                    outcome: CheckOutcome::Blocked,
                    requirement: None,
                    fix: None,
                }
            } else if matches!(architecture.as_str(), "x86_64" | "aarch64") {
                passed(
                    &format!("Linux on {architecture}"),
                    "An agent is ready for it",
                    None,
                )
            } else {
                Check {
                    label: format!("Linux on {architecture}"),
                    detail: "There’s no agent for this processor yet.".into(),
                    outcome: CheckOutcome::Blocked,
                    requirement: None,
                    fix: None,
                }
            },
            requirement(Requirement::Dbus, Requirement::Dbus.purpose()),
            requirement(Requirement::Xwayland, Requirement::Xwayland.purpose()),
            requirement(Requirement::Audio, Requirement::Audio.purpose()),
        ];
        // Whatever else is missing follows the rows every server gets.
        checks.extend(
            gaps.iter()
                .filter(|gap| {
                    !matches!(
                        gap.requirement,
                        Requirement::Dbus | Requirement::Xwayland | Requirement::Audio
                    )
                })
                .map(row),
        );
        checks
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
            self.error = Some((SOMETHING_WENT_WRONG.into(), INSTALL_UNAVAILABLE.into()));
            cx.notify();
            return;
        }
        let server = self.server.clone();
        let (progress, finished) = match install::start(&server, &self.status.value) {
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

    fn outdated(&self) -> Option<(String, String, Update)> {
        update_offer(&self.status.value)
    }

    /// While the server check or the app list has failed, read them again
    /// without waiting for Try again, so a server that becomes reachable
    /// recovers on its own.
    fn retry_failed_status(&mut self, cx: &mut Context<Self>) {
        if self.status.pending || self.catalogue.pending {
            return;
        }
        let failed = matches!(self.status(), api::ServerStatus::Failed(_))
            || matches!(self.catalogue(), api::Catalogue::Failed(_));
        if !failed {
            self.status_retry = None;
            return;
        }
        let failed_at = (self.status.finished_at, self.catalogue.finished_at);
        let attempt = match &self.status_retry {
            Some(retry) if retry.server == self.server && retry.failed_at == failed_at => return,
            Some(retry) if retry.server == self.server => retry.attempt + 1,
            _ => 0,
        };
        let delay = STATUS_RETRY
            .saturating_mul(1 << attempt.min(4))
            .min(STATUS_RETRY_MAX);
        let server = self.server.clone();
        let task = cx.spawn({
            let server = server.clone();
            async move |this, cx| {
                cx.background_executor().timer(delay).await;
                let _ = this.update(cx, |this, cx| {
                    if this.server == server {
                        cx.emit(LaunchpadEvent::Refresh);
                    }
                });
            }
        });
        self.status_retry = Some(StatusRetry {
            server,
            attempt,
            failed_at,
            _task: task,
        });
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
        self.status
            .observe(resources.get(&status_key(&self.server)));
        let read = &self.status;
        if let Some(Install::Diagnosing(diagnosis)) = self.installs.get_mut(&self.server) {
            // Only a read finished after the check began counts, not an
            // older reading the page hadn't drawn yet.
            let fresh = read.finished_at.is_some_and(|at| at >= diagnosis.started);
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

    /// Whether this server can run apps, from the server query.
    fn status(&self) -> api::ServerStatus {
        let read = &self.status;
        if !read.loaded {
            return match &read.error {
                Some(error) if !read.pending => api::ServerStatus::Failed(error.clone()),
                _ => api::ServerStatus::Checking,
            };
        }
        if let Some(error) = read.value["agentError"].as_str() {
            return api::ServerStatus::Failed(error.into());
        }
        let gaps = gaps(&read.value);
        if launch_agent(&read.value).is_err() {
            api::ServerStatus::NeedsInstall
        } else if !gaps.is_empty() {
            api::ServerStatus::Gaps(gaps)
        } else {
            api::ServerStatus::Ready
        }
    }

    /// A verified full agent; the backend chooses its admitted runtime build.
    fn launch_agent(&self) -> Result<String, String> {
        launch_agent(&self.status.value)
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
        let api::Catalogue::Ready(apps) = self.catalogue() else {
            return None;
        };
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
        let title = format!("Couldn’t open {name}");
        // A new attempt replaces what the last one reported.
        for ended in sessions.list().iter().filter(|s| {
            s.server == self.server
                && s.app_id == app
                && matches!(s.state, api::SessionState::Ended { .. })
        }) {
            sessions.forget(ended.id);
        }
        let request = api::LaunchRequest {
            app_id: app.to_owned(),
            name,
            action,
        };
        let missing = self.missing_for_launch(&request.name);
        if missing.is_err() {
            // The software may have been installed since the last check:
            // check again, so trying again sees it.
            cx.emit(LaunchpadEvent::Refresh);
        }
        self.error = missing
            .and_then(|()| self.launch_agent())
            .and_then(|agent| sessions.launch(self.launch_options(&agent, cx), request, cx))
            .err()
            .map(|error| (title, error));
        cx.notify();
    }

    /// Why no app can start yet: software every app needs is missing.
    fn missing_for_launch(&self, app: &str) -> Result<(), String> {
        let blocking: Vec<_> = gaps(&self.status.value)
            .into_iter()
            .filter(|gap| !gap.optional())
            .collect();
        if blocking.is_empty() {
            return Ok(());
        }
        let what = if blocking.len() == 1 { "it" } else { "them" };
        let advice = if blocking.iter().all(api::Gap::installs) {
            newport_domain::requirements::install_advice(
                what,
                self.os().as_ref(),
                &host(&self.status.value),
                "then try again.",
            )
        } else {
            "Check again once that’s fixed.".into()
        };
        Err(format!(
            "{app} needs {} on {}. {advice}",
            newport_domain::requirements::listed(&blocking),
            self.server_name
        ))
    }

    fn launch_options<'a>(&'a self, agent: &'a str, cx: &App) -> Launch<'a> {
        Launch {
            server: &self.server,
            expected_connection: cx
                .try_global::<State>()
                .and_then(|state| state.connections.borrow().get(&self.server).cloned())
                .unwrap_or_default(),
            agent,
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
        crate::controls::page_toolbar(cx)
            .child(crate::controls::page_search(
                &self.search,
                "Find an app",
                cx,
            ))
            .child(div().flex_1())
            .child(
                div()
                    .text_size(px(crate::ui::FONT_SIZE))
                    .text_color(cx.theme().muted_foreground)
                    .child(summary),
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
            api::ServerStatus::Ready | api::ServerStatus::Gaps(_) if activating => {
                self.activation_screen(cx)
            }
            api::ServerStatus::Ready | api::ServerStatus::Gaps(_) => return None,
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
                requirement: None,
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
        let rows = checks
            .into_iter()
            .enumerate()
            .map(|(index, check)| {
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
                        check.label.clone(),
                        "Checking…".to_owned(),
                    )
                } else {
                    (
                        Icon::new(IconName::Circle)
                            .size(px(16.))
                            .text_color(cx.theme().muted_foreground)
                            .into_any_element(),
                        check.label.clone(),
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
                    .when_some(check.fix.filter(|_| index < shown), |row, fix| {
                        let button =
                            crate::controls::compact_button(("remote-apps-check-fix", index));
                        let button = match fix {
                            api::Remedy::Command(command) => {
                                let open = self
                                    .terminal
                                    .as_ref()
                                    .is_some_and(|terminal| terminal.command == command);
                                let run = command.clone();
                                let title = check
                                    .requirement
                                    .map_or("Install".into(), |r| format!("Install {}", r.noun()));
                                button
                                    .icon(IconName::SquareTerminal)
                                    .label(if open { "Installing…" } else { "Install" })
                                    .tooltip(command)
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.open_terminal(title.clone(), run.clone(), window, cx)
                                    }))
                            }
                            api::Remedy::Configure(settings) => {
                                let copy = settings.clone();
                                button
                                    .icon(IconName::Copy)
                                    .label("Copy settings")
                                    .tooltip(settings)
                                    .on_click(move |_, _, cx| {
                                        cx.write_to_clipboard(ClipboardItem::new_string(
                                            copy.clone(),
                                        ))
                                    })
                            }
                            api::Remedy::Manual => return row,
                        };
                        row.child(
                            div()
                                .debug_selector(move || format!("remote-apps-check-fix-{index}"))
                                .child(button),
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
            })
            .collect::<Vec<_>>();
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
                .flex()
                .gap_2()
                .debug_selector(|| "remote-apps-activate".into())
                .when(gaps > 0, |row| {
                    row.child(
                        div()
                            .debug_selector(|| "remote-apps-check-again".into())
                            .child(
                                crate::controls::button("remote-apps-check-again")
                                    .label("Check again")
                                    .loading(self.status.pending)
                                    .on_click(cx.listener(|this, _, _, cx| this.recheck(cx))),
                            ),
                    )
                })
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

    /// Why `app` couldn't open, in at most two lines, and what fixes it.
    fn failure(&self, reason: &str, app: &str) -> (String, Option<api::Fix>) {
        let server = self.server_name.as_str();
        // Chromium and Electron apps print this and exit when run as root.
        if reason.contains("as root without --no-sandbox") {
            return (
                format!(
                    "{app} won’t run as root. Connect to {server} as a regular user to open it."
                ),
                None,
            );
        }
        // An app that needed X11, on a server without the XWayland that
        // would have given it.
        let xwayland = gaps(&self.status.value)
            .into_iter()
            .find(|gap| gap.requirement == api::Requirement::Xwayland && gap.installs());
        if let Some(gap) = xwayland
            && reason.lines().any(no_x11_display)
        {
            let os = self.os();
            let host = host(&self.status.value);
            let found = match &gap.finding {
                api::Finding::Outdated { version } => format!(
                    "{server} has XWayland {version}, and Newport needs {} or newer",
                    newport_domain::requirements::XWAYLAND_MINIMUM
                ),
                _ => format!("{server} has no XWayland"),
            };
            let advice = newport_domain::requirements::install_advice(
                "XWayland",
                os.as_ref(),
                &host,
                "then open it again.",
            );
            let fix = match gap.remedy(os.as_ref(), &host) {
                api::Remedy::Command(command) => Some(api::Fix::Command(command)),
                api::Remedy::Configure(settings) => Some(api::Fix::Configure(settings)),
                api::Remedy::Manual => None,
            };
            return (format!("{app} needs X11, and {found}. {advice}"), fix);
        }
        (first_lines(reason), None)
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
        if let api::ServerStatus::Gaps(gaps) = status {
            // XWayland for Wayland apps' X11 parts is offered where an app
            // needed it, and in the setup checks.
            let gaps: Vec<_> = gaps
                .iter()
                .filter(|gap| gap.requirement != api::Requirement::Xwayland)
                .cloned()
                .collect();
            let host = host(&self.status.value);
            let fix = |remedy| match remedy {
                api::Remedy::Command(command) => api::Fix::Command(command),
                api::Remedy::Configure(settings) => api::Fix::Configure(settings),
                api::Remedy::Manual => api::Fix::Refresh,
            };
            // Everything a package fixes installs together, what every app
            // needs named first.
            let mut installs: Vec<_> = gaps.iter().filter(|gap| gap.installs()).cloned().collect();
            installs.sort_by_key(api::Gap::optional);
            if !installs.is_empty() {
                let optional = installs.iter().all(api::Gap::optional);
                let (label, title, detail) = match installs.as_slice() {
                    [gap] => (
                        gap.requirement.noun().to_owned(),
                        gap.title(),
                        gap.detail(server, os.as_ref(), &host),
                    ),
                    _ => (
                        "missing software".to_owned(),
                        "Software is missing".to_owned(),
                        format!(
                            "{} need {} on {server}. {}",
                            if optional {
                                "Some remote apps"
                            } else {
                                "Remote apps"
                            },
                            newport_domain::requirements::listed(&installs),
                            newport_domain::requirements::install_advice(
                                "them",
                                os.as_ref(),
                                &host,
                                "then check again."
                            )
                        ),
                    ),
                };
                // Required software cannot be dismissed: nothing launches without it.
                alerts.push((
                    api::Alert {
                        id: format!("missing:{label}"),
                        severity: if optional {
                            api::Severity::Warning
                        } else {
                            api::Severity::Error
                        },
                        title,
                        detail,
                        fix: Some(fix(newport_domain::requirements::install_remedy(
                            &installs,
                            os.as_ref(),
                            &host,
                        ))),
                        output: None,
                    },
                    optional,
                ));
            }
            for gap in gaps.iter().filter(|gap| !gap.installs()) {
                alerts.push((
                    api::Alert {
                        id: format!("gap:{}", gap.requirement.name()),
                        severity: if gap.optional() {
                            api::Severity::Warning
                        } else {
                            api::Severity::Error
                        },
                        title: gap.title(),
                        detail: gap.detail(server, os.as_ref(), &host),
                        fix: Some(fix(gap.remedy(os.as_ref(), &host))),
                        output: None,
                    },
                    gap.optional(),
                ));
            }
        }
        for (read, what) in [(&self.settings, "preferences")] {
            if let Some(error) = read.error.as_ref().filter(|_| !read.pending) {
                alerts.push((
                    api::Alert {
                        id: format!("preferences:{what}:{error}"),
                        severity: api::Severity::Error,
                        title: format!("Couldn’t save or read Remote apps {what}"),
                        detail: error.clone(),
                        fix: None,
                        output: None,
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
                    output: None,
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
                    output: None,
                },
                false,
            )),
            Some(Install::Finished(..) | Install::Running(..)) => {}
            _ => {
                if matches!(
                    status,
                    api::ServerStatus::Ready | api::ServerStatus::Gaps(_)
                ) && let Some((installed, bundled, update)) = self.outdated()
                {
                    let failed = failed_update(&self.status.value);
                    let (title, detail) = match update {
                        _ if failed.is_some() && update != Update::Older => (
                            "This server runs an older Newport agent".to_owned(),
                            format!("Update failed: {}.", failed.unwrap_or_default()),
                        ),
                        Update::Newer => (
                            "A newer version of Newport Remote is available".to_owned(),
                            format!(
                                "Update installs it on {server}. Apps that are open keep running."
                            ),
                        ),
                        Update::Older => (
                            format!("{server} has a newer Newport Remote than this Newport"),
                            "Apps use the compatible installed agent. Update Newport to manage this newer release.".to_owned(),
                        ),
                        Update::Unordered => (
                            "Another version of Newport Remote is available".to_owned(),
                            format!(
                                "Update installs this Newport’s version on {server}. Apps that are open keep running."
                            ),
                        ),
                    };
                    alerts.push((
                        api::Alert {
                            // A later build's agent asks again.
                            id: format!("update:{installed}:{bundled}"),
                            severity: api::Severity::Warning,
                            title,
                            detail,
                            fix: (update != Update::Older).then_some(api::Fix::UpdateAgent),
                            output: None,
                        },
                        true,
                    ));
                }
            }
        }
        if let Some((title, error)) = &self.error {
            alerts.push((
                api::Alert {
                    id: format!("error:{error}"),
                    severity: api::Severity::Error,
                    title: title.clone(),
                    detail: error.clone(),
                    fix: None,
                    output: None,
                },
                true,
            ));
        }
        for session in sessions {
            if let api::SessionState::Uncertain { message } = &session.state {
                alerts.push((
                    api::Alert {
                        id: format!("session:{}", session.id.0),
                        severity: api::Severity::Warning,
                        title: format!("Check {}", session.app_name),
                        detail: format!(
                            "{message} Check its open windows before repeating the launch."
                        ),
                        fix: None,
                        output: None,
                    },
                    true,
                ));
                continue;
            }
            // Information only: the app keeps running.
            if session.state.is_active()
                && let Some(notice) = session.notice
            {
                let app = &session.app_name;
                let (title, detail) = match notice {
                    api::Notice::DialogsOnServerScreen => (
                        format!("File dialogs from {app} open on the server’s own screen"),
                        format!(
                            "Someone is signed in there. {app} waits until a dialog there is closed."
                        ),
                    ),
                    api::Notice::DialogsOnOtherMac => (
                        format!("File dialogs from {app} open on another Mac"),
                        format!(
                            "That Mac uses this server with the same account. {app} waits until a dialog there is closed."
                        ),
                    ),
                };
                alerts.push((
                    api::Alert {
                        id: format!("dialogs:{}", session.id.0),
                        severity: api::Severity::Info,
                        title,
                        detail,
                        fix: None,
                        output: None,
                    },
                    true,
                ));
            }
            if session.state == api::SessionState::Background {
                alerts.push((
                    api::Alert {
                        id: format!("background:{}", session.id.0),
                        severity: api::Severity::Info,
                        title: format!("{} is running without a window", session.app_name),
                        detail: "It may run in the background, such as in the system tray, which Remote apps can’t show.".into(),
                        fix: Some(api::Fix::Quit(session.id)),
                        output: None,
                    },
                    true,
                ));
                continue;
            }
            let api::SessionState::Ended {
                ending: Some(ending),
            } = &session.state
            else {
                continue;
            };
            let id = format!("session:{}", session.id.0);
            let alert = match ending {
                // A launch another process opened shows its window there, as
                // macOS does when an app is already running: nothing to report.
                api::Ending::HandedOff => continue,
                api::Ending::DisplayStopped => api::Alert {
                    id,
                    severity: api::Severity::Warning,
                    title: format!("{} closed", session.app_name),
                    detail: format!(
                        "The connection to {server} dropped or Needle stopped there, and the app’s windows closed with it. Open it again."
                    ),
                    fix: None,
                    output: None,
                },
                api::Ending::Failed(reason) => {
                    let (detail, fix) = self.failure(reason, &session.app_name);
                    api::Alert {
                        id,
                        severity: api::Severity::Error,
                        title: format!("Couldn’t open {}", session.app_name),
                        output: full_output(&detail, reason),
                        detail,
                        fix,
                    }
                }
                // Tray apps and daemons start fine but never show a window.
                api::Ending::NoWindow(output) => {
                    let detail = match output {
                        Some(output) => format!("It reported: {}", first_lines(output)),
                        None => "It may run in the background, such as in the system tray, which Remote apps can’t show.".into(),
                    };
                    api::Alert {
                        id,
                        severity: api::Severity::Warning,
                        title: format!("{} closed without opening a window", session.app_name),
                        output: output
                            .as_deref()
                            .filter(|output| first_lines(output) != *output || fills_alert(&detail))
                            .map(str::to_owned),
                        detail,
                        fix: None,
                    }
                }
                api::Ending::Refused { message, action } => api::Alert {
                    id,
                    severity: api::Severity::Error,
                    title: format!("Couldn’t open {}", session.app_name),
                    detail: message.clone(),
                    fix: action.map(api::Fix::Account),
                    output: None,
                },
                api::Ending::Unavailable(error) => api::Alert {
                    id,
                    severity: api::Severity::Warning,
                    title: format!("Can’t control {}", session.app_name),
                    detail: format!("{error} The app may still be running on the server."),
                    fix: None,
                    output: None,
                },
            };
            alerts.push((alert, true));
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
            api::Severity::Info => (IconName::Info, cx.theme().info),
        };
        let check = |index| {
            crate::controls::compact_button(("remote-apps-alert-check", index))
                .label("Check again")
                .on_click(cx.listener(|this, _, _, cx| this.refresh(cx)))
        };
        let mut actions = div().flex_shrink_0().flex().items_center().gap(px(6.));
        let mut command_line = None;
        match alert.fix.clone() {
            Some(api::Fix::Command(command)) => {
                let (run, copy) = (command.clone(), command.clone());
                // "missing:XWayland" installs XWayland.
                let title = match alert.id.strip_prefix("missing:") {
                    Some(software) => format!("Install {software}"),
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
            Some(api::Fix::Configure(settings)) => {
                let copy = settings.clone();
                command_line = Some(settings);
                actions = actions
                    .child(
                        div()
                            .debug_selector(move || format!("remote-apps-alert-copy-{index}"))
                            .child(
                                crate::controls::compact_button(("remote-apps-alert-copy", index))
                                    .icon(IconName::Copy)
                                    .label("Copy settings")
                                    .on_click(move |_, _, cx| {
                                        cx.write_to_clipboard(ClipboardItem::new_string(
                                            copy.clone(),
                                        ))
                                    }),
                            ),
                    )
                    .child(check(index));
            }
            Some(api::Fix::Refresh) => actions = actions.child(check(index)),
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
            Some(api::Fix::Account(action)) => {
                use newport_client::account::Action;
                let label = match action {
                    Action::SignIn => "Sign in",
                    Action::OpenPortal => "Use Pro on this computer",
                    _ => "See plans",
                };
                actions = actions.child(
                    div()
                        .debug_selector(move || format!("remote-apps-alert-account-{index}"))
                        .child(
                            crate::controls::compact_button(("remote-apps-alert-account", index))
                                .label(label)
                                .on_click(cx.listener(move |_, _, _, cx| {
                                    cx.emit(LaunchpadEvent::Account(action))
                                })),
                        ),
                );
            }
            Some(api::Fix::Quit(session)) => {
                actions = actions.child(
                    div()
                        .debug_selector(move || format!("remote-apps-alert-quit-{index}"))
                        .child(
                            crate::controls::compact_button(("remote-apps-alert-quit", index))
                                .label("Quit")
                                .on_click(move |_, _, cx| {
                                    if let Some(sessions) = sessions(cx) {
                                        sessions.stop(session);
                                    }
                                }),
                        ),
                );
            }
            None => {}
        }
        if let Some(output) = alert.output.clone() {
            let title = alert.title.clone();
            actions = actions.child(
                div()
                    .debug_selector(move || format!("remote-apps-alert-output-{index}"))
                    .child(
                        crate::controls::compact_button(("remote-apps-alert-output", index))
                            .label("Show full error")
                            .on_click(move |_, window, cx| {
                                full_error(title.clone(), output.clone(), window, cx)
                            }),
                    ),
            );
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
                    .child(
                        div()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(alert.title.clone()),
                    )
                    .child(
                        div()
                            .text_color(cx.theme().muted_foreground)
                            // Only an app's own report is cut short, and
                            // Show full error then has all of it.
                            .when(alert.output.is_some(), |detail| detail.line_clamp(2))
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
                    .child("Changes apply when a new GUI session starts."),
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
        let has_menu = !app.actions.is_empty() || !running.is_empty();
        let tile = tile_frame()
            .id(("remote-app", index))
            .debug_selector(move || format!("remote-app-{index}"))
            .role(Role::Button)
            .aria_label(app.name.clone())
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
                    .h(px(LABEL_HEIGHT))
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
            if !menu_app.actions.is_empty() {
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

/// The first two lines of what an app reported: usually its own words for
/// what went wrong, before what failed because of it.
fn first_lines(reason: &str) -> String {
    reason
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .take(2)
        .collect::<Vec<_>>()
        .join("\n")
}

/// Everything an app reported, when its alert shows less of it: other
/// words, fewer lines, or more text than the alert's two lines hold.
fn full_output(shown: &str, reported: &str) -> Option<String> {
    (shown != reported || fills_alert(shown)).then(|| reported.to_owned())
}

/// Whether an alert's text may not fit in the two lines it is clamped to.
fn fills_alert(shown: &str) -> bool {
    shown.chars().count() > CLAMPED_CHARS
}

/// About as much text as two lines of an alert hold at the narrowest
/// window width.
const CLAMPED_CHARS: usize = 120;

/// Everything an app reported, to read, select and copy.
fn full_error(title: String, output: String, window: &mut Window, cx: &mut App) {
    let text = cx.new(|cx| {
        crate::controls::code_editor(window, cx)
            .soft_wrap(true)
            .default_value(output.clone())
    });
    let title: SharedString = title.into();
    window.open_dialog(cx, move |dialog, window, cx| {
        let copy = output.clone();
        dialog
            .title(crate::controls::dialog_title(title.clone()))
            .w(px(640.))
            .max_w((window.viewport_size().width - px(32.)).max(px(240.)))
            .p(px(24.))
            .rounded(px(crate::ui::RADIUS))
            .border_0()
            .centered(true)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .pt_3()
                    .child(
                        div()
                            .h(px(320.))
                            .border_1()
                            .border_color(cx.theme().border)
                            .rounded(px(crate::ui::RADIUS))
                            .child(
                                gpui_kit::component::input::Editor::new(&text)
                                    .readonly(true)
                                    .appearance(false)
                                    .bordered(false)
                                    .aria_label(format!("{title}: what the app reported"))
                                    .h_full()
                                    .text_size(px(12.))
                                    .p(px(10.)),
                            ),
                    )
                    .child(
                        crate::project_dialog::actions()
                            .child(
                                crate::controls::form_button("remote-apps-full-error-copy")
                                    .icon(IconName::Copy)
                                    .label("Copy")
                                    .on_click(move |_, _, cx| {
                                        cx.write_to_clipboard(ClipboardItem::new_string(
                                            copy.clone(),
                                        ))
                                    }),
                            )
                            .child(
                                crate::controls::form_button("remote-apps-full-error-done")
                                    .primary()
                                    .label("Done")
                                    .on_click(|_, window, cx| window.close_dialog(cx)),
                            ),
                    ),
            )
    });
}

/// Toolkits' and runtimes' own words for having no X11 display to open.
fn no_x11_display(line: &str) -> bool {
    let line = line.to_ascii_lowercase();
    [
        "can't open display",
        "cannot open display",
        "unable to open display",
        "could not connect to display",
        "can't connect to x11 window server",
        "no x11 display variable",
        "missing x server or $display",
        "no display name and no $display",
    ]
    .iter()
    .any(|words| line.contains(words))
}

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

/// The grid's tiles start at the page edge, under the toolbar, and wrap
/// after `columns`.
fn grid(columns: usize) -> Div {
    div()
        .w(px(columns as f32 * (TILE_WIDTH + GAP_X) - GAP_X))
        .flex()
        .flex_wrap()
        .content_start()
        .gap_x(px(GAP_X))
        .gap_y(px(GAP_Y))
}

/// The box an app tile and its placeholder share, so loading doesn't shift
/// icons or names.
fn tile_frame() -> Div {
    div()
        .w(px(TILE_WIDTH))
        .h(px(TILE_HEIGHT))
        .flex_shrink_0()
        .flex()
        .flex_col()
        .items_center()
        .pt(px(12.))
        .px(px(6.))
        .gap(px(8.))
}

/// Placeholder tiles for one page while the server is checked or apps load.
fn skeleton((columns, rows): (usize, usize), cx: &App) -> Div {
    let mut placeholders = grid(columns);
    for index in 0..columns * rows {
        placeholders = placeholders.child(
            tile_frame()
                .debug_selector(move || format!("remote-app-placeholder-{index}"))
                .child(div().size(px(ICON)).rounded(px(14.)).bg(cx.theme().muted))
                // A name-width bar centred on the first label line.
                .child(
                    div()
                        .h(px(LABEL_HEIGHT))
                        .pt(px(3.))
                        .child(div().w(px(72.)).h(px(10.)).bg(cx.theme().muted)),
                ),
        );
    }
    placeholders
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
        // Remote apps need Newport Pro on this computer before anything else.
        if let Some(upsell) = self.upsell(cx) {
            return root.child(upsell).into_any_element();
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
            api::Catalogue::Loading => skeleton(self.layout(), cx).into_any_element(),
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
                    let mut grid = grid(columns);
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
        let panel = self.preferences_open.then(|| self.preferences(cx));
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
                    LaunchpadEvent::Account(action) => this.run_account(*action, cx),
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
        let entitlement = Entitlement::of(self.account.status());
        let seat_message = self
            .account
            .status()
            .license
            .as_ref()
            .and_then(|license| license.message.clone())
            .filter(|message| !message.trim().is_empty());
        // While sign-in waits for the browser the button stays usable: it
        // opens the same page again.
        let busy = matches!(
            self.account.pending(),
            Some(
                newport_client::account::Action::SignIn
                    | newport_client::account::Action::StartTrial
            )
        );
        view.update(cx, |view, cx| {
            view.set_account(entitlement, seat_message, busy);
            view.show(&id, &name, visit, window, cx);
            view.observe(resources);
            view.retry_failed_status(cx);
        });
        div()
            .size_full()
            .flex()
            .flex_col()
            .children(self.terminal_launch_banner(cx))
            .child(div().flex_1().min_h_0().child(view))
            .into_any_element()
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
                    Asked {
                        manual: true,
                        returned: false,
                        reconnected: false,
                    },
                );
            }
        }
        // A return reads everything again; the view reset itself when drawn.
        let returned = std::mem::take(&mut self.remote_apps_returned);
        let server = self.server.clone();
        // Reads that failed before the server's connection came back are
        // asked again at once rather than at their next poll.
        let transport = &self.snapshot["runtime"]["agentTransport"][&server];
        let connected = (transport["status"] == "connected")
            .then(|| transport["generation"].as_u64())
            .flatten();
        let current = Some((server.clone(), connected));
        let reconnected = connected.is_some()
            && std::mem::replace(&mut self.remote_apps_transport, current.clone()) != current;
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
        self.poll_remote_query(
            &status_key(&server),
            "remote_apps_status",
            &server,
            STATUS_POLL,
            Asked {
                manual,
                returned,
                reconnected,
            },
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
                Asked {
                    manual,
                    returned,
                    reconnected,
                },
            );
        }
    }

    fn poll_remote_query(
        &mut self,
        key: &str,
        method: &str,
        server: &str,
        every: Duration,
        asked: Asked,
    ) {
        let Asked {
            manual,
            returned,
            reconnected,
        } = asked;
        // A return asks again, but the backend answers from its cache while fresh.
        let due = manual
            || returned
            || self.resources.get(key).is_none_or(|read| {
                !read.pending
                    && (read.finished_at.is_none_or(|at| at.elapsed() >= every)
                        || reconnected && read.error.is_some())
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
    use super::{Update, failed_update, launch_agent, update_offer};
    use crate::page_perf_tests::{PageHost, frame, owner};
    use crate::remote_apps_api as api;
    use gpui_kit::{self as gpui, AppContext, Modifiers, TestAppContext, VisualTestContext};
    use serde_json::{Value, json};
    use std::{cell::RefCell, rc::Rc};

    const AGENT: &str = "newport-agent";

    #[test]
    fn launches_require_the_verified_full_product_not_the_bundled_hash() {
        let status = json!({"agent":"newport-agent", "installedVariant":"full", "revision":"newer", "bundledRevision":"ours"});
        assert_eq!(launch_agent(&status).unwrap(), "newport-agent");
        assert!(launch_agent(&json!({"agent":"/old/run-agent"})).is_err());
        assert!(
            launch_agent(&json!({"agent":"newport-agent", "installedVariant":"core"})).is_err()
        );
        assert_eq!(
            launch_agent(&json!({"agentError":"Incompatible protocol"})).unwrap_err(),
            "Incompatible protocol"
        );
    }

    #[test]
    fn a_failed_automatic_update_is_shown_only_for_a_different_build() {
        let mut status = json!({"installedVariant":"full", "bundledVariant":"full", "revision":"theirs", "bundledRevision":"ours", "buildRelation":"older",
            "automaticUpdate": {"at":1, "from":"theirs", "to":"ours", "error":"upload failed."}});
        assert_eq!(failed_update(&status).as_deref(), Some("upload failed"));
        status["automaticUpdate"]["to"] = json!("another");
        assert_eq!(failed_update(&status), None);
        status["automaticUpdate"]["to"] = json!("ours");
        status["automaticUpdate"]["error"] = json!(null);
        assert_eq!(failed_update(&status), None);
        status["automaticUpdate"]["error"] = json!("x");
        status["revision"] = json!("ours");
        assert_eq!(failed_update(&status), None);
    }

    #[test]
    fn update_offers_follow_the_shared_lifecycle() {
        let mut status = json!({"installedVariant":"full", "bundledVariant":"full", "revision":"theirs", "bundledRevision":"ours", "buildRelation":"older"});
        assert_eq!(update_offer(&status).unwrap().2, Update::Newer);
        status["buildRelation"] = json!("newer");
        assert_eq!(update_offer(&status).unwrap().2, Update::Older);
        status["buildRelation"] = json!("different");
        assert!(
            update_offer(&status).is_none(),
            "another build of the release updates on its own"
        );
        status["automaticUpdate"] =
            json!({"at":1, "from":"theirs", "to":"ours", "error":"upload failed."});
        assert_eq!(update_offer(&status).unwrap().2, Update::Unordered);
        status["installedVariant"] = json!("core");
        assert!(update_offer(&status).is_none());
        status["installedVariant"] = json!("full");
        status["revision"] = json!("ours");
        assert!(update_offer(&status).is_none());
    }

    /// A server with the agent installed, as the backend's status query reports it.
    fn ready() -> Value {
        json!({"installedVariant": "full", "agent": AGENT, "osRelease": "ID=ubuntu\nPRETTY_NAME=\"Ubuntu 24.04 LTS\""})
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

    /// The rows every server's checklist has.
    const ROWS: usize = 5;

    /// Let the diagnosis reveal every row.
    fn reveal(visual: &mut VisualTestContext) {
        visual
            .executor()
            .advance_clock(super::CHECK_STEP * (ROWS as u32 + 1));
        visual.run_until_parked();
        frame(visual);
    }

    /// An account whose backend answers every request with `status`, and
    /// records what was asked.
    fn account(
        status: serde_json::Value,
    ) -> (
        newport_client::account::Account,
        Rc<RefCell<Vec<&'static str>>>,
    ) {
        let asked = Rc::new(RefCell::new(Vec::new()));
        let record = asked.clone();
        let account = newport_client::account::Account::with_replies(move |method| {
            record.borrow_mut().push(method);
            Ok(status.clone())
        });
        (account, asked)
    }

    /// The account status of a computer that holds Newport Pro, or not.
    fn signed_in(license: serde_json::Value) -> serde_json::Value {
        json!({"status":"signedIn","email":"ada@example.com","license":license})
    }

    /// Put `account` in the app and let it read its first status.
    fn use_account(
        view: &gpui::Entity<crate::Newport>,
        visual: &mut VisualTestContext,
        mut account: newport_client::account::Account,
    ) {
        account.poll();
        account.poll();
        view.update(visual, |v, _| v.account = account);
        frame(visual);
    }

    /// The Remote apps page with scripted sessions, at an optional size, on a
    /// computer that holds Newport Pro.
    fn page(
        cx: &mut TestAppContext,
        size: Option<(f32, f32)>,
    ) -> (gpui::Entity<crate::Newport>, &mut VisualTestContext) {
        let (view, visual) = page_without_pro(cx, size);
        let (pro, _) = account(signed_in(
            json!({"state":"active","plan":"pro","seat":"pro"}),
        ));
        use_account(&view, visual, pro);
        (view, visual)
    }

    /// The page before the account status says anything about Pro.
    fn page_without_pro(
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
                "gaps": missing, "osRelease": "ID=debian"})
        };
        let dbus = json!({"requirement": "dbus", "state": "missing"});
        let xwayland = json!({"requirement": "xwayland", "state": "missing"});
        // The welcome screen: the wall and one button, nothing else.
        feed(
            &view,
            visual,
            linux(json!([dbus.clone(), xwayland.clone()])),
            None,
        );
        assert!(visual.debug_bounds("remote-apps-wall").is_some());
        assert!(visual.debug_bounds("remote-apps-get-started").is_some());
        assert!(visual.debug_bounds("remote-apps-alert-0").is_none());
        // Get started checks the server afresh, then reveals each row.
        click(visual, "remote-apps-get-started");
        assert!(visual.debug_bounds("remote-apps-diagnosis").is_some());
        assert!(visual.debug_bounds("remote-apps-check-again").is_none());
        answer(
            &view,
            visual,
            linux(json!([dbus.clone(), xwayland.clone()])),
        );
        reveal(visual);
        // Missing host packages block apps, not the shared agent installation.
        assert!(visual.debug_bounds("remote-apps-check-fix-2").is_some());
        assert!(visual.debug_bounds("remote-apps-check-fix-3").is_some());
        assert!(visual.debug_bounds("remote-apps-activate").is_some());
        assert!(visual.debug_bounds("remote-apps-check-again").is_some());
        // Rechecking dependencies does not replace or reinstall the agent.
        click(visual, "remote-apps-check-again");
        answer(&view, visual, linux(json!([xwayland.clone()])));
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
        // The release installed is this build's (when it carries one).
        let new = "new";
        feed(
            &view,
            visual,
            json!({"installedVariant": "full", "agent": format!("/c/releases/system/{new}/current/run-agent"), "revision": new,
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
        let installed = json!({"installedVariant": "full", "agent": "newport-agent",
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
        let xwayland = json!({"requirement": "xwayland", "state": "missing"});
        let missing = json!({"agent": null, "system": "Linux", "architecture": "aarch64",
            "gaps": [xwayland], "osRelease": "ID=debian"});
        let present = json!({"agent": null, "system": "Linux", "architecture": "aarch64",
            "gaps": [], "osRelease": "ID=debian"});
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
            assert_eq!(terminal.title, "Install XWayland");
            // No systemd user services: no recommendations, which would
            // bring systemd along.
            assert_eq!(
                terminal.command,
                "sudo apt update && sudo apt install --no-install-recommends xwayland"
            );
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
        assert_eq!(shown(&view, visual), ROWS);
        assert!(visual.debug_bounds("remote-apps-check-4").is_some());
        // Once XWayland is there, its row turns green without replaying the list.
        answer(&view, visual, present.clone());
        assert!(visual.debug_bounds("remote-apps-check-fix-3").is_none());
        assert_eq!(shown(&view, visual), ROWS);
        // Closing the shell checks once more, quietly.
        let before = read_at(&view, visual);
        click(visual, "remote-apps-terminal-close");
        assert!(visual.debug_bounds("remote-apps-terminal").is_none());
        assert_ne!(read_at(&view, visual), before);
        assert_eq!(shown(&view, visual), ROWS);
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
    fn install_shell_exit_closes_the_popup_and_rechecks_dependencies(cx: &mut TestAppContext) {
        let (view, visual) = page(cx, None);
        let missing = json!({"agent": null, "system": "Linux", "architecture": "aarch64",
            "gaps": [{"requirement": "xwayland", "state": "missing"}], "osRelease": "ID=debian"});
        feed(&view, visual, missing.clone(), None);
        click(visual, "remote-apps-get-started");
        answer(&view, visual, missing.clone());
        reveal(visual);
        let launchpad = view.read_with(visual, |v, _| v.remote_apps.clone().unwrap());
        let rechecks = Rc::new(RefCell::new(Vec::new()));
        let observed = rechecks.clone();
        let _subscription = visual.update(|_, cx| {
            cx.subscribe(&launchpad, move |_, event, _| {
                if let super::LaunchpadEvent::Recheck { replace } = event {
                    observed.borrow_mut().push(*replace);
                }
            })
        });
        let mut previous: Option<gpui::Entity<crate::terminal_view::TerminalView>> = None;
        // An explicit exit code and EOF all mean the shell is finished.
        for code in [Some(0), Some(1), None] {
            answer(&view, visual, missing.clone());
            click(visual, "remote-apps-check-fix-3");
            let shell =
                launchpad.read_with(visual, |v, _| v.terminal.as_ref().unwrap().view.clone());
            if let Some(previous) = previous.take() {
                previous.update(visual, |v, cx| {
                    v.status.exited(Some(0));
                    cx.notify();
                });
                frame(visual);
                assert!(
                    launchpad.read_with(visual, |v, _| v.terminal.is_some()),
                    "an old shell must not close its replacement"
                );
            }
            shell.update(visual, |v, cx| {
                v.status.ready();
                cx.notify();
            });
            visual.run_until_parked();
            launchpad.update(visual, |v, _| {
                v.terminal.as_mut().unwrap().exit.elapse_startup()
            });
            if code.is_none() {
                view.update(visual, |v, cx| v.navigate(crate::Page::Overview, cx));
                frame(visual);
            }
            rechecks.borrow_mut().clear();
            shell.update(visual, |v, cx| {
                v.status.exited(code);
                cx.notify();
            });
            frame(visual);
            assert!(launchpad.read_with(visual, |v, _| v.terminal.is_none()));
            assert!(visual.debug_bounds("remote-apps-terminal").is_none());
            assert_eq!(
                *rechecks.borrow(),
                vec![true],
                "exit must replace any stale dependency read"
            );
            view.update(visual, |v, cx| v.navigate(crate::Page::RemoteApps, cx));
            frame(visual);
            assert!(visual.debug_bounds("remote-apps-terminal").is_none());
            launchpad.read_with(visual, |v, _| {
                assert!(
                    matches!(v.installs.get(&v.server), Some(super::Install::Diagnosing(d))
                    if d.shown == ROWS),
                    "exit must not replay the checklist"
                );
            });
            previous = Some(shell);
        }
    }

    #[gpui::test]
    fn install_shell_keeps_startup_failures_visible_and_closes_after_retry(
        cx: &mut TestAppContext,
    ) {
        let (view, visual) = page(cx, None);
        let launchpad = view.read_with(visual, |v, _| v.remote_apps.clone().unwrap());
        for ready in [false, true] {
            launchpad.update_in(visual, |v, window, cx| {
                v.open_terminal(
                    "Install XWayland".into(),
                    "sudo apt install xwayland".into(),
                    window,
                    cx,
                );
            });
            frame(visual);
            let shell =
                launchpad.read_with(visual, |v, _| v.terminal.as_ref().unwrap().view.clone());
            shell.update(visual, |v, cx| {
                v.status.failed("connection lost");
                cx.notify();
            });
            frame(visual);
            assert!(
                visual.debug_bounds("remote-apps-terminal").is_some(),
                "failure stays retryable"
            );
            if ready {
                shell.update(visual, |v, cx| {
                    v.status.ready();
                    cx.notify();
                });
                visual.run_until_parked();
            }
            shell.update(visual, |v, cx| {
                v.status.exited(Some(1));
                cx.notify();
            });
            frame(visual);
            assert!(
                visual.debug_bounds("remote-apps-terminal").is_some(),
                "startup exit stays visible"
            );
            launchpad.update(visual, |v, _| {
                v.terminal.as_mut().unwrap().exit.elapse_startup()
            });
            shell.update(visual, |_, cx| cx.notify());
            frame(visual);
            assert!(
                visual.debug_bounds("remote-apps-terminal").is_some(),
                "later output must not reconsider a startup exit"
            );
            shell.update(visual, |v, cx| {
                v.status.begin();
                cx.notify();
            });
            visual.run_until_parked();
            shell.update(visual, |v, cx| {
                v.status.ready();
                cx.notify();
            });
            visual.run_until_parked();
            launchpad.update(visual, |v, _| {
                v.terminal.as_mut().unwrap().exit.elapse_startup()
            });
            shell.update(visual, |v, cx| {
                v.status.exited(Some(0));
                cx.notify();
            });
            frame(visual);
            assert!(
                visual.debug_bounds("remote-apps-terminal").is_none(),
                "a used shell closes after retry"
            );
        }
    }

    #[gpui::test]
    fn check_again_keeps_the_checklist_in_place(cx: &mut TestAppContext) {
        let (view, visual) = page(cx, None);
        let dbus = json!({"requirement": "dbus", "state": "missing"});
        let status = json!({"agent": null, "system": "Linux", "architecture": "aarch64",
            "gaps": [dbus], "osRelease": "ID=debian"});
        feed(&view, visual, status.clone(), None);
        click(visual, "remote-apps-get-started");
        answer(&view, visual, status);
        reveal(visual);
        click(visual, "remote-apps-check-again");
        view.update(visual, |v, cx| {
            let launchpad = v.remote_apps.as_ref().unwrap().read(cx);
            assert!(matches!(
                launchpad.installs.get(&launchpad.server),
                Some(super::Install::Diagnosing(d)) if d.shown == ROWS
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
    fn a_failed_check_is_retried_without_try_again(cx: &mut TestAppContext) {
        let (view, visual) = page(cx, None);
        view.update(visual, |v, _| {
            let key = super::status_key(&v.server);
            let generation = v.resources.begin_refresh(&key, false).unwrap();
            v.resources
                .complete(&key, generation, Err("No route to host".into()));
        });
        frame(visual);
        let launchpad = view.read_with(visual, |v, _| v.remote_apps.clone().unwrap());
        assert!(launchpad.read_with(visual, |v, _| matches!(
            v.status(),
            super::api::ServerStatus::Failed(_)
        )));
        let rechecks = Rc::new(RefCell::new(0));
        let observed = rechecks.clone();
        let _subscription = visual.update(|_, cx| {
            cx.subscribe(&launchpad, move |_, event, _| {
                if let super::LaunchpadEvent::Refresh = event {
                    *observed.borrow_mut() += 1;
                }
            })
        });
        visual.executor().advance_clock(super::STATUS_RETRY / 2);
        visual.run_until_parked();
        assert_eq!(*rechecks.borrow(), 0);
        visual.executor().advance_clock(super::STATUS_RETRY);
        visual.run_until_parked();
        assert_eq!(*rechecks.borrow(), 1);
        // A server that answers again stops the retries.
        feed(&view, visual, ready(), None);
        assert!(launchpad.read_with(visual, |v, _| v.status_retry.is_none()));
        visual.executor().advance_clock(super::STATUS_RETRY_MAX);
        visual.run_until_parked();
        assert_eq!(*rechecks.borrow(), 1);
        // An app list that couldn't be read is read again too.
        view.update(visual, |v, _| {
            let key = super::catalogue_key(&v.server);
            let generation = v.resources.begin_refresh(&key, false).unwrap();
            v.resources
                .complete(&key, generation, Err("No route to host".into()));
        });
        frame(visual);
        visual.executor().advance_clock(super::STATUS_RETRY);
        visual.run_until_parked();
        assert_eq!(*rechecks.borrow(), 2);
    }

    #[gpui::test]
    fn reads_that_failed_are_asked_again_once_the_server_reconnects(cx: &mut TestAppContext) {
        let (view, visual) = page(cx, Some((1000., 700.)));
        feed(&view, visual, ready(), None);
        let (work, requests) = std::sync::mpsc::sync_channel(64);
        view.update(visual, |v, cx| {
            v.work = work.into();
            v.window_visible = true;
            let server = v.server.clone();
            v.snapshot["runtime"]["agentTransport"][&server] =
                json!({"status": "connected", "generation": 1});
            v.poll_remote_apps(cx);
            let key = super::catalogue_key(&server);
            v.resources.fence(&key);
            let generation = v.resources.begin_refresh(&key, false).unwrap();
            v.resources
                .complete(&key, generation, Err("No route to host".into()));
            // Still the same connection: the failure waits for its poll.
            requests.try_iter().for_each(drop);
            v.poll_remote_apps(cx);
            assert!(v.resources.get(&key).is_some_and(|r| !r.pending));
            v.snapshot["runtime"]["agentTransport"][&server] =
                json!({"status": "connected", "generation": 2});
            v.poll_remote_apps(cx);
            assert!(v.resources.get(&key).is_some_and(|r| r.pending));
            assert!(
                requests
                    .try_iter()
                    .any(|r| r.method == "remote_apps_catalogue")
            );
        });
    }

    #[gpui::test]
    fn apps_replace_their_placeholders_in_place(cx: &mut TestAppContext) {
        let (view, visual) = page(cx, Some((1000., 700.)));
        feed(&view, visual, ready(), None);
        let placeholders = [
            "remote-app-placeholder-0",
            "remote-app-placeholder-1",
            "remote-app-placeholder-2",
        ]
        .map(|selector| visual.debug_bounds(selector).unwrap());
        let grid = visual.debug_bounds("remote-apps-grid").unwrap();
        assert_eq!(
            placeholders[0].origin.x,
            grid.origin.x + gpui::px(crate::controls::PAGE_PADDING)
        );
        feed(&view, visual, ready(), Some(catalogue()));
        assert!(visual.debug_bounds("remote-app-placeholder-0").is_none());
        let tiles = ["remote-app-0", "remote-app-1", "remote-app-2"]
            .map(|selector| visual.debug_bounds(selector).unwrap());
        assert_eq!(tiles, placeholders);
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

    /// The upsell button's label, when the page shows one.
    fn upsell_label(visual: &mut VisualTestContext) -> Option<&'static str> {
        frame(visual);
        [
            ("remote-apps-upsell: Sign in", "Sign in"),
            ("remote-apps-upsell: Start free trial", "Start free trial"),
            ("remote-apps-upsell: Upgrade to Pro", "Upgrade to Pro"),
            (
                "remote-apps-upsell: Use Pro on this computer",
                "Use Pro on this computer",
            ),
        ]
        .into_iter()
        .find(|(selector, _)| visual.debug_bounds(selector).is_some())
        .map(|(_, label)| label)
    }

    #[gpui::test]
    fn without_pro_the_page_offers_the_next_step(cx: &mut TestAppContext) {
        let (view, visual) = page_without_pro(cx, None);
        // Until the account status loads: the wall, and no upsell to flash.
        assert_eq!(upsell_label(visual), None);
        assert!(visual.debug_bounds("remote-apps-wall").is_some());
        for (status, label) in [
            (json!({"status":"signedOut"}), "Sign in"),
            (
                signed_in(json!({"state":"none","trialUsed":false})),
                "Start free trial",
            ),
            (
                signed_in(json!({"state":"expired","trialUsed":true})),
                "Upgrade to Pro",
            ),
            (
                signed_in(json!({"state":"active","plan":"pro","seat":"free"})),
                "Use Pro on this computer",
            ),
            (
                signed_in(json!({"state":"trial","plan":"trial","seat":"free"})),
                "Upgrade to Pro",
            ),
        ] {
            let (account, _) = account(status.clone());
            use_account(&view, visual, account);
            assert_eq!(upsell_label(visual), Some(label), "{status}");
            assert!(
                visual.debug_bounds("remote-apps-get-started").is_none(),
                "nothing reaches the server without Pro"
            );
        }
        // With Pro the page goes on to the server.
        let (pro, _) = account(signed_in(
            json!({"state":"trial","plan":"trial","seat":"trial"}),
        ));
        use_account(&view, visual, pro);
        assert_eq!(upsell_label(visual), None);
    }

    #[gpui::test]
    fn start_free_trial_asks_the_backend(cx: &mut TestAppContext) {
        let (view, visual) = page_without_pro(cx, None);
        let (free, asked) = account(signed_in(json!({"state":"none","trialUsed":false})));
        use_account(&view, visual, free);
        assert!(visual.debug_bounds("remote-apps-upgrade").is_some());
        click(visual, "remote-apps-upsell: Start free trial");
        assert_eq!(asked.borrow().last(), Some(&"account_start_trial"));
        // The trial's answer arrives before the next step.
        view.update(visual, |v, _| {
            v.account.poll();
        });
        click(visual, "remote-apps-upgrade");
        assert_eq!(asked.borrow().last(), Some(&"account_open_billing"));
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

    /// A server missing sound, which only some apps need, or D-Bus, which
    /// every app needs.
    fn package(required: bool) -> Value {
        let requirement = if required { "dbus" } else { "audio" };
        json!({"gaps": [{"requirement": requirement, "state": "missing"}], "osRelease": "ID=debian"})
    }

    #[gpui::test]
    fn missing_software_is_an_alert_with_a_fix(cx: &mut TestAppContext) {
        let (_, visual) = launchpad_with(package(false), cx);
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
            let key = super::INSTALLED_KEY.to_owned();
            let save = |installed: bool, server: &str| super::Mutation {
                key: super::INSTALLED_KEY.to_owned(),
                method: "remote_apps_installed_save",
                args: json!({"serverId": server, "installed": installed}),
            };
            assert!(v.resources.begin_refresh(&key, false).is_some());
            let server = v.server.clone();
            v.queue_remote_mutation(save(true, &server));
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
            v.queue_remote_mutation(save(false, &server));
            assert_eq!(v.remote_apps_mutations.len(), 1);
            v.resources.complete(&key, generation, Ok(json!([server])));
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
        extra["osRelease"] = json!("ID=slackware\nPRETTY_NAME=\"Slackware 15.0\"");
        let (_, visual) = launchpad_with(extra, cx);
        assert!(visual.debug_bounds("remote-apps-alert-0").is_some());
        assert!(visual.debug_bounds("remote-apps-alert-run-0").is_none());
    }

    #[gpui::test]
    fn nixos_gets_settings_to_copy_instead_of_a_command(cx: &mut TestAppContext) {
        let mut extra = package(false);
        extra["osRelease"] = json!("ID=nixos\nPRETTY_NAME=\"NixOS 26.05 (Yarara)\"");
        let (view, visual) = launchpad_with(extra, cx);
        assert!(visual.debug_bounds("remote-apps-alert-run-0").is_none());
        assert!(visual.debug_bounds("remote-apps-alert-copy-0").is_some());
        view.read_with(visual, |v, cx| {
            let launchpad = v.remote_apps.as_ref().unwrap().read(cx);
            let alerts = launchpad.alerts(&launchpad.status(), &[]);
            assert_eq!(
                alerts[0].0.fix,
                Some(api::Fix::Configure(
                    "services.pipewire = { enable = true; pulse.enable = true; };".into()
                ))
            );
        });
    }

    #[gpui::test]
    fn optional_software_says_only_some_apps_need_it(cx: &mut TestAppContext) {
        let extra = json!({"gaps": [
            {"requirement": "gles", "state": "missing"},
            {"requirement": "audio", "state": "missing"},
        ], "host": {"elevation": "sudo", "updater": null}, "osRelease": "ID=alpine"});
        let (view, visual) = launchpad_with(extra, cx);
        view.read_with(visual, |v, cx| {
            let launchpad = v.remote_apps.as_ref().unwrap().read(cx);
            let alerts = launchpad.alerts(&launchpad.status(), &[]);
            let (install, dismissible) = &alerts[0];
            assert!(
                install
                    .detail
                    .starts_with("Some remote apps need OpenGL ES and a sound server on "),
                "{}",
                install.detail
            );
            // No systemd user services: the sound server is one Needle starts.
            assert_eq!(
                install.fix,
                Some(api::Fix::Command(
                    "sudo apk update && sudo apk add mesa-gles pulseaudio".into()
                ))
            );
            assert!(dismissible);
        });
    }

    #[gpui::test]
    fn required_software_is_named_before_optional(cx: &mut TestAppContext) {
        let extra = json!({"gaps": [
            {"requirement": "audio", "state": "missing"},
            {"requirement": "fonts", "state": "missing"},
        ], "host": {"elevation": "sudo", "updater": null}, "osRelease": "ID=debian"});
        let (view, visual) = launchpad_with(extra, cx);
        view.read_with(visual, |v, cx| {
            let launchpad = v.remote_apps.as_ref().unwrap().read(cx);
            let alerts = launchpad.alerts(&launchpad.status(), &[]);
            let (install, dismissible) = &alerts[0];
            assert!(
                install
                    .detail
                    .starts_with("Remote apps need a font and a sound server on "),
                "{}",
                install.detail
            );
            assert!(!dismissible);
        });
    }

    #[gpui::test]
    fn everything_a_package_fixes_installs_from_one_alert(cx: &mut TestAppContext) {
        let extra = json!({"gaps": [
            {"requirement": "dbus", "state": "missing"},
            {"requirement": "fonts", "state": "missing"},
            {"requirement": "audio", "state": "inactive", "reason": "root"},
        ], "host": {"elevation": "root", "updater": null}, "osRelease": "ID=arch"});
        let (view, visual) = launchpad_with(extra, cx);
        view.read_with(visual, |v, cx| {
            let launchpad = v.remote_apps.as_ref().unwrap().read(cx);
            let alerts = launchpad.alerts(&launchpad.status(), &[]);
            let (install, dismissible) = &alerts[0];
            assert_eq!(install.title, "Software is missing");
            assert!(
                install.detail.contains("D-Bus and a font"),
                "{}",
                install.detail
            );
            assert_eq!(
                install.fix,
                Some(api::Fix::Command(
                    "pacman -Syu --needed dbus ttf-dejavu".into()
                ))
            );
            assert!(!dismissible);
            let (root, dismissible) = &alerts[1];
            assert_eq!(root.title, "No sound for root");
            assert_eq!(root.fix, Some(api::Fix::Refresh));
            assert!(dismissible);
            // The checklist says the same, row by row.
            let checks = launchpad.checks();
            assert_eq!(checks[2].label, "D-Bus is missing");
            assert_eq!(checks[4].label, "No sound for root");
            assert!(checks[4].fix.is_none());
            assert_eq!(checks[5].label, "No fonts are installed");
        });
    }

    #[gpui::test]
    fn apps_dont_start_without_the_software_every_app_needs(cx: &mut TestAppContext) {
        let extra = json!({"gaps": [
            {"requirement": "dbus", "state": "missing"},
            {"requirement": "audio", "state": "missing"},
        ], "host": {"elevation": "sudo", "updater": null}, "osRelease": "ID=debian"});
        let (view, visual) = launchpad_with(extra, cx);
        view.read_with(visual, |v, cx| {
            let launchpad = v.remote_apps.as_ref().unwrap().read(cx);
            let server = launchpad.server_name.clone();
            assert_eq!(
                launchpad.missing_for_launch("Calculator").unwrap_err(),
                format!("Calculator needs D-Bus on {server}. Install it, then try again.")
            );
        });
        // Refusing checks the server again, so trying again after installing works.
        let launchpad = view.read_with(visual, |v, _| v.remote_apps.clone().unwrap());
        let rechecks = Rc::new(RefCell::new(0));
        let observed = rechecks.clone();
        let _subscription = visual.update(|_, cx| {
            cx.subscribe(&launchpad, move |_, event, _| {
                if let super::LaunchpadEvent::Refresh = event {
                    *observed.borrow_mut() += 1;
                }
            })
        });
        launchpad.update(visual, |launchpad, cx| {
            launchpad.launch("org.gnome.Calculator.desktop", None, cx)
        });
        assert_eq!(*rechecks.borrow(), 1);
        // Software only some apps need doesn't stop a launch.
        let extra = json!({"gaps": [{"requirement": "audio", "state": "missing"}]});
        let (view, visual) = launchpad_with(extra, cx);
        view.read_with(visual, |v, cx| {
            let launchpad = v.remote_apps.as_ref().unwrap().read(cx);
            assert!(launchpad.missing_for_launch("Calculator").is_ok());
        });
    }

    #[gpui::test]
    fn required_software_cannot_be_dismissed(cx: &mut TestAppContext) {
        let (_, visual) = launchpad_with(package(true), cx);
        assert!(visual.debug_bounds("remote-apps-alert-0").is_some());
        assert!(visual.debug_bounds("remote-apps-alert-dismiss-0").is_none());
    }

    #[gpui::test]
    fn an_unusable_xwayland_shows_in_the_checklist_until_rechecked(cx: &mut TestAppContext) {
        let (view, visual) = launchpad_with(json!({}), cx);
        for (gap, title, installable) in [
            (
                json!({"requirement": "xwayland", "state": "outdated", "version": "22.1.9"}),
                "XWayland is too old",
                true,
            ),
            (
                json!({"requirement": "xwayland", "state": "unverified"}),
                "XWayland couldn’t be checked",
                false,
            ),
        ] {
            let mut status = ready();
            status["gaps"] = json!([gap]);
            status["osRelease"] = json!("ID=debian");
            answer(&view, visual, status);
            view.read_with(visual, |v, cx| {
                let launchpad = v.remote_apps.as_ref().unwrap().read(cx);
                let status = launchpad.status();
                assert!(launchpad.alerts(&status, &[]).is_empty());
                assert!(launchpad.launch_agent().is_ok());
                let checks = launchpad.checks();
                assert_eq!(checks[3].label, title);
                assert!(checks[3].outcome == super::CheckOutcome::Gap);
                assert_eq!(checks[3].fix.is_some(), installable);
            });
            assert!(visual.debug_bounds("remote-apps-alert-0").is_none());
        }
        answer(&view, visual, ready());
        view.read_with(visual, |v, cx| {
            let launchpad = v.remote_apps.as_ref().unwrap().read(cx);
            assert!(matches!(launchpad.status(), api::ServerStatus::Ready));
            assert!(launchpad.checks()[3].outcome == super::CheckOutcome::Passed);
        });
        assert!(visual.debug_bounds("remote-apps-alert-0").is_none());
    }

    #[gpui::test]
    fn failures_show_two_lines_and_keep_everything_reported(cx: &mut TestAppContext) {
        let reported = "The profile appears to be in use by another Chromium process (3746) on another computer (amd-halo).\nError: Can't open display:\nFailed sending zygote boot message: Broken pipe (32)\nwrite: Broken pipe (32)";
        let failed = |app: &str| api::SessionSummary {
            id: api::SessionId(1),
            server: "server".into(),
            app_id: format!("{app}.desktop"),
            app_name: app.into(),
            state: api::SessionState::Ended {
                ending: Some(api::Ending::Failed(reported.into())),
            },
            notice: None,
        };
        // A server with XWayland: the app's own first two lines, and all of it on request.
        let (view, visual) = launchpad_with(json!({}), cx);
        view.read_with(visual, |v, cx| {
            let launchpad = v.remote_apps.as_ref().unwrap().read(cx);
            let alerts = launchpad.alerts(&launchpad.status(), &[failed("Chromium")]);
            let (alert, _) = &alerts[0];
            assert_eq!(
                alert.detail,
                "The profile appears to be in use by another Chromium process (3746) on another computer (amd-halo).\nError: Can't open display:"
            );
            assert_eq!(alert.output.as_deref(), Some(reported));
            assert!(alert.fix.is_none());
        });
        // Without XWayland, the missing X11 is the reason, and installing it the fix.
        let (view, visual) = launchpad_with(
            json!({"gaps": [{"requirement": "xwayland", "state": "missing"}]}),
            cx,
        );
        view.read_with(visual, |v, cx| {
            let launchpad = v.remote_apps.as_ref().unwrap().read(cx);
            let alerts = launchpad.alerts(&launchpad.status(), &[failed("Chromium")]);
            // Only the app that needed it raises it.
            assert_eq!(alerts.len(), 1);
            let (alert, dismissible) = &alerts[0];
            assert!(dismissible);
            assert!(
                alert.detail.starts_with("Chromium needs X11, and"),
                "{}",
                alert.detail
            );
            assert!(alert.detail.contains("has no XWayland. Install XWayland"), "{}", alert.detail);
            assert!(matches!(&alert.fix, Some(api::Fix::Command(command)) if command.contains("xwayland")));
            assert_eq!(alert.output.as_deref(), Some(reported));
        });
        let (view, visual) = launchpad_with(
            json!({"gaps": [{"requirement": "xwayland", "state": "missing"}]}),
            cx,
        );
        view.update(visual, |v, cx| {
            v.remote_apps.as_ref().unwrap().update(cx, |launchpad, cx| {
                let short = api::SessionSummary {
                    state: api::SessionState::Ended {
                        ending: Some(api::Ending::Failed("kate: not found".into())),
                    },
                    ..failed("Kate")
                };
                let alerts = launchpad.alerts(&launchpad.status(), &[short]);
                // Nothing more to read than the banner shows.
                assert_eq!(alerts[0].0.detail, "kate: not found");
                assert!(alerts[0].0.output.is_none());
                cx.notify();
            });
        });
    }

    #[gpui::test]
    fn alerts_offer_the_full_output_only_when_their_text_is_cut(cx: &mut TestAppContext) {
        let ended = |ending| api::SessionSummary {
            id: api::SessionId(1),
            server: "server".into(),
            app_id: "kate.desktop".into(),
            app_name: "Kate".into(),
            state: api::SessionState::Ended {
                ending: Some(ending),
            },
            notice: None,
        };
        let long = "x".repeat(super::CLAMPED_CHARS + 1);
        let fits = "y".repeat(super::CLAMPED_CHARS);
        let (view, visual) = launchpad_with(json!({}), cx);
        view.read_with(visual, |v, cx| {
            let launchpad = v.remote_apps.as_ref().unwrap().read(cx);
            let alerts = |ending| {
                let mut alerts = launchpad.alerts(&launchpad.status(), &[ended(ending)]);
                alerts.remove(0).0
            };
            // One line too long for two lines of the alert.
            let alert = alerts(api::Ending::Failed(long.clone()));
            assert_eq!(alert.detail, long);
            assert_eq!(alert.output.as_deref(), Some(long.as_str()));
            // One short line is shown whole.
            let alert = alerts(api::Ending::Failed(fits.clone()));
            assert_eq!(alert.detail, fits);
            assert!(alert.output.is_none());
            // The wording "It reported: " counts towards what the alert holds.
            let alert = alerts(api::Ending::NoWindow(Some(fits.clone())));
            assert_eq!(alert.detail, format!("It reported: {fits}"));
            assert_eq!(alert.output.as_deref(), Some(fits.as_str()));
            let alert = alerts(api::Ending::NoWindow(Some("Started in the tray".into())));
            assert!(alert.output.is_none());
        });
    }

    #[gpui::test]
    fn alerts_without_output_are_not_cut(cx: &mut TestAppContext) {
        let (view, visual) = launchpad_with(
            json!({"gaps": [{"requirement": "portal", "state": "missing"}],
                "host": {"elevation": "sudo", "updater": null}, "osRelease": "ID=alpine"}),
            cx,
        );
        view.read_with(visual, |v, cx| {
            let launchpad = v.remote_apps.as_ref().unwrap().read(cx);
            let alerts = launchpad.alerts(&launchpad.status(), &[]);
            // Nothing offers more to read, so the text must show whole.
            assert!(!alerts.is_empty());
            assert!(alerts.iter().all(|(alert, _)| alert.output.is_none()));
        });
    }

    #[gpui::test]
    fn launches_that_end_without_a_window_say_why(cx: &mut TestAppContext) {
        let (view, visual) = launchpad_with(json!({}), cx);
        let session = |id, ending| api::SessionSummary {
            id: api::SessionId(id),
            server: "server".into(),
            app_id: "kate.desktop".into(),
            app_name: "Kate".into(),
            state: api::SessionState::Ended {
                ending: Some(ending),
            },
            notice: None,
        };
        let sessions = [
            session(1, api::Ending::Failed("kate: not found".into())),
            session(2, api::Ending::NoWindow(Some("Started in the tray".into()))),
            session(3, api::Ending::NoWindow(None)),
            session(4, api::Ending::HandedOff),
            api::SessionSummary {
                state: api::SessionState::Background,
                ..session(5, api::Ending::HandedOff)
            },
            session(6, api::Ending::DisplayStopped),
            session(
                7,
                api::Ending::Failed(
                    "[1:2:ERROR:zygote_host_impl_linux.cc(101)] Running as root without --no-sandbox is not supported. See https://crbug.com/638180."
                        .into(),
                ),
            ),
        ];
        view.read_with(visual, |v, cx| {
            let launchpad = v.remote_apps.as_ref().unwrap().read(cx);
            let alerts = launchpad.alerts(&launchpad.status(), &sessions);
            let alert = |id: &str| {
                alerts
                    .iter()
                    .find(|(alert, _)| alert.id == id)
                    .map(|(alert, dismissible)| {
                        assert!(dismissible);
                        assert!(alert.fix.is_none());
                        (alert.severity, alert.title.as_str(), alert.detail.as_str())
                    })
                    .unwrap()
            };
            assert_eq!(
                alert("session:1"),
                (
                    api::Severity::Error,
                    "Couldn’t open Kate",
                    "kate: not found"
                )
            );
            assert_eq!(
                alert("session:2"),
                (
                    api::Severity::Warning,
                    "Kate closed without opening a window",
                    "It reported: Started in the tray"
                )
            );
            let (severity, title, detail) = alert("session:3");
            assert_eq!(
                (severity, title),
                (
                    api::Severity::Warning,
                    "Kate closed without opening a window"
                )
            );
            assert!(detail.contains("system tray"), "{detail}");
            // Its windows went with the display: it closed, nothing runs.
            let (severity, title, detail) = alert("session:6");
            assert_eq!((severity, title), (api::Severity::Warning, "Kate closed"));
            assert!(
                detail.ends_with("closed with it. Open it again."),
                "{detail}"
            );
            // A refusal it is known for says what to do instead.
            let (_, _, detail) = alert("session:7");
            assert!(detail.starts_with("Kate won’t run as root"), "{detail}");
            assert!(
                detail.ends_with("as a regular user to open it."),
                "{detail}"
            );
            // A hand-off shows its window elsewhere and needs no alert.
            assert!(!alerts.iter().any(|(alert, _)| alert.id == "session:4"));
            let background = alerts
                .iter()
                .find(|(alert, _)| alert.id == "background:5")
                .map(|(alert, _)| alert)
                .unwrap();
            assert_eq!(
                (background.severity, background.title.as_str()),
                (api::Severity::Info, "Kate is running without a window")
            );
            assert_eq!(background.fix, Some(api::Fix::Quit(api::SessionId(5))));
            assert!(alerts.iter().all(|(alert, _)| alert.id != "session:5"));
            assert_eq!(alerts.last().unwrap().0.severity, api::Severity::Info);
        });
    }

    #[gpui::test]
    fn running_apps_say_where_their_file_dialogs_open(cx: &mut TestAppContext) {
        let (view, visual) = launchpad_with(json!({}), cx);
        let session = |id, state, notice| api::SessionSummary {
            id: api::SessionId(id),
            server: "server".into(),
            app_id: "chromium_chromium.desktop".into(),
            app_name: "Chromium".into(),
            state,
            notice: Some(notice),
        };
        let sessions = [
            session(
                1,
                api::SessionState::Running,
                api::Notice::DialogsOnServerScreen,
            ),
            session(
                2,
                api::SessionState::Background,
                api::Notice::DialogsOnOtherMac,
            ),
            session(
                3,
                api::SessionState::Ended {
                    ending: Some(api::Ending::HandedOff),
                },
                api::Notice::DialogsOnServerScreen,
            ),
        ];
        view.read_with(visual, |v, cx| {
            let launchpad = v.remote_apps.as_ref().unwrap().read(cx);
            let alerts = launchpad.alerts(&launchpad.status(), &sessions);
            let alert = |id: &str| {
                alerts
                    .iter()
                    .find(|(alert, _)| alert.id == id)
                    .map(|(alert, dismissible)| {
                        assert!(dismissible);
                        assert!(alert.fix.is_none());
                        (alert.severity, alert.title.as_str(), alert.detail.as_str())
                    })
            };
            assert_eq!(
                alert("dialogs:1"),
                Some((
                    api::Severity::Info,
                    "File dialogs from Chromium open on the server’s own screen",
                    "Someone is signed in there. Chromium waits until a dialog there is closed."
                ))
            );
            assert_eq!(
                alert("dialogs:2"),
                Some((
                    api::Severity::Info,
                    "File dialogs from Chromium open on another Mac",
                    "That Mac uses this server with the same account. Chromium waits until a dialog there is closed."
                ))
            );
            // The background note stays beside it.
            assert!(alerts.iter().any(|(alert, _)| alert.id == "background:2"));
            // An ended row has neither its notice nor a hand-off alert.
            assert_eq!(alert("dialogs:3"), None);
            assert!(!alerts.iter().any(|(alert, _)| alert.id == "session:3"));
        });
    }

    #[gpui::test]
    fn xwayland_is_offered_never_required(cx: &mut TestAppContext) {
        let mut status = ready();
        status["gaps"] = json!([{"requirement": "xwayland", "state": "missing"}]);
        let (view, visual) = launchpad_with(status, cx);
        view.read_with(visual, |v, cx| {
            let pad = v.remote_apps.as_ref().unwrap().read(cx);
            assert!(pad.launch_agent().is_ok());
            let row = &pad.checks()[3];
            assert_eq!(row.label, "XWayland is missing");
            assert!(row.outcome == super::CheckOutcome::Gap);
            assert!(row.fix.is_some());
            // The page raises no alert for it; an app that needed it does.
            assert!(pad.alerts(&pad.status(), &[]).is_empty());
        });
        click(visual, "remote-apps-preferences");
        assert!(
            visual
                .debug_bounds("remote-apps-preferences-panel")
                .is_some()
        );
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
