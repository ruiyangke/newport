use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::io;
use std::os::fd::AsRawFd;
use std::os::unix::net::UnixStream;
use std::time::{Duration, Instant};

pub const VERSION: u32 = 1;
pub const MAX_HEADER: usize = 128 * 1024;
pub const CHUNK: usize = 64 * 1024;
const FRAME_TIME: Duration = Duration::from_secs(2);

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum Board {
    General {},
    /// Only unique disposable fixture names are accepted by the helper.
    Test {
        name: String,
    },
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase", deny_unknown_fields)]
pub enum Command {
    Hello {
        version: u32,
        board: Board,
    },
    Publish {
        generation: u64,
        expected_revision: isize,
        formats: Vec<String>,
    },
    /// Replace the current publication's promises with the files in `folder`
    /// (a folder relative to the clipboard file cache, not a path list, to
    /// stay under `MAX_HEADER`).
    AttachFiles {
        generation: u64,
        expected_revision: isize,
        folder: String,
    },
    Revoke {
        generation: u64,
    },
    Data {
        generation: u64,
        id: u64,
        offset: usize,
        eof: bool,
    },
    Failed {
        generation: u64,
        id: u64,
    },
    ReadLocal {
        id: u64,
        revision: isize,
        format: String,
    },
    CancelLocal {
        id: u64,
    },
    Stop {},
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase", deny_unknown_fields)]
pub enum Event {
    Ready {
        version: u32,
    },
    Snapshot {
        revision: isize,
        formats: Vec<String>,
        origin: Option<u64>,
    },
    Published {
        generation: u64,
        revision: isize,
    },
    /// Answers every `AttachFiles`; `revision` is `None` on refusal.
    Attached {
        generation: u64,
        revision: Option<isize>,
    },
    Rejected {
        generation: u64,
    },
    Revoked {
        generation: u64,
        revision: Option<isize>,
    },
    Read {
        generation: u64,
        id: u64,
        format: String,
    },
    Cancelled {
        generation: u64,
        id: u64,
    },
    LocalData {
        id: u64,
        revision: isize,
        offset: usize,
        eof: bool,
    },
    LocalFailed {
        id: u64,
    },
}

pub struct Frame<T> {
    pub header: T,
    pub bytes: Vec<u8>,
}

fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

/// Timeout before the first byte is harmless. Once a frame has started, any
/// timeout/truncation poisons the stream; callers must close it, never retry the
/// next header from the middle of the abandoned frame.
pub fn receive<T: DeserializeOwned>(
    stream: &mut UnixStream,
    idle: Duration,
) -> io::Result<Option<Frame<T>>> {
    // All I/O on this private socket uses poll. Darwin can block a large send
    // despite MSG_DONTWAIT, so the descriptor itself must stay nonblocking too.
    stream.set_nonblocking(true)?;
    let mut prefix = [0; 8];
    match read_some(
        stream,
        &mut prefix[..1],
        Instant::now() + idle.max(Duration::from_millis(1)),
    ) {
        Ok(0) => return Err(io::ErrorKind::UnexpectedEof.into()),
        Ok(_) => {}
        Err(e)
            if matches!(
                e.kind(),
                io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
            ) =>
        {
            return Ok(None);
        }
        Err(e) => return Err(e),
    }
    let deadline = Instant::now() + FRAME_TIME;
    read_until(stream, &mut prefix[1..], deadline)?;
    let header_len = u32::from_be_bytes(prefix[..4].try_into().unwrap()) as usize;
    let data_len = u32::from_be_bytes(prefix[4..].try_into().unwrap()) as usize;
    if header_len == 0 || header_len > MAX_HEADER || data_len > CHUNK {
        return Err(invalid("Clipboard helper frame limit"));
    }
    let mut header = vec![0; header_len];
    read_until(stream, &mut header, deadline)?;
    let header =
        serde_json::from_slice(&header).map_err(|_| invalid("Invalid clipboard helper header"))?;
    let mut bytes = vec![0; data_len];
    read_until(stream, &mut bytes, deadline)?;
    Ok(Some(Frame { header, bytes }))
}

fn read_until(stream: &mut UnixStream, mut bytes: &mut [u8], deadline: Instant) -> io::Result<()> {
    while !bytes.is_empty() {
        match read_some(stream, bytes, deadline) {
            Ok(0) => return Err(io::ErrorKind::UnexpectedEof.into()),
            Ok(n) => bytes = &mut bytes[n..],
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

// Darwin rejects SO_RCVTIMEO updates after the peer closes, including when
// unread bytes remain. Poll on a consistently nonblocking socket preserves
// frame deadlines and drains those bytes without updating socket timeouts.
fn ready(stream: &UnixStream, events: libc::c_short, deadline: Instant) -> io::Result<()> {
    loop {
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .ok_or(io::ErrorKind::TimedOut)?;
        let ms = remaining
            .as_millis()
            .saturating_add(1)
            .min(i32::MAX as u128) as i32;
        let mut fd = libc::pollfd {
            fd: stream.as_raw_fd(),
            events,
            revents: 0,
        };
        // One initialized descriptor, live for this entire call.
        let result = unsafe { libc::poll(&mut fd, 1, ms) };
        if result > 0 {
            return Ok(());
        }
        if result == 0 {
            return Err(io::ErrorKind::TimedOut.into());
        }
        let error = io::Error::last_os_error();
        if error.kind() != io::ErrorKind::Interrupted {
            return Err(error);
        }
    }
}

fn read_some(stream: &UnixStream, bytes: &mut [u8], deadline: Instant) -> io::Result<usize> {
    loop {
        ready(stream, libc::POLLIN, deadline)?;
        // The writable slice and stream remain live; recv initializes only its
        // reported byte count. Every reader/writer uses nonblocking I/O.
        let count = unsafe {
            libc::recv(
                stream.as_raw_fd(),
                bytes.as_mut_ptr().cast(),
                bytes.len(),
                libc::MSG_DONTWAIT,
            )
        };
        if count >= 0 {
            return Ok(count as usize);
        }
        let error = io::Error::last_os_error();
        if !matches!(
            error.kind(),
            io::ErrorKind::Interrupted | io::ErrorKind::WouldBlock
        ) {
            return Err(error);
        }
    }
}

fn write_some(stream: &UnixStream, bytes: &[u8], deadline: Instant) -> io::Result<usize> {
    loop {
        ready(stream, libc::POLLOUT, deadline)?;
        // MSG_NOSIGNAL converts a dead peer to an error rather than terminating
        // the application. O_NONBLOCK is also enabled at every entry point.
        let count = unsafe {
            libc::send(
                stream.as_raw_fd(),
                bytes.as_ptr().cast(),
                bytes.len(),
                libc::MSG_DONTWAIT | libc::MSG_NOSIGNAL,
            )
        };
        if count >= 0 {
            return Ok(count as usize);
        }
        let error = io::Error::last_os_error();
        if !matches!(
            error.kind(),
            io::ErrorKind::Interrupted | io::ErrorKind::WouldBlock
        ) {
            return Err(error);
        }
    }
}

pub fn send<T: Serialize>(stream: &mut UnixStream, header: &T, bytes: &[u8]) -> io::Result<()> {
    stream.set_nonblocking(true)?;
    let header = serde_json::to_vec(header)
        .map_err(|_| invalid("Could not encode clipboard helper header"))?;
    if header.len() > MAX_HEADER || bytes.len() > CHUNK {
        return Err(invalid("Clipboard helper frame limit"));
    }
    let mut prefix = [0; 8];
    prefix[..4].copy_from_slice(&(header.len() as u32).to_be_bytes());
    prefix[4..].copy_from_slice(&(bytes.len() as u32).to_be_bytes());
    let deadline = Instant::now() + FRAME_TIME;
    for part in [&prefix[..], &header, bytes] {
        let mut part = part;
        while !part.is_empty() {
            match write_some(stream, part, deadline) {
                Ok(0) => return Err(io::ErrorKind::WriteZero.into()),
                Ok(n) => part = &part[n..],
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(e) => return Err(e),
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    #[test]
    fn fragmented_frames_and_idle_timeout_preserve_boundaries() {
        let (mut a, mut b) = UnixStream::pair().unwrap();
        assert!(
            receive::<Command>(&mut b, Duration::from_millis(1))
                .unwrap()
                .is_none()
        );
        let producer = std::thread::spawn(move || {
            let json = br#"{"type":"data","generation":1,"id":1,"offset":0,"eof":true}"#;
            let mut frame = Vec::new();
            frame.extend_from_slice(&(json.len() as u32).to_be_bytes());
            frame.extend_from_slice(&3u32.to_be_bytes());
            frame.extend_from_slice(json);
            frame.extend_from_slice(b"abc");
            for byte in frame {
                a.write_all(&[byte]).unwrap();
            }
        });
        let frame = receive::<Command>(&mut b, Duration::from_secs(1))
            .unwrap()
            .unwrap();
        assert!(matches!(
            frame.header,
            Command::Data {
                id: 1,
                eof: true,
                ..
            }
        ));
        assert_eq!(frame.bytes, b"abc");
        producer.join().unwrap();
    }

    #[test]
    fn attach_files_round_trips_and_rejects_unknown_fields() {
        let command = serde_json::to_string(&Command::AttachFiles {
            generation: 3,
            expected_revision: 9,
            folder: "folder".into(),
        })
        .unwrap();
        assert_eq!(
            command,
            r#"{"type":"attachFiles","generation":3,"expected_revision":9,"folder":"folder"}"#
        );
        assert!(matches!(
            serde_json::from_str::<Command>(&command).unwrap(),
            Command::AttachFiles {
                generation: 3,
                expected_revision: 9,
                ..
            }
        ));
        assert!(serde_json::from_str::<Command>(
            r#"{"type":"attachFiles","generation":3,"expected_revision":9,"folder":"f","paths":[]}"#
        )
        .is_err());
        let event = serde_json::to_string(&Event::Attached {
            generation: 3,
            revision: None,
        })
        .unwrap();
        assert_eq!(
            event,
            r#"{"type":"attached","generation":3,"revision":null}"#
        );
    }

    #[test]
    fn oversized_lengths_unknown_fields_and_truncation_are_rejected() {
        for bytes in [
            [((MAX_HEADER + 1) as u32).to_be_bytes(), 0u32.to_be_bytes()].concat(),
            [1u32.to_be_bytes(), ((CHUNK + 1) as u32).to_be_bytes()].concat(),
            vec![0, 0, 0, 2],
        ] {
            let (mut a, mut b) = UnixStream::pair().unwrap();
            a.write_all(&bytes).unwrap();
            drop(a);
            assert!(receive::<Command>(&mut b, Duration::from_secs(1)).is_err());
        }
        assert!(serde_json::from_str::<Command>(r#"{"type":"stop","unexpected":1}"#).is_err());
        assert!(serde_json::from_str::<Board>(r#"{"kind":"general","unexpected":1}"#).is_err());
    }

    #[test]
    fn partial_frame_timeout_and_unread_peer_have_absolute_deadlines() {
        let (mut a, mut b) = UnixStream::pair().unwrap();
        a.write_all(&[0]).unwrap();
        let start = Instant::now();
        let error = receive::<Command>(&mut b, Duration::from_millis(5))
            .err()
            .unwrap();
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
        assert!(start.elapsed() < Duration::from_secs(4));
        let start = Instant::now();
        let bytes = vec![0; CHUNK];
        let mut timed_out = false;
        for _ in 0..128 {
            if let Err(error) = send(
                &mut a,
                &Command::Data {
                    generation: 1,
                    id: 1,
                    offset: 0,
                    eof: false,
                },
                &bytes,
            ) {
                assert_eq!(error.kind(), io::ErrorKind::TimedOut);
                timed_out = true;
                break;
            }
        }
        assert!(timed_out);
        assert!(start.elapsed() < Duration::from_secs(4));
    }
}
