//! Desktop protobuf framing around the independent LSP byte stream.
use crate::backend::exchange::Opened;
use bytes::Bytes;
use newport_platform::ipc::{
    self,
    framed::{Reader, Writer},
};
use newport_protocol::{
    common::Present,
    desktop::{self, OpaqueFlow, OpaqueKind, stream, wire},
};
use std::{
    io::{self, Read, Write},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

pub(crate) trait Source: Send + 'static {
    fn next(&mut self) -> io::Result<Option<Bytes>>;
    fn eof(&mut self) -> io::Result<()>;
    fn observe(&self, _message: &stream::OpaqueServer) -> io::Result<()> {
        Ok(())
    }
}
struct Lifetime {
    cancel: ipc::Stream,
    flow: Mutex<OpaqueFlow>,
}
impl Lifetime {
    fn cancel(&self) {
        let _ = self.cancel.shutdown(std::net::Shutdown::Both);
    }
}
impl Drop for Lifetime {
    fn drop(&mut self) {
        self.cancel();
    }
}
pub(crate) struct Input {
    reader: Reader,
    lifetime: Arc<Lifetime>,
}
pub(crate) struct Output {
    writer: Writer,
    lifetime: Arc<Lifetime>,
    finished: bool,
}
impl Source for Input {
    fn observe(&self, message: &stream::OpaqueServer) -> io::Result<()> {
        self.lifetime
            .flow
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .server(message)
            .map_err(invalid)
    }
    fn next(&mut self) -> io::Result<Option<Bytes>> {
        self.reader
            .next_idle(desktop::OPAQUE_LIMITS.message, Duration::from_secs(30))
            .map(|frame| frame.map(|frame| frame.body()))
    }
    fn eof(&mut self) -> io::Result<()> {
        match self.reader.next_until(
            desktop::OPAQUE_LIMITS.message,
            Instant::now() + desktop::CLOSURE_TIMEOUT,
        )? {
            None => Ok(()),
            Some(_) => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Data after the language-server completion",
            )),
        }
    }
}
impl Drop for Input {
    fn drop(&mut self) {
        self.lifetime.cancel();
    }
}
impl Write for Output {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if self.finished {
            return Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "Language-server input already ended",
            ));
        }
        if bytes.is_empty() {
            return Ok(0);
        }
        let len = bytes.len().min(desktop::OPAQUE_CHUNK);
        let message = stream::OpaqueClient::Data(Box::new(Bytes::copy_from_slice(&bytes[..len])));
        self.lifetime
            .flow
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .client(&message)
            .map_err(invalid)?;
        let frame = wire::encode_opaque_client(&message).map_err(invalid)?;
        if let Err(error) = self
            .writer
            .send_until(&frame, Instant::now() + Duration::from_secs(30))
        {
            self.finished = true;
            self.lifetime.cancel();
            return Err(error);
        }
        Ok(len)
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
impl Drop for Output {
    fn drop(&mut self) {
        if self.finished {
            return;
        }
        self.finished = true;
        let message = stream::OpaqueClient::InputComplete(Box::new(Present {}));
        let valid = self
            .lifetime
            .flow
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .client(&message)
            .is_ok();
        if !valid {
            return;
        }
        let sent = wire::encode_opaque_client(&message)
            .map_err(invalid)
            .and_then(|frame| {
                self.writer
                    .send_until(&frame, Instant::now() + Duration::from_secs(5))
            });
        if sent.is_err() {
            self.lifetime.cancel();
        }
    }
}
pub(crate) fn split(opened: Opened) -> io::Result<(Input, Output)> {
    if !matches!(&opened.identity, stream::Opened::Lsp(_)) {
        let _ = opened.cancel();
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Not a language-server stream",
        ));
    }
    let lifetime = Arc::new(Lifetime {
        cancel: opened.reader.try_clone_stream()?,
        flow: Mutex::new(OpaqueFlow::new(OpaqueKind::Lsp)),
    });
    Ok((
        Input {
            reader: opened.reader,
            lifetime: lifetime.clone(),
        },
        Output {
            writer: opened.writer,
            lifetime,
            finished: false,
        },
    ))
}
fn invalid(error: impl std::error::Error + Send + Sync + 'static) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, error)
}

/// Flatten validated payload frames without discarding the typed final result.
pub(crate) struct Frames<S> {
    source: S,
    pending: Bytes,
    flow: OpaqueFlow,
    done: bool,
}
impl<S: Source> Frames<S> {
    pub(crate) fn new(source: S) -> Self {
        Self {
            source,
            pending: Bytes::new(),
            flow: OpaqueFlow::new(OpaqueKind::Lsp),
            done: false,
        }
    }
}
impl<S: Source> Read for Frames<S> {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        if output.is_empty() {
            return Ok(0);
        }
        while self.pending.is_empty() {
            if self.done {
                return Ok(0);
            }
            let Some(body) = self.source.next()? else {
                self.done = true;
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "The connection to the server was lost.",
                ));
            };
            let value = wire::decode_opaque_server(body).map_err(invalid)?;
            self.flow.server(&value).map_err(invalid)?;
            self.source.observe(&value)?;
            match value {
                stream::OpaqueServer::Data(data) => self.pending = *data,
                stream::OpaqueServer::Completion(completion) => {
                    self.done = true;
                    self.source.eof()?;
                    self.flow.eof().map_err(invalid)?;
                    let stream::Completion::Lsp(completion) = *completion else {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            "Wrong language-server completion",
                        ));
                    };
                    if let Some(error) = completion.error {
                        return Err(io::Error::other(error));
                    }
                    return Ok(0);
                }
            }
        }
        let size = output.len().min(self.pending.len());
        output[..size].copy_from_slice(&self.pending.split_to(size));
        Ok(size)
    }
}
#[cfg(test)]
pub(crate) struct Fixture<R>(pub R);
#[cfg(test)]
impl<R: Read + Send + 'static> Source for Fixture<R> {
    fn next(&mut self) -> io::Result<Option<Bytes>> {
        newport_protocol::encoding::protobuf::read_frame(
            &mut self.0,
            desktop::OPAQUE_LIMITS.message,
        )
        .map_err(invalid)
    }
    fn eof(&mut self) -> io::Result<()> {
        match self.next()? {
            None => Ok(()),
            Some(_) => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Data after completion",
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn complete() -> Vec<u8> {
        wire::encode_opaque_server(&stream::OpaqueServer::Completion(Box::new(
            stream::Completion::Lsp(Box::new(stream::LspCompletion { error: None })),
        )))
        .unwrap()
    }
    fn data(value: &[u8]) -> Vec<u8> {
        wire::encode_opaque_server(&stream::OpaqueServer::Data(Box::new(
            Bytes::copy_from_slice(value),
        )))
        .unwrap()
    }
    #[test]
    fn adjacent_frames_keep_payload_order_and_require_completion_eof() {
        let mut bytes = data(b"first");
        bytes.extend(data(b"second"));
        bytes.extend(complete());
        let mut frames = Frames::new(Fixture(std::io::Cursor::new(bytes)));
        let mut output = Vec::new();
        frames.read_to_end(&mut output).unwrap();
        assert_eq!(output, b"firstsecond");
        for bytes in [
            data(b"missing final"),
            {
                let mut bytes = complete();
                bytes.extend(data(b"late"));
                bytes
            },
            {
                let mut bytes = complete();
                bytes.push(0x80);
                bytes
            },
        ] {
            assert!(
                Frames::new(Fixture(std::io::Cursor::new(bytes)))
                    .read_to_end(&mut Vec::new())
                    .is_err()
            );
        }
    }
    #[test]
    fn remote_failure_retains_its_typed_code_and_retry() {
        let bytes = wire::encode_opaque_server(&stream::OpaqueServer::Completion(Box::new(
            stream::Completion::Lsp(Box::new(stream::LspCompletion {
                error: Some(newport_protocol::common::Error {
                    code: "LSP_STOPPED".into(),
                    message: "stopped".into(),
                    retry: newport_protocol::common::Retry::No,
                }),
            })),
        )))
        .unwrap();
        let error = Frames::new(Fixture(std::io::Cursor::new(bytes)))
            .read_to_end(&mut Vec::new())
            .unwrap_err();
        let remote = error
            .get_ref()
            .unwrap()
            .downcast_ref::<newport_protocol::common::Error>()
            .unwrap();
        assert_eq!(remote.code, "LSP_STOPPED");
        assert_eq!(remote.retry, newport_protocol::common::Retry::No);
    }
    #[test]
    fn final_connection_drop_cancels_an_idle_native_stream() {
        let (client, mut peer) = ipc::Stream::pair().unwrap();
        peer.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
        let writer = Writer::new(client.try_clone().unwrap());
        let opened = Opened {
            identity: stream::Opened::Lsp(Box::new(stream::LspOpened {
                version: desktop::VERSION,
                server: uuid::Uuid::new_v4(),
            })),
            reader: Reader::new(client),
            writer,
        };
        let (connection, _events) =
            crate::lsp::Connection::start(opened, serde_json::Value::Null).unwrap();
        let clone = connection.clone();
        drop(connection);
        drop(clone);
        loop {
            match newport_protocol::encoding::protobuf::read_frame(
                &mut peer,
                desktop::OPAQUE_LIMITS.message,
            ) {
                Ok(None) => break,
                Ok(Some(body)) => assert!(matches!(
                    wire::decode_opaque_client(body).unwrap(),
                    stream::OpaqueClient::InputComplete(_)
                )),
                Err(error) => panic!("native cancellation did not close: {error}"),
            }
        }
    }
}
