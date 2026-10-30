//! Service IO over an exec channel or an authenticated worker forwarding channel.
//! Both expose stdout, stderr and final process outcome; transport loss never
//! manufactures a successful worker exit.
use crate::agent_runtime::protocol::{self as control, worker_stream as wire};
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

#[path = "service_channel_stream.rs"]
mod stream;
pub use stream::Stream;

pub struct Channel {
    reader: Reader,
    writer: Writer,
    fate: Option<newport_ssh::Fate>,
}
impl From<newport_ssh::Channel> for Channel {
    fn from(channel: newport_ssh::Channel) -> Self {
        Self::on_link(channel, false)
    }
}
#[cfg(test)]
impl From<russh::Channel<client::Msg>> for Channel {
    fn from(channel: russh::Channel<client::Msg>) -> Self {
        Self::new(channel, false)
    }
}
impl Channel {
    fn on_link(channel: newport_ssh::Channel, framed: bool) -> Self {
        let fate = channel.fate();
        let (reader, writer) = channel.split();
        Self::with(reader, writer, framed, Some(fate))
    }
    #[cfg(test)]
    fn new(channel: russh::Channel<client::Msg>, framed: bool) -> Self {
        let (reader, writer) = channel.split();
        Self::with(reader, writer, framed, None)
    }
    fn with(
        reader: russh::ChannelReadHalf,
        writer: russh::ChannelWriteHalf<client::Msg>,
        framed: bool,
        fate: Option<newport_ssh::Fate>,
    ) -> Self {
        Self {
            reader: Reader {
                raw: reader,
                framed,
                pending: io::Cursor::new(vec![]),
                decoder: Decoder::default(),
                ended: false,
            },
            writer: Writer(Arc::new(Writing {
                raw: Some(writer),
                framed,
                input: Mutex::new(InputState::Open),
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
        open: &wire::Open,
    ) -> Result<Self, String> {
        Self::on_link(channel, true).admit(open).await
    }
    async fn admit(mut self, open: &wire::Open) -> Result<Self, String> {
        let channel = &mut self;
        let bytes = open
            .encode()
            .map_err(|_| "Invalid worker stream admission")?;
        let exchange = async {
            let mut framed = Vec::with_capacity(4 + bytes.len());
            framed.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
            framed.extend_from_slice(&bytes);
            channel
                .writer
                .raw()
                .data(&framed[..])
                .await
                .map_err(|e| e.to_string())?;
            let mut length = [0; 4];
            channel.reader.exact(&mut length).await?;
            let length = u32::from_be_bytes(length) as usize;
            if length == 0 || length > control::MAX_FRAME {
                return Err("Invalid worker admission reply length".into());
            }
            let mut bytes = vec![0; length];
            channel.reader.exact(&mut bytes).await?;
            let reply: wire::Admission =
                control::decode(&bytes).map_err(|_| "Invalid worker admission reply")?;
            reply
                .validate_for(open)
                .map_err(|_| "Worker admission identity mismatch")?;
            match reply {
                wire::Admission::Started { .. } => Ok(()),
                wire::Admission::Refused { reason } => {
                    Err(format!("Worker stream admission refused: {reason:?}"))
                }
            }
        };
        match tokio::time::timeout(Duration::from_secs(5), exchange).await {
            Ok(Ok(())) => Ok(self),
            result => {
                let (reader, writer) = self.split();
                drop(reader);
                let _ = tokio::time::timeout(Duration::from_secs(1), writer.close()).await;
                let reason = match result {
                    Ok(Err(error)) => error,
                    _ => "Worker stream admission timed out".into(),
                };
                Err(format!(
                    "{reason}. The worker was not retried because its admission may have completed."
                ))
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
#[derive(Default)]
struct Decoder {
    header: [u8; 5],
    used: usize,
    data: Vec<u8>,
    filled: usize,
}
impl Decoder {
    fn consume(&mut self, input: &mut io::Cursor<Vec<u8>>) -> Result<Option<wire::Frame>, String> {
        if self.used < 5 {
            self.used += input
                .read(&mut self.header[self.used..])
                .map_err(|e| e.to_string())?;
            if self.used < 5 {
                return Ok(None);
            }
            let header = wire::Header::decode(self.header, wire::Direction::FromWorker)
                .map_err(|e| e.to_string())?;
            self.data.resize(header.length, 0);
        }
        self.filled += input
            .read(&mut self.data[self.filled..])
            .map_err(|e| e.to_string())?;
        if self.filled < self.data.len() {
            return Ok(None);
        }
        let header = wire::Header::decode(self.header, wire::Direction::FromWorker)
            .map_err(|e| e.to_string())?;
        let frame = wire::Frame {
            kind: header.kind,
            data: std::mem::take(&mut self.data),
        };
        frame
            .validate(wire::Direction::FromWorker)
            .map_err(|e| e.to_string())?;
        self.used = 0;
        self.filled = 0;
        Ok(Some(frame))
    }
}
pub struct Reader {
    raw: russh::ChannelReadHalf,
    framed: bool,
    pending: io::Cursor<Vec<u8>>,
    decoder: Decoder,
    ended: bool,
}
impl Reader {
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
    async fn exact(&mut self, mut bytes: &mut [u8]) -> Result<(), String> {
        while !bytes.is_empty() {
            let count = self.pending.read(bytes).map_err(|e| e.to_string())?;
            bytes = &mut bytes[count..];
            if !bytes.is_empty() {
                self.refill().await?;
            }
        }
        Ok(())
    }
    pub async fn wait(&mut self) -> Result<Option<ChannelMsg>, String> {
        if !self.framed {
            return Ok(self.raw.wait().await);
        }
        if self.ended {
            return Ok(None);
        }
        let result = self.frame().await;
        if result.is_err() {
            self.ended = true;
        }
        result.map(Some)
    }
    async fn frame(&mut self) -> Result<ChannelMsg, String> {
        loop {
            if let Some(frame) = self.decoder.consume(&mut self.pending)? {
                let kind = frame.kind;
                let final_frame = matches!(kind, wire::Kind::ExitStatus | wire::Kind::ExitSignal);
                if final_frame {
                    if self.pending.position() != self.pending.get_ref().len() as u64 {
                        return Err("Worker sent bytes after final status".into());
                    }
                    self.ended = true;
                }
                return Ok(match kind {
                    wire::Kind::Output => ChannelMsg::Data {
                        data: frame.data.into(),
                    },
                    wire::Kind::Error => ChannelMsg::ExtendedData {
                        data: frame.data.into(),
                        ext: 1,
                    },
                    wire::Kind::ExitStatus => ChannelMsg::ExitStatus {
                        exit_status: u32::from_be_bytes(frame.data.try_into().unwrap()),
                    },
                    wire::Kind::ExitSignal => ChannelMsg::ExitSignal {
                        signal_name: russh::Sig::Custom(format!(
                            "signal {}",
                            u32::from_be_bytes(frame.data.try_into().unwrap())
                        )),
                        core_dumped: false,
                        error_message: String::new(),
                        lang_tag: String::new(),
                    },
                    _ => return Err("Invalid worker output frame".into()),
                });
            }
            self.refill().await?;
        }
    }
}

struct Writing {
    raw: Option<russh::ChannelWriteHalf<client::Msg>>,
    framed: bool,
    input: Mutex<InputState>,
    closed: AtomicBool,
    close_sent: AtomicBool,
}
enum InputState {
    Open,
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
    pub async fn data(&self, bytes: &[u8]) -> Result<(), String> {
        let eof = self.0.input.lock().await;
        if !matches!(*eof, InputState::Open) || self.0.closed.load(Ordering::Acquire) {
            return Err("Worker input is closed".into());
        }
        if !self.0.framed {
            return self.raw().data(bytes).await.map_err(|e| e.to_string());
        }
        for bytes in bytes.chunks(wire::MAX_CHUNK) {
            let mut frame = Vec::with_capacity(5 + bytes.len());
            frame.extend_from_slice(
                &wire::Header {
                    kind: wire::Kind::Input,
                    length: bytes.len(),
                }
                .encode(wire::Direction::ToWorker)
                .map_err(|e| e.to_string())?,
            );
            frame.extend_from_slice(bytes);
            self.raw()
                .data(&frame[..])
                .await
                .map_err(|e| e.to_string())?;
        }
        Ok(())
    }
    pub async fn eof(&self) -> Result<(), String> {
        let mut eof = self.0.input.lock().await;
        match *eof {
            InputState::Closed => return Ok(()),
            InputState::Closing => return Err("Worker input EOF was interrupted".into()),
            InputState::Open => (),
        }
        *eof = InputState::Closing;
        let result = if self.0.framed {
            let frame = wire::Header {
                kind: wire::Kind::InputEof,
                length: 0,
            }
            .encode(wire::Direction::ToWorker)
            .map_err(|e| e.to_string())?;
            self.raw().data(&frame[..]).await.map_err(|e| e.to_string())
        } else {
            self.raw().eof().await.map_err(|e| e.to_string())
        };
        if result.is_ok() {
            *eof = InputState::Closed;
        }
        result
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
