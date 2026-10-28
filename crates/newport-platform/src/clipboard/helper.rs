//! Supervised native clipboard helper. All client operations, including Drop,
//! belong on a background supervisor, never the application's UI thread. This
//! owns no SSH connection and grants no sharing permission: the host router
//! maps each monotonically increasing generation to an exact authorized offer.
pub mod owner;
mod server;
mod wire;
use std::io;
use std::net::Shutdown;
use std::os::fd::{FromRawFd, OwnedFd};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::process::{Child, Command as Process, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
pub use wire::{Board, CHUNK, Command, Event, VERSION};

pub const ARGUMENT: &str = "--clipboard-provider";

/// A bounded wire event. Only LocalData may carry bytes; callers must assemble
/// them against a requested ID/revision and retain the connection/source fence.
pub struct Packet {
    pub event: Event,
    pub bytes: Vec<u8>,
}

/// Run only as the desktop's early helper branch, before profile/credential/UI
/// startup. A valid handshake over an inherited Unix socket is required before
/// opening any native pasteboard; an ordinary terminal stdin is rejected.
pub fn run_from_stdio() -> Result<(), String> {
    // Validate even a closed stdin without constructing a BorrowedFd for an
    // invalid descriptor. The successful duplicate is exclusively ours.
    let fd = unsafe { libc::fcntl(0, libc::F_DUPFD_CLOEXEC, 3) };
    if fd < 0 {
        return Err(io::Error::last_os_error().to_string());
    }
    let owned = unsafe { OwnedFd::from_raw_fd(fd) };
    let stream = UnixStream::from(owned);
    stream
        .peer_addr()
        .map_err(|_| "Clipboard helper requires an inherited Unix socket")?;
    server::run(stream).map_err(|e| e.to_string())
}

#[derive(Clone)]
pub struct Sender(Arc<Mutex<UnixStream>>);
impl Sender {
    /// One bounded frame per lock acquisition. Control/revocation messages may
    /// interleave between payload chunks. Any interrupted frame closes the
    /// entire connection so it cannot be mistaken for another message.
    pub fn send(&self, command: &Command, bytes: &[u8]) -> io::Result<()> {
        let mut stream = self.0.lock().unwrap_or_else(|e| e.into_inner());
        let result = wire::send(&mut stream, command, bytes);
        if result.is_err() {
            let _ = stream.shutdown(Shutdown::Both);
        }
        result
    }
}

pub struct Client {
    child: Child,
    stream: UnixStream,
    sender: Sender,
}
impl Client {
    pub fn spawn(executable: &Path, board: Board) -> io::Result<Self> {
        let (stream, child_stream) = UnixStream::pair()?;
        let input: OwnedFd = child_stream.try_clone()?.into();
        let output: OwnedFd = child_stream.into();
        let sender = Sender(Arc::new(Mutex::new(stream.try_clone()?)));
        let child = Process::new(executable)
            .arg(ARGUMENT)
            .stdin(Stdio::from(input))
            .stdout(Stdio::from(output))
            .stderr(Stdio::inherit())
            .spawn()?;
        let mut client = Self {
            child,
            stream,
            sender,
        };
        client.sender.send(
            &Command::Hello {
                version: VERSION,
                board,
            },
            &[],
        )?;
        match client.receive(Duration::from_secs(3))? {
            Some(Event::Ready { version: VERSION }) => Ok(client),
            _ => Err(io::Error::other("Clipboard helper handshake failed")),
        }
    }
    pub fn sender(&self) -> Sender {
        self.sender.clone()
    }
    pub fn process_id(&self) -> u32 {
        self.child.id()
    }
    pub fn receive(&mut self, idle: Duration) -> io::Result<Option<Event>> {
        match self.receive_packet(idle)? {
            Some(packet) if packet.bytes.is_empty() => Ok(Some(packet.event)),
            Some(_) => {
                let _ = self.stream.shutdown(Shutdown::Both);
                Err(io::Error::other(
                    "Use receive_packet for local clipboard reads",
                ))
            }
            None => Ok(None),
        }
    }
    pub fn receive_packet(&mut self, idle: Duration) -> io::Result<Option<Packet>> {
        match wire::receive::<Event>(&mut self.stream, idle) {
            Ok(Some(frame))
                if frame.bytes.is_empty() || matches!(frame.header, Event::LocalData { .. }) =>
            {
                Ok(Some(Packet {
                    event: frame.header,
                    bytes: frame.bytes,
                }))
            }
            Ok(None) => Ok(None),
            Ok(Some(_)) => {
                let _ = self.stream.shutdown(Shutdown::Both);
                Err(io::Error::other("Unexpected helper event bytes"))
            }
            Err(e) => {
                let _ = self.stream.shutdown(Shutdown::Both);
                Err(e)
            }
        }
    }
}
impl Drop for Client {
    fn drop(&mut self) {
        // EOF reaches the independent control reader even while AppKit waits.
        // Give the native loop time to withdraw, then bound a wedged helper.
        let _ = self.stream.shutdown(Shutdown::Both);
        let deadline = Instant::now() + Duration::from_secs(1);
        loop {
            match self.child.try_wait() {
                Ok(Some(_)) => return,
                Ok(None) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(10))
                }
                Ok(None) => {
                    let _ = self.child.kill();
                    let _ = self.child.wait();
                    return;
                }
                // Do not signal a possibly reused PID after ownership/reaping
                // can no longer be established (for example ECHILD).
                Err(_) => return,
            }
        }
    }
}
