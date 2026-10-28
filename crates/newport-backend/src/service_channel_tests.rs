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
fn opening() -> wire::Open {
    wire::Open {
        version: wire::VERSION,
        runtime_id: uuid::Uuid::new_v4().to_string(),
        worker_id: uuid::Uuid::new_v4().to_string(),
        build_sha256: "a".repeat(64),
        ticket: "b".repeat(64),
        service: "monitor".into(),
        arguments: vec!["literal ' argument".into()],
    }
}
fn admission(open: &wire::Open) -> Vec<u8> {
    let reply = wire::Admission::Started {
        runtime_id: open.runtime_id.clone(),
        worker_id: open.worker_id.clone(),
        build_sha256: open.build_sha256.clone(),
    };
    let bytes = serde_json::to_vec(&reply).unwrap();
    let mut result = (bytes.len() as u32).to_be_bytes().to_vec();
    result.extend(bytes);
    result
}
fn frame(kind: wire::Kind, data: &[u8]) -> Vec<u8> {
    let mut bytes = Vec::new();
    wire::Frame {
        kind,
        data: data.to_vec(),
    }
    .write(&mut bytes, wire::Direction::FromWorker)
    .unwrap();
    bytes
}
async fn read_open(stream: &mut russh::ChannelStream<server::Msg>) -> wire::Open {
    let length = stream.read_u32().await.unwrap() as usize;
    assert!(length <= wire::MAX_OPEN_BYTES);
    let mut bytes = vec![0; length];
    stream.read_exact(&mut bytes).await.unwrap();
    wire::Open::decode(&bytes).unwrap()
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
            reply.extend(frame(wire::Kind::Output, b"ready"));
            stream.write_all(&reply).await.unwrap();
            let mut length = 0;
            loop {
                let mut header = [0; 5];
                stream.read_exact(&mut header).await.unwrap();
                let header = wire::Header::decode(header, wire::Direction::ToWorker).unwrap();
                if header.kind == wire::Kind::InputEof {
                    break;
                }
                let mut data = vec![0; header.length];
                stream.read_exact(&mut data).await.unwrap();
                assert!(data.iter().all(|b| *b == 42));
                length += data.len();
                stream
                    .write_all(&frame(wire::Kind::Output, &data))
                    .await
                    .unwrap();
            }
            // A raw SSH EOF here would cancel the runtime's worker rather than
            // merely close stdin. The semantic EOF frame must leave IO open.
            assert!(
                tokio::time::timeout(Duration::from_millis(20), stream.read_u8())
                    .await
                    .is_err()
            );
            stream
                .write_all(&frame(wire::Kind::Output, b"tail"))
                .await
                .unwrap();
            stream
                .write_all(&frame(wire::Kind::ExitStatus, &0u32.to_be_bytes()))
                .await
                .unwrap();
            stream.flush().await.unwrap();
            length
        });
        let channel = Channel::forwarded(local, &opening()).await.unwrap();
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
        let mut channel = Channel::new(local, true);
        let bytes = frame(wire::Kind::Output, b"complete payload");
        remote.data(&bytes[..2]).await.unwrap();
        assert!(
            tokio::time::timeout(Duration::from_millis(20), channel.wait())
                .await
                .is_err()
        );
        remote.data(&bytes[2..7]).await.unwrap();
        assert!(
            tokio::time::timeout(Duration::from_millis(20), channel.wait())
                .await
                .is_err()
        );
        remote.data(&bytes[7..]).await.unwrap();
        let Some(ChannelMsg::Data { data }) = channel.wait().await.unwrap() else {
            panic!("Missing payload")
        };
        assert_eq!(&data[..], b"complete payload");
        remote
            .data(&frame(wire::Kind::ExitStatus, &3u32.to_be_bytes())[..])
            .await
            .unwrap();
        assert!(matches!(
            channel.wait().await.unwrap(),
            Some(ChannelMsg::ExitStatus { exit_status: 3 })
        ));
        assert!(channel.wait().await.unwrap().is_none());
    })
    .await;
}

#[tokio::test]
async fn malformed_or_truncated_output_never_becomes_clean_eof() {
    bounded(async {
        let mut fixture = Fixture::new(64 * 1024).await;
        for bytes in [
            vec![3, 0],
            vec![3, 0, 0, 0, 3, b'x'],
            vec![3, 255, 255, 255, 255],
            vec![1, 0, 0, 0, 1, 9],
            vec![6, 0, 0, 0, 4, 0, 0, 0, 0],
            [frame(wire::Kind::ExitStatus, &0u32.to_be_bytes()), vec![9]].concat(),
        ] {
            let (local, remote) = fixture.open().await;
            let mut stream = Channel::new(local, true).into_stream();
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
        for kind in [wire::Kind::ExitStatus, wire::Kind::ExitSignal] {
            let (local, remote) = fixture.open().await;
            let mut stream = Channel::new(local, true).into_stream();
            let sender = tokio::spawn(async move {
                remote
                    .data(&frame(wire::Kind::Output, b"stdout")[..])
                    .await
                    .unwrap();
                for _ in 0..3 {
                    remote
                        .data(&frame(wire::Kind::Error, &vec![b'x'; wire::MAX_CHUNK])[..])
                        .await
                        .unwrap();
                }
                remote
                    .data(&frame(wire::Kind::Error, b"diagnostic tail")[..])
                    .await
                    .unwrap();
                remote
                    .data(&frame(kind, &7u32.to_be_bytes())[..])
                    .await
                    .unwrap();
            });
            let mut bytes = Vec::new();
            let error = stream
                .read_to_end(&mut bytes)
                .await
                .unwrap_err()
                .to_string();
            assert_eq!(bytes, b"stdout");
            assert!(error.ends_with("diagnostic tail") && error.len() <= 65536 + 64);
            assert!(error.contains(if kind == wire::Kind::ExitStatus {
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
            let error = Channel::forwarded(local, &opening()).await.err().unwrap();
            assert!(error.contains("not retried"));
            assert!(!error.contains(&"b".repeat(64)));
            server.await.unwrap();
        }
    })
    .await;
}

#[tokio::test]
async fn blocked_input_drop_closes_only_its_channel() {
    bounded(async {
        let mut fixture = Fixture::new(0).await;
        let (local, mut remote) = fixture.open().await;
        let mut stream = Channel::new(local, true).into_stream();
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
        let mut stream = Channel::new(local, true).into_stream();
        remote
            .data(&frame(wire::Kind::Output, b"independent")[..])
            .await
            .unwrap();
        remote
            .data(&frame(wire::Kind::ExitStatus, &0u32.to_be_bytes())[..])
            .await
            .unwrap();
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
        let mut stream = Channel::new(local, true).into_stream();
        let mut accepted = 0;
        // Fill the bounded queue without yielding to the relay. The last write
        // is cancelled on Pending; the next poll has a different caller buffer.
        std::future::poll_fn(|cx| {
            loop {
                match Pin::new(&mut stream).poll_write(cx, &[42; wire::MAX_CHUNK]) {
                    Poll::Ready(Ok(length)) => accepted += length,
                    Poll::Ready(Err(error)) => panic!("Unexpected input failure: {error}"),
                    Poll::Pending => return Poll::Ready(()),
                }
            }
        })
        .await;
        assert!(accepted > 0 && accepted <= 8 * wire::MAX_CHUNK);
        let server = tokio::spawn(async move {
            let mut remote = remote.into_stream();
            let mut received = Vec::new();
            loop {
                let mut header = [0; 5];
                remote.read_exact(&mut header).await.unwrap();
                let header = wire::Header::decode(header, wire::Direction::ToWorker).unwrap();
                if header.kind == wire::Kind::InputEof {
                    break;
                }
                let mut bytes = vec![0; header.length];
                remote.read_exact(&mut bytes).await.unwrap();
                received.extend(bytes);
            }
            assert_eq!(received.len(), accepted + b"replacement".len());
            assert!(received[..accepted].iter().all(|byte| *byte == 42));
            assert_eq!(&received[accepted..], b"replacement");
            remote
                .write_all(&frame(wire::Kind::ExitStatus, &0u32.to_be_bytes()))
                .await
                .unwrap();
            remote.flush().await.unwrap();
        });
        stream.write_all(b"replacement").await.unwrap();
        stream.shutdown().await.unwrap();
        stream.read_to_end(&mut Vec::new()).await.unwrap();
        server.await.unwrap();
    })
    .await;
}
