//! Service IO over an exec channel or an authenticated worker forwarding channel.
//! Both expose stdout, stderr and final process outcome; transport loss never
//! manufactures a successful worker exit.
use newport_protocol::{
    encoding::protobuf::{FrameDecoder, FrameError, LengthDecoder},
    runtime::wire::Message,
    worker::{
        Chunk, ClientControl, Direction, Exit, MAX_CHUNK, ServerControl, Transport, admission, wire,
    },
};
use russh::{ChannelMsg, client};
use std::{
    io::{self, Read},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tokio::sync::Mutex;

const EXIT_CLOSE_TIMEOUT: Duration = Duration::from_secs(2);

#[path = "service_channel_stream.rs"]
mod stream;
pub use stream::Stream;

#[derive(Debug)]
pub enum AdmissionError {
    Encode(newport_protocol::runtime::wire::Error),
    Write(russh::Error),
    Read(io::Error),
    Frame(FrameError),
    Decode(newport_protocol::runtime::wire::Error),
    Identity(newport_protocol::runtime::Invalid),
    Closed,
    Unexpected,
    Refused(newport_protocol::common::Error),
    Timeout(tokio::time::error::Elapsed),
}
impl AdmissionError {
    pub(crate) fn transient(&self) -> bool {
        match self {
            Self::Write(_) | Self::Read(_) | Self::Closed | Self::Timeout(_) => true,
            Self::Refused(error) => error.retry == newport_protocol::common::Retry::Yes,
            _ => false,
        }
    }
    pub(crate) fn retry(&self) -> newport_protocol::common::Retry {
        use newport_protocol::common::Retry;
        match self {
            Self::Encode(_) => Retry::No,
            Self::Refused(error) => error.retry,
            _ => Retry::Unknown,
        }
    }
}
impl std::fmt::Display for AdmissionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Encode(error) => return write!(f, "Invalid worker stream admission: {error}"),
            Self::Write(error) => write!(f, "Worker admission could not be sent: {error}")?,
            Self::Read(error) => write!(f, "Worker admission could not be read: {error}")?,
            Self::Frame(error) => write!(f, "Invalid worker admission frame: {error}")?,
            Self::Decode(error) => write!(f, "Invalid worker admission reply: {error}")?,
            Self::Identity(error) => write!(f, "Worker admission identity mismatch: {error}")?,
            Self::Closed => f.write_str("Worker admission ended without a complete reply")?,
            Self::Unexpected => {
                f.write_str("Worker admission received an unexpected SSH message")?
            }
            Self::Refused(error) => return write!(f, "Worker stream admission refused: {error}"),
            Self::Timeout(_) => f.write_str("Worker stream admission timed out")?,
        }
        f.write_str(". The worker was not retried because its admission may have completed.")
    }
}
impl std::error::Error for AdmissionError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Encode(error) | Self::Decode(error) => Some(error),
            Self::Write(error) => Some(error),
            Self::Read(error) => Some(error),
            Self::Frame(error) => Some(error),
            Self::Identity(error) => Some(error),
            Self::Refused(error) => Some(error),
            Self::Timeout(error) => Some(error),
            Self::Closed | Self::Unexpected => None,
        }
    }
}

pub struct Channel {
    reader: Reader,
    writer: Writer,
    fate: Option<newport_ssh::Fate>,
}
impl From<newport_ssh::Channel> for Channel {
    fn from(channel: newport_ssh::Channel) -> Self {
        Self::on_link(channel, None)
    }
}
#[cfg(any(test, feature = "test-support"))]
impl From<russh::Channel<client::Msg>> for Channel {
    fn from(channel: russh::Channel<client::Msg>) -> Self {
        Self::new(channel, None)
    }
}
impl Channel {
    fn on_link(channel: newport_ssh::Channel, transport: Option<Transport>) -> Self {
        let fate = channel.fate();
        let (reader, writer) = channel.split();
        Self::with(reader, writer, transport, Some(fate))
    }
    #[cfg(any(test, feature = "test-support"))]
    fn new(channel: russh::Channel<client::Msg>, transport: Option<Transport>) -> Self {
        let (reader, writer) = channel.split();
        Self::with(reader, writer, transport, None)
    }
    fn with(
        reader: russh::ChannelReadHalf,
        writer: russh::ChannelWriteHalf<client::Msg>,
        transport: Option<Transport>,
        fate: Option<newport_ssh::Fate>,
    ) -> Self {
        Self {
            reader: Reader {
                raw: reader,
                forwarding: transport.map(Forwarding::new),
                pending: io::Cursor::new(vec![]),
                exit_deadline: None,
                worker_exit: None,
                ended: false,
            },
            writer: Writer(Arc::new(Writing {
                raw: Some(writer),
                transport,
                input: Mutex::new(InputState {
                    phase: InputPhase::Open,
                    decoder: match transport {
                        Some(Transport::Protobuf { client, .. }) => {
                            Some(FrameDecoder::new(client.message))
                        }
                        _ => None,
                    },
                }),
                closed: AtomicBool::new(false),
                close_sent: AtomicBool::new(false),
            })),
            fate,
        }
    }
    /// Once opening bytes may have been sent, no error is safe to replay as an
    /// exec command. The caller may fall back only before entering this method.
    pub(crate) async fn forwarded(
        channel: newport_ssh::Channel,
        open: &admission::Open,
        transport: Transport,
    ) -> Result<Self, AdmissionError> {
        Self::on_link(channel, Some(transport)).admit(open).await
    }
    async fn admit(mut self, open: &admission::Open) -> Result<Self, AdmissionError> {
        let channel = &mut self;
        let bytes = open.encode().map_err(AdmissionError::Encode)?;
        let exchange = async {
            channel
                .writer
                .raw()
                .data(&bytes[..])
                .await
                .map_err(AdmissionError::Write)?;
            let mut prefix = LengthDecoder::default();
            let length = loop {
                let mut byte = [0];
                channel.reader.exact(&mut byte).await?;
                if let Some(length) = prefix
                    .push(byte[0], admission::Admission::LIMITS.message)
                    .map_err(AdmissionError::Frame)?
                {
                    break length;
                }
            };
            let mut body = vec![0; length];
            channel.reader.exact(&mut body).await?;
            let reply =
                admission::Admission::decode(body.into()).map_err(AdmissionError::Decode)?;
            reply.validate_for(open).map_err(AdmissionError::Identity)?;
            match reply {
                admission::Admission::Started(_) => Ok(()),
                admission::Admission::Refused(reason) => Err(AdmissionError::Refused(reason)),
            }
        };
        let result = tokio::time::timeout(Duration::from_secs(5), exchange)
            .await
            .map_err(AdmissionError::Timeout)
            .and_then(|result| result);
        match result {
            Ok(()) => Ok(self),
            Err(error) => {
                let (reader, writer) = self.split();
                drop(reader);
                let _ = tokio::time::timeout(Duration::from_secs(1), writer.close()).await;
                Err(error)
            }
        }
    }
    pub fn split(self) -> (Reader, Writer) {
        (self.reader, self.writer)
    }
    pub async fn wait(&mut self) -> Result<Option<ChannelMsg>, String> {
        self.reader.wait().await
    }
    pub async fn close(&self) -> Result<(), String> {
        self.writer.close().await
    }
    pub fn into_stream(self) -> Stream {
        Stream::new(self)
    }
}

// The decoder owns partial framing across cancelled wait futures. RPC relays
// can select another input branch without losing a consumed header or payload.
struct Forwarding {
    transport: Transport,
    frames: FrameDecoder,
    controls: wire::ServerDecoder,
}
impl Forwarding {
    fn new(transport: Transport) -> Self {
        Self {
            transport,
            frames: FrameDecoder::new(transport.limits(Direction::FromWorker).message),
            controls: wire::ServerDecoder::default(),
        }
    }
}
pub struct Reader {
    raw: russh::ChannelReadHalf,
    forwarding: Option<Forwarding>,
    pending: io::Cursor<Vec<u8>>,
    exit_deadline: Option<tokio::time::Instant>,
    worker_exit: Option<Exit>,
    ended: bool,
}
impl Reader {
    /// A typed process outcome only after successful transport closure. Failed
    /// drains erase it; callers must not publish the earlier ChannelMsg as final.
    pub(crate) fn worker_exit(&self) -> Option<Exit> {
        self.ended.then_some(self.worker_exit).flatten()
    }

    async fn refill(&mut self) -> Result<(), String> {
        loop {
            match self.raw.wait().await {
                Some(ChannelMsg::Data { data }) if !data.is_empty() => {
                    self.pending = io::Cursor::new(data.to_vec());
                    return Ok(());
                }
                Some(ChannelMsg::WindowAdjusted { .. }) | Some(ChannelMsg::Data { .. }) => (),
                _ => return Err("Worker transport ended without a complete final status".into()),
            }
        }
    }
    async fn exact(&mut self, mut bytes: &mut [u8]) -> Result<(), AdmissionError> {
        while !bytes.is_empty() {
            let count = self.pending.read(bytes).map_err(AdmissionError::Read)?;
            bytes = &mut bytes[count..];
            if !bytes.is_empty() {
                loop {
                    match self.raw.wait().await {
                        Some(ChannelMsg::Data { data }) if !data.is_empty() => {
                            self.pending = io::Cursor::new(data.to_vec());
                            break;
                        }
                        Some(ChannelMsg::WindowAdjusted { .. } | ChannelMsg::Data { .. }) => (),
                        Some(ChannelMsg::Eof | ChannelMsg::Close) | None => {
                            return Err(AdmissionError::Closed);
                        }
                        Some(_) => return Err(AdmissionError::Unexpected),
                    }
                }
            }
        }
        Ok(())
    }
    pub async fn wait(&mut self) -> Result<Option<ChannelMsg>, String> {
        if self.forwarding.is_none() {
            return Ok(self.raw.wait().await);
        }
        if self.ended {
            return Ok(None);
        }
        if let Some(deadline) = self.exit_deadline {
            let result = self.finish_transport(deadline).await;
            self.ended = true;
            if result.is_err() {
                self.worker_exit = None;
            }
            return result.map(|()| None);
        }
        let result = self.frame().await;
        if result.is_err() {
            self.ended = true;
        }
        result.map(Some)
    }
    async fn finish_transport(&mut self, deadline: tokio::time::Instant) -> Result<(), String> {
        loop {
            if tokio::time::Instant::now() >= deadline {
                return Err("Worker transport did not close after final status".into());
            }
            let message = tokio::time::timeout_at(deadline, self.raw.wait())
                .await
                .map_err(|_| "Worker transport did not close after final status")?;
            match message {
                Some(ChannelMsg::Eof | ChannelMsg::Close) => return Ok(()),
                Some(ChannelMsg::WindowAdjusted { .. }) => (),
                Some(ChannelMsg::Data { .. } | ChannelMsg::ExtendedData { .. }) => {
                    return Err("Worker sent bytes after final status".into());
                }
                Some(_) => {
                    return Err("Worker sent an unexpected SSH message after final status".into());
                }
                None => {
                    return Err(
                        "Worker transport disappeared after final status without EOF or close"
                            .into(),
                    );
                }
            }
        }
    }
    async fn frame(&mut self) -> Result<ChannelMsg, String> {
        loop {
            if let Some(message) = self.decode_pending()? {
                return Ok(message);
            }
            self.refill().await?;
        }
    }
    fn decode_pending(&mut self) -> Result<Option<ChannelMsg>, String> {
        let forwarding = self.forwarding.as_mut().ok_or("Missing worker framing")?;
        let offset = usize::try_from(self.pending.position())
            .map_err(|_| "Invalid worker input position")?;
        let mut bytes = self
            .pending
            .get_ref()
            .get(offset..)
            .ok_or("Invalid worker input position")?;
        let frame = forwarding
            .frames
            .consume(&mut bytes)
            .map_err(|error| error.to_string())?;
        let consumed = self.pending.get_ref().len() - bytes.len();
        self.pending.set_position(consumed as u64);
        let Some(frame) = frame else {
            return Ok(None);
        };
        let message = forwarding
            .controls
            .decode(
                frame.body(),
                &forwarding.transport.limits(Direction::FromWorker),
            )
            .map_err(|error| error.to_string())?;
        let message = match message {
            wire::Frame::Service(body) => {
                let bytes = match forwarding.transport {
                    Transport::Protobuf { server, .. } => {
                        if body.len() > server.message {
                            return Err("Worker service output exceeds its frame limit".into());
                        }
                        frame.into_bytes()
                    }
                    Transport::Opaque => wire::decode_opaque(body)
                        .map_err(|error| error.to_string())?
                        .into_data(),
                };
                ChannelMsg::Data { data: bytes }
            }
            wire::Frame::Control(ServerControl::Stderr(chunk)) => ChannelMsg::ExtendedData {
                data: chunk.into_data(),
                ext: 1,
            },
            wire::Frame::Control(ServerControl::Exit(exit)) => {
                if self.pending.position() != self.pending.get_ref().len() as u64 {
                    return Err("Worker sent bytes after final status".into());
                }
                // Keep this absolute deadline in the reader, so cancelling a
                // pending wait cannot restart the bounded transport drain.
                self.exit_deadline = Some(tokio::time::Instant::now() + EXIT_CLOSE_TIMEOUT);
                self.worker_exit = Some(exit);
                match exit {
                    Exit::Status(exit_status) => ChannelMsg::ExitStatus { exit_status },
                    Exit::Signal(signal) => ChannelMsg::ExitSignal {
                        signal_name: russh::Sig::Custom(format!("signal {}", signal.number())),
                        core_dumped: false,
                        error_message: String::new(),
                        lang_tag: String::new(),
                    },
                }
            }
        };
        Ok(Some(message))
    }
}

struct Writing {
    raw: Option<russh::ChannelWriteHalf<client::Msg>>,
    transport: Option<Transport>,
    input: Mutex<InputState>,
    closed: AtomicBool,
    close_sent: AtomicBool,
}
struct InputState {
    phase: InputPhase,
    decoder: Option<FrameDecoder>,
}
enum InputPhase {
    Open,
    Sending,
    Closing,
    Closed,
}
impl Drop for Writing {
    fn drop(&mut self) {
        if !self.close_sent.swap(true, Ordering::AcqRel)
            && let Some(writer) = self.raw.take()
            && let Ok(runtime) = tokio::runtime::Handle::try_current()
        {
            runtime.spawn(async move {
                let _ = tokio::time::timeout(Duration::from_secs(3), writer.close()).await;
            });
        }
    }
}
#[derive(Clone)]
pub struct Writer(Arc<Writing>);
impl Writer {
    fn raw(&self) -> &russh::ChannelWriteHalf<client::Msg> {
        self.0.raw.as_ref().unwrap()
    }
    pub async fn data(&self, mut bytes: &[u8]) -> Result<(), String> {
        let mut input = self.0.input.lock().await;
        if !matches!(input.phase, InputPhase::Open) || self.0.closed.load(Ordering::Acquire) {
            return Err("Worker input is closed or its last write was interrupted".into());
        }
        // Cancellation during a transport send leaves this phase in place.
        // Reusing the writer could append data after a partially sent frame.
        input.phase = InputPhase::Sending;
        match self.0.transport {
            None => self
                .raw()
                .data(bytes)
                .await
                .map_err(|error| error.to_string())?,
            Some(Transport::Opaque) => {
                for bytes in bytes.chunks(MAX_CHUNK) {
                    let chunk =
                        Chunk::new(bytes.to_vec().into()).map_err(|error| error.to_string())?;
                    let frame = wire::encode_opaque(&chunk, Direction::ToWorker)
                        .map_err(|error| error.to_string())?;
                    self.raw()
                        .data(frame.as_slice())
                        .await
                        .map_err(|error| error.to_string())?;
                }
            }
            Some(Transport::Protobuf { client, .. }) => {
                while !bytes.is_empty() {
                    let frame = input
                        .decoder
                        .as_mut()
                        .ok_or("Missing worker input framing")?
                        .consume(&mut bytes)
                        .map_err(|error| error.to_string())?;
                    if let Some(frame) = frame {
                        wire::reject_controls(&frame.body(), client.message)
                            .map_err(|error| error.to_string())?;
                        let bytes = frame.into_bytes();
                        self.raw()
                            .data(bytes.as_ref())
                            .await
                            .map_err(|error| error.to_string())?;
                    }
                }
            }
        }
        input.phase = InputPhase::Open;
        Ok(())
    }
    pub async fn flush(&self) -> Result<(), String> {
        let mut input = self.0.input.lock().await;
        match input.phase {
            InputPhase::Closed => return Ok(()),
            InputPhase::Open if !self.0.closed.load(Ordering::Acquire) => (),
            _ => return Err("Worker input is closed or its last write was interrupted".into()),
        }
        if input
            .decoder
            .as_ref()
            .is_some_and(|decoder| !decoder.is_at_boundary())
        {
            input.phase = InputPhase::Sending;
            return Err("Worker input flush ended inside a service frame".into());
        }
        Ok(())
    }
    pub async fn eof(&self) -> Result<(), String> {
        let mut input = self.0.input.lock().await;
        match input.phase {
            InputPhase::Closed => return Ok(()),
            InputPhase::Closing => return Err("Worker input EOF was interrupted".into()),
            InputPhase::Sending => return Err("Worker input write was interrupted".into()),
            InputPhase::Open => (),
        }
        input.phase = InputPhase::Closing;
        if let Some(decoder) = input.decoder.as_mut() {
            decoder.finish().map_err(|error| error.to_string())?;
        }
        let result = if self.0.transport.is_some() {
            let frame = wire::encode_client(ClientControl::InputComplete)
                .map_err(|error| error.to_string())?;
            self.raw()
                .data(frame.as_slice())
                .await
                .map_err(|error| error.to_string())
        } else {
            self.raw().eof().await.map_err(|error| error.to_string())
        };
        if result.is_ok() {
            input.phase = InputPhase::Closed;
        }
        result
    }
    /// Signals are separate worker controls, permitted after InputComplete.
    /// A cancelled write poisons this writer just as a cancelled data send does.
    pub(crate) async fn signal(
        &self,
        signal: newport_protocol::worker::Signal,
    ) -> Result<(), String> {
        if self.0.transport.is_none() {
            return Err("Worker signals require a forwarded channel".into());
        }
        let mut input = self.0.input.lock().await;
        if self.0.closed.load(Ordering::Acquire)
            || !matches!(input.phase, InputPhase::Open | InputPhase::Closed)
            || (matches!(input.phase, InputPhase::Open)
                && input
                    .decoder
                    .as_ref()
                    .is_some_and(|decoder| !decoder.is_at_boundary()))
        {
            return Err("Worker input is closed or its last write was interrupted".into());
        }
        let previous = std::mem::replace(&mut input.phase, InputPhase::Sending);
        let bytes = wire::encode_client(ClientControl::Signal(signal))
            .map_err(|error| error.to_string())?;
        self.raw()
            .data(bytes.as_slice())
            .await
            .map_err(|error| error.to_string())?;
        input.phase = previous;
        Ok(())
    }
    pub async fn close(&self) -> Result<(), String> {
        self.0.closed.store(true, Ordering::Release);
        self.raw().close().await.map_err(|e| e.to_string())?;
        self.0.close_sent.store(true, Ordering::Release);
        Ok(())
    }
}
pub(crate) fn signal_text(signal: &russh::Sig) -> String {
    match signal {
        russh::Sig::Custom(value) => value.clone(),
        other => format!("{other:?}"),
    }
}

#[cfg(test)]
#[path = "service_channel_tests.rs"]
mod tests;
