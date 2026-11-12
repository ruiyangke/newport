//! Async Git client. Cancellation retires the owning channel before it can be reused.
use newport_protocol::desktop::git::{CollectedDiff, GitResult};
use newport_protocol::{
    encoding::protobuf::{FrameError, tokio::FrameReader},
    git::{
        self as protocol, Ack, Call, ClientMessage, Configure, Error, Reply, Request,
        ServerMessage,
        wire::{self, Message},
    },
};
use serde_json::{Value, json};
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt};
use uuid::Uuid;

pub struct Client<S> {
    stream: FrameReader<S>,
    methods: Vec<String>,
    pub on_log: Option<Box<dyn Fn(protocol::CommandLog) + Send + Sync>>,
}
impl<S: AsyncRead + AsyncWrite + Unpin> Client<S> {
    pub(crate) fn stream(&self) -> &S {
        self.stream.get_ref()
    }
    #[cfg(test)]
    pub async fn start(stream: S) -> Result<(Self, Value), Error> {
        Self::start_with_identity(stream, Uuid::new_v4().to_string()).await
    }
    pub async fn start_with_identity(stream: S, client_id: String) -> Result<(Self, Value), Error> {
        let mut stream = FrameReader::new(stream, wire::FRAME_LIMITS);
        let id = Uuid::new_v4().to_string();
        send(
            stream.get_mut(),
            &ClientMessage::Configure(Configure {
                id: id.clone(),
                client_id,
                client_version: env!("CARGO_PKG_VERSION").into(),
                command_logs: true,
            }),
        )
        .await?;
        let ready = match receive(&mut stream).await? {
            ServerMessage::Ready(ready) if ready.id == id => ready,
            ServerMessage::Failed(error) => return Err(error),
            _ => return Err(invalid()),
        };
        let methods = ready.capabilities.methods.clone();
        Ok((
            Self {
                stream,
                methods,
                on_log: None,
            },
            json!({"capabilities":ready.capabilities, "limits":ready.limits}),
        ))
    }
    pub async fn ping(&mut self) -> Result<(), Error> {
        let nonce = Uuid::new_v4().to_string();
        send(self.stream.get_mut(), &ClientMessage::Ping(nonce.clone())).await?;
        if !matches!(receive(&mut self.stream).await?, ServerMessage::Pong(reply) if reply == nonce)
        {
            return Err(invalid());
        }
        Ok(())
    }
    pub async fn request(&mut self, request: Request) -> Result<Value, Error> {
        let value = self.request_typed(request.clone()).await?;
        super::project_result(&value, &request)
    }
    pub(crate) async fn request_typed(&mut self, request: Request) -> Result<GitResult, Error> {
        request
            .validate()
            .map_err(|error| Error::invalid(error.to_string()))?;
        if !self.methods.iter().any(|method| method == request.method()) {
            return Err(Error::new(
                "UNSUPPORTED_METHOD",
                "The agent does not support this Git method.",
            ));
        }
        let call = Call {
            id: Uuid::new_v4().to_string(),
            request: Box::new(request),
        };
        let expected_snapshot = match call.request.as_ref() {
            Request::Diff { snapshot, .. } => Some(snapshot.as_str()),
            Request::CommitDiff { commit_oid, .. } => Some(commit_oid.as_str()),
            _ => None,
        };
        send(self.stream.get_mut(), &ClientMessage::Call(call.clone())).await?;
        let mut stream_id = None;
        let mut data = Vec::new();
        let mut last_seq = 0u32;
        let mut log_count = 0;
        loop {
            match receive(&mut self.stream).await? {
                ServerMessage::Log(log) if log.id == call.id => {
                    log_count += 1;
                    if log_count > 32 {
                        return Err(invalid());
                    }
                    if let Some(callback) = &self.on_log {
                        callback(log.entry);
                    }
                }
                ServerMessage::Begin(begin)
                    if begin.id == call.id
                        && stream_id.is_none()
                        && expected_snapshot == Some(begin.snapshot.as_str()) =>
                {
                    stream_id = Some(begin.stream_id);
                }
                ServerMessage::Chunk(chunk)
                    if chunk.id == call.id
                        && stream_id.as_ref() == Some(&chunk.stream_id)
                        && Some(chunk.seq) == last_seq.checked_add(1) =>
                {
                    if data
                        .len()
                        .checked_add(chunk.data.0.len())
                        .is_none_or(|length| length > protocol::MAX_DIFF)
                    {
                        return Err(Error::new(
                            "PROTOCOL_ERROR",
                            "Git stream exceeds its limit.",
                        ));
                    }
                    data.extend(chunk.data.0);
                    last_seq = chunk.seq;
                    send(
                        self.stream.get_mut(),
                        &ClientMessage::Ack(Ack {
                            stream_id: chunk.stream_id,
                            seq: chunk.seq,
                        }),
                    )
                    .await?;
                }
                ServerMessage::Response(response) => {
                    response.validate_for(&call).map_err(|_| invalid())?;
                    let value = response.result?;
                    if stream_id.is_some() {
                        let Reply::StreamComplete(summary) = value else {
                            return Err(invalid());
                        };
                        if summary.last_seq != last_seq || summary.bytes != data.len() as u64 {
                            return Err(invalid());
                        }
                        let diff = wire::decode_diff(data.into()).map_err(|_| invalid())?;
                        return Ok(GitResult::Diff(Box::new(CollectedDiff {
                            snapshot: expected_snapshot.ok_or_else(invalid)?.to_owned(),
                            diff,
                        })));
                    }
                    if expected_snapshot.is_some() {
                        return Err(invalid());
                    }
                    return Ok(GitResult::Reply(Box::new(value)));
                }
                ServerMessage::Failed(error) => return Err(error),
                _ => return Err(invalid()),
            }
        }
    }
}
fn invalid() -> Error {
    Error::new(
        "PROTOCOL_ERROR",
        "Invalid Git agent response. Reconnect before continuing.",
    )
}

pub(super) fn validate_request(request: &Request) -> Result<(), Error> {
    request
        .validate()
        .map_err(|error| Error::invalid(error.to_string()))?;
    // Match the correlation ID representation used by request() so this checks
    // the complete encoded frame before connection or journal side effects.
    encode(&ClientMessage::Call(Call {
        id: Uuid::nil().to_string(),
        request: Box::new(request.clone()),
    }))?;
    Ok(())
}

fn encode(message: &impl Message) -> Result<Vec<u8>, Error> {
    message
        .encode()
        .map_err(|error| Error::invalid(format!("Invalid Git message: {error}")))
}

async fn receive<S: AsyncRead + Unpin>(
    reader: &mut FrameReader<S>,
) -> Result<ServerMessage, Error> {
    let body = reader
        .next()
        .await
        .map_err(|error| match error {
            FrameError::Io(_) | FrameError::Truncated => {
                Error::transport(format!("Git channel could not be read: {error}"))
            }
            _ => Error::new("PROTOCOL_ERROR", format!("Invalid Git frame: {error}")),
        })?
        .ok_or_else(|| Error::transport("Git channel closed."))?;
    ServerMessage::decode(body)
        .map_err(|error| Error::new("PROTOCOL_ERROR", format!("Invalid Git response: {error}")))
}
async fn send(stream: &mut (impl AsyncWrite + Unpin), message: &impl Message) -> Result<(), Error> {
    let frame = encode(message)?;
    stream
        .write_all(&frame)
        .await
        .map_err(|error| Error::transport(format!("Could not send Git request: {error}")))?;
    stream
        .flush()
        .await
        .map_err(|error| Error::transport(format!("Could not flush Git request: {error}")))
}

/// A failed read must retire its partially consumed protocol stream.
/// This deliberately does not receive the shared connection, so it cannot
/// disconnect another lane while releasing a failed read.
pub(super) async fn response_typed<S: AsyncRead + AsyncWrite + Unpin>(
    slot: &mut Option<Client<S>>,
    request: Request,
    deadline: std::time::Duration,
    session_cancel: &mut tokio::sync::watch::Receiver<bool>,
) -> Result<GitResult, Error> {
    let mutation = super::is_mutation(&request);
    // A query invalidation can drop this entire future. Own the stream during
    // I/O so cancellation drops it instead of leaving a partial frame in a lane.
    let mut client = slot
        .take()
        .ok_or_else(|| Error::transport("Git client is not initialized"))?;
    let mut result = tokio::select! {
        biased;
        _ = session_cancel.changed() => Err(Error::new("CANCELLED", "Git session was disconnected.")),
        result = tokio::time::timeout(deadline, client.request_typed(request)) =>
            result.unwrap_or_else(|_| Err(Error::transport("Git request timed out. Reconnect before continuing."))),
    };
    if !mutation
        && let Err(error) = &mut result
        && matches!(error.code.as_str(), "TRANSPORT_ERROR" | "PROTOCOL_ERROR")
    {
        // This stream cannot be reused, but its SSH connection
        // may still be serving other read lanes successfully.
        error.code = "READ_CHANNEL_ERROR".into();
        error.message = "The Git read channel was interrupted. Retry this read.".into();
    } else {
        *slot = Some(client);
    }
    result
}
#[cfg(test)]
pub(super) async fn response<S: AsyncRead + AsyncWrite + Unpin>(
    slot: &mut Option<Client<S>>,
    request: Request,
    deadline: std::time::Duration,
    session_cancel: &mut tokio::sync::watch::Receiver<bool>,
) -> Result<Value, Error> {
    let value = response_typed(slot, request.clone(), deadline, session_cancel).await?;
    super::project_result(&value, &request)
}

#[cfg(test)]
mod tests {
    use super::*;
    use protocol::{
        Begin, Binary, Chunk, Closed, CommandLog, Log, Ready, Response, StreamComplete,
    };
    use std::time::Duration;
    use tokio::io::AsyncReadExt;

    fn client(stream: tokio::io::DuplexStream) -> Client<tokio::io::DuplexStream> {
        Client {
            stream: FrameReader::new(stream, wire::FRAME_LIMITS),
            methods: protocol::METHODS
                .iter()
                .map(|method| (*method).into())
                .collect(),
            on_log: None,
        }
    }
    async fn read_client(stream: &mut tokio::io::DuplexStream) -> ClientMessage {
        let mut prefix = newport_protocol::encoding::protobuf::LengthDecoder::default();
        let length = loop {
            if let Some(length) = prefix
                .push(stream.read_u8().await.unwrap(), protocol::MAX_FRAME)
                .unwrap()
            {
                break length;
            }
        };
        let mut body = vec![0; length];
        stream.read_exact(&mut body).await.unwrap();
        ClientMessage::decode(body.into()).unwrap()
    }
    async fn handshake(peer: &mut tokio::io::DuplexStream) {
        let ClientMessage::Configure(configure) = read_client(peer).await else {
            panic!()
        };
        send(
            peer,
            &ServerMessage::Ready(Ready {
                id: configure.id,
                limits: protocol::Limits::default(),
                capabilities: protocol::Capabilities {
                    methods: protocol::METHODS
                        .iter()
                        .map(|method| (*method).into())
                        .collect(),
                    object_formats: vec!["sha1".into(), "sha256".into()],
                    actions: vec![],
                    features: vec![],
                },
            }),
        )
        .await
        .unwrap();
    }
    fn close(id: String) -> ServerMessage {
        ServerMessage::Response(Response {
            id,
            result: Ok(Reply::Close(Box::new(Closed { closed: true }))),
        })
    }
    fn stream_done(id: String, last_seq: u32, bytes: usize) -> ServerMessage {
        ServerMessage::Response(Response {
            id,
            result: Ok(Reply::StreamComplete(Box::new(StreamComplete {
                last_seq,
                bytes: bytes as u64,
            }))),
        })
    }
    fn diff(length: usize) -> protocol::Diff {
        serde_json::from_value(json!({"files":[{
            "oldPath":protocol::Path::new(b"file"),"newPath":protocol::Path::new(b"file"),
            "oldOid":null,"newOid":null,"oldMode":0,"newMode":33188,"status":"Untracked","binary":false,"additions":1,"deletions":0,
            "hunks":[{"id":"hunk","oldStart":0,"oldLines":0,"newStart":1,"newLines":1,"lines":[{"id":"line","origin":"+","oldLine":null,"newLine":1,"content":protocol::Path::new(&vec![b'x'; length])}]}]
        }],"truncated":false,"readOnly":false,"sourceSnapshot":"snap","entryId":"entry","side":"index_to_worktree","contextLines":3})).unwrap()
    }
    fn diff_request() -> Request {
        Request::Diff {
            repo_id: "repo".into(),
            snapshot: "snap".into(),
            entry_id: "entry".into(),
            side: protocol::Side::IndexToWorktree,
            context_lines: 3,
        }
    }

    #[tokio::test]
    async fn oversized_request_writes_nothing_and_preserves_the_channel() {
        let request = Request::Start {
            operation_id: Uuid::new_v4().to_string(),
            repo_id: "repo".into(),
            expected_snapshot: "snapshot".into(),
            action: protocol::Action::Stage {
                entry_ids: vec!["x".repeat(104); 10_000],
                hunks: None,
            },
        };
        request.validate().unwrap();
        let (stream, mut peer) = tokio::io::duplex(1024);
        let mut slot = Some(client(stream));
        let (_cancel, mut cancelled) = tokio::sync::watch::channel(false);
        let error = response(&mut slot, request, Duration::from_secs(1), &mut cancelled)
            .await
            .unwrap_err();
        assert_eq!(error.code, "INVALID_REQUEST");
        assert!(slot.is_some());
        tokio::time::timeout(Duration::from_secs(2), async {
            let agent = async {
                let ClientMessage::Call(call) = read_client(&mut peer).await else {
                    panic!("expected a Git call")
                };
                assert!(matches!(call.request.as_ref(), Request::Close { .. }));
                send(&mut peer, &close(call.id)).await.unwrap();
            };
            let request = response(
                &mut slot,
                Request::Close {
                    repo_id: "repo".into(),
                },
                Duration::from_secs(1),
                &mut cancelled,
            );
            let (result, ()) = tokio::join!(request, agent);
            assert_eq!(result.unwrap(), json!({"closed":true}));
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn dropping_an_inflight_query_discards_its_partial_protocol_stream() {
        let (stream, mut peer) = tokio::io::duplex(4096);
        let mut slot = Some(client(stream));
        let (_session, mut cancel) = tokio::sync::watch::channel(false);
        let agent = tokio::spawn(async move {
            read_client(&mut peer).await;
            assert_eq!(peer.read(&mut [0]).await.unwrap(), 0);
        });
        let result = tokio::time::timeout(
            Duration::from_millis(20),
            response(
                &mut slot,
                Request::Open {
                    path: protocol::Path::new(b"/tmp"),
                },
                Duration::from_secs(10),
                &mut cancel,
            ),
        )
        .await;
        assert!(result.is_err());
        assert!(slot.is_none());
        agent.await.unwrap();
    }

    #[tokio::test]
    async fn failed_reads_retire_their_channel_without_escalating_write_errors() {
        for mode in ["eof", "protocol", "timeout", "write"] {
            let (stream, mut peer) = tokio::io::duplex(4096);
            let mut slot = Some(client(stream));
            let (session, mut cancel) = tokio::sync::watch::channel(false);
            let agent = tokio::spawn(async move {
                read_client(&mut peer).await;
                match mode {
                    "protocol" => send(&mut peer, &close(Uuid::new_v4().to_string()))
                        .await
                        .unwrap(),
                    "timeout" => std::future::pending::<()>().await,
                    _ => {}
                }
            });
            let request = if mode == "write" {
                Request::Start {
                    operation_id: Uuid::new_v4().to_string(),
                    repo_id: "repo".into(),
                    expected_snapshot: "snapshot".into(),
                    action: protocol::Action::Stage {
                        entry_ids: vec!["entry".into()],
                        hunks: None,
                    },
                }
            } else {
                Request::Close {
                    repo_id: "repo".into(),
                }
            };
            let error = response(&mut slot, request, Duration::from_millis(20), &mut cancel)
                .await
                .unwrap_err();
            assert_eq!(
                error.code,
                if mode == "write" {
                    "TRANSPORT_ERROR"
                } else {
                    "READ_CHANNEL_ERROR"
                }
            );
            assert_eq!(slot.is_none(), mode != "write");
            assert!(!*session.borrow());
            agent.abort();
            let _ = agent.await;
        }
    }

    #[tokio::test]
    async fn keepalive_covers_all_four_lanes() {
        let mut peers = Vec::new();
        let lanes = std::array::from_fn(|_| {
            let (stream, mut peer) = tokio::io::duplex(1024);
            peers.push(tokio::spawn(async move {
                let ClientMessage::Ping(nonce) = read_client(&mut peer).await else {
                    panic!()
                };
                send(&mut peer, &ServerMessage::Pong(nonce)).await.unwrap();
            }));
            tokio::sync::Mutex::new(Some(client(stream)))
        });
        super::super::ping_lanes(&lanes).await;
        for peer in peers {
            tokio::time::timeout(Duration::from_secs(1), peer)
                .await
                .unwrap()
                .unwrap();
        }
        for lane in lanes {
            assert!(lane.into_inner().is_some());
        }
    }

    #[tokio::test]
    async fn failed_idle_keepalive_preserves_an_active_lane() {
        let mut peers = Vec::new();
        let lanes = std::array::from_fn(|_| {
            let (stream, peer) = tokio::io::duplex(1024);
            peers.push(peer);
            tokio::sync::Mutex::new(Some(client(stream)))
        });
        let mut active_peer = peers.remove(0);
        drop(peers);
        let mut active = lanes[0].lock().await;
        tokio::time::timeout(Duration::from_secs(1), super::super::ping_lanes(&lanes))
            .await
            .unwrap();
        for lane in &lanes[1..] {
            assert!(lane.lock().await.is_none());
        }
        let agent = tokio::spawn(async move {
            let ClientMessage::Ping(nonce) = read_client(&mut active_peer).await else {
                panic!()
            };
            send(&mut active_peer, &ServerMessage::Pong(nonce))
                .await
                .unwrap();
        });
        active.as_mut().unwrap().ping().await.unwrap();
        agent.await.unwrap();
    }

    #[tokio::test]
    async fn large_diff_streams_use_the_acknowledgement_window_without_deadlock() {
        let (stream, mut peer) = tokio::io::duplex(1024);
        let padding = protocol::CHUNK_SIZE * (protocol::STREAM_WINDOW as usize * 3 + 1);
        let data = wire::encode_diff(&diff(padding)).unwrap();
        let agent = tokio::spawn(async move {
            handshake(&mut peer).await;
            let ClientMessage::Call(call) = read_client(&mut peer).await else {
                panic!()
            };
            let stream_id = Uuid::new_v4().to_string();
            send(
                &mut peer,
                &ServerMessage::Begin(Begin {
                    id: call.id.clone(),
                    stream_id: stream_id.clone(),
                    snapshot: "snap".into(),
                }),
            )
            .await
            .unwrap();
            let mut sent = 0;
            for batch in data.chunks(protocol::CHUNK_SIZE * protocol::STREAM_WINDOW as usize) {
                let first = sent + 1;
                for chunk in batch.chunks(protocol::CHUNK_SIZE) {
                    sent += 1;
                    send(
                        &mut peer,
                        &ServerMessage::Chunk(Chunk {
                            id: call.id.clone(),
                            stream_id: stream_id.clone(),
                            seq: sent,
                            data: Binary(chunk.to_vec()),
                        }),
                    )
                    .await
                    .unwrap();
                }
                for expected in first..=sent {
                    assert!(
                        matches!(read_client(&mut peer).await, ClientMessage::Ack(Ack { stream_id: actual, seq }) if seq == expected && actual == stream_id)
                    );
                }
            }
            send(&mut peer, &stream_done(call.id, sent, data.len()))
                .await
                .unwrap();
        });
        tokio::time::timeout(Duration::from_secs(5), async {
            let (mut client, _) = Client::start(stream).await.unwrap();
            let value = client.request(diff_request()).await.unwrap();
            assert_eq!(
                value["diff"]["files"][0]["hunks"][0]["lines"][0]["content"]["display"]
                    .as_str()
                    .unwrap()
                    .len(),
                padding
            );
            agent.await.unwrap();
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn streamed_diff_requires_matching_identity_sequence_snapshot_and_totals() {
        for mode in ["valid", "sequence", "stream", "snapshot", "total"] {
            let (stream, mut peer) = tokio::io::duplex(8192);
            let agent = tokio::spawn(async move {
                handshake(&mut peer).await;
                let ClientMessage::Call(call) = read_client(&mut peer).await else {
                    panic!()
                };
                let stream_id = Uuid::new_v4().to_string();
                send(
                    &mut peer,
                    &ServerMessage::Begin(Begin {
                        id: call.id.clone(),
                        stream_id: stream_id.clone(),
                        snapshot: if mode == "snapshot" { "wrong" } else { "snap" }.into(),
                    }),
                )
                .await
                .unwrap();
                let data = wire::encode_diff(&diff(16)).unwrap();
                send(
                    &mut peer,
                    &ServerMessage::Chunk(Chunk {
                        id: call.id.clone(),
                        stream_id: if mode == "stream" {
                            Uuid::new_v4().to_string()
                        } else {
                            stream_id.clone()
                        },
                        seq: if mode == "sequence" { 2 } else { 1 },
                        data: Binary(data.clone()),
                    }),
                )
                .await
                .unwrap();
                if matches!(mode, "valid" | "total") {
                    assert!(matches!(
                        read_client(&mut peer).await,
                        ClientMessage::Ack(Ack { seq: 1, .. })
                    ));
                }
                send(
                    &mut peer,
                    &stream_done(call.id, 1, data.len() + usize::from(mode == "total")),
                )
                .await
                .unwrap();
            });
            let (mut client, _) = Client::start(stream).await.unwrap();
            let result = client.request(diff_request()).await;
            if mode == "valid" {
                assert_eq!(result.unwrap()["diff"]["truncated"], false);
            } else {
                assert_eq!(result.unwrap_err().code, "PROTOCOL_ERROR");
            }
            agent.await.unwrap();
        }
    }

    #[tokio::test]
    async fn rejects_wrong_response_identity_and_operation() {
        for wrong_operation in [false, true] {
            let (stream, mut peer) = tokio::io::duplex(4096);
            let agent = tokio::spawn(async move {
                handshake(&mut peer).await;
                let ClientMessage::Call(call) = read_client(&mut peer).await else {
                    panic!()
                };
                let reply = if wrong_operation {
                    stream_done(call.id, 0, 0)
                } else {
                    close(Uuid::new_v4().to_string())
                };
                send(&mut peer, &reply).await.unwrap();
            });
            let (mut client, _) = Client::start(stream).await.unwrap();
            assert_eq!(
                client
                    .request(Request::Close {
                        repo_id: "repo".into()
                    })
                    .await
                    .unwrap_err()
                    .code,
                "PROTOCOL_ERROR"
            );
            agent.await.unwrap();
        }
    }

    #[tokio::test]
    async fn coalesced_logs_and_replies_preserve_read_ahead_and_error_results() {
        let (stream, mut peer) = tokio::io::duplex(4096);
        let received = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let captured = received.clone();
        let mut client = client(stream);
        client.on_log = Some(Box::new(move |entry| captured.lock().unwrap().push(entry)));
        let agent = tokio::spawn(async move {
            for failed in [false, true] {
                let ClientMessage::Call(call) = read_client(&mut peer).await else {
                    panic!()
                };
                let log = ServerMessage::Log(Log {
                    id: call.id.clone(),
                    entry: CommandLog {
                        command: "git status".into(),
                        duration_ms: 3,
                        exit_code: Some(i32::from(failed)),
                        output: "diagnostic".into(),
                        interrupted: false,
                    },
                });
                let reply = if failed {
                    ServerMessage::Response(Response {
                        id: call.id,
                        result: Err(Error::new("GIT_ERROR", "failed")),
                    })
                } else {
                    close(call.id)
                };
                let frames = [log.encode().unwrap(), reply.encode().unwrap()].concat();
                peer.write_all(&frames).await.unwrap();
            }
        });
        assert_eq!(
            client
                .request(Request::Close {
                    repo_id: "repo".into()
                })
                .await
                .unwrap(),
            json!({"closed":true})
        );
        assert_eq!(
            client
                .request(Request::Close {
                    repo_id: "repo".into()
                })
                .await
                .unwrap_err()
                .code,
            "GIT_ERROR"
        );
        assert_eq!(received.lock().unwrap().len(), 2);
        agent.await.unwrap();
    }
}
