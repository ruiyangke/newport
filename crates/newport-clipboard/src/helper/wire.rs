pub use newport_protocol::clipboard::helper::CHUNK;
use newport_protocol::{
    clipboard::helper::{
        Frame, LIMITS,
        wire::{self as codec, Message},
    },
    encoding::protobuf,
};
use std::{
    io::{self, Read},
    os::{fd::AsRawFd, unix::net::UnixStream},
    time::{Duration, Instant},
};
const FRAME_TIME: Duration = Duration::from_secs(2);
struct Reader<'a> {
    stream: &'a UnixStream,
    deadline: Instant,
}
impl Read for Reader<'_> {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        read_some(self.stream, bytes, self.deadline)
    }
}
/// An idle timeout consumes nothing. Any failure after the first byte retires
/// the caller's connection, retaining one deadline for the whole frame.
pub fn receive<T: Message>(
    stream: &mut UnixStream,
    idle: Duration,
) -> io::Result<Option<Frame<T>>> {
    stream.set_nonblocking(true)?;
    let mut first = [0];
    if !first_byte(read_some(
        stream,
        &mut first,
        Instant::now() + idle.max(Duration::from_millis(1)),
    ))? {
        return Ok(None);
    }
    let mut input = first.as_slice().chain(Reader {
        stream,
        deadline: Instant::now() + FRAME_TIME,
    });
    let body = protobuf::read_frame(&mut input, LIMITS.message)
        .map_err(codec::Error::from)?
        .ok_or(io::ErrorKind::UnexpectedEof)?;
    codec::decode(body).map(Some).map_err(io::Error::from)
}
fn first_byte(result: io::Result<usize>) -> io::Result<bool> {
    match result {
        Ok(0) => Err(io::ErrorKind::UnexpectedEof.into()),
        Ok(_) => Ok(true),
        Err(error)
            if matches!(
                error.kind(),
                io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
            ) && !error.get_ref().is_some_and(|cause| cause.is::<LateRead>()) =>
        {
            Ok(false)
        }
        Err(error) => Err(error),
    }
}
pub fn send<T: Message>(stream: &mut UnixStream, header: &T, bytes: &[u8]) -> io::Result<()> {
    let frame = codec::encode(header, bytes)?;
    stream.set_nonblocking(true)?;
    let deadline = Instant::now() + FRAME_TIME;
    let mut part = frame.as_slice();
    while !part.is_empty() {
        match write_some(stream, part, deadline) {
            Ok(0) => return Err(io::ErrorKind::WriteZero.into()),
            Ok(count) => part = &part[count..],
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => return Err(error),
        }
    }
    Ok(())
}
#[derive(Debug)]
struct LateRead;
impl std::fmt::Display for LateRead {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("Clipboard helper deadline elapsed after consuming bytes")
    }
}
impl std::error::Error for LateRead {}
fn check_deadline(deadline: Instant) -> io::Result<()> {
    if Instant::now() >= deadline {
        Err(io::ErrorKind::TimedOut.into())
    } else {
        Ok(())
    }
}
fn completed_read(count: usize, deadline: Instant) -> io::Result<usize> {
    if let Err(error) = check_deadline(deadline) {
        // Once bytes were consumed, an idle retry would discard the prefix and
        // desynchronize the connection. Keep that distinction in the cause.
        return if count == 0 {
            Err(error)
        } else {
            Err(io::Error::new(io::ErrorKind::TimedOut, LateRead))
        };
    }
    Ok(count)
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
        // SAFETY: One initialized descriptor, live for this entire call.
        let result = unsafe { libc::poll(&mut fd, 1, ms) };
        if result > 0 {
            return check_deadline(deadline);
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
        // SAFETY: The writable slice and stream remain live; recv initializes only its
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
            return completed_read(count as usize, deadline);
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
        // SAFETY: The slice and stream remain live for the call. MSG_NOSIGNAL turns a dead
        // peer into an error rather than a signal, and O_NONBLOCK is set at every entry point.
        let count = unsafe {
            libc::send(
                stream.as_raw_fd(),
                bytes.as_ptr().cast(),
                bytes.len(),
                libc::MSG_DONTWAIT | libc::MSG_NOSIGNAL,
            )
        };
        if count >= 0 {
            check_deadline(deadline)?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use newport_protocol::clipboard::helper::{Command, Event};
    use std::io::Write;
    #[test]
    fn a_late_consumed_first_byte_is_not_an_idle_timeout() {
        let expired = Instant::now();
        let error = first_byte(completed_read(1, expired)).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
        assert!(error.get_ref().unwrap().is::<LateRead>());
        assert!(!first_byte(Err(io::ErrorKind::TimedOut.into())).unwrap());
        assert!(!first_byte(Err(io::ErrorKind::WouldBlock.into())).unwrap());
        assert!(first_byte(completed_read(1, Instant::now() + FRAME_TIME)).unwrap());
    }
    #[test]
    fn expired_readiness_does_not_consume_or_send_bytes() {
        let (mut peer, stream) = UnixStream::pair().unwrap();
        peer.write_all(b"x").unwrap();
        let mut byte = [0];
        assert_eq!(
            read_some(&stream, &mut byte, Instant::now())
                .unwrap_err()
                .kind(),
            io::ErrorKind::TimedOut
        );
        assert_eq!(
            read_some(&stream, &mut byte, Instant::now() + FRAME_TIME).unwrap(),
            1
        );
        assert_eq!(byte, *b"x");
        assert_eq!(
            write_some(&stream, b"y", Instant::now())
                .unwrap_err()
                .kind(),
            io::ErrorKind::TimedOut
        );
        peer.set_nonblocking(true).unwrap();
        assert_eq!(
            peer.read(&mut byte).unwrap_err().kind(),
            io::ErrorKind::WouldBlock
        );
    }
    #[test]
    fn fragmented_frames_and_idle_timeout_preserve_boundaries() {
        let (mut writer, mut reader) = UnixStream::pair().unwrap();
        assert!(
            receive::<Command>(&mut reader, Duration::from_millis(1))
                .unwrap()
                .is_none()
        );
        let producer = std::thread::spawn(move || {
            let bytes = codec::encode(
                &Command::Data {
                    generation: 1,
                    id: 1,
                    offset: 0,
                    eof: true,
                },
                b"abc",
            )
            .unwrap();
            for byte in bytes {
                writer.write_all(&[byte]).unwrap();
            }
        });
        let frame = receive::<Command>(&mut reader, Duration::from_secs(1))
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
    fn adjacent_frames_and_peer_close_do_not_lose_bytes() {
        let (mut writer, mut reader) = UnixStream::pair().unwrap();
        send(
            &mut writer,
            &Event::Attached {
                generation: 3,
                revision: None,
            },
            &[],
        )
        .unwrap();
        send(
            &mut writer,
            &Event::Attached {
                generation: 3,
                revision: Some(-1),
            },
            &[],
        )
        .unwrap();
        drop(writer);
        assert!(matches!(
            receive::<Event>(&mut reader, Duration::from_secs(1))
                .unwrap()
                .unwrap()
                .header,
            Event::Attached { revision: None, .. }
        ));
        assert!(matches!(
            receive::<Event>(&mut reader, Duration::from_secs(1))
                .unwrap()
                .unwrap()
                .header,
            Event::Attached {
                revision: Some(-1),
                ..
            }
        ));
        assert!(receive::<Event>(&mut reader, Duration::from_secs(1)).is_err());
    }
    #[test]
    fn partial_frames_and_blocked_output_have_absolute_deadlines() {
        let (mut writer, mut reader) = UnixStream::pair().unwrap();
        writer.write_all(&[0x80]).unwrap();
        let started = Instant::now();
        assert_eq!(
            receive::<Command>(&mut reader, Duration::from_millis(1))
                .err()
                .unwrap()
                .kind(),
            io::ErrorKind::TimedOut
        );
        assert!(started.elapsed() < Duration::from_secs(4));
        let started = Instant::now();
        let bytes = vec![0; CHUNK];
        let mut failed = false;
        for _ in 0..128 {
            if let Err(error) = send(
                &mut writer,
                &Command::Data {
                    generation: 1,
                    id: 1,
                    offset: 0,
                    eof: false,
                },
                &bytes,
            ) {
                assert_eq!(error.kind(), io::ErrorKind::TimedOut);
                failed = true;
                break;
            }
        }
        assert!(failed);
        assert!(started.elapsed() < Duration::from_secs(4));
    }
}
