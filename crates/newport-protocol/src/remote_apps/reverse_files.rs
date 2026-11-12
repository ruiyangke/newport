//! File bytes for an authenticated reverse drag, distinct from URI preview text.
//! Only with `features.drag_out.files`. The owner must also match the active
//! gesture/window/surface before opening or staging files.
//! File transfer Ready means staged bytes, never an Xdnd/native drop receipt.
use crate::remote_apps::{file_upload, v1::WindowId};
use serde::{Deserialize, Serialize};
/// Chunks of file bytes that may await peer acknowledgment.
pub const WINDOW_CHUNKS: usize = 32;
pub const PREVIEW_MIME: &str = "application/x-newport-remote-files";
/// Hard bound for snapshot preparation and transfer before native dragging.
pub const PREPARATION_SECONDS: u64 = 120;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Context {
    pub drag: u64,
    pub gesture: u64,
    pub window: WindowId,
    pub surface: u64,
}
impl Context {
    pub fn validate(&self) -> Result<(), String> {
        self.window.validate()?;
        if self.drag == 0 || self.gesture == 0 || self.surface == 0 {
            return Err("Invalid reverse file context".into());
        }
        Ok(())
    }
}

/// The remote sender transmits manifests and bytes, never remote filesystem
/// paths. The existing transfer messages are reused with direction reversed.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Server {
    pub context: Context,
    pub message: file_upload::Client,
}
impl Server {
    pub fn validate(&self) -> Result<(), String> {
        self.context.validate()?;
        self.message.validate()?;
        if self.message.transfer() != self.context.drag {
            return Err("Reverse file identity mismatch".into());
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Client {
    pub context: Context,
    pub message: file_upload::Server,
}
impl Client {
    pub fn validate(&self) -> Result<(), String> {
        self.context.validate()?;
        self.message.validate()?;
        let id = match self.message {
            file_upload::Server::Ack { transfer, .. }
            | file_upload::Server::Ready { transfer }
            | file_upload::Server::Cancelled { transfer }
            | file_upload::Server::Rejected { transfer, .. } => transfer,
        };
        if id != self.context.drag {
            return Err("Reverse file identity mismatch".into());
        }
        Ok(())
    }
}

/// Validate before touching disk. A stale surface or drag cannot reuse another
/// transfer's credit or expose a completed batch from a previous gesture.
pub fn authorize(context: &Context, active: &Context) -> Result<(), String> {
    context.validate()?;
    active.validate()?;
    if context != active {
        return Err("Stale reverse drag context".into());
    }
    Ok(())
}

#[cfg(all(test, feature = "protobuf"))]
mod tests {
    use super::*;
    fn context() -> Context {
        Context {
            drag: 7,
            gesture: 4,
            window: WindowId {
                id: 9,
                generation: 3,
            },
            surface: 2,
        }
    }
    #[test]
    fn authorization_requires_the_entire_current_identity() {
        let active = context();
        authorize(&active, &active).unwrap();
        for field in 0..5 {
            let mut stale = active.clone();
            match field {
                0 => stale.drag += 1,
                1 => stale.gesture += 1,
                2 => stale.window.id += 1,
                3 => stale.window.generation += 1,
                _ => stale.surface += 1,
            }
            assert!(authorize(&stale, &active).is_err());
        }
    }
    #[test]
    fn bounds_identity_and_strict_fields_before_framing() {
        let data = vec![255; file_upload::CHUNK_BYTES].into();
        let server = Server {
            context: context(),
            message: file_upload::Client::Chunk {
                transfer: 7,
                file: 0,
                offset: 0,
                data,
            },
        };
        let message = crate::remote_apps::v1::ServerMessage::ReverseFiles {
            session: 1,
            message: server.clone(),
        };
        let mut bytes = Vec::new();
        crate::remote_apps::v1::write_server(&mut bytes, &message).unwrap();
        let packet = crate::remote_apps::Packet::read(&mut bytes.as_slice()).unwrap();
        let decoded: crate::remote_apps::v1::ServerMessage = packet.message;
        assert_eq!(decoded.session(), 1);
        let mut invalid = server.clone();
        invalid.context.drag += 1;
        let mut rejected = Vec::new();
        assert!(
            crate::remote_apps::v1::write_server(
                &mut rejected,
                &crate::remote_apps::v1::ServerMessage::ReverseFiles {
                    session: 1,
                    message: invalid
                }
            )
            .is_err()
        );
        assert!(rejected.is_empty());
        let mut value = serde_json::to_value(server).unwrap();
        value["path"] = "/remote/private".into();
        assert!(serde_json::from_value::<Server>(value).is_err());
        assert!(
            Client {
                context: context(),
                message: file_upload::Server::Ready { transfer: 8 }
            }
            .validate()
            .is_err()
        );
    }
}
