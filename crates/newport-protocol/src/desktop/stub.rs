//! Typed local window/presentation contract. Native transport descriptors remain in OS adapters.

pub const MAX_MESSAGE_BYTES: usize = 16 * 1024;
pub const MAX_TITLE_BYTES: usize = 1024;
pub const MAX_TEXT_BYTES: usize = 4096;
pub const MAX_NAME_BYTES: usize = 256;
/// A window shows one surface while the host writes another; a third covers
/// the WindowServer still holding the previous one.
pub const MAX_SURFACES_PER_WINDOW: u8 = 3;
pub const MAX_WINDOWS_PER_STUB: usize = 256;
/// Larger than any Mac display, so a corrupt size never allocates a window.
pub const MAX_WINDOW_EXTENT: u32 = 16_384;

/// Modifier bits in [`ToHost::Key`].
pub mod modifier {
    pub const SHIFT: u32 = 1;
    pub const CONTROL: u32 = 2;
    pub const OPTION: u32 = 4;
    pub const COMMAND: u32 = 8;
    pub const CAPS_LOCK: u32 = 16;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LaunchReason {
    /// Dock, Finder, Spotlight or login: the user asked for the app.
    User,
    /// Newport opened the stub for a window that arrived.
    Host,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WindowKind {
    Normal,
    Dialog,
    Menu,
    Tooltip,
}

impl WindowKind {
    pub fn is_popup(self) -> bool {
        matches!(self, Self::Menu | Self::Tooltip)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decoration {
    /// A titled window with traffic lights.
    Server,
    /// The app draws its own title bar; traffic lights are hidden.
    Client,
    None,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Actions {
    pub close: bool,
    pub minimize: bool,
    pub maximize: bool,
    pub resize: bool,
    pub fullscreen: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Activate {
    No,
    Yes,
    /// Open behind the active app and bounce the Dock tile.
    Attention,
}

#[derive(Clone, Debug, PartialEq)]
pub struct WindowState {
    pub window: u64,
    pub generation: u64,
    pub kind: WindowKind,
    /// Zero for none.
    pub parent: u64,
    pub modal: bool,
    /// Logical points, relative to the parent for popups.
    pub position: [i32; 2],
    pub logical: [u32; 2],
    pub pixels: [u32; 2],
    /// Resize the Mac window to `logical`. False while the user is still
    /// resizing, so a reply to an older request can't pull the window back.
    pub apply_size: bool,
    pub decoration: Decoration,
    pub actions: Actions,
    pub min: Option<[u32; 2]>,
    pub max: Option<[u32; 2]>,
    pub title: String,
    pub activate: Activate,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Operation {
    /// Start a window drag on the next mouse-dragged event of this press.
    Move,
    Minimize,
    Zoom {
        maximized: bool,
    },
    Fullscreen {
        enabled: bool,
    },
    Activate,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ToStub {
    Welcome {
        name: String,
        server: String,
    },
    /// Create or replace a window. Carries one surface right per pool slot.
    Window {
        state: WindowState,
        surfaces: u8,
    },
    /// Add a pool slot to the current generation. Carries one surface right.
    Surface {
        window: u64,
        generation: u64,
        index: u8,
    },
    /// Show pool slot `index`; the host has finished writing it.
    Frame {
        window: u64,
        generation: u64,
        index: u8,
    },
    Title {
        window: u64,
        title: String,
    },
    Operation {
        window: u64,
        operation: Operation,
    },
    Destroy {
        window: u64,
    },
    /// Answer to [`ToHost::QuitAsked`].
    QuitReply {
        allow: bool,
    },
    Alert {
        message: String,
        detail: String,
    },
    /// Exit now: the app has no windows left or the session ended.
    Quit,
    /// The remote text field with focus in this window: `caret` is the
    /// bottom of its caret in window points, when the app reports one.
    TextContext {
        window: u64,
        editing: bool,
        caret: Option<[i32; 2]>,
    },
    /// The cursor to show over this window, one of `cursor`'s.
    Cursor {
        window: u64,
        cursor: u8,
    },
}

/// The cursors a stub shows, by `NSCursor`.
pub mod cursor {
    pub const ARROW: u8 = 0;
    pub const I_BEAM: u8 = 1;
    pub const VERTICAL_I_BEAM: u8 = 2;
    pub const POINTING_HAND: u8 = 3;
    pub const CROSSHAIR: u8 = 4;
    pub const OPEN_HAND: u8 = 5;
    pub const CLOSED_HAND: u8 = 6;
    pub const NOT_ALLOWED: u8 = 7;
    pub const DRAG_COPY: u8 = 8;
    pub const DRAG_LINK: u8 = 9;
    pub const CONTEXT_MENU: u8 = 10;
    pub const RESIZE_LEFT: u8 = 11;
    pub const RESIZE_RIGHT: u8 = 12;
    pub const RESIZE_LEFT_RIGHT: u8 = 13;
    pub const RESIZE_UP: u8 = 14;
    pub const RESIZE_DOWN: u8 = 15;
    pub const RESIZE_UP_DOWN: u8 = 16;
    pub const RESIZE_NORTH_WEST_SOUTH_EAST: u8 = 17;
    pub const RESIZE_NORTH_EAST_SOUTH_WEST: u8 = 18;
    pub const RESIZE_COLUMN: u8 = 19;
    pub const RESIZE_ROW: u8 = 20;
    pub const HIDDEN: u8 = 21;
}

#[derive(Clone, Debug, PartialEq)]
pub enum ToHost {
    /// Carries a send right to the stub's receive port.
    Hello {
        version: u32,
        reason: LaunchReason,
        bundle_id: String,
    },
    /// Window points, top-left origin, in the generation the stub shows.
    Pointer {
        window: u64,
        generation: u64,
        x: f64,
        y: f64,
        modifiers: u32,
    },
    Button {
        window: u64,
        generation: u64,
        x: f64,
        y: f64,
        button: u8,
        down: bool,
        modifiers: u32,
    },
    /// Deltas in points; `precise` for trackpads, else wheel lines. `phase`
    /// and `momentum` are AppKit's `NSEventPhase` bits.
    Scroll {
        window: u64,
        generation: u64,
        x: f64,
        y: f64,
        dx: f64,
        dy: f64,
        precise: bool,
        phase: u64,
        momentum: u64,
        modifiers: u32,
    },
    Key {
        window: u64,
        generation: u64,
        code: u16,
        characters: String,
        unmodified: String,
        modifiers: u32,
        repeat: bool,
    },
    /// A key went up. AppKit sends none for a key released while Command
    /// is held, or once the window is no longer key.
    KeyUp {
        window: u64,
        generation: u64,
        code: u16,
        modifiers: u32,
    },
    /// A modifier key went down or up: its key code, and AppKit's
    /// `modifierFlags` after it, which tell the left and right keys apart.
    Modifiers {
        window: u64,
        generation: u64,
        code: u16,
        flags: u64,
    },
    /// The pointer left the window.
    Exit {
        window: u64,
        generation: u64,
    },
    /// Text committed by the input method.
    Text {
        window: u64,
        generation: u64,
        text: String,
    },
    /// The input method's composition, shown in the app until committed;
    /// `cursor` counts characters. Empty text cancels it.
    Preedit {
        window: u64,
        generation: u64,
        text: String,
        cursor: u32,
    },
    Focus {
        window: u64,
        key: bool,
    },
    /// The user resized the window to this content size.
    Resized {
        window: u64,
        logical: [u32; 2],
    },
    CloseAsked {
        window: u64,
    },
    Minimized {
        window: u64,
        minimized: bool,
    },
    Fullscreen {
        window: u64,
        enabled: bool,
    },
    Presented {
        window: u64,
        generation: u64,
        index: u8,
    },
    AppActive {
        active: bool,
    },
    QuitAsked,
    /// Dock tile clicked while the app has no windows.
    Reopen,
    /// The screen a window is on, in AppKit's global coordinates (points,
    /// y up): its backing scale, its usable area, and the window content's
    /// top-left corner.
    Display {
        window: u64,
        scale: f64,
        work_area: [f64; 4],
        content: [f64; 2],
    },
    /// Whether any part of the window is on screen; a minimized, hidden or
    /// covered window needs no frames.
    Visible {
        window: u64,
        visible: bool,
    },
}

pub const VERSION: u32 = 1;
pub const LIMITS: crate::common::Limits = crate::common::Limits {
    message: MAX_MESSAGE_BYTES - 10,
    metadata: MAX_MESSAGE_BYTES - 10,
};
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Invalid(pub &'static str);
impl std::fmt::Display for Invalid {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Invalid desktop stub {}", self.0)
    }
}
impl std::error::Error for Invalid {}
#[cfg(feature = "protobuf")]
pub mod wire;
fn text(value: &str, limit: usize) -> Result<(), Invalid> {
    if value.len() > limit || value.contains('\0') {
        Err(Invalid("text"))
    } else {
        Ok(())
    }
}
fn size(value: [u32; 2]) -> Result<(), Invalid> {
    if value
        .into_iter()
        .all(|v| (1..=MAX_WINDOW_EXTENT).contains(&v))
    {
        Ok(())
    } else {
        Err(Invalid("window extent"))
    }
}
fn index(value: u8) -> Result<(), Invalid> {
    if value < MAX_SURFACES_PER_WINDOW {
        Ok(())
    } else {
        Err(Invalid("surface index"))
    }
}
fn finite(values: impl IntoIterator<Item = f64>) -> Result<(), Invalid> {
    if values.into_iter().all(f64::is_finite) {
        Ok(())
    } else {
        Err(Invalid("non-finite coordinate"))
    }
}
impl WindowState {
    pub fn validate(&self) -> Result<(), Invalid> {
        size(self.logical)?;
        size(self.pixels)?;
        if let Some(value) = self.min {
            size(value)?;
        }
        if let Some(value) = self.max {
            size(value)?;
        }
        text(&self.title, MAX_TITLE_BYTES)
    }
}
impl ToStub {
    pub fn ports(&self) -> usize {
        match self {
            Self::Window { surfaces, .. } => usize::from(*surfaces),
            Self::Surface { .. } => 1,
            _ => 0,
        }
    }
    pub fn validate(&self) -> Result<(), Invalid> {
        match self {
            Self::Welcome { name, server } => {
                text(name, MAX_NAME_BYTES)?;
                text(server, MAX_NAME_BYTES)?;
            }
            Self::Window { state, surfaces } => {
                state.validate()?;
                if !(1..=MAX_SURFACES_PER_WINDOW).contains(surfaces) {
                    return Err(Invalid("surface count"));
                }
            }
            Self::Surface { index: slot, .. } | Self::Frame { index: slot, .. } => index(*slot)?,
            Self::Title { title, .. } => text(title, MAX_TITLE_BYTES)?,
            Self::Alert { message, detail } => {
                text(message, MAX_TEXT_BYTES)?;
                text(detail, MAX_TEXT_BYTES)?;
            }
            Self::Cursor { cursor: value, .. } if *value > cursor::HIDDEN => {
                return Err(Invalid("cursor"));
            }
            Self::Operation { .. }
            | Self::Destroy { .. }
            | Self::QuitReply { .. }
            | Self::Quit
            | Self::TextContext { .. }
            | Self::Cursor { .. } => {}
        }
        Ok(())
    }
}
impl ToHost {
    pub fn ports(&self) -> usize {
        usize::from(matches!(self, Self::Hello { .. }))
    }
    pub fn validate(&self) -> Result<(), Invalid> {
        match self {
            Self::Hello {
                version, bundle_id, ..
            } => {
                if *version != VERSION {
                    return Err(Invalid("version"));
                }
                text(bundle_id, MAX_NAME_BYTES)?;
            }
            Self::Pointer { x, y, .. } | Self::Button { x, y, .. } => finite([*x, *y])?,
            Self::Scroll { x, y, dx, dy, .. } => finite([*x, *y, *dx, *dy])?,
            Self::Key {
                characters,
                unmodified,
                ..
            } => {
                text(characters, MAX_TEXT_BYTES)?;
                text(unmodified, MAX_TEXT_BYTES)?;
            }
            Self::Text { text: value, .. } | Self::Preedit { text: value, .. } => {
                text(value, MAX_TEXT_BYTES)?
            }
            Self::Resized { logical, .. } => size(*logical)?,
            Self::Presented { index: slot, .. } => index(*slot)?,
            Self::Display {
                scale,
                work_area,
                content,
                ..
            } => finite([
                *scale,
                work_area[0],
                work_area[1],
                work_area[2],
                work_area[3],
                content[0],
                content[1],
            ])?,
            Self::KeyUp { .. }
            | Self::Modifiers { .. }
            | Self::Exit { .. }
            | Self::Focus { .. }
            | Self::CloseAsked { .. }
            | Self::Minimized { .. }
            | Self::Fullscreen { .. }
            | Self::AppActive { .. }
            | Self::QuitAsked
            | Self::Reopen
            | Self::Visible { .. } => {}
        }
        Ok(())
    }
}
/// Input and window state are admitted only after an exact current Welcome.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Phase {
    #[default]
    AwaitingWelcome,
    Ready,
    Refused,
}
#[derive(Debug, Default)]
pub struct Admission {
    phase: Phase,
}
impl Admission {
    pub fn phase(&self) -> Phase {
        self.phase
    }
    pub fn accept(&mut self, message: &ToStub) -> Result<(), Invalid> {
        message.validate()?;
        match (self.phase, message) {
            (Phase::AwaitingWelcome, ToStub::Welcome { .. }) => self.phase = Phase::Ready,
            (Phase::AwaitingWelcome, ToStub::Alert { .. }) => {}
            (Phase::AwaitingWelcome, ToStub::Quit) => self.phase = Phase::Refused,
            (Phase::Ready, ToStub::Welcome { .. })
            | (Phase::Refused, _)
            | (Phase::AwaitingWelcome, _) => return Err(Invalid("admission phase")),
            (Phase::Ready, _) => {}
        }
        Ok(())
    }
}
