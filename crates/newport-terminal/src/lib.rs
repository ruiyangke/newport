//! Terminal emulation and input state, independent of GPUI and SSH transport.
//!
//! Alacritty owns the VT parser and both screen buffers. The UI consumes owned
//! viewport snapshots, and the transport consumes the replies in [`Effects`].
//! A program may copy text to the host clipboard (OSC 52 writes, as tmux
//! does); no remote escape sequence can read or clear it.

mod engine;
mod frame;
mod handler;
mod input;
mod links;
mod queries;
mod reflow;
mod search;
mod selection;
mod types;

pub use engine::{Effects, Engine, Theme};
pub use frame::{Cell, Frame, Row};
pub use input::{KeyInput, MouseInput};
pub use search::{MAX_MATCHES, Position, SearchResults};
pub use selection::SelectionPosition;
pub use types::*;
