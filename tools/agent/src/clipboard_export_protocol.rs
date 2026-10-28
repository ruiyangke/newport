//! Optional host exports multiplexed on the existing integration channel.
//! Enabling this wire protocol grants no clipboard access. Runtime control
//! assigns the provider, export permission and focus independently.
use super::clipboard::{Chunk, CopyLease, ExportOffer, Origin};
use serde::{Deserialize, Serialize};
use std::io;

pub const CAPABILITY: &str = "clipboard-export";
pub const VERSION: u32 = 1;
pub const COPY_VERSION: u32 = 2;
pub const VERSIONS: &[u32] = &[VERSION, COPY_VERSION];
pub const FRAME: u8 = b'V';
pub const MAX_FRAME: usize = 72 * 1024;
pub const MAX_READS: usize = 8;
pub const CHUNK: usize = 64 * 1024;
pub const MAX_BYTES: usize = 32 * 1024 * 1024;

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase", deny_unknown_fields)]
pub enum Control {
    Enable {
        version: u32,
    },
    Disable,
    Read {
        id: u64,
        offer: ExportOffer,
        format: String,
    },
    Cancel {
        id: u64,
    },
    /// Version 2 only. Focus authorizes this acknowledgment, not later reads.
    /// A successful commit replaces the previous committed copy.
    Commit {
        id: u64,
        offer: ExportOffer,
    },
    Release {
        copy: CopyLease,
    },
    ReadCopy {
        id: u64,
        copy: CopyLease,
        format: String,
    },
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase", deny_unknown_fields)]
pub enum Event {
    /// Invalidates focused reads. Version 2 committed copies have their own
    /// lifetime, reported by Committed and CopyRevoked.
    Offer {
        offer: Option<ExportOffer>,
    },
    /// IDs never repeat within an integration channel. None is a failed read;
    /// consumers must discard the complete representation on any failure.
    Data {
        id: u64,
        offset: usize,
        chunk: Option<Chunk>,
    },
    /// Version 2 only. None rejects the proposal without replacing a prior copy.
    Committed {
        id: u64,
        copy: Option<CopyLease>,
    },
    CopyRevoked {
        id: String,
    },
}
fn invalid() -> io::Error {
    io::Error::other("Invalid clipboard export message")
}
fn format(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 256
        && !name.chars().any(char::is_control)
        && !name
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
}
fn offer(value: &ExportOffer) -> io::Result<()> {
    let source = &value.offer;
    if uuid::Uuid::parse_str(&source.runtime).is_err()
        || source.revision <= 0
        || value.route_revision == 0
        || source.formats.is_empty()
        || source.formats.len() > 256
        || source.formats.iter().any(|name| !format(name))
        || !matches!(&source.origin, Origin::Native { worker, revision }
            if *revision > 0 && !worker.is_empty() && worker.len() <= 64
                && worker.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'-'))
    {
        return Err(invalid());
    }
    let unique: std::collections::BTreeSet<_> = source.formats.iter().collect();
    if unique.len() != source.formats.len() {
        return Err(invalid());
    }
    Ok(())
}
fn copy(value: &CopyLease) -> io::Result<()> {
    if uuid::Uuid::parse_str(&value.id).is_err() {
        return Err(invalid());
    }
    offer(&value.offer)
}
fn encode(value: &impl Serialize) -> io::Result<Vec<u8>> {
    let bytes = crate::serialization::encode(value)?;
    if bytes.len() > MAX_FRAME {
        return Err(invalid());
    }
    Ok(bytes)
}
impl Control {
    fn validate(&self) -> io::Result<()> {
        match self {
            Self::Enable { version } if !VERSIONS.contains(version) => Err(invalid()),
            Self::Commit { id, offer: value } => {
                if *id == 0 {
                    return Err(invalid());
                }
                offer(value)
            }
            Self::Release { copy: value } => copy(value),
            Self::ReadCopy {
                id,
                copy: value,
                format: name,
            } => {
                copy(value)?;
                if *id == 0 || !value.offer.offer.formats.contains(name) {
                    return Err(invalid());
                }
                Ok(())
            }
            Self::Read {
                id,
                offer: value,
                format: name,
            } => {
                offer(value)?;
                if *id == 0 || !value.offer.formats.contains(name) {
                    return Err(invalid());
                }
                Ok(())
            }
            Self::Cancel { id: 0 } => Err(invalid()),
            _ => Ok(()),
        }
    }
    pub fn encode(&self) -> io::Result<Vec<u8>> {
        self.validate()?;
        encode(self)
    }
    pub fn decode(bytes: &[u8]) -> io::Result<Self> {
        let value: Self = crate::serialization::decode(bytes, MAX_FRAME)?;
        value.validate()?;
        Ok(value)
    }
}
impl Event {
    fn validate(&self) -> io::Result<()> {
        match self {
            Self::Committed { id, copy: value } => {
                if *id == 0 {
                    return Err(invalid());
                }
                value.as_ref().map(copy).transpose().map(|_| ())
            }
            Self::CopyRevoked { id } => {
                uuid::Uuid::parse_str(id).map(|_| ()).map_err(|_| invalid())
            }
            Self::Offer { offer: Some(value) } => offer(value),
            Self::Data { id, offset, chunk } => {
                if *id == 0 || *offset > MAX_BYTES {
                    return Err(invalid());
                }
                if chunk.as_ref().is_some_and(|chunk| {
                    chunk.bytes.len() > CHUNK
                        || (chunk.bytes.is_empty() && !chunk.eof)
                        || offset
                            .checked_add(chunk.bytes.len())
                            .is_none_or(|end| end > MAX_BYTES)
                }) {
                    return Err(invalid());
                }
                Ok(())
            }
            Self::Offer { offer: None } => Ok(()),
        }
    }
    pub fn encode(&self) -> io::Result<Vec<u8>> {
        self.validate()?;
        encode(self)
    }
    pub fn decode(bytes: &[u8]) -> io::Result<Self> {
        let value: Self = crate::serialization::decode(bytes, MAX_FRAME)?;
        value.validate()?;
        Ok(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> ExportOffer {
        ExportOffer {
            offer: super::super::clipboard::Offer {
                runtime: uuid::Uuid::new_v4().to_string(),
                revision: 1,
                origin: Origin::Native {
                    worker: "native".into(),
                    revision: 1,
                },
                formats: (0..256)
                    .map(|n| format!("application/{n:03}{}", "x".repeat(241)))
                    .collect(),
            },
            route_revision: 1,
        }
    }
    #[test]
    fn maximum_offer_and_chunk_fit_one_bounded_frame() {
        let offer = fixture();
        assert_eq!(offer.offer.formats[0].len(), 256);
        let control = Control::Read {
            id: 1,
            format: offer.offer.formats[0].clone(),
            offer: offer.clone(),
        };
        Control::decode(&control.encode().unwrap()).unwrap();
        Event::decode(&Event::Offer { offer: Some(offer) }.encode().unwrap()).unwrap();
        let data = Event::Data {
            id: 1,
            offset: MAX_BYTES - CHUNK,
            chunk: Some(Chunk {
                bytes: vec![255; CHUNK],
                eof: true,
            }),
        };
        Event::decode(&data.encode().unwrap()).unwrap();
    }
    #[test]
    fn copy_protocol_is_bounded_and_validates_identity_and_request_ids() {
        for version in VERSIONS {
            Control::decode(&Control::Enable { version: *version }.encode().unwrap()).unwrap();
        }
        let copy = CopyLease {
            id: uuid::Uuid::new_v4().to_string(),
            offer: fixture(),
        };
        for command in [
            Control::Commit {
                id: 1,
                offer: copy.offer.clone(),
            },
            Control::ReadCopy {
                id: 2,
                format: copy.offer.offer.formats[0].clone(),
                copy: copy.clone(),
            },
            Control::Release { copy: copy.clone() },
        ] {
            Control::decode(&command.encode().unwrap()).unwrap();
        }
        for event in [
            Event::Committed {
                id: 1,
                copy: Some(copy.clone()),
            },
            Event::Committed { id: 2, copy: None },
            Event::CopyRevoked {
                id: copy.id.clone(),
            },
        ] {
            Event::decode(&event.encode().unwrap()).unwrap();
        }
        assert!(
            Control::Commit {
                id: 0,
                offer: fixture()
            }
            .encode()
            .is_err()
        );
        assert!(Event::Committed { id: 0, copy: None }.encode().is_err());
        assert!(
            Control::ReadCopy {
                id: 1,
                copy: copy.clone(),
                format: "not/offered".into()
            }
            .encode()
            .is_err()
        );
        let mut forged = copy;
        forged.id = "not-a-copy-id".into();
        assert!(
            Control::Release {
                copy: forged.clone()
            }
            .encode()
            .is_err()
        );
        assert!(
            Event::Committed {
                id: 1,
                copy: Some(forged)
            }
            .encode()
            .is_err()
        );
        assert!(
            Event::CopyRevoked {
                id: "invalid".into()
            }
            .encode()
            .is_err()
        );
    }
    #[test]
    fn rejects_forged_origins_duplicate_formats_and_invalid_chunks() {
        let mut value = fixture();
        value.offer.origin = Origin::Host;
        assert!(Event::Offer { offer: Some(value) }.encode().is_err());
        let mut value = fixture();
        value.offer.formats[1] = value.offer.formats[0].clone();
        assert!(Event::Offer { offer: Some(value) }.encode().is_err());
        for (offset, bytes, eof) in [
            (0, vec![], false),
            (0, vec![0; CHUNK + 1], true),
            (MAX_BYTES, vec![0], true),
        ] {
            assert!(
                Event::Data {
                    id: 1,
                    offset,
                    chunk: Some(Chunk { bytes, eof })
                }
                .encode()
                .is_err()
            );
        }
        assert!(
            Control::Enable {
                version: COPY_VERSION + 1
            }
            .encode()
            .is_err()
        );
        let mut bytes = Control::Disable.encode().unwrap();
        bytes.push(0);
        assert!(Control::decode(&bytes).is_err());
        assert!(Event::decode(&vec![0; MAX_FRAME + 1]).is_err());
    }
}
