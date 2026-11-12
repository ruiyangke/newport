//! Revision-tagged host clipboard offers and bounded demand replies.
use serde::{Deserialize, Serialize};
use std::{collections::BTreeSet, fmt};

#[cfg(feature = "compression")]
pub mod compression;

pub const CHUNK: usize = 64 * 1024;
pub const LIMIT: usize = 32 * 1024 * 1024;
pub const MAX_FORMATS: usize = 256;
pub const MAX_FORMAT_BYTES: usize = 256;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Offer {
    pub revision: i64,
    pub formats: Vec<String>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Request {
    pub id: u64,
    pub revision: i64,
    pub format: String,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Encoding {
    Raw,
    Failed,
    Zlib,
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reply {
    pub id: u64,
    pub revision: i64,
    pub encoding: Encoding,
    pub done: bool,
    #[serde(with = "serde_bytes")]
    pub bytes: Vec<u8>,
}
impl fmt::Debug for Reply {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ClipboardReply")
            .field("id", &self.id)
            .field("revision", &self.revision)
            .field("encoding", &self.encoding)
            .field("done", &self.done)
            .field("length", &self.bytes.len())
            .finish()
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Invalid;
impl fmt::Display for Invalid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Invalid host clipboard message")
    }
}
impl std::error::Error for Invalid {}
impl From<Invalid> for std::io::Error {
    fn from(value: Invalid) -> Self {
        Self::new(std::io::ErrorKind::InvalidData, value)
    }
}
pub fn valid_format(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_FORMAT_BYTES
        && !value.chars().any(char::is_control)
        && !value
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
}
impl Offer {
    pub fn validate(&self) -> Result<(), Invalid> {
        if self.formats.len() > MAX_FORMATS
            || self.formats.iter().any(|format| !valid_format(format))
            || self.formats.iter().collect::<BTreeSet<_>>().len() != self.formats.len()
        {
            return Err(Invalid);
        }
        Ok(())
    }
}
impl Request {
    pub fn validate(&self) -> Result<(), Invalid> {
        if self.id == 0 || !valid_format(&self.format) {
            return Err(Invalid);
        }
        Ok(())
    }
}
impl Reply {
    pub fn new(
        request: &Request,
        encoding: Encoding,
        done: bool,
        bytes: impl Into<Vec<u8>>,
    ) -> Self {
        Self {
            id: request.id,
            revision: request.revision,
            encoding,
            done,
            bytes: bytes.into(),
        }
    }
    pub fn validate(&self) -> Result<(), Invalid> {
        if self.id == 0
            || self.bytes.len() > CHUNK
            || (self.encoding == Encoding::Failed && (!self.done || !self.bytes.is_empty()))
        {
            return Err(Invalid);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_and_collections_are_bounded_before_consumption() {
        for format in ["", "../text", "image/\0", "text/", "/plain", "text/./plain"] {
            assert!(!valid_format(format));
            assert!(
                Offer {
                    revision: i64::MIN,
                    formats: vec![format.into()]
                }
                .validate()
                .is_err()
            );
            assert!(
                Request {
                    id: u64::MAX,
                    revision: i64::MIN,
                    format: format.into()
                }
                .validate()
                .is_err()
            );
        }
        assert!(!valid_format(&"x".repeat(MAX_FORMAT_BYTES + 1)));
        assert!(
            Offer {
                revision: 1,
                formats: vec!["text/plain".into(); 2]
            }
            .validate()
            .is_err()
        );
        let formats = (0..MAX_FORMATS)
            .map(|id| format!("application/format-{id}"))
            .collect();
        let mut offer = Offer {
            revision: i64::MIN,
            formats,
        };
        offer.validate().unwrap();
        offer.formats.push("text/plain".into());
        assert!(offer.validate().is_err());
    }

    #[test]
    fn failed_replies_are_distinct_from_empty_data_and_keep_alives() {
        let request = Request {
            id: u64::MAX,
            revision: i64::MIN,
            format: "image/png".into(),
        };
        request.validate().unwrap();
        for (encoding, done) in [
            (Encoding::Raw, false),
            (Encoding::Raw, true),
            (Encoding::Failed, true),
        ] {
            Reply::new(&request, encoding, done, vec![])
                .validate()
                .unwrap();
        }
        assert!(
            Reply::new(&request, Encoding::Failed, false, vec![])
                .validate()
                .is_err()
        );
        assert!(
            Reply::new(&request, Encoding::Failed, true, vec![0])
                .validate()
                .is_err()
        );
        assert!(
            Reply::new(&request, Encoding::Raw, true, vec![0; CHUNK + 1])
                .validate()
                .is_err()
        );
    }
}
