pub use crate::runtime_protocol::clipboard::{
    Access, Chunk, CopyLease, ExportOffer, NativeRequest, Offer, Origin, ProviderLease,
    ProviderPoll, Route,
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::{
    io::{self, Read, Write},
    os::unix::net::UnixStream,
    time::{Duration, Instant},
};

pub(super) const MAX_FRAME: usize = 72 * 1024;
pub(super) const MAX_CONNECTIONS: usize = 32;
#[derive(Clone, Serialize, Deserialize)]
pub(super) enum ExportRead {
    Focused(ExportOffer),
    Committed(CopyLease),
}
impl ExportRead {
    pub fn offer(&self) -> &ExportOffer {
        match self {
            Self::Focused(offer) => offer,
            Self::Committed(copy) => &copy.offer,
        }
    }
}
#[derive(Serialize, Deserialize)]
pub(super) enum Request {
    Authenticate(Access),
    Offer,
    Read {
        runtime: String,
        revision: i64,
        format: String,
        offset: usize,
        limit: usize,
        export: Option<ExportRead>,
    },
    Claim,
    Enabled(bool),
    Publish {
        revision: i64,
        formats: Vec<String>,
    },
    Poll,
    Reply(#[serde(with = "serde_bytes")] Vec<u8>),
    NativePoll,
    NativePublish {
        lease: ProviderLease,
        expected_runtime: String,
        expected_revision: i64,
        revision: u64,
        formats: Vec<String>,
    },
    NativeReply {
        request: NativeRequest,
        chunk: Option<Chunk>,
    },
    ExportOffer,
    CommitCopy(ExportOffer),
    CopyStatus,
    ReleaseCopy(CopyLease),
}
#[derive(Serialize, Deserialize)]
pub(super) enum Response {
    Ready,
    Offer(Offer),
    Chunk(Chunk),
    Requests(Vec<(u64, i64, String)>),
    Provider(ProviderPoll),
    Published(bool),
    Export(Option<ExportOffer>),
    Copy(Option<CopyLease>),
    Error(String),
}
pub(super) fn write(stream: &mut impl Write, value: &impl Serialize) -> io::Result<()> {
    let bytes = crate::serialization::encode(value)?;
    if bytes.len() > MAX_FRAME {
        return Err(io::Error::other("Clipboard IPC frame exceeds limit"));
    }
    stream.write_all(&(bytes.len() as u32).to_be_bytes())?;
    stream.write_all(&bytes)
}
pub(super) fn read<T: DeserializeOwned>(stream: &mut impl Read) -> io::Result<T> {
    let mut header = [0; 4];
    stream.read_exact(&mut header)?;
    let count = u32::from_be_bytes(header) as usize;
    if count == 0 || count > MAX_FRAME {
        return Err(io::Error::other("Invalid clipboard IPC frame length"));
    }
    let mut bytes = vec![0; count];
    stream.read_exact(&mut bytes)?;
    crate::serialization::decode(&bytes, MAX_FRAME)
}
pub(super) struct Deadline<'a> {
    stream: &'a mut UnixStream,
    until: Instant,
}
impl<'a> Deadline<'a> {
    pub fn new(stream: &'a mut UnixStream, timeout: Duration) -> Self {
        Self {
            stream,
            until: Instant::now() + timeout,
        }
    }
    fn remaining(&self) -> io::Result<Duration> {
        let left = self.until.saturating_duration_since(Instant::now());
        if left.is_zero() {
            Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "Clipboard IPC deadline expired",
            ))
        } else {
            Ok(left)
        }
    }
}
impl Read for Deadline<'_> {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        self.stream.set_read_timeout(Some(self.remaining()?))?;
        self.stream.read(bytes)
    }
}
impl Write for Deadline<'_> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.stream.set_write_timeout(Some(self.remaining()?))?;
        self.stream.write(bytes)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.stream.flush()
    }
}
