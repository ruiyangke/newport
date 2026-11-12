//! Copy-only remote-to-native drag. Preview availability is not drop completion.
//! A read grants one bounded burst; the next read acknowledges the whole burst.
use crate::remote_apps::v1::WindowId;
use serde::{Deserialize, Serialize};
pub const MAX_BYTES: usize = 8 * 1024 * 1024;
pub use crate::remote_apps::CHUNK_BYTES;
pub const BURST_CHUNKS: usize = 16;
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Client {
    Read {
        drag: u64,
        offset: u32,
    },
    /// All preview bytes are installed in a willing native drag destination.
    Ready {
        drag: u64,
    },
    /// Only valid after the remote source has issued Drop.
    Complete {
        drag: u64,
        copied: bool,
    },
    Cancel {
        drag: u64,
    },
}
impl Client {
    pub fn id(&self) -> u64 {
        match self {
            Self::Read { drag, .. }
            | Self::Ready { drag }
            | Self::Complete { drag, .. }
            | Self::Cancel { drag } => *drag,
        }
    }
    pub fn validate(&self) -> Result<(), String> {
        if self.id() == 0 {
            return Err("Invalid reverse drag identity".into());
        }
        if let Self::Read { offset, .. } = self
            && *offset as usize >= MAX_BYTES
        {
            return Err("Invalid reverse drag offset".into());
        }
        Ok(())
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Server {
    Offer {
        drag: u64,
        gesture: u64,
        window: WindowId,
        surface: u64,
        mime: String,
        bytes: u32,
    },
    Chunk {
        drag: u64,
        offset: u32,
        data: bytes::Bytes,
    },
    Drop {
        drag: u64,
    },
    Cancel {
        drag: u64,
    },
    /// The exit of `gesture` can't apply: its window changed under the press,
    /// or the press ended. That gesture alone is refused, before any offer.
    ExitRefused {
        gesture: u64,
    },
}
impl Server {
    /// The drag the message is about; a refused exit has none yet.
    pub fn id(&self) -> Option<u64> {
        match self {
            Self::Offer { drag, .. }
            | Self::Chunk { drag, .. }
            | Self::Drop { drag }
            | Self::Cancel { drag } => Some(*drag),
            Self::ExitRefused { .. } => None,
        }
    }
    pub fn validate(&self) -> Result<(), String> {
        if self.id() == Some(0) || matches!(self, Self::ExitRefused { gesture: 0 }) {
            return Err("Invalid reverse drag identity".into());
        }
        match self {
            Self::Offer {
                gesture,
                window,
                surface,
                mime,
                bytes,
                ..
            } => {
                window.validate()?;
                if *gesture == 0
                    || *surface == 0
                    || *bytes as usize > MAX_BYTES
                    || mime.is_empty()
                    || mime.len() > 256
                    || !mime.bytes().all(|b| b.is_ascii_graphic())
                {
                    return Err("Invalid reverse drag offer".into());
                }
            }
            Self::Chunk { offset, data, .. }
                if (data.is_empty()
                    || data.len() > CHUNK_BYTES
                    || (*offset as usize)
                        .checked_add(data.len())
                        .is_none_or(|end| end > MAX_BYTES)) =>
            {
                return Err("Invalid reverse drag chunk".into());
            }
            _ => {}
        }
        Ok(())
    }
}
#[derive(Debug, PartialEq, Eq)]
pub enum Effect {
    Chunks(Vec<Server>),
    Ready,
    Complete { copied: bool },
    Cancelled,
}
/// One immutable preview snapshot. Owner manages visit deadlines, gesture
/// authorization and session identity; this machine fences byte and drop order.
pub struct Sender {
    id: u64,
    data: bytes::Bytes,
    offset: usize,
    ready: bool,
    dropped: bool,
    terminal: bool,
}
impl Sender {
    pub fn new(
        id: u64,
        gesture: u64,
        window: WindowId,
        surface: u64,
        mime: String,
        data: Vec<u8>,
    ) -> Result<(Self, Server), String> {
        if data.len() > MAX_BYTES {
            return Err("Reverse drag exceeds limit".into());
        }
        let offer = Server::Offer {
            drag: id,
            gesture,
            window,
            surface,
            mime,
            bytes: data.len() as u32,
        };
        offer.validate()?;
        Ok((
            Self {
                id,
                data: data.into(),
                offset: 0,
                ready: false,
                dropped: false,
                terminal: false,
            },
            offer,
        ))
    }
    pub fn accept(&mut self, message: Client) -> Result<Effect, String> {
        message.validate()?;
        if self.terminal || message.id() != self.id {
            return Err("Stale reverse drag message".into());
        }
        match message {
            Client::Read { offset, .. } => {
                if self.ready || offset as usize != self.offset || self.offset >= self.data.len() {
                    return Err("Unexpected reverse drag read".into());
                }
                let end = (self.offset + CHUNK_BYTES * BURST_CHUNKS).min(self.data.len());
                let mut chunks = Vec::new();
                while self.offset < end {
                    let next = (self.offset + CHUNK_BYTES).min(end);
                    chunks.push(Server::Chunk {
                        drag: self.id,
                        offset: self.offset as u32,
                        data: self.data.slice(self.offset..next),
                    });
                    self.offset = next;
                }
                Ok(Effect::Chunks(chunks))
            }
            Client::Ready { .. } => {
                if self.ready || self.offset != self.data.len() {
                    return Err("Preview is not ready".into());
                }
                self.ready = true;
                // Native peer now owns the preview snapshot. No second transfer
                // is retained here while waiting for the actual remote drop.
                self.data = bytes::Bytes::new();
                Ok(Effect::Ready)
            }
            Client::Complete { copied, .. } => {
                if !self.dropped {
                    return Err("Native receipt preceded remote drop".into());
                }
                self.terminal = true;
                Ok(Effect::Complete { copied })
            }
            Client::Cancel { .. } => {
                self.terminal = true;
                self.data = bytes::Bytes::new();
                Ok(Effect::Cancelled)
            }
        }
    }
    pub fn dropped(&mut self) -> Result<Server, String> {
        if self.terminal || !self.ready || self.dropped {
            return Err("Unexpected remote drop".into());
        }
        self.dropped = true;
        Ok(Server::Drop { drag: self.id })
    }
}
#[cfg(all(test, feature = "protobuf"))]
mod tests {
    use super::*;
    fn sender(data: Vec<u8>) -> Sender {
        Sender::new(
            7,
            1,
            WindowId {
                id: 9,
                generation: 3,
            },
            2,
            "text/plain;charset=utf-8".into(),
            data,
        )
        .unwrap()
        .0
    }
    #[test]
    fn bounded_credit_exact_bytes_and_drop_receipt_order() {
        let data: Vec<u8> = (0..(CHUNK_BYTES * BURST_CHUNKS + 17))
            .map(|i| i as u8)
            .collect();
        let mut s = sender(data.clone());
        assert!(s.dropped().is_err());
        assert!(s.accept(Client::Ready { drag: 7 }).is_err());
        assert!(s.accept(Client::Read { drag: 6, offset: 0 }).is_err());
        assert!(s.accept(Client::Read { drag: 7, offset: 1 }).is_err());
        let mut received = Vec::new();
        while received.len() < data.len() {
            let offset = received.len() as u32;
            let Effect::Chunks(chunks) = s.accept(Client::Read { drag: 7, offset }).unwrap() else {
                panic!()
            };
            assert!(chunks.len() <= BURST_CHUNKS);
            for chunk in chunks {
                chunk.validate().unwrap();
                let Server::Chunk { offset, data, .. } = chunk else {
                    panic!()
                };
                assert_eq!(offset as usize, received.len());
                received.extend(data);
            }
            assert!(s.accept(Client::Read { drag: 7, offset }).is_err());
        }
        assert_eq!(received, data);
        assert_eq!(s.accept(Client::Ready { drag: 7 }).unwrap(), Effect::Ready);
        assert!(s.accept(Client::Ready { drag: 7 }).is_err());
        assert!(
            s.accept(Client::Complete {
                drag: 7,
                copied: true
            })
            .is_err()
        );
        assert_eq!(s.dropped().unwrap(), Server::Drop { drag: 7 });
        assert!(s.dropped().is_err());
        assert_eq!(
            s.accept(Client::Complete {
                drag: 7,
                copied: true
            })
            .unwrap(),
            Effect::Complete { copied: true }
        );
        assert!(
            s.accept(Client::Complete {
                drag: 7,
                copied: true
            })
            .is_err()
        );
    }
    #[test]
    fn empty_preview_and_cancel_never_imply_success() {
        let mut s = sender(vec![]);
        assert!(s.accept(Client::Read { drag: 7, offset: 0 }).is_err());
        assert_eq!(s.accept(Client::Ready { drag: 7 }).unwrap(), Effect::Ready);
        assert_eq!(
            s.accept(Client::Cancel { drag: 7 }).unwrap(),
            Effect::Cancelled
        );
        assert!(s.dropped().is_err());
        assert!(
            s.accept(Client::Complete {
                drag: 7,
                copied: true
            })
            .is_err()
        );
        let mut s = sender(vec![1]);
        assert_eq!(
            s.accept(Client::Cancel { drag: 7 }).unwrap(),
            Effect::Cancelled
        );
        assert!(s.accept(Client::Read { drag: 7, offset: 0 }).is_err());
    }
    #[test]
    fn framed_reverse_drag_validates_before_writing() {
        use crate::remote_apps::v1::{ServerMessage, write_server};
        let message = ServerMessage::ReverseDrag {
            session: 4,
            message: Server::Chunk {
                drag: 7,
                offset: 0,
                data: vec![255; CHUNK_BYTES].into(),
            },
        };
        let mut wire = Vec::new();
        write_server(&mut wire, &message).unwrap();
        let packet = crate::remote_apps::Packet::read(&mut wire.as_slice()).unwrap();
        let decoded: ServerMessage = packet.message;
        assert_eq!(decoded.session(), 4);
        let mut rejected = Vec::new();
        assert!(
            write_server(
                &mut rejected,
                &ServerMessage::ReverseDrag {
                    session: 4,
                    message: Server::Chunk {
                        drag: 7,
                        offset: 0,
                        data: vec![1; CHUNK_BYTES + 1].into()
                    }
                }
            )
            .is_err()
        );
        assert!(rejected.is_empty());
    }
    #[test]
    fn wire_bounds_and_unknown_fields() {
        assert!(
            Server::Chunk {
                drag: 1,
                offset: u32::MAX,
                data: vec![0].into()
            }
            .validate()
            .is_err()
        );
        assert!(
            Server::Chunk {
                drag: 1,
                offset: 0,
                data: vec![0; CHUNK_BYTES + 1].into()
            }
            .validate()
            .is_err()
        );
        assert!(
            serde_json::from_str::<Client>(r#"{"kind":"ready","drag":1,"copied":true}"#).is_err()
        );
        let chunk = Server::Chunk {
            drag: u64::MAX,
            offset: 0,
            data: vec![255; CHUNK_BYTES].into(),
        };
        let bytes = serde_json::to_vec(&chunk).unwrap();
        assert_eq!(serde_json::from_slice::<Server>(&bytes).unwrap(), chunk);
    }
    #[test]
    fn a_refused_exit_names_its_gesture() {
        let refused: Server =
            serde_json::from_str(r#"{"kind":"exit_refused","gesture":3}"#).unwrap();
        assert_eq!(refused, Server::ExitRefused { gesture: 3 });
        assert_eq!(refused.id(), None);
        refused.validate().unwrap();
        assert!(Server::ExitRefused { gesture: 0 }.validate().is_err());
        assert!(
            serde_json::from_str::<Server>(r#"{"kind":"exit_refused","gesture":3,"drag":1}"#)
                .is_err()
        );
    }
}
