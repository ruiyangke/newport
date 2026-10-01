//! Async client for the bounded Git channel. A caller discards it after any
//! transport/protocol failure: partially read frames must never be reused.
use super::protocol::{self, Error, Message, Request};
use base64::{engine::general_purpose::STANDARD, Engine};
use serde_json::{json, Value};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use uuid::Uuid;

pub struct Client<S> {
    stream: S,
    methods: Vec<String>,

    pub on_log: Option<Box<dyn Fn(protocol::CommandLog) + Send + Sync>>,
}
impl<S: AsyncRead + AsyncWrite + Unpin> Client<S> {
    #[cfg(test)]
    pub async fn start(stream: S) -> Result<(Self, Value), Error> {
        Self::start_with_identity(stream, Uuid::new_v4().to_string()).await
    }
    pub async fn start_with_identity(
        mut stream: S,
        client_id: String,
    ) -> Result<(Self, Value), Error> {
        let hello = receive(&mut stream).await?;
        let Message::Hello {
            protocol: name,
            versions,
            limits,
            ..
        } = hello
        else {
            return Err(invalid());
        };
        if name != "newport.git"
            || !versions.contains(&protocol::VERSION)
            || limits["maxFrameBytes"].as_u64() != Some(protocol::MAX_FRAME as u64)
            || limits["maxChunkBytes"].as_u64() != Some(protocol::CHUNK_SIZE as u64)
        {
            return Err(Error::new(
                "UNSUPPORTED_PROTOCOL",
                "Install an agent with compatible Git support.",
            ));
        }
        let selected_version = protocol::VERSION;
        let id = Uuid::new_v4().to_string();
        send(
            &mut stream,
            &Message::Initialize {
                id: id.clone(),
                version: selected_version,
                client_id,
                client_version: env!("CARGO_PKG_VERSION").into(),
                command_logs: limits["commandLogs"] == true,
            },
        )
        .await?;
        let Message::Ready {
            id: reply,
            version,
            capabilities,
        } = receive(&mut stream).await?
        else {
            return Err(invalid());
        };
        if reply != id || version != selected_version {
            return Err(invalid());
        }
        let methods = capabilities["methods"]
            .as_array()
            .ok_or_else(invalid)?
            .iter()
            .map(|v| v.as_str().map(str::to_owned).ok_or_else(invalid))
            .collect::<Result<Vec<_>, _>>()?;
        Ok((
            Self {
                stream,
                methods,
                on_log: None,
            },
            json!({"capabilities":capabilities,"limits":limits}),
        ))
    }
    pub async fn ping(&mut self) -> Result<(), Error> {
        let nonce = Uuid::new_v4().to_string();
        send(
            &mut self.stream,
            &Message::Ping {
                nonce: nonce.clone(),
            },
        )
        .await?;
        if !matches!(receive(&mut self.stream).await?,Message::Pong { nonce:reply } if reply==nonce)
        {
            return Err(invalid());
        }
        Ok(())
    }
    pub async fn request(&mut self, request: Request) -> Result<Value, Error> {
        let value = serde_json::to_value(&request).map_err(|_| invalid())?;
        let method = value["method"].as_str().ok_or_else(invalid)?.to_owned();
        if !self.methods.contains(&method) {
            return Err(Error::new(
                "UNSUPPORTED_METHOD",
                "The agent does not support this Git method.",
            ));
        }
        let expected_snapshot = match &request {
            Request::Diff { snapshot, .. } => Some(snapshot.as_str()),
            Request::CommitDiff { commit_oid, .. } => Some(commit_oid.as_str()),
            _ => None,
        };
        let id = Uuid::new_v4().to_string();
        send(
            &mut self.stream,
            &Message::Request {
                id: id.clone(),
                method,
                params: value["params"].clone(),
            },
        )
        .await?;
        let mut stream_id = None;
        let mut data = Vec::new();
        let mut last_seq = 0;
        let mut log_count = 0;
        loop {
            match receive(&mut self.stream).await? {
                Message::CommandLog { id: reply, entry } if reply == id => {
                    log_count += 1;
                    if log_count > 32 || entry.command.len() > 256 || entry.output.len() > 32768 {
                        return Err(invalid());
                    }
                    if let Some(callback) = &self.on_log {
                        callback(entry);
                    }
                }
                Message::Begin {
                    id: reply,
                    stream_id: stream,
                    snapshot,
                } if reply == id
                    && stream_id.is_none()
                    && expected_snapshot == Some(snapshot.as_str()) =>
                {
                    stream_id = Some(stream)
                }
                Message::Chunk {
                    id: reply,
                    stream_id: stream,
                    seq,
                    bytes_b64,
                } if reply == id && stream_id.as_ref() == Some(&stream) && seq == last_seq + 1 => {
                    let bytes = STANDARD.decode(&bytes_b64).map_err(|_| invalid())?;
                    if bytes.is_empty()
                        || bytes.len() > protocol::CHUNK_SIZE
                        || data.len() + bytes.len() > protocol::MAX_DIFF
                    {
                        return Err(Error::new(
                            "PROTOCOL_ERROR",
                            "Git stream exceeds its limit.",
                        ));
                    }
                    data.extend(bytes);
                    last_seq = seq;
                    send(
                        &mut self.stream,
                        &Message::Ack {
                            stream_id: stream,
                            seq,
                        },
                    )
                    .await?;
                }
                Message::Response {
                    id: reply,
                    result: Some(value),
                    error: None,
                } if reply == id => {
                    if stream_id.is_some() {
                        if value["lastSeq"].as_u64() != Some(last_seq as u64)
                            || value["bytes"].as_u64() != Some(data.len() as u64)
                        {
                            return Err(invalid());
                        }
                        let diff = protocol::decode_value(&data).map_err(|_| invalid())?;
                        return Ok(json!({"snapshot":expected_snapshot,"diff":diff}));
                    }
                    if expected_snapshot.is_some() {
                        return Err(invalid());
                    }
                    return Ok(value);
                }
                Message::Response {
                    id: reply,
                    result: None,
                    error: Some(error),
                } if reply == id => return Err(error),
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
async fn receive(stream: &mut (impl AsyncRead + Unpin)) -> Result<Message, Error> {
    let mut header = [0; 5];
    stream
        .read_exact(&mut header)
        .await
        .map_err(|_| Error::transport("Git channel closed or could not be read."))?;
    if header[0] != b'M' {
        return Err(Error::new(
            "UNSUPPORTED_PROTOCOL",
            "Update the remote agent: this app requires MessagePack Git protocol v4.",
        ));
    }
    let size = protocol::payload_length(header).map_err(|_| invalid())?;
    let mut bytes = vec![0; size];
    stream
        .read_exact(&mut bytes)
        .await
        .map_err(|_| Error::transport("Incomplete Git response."))?;
    protocol::decode(&bytes).map_err(|_| invalid())
}
async fn send(stream: &mut (impl AsyncWrite + Unpin), message: &Message) -> Result<(), Error> {
    let frame = protocol::encode(message).map_err(|_| invalid())?;
    stream
        .write_all(&frame)
        .await
        .map_err(|_| Error::transport("Could not send Git request."))?;
    stream
        .flush()
        .await
        .map_err(|_| Error::transport("Could not flush Git request."))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn failed_reads_retire_their_channel_without_escalating_write_errors() {
        use super::super::reads;
        use std::time::Duration;
        for mode in ["eof", "protocol", "timeout", "write"] {
            let (stream, mut peer) = tokio::io::duplex(4096);
            let mut slot = Some(Client {
                on_log: None,
                stream,
                methods: vec!["repo.open".into(), "operation.start".into()],
            });
            let (session, mut cancel) = tokio::sync::watch::channel(false);
            let agent = tokio::spawn(async move {
                receive(&mut peer).await.unwrap();
                match mode {
                    "protocol" => {
                        send(&mut peer, &Message::success("wrong-id".into(), json!({})))
                            .await
                            .unwrap();
                    }
                    "timeout" => {
                        std::future::pending::<()>().await;
                    }
                    _ => {}
                }
            });
            let request = if mode == "write" {
                serde_json::from_value(json!({"method":"operation.start","params":{"operationId":Uuid::new_v4().to_string(),"repoId":"repo","expectedSnapshot":"snapshot","action":{"kind":"stage","entryIds":["entry"]}}})).unwrap()
            } else {
                Request::Open {
                    path: protocol::Path::new(b"/tmp"),
                }
            };
            let error = reads::response(
                &mut slot,
                request,
                Duration::from_millis(20),
                &mut cancel,
                &mut None,
            )
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

    /// Controlled delayed replies isolate native cancellation latency from
    /// repository scan cost. Baseline and cancelled reads use identical frames.
    #[tokio::test]
    async fn benchmark_native_read_cancellation() {
        use super::super::reads;
        use std::time::{Duration, Instant};
        for size in [1024, 64 * 1024, 512 * 1024] {
            for cancel_read in [false, true] {
                let registry = reads::Reads::default();
                let server = Uuid::new_v4();
                let id = registry.register(server).unwrap();
                let mut read = Some(registry.claim(server, id).unwrap());
                let (stream, mut peer) = tokio::io::duplex(4096);
                let mut slot = Some(Client {
                    on_log: None,
                    stream,
                    methods: vec!["repo.open".into()],
                });
                let (_session, mut session_cancel) = tokio::sync::watch::channel(false);
                let (sent, dispatched) = tokio::sync::oneshot::channel();
                let agent = tokio::spawn(async move {
                    let Message::Request { id, .. } = receive(&mut peer).await.unwrap() else {
                        panic!("request expected")
                    };
                    sent.send(()).unwrap();
                    tokio::time::sleep(Duration::from_millis(80)).await;
                    let response = Message::success(id, json!({"data":"x".repeat(size)}));
                    let bytes = protocol::encode(&response).unwrap().len();
                    let _ = send(&mut peer, &response).await;
                    bytes
                });
                let started = Instant::now();
                let cancel = async {
                    dispatched.await.unwrap();
                    if cancel_read {
                        registry.cancel(server, id);
                    }
                };
                let (result, ()) = tokio::join!(
                    reads::response(
                        &mut slot,
                        Request::Open {
                            path: protocol::Path::new(b"/tmp")
                        },
                        Duration::from_secs(2),
                        &mut session_cancel,
                        &mut read
                    ),
                    cancel
                );
                let elapsed = started.elapsed().as_secs_f64() * 1000.;
                let response_bytes = if cancel_read {
                    assert_eq!(result.unwrap_err().code, "READ_CANCELLED");
                    agent.abort();
                    let _ = agent.await;
                    0
                } else {
                    assert_eq!(result.unwrap()["data"].as_str().unwrap().len(), size);
                    agent.await.unwrap()
                };
                eprintln!(
                    "CANCEL_BENCH {}",
                    json!({"payloadBytes":size,"cancelled":cancel_read,"elapsedMs":elapsed,"responseBytes":response_bytes,"simulatedReplyDelayMs":80})
                );
            }
        }
    }

    #[tokio::test]
    async fn cancelling_a_dispatched_read_closes_only_its_stream() {
        use super::super::reads;
        use std::time::Duration;
        let registry = reads::Reads::default();
        let server = Uuid::new_v4();
        let id = registry.register(server).unwrap();
        let mut read = Some(registry.claim(server, id).unwrap());
        let (stream, mut peer) = tokio::io::duplex(1024);
        let mut slot = Some(Client {
            on_log: None,
            stream,
            methods: vec!["repo.open".into()],
        });
        let (session, mut session_cancel) = tokio::sync::watch::channel(false);
        let request = Request::Open {
            path: protocol::Path::new(b"/tmp"),
        };
        let cancel = async {
            assert!(matches!(
                receive(&mut peer).await.unwrap(),
                Message::Request { .. }
            ));
            registry.cancel(server, id);
            let mut byte = [0];
            assert_eq!(peer.read(&mut byte).await.unwrap(), 0);
        };
        let (result, ()) = tokio::time::timeout(Duration::from_secs(1), async {
            tokio::join!(
                reads::response(
                    &mut slot,
                    request,
                    Duration::from_secs(35),
                    &mut session_cancel,
                    &mut read
                ),
                cancel
            )
        })
        .await
        .unwrap();
        assert_eq!(result.unwrap_err().code, "READ_CANCELLED");
        assert!(slot.is_none());
        assert!(!*session.borrow());
        // Another lane still completes a framed request on the same session.
        let (stream, mut peer) = tokio::io::duplex(1024);
        let mut other = Some(Client {
            on_log: None,
            stream,
            methods: vec!["repo.open".into()],
        });
        let agent = async {
            let Message::Request { id, .. } = receive(&mut peer).await.unwrap() else {
                panic!("request expected")
            };
            send(&mut peer, &Message::success(id, json!({"ok":true})))
                .await
                .unwrap();
        };
        let mut no_read = None;
        let (result, ()) = tokio::join!(
            reads::response(
                &mut other,
                Request::Open {
                    path: protocol::Path::new(b"/tmp")
                },
                Duration::from_secs(1),
                &mut session_cancel,
                &mut no_read
            ),
            agent
        );
        assert_eq!(result.unwrap()["ok"], true);
        assert!(other.is_some());
    }

    #[tokio::test]
    async fn keepalive_covers_all_four_lanes() {
        let mut peers = Vec::new();
        let lanes = std::array::from_fn(|_| {
            let (stream, mut peer) = tokio::io::duplex(1024);
            peers.push(tokio::spawn(async move {
                let Message::Ping { nonce } = receive(&mut peer).await.unwrap() else {
                    panic!("expected keepalive")
                };
                send(&mut peer, &Message::Pong { nonce }).await.unwrap();
            }));
            tokio::sync::Mutex::new(Some(Client {
                on_log: None,
                stream,
                methods: vec![],
            }))
        });
        super::super::ping_lanes(&lanes).await;
        for peer in peers {
            tokio::time::timeout(std::time::Duration::from_secs(1), peer)
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
            tokio::sync::Mutex::new(Some(Client {
                on_log: None,
                stream,
                methods: vec![],
            }))
        });
        let mut active_peer = peers.remove(0);
        drop(peers); // The three idle agents exited; the active stream is live.
        let mut active = lanes[0].lock().await;
        tokio::time::timeout(
            std::time::Duration::from_secs(1),
            super::super::ping_lanes(&lanes),
        )
        .await
        .unwrap();
        for lane in &lanes[1..] {
            assert!(
                lane.lock().await.is_none(),
                "failed stream must be discarded"
            );
        }
        let response = tokio::spawn(async move {
            let Message::Ping { nonce } = receive(&mut active_peer).await.unwrap() else {
                panic!("unexpected protocol data on active lane")
            };
            send(&mut active_peer, &Message::Pong { nonce })
                .await
                .unwrap();
        });
        active.as_mut().unwrap().ping().await.unwrap();
        response.await.unwrap();
    }
    async fn handshake(stream: &mut tokio::io::DuplexStream) {
        send(stream,&Message::Hello { protocol:"newport.git".into(),versions:vec![protocol::VERSION],instance_id:"test".into(),limits:json!({"maxFrameBytes":protocol::MAX_FRAME,"maxChunkBytes":protocol::CHUNK_SIZE}) }).await.unwrap();
        let Message::Initialize { id, .. } = receive(stream).await.unwrap() else {
            panic!()
        };
        send(
            stream,
            &Message::Ready {
                id,
                version: protocol::VERSION,
                capabilities: json!({"methods":protocol::METHODS}),
            },
        )
        .await
        .unwrap();
    }
    #[tokio::test]
    async fn existing_client_accepts_pipelined_chunks() {
        let (a, mut b) = tokio::io::duplex(1024);
        let padding = protocol::CHUNK_SIZE * (protocol::STREAM_WINDOW as usize * 3 + 1);
        let stream_id = Uuid::new_v4().to_string();
        let data =
            protocol::encode_value(&json!({"files":[],"padding":"x".repeat(padding)})).unwrap();
        let expected = data.len();
        let agent = tokio::spawn(async move {
            handshake(&mut b).await;
            let Message::Request { id, .. } = receive(&mut b).await.unwrap() else {
                panic!()
            };
            send(
                &mut b,
                &Message::Begin {
                    id: id.clone(),
                    stream_id: stream_id.clone(),
                    snapshot: "snap".into(),
                },
            )
            .await
            .unwrap();
            let mut sent = 0;
            for batch in data.chunks(protocol::CHUNK_SIZE * protocol::STREAM_WINDOW as usize) {
                let first = sent + 1;
                for chunk in batch.chunks(protocol::CHUNK_SIZE) {
                    sent += 1;
                    send(
                        &mut b,
                        &Message::Chunk {
                            id: id.clone(),
                            stream_id: stream_id.clone(),
                            seq: sent,
                            bytes_b64: STANDARD.encode(chunk),
                        },
                    )
                    .await
                    .unwrap();
                }
                for expected in first..=sent {
                    assert!(
                        matches!(receive(&mut b).await.unwrap(), Message::Ack { seq, .. } if seq == expected)
                    );
                }
            }
            send(
                &mut b,
                &Message::success(id, json!({"lastSeq":sent,"bytes":expected})),
            )
            .await
            .unwrap();
        });
        let result = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            let (mut client, _) = Client::start(a).await.unwrap();
            let value = client
                .request(Request::Diff {
                    repo_id: "repo".into(),
                    snapshot: "snap".into(),
                    entry_id: "entry".into(),
                    side: protocol::Side::IndexToWorktree,
                    context_lines: 3,
                })
                .await
                .unwrap();
            assert_eq!(value["diff"]["padding"].as_str().unwrap().len(), padding);
            agent.await.unwrap();
        })
        .await;
        assert!(
            result.is_ok(),
            "pipelining must not deadlock an existing client"
        );
    }

    #[tokio::test]
    async fn msgpack_handshake_and_pipelined_binary_chunks() {
        let selected_version = protocol::VERSION;
        let (a, mut b) = tokio::io::duplex(1024);
        let padding = protocol::CHUNK_SIZE * (protocol::STREAM_WINDOW as usize * 3 + 1);
        let stream_id = Uuid::new_v4().to_string();
        let data =
            protocol::encode_value(&json!({"files":[],"padding":"x".repeat(padding)})).unwrap();
        let expected = data.len();
        let agent = tokio::spawn(async move {
            send(&mut b, &Message::Hello { protocol: "newport.git".into(), versions: vec![selected_version], instance_id:"test".into(), limits:json!({"maxFrameBytes":protocol::MAX_FRAME,"maxChunkBytes":protocol::CHUNK_SIZE}) }).await.unwrap();
            let Message::Initialize { id, version, .. } = receive(&mut b).await.unwrap() else {
                panic!()
            };
            assert_eq!(version, selected_version);
            send(
                &mut b,
                &Message::Ready {
                    id,
                    version,
                    capabilities: json!({"methods":protocol::METHODS}),
                },
            )
            .await
            .unwrap();
            let Message::Request { id, .. } = receive(&mut b).await.unwrap() else {
                panic!()
            };
            send(
                &mut b,
                &Message::Begin {
                    id: id.clone(),
                    stream_id: stream_id.clone(),
                    snapshot: "snap".into(),
                },
            )
            .await
            .unwrap();
            let mut sent = 0;
            for batch in data.chunks(protocol::CHUNK_SIZE * protocol::STREAM_WINDOW as usize) {
                let first = sent + 1;
                for chunk in batch.chunks(protocol::CHUNK_SIZE) {
                    sent += 1;
                    send(
                        &mut b,
                        &Message::Chunk {
                            id: id.clone(),
                            stream_id: stream_id.clone(),
                            seq: sent,
                            bytes_b64: STANDARD.encode(chunk),
                        },
                    )
                    .await
                    .unwrap();
                }
                for expected in first..=sent {
                    assert!(
                        matches!(receive(&mut b).await.unwrap(), Message::Ack { seq, .. } if seq == expected)
                    );
                }
            }
            send(
                &mut b,
                &Message::success(id, json!({"lastSeq":sent,"bytes":expected})),
            )
            .await
            .unwrap();
        });
        let result = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            let (mut client, _) = Client::start(a).await.unwrap();
            let value = client
                .request(Request::Diff {
                    repo_id: "repo".into(),
                    snapshot: "snap".into(),
                    entry_id: "entry".into(),
                    side: protocol::Side::IndexToWorktree,
                    context_lines: 3,
                })
                .await
                .unwrap();
            assert_eq!(value["diff"]["padding"].as_str().unwrap().len(), padding);
            agent.await.unwrap();
        })
        .await;
        assert!(
            result.is_ok(),
            "pipelining must not deadlock an existing client"
        );
    }

    #[tokio::test]
    async fn streamed_diff_requires_order_and_acknowledgement() {
        let (a, mut b) = tokio::io::duplex(128);
        let agent = tokio::spawn(async move {
            handshake(&mut b).await;
            let Message::Request { id, .. } = receive(&mut b).await.unwrap() else {
                panic!()
            };
            send(
                &mut b,
                &Message::Begin {
                    id: id.clone(),
                    stream_id: "s".into(),
                    snapshot: "snap".into(),
                },
            )
            .await
            .unwrap();
            let data = protocol::encode_value(&json!({"files":[],"truncated":false})).unwrap();
            send(
                &mut b,
                &Message::Chunk {
                    id: id.clone(),
                    stream_id: "s".into(),
                    seq: 1,
                    bytes_b64: STANDARD.encode(&data),
                },
            )
            .await
            .unwrap();
            assert!(matches!(
                receive(&mut b).await.unwrap(),
                Message::Ack { seq: 1, .. }
            ));
            send(
                &mut b,
                &Message::success(id, json!({"lastSeq":1,"bytes":data.len()})),
            )
            .await
            .unwrap();
        });
        let (mut client, _) = Client::start(a).await.unwrap();
        let result = client
            .request(Request::Diff {
                repo_id: "repo".into(),
                snapshot: "snap".into(),
                entry_id: "entry".into(),
                side: protocol::Side::HeadToIndex,
                context_lines: 3,
            })
            .await
            .unwrap();
        assert_eq!(result["diff"]["truncated"], false);
        agent.await.unwrap();
    }
    #[tokio::test]
    async fn rejects_wrong_response_id() {
        let (a, mut b) = tokio::io::duplex(4096);
        let agent = tokio::spawn(async move {
            handshake(&mut b).await;
            receive(&mut b).await.unwrap();
            send(&mut b, &Message::success("wrong".into(), json!({})))
                .await
                .unwrap();
        });
        let (mut client, _) = Client::start(a).await.unwrap();
        let error = client
            .request(Request::Open {
                path: protocol::Path::new(b"/tmp"),
            })
            .await
            .unwrap_err();
        assert_eq!(error.code, "PROTOCOL_ERROR");
        agent.await.unwrap();
    }
    #[tokio::test]
    async fn command_logs_do_not_change_success_or_error_responses() {
        let (stream, mut peer) = tokio::io::duplex(4096);
        let received = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let captured = received.clone();
        let mut client = Client {
            stream,
            methods: vec!["repo.open".into()],
            on_log: Some(Box::new(move |entry| captured.lock().unwrap().push(entry))),
        };
        let agent = tokio::spawn(async move {
            for failed in [false, true] {
                let Message::Request { id, .. } = receive(&mut peer).await.unwrap() else {
                    panic!("request expected")
                };
                send(
                    &mut peer,
                    &Message::CommandLog {
                        id: id.clone(),
                        entry: protocol::CommandLog {
                            command: "git status".into(),
                            duration_ms: 3,
                            exit_code: Some(if failed { 1 } else { 0 }),
                            output: "diagnostic".into(),
                            interrupted: false,
                        },
                    },
                )
                .await
                .unwrap();
                let reply = if failed {
                    Message::failure(id, Error::new("GIT_ERROR", "failed"))
                } else {
                    Message::success(id, json!({"ok":true}))
                };
                send(&mut peer, &reply).await.unwrap();
            }
        });
        let request = || Request::Open {
            path: protocol::Path::new(b"/tmp"),
        };
        assert_eq!(client.request(request()).await.unwrap(), json!({"ok":true}));
        assert_eq!(
            client.request(request()).await.unwrap_err().code,
            "GIT_ERROR"
        );
        assert_eq!(received.lock().unwrap().len(), 2);
        agent.await.unwrap();
    }
}
