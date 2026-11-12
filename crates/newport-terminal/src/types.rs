//! Newport's UI boundary types; emulator internals stay inside this crate.
pub use alacritty_terminal::vte::ansi::Rgb as RgbColor;
use bitflags::bitflags;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    InvalidValue,
    LimitExceeded,
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::InvalidValue => "invalid terminal value",
            Self::LimitExceeded => "terminal limit exceeded",
        })
    }
}
impl std::error::Error for Error {}
pub type Result<T> = std::result::Result<T, Error>;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PaletteIndex(pub u8);
impl PaletteIndex {
    pub const RED: Self = Self(1);
    pub const GREEN: Self = Self(2);
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum StyleColor {
    #[default]
    None,
    Palette(PaletteIndex),
    Rgb(RgbColor),
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Underline {
    #[default]
    None,
    Single,
    Double,
    Curly,
    Dotted,
    Dashed,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Style {
    pub bold: bool,
    pub italic: bool,
    pub faint: bool,
    pub inverse: bool,
    pub invisible: bool,
    pub strikethrough: bool,
    pub underline: Underline,
    pub underline_color: StyleColor,
    pub fg_color: StyleColor,
    pub bg_color: StyleColor,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CellWide {
    Narrow,
    Wide,
    SpacerHead,
    SpacerTail,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CursorVisualStyle {
    Block,
    BlockHollow,
    Bar,
    Underline,
}
#[derive(Clone, Copy, Debug)]
pub struct Cursor {
    pub visible: bool,
    pub blinking: bool,
    pub visual_style: CursorVisualStyle,
    pub viewport: Option<CursorPosition>,
}
#[derive(Clone, Copy, Debug)]
pub struct CursorPosition {
    pub x: u16,
    pub y: u16,
    pub at_wide_tail: bool,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PointCoordinate {
    pub x: u16,
    pub y: u32,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Point {
    Active(PointCoordinate),
    Viewport(PointCoordinate),
    Screen(PointCoordinate),
    History(PointCoordinate),
}
#[derive(Clone, Copy, Debug)]
pub enum ScrollViewport {
    Top,
    Bottom,
    Delta(isize),
    Row(usize),
}
#[allow(non_camel_case_types)]
#[derive(Clone, Copy, Debug)]
pub enum Mode {
    FOCUS_EVENT,
    BRACKETED_PASTE,
    DECCKM,
    ALT_SCROLL,
    X10_MOUSE,
    NORMAL_MOUSE,
    BUTTON_MOUSE,
    ANY_MOUSE,
}
bitflags! {
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
    pub struct Modifiers: u8 { const SHIFT=1; const ALT=2; const CTRL=4; const SUPER=8; const CAPS_LOCK=64; const NUM_LOCK=128; }
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
    pub struct KittyKeyFlags: u8 { const DISAMBIGUATE=1; const REPORT_EVENTS=2; const REPORT_ALTERNATES=4; const REPORT_ALL=8; const REPORT_TEXT=16; }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyAction {
    Press,
    Repeat,
    Release,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MouseAction {
    Press,
    Release,
    Motion,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MouseButton {
    Left,
    Middle,
    Right,
    Four,
    Five,
    Six,
    Seven,
    Eight,
    Nine,
    Ten,
    Eleven,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    A,
    AltLeft,
    AltRight,
    ArrowDown,
    ArrowLeft,
    ArrowRight,
    ArrowUp,
    AudioVolumeDown,
    AudioVolumeMute,
    AudioVolumeUp,
    B,
    Backquote,
    Backslash,
    Backspace,
    BracketLeft,
    BracketRight,
    BrowserBack,
    BrowserFavorites,
    BrowserForward,
    BrowserHome,
    BrowserRefresh,
    BrowserSearch,
    BrowserStop,
    C,
    CapsLock,
    Comma,
    ContextMenu,
    ControlLeft,
    ControlRight,
    Convert,
    Copy,
    Cut,
    D,
    Delete,
    Digit0,
    Digit1,
    Digit2,
    Digit3,
    Digit4,
    Digit5,
    Digit6,
    Digit7,
    Digit8,
    Digit9,
    E,
    Eject,
    End,
    Enter,
    Equal,
    Escape,
    F,
    F1,
    F10,
    F11,
    F12,
    F13,
    F14,
    F15,
    F16,
    F17,
    F18,
    F19,
    F2,
    F20,
    F21,
    F22,
    F23,
    F24,
    F3,
    F4,
    F5,
    F6,
    F7,
    F8,
    F9,
    G,
    H,
    Help,
    Home,
    I,
    Insert,
    IntlBackslash,
    IntlRo,
    IntlYen,
    J,
    K,
    KanaMode,
    L,
    LaunchApp1,
    LaunchApp2,
    LaunchMail,
    M,
    MediaPlayPause,
    MediaSelect,
    MediaStop,
    MediaTrackNext,
    MediaTrackPrevious,
    MetaLeft,
    MetaRight,
    Minus,
    N,
    NonConvert,
    NumLock,
    Numpad0,
    Numpad1,
    Numpad2,
    Numpad3,
    Numpad4,
    Numpad5,
    Numpad6,
    Numpad7,
    Numpad8,
    Numpad9,
    NumpadAdd,
    NumpadComma,
    NumpadDecimal,
    NumpadDivide,
    NumpadEnter,
    NumpadEqual,
    NumpadMultiply,
    NumpadSubtract,
    O,
    P,
    PageDown,
    PageUp,
    Paste,
    Pause,
    Period,
    Power,
    PrintScreen,
    Q,
    Quote,
    R,
    S,
    ScrollLock,
    Semicolon,
    ShiftLeft,
    ShiftRight,
    Slash,
    Sleep,
    Space,
    T,
    Tab,
    U,
    Unidentified,
    V,
    W,
    WakeUp,
    X,
    Y,
    Z,
    F25,
    F26,
    F27,
    F28,
    F29,
    F30,
    F31,
    F32,
    F33,
    F34,
    F35,
    NumpadSeparator,
}
