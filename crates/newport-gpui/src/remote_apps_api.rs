//! Plain data for the Remote apps page and its session engine.
pub use newport_domain::server_os::{PackageManager, PackageNames, ServerOs, install_command};
use std::sync::Arc;

/// Whether a server can run remote apps yet.
#[derive(Clone, Debug, PartialEq)]
pub enum ServerStatus {
    /// Checking the server for the agent.
    Checking,
    /// The streaming agent is not installed.
    NeedsInstall,
    /// The agent is installed but the server lacks programs it needs.
    MissingPackages(Vec<MissingPackage>),
    /// Applications can be listed and launched.
    Ready,
    /// The last check failed; the user may retry.
    Failed(String),
}

/// A server program the agent needs, with an install command when known.
#[derive(Clone, Debug, PartialEq)]
pub struct MissingPackage {
    /// Stable: `dbus`, `xvfb` or `audio`.
    pub id: String,
    pub name: String,
    /// Why it is needed, for example "X11-only applications".
    pub purpose: String,
    /// False when only some applications need it.
    pub required: bool,
    /// The agent can't be installed without it.
    pub needed_to_install: bool,
    /// Its name in each package manager; the page builds the install command
    /// for the server's distribution.
    pub packages: PackageNames,
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
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    Warning,
    Error,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Fix {
    /// A command the user runs in a terminal on the server.
    Command(String),
    /// Install packages with the server's package manager.
    Install(PackageNames),
    /// Check the server again, after the user fixed something outside Newport.
    Refresh,
    /// Replace the server's agent with the one this build carries.
    UpdateAgent,
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

/// A program started directly, for applications without a desktop entry or
/// that need particular arguments.
#[derive(Clone, Debug, PartialEq)]
pub struct CommandRequest {
    /// Identifies the command's sessions, like an application id.
    pub id: String,
    /// Shown for the session, for example "Firefox (clean profile)".
    pub name: String,
    /// The program and its arguments, not interpreted by a shell. The program
    /// may be a name on the server's `PATH`.
    pub argv: Vec<String>,
    /// Environment variables added for the program.
    pub env: Vec<(String, String)>,
    pub display: DisplayBackend,
}

/// Which display server a command runs under.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DisplayBackend {
    /// The private X11 display, as for catalogue applications.
    #[default]
    Auto,
    X11,
    Wayland,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SessionId(pub u64);

#[derive(Clone, Debug, PartialEq)]
pub enum SessionState {
    Starting,
    Running,
    Ended { error: Option<String> },
}

#[derive(Clone, Debug, PartialEq)]
pub struct SessionSummary {
    pub id: SessionId,
    pub server: String,
    /// The catalogue id of the launched application.
    pub app_id: String,
    pub app_name: String,
    pub state: SessionState,
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
