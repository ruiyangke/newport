//! Terminal emulation and input state, independent of GPUI and SSH transport.
//!
//! Ghostty owns the VT parser and both screen buffers. The UI consumes owned
//! viewport snapshots, and the transport consumes the replies in [`Effects`].
//! No remote escape sequence is allowed to access the host clipboard.

mod engine;
mod frame;
mod input;
mod search;
mod selection;

pub use engine::{Effects, Engine, Theme};
pub use frame::{Cell, Frame, Row};
pub use input::{KeyInput, MouseInput};
pub use libghostty_vt::error::{Error, Result};
pub use libghostty_vt::key::KittyKeyFlags;
pub use libghostty_vt::key::{Action as KeyAction, Key, Mods as Modifiers};
pub use libghostty_vt::mouse::{Action as MouseAction, Button as MouseButton};
pub use libghostty_vt::render::{Cursor, CursorVisualStyle};
pub use libghostty_vt::screen::CellWide;
pub use libghostty_vt::style::{Palette, PaletteIndex, RgbColor, Style, StyleColor, Underline};
pub use libghostty_vt::terminal::{Mode, Point, PointCoordinate, ScrollViewport};
pub use search::{MAX_MATCHES, Position, SearchResults};
pub use selection::SelectionPosition;
