use super::*;
use russh::server;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

struct Client;
impl client::Handler for Client {
    type Error = anyhow::Error;
    async fn check_server_key(
        &mut self,
        _: &russh::keys::PublicKeyOrCertificate,
    ) -> anyhow::Result<bool> {
        Ok(true)
    }
}
struct Server(tokio::sync::mpsc::UnboundedSender<russh::Channel<server::Msg>>);
impl server::Handler for Server {
    type Error = anyhow::Error;
    async fn auth_none(&mut self, _: &str) -> anyhow::Result<server::Auth> {
        Ok(server::Auth::Accept)
    }
    async fn channel_open_direct_streamlocal(
        &mut self,
        channel: russh::Channel<server::Msg>,
        path: &str,
        reply: server::ChannelOpenHandle,
        _: &mut server::Session,
    ) -> anyhow::Result<()> {
        assert_eq!(path, "/tmp/fixture/streams.sock");
        reply.accept().await;
        self.0
            .send(channel)
            .map_err(|_| anyhow::anyhow!("Fixture closed"))?;
        Ok(())
    }
}
struct Fixture {
    client: client::Handle<Client>,
    channels: tokio::sync::mpsc::UnboundedReceiver<russh::Channel<server::Msg>>,
    server: tokio::task::JoinHandle<()>,
}
impl Fixture {
    async fn new(window_size: u32) -> Self {
        let key = russh::keys::PrivateKey::from(
            russh::keys::ssh_key::private::Ed25519Keypair::from_seed(&[23; 32]),
        );
        let config = server::Config {
            keys: vec![key],
            window_size,
            auth_rejection_time: Duration::ZERO,
            ..Default::default()
        };
        let (local, remote) = tokio::io::duplex(64 * 1024);
        let (send, channels) = tokio::sync::mpsc::unbounded_channel();
        let server = tokio::spawn(async move {
            let session = server::run_stream(Arc::new(config), remote, Server(send))
                .await
                .unwrap();
            let _ = session.await;
        });
        let mut client = client::connect_stream(Arc::new(client::Config::default()), local, Client)
            .await
            .unwrap();
        assert!(client.authenticate_none("fixture").await.unwrap().success());
        Self {
            client,
            channels,
            server,
        }
    }
    async fn open(&mut self) -> (russh::Channel<client::Msg>, russh::Channel<server::Msg>) {
        let local = self
            .client
            .channel_open_direct_streamlocal("/tmp/fixture/streams.sock")
            .await
            .unwrap();
        (local, self.channels.recv().await.unwrap())
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.server.abort();
    }
}
async fn bounded<F: std::future::Future>(future: F) -> F::Output {
    tokio::time::timeout(Duration::from_secs(12), future)
        .await
        .expect("Fixture stalled")
}
fn opening() -> admission::Open {
    admission::Open {
        version: admission::VERSION,
        runtime_id: uuid::Uuid::new_v4().to_string(),
        worker_id: uuid::Uuid::new_v4().to_string(),
        build_sha256: "a".repeat(64),
        ticket: newport_protocol::common::Secret {
            value: "b".repeat(64).into_bytes().into(),
        },
        service: "monitor".into(),
        service_version: 1,
        contract_id: Some(newport_protocol::monitor::CONTRACT_ID.into()),
        arguments: vec!["literal ' argument".into()],
    }
}
fn admission(open: &admission::Open) -> Vec<u8> {
    admission::Admission::Started(admission::Started {
        runtime_id: open.runtime_id.clone(),
        worker_id: open.worker_id.clone(),
        build_sha256: open.build_sha256.clone(),
    })
    .encode()
    .unwrap()
}

fn output(data: &[u8]) -> Vec<u8> {
    wire::encode_opaque(
        &Chunk::new(data.to_vec().into()).unwrap(),
        Direction::FromWorker,
    )
    .unwrap()
}
fn stderr(data: &[u8]) -> Vec<u8> {
    wire::encode_server(&ServerControl::Stderr(
        Chunk::new(data.to_vec().into()).unwrap(),
    ))
    .unwrap()
}
fn exit(outcome: Exit) -> Vec<u8> {
    wire::encode_server(&ServerControl::Exit(outcome)).unwrap()
}
async fn read_input(stream: &mut russh::ChannelStream<server::Msg>) -> Option<Vec<u8>> {
    let mut prefix = LengthDecoder::default();
    let length = loop {
        if let Some(length) = prefix
            .push(stream.read_u8().await.unwrap(), wire::OPAQUE_LIMITS.message)
            .unwrap()
        {
            break length;
        }
    };
    let mut body = vec![0; length];
    stream.read_exact(&mut body).await.unwrap();
    match wire::decode_client(body.into(), &wire::OPAQUE_LIMITS).unwrap() {
        wire::Frame::Service(body) => Some(wire::decode_opaque(body).unwrap().into_data().to_vec()),
        wire::Frame::Control(ClientControl::InputComplete) => None,
        other => panic!("unexpected worker input: {other:?}"),
    }
}
async fn read_open(stream: &mut russh::ChannelStream<server::Msg>) -> admission::Open {
    let mut prefix = LengthDecoder::default();
    let length = loop {
        if let Some(length) = prefix
            .push(
                stream.read_u8().await.unwrap(),
                admission::Open::LIMITS.message,
            )
            .unwrap()
        {
            break length;
        }
    };
    let mut bytes = vec![0; length];
    stream.read_exact(&mut bytes).await.unwrap();
    admission::Open::decode(bytes.into()).unwrap()
}

#[tokio::test]
async fn admission_coalesces_output_and_input_eof_preserves_final_output() {
    bounded(async {
        let mut fixture = Fixture::new(64 * 1024).await;
        let (local, remote) = fixture.open().await;
        let remote = tokio::spawn(async move {
            let mut stream = remote.into_stream();
            let open = read_open(&mut stream).await;
            assert_eq!(open.arguments, ["literal ' argument"]);
            let mut reply = admission(&open);
            reply.extend(output(b"ready"));
            stream.write_all(&reply).await.unwrap();
            let mut length = 0;
            loop {
                let Some(data) = read_input(&mut stream).await else {
                    break;
                };
                assert!(data.iter().all(|b| *b == 42));
                length += data.len();
                stream.write_all(&output(&data)).await.unwrap();
            }
            // A raw SSH EOF here would cancel the runtime's worker rather than
            // merely close stdin. The semantic EOF frame must leave IO open.
            assert!(
                tokio::time::timeout(Duration::from_millis(20), stream.read_u8())
                    .await
                    .is_err()
            );
            stream.write_all(&output(b"tail")).await.unwrap();
            stream.write_all(&exit(Exit::Status(0))).await.unwrap();
            stream.flush().await.unwrap();
            stream.shutdown().await.unwrap();
            length
        });
        let channel = Channel::new(local, Some(Transport::Opaque))
            .admit(&opening())
            .await
            .unwrap();
        let stream = channel.into_stream();
        assert!(!stream.uses_session_slot());
        let (mut reader, mut writer) = tokio::io::split(stream);
        let input = vec![42; 2 * 1024 * 1024];
        let mut output = Vec::new();
        let ((), _) = tokio::try_join!(
            async {
                writer.write_all(&input).await?;
                writer.flush().await?;
                writer.shutdown().await
            },
            reader.read_to_end(&mut output)
        )
        .unwrap();
        assert_eq!(remote.await.unwrap(), input.len());
        assert_eq!(&output[..5], b"ready");
        assert_eq!(&output[5..output.len() - 4], &input);
        assert!(output.ends_with(b"tail"));
    })
    .await;
}

#[tokio::test]
async fn cancelled_wait_retains_partial_headers_and_payloads() {
    bounded(async {
        let mut fixture = Fixture::new(64 * 1024).await;
        let (local, remote) = fixture.open().await;
        let mut channel = Channel::new(local, Some(Transport::Opaque));
        let payload = vec![b'x'; 512];
        let bytes = output(&payload);
        remote.data(&bytes[..1]).await.unwrap();
        assert!(
            tokio::time::timeout(Duration::from_millis(20), channel.wait())
                .await
                .is_err()
        );
        remote.data(&bytes[1..7]).await.unwrap();
        assert!(
            tokio::time::timeout(Duration::from_millis(20), channel.wait())
                .await
                .is_err()
        );
        remote.data(&bytes[7..]).await.unwrap();
        let Some(ChannelMsg::Data { data }) = channel.wait().await.unwrap() else {
            panic!("Missing payload")
        };
        assert_eq!(&data[..], payload);
        remote.data(&exit(Exit::Status(3))[..]).await.unwrap();
        remote.eof().await.unwrap();
        assert!(matches!(
            channel.wait().await.unwrap(),
            Some(ChannelMsg::ExitStatus { exit_status: 3 })
        ));
        assert!(channel.wait().await.unwrap().is_none());
    })
    .await;
}

#[tokio::test]
async fn final_status_rejects_later_ssh_packets() {
    bounded(async {
        let mut fixture = Fixture::new(64 * 1024).await;
        for trailing in 0..4 {
            let (local, remote) = fixture.open().await;
            let mut channel = Channel::new(local, Some(Transport::GIT));
            remote.data(&exit(Exit::Status(0))[..]).await.unwrap();
            assert!(matches!(
                channel.wait().await.unwrap(),
                Some(ChannelMsg::ExitStatus { exit_status: 0 })
            ));
            // Observing the status before sending this packet ensures it cannot
            // share the pending buffer that contained the final protocol frame.
            match trailing {
                0 => remote.data(&b"late output"[..]).await,
                1 => remote.extended_data(1, &b"late stderr"[..]).await,
                2 => remote.exit_status(0).await,
                _ => remote.data(&exit(Exit::Status(0))[..]).await,
            }
            .unwrap();
            assert!(
                channel
                    .wait()
                    .await
                    .unwrap_err()
                    .contains("after final status")
            );
            assert!(channel.wait().await.unwrap().is_none());
            channel.close().await.unwrap();
        }
    })
    .await;
}

#[tokio::test]
async fn stream_waits_for_transport_closure_and_rejects_later_data() {
    bounded(async {
        let mut fixture = Fixture::new(64 * 1024).await;
        let (local, remote) = fixture.open().await;
        let mut stream = Channel::new(local, Some(Transport::Opaque)).into_stream();
        remote.data(&exit(Exit::Status(0))[..]).await.unwrap();
        let mut received = Vec::new();
        assert!(
            tokio::time::timeout(Duration::from_millis(20), stream.read_to_end(&mut received),)
                .await
                .is_err()
        );
        remote.data(&output(b"too late")[..]).await.unwrap();
        remote.eof().await.unwrap();
        assert!(
            stream
                .read_to_end(&mut received)
                .await
                .unwrap_err()
                .to_string()
                .contains("after final status")
        );
        assert!(received.is_empty());
    })
    .await;
}

#[tokio::test]
async fn cancelled_exit_drain_does_not_extend_the_closure_deadline() {
    bounded(async {
        let mut fixture = Fixture::new(64 * 1024).await;
        let (local, remote) = fixture.open().await;
        let mut channel = Channel::new(local, Some(Transport::Opaque));
        remote.data(&exit(Exit::Status(0))[..]).await.unwrap();
        assert!(matches!(
            channel.wait().await.unwrap(),
            Some(ChannelMsg::ExitStatus { exit_status: 0 })
        ));
        assert!(
            tokio::time::timeout(EXIT_CLOSE_TIMEOUT / 4 * 3, channel.wait())
                .await
                .is_err()
        );
        // A restarted close timeout would exceed this bound. The original
        // deadline must survive cancellation while the peer remains open.
        let error = tokio::time::timeout(EXIT_CLOSE_TIMEOUT / 2, channel.wait())
            .await
            .expect("Cancelled wait extended the closure deadline")
            .unwrap_err();
        assert!(error.contains("did not close after final status"));
        assert!(channel.wait().await.unwrap().is_none());
        channel.close().await.unwrap();
    })
    .await;
}

#[tokio::test]
async fn malformed_or_truncated_output_never_becomes_clean_eof() {
    bounded(async {
        let mut fixture = Fixture::new(64 * 1024).await;
        let mut oversized = Vec::new();
        newport_protocol::encoding::protobuf::write_frame(
            &mut oversized,
            &vec![0; wire::OPAQUE_LIMITS.message + 1],
        )
        .unwrap();
        let mut invalid_signal = exit(Exit::Signal(
            newport_protocol::worker::Signal::new(1).unwrap(),
        ));
        *invalid_signal.last_mut().unwrap() = 0;
        for bytes in [
            vec![0x80],
            vec![4, 0x0a, 3, b'x'],
            oversized,
            wire::encode_client(ClientControl::InputComplete).unwrap(),
            invalid_signal,
            [exit(Exit::Status(0)), vec![9]].concat(),
        ] {
            let (local, remote) = fixture.open().await;
            let mut stream = Channel::new(local, Some(Transport::Opaque)).into_stream();
            remote.data(&bytes[..]).await.unwrap();
            remote.eof().await.unwrap();
            assert!(stream.read_to_end(&mut Vec::new()).await.is_err());
        }
    })
    .await;
}

#[tokio::test]
async fn stream_reports_nonzero_status_signal_and_bounded_stderr_after_stdout() {
    bounded(async {
        let mut fixture = Fixture::new(64 * 1024).await;
        for outcome in [
            Exit::Status(7),
            Exit::Signal(newport_protocol::worker::Signal::new(7).unwrap()),
        ] {
            let (local, remote) = fixture.open().await;
            let mut stream = Channel::new(local, Some(Transport::Opaque)).into_stream();
            let sender = tokio::spawn(async move {
                remote.data(&output(b"stdout")[..]).await.unwrap();
                for _ in 0..3 {
                    remote
                        .data(&stderr(&vec![b'x'; MAX_CHUNK])[..])
                        .await
                        .unwrap();
                }
                remote.data(&stderr(b"diagnostic tail")[..]).await.unwrap();
                remote.data(&exit(outcome)[..]).await.unwrap();
                remote.eof().await.unwrap();
            });
            let mut bytes = Vec::new();
            let error = stream
                .read_to_end(&mut bytes)
                .await
                .unwrap_err()
                .to_string();
            assert_eq!(bytes, b"stdout");
            assert!(error.ends_with("diagnostic tail") && error.len() <= 65536 + 64);
            assert!(error.contains(if matches!(outcome, Exit::Status(_)) {
                "status 7"
            } else {
                "signal 7"
            }));
            sender.await.unwrap();
        }
    })
    .await;
}

#[tokio::test]
async fn admission_identity_mismatch_or_loss_is_not_retried() {
    bounded(async {
        let mut fixture = Fixture::new(64 * 1024).await;
        for mismatch in [true, false] {
            let (local, remote) = fixture.open().await;
            let server = tokio::spawn(async move {
                let mut stream = remote.into_stream();
                let mut open = read_open(&mut stream).await;
                if mismatch {
                    open.worker_id = uuid::Uuid::new_v4().to_string();
                    stream.write_all(&admission(&open)).await.unwrap();
                    stream.flush().await.unwrap();
                }
            });
            let error = Channel::new(local, Some(Transport::Opaque))
                .admit(&opening())
                .await
                .err()
                .unwrap();
            if mismatch {
                assert!(matches!(&error, AdmissionError::Identity(_)));
                assert!(!error.transient());
            } else {
                assert!(matches!(&error, AdmissionError::Closed));
                assert!(error.transient());
            }
            assert_eq!(error.retry(), newport_protocol::common::Retry::Unknown);
            assert!(error.to_string().contains("not retried"));
            assert!(!format!("{error:?}: {error}").contains(&"b".repeat(64)));
            server.await.unwrap();
        }
    })
    .await;
}

#[tokio::test]
async fn admission_refusals_preserve_the_generated_failure_without_more_client_bytes() {
    use newport_protocol::common::{Error, Retry};
    use std::error::Error as _;
    bounded(async {
        let mut fixture = Fixture::new(64 * 1024).await;
        for retry in [Retry::No, Retry::Yes, Retry::Unknown] {
            let failure = Error {
                code: "unavailable".into(),
                message: "Worker process could not start".into(),
                retry,
            };
            let bytes = admission::Admission::Refused(failure.clone())
                .encode()
                .unwrap();
            let (local, remote) = fixture.open().await;
            let server = tokio::spawn(async move {
                let mut stream = remote.into_stream();
                let _ = read_open(&mut stream).await;
                stream.write_all(&bytes).await.unwrap();
                stream.flush().await.unwrap();
                stream.shutdown().await.unwrap();
                let mut extra = Vec::new();
                stream.read_to_end(&mut extra).await.unwrap();
                assert!(extra.is_empty());
            });
            let error = Channel::new(local, Some(Transport::GIT))
                .admit(&opening())
                .await
                .err()
                .unwrap();
            assert!(matches!(&error, AdmissionError::Refused(value) if value == &failure));
            assert_eq!(
                error.source().unwrap().downcast_ref::<Error>(),
                Some(&failure)
            );
            assert_eq!(error.retry(), retry);
            assert!(!format!("{error:?}: {error}").contains(&"b".repeat(64)));
            server.await.unwrap();
            assert!(fixture.channels.try_recv().is_err());
            assert!(!fixture.client.is_closed());
        }
    })
    .await;
}

#[tokio::test]
async fn malformed_admission_frames_keep_their_cause_and_close_only_their_channel() {
    use std::error::Error as _;
    bounded(async {
        let mut fixture = Fixture::new(64 * 1024).await;
        for bytes in [vec![0x80; 10], vec![1, 0]] {
            let (local, remote) = fixture.open().await;
            let server = tokio::spawn(async move {
                let mut stream = remote.into_stream();
                let _ = read_open(&mut stream).await;
                stream.write_all(&bytes).await.unwrap();
                stream.flush().await.unwrap();
                let mut extra = Vec::new();
                stream.read_to_end(&mut extra).await.unwrap();
                assert!(extra.is_empty());
            });
            let error = Channel::new(local, Some(Transport::GIT))
                .admit(&opening())
                .await
                .err()
                .unwrap();
            assert!(matches!(
                &error,
                AdmissionError::Frame(_) | AdmissionError::Decode(_)
            ));
            assert!(error.source().is_some());
            assert!(!error.transient());
            assert_eq!(error.retry(), newport_protocol::common::Retry::Unknown);
            server.await.unwrap();
            assert!(!fixture.client.is_closed());
        }
    })
    .await;
}

#[tokio::test]
async fn blocked_input_drop_closes_only_its_channel() {
    bounded(async {
        let mut fixture = Fixture::new(0).await;
        let (local, mut remote) = fixture.open().await;
        let mut stream = Channel::new(local, Some(Transport::Opaque)).into_stream();
        let input = vec![5; 8 * 1024 * 1024];
        assert!(
            tokio::time::timeout(Duration::from_millis(50), stream.write_all(&input))
                .await
                .is_err()
        );
        drop(stream);
        while let Some(message) = remote.wait().await {
            if matches!(message, ChannelMsg::Close) {
                break;
            }
        }
        assert!(!fixture.client.is_closed());
        let (local, remote) = fixture.open().await;
        let mut stream = Channel::new(local, Some(Transport::Opaque)).into_stream();
        remote.data(&output(b"independent")[..]).await.unwrap();
        remote.data(&exit(Exit::Status(0))[..]).await.unwrap();
        remote.eof().await.unwrap();
        let mut output = Vec::new();
        stream.read_to_end(&mut output).await.unwrap();
        assert_eq!(output, b"independent");
    })
    .await;
}

#[tokio::test]
async fn legacy_channel_retains_stdout_stderr_and_exit_status() {
    bounded(async {
        let mut fixture = Fixture::new(64 * 1024).await;
        let (local, remote) = fixture.open().await;
        let mut stream = Channel::from(local).into_stream();
        assert!(stream.uses_session_slot());
        remote.data(&b"stdout"[..]).await.unwrap();
        remote
            .extended_data(1, &b"failure detail"[..])
            .await
            .unwrap();
        remote.exit_status(9).await.unwrap();
        remote.close().await.unwrap();
        let mut output = Vec::new();
        let error = stream
            .read_to_end(&mut output)
            .await
            .unwrap_err()
            .to_string();
        assert_eq!(output, b"stdout");
        assert!(error.contains("status 9") && error.ends_with("failure detail"));
    })
    .await;
}

#[tokio::test]
async fn cancelled_pending_write_does_not_accept_or_replay_old_bytes() {
    use std::{pin::Pin, task::Poll};
    use tokio::io::AsyncWrite;
    bounded(async {
        let mut fixture = Fixture::new(64 * 1024).await;
        let (local, remote) = fixture.open().await;
        let mut stream = Channel::new(local, Some(Transport::Opaque)).into_stream();
        let mut accepted = 0;
        // Fill the bounded queue without yielding to the relay. The last write
        // is cancelled on Pending; the next poll has a different caller buffer.
        std::future::poll_fn(|cx| {
            loop {
                match Pin::new(&mut stream).poll_write(cx, &[42; MAX_CHUNK]) {
                    Poll::Ready(Ok(length)) => accepted += length,
                    Poll::Ready(Err(error)) => panic!("Unexpected input failure: {error}"),
                    Poll::Pending => return Poll::Ready(()),
                }
            }
        })
        .await;
        assert!(accepted > 0 && accepted <= 8 * MAX_CHUNK);
        let server = tokio::spawn(async move {
            let mut remote = remote.into_stream();
            let mut received = Vec::new();
            loop {
                let Some(bytes) = read_input(&mut remote).await else {
                    break;
                };
                received.extend(bytes);
            }
            assert_eq!(received.len(), accepted + b"replacement".len());
            assert!(received[..accepted].iter().all(|byte| *byte == 42));
            assert_eq!(&received[accepted..], b"replacement");
            remote.write_all(&exit(Exit::Status(0))).await.unwrap();
            remote.flush().await.unwrap();
            remote.shutdown().await.unwrap();
        });
        stream.write_all(b"replacement").await.unwrap();
        stream.shutdown().await.unwrap();
        stream.read_to_end(&mut Vec::new()).await.unwrap();
        server.await.unwrap();
    })
    .await;
}

fn native_request() -> Vec<u8> {
    use newport_protocol::git::{self, wire::Message};
    git::ClientMessage::Call(git::Call {
        id: uuid::Uuid::new_v4().to_string(),
        request: Box::new(git::Request::Start {
            operation_id: uuid::Uuid::new_v4().to_string(),
            repo_id: "repo".into(),
            expected_snapshot: "snapshot".into(),
            action: git::Action::Stage {
                entry_ids: vec!["entry".repeat(20); 512],
                hunks: None,
            },
        }),
    })
    .encode()
    .unwrap()
}

fn native_log() -> Vec<u8> {
    use newport_protocol::git::{self, wire::Message};
    git::ServerMessage::Log(git::Log {
        id: uuid::Uuid::new_v4().to_string(),
        entry: git::CommandLog {
            command: "git status".into(),
            duration_ms: 1,
            exit_code: Some(0),
            // The log output limit is 32 KiB; its envelope still makes the
            // complete native frame exceed one worker transport chunk.
            output: "x".repeat(32 * 1024),
            interrupted: false,
        },
    })
    .encode()
    .unwrap()
}

fn extended_prefix(frame: &[u8]) -> Vec<u8> {
    let header = frame.iter().position(|byte| byte & 0x80 == 0).unwrap();
    let mut result = frame[..=header].to_vec();
    result[header] |= 0x80;
    result.push(0);
    result.extend_from_slice(&frame[header + 1..]);
    result
}

#[tokio::test]
async fn native_service_frames_cross_fragmented_writes_and_reads_unchanged() {
    use newport_protocol::git::{self, wire::Message};
    bounded(async {
        let mut fixture = Fixture::new(64 * 1024).await;
        let (local, remote) = fixture.open().await;
        let input = [
            extended_prefix(&native_request()),
            git::ClientMessage::Ping("done".into()).encode().unwrap(),
        ]
        .concat();
        let large = extended_prefix(&native_log());
        assert!(input.len() > MAX_CHUNK && large.len() > MAX_CHUNK);
        let pong = git::ServerMessage::Pong("done".into()).encode().unwrap();
        let expected = [large.clone(), pong.clone()].concat();
        let sent = input.clone();
        let server = tokio::spawn(async move {
            let mut stream = remote.into_stream();
            let mut received = vec![0; sent.len()];
            stream.read_exact(&mut received).await.unwrap();
            assert_eq!(received, sent);
            assert!(read_input(&mut stream).await.is_none());
            let output = [large, stderr(b"diagnostic"), pong, exit(Exit::Status(0))].concat();
            for piece in output.chunks(997) {
                stream.write_all(piece).await.unwrap();
            }
            stream.flush().await.unwrap();
            stream.shutdown().await.unwrap();
        });
        let channel = Channel::new(local, Some(Transport::GIT));
        let (mut reader, mut writer) = tokio::io::split(channel.into_stream());
        let mut received = Vec::new();
        let ((), _) = tokio::try_join!(
            async {
                for piece in input.chunks(997) {
                    writer.write_all(piece).await?;
                }
                writer.flush().await?;
                writer.shutdown().await
            },
            reader.read_to_end(&mut received)
        )
        .unwrap();
        assert_eq!(received, expected);
        server.await.unwrap();
    })
    .await;
}

#[tokio::test]
async fn native_input_rejects_transport_controls_and_incomplete_eof_without_sending() {
    bounded(async {
        let mut fixture = Fixture::new(64 * 1024).await;
        let control = wire::encode_client(ClientControl::InputComplete).unwrap();
        let request = native_request();
        let mut mixed = newport_protocol::encoding::protobuf::read_frame(
            &mut request.as_slice(),
            newport_protocol::git::MAX_FRAME,
        )
        .unwrap()
        .unwrap()
        .to_vec();
        mixed.extend_from_slice(
            &newport_protocol::encoding::protobuf::read_frame(
                &mut control.as_slice(),
                wire::CONTROL_LIMITS.message,
            )
            .unwrap()
            .unwrap(),
        );
        let mut mixed_frame = Vec::new();
        newport_protocol::encoding::protobuf::write_frame(&mut mixed_frame, &mixed).unwrap();
        for (bytes, complete) in [
            (control, true),
            (mixed_frame, true),
            (vec![0x80], false),
            (vec![2, 0x08], false),
        ] {
            let (local, remote) = fixture.open().await;
            let (reader, writer) = Channel::new(local, Some(Transport::GIT)).split();
            let result = writer.data(&bytes).await;
            assert_eq!(result.is_err(), complete);
            assert!(
                writer
                    .signal(newport_protocol::worker::Signal::new(15).unwrap())
                    .await
                    .is_err()
            );
            assert!(writer.eof().await.is_err());
            assert!(
                writer
                    .signal(newport_protocol::worker::Signal::new(15).unwrap())
                    .await
                    .is_err()
            );
            assert!(writer.data(&request).await.is_err());
            let mut peer = remote.into_stream();
            assert!(
                tokio::time::timeout(Duration::from_millis(20), peer.read_u8())
                    .await
                    .is_err()
            );
            writer.close().await.unwrap();
            drop(reader);
        }
    })
    .await;
}

#[tokio::test]
async fn native_data_limits_do_not_shrink_worker_control_limits() {
    bounded(async {
        let limits = newport_protocol::common::Limits { message: 64, metadata: 64 };
        let transport = Transport::Protobuf { client: limits, server: limits };
        let mut fixture = Fixture::new(64 * 1024).await;
        let (local, remote) = fixture.open().await;
        let (mut reader, writer) = Channel::new(local, Some(transport)).split();
        assert!(writer.data(&[65]).await.unwrap_err().contains("over the limit"));
        assert!(writer.eof().await.is_err());
        remote.data(&stderr(&vec![b'x'; MAX_CHUNK])[..]).await.unwrap();
        assert!(matches!(reader.wait().await.unwrap(), Some(ChannelMsg::ExtendedData { data, ext: 1 }) if data.len() == MAX_CHUNK));
        remote.data(&exit(Exit::Status(0))[..]).await.unwrap();
        remote.eof().await.unwrap();
        assert!(matches!(reader.wait().await.unwrap(), Some(ChannelMsg::ExitStatus { exit_status: 0 })));
        assert!(reader.wait().await.unwrap().is_none());
        writer.close().await.unwrap();

        let (local, remote) = fixture.open().await;
        let mut channel = Channel::new(local, Some(transport));
        remote.data(&output(&[b'x'; 65])[..]).await.unwrap();
        assert!(channel.wait().await.unwrap_err().contains("exceeds its frame limit"));
        assert!(channel.wait().await.unwrap().is_none());
        channel.close().await.unwrap();
    }).await;
}

#[tokio::test]
async fn cancelled_transport_write_cannot_reuse_a_partially_sent_frame() {
    bounded(async {
        let mut fixture = Fixture::new(0).await;
        for transport in [Transport::Opaque, Transport::GIT] {
            let (local, mut remote) = fixture.open().await;
            let (reader, writer) = Channel::new(local, Some(transport)).split();
            let request = native_request();
            assert!(
                tokio::time::timeout(Duration::from_millis(20), writer.data(&request))
                    .await
                    .is_err()
            );
            assert!(
                writer
                    .data(&request)
                    .await
                    .unwrap_err()
                    .contains("interrupted")
            );
            assert!(writer.eof().await.unwrap_err().contains("interrupted"));
            assert!(
                writer
                    .signal(newport_protocol::worker::Signal::new(15).unwrap())
                    .await
                    .is_err()
            );
            writer.close().await.unwrap();
            while let Some(message) = remote.wait().await {
                if matches!(message, ChannelMsg::Close) {
                    break;
                }
            }
            drop(reader);
            assert!(!fixture.client.is_closed());
        }
    })
    .await;
}

#[tokio::test]
async fn flushing_an_incomplete_native_frame_fails_and_retires_the_channel() {
    bounded(async {
        let mut fixture = Fixture::new(64 * 1024).await;
        for partial in [&[0x80][..], &[2, 0x08][..]] {
            let (local, mut remote) = fixture.open().await;
            let mut stream = Channel::new(local, Some(Transport::GIT)).into_stream();
            stream.write_all(partial).await.unwrap();
            assert!(
                stream
                    .flush()
                    .await
                    .unwrap_err()
                    .to_string()
                    .contains("inside a service frame")
            );
            while let Some(message) = remote.wait().await {
                assert!(!matches!(message, ChannelMsg::Data { .. }));
                if matches!(message, ChannelMsg::Close) {
                    break;
                }
            }
            assert!(!fixture.client.is_closed());
            assert!(stream.write_all(&native_request()).await.is_err());
        }
    })
    .await;
}

#[tokio::test]
async fn cancelled_native_reads_retain_original_prefix_and_complete_payload() {
    bounded(async {
        let mut fixture = Fixture::new(128 * 1024).await;
        let (local, remote) = fixture.open().await;
        let mut channel = Channel::new(local, Some(Transport::GIT));
        let bytes = extended_prefix(&native_log());
        remote.data(&bytes[..1]).await.unwrap();
        assert!(
            tokio::time::timeout(Duration::from_millis(20), channel.wait())
                .await
                .is_err()
        );
        remote.data(&bytes[1..MAX_CHUNK]).await.unwrap();
        assert!(
            tokio::time::timeout(Duration::from_millis(20), channel.wait())
                .await
                .is_err()
        );
        remote.data(&bytes[MAX_CHUNK..]).await.unwrap();
        let Some(ChannelMsg::Data { data }) = channel.wait().await.unwrap() else {
            panic!("missing native service frame")
        };
        assert_eq!(&data[..], bytes);
        remote.data(&exit(Exit::Status(0))[..]).await.unwrap();
        remote.eof().await.unwrap();
        assert!(matches!(
            channel.wait().await.unwrap(),
            Some(ChannelMsg::ExitStatus { exit_status: 0 })
        ));
        assert!(channel.wait().await.unwrap().is_none());
        channel.close().await.unwrap();
    })
    .await;
}

#[tokio::test]
async fn native_frames_cannot_mix_service_output_and_worker_exit() {
    bounded(async {
        let mut fixture = Fixture::new(128 * 1024).await;
        let native = native_log();
        let control = exit(Exit::Status(0));
        let mut body = newport_protocol::encoding::protobuf::read_frame(
            &mut native.as_slice(),
            newport_protocol::git::MAX_FRAME,
        )
        .unwrap()
        .unwrap()
        .to_vec();
        body.extend_from_slice(
            &newport_protocol::encoding::protobuf::read_frame(
                &mut control.as_slice(),
                wire::CONTROL_LIMITS.message,
            )
            .unwrap()
            .unwrap(),
        );
        let mut mixed = Vec::new();
        newport_protocol::encoding::protobuf::write_frame(&mut mixed, &body).unwrap();
        let (local, remote) = fixture.open().await;
        let mut stream = Channel::new(local, Some(Transport::GIT)).into_stream();
        remote.data(mixed.as_slice()).await.unwrap();
        let mut received = Vec::new();
        assert!(
            stream
                .read_to_end(&mut received)
                .await
                .unwrap_err()
                .to_string()
                .contains("mixed")
        );
        assert!(received.is_empty());
    })
    .await;
}

#[tokio::test]
async fn typed_worker_exit_is_published_only_after_validated_transport_closure() {
    bounded(async {
        let mut fixture = Fixture::new(64 * 1024).await;
        for trailing in [false, true] {
            let (local, remote) = fixture.open().await;
            let (mut reader, writer) = Channel::new(local, Some(Transport::GIT)).split();
            let outcome = Exit::Signal(newport_protocol::worker::Signal::new(7).unwrap());
            assert_eq!(reader.worker_exit(), None);
            remote.data(&exit(outcome)[..]).await.unwrap();
            assert!(matches!(
                reader.wait().await.unwrap(),
                Some(ChannelMsg::ExitSignal { .. })
            ));
            assert_eq!(
                reader.worker_exit(),
                None,
                "SSH closure is not validated yet"
            );
            if trailing {
                remote.data(&b"late"[..]).await.unwrap();
                assert!(reader.wait().await.is_err());
                assert_eq!(reader.worker_exit(), None);
            } else {
                remote.eof().await.unwrap();
                assert!(reader.wait().await.unwrap().is_none());
                assert_eq!(reader.worker_exit(), Some(outcome));
            }
            writer.close().await.unwrap();
        }
    })
    .await;
}

#[tokio::test]
async fn typed_signal_after_input_complete_reaches_the_same_worker_once() {
    bounded(async {
        let mut fixture = Fixture::new(64 * 1024).await;
        let (local, remote) = fixture.open().await;
        let (_reader, writer) = Channel::new(local, Some(Transport::GIT)).split();
        let mut remote = remote.into_stream();
        writer.eof().await.unwrap();
        assert!(read_input(&mut remote).await.is_none());
        let signal = newport_protocol::worker::Signal::new(15).unwrap();
        writer.signal(signal).await.unwrap();
        let mut length = LengthDecoder::default();
        let length = loop { if let Some(length) = length.push(remote.read_u8().await.unwrap(),wire::CONTROL_LIMITS.message).unwrap() { break length; } };
        let mut body = vec![0;length]; remote.read_exact(&mut body).await.unwrap();
        assert!(matches!(wire::decode_client(body.into(),&wire::CONTROL_LIMITS).unwrap(),wire::Frame::Control(ClientControl::Signal(actual)) if actual == signal));
        assert!(tokio::time::timeout(Duration::from_millis(20),remote.read_u8()).await.is_err());
        writer.close().await.unwrap(); assert!(writer.signal(signal).await.is_err());
    }).await;
}
