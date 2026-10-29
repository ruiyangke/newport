//! Plain data for the Remote apps page and its session engine.
pub use newport_domain::requirements::{Finding, Gap, Host, Remedy, Requirement};
pub use newport_domain::server_os::ServerOs;
use std::sync::Arc;

/// Whether a server can run remote apps yet.
#[derive(Clone, Debug, PartialEq)]
pub enum ServerStatus {
    /// Checking the server for the agent.
    Checking,
    /// The streaming agent is not installed.
    NeedsInstall,
    /// The agent is installed but the server falls short of what apps need.
    Gaps(Vec<Gap>),
    /// Applications can be listed and launched.
    Ready,
    /// The last check failed; the user may retry.
    Failed(String),
}

/// A problem the page shows above the applications, with a way to fix it.
#[derive(Clone, Debug, PartialEq)]
pub struct Alert {
    /// Stable while the problem lasts, so a dismissal sticks.
    pub id: String,
    pub severity: Severity,
    /// What is wrong, for example "Sound is unavailable".
    pub title: String,
    /// What to do about it.
    pub detail: String,
    pub fix: Option<Fix>,
    /// Everything the app reported, when the detail shows only part of it.
    pub output: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    Info,
    Warning,
    Error,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Fix {
    /// A command the user runs in a terminal on the server.
    Command(String),
    /// NixOS settings to add to the system configuration.
    Configure(String),
    /// Check the server again, after the user fixed something outside Newport.
    Refresh,
    /// Replace the server's agent with the one this build carries.
    UpdateAgent,
    /// An account step: sign in, see plans, or move a Pro seat here.
    Account(newport_client::account::Action),
    /// Quit a session that is still running.
    Quit(SessionId),
}

/// An installed application on the server.
#[derive(Clone, Debug, PartialEq)]
pub struct RemoteApp {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    /// PNG bytes from the agent's catalogue.
    pub icon_png: Option<Arc<[u8]>>,
    pub actions: Vec<AppAction>,
}

/// An application action such as "New Private Window".
#[derive(Clone, Debug, PartialEq)]
pub struct AppAction {
    pub id: String,
    pub name: String,
}

/// The application catalogue for one server.
#[derive(Clone, Debug, PartialEq)]
pub enum Catalogue {
    Loading,
    Ready(Vec<RemoteApp>),
    Failed(String),
}

#[derive(Clone, Debug, PartialEq)]
pub struct LaunchRequest {
    pub app_id: String,
    /// The application's name, shown for its session.
    pub name: String,
    /// A desktop action such as `new-private-window`.
    pub action: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SessionId(pub u64);

#[derive(Clone, Debug, PartialEq)]
pub enum SessionState {
    Starting,
    /// It still runs, but opened no window in time. A window may still open.
    Background,
    Running,
    Stopping,
    Uncertain {
        message: String,
    },
    /// `None` when it ended cleanly.
    Ended {
        ending: Option<Ending>,
    },
}

/// How a session ended, when there is something to say about it. Only builds
/// that stream construct these.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(not(newport_remote_apps), allow(dead_code))]
pub enum Ending {
    /// It never opened: the license, the server or the agent refused, with
    /// the account step that fixes it, if any.
    Refused {
        message: String,
        action: Option<newport_client::account::Action>,
    },
    /// Its original connection or display can no longer be controlled. This
    /// does not establish that the remote process exited.
    Unavailable(String),
    /// The launch failed after the server accepted it, with what it reported.
    Failed(String),
    /// It ended without opening a window, with the end of what it reported.
    NoWindow(Option<String>),
    /// An instance of the app that was already running took the launch.
    HandedOff,
    /// Its display stopped on the server, and its windows closed with it.
    DisplayStopped,
}

/// Something to say about a running session that needs no action.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(not(newport_remote_apps), allow(dead_code))]
pub enum Notice {
    /// Its file dialogs open on the server's own screen, where someone is
    /// signed in.
    DialogsOnServerScreen,
    /// Its file dialogs open on another Mac using this server.
    DialogsOnOtherMac,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SessionSummary {
    pub id: SessionId,
    pub server: String,
    /// The catalogue id of the launched application.
    pub app_id: String,
    pub app_name: String,
    pub state: SessionState,
    /// Only while the session is active.
    pub notice: Option<Notice>,
}

/// Codec preference chosen on the page, mapped to the streaming modes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CodecPreference {
    /// Balance processing and bandwidth automatically.
    #[default]
    Auto,
    /// Exact pixels, more bandwidth.
    Lossless,
    /// Least processing, most bandwidth.
    Fast,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StreamingSettings {
    /// Frames-per-second goal: 30 or 60.
    pub fps_goal: u32,
    pub codec: CodecPreference,
    /// Play sound from remote applications.
    pub audio: bool,
    /// Show the diagnostic overlay in remote windows.
    pub hud: bool,
}
impl Default for StreamingSettings {
    fn default() -> Self {
        Self {
            fps_goal: 60,
            codec: CodecPreference::Auto,
            audio: true,
            hud: false,
        }
    }
}

impl SessionState {
    pub fn is_active(&self) -> bool {
        !matches!(self, Self::Ended { .. })
    }
}
