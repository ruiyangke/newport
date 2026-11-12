//! Process controls shared by worker transports. Service messages keep their
//! own contracts; these controls are consumed by the transport adapters.
use bytes::Bytes;
use std::fmt;
pub mod admission;
mod transport;
pub use transport::Transport;

#[cfg(feature = "protobuf")]
pub mod wire;

/// The largest opaque stream or diagnostic chunk.
pub const MAX_CHUNK: usize = 32 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    ToWorker,
    FromWorker,
}

/// A Linux signal number, independent of the desktop's native signal constants.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Signal(u32);

impl Signal {
    pub fn new(value: u32) -> Result<Self, InvalidSignal> {
        (1..=64)
            .contains(&value)
            .then_some(Self(value))
            .ok_or(InvalidSignal)
    }

    pub fn number(self) -> u32 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InvalidSignal;

impl fmt::Display for InvalidSignal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("invalid worker signal")
    }
}
impl std::error::Error for InvalidSignal {}

/// A bounded, nonempty chunk whose contents are excluded from diagnostics.
#[derive(Clone, PartialEq, Eq)]
pub struct Chunk(Bytes);

impl Chunk {
    pub fn new(data: Bytes) -> Result<Self, InvalidChunk> {
        if data.is_empty() || data.len() > MAX_CHUNK {
            return Err(InvalidChunk);
        }
        Ok(Self(data))
    }

    pub fn data(&self) -> &Bytes {
        &self.0
    }

    pub fn into_data(self) -> Bytes {
        self.0
    }
}

impl fmt::Debug for Chunk {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Chunk")
            .field("length", &self.0.len())
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InvalidChunk;

impl fmt::Display for InvalidChunk {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("invalid worker chunk length")
    }
}
impl std::error::Error for InvalidChunk {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Exit {
    Status(u32),
    Signal(Signal),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClientControl {
    /// Close the worker's stdin while retaining stdout, stderr and exit status.
    /// Closing the transport itself cancels the worker.
    InputComplete,
    Signal(Signal),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ServerControl {
    Stderr(Chunk),
    Exit(Exit),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunks_are_bounded_shared_and_redacted() {
        let bytes = Bytes::from_static(b"private diagnostic");
        let chunk = Chunk::new(bytes.clone()).unwrap();
        assert_eq!(chunk.data().as_ptr(), bytes.as_ptr());
        assert_eq!(chunk.clone().into_data().as_ptr(), bytes.as_ptr());
        assert!(!format!("{chunk:?}").contains("private"));
        assert!(Chunk::new(Bytes::new()).is_err());
        assert!(Chunk::new(vec![0; MAX_CHUNK].into()).is_ok());
        assert!(Chunk::new(vec![0; MAX_CHUNK + 1].into()).is_err());
    }

    #[test]
    fn signals_use_linux_numbers_on_every_platform() {
        for value in [1, 9, 15, 64] {
            assert_eq!(Signal::new(value).unwrap().number(), value);
        }
        for value in [0, 65, u32::MAX] {
            assert_eq!(Signal::new(value), Err(InvalidSignal));
        }
    }
}
