use super::{FileWrite, Identity};
pub(super) use contract::{
    Message, Outcome, Packet, PublicationRejection, PublicationStage, VERSION,
};
use newport_protocol::{
    clipboard::provider::{self as contract, wire as codec},
    encoding::protobuf::FrameDecoder,
};
use std::{
    ffi::OsString,
    io,
    os::windows::ffi::{OsStrExt, OsStringExt},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
const FRAME_TIME: Duration = Duration::from_secs(20);
fn invalid(error: impl Into<Box<dyn std::error::Error + Send + Sync>>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, error)
}
pub(super) fn revision(value: isize) -> i64 {
    // Every supported Windows pointer width fits a signed protocol revision.
    const {
        assert!(std::mem::size_of::<isize>() <= std::mem::size_of::<i64>());
    }
    value as i64
}
pub(super) fn owner(value: usize) -> u64 {
    const {
        assert!(std::mem::size_of::<usize>() <= std::mem::size_of::<u64>());
    }
    value as u64
}
pub(super) fn native_revision(value: i64) -> io::Result<isize> {
    isize::try_from(value).map_err(invalid)
}
pub(super) fn identity(value: Identity) -> contract::Identity {
    contract::Identity {
        sequence: value.sequence,
        owner: value.owner,
    }
}
pub(super) fn native_identity(value: contract::Identity) -> Identity {
    Identity {
        sequence: value.sequence,
        owner: value.owner,
    }
}
pub(super) fn path(value: &Path) -> io::Result<contract::NativePath> {
    let value = contract::NativePath(value.as_os_str().encode_wide().collect());
    value.validate().map_err(invalid)?;
    Ok(value)
}
pub(super) fn native_path(value: contract::NativePath) -> io::Result<PathBuf> {
    value.validate().map_err(invalid)?;
    Ok(OsString::from_wide(&value.0).into())
}
pub(super) fn receipt(value: FileWrite) -> contract::FileWrite {
    match value {
        FileWrite::Unchanged => contract::FileWrite::Unchanged,
        FileWrite::Uncertain => contract::FileWrite::Uncertain,
        FileWrite::Committed {
            revision: value,
            complete,
        } => contract::FileWrite::Committed {
            revision: revision(value),
            complete,
        },
    }
}
pub(super) fn native_receipt(value: contract::FileWrite) -> io::Result<FileWrite> {
    Ok(match value {
        contract::FileWrite::Unchanged => FileWrite::Unchanged,
        contract::FileWrite::Uncertain => FileWrite::Uncertain,
        contract::FileWrite::Committed { revision, complete } => FileWrite::Committed {
            revision: native_revision(revision)?,
            complete,
        },
    })
}
pub(super) struct Reader {
    decoder: FrameDecoder,
    buffer: [u8; 8192],
    start: usize,
    end: usize,
    deadline: Option<Instant>,
    buffer_deadline: Instant,
    closed: bool,
}
impl Reader {
    pub(super) fn new() -> Self {
        Self {
            decoder: FrameDecoder::new(contract::LIMITS.message),
            buffer: [0; 8192],
            start: 0,
            end: 0,
            deadline: None,
            buffer_deadline: Instant::now(),
            closed: false,
        }
    }
    pub(super) async fn read(
        &mut self,
        reader: &mut (impl AsyncRead + Unpin),
    ) -> io::Result<Packet> {
        if self.closed {
            return Err(io::ErrorKind::NotConnected.into());
        }
        let result = self.read_inner(reader).await;
        if result.is_err() {
            self.closed = true;
        }
        result
    }
    async fn read_inner(&mut self, reader: &mut (impl AsyncRead + Unpin)) -> io::Result<Packet> {
        loop {
            if self.start < self.end {
                let mut input = &self.buffer[self.start..self.end];
                let frame = self
                    .decoder
                    .consume(&mut input)
                    .map_err(codec::Error::from)?;
                self.start = self.end - input.len();
                if let Some(frame) = frame {
                    self.deadline = (self.start < self.end).then_some(self.buffer_deadline);
                    return codec::decode(frame.body()).map_err(io::Error::from);
                }
            }
            let read = if let Some(deadline) = self.deadline {
                if Instant::now() >= deadline {
                    return Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        "Clipboard provider frame deadline expired",
                    ));
                }
                tokio::time::timeout_at(
                    tokio::time::Instant::from_std(deadline),
                    reader.read(&mut self.buffer),
                )
                .await
                .map_err(|error| io::Error::new(io::ErrorKind::TimedOut, error))??
            } else {
                reader.read(&mut self.buffer).await?
            };
            if self
                .deadline
                .is_some_and(|deadline| Instant::now() >= deadline)
            {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "Clipboard provider frame deadline expired",
                ));
            }
            if read == 0 {
                self.decoder.finish().map_err(codec::Error::from)?;
                return Err(io::ErrorKind::UnexpectedEof.into());
            }
            self.start = 0;
            self.end = read;
            self.buffer_deadline = Instant::now() + FRAME_TIME;
            self.deadline.get_or_insert(self.buffer_deadline);
        }
    }
}
pub(super) async fn write(
    writer: &mut (impl AsyncWrite + Unpin),
    packet: Packet,
) -> io::Result<()> {
    let bytes = codec::encode(packet)?;
    write_until(writer, &bytes, Instant::now() + FRAME_TIME).await
}
async fn write_until(
    writer: &mut (impl AsyncWrite + Unpin),
    bytes: &[u8],
    deadline: Instant,
) -> io::Result<()> {
    check_write_deadline(deadline)?;
    tokio::time::timeout_at(tokio::time::Instant::from_std(deadline), async {
        writer.write_all(bytes).await?;
        writer.flush().await
    })
    .await
    .map_err(|error| io::Error::new(io::ErrorKind::TimedOut, error))??;
    // A ready inner future wins over Tokio's timeout even when its final poll
    // runs late. The caller retires the writer after this uncertain outcome.
    check_write_deadline(deadline)
}
fn check_write_deadline(deadline: Instant) -> io::Result<()> {
    if Instant::now() >= deadline {
        Err(io::Error::new(
            io::ErrorKind::TimedOut,
            "Clipboard provider write deadline expired",
        ))
    } else {
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        future::Future,
        pin::Pin,
        sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        },
        task::{Context, Poll},
    };

    struct DeferredWriter {
        ready: Arc<AtomicBool>,
        bytes: Vec<u8>,
        fail: bool,
    }
    impl AsyncWrite for DeferredWriter {
        fn poll_write(
            mut self: Pin<&mut Self>,
            _: &mut Context<'_>,
            bytes: &[u8],
        ) -> Poll<io::Result<usize>> {
            if !self.ready.load(Ordering::Acquire) {
                return Poll::Pending;
            }
            if self.fail {
                return Poll::Ready(Err(io::Error::new(io::ErrorKind::BrokenPipe, WriteFailure)));
            }
            self.bytes.extend_from_slice(bytes);
            Poll::Ready(Ok(bytes.len()))
        }
        fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
            Poll::Ready(Ok(()))
        }
        fn poll_shutdown(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
            Poll::Ready(Ok(()))
        }
    }
    #[derive(Debug)]
    struct WriteFailure;
    impl std::fmt::Display for WriteFailure {
        fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            formatter.write_str("fixture write failure")
        }
    }
    impl std::error::Error for WriteFailure {}

    #[tokio::test]
    async fn a_ready_write_after_the_deadline_remains_an_uncertain_timeout() {
        let ready = Arc::new(AtomicBool::new(false));
        let mut writer = DeferredWriter {
            ready: ready.clone(),
            bytes: Vec::new(),
            fail: false,
        };
        let deadline = Instant::now() + Duration::from_secs(1);
        let mut pending = Box::pin(write_until(&mut writer, b"frame", deadline));
        std::future::poll_fn(|context| {
            assert!(pending.as_mut().poll(context).is_pending());
            Poll::Ready(())
        })
        .await;
        tokio::time::sleep_until(tokio::time::Instant::from_std(deadline)).await;
        ready.store(true, Ordering::Release);
        let error = pending.await.unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
        assert_eq!(writer.bytes, b"frame");
    }
    #[tokio::test]
    async fn expired_writes_send_nothing_and_io_failures_keep_their_cause() {
        let mut writer = DeferredWriter {
            ready: Arc::new(AtomicBool::new(true)),
            bytes: Vec::new(),
            fail: false,
        };
        assert_eq!(
            write_until(&mut writer, b"frame", Instant::now())
                .await
                .unwrap_err()
                .kind(),
            io::ErrorKind::TimedOut
        );
        assert!(writer.bytes.is_empty());
        writer.fail = true;
        let error = write_until(&mut writer, b"frame", Instant::now() + FRAME_TIME)
            .await
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::BrokenPipe);
        assert!(error.get_ref().unwrap().is::<WriteFailure>());
    }
    #[test]
    fn native_paths_preserve_unpaired_utf16_and_reject_nuls() {
        let units = [67, 58, 92, 0xd800, 46, 116, 120, 116];
        let native = PathBuf::from(OsString::from_wide(&units));
        let encoded = path(&native).unwrap();
        assert_eq!(encoded.0, units);
        assert_eq!(native_path(encoded).unwrap(), native);
        assert!(path(Path::new(&OsString::from_wide(&[65, 0, 66]))).is_err());
    }
    #[test]
    fn native_revision_conversion_is_checked() {
        for value in [isize::MIN, -1, 0, isize::MAX] {
            assert_eq!(native_revision(revision(value)).unwrap(), value);
        }
        #[cfg(target_pointer_width = "32")]
        assert!(native_revision(i64::MAX).is_err());
    }
    #[tokio::test]
    async fn adjacent_frames_are_read_without_reframing() {
        let (mut writer, mut reader) = tokio::io::duplex(1024);
        let mut bytes = codec::encode(
            Message::Admitted {
                id: 1,
                accepted: false,
            }
            .into(),
        )
        .unwrap();
        bytes.extend(
            codec::encode(
                Message::Data {
                    id: 2,
                    present: true,
                }
                .into(),
            )
            .unwrap(),
        );
        writer.write_all(&bytes).await.unwrap();
        let mut decoder = Reader::new();
        assert!(matches!(
            decoder.read(&mut reader).await.unwrap().message,
            Message::Admitted {
                id: 1,
                accepted: false
            }
        ));
        assert!(matches!(
            decoder.read(&mut reader).await.unwrap().message,
            Message::Data {
                id: 2,
                present: true
            }
        ));
    }
    #[tokio::test]
    async fn cancelled_partial_prefix_and_body_retain_bytes_and_the_original_deadline() {
        let (mut writer, mut reader) = tokio::io::duplex(1024);
        let frame = codec::encode(Packet {
            message: Message::Data {
                id: 9,
                present: true,
            },
            data: vec![42; 512],
        })
        .unwrap();
        writer.write_all(&frame[..1]).await.unwrap();
        let mut decoder = Reader::new();
        assert!(
            tokio::time::timeout(Duration::from_millis(10), decoder.read(&mut reader))
                .await
                .is_err()
        );
        let deadline = decoder.deadline;
        writer.write_all(&frame[1..30]).await.unwrap();
        assert!(
            tokio::time::timeout(Duration::from_millis(10), decoder.read(&mut reader))
                .await
                .is_err()
        );
        assert_eq!(decoder.deadline, deadline);
        writer.write_all(&frame[30..]).await.unwrap();
        let packet = decoder.read(&mut reader).await.unwrap();
        assert!(matches!(
            packet.message,
            Message::Data {
                id: 9,
                present: true
            }
        ));
        assert_eq!(packet.data, vec![42; 512]);
    }
    #[tokio::test]
    async fn a_coalesced_partial_next_frame_keeps_its_arrival_deadline() {
        let (mut writer, mut reader) = tokio::io::duplex(1024);
        let first = codec::encode(Message::Snapshot { id: 1 }.into()).unwrap();
        let second = codec::encode(Packet {
            message: Message::Data {
                id: 2,
                present: true,
            },
            data: vec![42; 256],
        })
        .unwrap();
        let mut bytes = first;
        bytes.extend(&second[..1]);
        writer.write_all(&bytes).await.unwrap();
        let mut decoder = Reader::new();
        assert!(matches!(
            decoder.read(&mut reader).await.unwrap().message,
            Message::Snapshot { id: 1 }
        ));
        assert_eq!(decoder.end - decoder.start, 1);
        let arrived = decoder.buffer_deadline;
        assert_eq!(decoder.deadline, Some(arrived));
        assert!(
            tokio::time::timeout(Duration::from_millis(10), decoder.read(&mut reader))
                .await
                .is_err()
        );
        assert_eq!(decoder.deadline, Some(arrived));
        writer.write_all(&second[1..]).await.unwrap();
        assert!(matches!(
            decoder.read(&mut reader).await.unwrap().message,
            Message::Data {
                id: 2,
                present: true
            }
        ));
    }
    #[tokio::test]
    async fn malformed_and_expired_partial_frames_retire_the_reader() {
        let (mut writer, mut reader) = tokio::io::duplex(1024);
        writer.write_all(&[2, 122, 1]).await.unwrap();
        let mut decoder = Reader::new();
        assert_eq!(
            decoder.read(&mut reader).await.unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
        assert_eq!(
            decoder.read(&mut reader).await.unwrap_err().kind(),
            io::ErrorKind::NotConnected
        );
        let (mut writer, mut reader) = tokio::io::duplex(1024);
        let frame = codec::encode(Packet {
            message: Message::Data {
                id: 1,
                present: true,
            },
            data: vec![42; 256],
        })
        .unwrap();
        writer.write_all(&frame[..1]).await.unwrap();
        let mut decoder = Reader::new();
        assert!(
            tokio::time::timeout(Duration::from_millis(10), decoder.read(&mut reader))
                .await
                .is_err()
        );
        decoder.deadline = Some(Instant::now());
        writer.write_all(&frame[1..]).await.unwrap();
        assert_eq!(
            decoder.read(&mut reader).await.unwrap_err().kind(),
            io::ErrorKind::TimedOut
        );
        assert_eq!(
            decoder.read(&mut reader).await.unwrap_err().kind(),
            io::ErrorKind::NotConnected
        );
    }
}
