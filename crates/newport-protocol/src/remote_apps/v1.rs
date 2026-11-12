//! The per-window wire contract between the client and the agent: protocol 1.
use crate::remote_apps::Geometry;
use serde::{Deserialize, Serialize};

pub const VERSION: u16 = 1;
/// The largest Opus packet, as RFC 6716 bounds it.
pub const MAX_AUDIO_BYTES: usize = 1275;
/// Microseconds relative to a shared host-session monotonic epoch. For video,
/// bounds cover the tile reads; for audio, the estimated end of the PCM block.
/// Negative times are valid when capture predates the session epoch.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MediaTime {
    pub earliest_us: i64,
    pub latest_us: i64,
}
impl MediaTime {
    pub fn validate(&self) -> Result<(), String> {
        if self
            .latest_us
            .checked_sub(self.earliest_us)
            .is_none_or(|span| span < 0)
        {
            return Err("Invalid media capture interval".into());
        }
        Ok(())
    }
}
fn is_zero(value: &u64) -> bool {
    *value == 0
}
fn is_false(value: &bool) -> bool {
    !value
}
/// How far past each edge of its surface a held pointer may be, in surface
/// pixels.
pub const HELD_POINTER_REACH: i64 = 1 << 15;
/// `Ended` reason: another computer took the stable display over, and
/// `computer` names it when it gave a name.
pub const ENDED_TAKEN_OVER: &str = "taken-over";
/// The client sends `Heartbeat` this often; the agent ends a session it
/// hasn't heard from for [`HEARTBEAT_TIMEOUT`], such as after the network
/// dropped without closing the connection.
pub const HEARTBEAT_INTERVAL: std::time::Duration = std::time::Duration::from_secs(10);
pub const HEARTBEAT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);
pub const MAX_WINDOWS: usize = 64;
/// Declared BGRA surface bytes of all of a session's windows together. The
/// Wayland display holds a window that would exceed it, unannounced or at its
/// previous size, until other windows shrink or close.
pub const MAX_SURFACE_BYTES: usize = 1024 * 1024 * 1024;
pub const MAX_PENDING_OPERATIONS: usize = 64;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct WindowId {
    pub id: u64,
    pub generation: u64,
}
impl WindowId {
    pub fn validate(self) -> Result<(), String> {
        if self.id == 0 || self.generation == 0 {
            return Err("Invalid window identity".into());
        }
        Ok(())
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Scale {
    pub numerator: u16,
    pub denominator: u16,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Surface {
    pub generation: u64,
    pub logical_width: u32,
    pub logical_height: u32,
    pub pixel_width: u32,
    pub pixel_height: u32,
    pub scale: Scale,
}
impl Surface {
    pub fn geometry(self) -> Result<Geometry, String> {
        let Scale {
            numerator,
            denominator,
        } = self.scale;
        if self.generation == 0
            || numerator == 0
            || denominator == 0
            || numerator > 64
            || denominator > 64
            || numerator as u32 > denominator as u32 * 4
        {
            return Err("Invalid surface generation or scale".into());
        }
        for (logical, pixel) in [
            (self.logical_width, self.pixel_width),
            (self.logical_height, self.pixel_height),
        ] {
            if !(1..=4096).contains(&logical)
                || (logical as u64 * numerator as u64).div_ceil(denominator as u64) != pixel as u64
            {
                return Err("Inconsistent logical and pixel geometry".into());
            }
        }
        Geometry::surface(self.pixel_width as usize, self.pixel_height as usize)
    }
    pub fn bytes(self) -> Result<usize, String> {
        let g = self.geometry()?;
        Ok(g.width * g.height * 4)
    }
    /// Whether a held pointer at `x`, `y` is within [`HELD_POINTER_REACH`].
    pub fn reaches(self, x: i32, y: i32) -> bool {
        let reach = |value: i32, size: u32| {
            (-HELD_POINTER_REACH..i64::from(size) + HELD_POINTER_REACH).contains(&i64::from(value))
        };
        reach(x, self.pixel_width) && reach(y, self.pixel_height)
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WindowKind {
    Normal,
    Dialog,
    Menu,
    Tooltip,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Decoration {
    Client,
    Server,
    None,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Actions {
    pub close: bool,
    pub minimize: bool,
    pub maximize: bool,
    pub resize: bool,
    #[serde(default)]
    pub fullscreen: bool,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Constraints {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub physical: Option<crate::remote_apps::sizing::PixelConstraints>,
    pub min: Option<[u32; 2]>,
    pub max: Option<[u32; 2]>,
}
impl Constraints {
    /// Validate requested logical geometry using physical hints when available.
    /// Logical projections describe the metadata's old scale and must not veto
    /// an otherwise valid resize at a new scale.
    pub fn validate_resize(self, size: [u32; 2], scale: Scale) -> Result<(), String> {
        self.validate()?;
        if size.iter().any(|n| !(1..=4096).contains(n))
            || scale.numerator == 0
            || scale.denominator == 0
            || scale.numerator > 64
            || scale.denominator > 64
            || u32::from(scale.numerator) > u32::from(scale.denominator) * 4
        {
            return Err("Invalid requested geometry".into());
        }
        let pixels =
            size.map(|n| (n * u32::from(scale.numerator)).div_ceil(u32::from(scale.denominator)));
        if pixels.iter().any(|n| *n > 4096) {
            return Err("Requested pixels exceed surface limit".into());
        }
        if let Some(physical) = self.physical {
            if !physical.accepts(pixels) {
                return Err("Resize violates physical size hints".into());
            }
        } else if (0..2).any(|i| {
            self.min.is_some_and(|lo| size[i] < lo[i]) || self.max.is_some_and(|hi| size[i] > hi[i])
        }) {
            return Err("Resize violates window constraints".into());
        }
        Ok(())
    }
    pub fn validate(self) -> Result<(), String> {
        if let Some(physical) = self.physical {
            physical.validate()?;
        }
        if [self.min, self.max]
            .into_iter()
            .flatten()
            .flatten()
            .any(|n| !(1..=4096).contains(&n))
        {
            return Err("Invalid window size constraint".into());
        }
        if let (Some(min), Some(max)) = (self.min, self.max)
            && (min[0] > max[0] || min[1] > max[1])
        {
            return Err("Inverted window constraints".into());
        }
        Ok(())
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Window {
    pub key: WindowId,
    pub parent: Option<WindowId>,
    pub kind: WindowKind,
    pub modal: bool,
    /// Logical coordinates relative to parent for popups, session space otherwise.
    pub position: [i32; 2],
    pub title: String,
    pub decoration: Decoration,
    pub actions: Actions,
    pub constraints: Constraints,
    #[serde(default)]
    pub minimized: bool,
    #[serde(default)]
    pub maximized: bool,
    #[serde(default)]
    pub fullscreen: bool,
    /// Which app owns the window.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::remote_apps::window_application::hint"
    )]
    pub application: Option<crate::remote_apps::window_application::WindowApplication>,
}
impl Window {
    /// A fullscreen window takes the size of the client's viewport, whatever
    /// its constraints say.
    pub fn fullscreen_viewport(&self) -> bool {
        self.fullscreen
            && !self.minimized
            && matches!(self.kind, WindowKind::Normal | WindowKind::Dialog)
    }
    pub fn resize_constraints(&self) -> Constraints {
        if self.fullscreen_viewport() {
            Constraints::default()
        } else {
            self.constraints
        }
    }
    pub fn validate(&self) -> Result<(), String> {
        self.key.validate()?;
        if let Some(parent) = self.parent {
            parent.validate()?;
            if parent == self.key {
                return Err("Window cannot parent itself".into());
            }
        }
        if self.title.len() > 4096 || self.position.iter().any(|p| !(-32768..=32768).contains(p)) {
            return Err("Invalid window metadata".into());
        }
        if let Some(application) = &self.application {
            application.validate()?;
        }
        if (self.modal || matches!(self.kind, WindowKind::Menu | WindowKind::Tooltip))
            && self.parent.is_none()
        {
            return Err("Transient window requires a parent".into());
        }
        if matches!(self.kind, WindowKind::Menu | WindowKind::Tooltip)
            && (self.decoration != Decoration::None || self.modal)
        {
            return Err("Invalid popup decoration/modality".into());
        }
        self.constraints.validate()
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerMessage {
    ApplicationLifecycle(crate::remote_apps::session_application_lifecycle::Update),
    SessionApplications {
        session: u64,
        snapshot: crate::remote_apps::session_applications::Snapshot,
    },
    SessionActivity {
        session: u64,
        activity: crate::remote_apps::session_activity::Snapshot,
    },
    LaunchResult {
        session: u64,
        request: u64,
        outcome: crate::remote_apps::session_launch::Outcome,
    },
    Audio {
        session: u64,
        sequence: u64,
        sample_time: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        captured: Option<MediaTime>,
        #[serde(default, skip_serializing_if = "is_false")]
        hole: bool,
        /// The stream this packet belongs to, numbered from one.
        #[serde(default, skip_serializing_if = "is_zero")]
        epoch: u64,
        /// One Opus packet.
        data: bytes::Bytes,
    },
    /// Whether the session's audio plays.
    AudioState {
        session: u64,
        state: crate::remote_apps::audio::State,
    },
    ReverseFiles {
        session: u64,
        message: crate::remote_apps::reverse_files::Server,
    },
    ReverseDrag {
        session: u64,
        message: crate::remote_apps::reverse_drag::Server,
    },
    Drag {
        session: u64,
        message: crate::remote_apps::drag::Server,
    },
    FileUpload {
        session: u64,
        message: crate::remote_apps::file_upload::Server,
    },
    TextInput {
        session: u64,
        message: crate::remote_apps::text_input::Server,
    },
    /// What this session has.
    Welcome {
        session: u64,
        version: u16,
        #[serde(default)]
        features: crate::remote_apps::features::Features,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        performance: Option<crate::remote_apps::performance::Settings>,
    },
    WindowCreated {
        session: u64,
        window: Window,
        surface: Surface,
        /// The window was open on the display before this viewer: it moved
        /// here when this viewer took the display over, and takes no focus.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        moved: bool,
    },
    WindowUpdated {
        session: u64,
        window: Window,
    },
    SurfaceChanged {
        session: u64,
        window: WindowId,
        surface: Surface,
    },
    Frame {
        session: u64,
        window: WindowId,
        surface: u64,
        sequence: u64,
        keyframe: bool,
        tiles: Vec<crate::remote_apps::tile::Tile>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        captured: Option<MediaTime>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        diagnostics: Option<FrameDiagnostics>,
    },
    WindowAction {
        session: u64,
        window: WindowId,
        action: Operation,
    },
    /// The cursor a window wants while the pointer is over it.
    Cursor {
        session: u64,
        window: WindowId,
        cursor: Cursor,
    },
    Error {
        session: u64,
        message: String,
    },
    WindowDestroyed {
        session: u64,
        window: WindowId,
    },
    OperationResult {
        session: u64,
        request: u64,
        window: WindowId,
        applied: bool,
        message: Option<String>,
    },
    Closed {
        session: u64,
    },
    /// Sent instead of Welcome when the session can't start, such as a
    /// missing or invalid Newport Pro license. `reason` is a stable code;
    /// `message` is worded for people. `display` names the display when it
    /// refused the relay.
    Rejected {
        reason: String,
        message: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        display: Option<crate::display::attachment::DisplayIdentity>,
    },
    /// The agent ended a running session, such as when its license lease ran
    /// out without a renewal. Followed by the session closing.
    Ended {
        session: u64,
        reason: String,
        message: String,
        /// The computer showing the display now, after [`ENDED_TAKEN_OVER`].
        #[serde(default, skip_serializing_if = "Option::is_none")]
        computer: Option<String>,
        /// That computer's Newport session, which tells computers apart as
        /// their names needn't.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        computer_session: Option<String>,
    },
}

/// A Newport Pro license token. Its debug form is redacted so a trace that
/// prints protocol messages never reveals it; it serializes as the plain token.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct LicenseToken(pub String);

impl std::fmt::Debug for LicenseToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("LicenseToken(..)")
    }
}
impl ServerMessage {
    /// The bytes of image or sound data it carries.
    pub fn data_len(&self) -> usize {
        match self {
            Self::Frame { tiles, .. } => tiles.iter().map(|tile| tile.data.len()).sum(),
            Self::Audio { data, .. } => data.len(),
            _ => 0,
        }
    }
    pub fn session(&self) -> u64 {
        match self {
            Self::ApplicationLifecycle(update) => update.session,
            Self::Audio { session, .. }
            | Self::AudioState { session, .. }
            | Self::SessionActivity { session, .. }
            | Self::SessionApplications { session, .. }
            | Self::LaunchResult { session, .. }
            | Self::ReverseFiles { session, .. }
            | Self::ReverseDrag { session, .. }
            | Self::Drag { session, .. }
            | Self::FileUpload { session, .. }
            | Self::TextInput { session, .. }
            | Self::Welcome { session, .. }
            | Self::WindowCreated { session, .. }
            | Self::WindowUpdated { session, .. }
            | Self::SurfaceChanged { session, .. }
            | Self::Frame { session, .. }
            | Self::WindowAction { session, .. }
            | Self::Cursor { session, .. }
            | Self::Error { session, .. }
            | Self::WindowDestroyed { session, .. }
            | Self::OperationResult { session, .. }
            | Self::Ended { session, .. }
            | Self::Closed { session, .. } => *session,
            // Sent before any session exists.
            Self::Rejected { .. } => 0,
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum Operation {
    /// Host-to-client activation; preserves maximize/fullscreen.
    Activate,
    /// Host-to-client request to start a local native window drag.
    Move,
    Close,
    Minimize,
    Maximize,
    Restore,
    Fullscreen {
        enabled: bool,
    },
    /// Host-to-client request to show the native
    /// window menu at a point in logical window coordinates.
    ShowWindowMenu {
        x: i32,
        y: i32,
    },
    /// Host-to-client request to start a local native
    /// resize from an edge or corner during the current primary press.
    StartResize {
        edge: ResizeEdge,
    },
    Resize {
        logical_width: u32,
        logical_height: u32,
        scale: Scale,
    },
    /// Acceptance precedes the application's
    /// configure/commit and any resulting SurfaceChanged notification.
    SetDisplay {
        profile: crate::remote_apps::display::DisplayProfile,
    },
    /// Only updates popup constraints; it neither
    /// resizes the toplevel nor changes its rendering scale.
    SetPopupBounds {
        bounds: crate::remote_apps::display::PopupBounds,
    },
}
/// The edge or corner a native resize moves.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResizeEdge {
    Top,
    Bottom,
    Left,
    Right,
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}
impl ResizeEdge {
    /// Which sides move: `[left, top, right, bottom]`.
    pub fn sides(self) -> [bool; 4] {
        match self {
            Self::Top => [false, true, false, false],
            Self::Bottom => [false, false, false, true],
            Self::Left => [true, false, false, false],
            Self::Right => [false, false, true, false],
            Self::TopLeft => [true, true, false, false],
            Self::TopRight => [false, true, true, false],
            Self::BottomLeft => [true, false, false, true],
            Self::BottomRight => [false, false, true, true],
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientMessage {
    ApplicationPresented(crate::remote_apps::session_application_lifecycle::Presented),
    /// Stop only the owned group for this launch request, never the display.
    StopApplication {
        session: u64,
        request: u64,
    },
    Launch {
        session: u64,
        request: u64,
        target: crate::remote_apps::session_launch::Target,
    },
    /// The client is still there (see [`HEARTBEAT_INTERVAL`]).
    Heartbeat {
        session: u64,
    },
    ReverseFiles {
        session: u64,
        message: crate::remote_apps::reverse_files::Client,
    },
    /// Optional reverse-drag viewport exit, before a host-assigned visit exists.
    ReverseDragExit {
        session: u64,
        gesture: u64,
        window: WindowId,
        surface: u64,
    },
    ReverseDrag {
        session: u64,
        message: crate::remote_apps::reverse_drag::Client,
    },
    Drag {
        session: u64,
        message: crate::remote_apps::drag::Client,
    },
    FileUpload {
        session: u64,
        message: crate::remote_apps::file_upload::Client,
    },
    TextInput {
        session: u64,
        message: crate::remote_apps::text_input::Client,
    },
    Hello {
        versions: Vec<u16>,
        /// What the client will do in this session.
        #[serde(default)]
        features: crate::remote_apps::features::Features,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        performance: Option<crate::remote_apps::performance::Settings>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        display: Option<crate::remote_apps::display::DisplayProfile>,
        /// The Newport Pro license the agent checks before Welcome.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        license: Option<LicenseToken>,
    },
    /// A renewed license for a running session; the agent moves the session's
    /// deadline if it verifies and belongs to the same account and computer.
    License {
        session: u64,
        token: LicenseToken,
    },
    Request {
        session: u64,
        request: u64,
        window: WindowId,
        operation: Operation,
    },
    Ack {
        session: u64,
        window: WindowId,
        surface: u64,
        sequence: u64,
    },
    Input {
        session: u64,
        window: WindowId,
        surface: u64,
        input: Input,
    },
    Quit {
        session: u64,
    },
}

/// The Linux modifiers held during an event, after the presenter maps the
/// Mac's. The agent presses or releases modifier keys to match them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Modifiers {
    #[serde(default, skip_serializing_if = "is_false")]
    pub shift: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub control: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub alt: bool,
    #[serde(default, skip_serializing_if = "is_false", rename = "super")]
    pub logo: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub caps_lock: bool,
}

impl Modifiers {
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Button {
    Left,
    Middle,
    Right,
    Back,
    Forward,
}

/// Where a scroll is in its gesture: a mouse wheel notch, fingers on a
/// trackpad, or the coast after they lift.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScrollPhase {
    Wheel,
    Begin,
    Update,
    End,
    Momentum,
    MomentumEnd,
}

/// A window's cursor: hidden, or one of the shapes `wp_cursor_shape_device_v1`
/// names, which are the CSS cursor names.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Cursor {
    Hidden,
    Shape { shape: CursorShape },
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CursorShape {
    #[default]
    Default,
    ContextMenu,
    Help,
    Pointer,
    Progress,
    Wait,
    Cell,
    Crosshair,
    Text,
    VerticalText,
    Alias,
    Copy,
    Move,
    NoDrop,
    NotAllowed,
    Grab,
    Grabbing,
    EResize,
    NResize,
    NeResize,
    NwResize,
    SResize,
    SeResize,
    SwResize,
    WResize,
    EwResize,
    NsResize,
    NeswResize,
    NwseResize,
    ColResize,
    RowResize,
    AllScroll,
    ZoomIn,
    ZoomOut,
}

/// The highest Linux evdev key code (`KEY_MAX`).
pub const MAX_KEY_CODE: u16 = 0x2ff;
/// The longest scroll delta, in surface pixels.
pub const MAX_SCROLL_PIXELS: f64 = 10_000.0;
/// The most wheel notches one scroll carries, in 1/120 steps.
pub const MAX_SCROLL_DISCRETE: i32 = 100 * 120;

/// Pointer coordinates and scroll deltas are target-surface pixels, never
/// root-display pixels.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Input {
    Focus,
    /// The window stopped being the key window.
    Blur,
    /// Motion; it also means the pointer is in the window.
    Pointer {
        x: u32,
        y: u32,
        #[serde(default, skip_serializing_if = "Modifiers::is_empty")]
        modifiers: Modifiers,
    },
    /// The pointer while a press in this window is held; it may lie up to
    /// [`HELD_POINTER_REACH`] outside the surface, as under its implicit
    /// grab on a Linux desktop.
    HeldPointer {
        x: i32,
        y: i32,
        #[serde(default, skip_serializing_if = "Modifiers::is_empty")]
        modifiers: Modifiers,
    },
    /// The pointer left the window. The next `Pointer` enters it again.
    Leave,
    Button {
        button: Button,
        down: bool,
        #[serde(default, skip_serializing_if = "Modifiers::is_empty")]
        modifiers: Modifiers,
    },
    /// Positive `dy` scrolls down. `discrete` is wheel notches in 1/120
    /// steps, only for mouse wheels.
    Scroll {
        /// Position and scrolling are delivered in one pointer frame.
        position: [u32; 2],
        dx: f64,
        dy: f64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        discrete: Option<[i32; 2]>,
        phase: ScrollPhase,
        #[serde(default, skip_serializing_if = "Modifiers::is_empty")]
        modifiers: Modifiers,
    },
    /// One key going down or up. `key` is what it means on the client's
    /// keyboard, a character or a name; `code` is the physical key as a Linux evdev code,
    /// which pairs an up with its down.
    Key {
        key: String,
        code: u16,
        down: bool,
        #[serde(default, skip_serializing_if = "is_false")]
        repeat: bool,
        #[serde(default, skip_serializing_if = "Modifiers::is_empty")]
        modifiers: Modifiers,
    },
}
impl Input {
    /// Rejects values no presenter sends: a malformed key or scroll.
    pub fn validate(&self) -> Result<(), String> {
        match self {
            Self::Key { key, code, .. } => {
                if key.is_empty() || key.len() > 64 || key.contains('\0') {
                    return Err("Invalid key".into());
                }
                if *code > MAX_KEY_CODE {
                    return Err("Invalid key code".into());
                }
            }
            Self::Scroll {
                dx, dy, discrete, ..
            } => {
                if [dx, dy]
                    .iter()
                    .any(|delta| !delta.is_finite() || delta.abs() > MAX_SCROLL_PIXELS)
                {
                    return Err("Invalid scroll delta".into());
                }
                if discrete.is_some_and(|steps| {
                    steps
                        .iter()
                        .any(|step| step.unsigned_abs() > MAX_SCROLL_DISCRETE as u32)
                }) {
                    return Err("Invalid scroll steps".into());
                }
            }
            _ => (),
        }
        Ok(())
    }
    /// The modifiers this event carries.
    pub fn modifiers(&self) -> Option<Modifiers> {
        match self {
            Self::Pointer { modifiers, .. }
            | Self::HeldPointer { modifiers, .. }
            | Self::Button { modifiers, .. }
            | Self::Scroll { modifiers, .. }
            | Self::Key { modifiers, .. } => Some(*modifiers),
            Self::Focus | Self::Blur | Self::Leave => None,
        }
    }
    /// A key or button going up. These skip the surface and modal checks so a
    /// press is always cleaned up.
    pub fn is_release(&self) -> bool {
        matches!(
            self,
            Self::Button { down: false, .. } | Self::Key { down: false, .. }
        )
    }
}

/// Write one bounded server packet. Validation/serialization completes before
/// touching the output; any subsequent I/O failure is terminal to the stream.
#[cfg(feature = "protobuf")]
pub fn write_server(
    writer: &mut impl std::io::Write,
    message: &ServerMessage,
) -> Result<(), String> {
    if let ServerMessage::ApplicationLifecycle(update) = message {
        update.validate()?;
    }
    if let ServerMessage::SessionApplications { session, snapshot } = message {
        if *session == 0 {
            return Err("Invalid session application inventory envelope".into());
        }
        snapshot.validate()?;
    }
    if let ServerMessage::LaunchResult {
        session,
        request,
        outcome,
    } = message
    {
        if *session == 0 || *request == 0 {
            return Err("Invalid application launch identity".into());
        }
        outcome.validate()?;
    }
    if let ServerMessage::ReverseFiles { message, .. } = message {
        message.validate()?;
    }
    if let ServerMessage::ReverseDrag { message, .. } = message {
        message.validate()?;
    }
    if let ServerMessage::Drag { message, .. } = message {
        message.validate()?;
    }
    if let ServerMessage::FileUpload { message, .. } = message {
        message.validate()?;
    }
    if let ServerMessage::TextInput { message, .. } = message {
        message.validate()?;
    }
    if let ServerMessage::AudioState { state, .. } = message {
        state.validate()?;
    }
    if let ServerMessage::Audio {
        captured: Some(time),
        ..
    }
    | ServerMessage::Frame {
        captured: Some(time),
        ..
    } = message
    {
        time.validate()?;
    }
    if let ServerMessage::Audio {
        session,
        sequence,
        sample_time,
        data,
        ..
    } = message
        && (*session == 0
            || *sequence == 0
            || sequence.checked_sub(1).and_then(|n| n.checked_mul(960)) != Some(*sample_time)
            || data.is_empty()
            || data.len() > MAX_AUDIO_BYTES)
    {
        return Err("Invalid audio packet".into());
    }
    if message.data_len() > crate::remote_apps::MAX_PAYLOAD_BYTES {
        return Err("Invalid server payload".into());
    }
    let encoded = crate::remote_apps::wire::encode_server(message)?;
    writer.write_all(&encoded).map_err(|e| e.to_string())
}

#[cfg(all(test, feature = "protobuf"))]
mod tests {
    use super::*;
    fn wire_server(message: &ServerMessage) -> ServerMessage {
        let mut bytes = Vec::new();
        write_server(&mut bytes, message).unwrap();
        crate::remote_apps::Packet::read(&mut bytes.as_slice())
            .unwrap()
            .message
    }
    fn wire_client(message: &ClientMessage) -> ClientMessage {
        let bytes = crate::remote_apps::wire::encode_client(message).unwrap();
        crate::remote_apps::wire::read_client(&mut bytes.as_slice())
            .unwrap()
            .0
    }
    #[test]
    fn audio_state_and_epochs_are_bounded() {
        use crate::remote_apps::audio::{MAX_REASON, State};
        let state = ServerMessage::AudioState {
            session: 1,
            state: State::Waiting {
                reason: "No sound server runs at /run/user/1000/pulse/native".into(),
            },
        };
        assert!(matches!(
            wire_server(&state),
            ServerMessage::AudioState {
                state: State::Waiting { .. },
                ..
            }
        ));
        let long = ServerMessage::AudioState {
            session: 1,
            state: State::Waiting {
                reason: "x".repeat(MAX_REASON + 1),
            },
        };
        assert!(write_server(&mut Vec::new(), &long).is_err());
        let audio = |epoch| ServerMessage::Audio {
            session: 1,
            sequence: 1,
            sample_time: 0,
            captured: None,
            hole: false,
            epoch,
            data: vec![1].into(),
        };
        for epoch in [0, 2] {
            assert!(
                matches!(wire_server(&audio(epoch)), ServerMessage::Audio { epoch: read, .. } if read == epoch)
            );
        }
    }
    fn calculator() -> Window {
        Window {
            key: WindowId {
                id: 1,
                generation: 1,
            },
            parent: None,
            kind: WindowKind::Normal,
            modal: false,
            position: [0, 0],
            title: "Calculator".into(),
            decoration: Decoration::Client,
            actions: Actions::default(),
            constraints: Default::default(),
            minimized: false,
            maximized: false,
            application: None,
            fullscreen: false,
        }
    }
    #[test]
    fn a_window_names_its_app_when_the_agent_matched_one() {
        let window = calculator();
        let updated = |window: &Window| ServerMessage::WindowUpdated {
            session: 1,
            window: window.clone(),
        };
        assert!(
            matches!(wire_server(&updated(&window)), ServerMessage::WindowUpdated { window: read, .. } if read == window)
        );
        let named = |desktop_id: &str| Window {
            application: crate::remote_apps::window_application::WindowApplication::new(
                None,
                Some(desktop_id.into()),
                None,
            ),
            ..window.clone()
        };
        let calculator = named("org.gnome.Calculator.desktop");
        calculator.validate().unwrap();
        assert!(
            matches!(wire_server(&updated(&calculator)), ServerMessage::WindowUpdated { window: read, .. } if read == calculator)
        );
        for invalid in ["", "calculator", "../x.desktop"] {
            assert!(named(invalid).validate().is_err());
        }
    }
    #[test]
    fn licenses_travel_as_secrets() {
        let hello = ClientMessage::Hello {
            versions: vec![VERSION],
            features: Default::default(),
            performance: None,
            display: None,
            license: Some(LicenseToken("v4.public.x".into())),
        };
        assert!(matches!(
            wire_client(&hello),
            ClientMessage::Hello { license: Some(LicenseToken(token)), .. } if token == "v4.public.x"
        ));
        let renewal = ClientMessage::License {
            session: 3,
            token: LicenseToken("v4.public.y".into()),
        };
        assert!(matches!(
            wire_client(&renewal),
            ClientMessage::License { session: 3, token: LicenseToken(token) } if token == "v4.public.y"
        ));
        // Debug output never shows a token, of the message or of its frame.
        assert!(!format!("{hello:?}").contains("v4.public"));
        let frame =
            crate::encoding::protobuf::generated::newport::remote::apps::ClientFrame::from(&hello);
        assert!(!format!("{frame:?}").contains("v4.public"));
    }
    #[test]
    fn a_takeover_names_the_computer_showing_the_display_as_data() {
        let ended = |computer: Option<&str>, session: Option<&str>| ServerMessage::Ended {
            session: 3,
            reason: ENDED_TAKEN_OVER.into(),
            message: "Shown on studio-mac.".into(),
            computer: computer.map(str::to_owned),
            computer_session: session.map(str::to_owned),
        };
        assert!(matches!(
            wire_server(&ended(Some("studio-mac"), Some("s-1"))),
            ServerMessage::Ended {
                computer: Some(computer),
                computer_session: Some(session),
                ..
            } if computer == "studio-mac" && session == "s-1"
        ));
        assert!(matches!(
            wire_server(&ended(None, None)),
            ServerMessage::Ended {
                computer: None,
                computer_session: None,
                ..
            }
        ));
    }
    #[test]
    fn window_actions_round_trip() {
        for action in [
            Operation::ShowWindowMenu { x: 12, y: -4 },
            Operation::StartResize {
                edge: ResizeEdge::BottomRight,
            },
            Operation::Fullscreen { enabled: false },
            Operation::Activate,
        ] {
            let message = ServerMessage::WindowAction {
                session: 1,
                window: WindowId {
                    id: 2,
                    generation: 3,
                },
                action: action.clone(),
            };
            let ServerMessage::WindowAction { action: read, .. } = wire_server(&message) else {
                panic!()
            };
            assert_eq!(format!("{read:?}"), format!("{action:?}"));
        }
        assert_eq!(ResizeEdge::TopLeft.sides(), [true, true, false, false]);
        assert_eq!(ResizeEdge::Right.sides(), [false, false, true, false]);
    }
    #[test]
    fn input_events_round_trip() {
        let modifiers = Modifiers {
            shift: true,
            logo: true,
            ..Modifiers::default()
        };
        let events = [
            Input::Pointer {
                x: 1,
                y: 2,
                modifiers,
            },
            Input::HeldPointer {
                x: -1,
                y: 2,
                modifiers: Modifiers::default(),
            },
            Input::Leave,
            Input::Focus,
            Input::Blur,
            Input::Button {
                button: Button::Back,
                down: true,
                modifiers,
            },
            Input::Scroll {
                position: [2, 2],
                dx: 0.5,
                dy: -12.25,
                discrete: Some([0, -120]),
                phase: ScrollPhase::Wheel,
                modifiers,
            },
            Input::Key {
                key: "A".into(),
                code: 30,
                down: false,
                repeat: true,
                modifiers,
            },
        ];
        for event in events {
            let message = ClientMessage::Input {
                session: 1,
                window: WindowId {
                    id: 1,
                    generation: 1,
                },
                surface: 1,
                input: event.clone(),
            };
            let ClientMessage::Input { input, .. } = wire_client(&message) else {
                panic!()
            };
            assert_eq!(input, event);
        }
    }
    #[test]
    fn every_cursor_round_trips() {
        let shapes = crate::remote_apps::samples::cursor_shapes();
        assert_eq!(shapes.len(), 34);
        for cursor in shapes
            .into_iter()
            .map(|shape| Cursor::Shape { shape })
            .chain([Cursor::Hidden])
        {
            let message = ServerMessage::Cursor {
                session: 1,
                window: WindowId {
                    id: 2,
                    generation: 1,
                },
                cursor,
            };
            assert!(
                matches!(wire_server(&message), ServerMessage::Cursor { cursor: read, .. } if read == cursor)
            );
        }
    }
    #[test]
    fn malformed_keys_and_scrolls_are_rejected() {
        let key = |key: &str, code| Input::Key {
            key: key.into(),
            code,
            down: true,
            repeat: false,
            modifiers: Modifiers::default(),
        };
        assert!(key("a", MAX_KEY_CODE).validate().is_ok());
        for invalid in [
            key("", 30),
            key(&"a".repeat(65), 30),
            key("a\0", 30),
            key("a", MAX_KEY_CODE + 1),
        ] {
            assert!(invalid.validate().is_err(), "{invalid:?}");
        }
        let scroll = |dy, discrete| Input::Scroll {
            position: [2, 2],
            dx: 0.0,
            dy,
            discrete,
            phase: ScrollPhase::Update,
            modifiers: Modifiers::default(),
        };
        assert!(
            scroll(MAX_SCROLL_PIXELS, Some([0, MAX_SCROLL_DISCRETE]))
                .validate()
                .is_ok()
        );
        for invalid in [
            scroll(f64::NAN, None),
            scroll(f64::INFINITY, None),
            scroll(-MAX_SCROLL_PIXELS - 1.0, None),
            scroll(1.0, Some([MAX_SCROLL_DISCRETE + 1, 0])),
            scroll(1.0, Some([0, i32::MIN])),
        ] {
            assert!(invalid.validate().is_err(), "{invalid:?}");
        }
    }
    #[test]
    fn small_surfaces_and_fractional_scale_validate_without_float_rounding() {
        let s = Surface {
            generation: 1,
            logical_width: 3,
            logical_height: 5,
            pixel_width: 5,
            pixel_height: 8,
            scale: Scale {
                numerator: 3,
                denominator: 2,
            },
        };
        assert_eq!(s.bytes().unwrap(), 160);
        assert!(
            Surface {
                pixel_width: 4,
                ..s
            }
            .geometry()
            .is_err()
        );
        assert!(
            Surface {
                scale: Scale {
                    numerator: 1,
                    denominator: 0
                },
                ..s
            }
            .geometry()
            .is_err()
        );
        assert!(Surface { generation: 0, ..s }.geometry().is_err());
    }
    #[test]
    fn a_held_pointer_reaches_a_bounded_distance_past_each_edge() {
        let s = Surface {
            generation: 1,
            logical_width: 200,
            logical_height: 100,
            pixel_width: 400,
            pixel_height: 200,
            scale: Scale {
                numerator: 2,
                denominator: 1,
            },
        };
        let reach = HELD_POINTER_REACH as i32;
        for (x, y) in [
            (0, 0),
            (-reach, -reach),
            (399 + reach, 199 + reach),
            (-5, 250),
        ] {
            assert!(s.reaches(x, y), "{x},{y}");
        }
        for (x, y) in [
            (-reach - 1, 0),
            (400 + reach, 0),
            (0, 200 + reach),
            (i32::MIN, 0),
        ] {
            assert!(!s.reaches(x, y), "{x},{y}");
        }
    }
    #[test]
    fn metadata_is_bounded_apart_from_the_data_a_message_carries() {
        let frame = |data: usize| ServerMessage::Frame {
            session: 1,
            window: WindowId {
                id: 1,
                generation: 1,
            },
            surface: 1,
            sequence: 1,
            keyframe: true,
            tiles: vec![crate::remote_apps::tile::Tile::new(
                0,
                crate::remote_apps::tile::Codec::RawBgra,
                vec![0; data],
            )],
            captured: None,
            diagnostics: None,
        };
        let packet = wire_server(&frame(crate::remote_apps::MAX_HEADER_BYTES * 4));
        assert_eq!(packet.data_len(), crate::remote_apps::MAX_HEADER_BYTES * 4);
        let error = ServerMessage::Error {
            session: 1,
            message: "x".repeat(crate::remote_apps::MAX_HEADER_BYTES + 1),
        };
        let mut rejected = Vec::new();
        assert!(write_server(&mut rejected, &error).is_err());
        assert!(rejected.is_empty());
        let oversized = frame(crate::remote_apps::MAX_PAYLOAD_BYTES + 1);
        assert!(write_server(&mut rejected, &oversized).is_err());
        assert!(rejected.is_empty());
    }
    #[test]
    fn audio_framing_bounds_and_clock_are_validated_before_writing() {
        for (session, sequence, sample_time, size, valid) in [
            (1, 1, 0, 20, true),
            (1, 3, 1920, 1275, true),
            (0, 1, 0, 20, false),
            (1, 0, 0, 20, false),
            (1, 2, 0, 20, false),
            (1, 1, 0, 0, false),
            (1, 1, 0, 1276, false),
            (1, u64::MAX, 0, 20, false),
        ] {
            let audio = ServerMessage::Audio {
                session,
                sequence,
                sample_time,
                captured: None,
                hole: false,
                epoch: 0,
                data: vec![0; size].into(),
            };
            let mut bytes = Vec::new();
            assert_eq!(write_server(&mut bytes, &audio).is_ok(), valid);
            if !valid {
                assert!(bytes.is_empty());
            } else {
                let packet = crate::remote_apps::Packet::read(&mut bytes.as_slice()).unwrap();
                assert_eq!(packet.message.data_len(), size);
            }
            // A client refuses the same audio from an agent that doesn't check.
            let unchecked = crate::remote_apps::wire::encode_server(&audio).unwrap();
            let read = crate::remote_apps::Packet::read(&mut unchecked.as_slice());
            if size == 0 || size > MAX_AUDIO_BYTES {
                assert!(matches!(
                    read,
                    Err(crate::remote_apps::wire::ReadError::InvalidAudioPayload)
                ));
            }
        }
    }
}

#[cfg(all(test, feature = "protobuf"))]
mod physical_resize_tests {
    use super::*;
    use crate::remote_apps::sizing::PixelConstraints;
    #[test]
    fn physical_bounds_override_old_scale_projection_only_when_present() {
        let mut constraints = Constraints {
            min: Some([100, 100]),
            physical: Some(PixelConstraints {
                min: Some([100, 100]),
                ..Default::default()
            }),
            ..Default::default()
        };
        let double = Scale {
            numerator: 2,
            denominator: 1,
        };
        assert!(constraints.validate_resize([50, 50], double).is_ok());
        assert!(constraints.validate_resize([49, 50], double).is_err());
        constraints.physical = None;
        assert!(constraints.validate_resize([50, 50], double).is_err());
    }
    #[test]
    fn grid_aspect_and_surface_limits_are_enforced_together() {
        let constraints = Constraints {
            physical: Some(PixelConstraints {
                base: Some([2, 4]),
                increment: Some([8, 16]),
                aspect: Some([[1, 2], [2, 1]]),
                ..Default::default()
            }),
            ..Default::default()
        };
        let one = Scale {
            numerator: 1,
            denominator: 1,
        };
        assert!(constraints.validate_resize([10, 20], one).is_ok());
        for size in [[11, 20], [10, 21], [10, 52], [4097, 20], [0, 0]] {
            assert!(constraints.validate_resize(size, one).is_err());
        }
        assert!(
            constraints
                .validate_resize(
                    [10, 20],
                    Scale {
                        numerator: 1,
                        denominator: 0
                    }
                )
                .is_err()
        );
    }
}

/// Host-side observations; ACK service time is not network RTT.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct FrameDiagnostics {
    pub target_fps: u32,
    pub fast_codec: bool,
    pub ack_ms: f64,
    pub work_ms: f64,
    /// Backend capture work consumed by this frame, including any SHM snapshot
    /// and composition performed before encoding (included in work_ms).
    /// Measures active elapsed work, not the age of the captured pixels.
    #[serde(default)]
    pub capture_ms: f64,
    /// Time an ACK spent waiting for the host loop, excluded from ack_ms.
    #[serde(default)]
    pub dispatch_ms: f64,
}
