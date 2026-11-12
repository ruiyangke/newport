//! Shared wire geometry and bounded packet framing.

#[cfg(feature = "protobuf")]
pub use crate::encoding::protobuf::DESCRIPTORS;
/// The protocol's revision, from Newport through the relay to the display.
/// A display accepts only a relay of its own revision, so a change an older
/// peer can't read bumps it.
pub const REVISION: u32 = 1;
#[cfg(feature = "protobuf")]
mod convert;
pub mod display;
pub mod features;
pub mod performance;
#[cfg(all(test, feature = "protobuf"))]
mod samples;
pub mod session_activity;
pub mod session_application_lifecycle;
pub mod session_applications;
pub mod session_launch;
pub mod sizing;
pub mod window_application;
#[cfg(feature = "protobuf")]
use std::io::Read;
pub const WIDTH: usize = 2400;
pub const HEIGHT: usize = 1600;
pub const TILE: usize = 256;
#[cfg(all(test, feature = "protobuf"))]
pub const COLS: usize = WIDTH.div_ceil(TILE);
#[cfg(all(test, feature = "protobuf"))]
pub const COUNT: usize = COLS * HEIGHT.div_ceil(TILE);
#[cfg(all(test, feature = "protobuf"))]
pub fn geometry(id: usize) -> (usize, usize, usize, usize) {
    let x = id % COLS * TILE;
    let y = id / COLS * TILE;
    (x, y, TILE.min(WIDTH - x), TILE.min(HEIGHT - y))
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Geometry {
    pub width: usize,
    pub height: usize,
}
impl Default for Geometry {
    fn default() -> Self {
        Self {
            width: WIDTH,
            height: HEIGHT,
        }
    }
}
impl Geometry {
    pub fn new(width: usize, height: usize) -> Result<Self, String> {
        if !(256..=4096).contains(&width) || !(256..=4096).contains(&height) {
            return Err("Invalid viewport".into());
        }
        Ok(Self { width, height })
    }
    /// Pixel surfaces may be smaller than a viewport (menus and dialogs).
    pub fn surface(width: usize, height: usize) -> Result<Self, String> {
        if !(1..=4096).contains(&width) || !(1..=4096).contains(&height) {
            return Err("Invalid surface dimensions".into());
        }
        Ok(Self { width, height })
    }
    pub fn tile(self, id: usize) -> (usize, usize, usize, usize) {
        let cols = self.width.div_ceil(TILE);
        let x = id % cols * TILE;
        let y = id / cols * TILE;
        (x, y, TILE.min(self.width - x), TILE.min(self.height - y))
    }
}
#[derive(Clone, Debug, Default, serde::Deserialize)]
pub struct WindowMetadata {
    pub id: u64,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub decorated: bool,
    #[serde(default)]
    pub close: bool,
    #[serde(default)]
    pub minimize: bool,
    #[serde(default)]
    pub maximize: bool,
    #[serde(default)]
    pub fixed_width: Option<usize>,
    #[serde(default)]
    pub fixed_height: Option<usize>,
}

/// The most a message apart from its data may take: its metadata.
pub const MAX_HEADER_BYTES: usize = 32 * 1024;
/// The most data one agent message carries: a frame's tiles or audio.
pub const MAX_PAYLOAD_BYTES: usize = 32 * 1024 * 1024;
/// What a client message may take, as the agent reads it.
pub const CLIENT_LIMITS: crate::common::Limits = crate::common::Limits {
    message: 64 * 1024,
    metadata: 64 * 1024,
};
/// The bytes one file upload or reverse drag chunk carries. Each
/// chunk is a credit in its transfer's flow control.
pub const CHUNK_BYTES: usize = 4096;

/// The largest agent message: its data, and the rest of the message.
pub const MAX_SERVER_BYTES: usize = MAX_PAYLOAD_BYTES + MAX_HEADER_BYTES;
/// What an agent message may take, as [`Packet::read`] reads it.
pub const SERVER_LIMITS: crate::common::Limits = crate::common::Limits {
    message: MAX_SERVER_BYTES,
    metadata: MAX_HEADER_BYTES,
};

/// One agent message, fully read and bounded. Semantic validation belongs to
/// the session.
#[cfg(feature = "protobuf")]
#[derive(Debug)]
pub struct Packet {
    pub message: v1::ServerMessage,
    /// The bytes its frame took on the wire.
    pub wire_bytes: usize,
}
#[cfg(feature = "protobuf")]
impl Packet {
    pub fn read(reader: &mut impl Read) -> Result<Self, wire::ReadError> {
        let (message, wire_bytes) = wire::read_server(reader)?;
        if let v1::ServerMessage::Audio { data, .. } = &message
            && (data.is_empty() || data.len() > v1::MAX_AUDIO_BYTES)
        {
            return Err(wire::ReadError::InvalidAudioPayload);
        }
        Ok(Self {
            message,
            wire_bytes,
        })
    }
}

#[cfg(all(test, feature = "protobuf"))]
mod byte_fields_tests {
    use serde_json::json;
    fn round_trip<T: serde::Serialize + serde::de::DeserializeOwned>(value: serde_json::Value) {
        let decoded: T = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(serde_json::to_value(decoded).unwrap(), value);
    }
    /// Chunks keep their JSON form, an array of numbers, and come back whole.
    #[test]
    fn chunk_data_keeps_its_json_form() {
        let data = json!([0, 1, 127, 255]);
        round_trip::<crate::remote_apps::file_upload::Client>(
            json!({"kind":"chunk","transfer":1,"file":0,"offset":0,"data":data}),
        );
        round_trip::<crate::remote_apps::reverse_drag::Server>(
            json!({"kind":"chunk","drag":1,"offset":0,"data":data}),
        );
        let too_large = json!({"kind":"chunk","drag":1,"offset":0,"data":[256]});
        assert!(
            serde_json::from_value::<crate::remote_apps::reverse_drag::Server>(too_large).is_err()
        );
    }
}

pub mod tile;
pub mod v1;
#[cfg(feature = "protobuf")]
pub mod wire;

pub mod text_input;

pub mod file_tree;
pub mod file_upload;

pub mod drag;

pub mod apps;

pub mod reverse_drag;
pub mod reverse_files;

/// Audio, with `features.audio`: it may start, stop and start again during
/// the session. The agent reports each change as a [`State`], and numbers
/// each stream's packets from one under a new epoch.
pub mod audio {
    /// Longest reason a [`State`] carries, in bytes.
    pub const MAX_REASON: usize = 1024;
    #[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    #[serde(tag = "status", rename_all = "snake_case")]
    pub enum State {
        /// No sound server plays for the session yet, or it stopped.
        Waiting {
            reason: String,
        },
        Playing,
    }
    impl State {
        pub fn validate(&self) -> Result<(), String> {
            match self {
                Self::Waiting { reason } if reason.len() > MAX_REASON || reason.contains('\0') => {
                    Err("Invalid audio state".into())
                }
                _ => Ok(()),
            }
        }
    }
}

mod size;
