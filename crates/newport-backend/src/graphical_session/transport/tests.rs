use super::*;
use russh::{Channel, client, server};

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
struct Server(tokio::sync::mpsc::UnboundedSender<Channel<server::Msg>>);
impl server::Handler for Server {
    type Error = anyhow::Error;
    async fn auth_none(&mut self, _: &str) -> anyhow::Result<server::Auth> {
        Ok(server::Auth::Accept)
    }
    async fn channel_open_session(
        &mut self,
        channel: Channel<server::Msg>,
        reply: server::ChannelOpenHandle,
        _: &mut server::Session,
    ) -> anyhow::Result<()> {
        reply.accept().await;
        self.0
            .send(channel)
            .map_err(|_| anyhow::anyhow!("Fixture closed"))?;
        Ok(())
    }
}
struct Fixture {
    client: client::Handle<Client>,
    channels: tokio::sync::mpsc::UnboundedReceiver<Channel<server::Msg>>,
    server: tokio::task::JoinHandle<()>,
}
impl Fixture {
    async fn new(window_size: u32) -> Self {
        let key = russh::keys::PrivateKey::from(
            russh::keys::ssh_key::private::Ed25519Keypair::from_seed(&[19; 32]),
        );
        let config = server::Config {
            keys: vec![key],
            window_size,
            auth_rejection_time: Duration::ZERO,
            ..Default::default()
        };
        // Both SSH endpoints may flush a complete channel window before reading.
        // A 64 KiB in-memory duplex can fill in both directions with the
        // initial window plus framing and deadlock the two SSH event loops.
        // Exercise actual loopback TCP buffering; keep the channel windows and
        // deadlines unchanged so flow control and cancellation are still tested.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let (local, accepted) =
            tokio::join!(tokio::net::TcpStream::connect(address), listener.accept());
        let local = local.unwrap();
        let (remote, _) = accepted.unwrap();
        local.set_nodelay(true).unwrap();
        remote.set_nodelay(true).unwrap();
        drop(listener);
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
    async fn open(&mut self) -> (super::Channel, Channel<server::Msg>) {
        let local = self.client.channel_open_session().await.unwrap();
        let remote = self.channels.recv().await.unwrap();
        (super::Channel::attach(local), remote)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.server.abort();
    }
}
async fn bounded<F: std::future::Future>(future: F) -> F::Output {
    tokio::time::timeout(Duration::from_secs(8), future)
        .await
        .expect("Fixture stalled")
}

#[tokio::test]
async fn duplex_stream_half_close_and_final_diagnostics() {
    bounded(async {
        let mut fixture = Fixture::new(64 * 1024).await;
        let (channel, mut remote) = fixture.open().await;
        let remote_task = tokio::spawn(async move {
            remote
                .extended_data(1, &[b'a'; STDERR_LIMIT][..])
                .await
                .unwrap();
            remote
                .extended_data(1, &b"diagnostic tail"[..])
                .await
                .unwrap();
            let mut length = 0;
            while let Some(message) = remote.wait().await {
                match message {
                    ChannelMsg::Data { data } => {
                        assert!(data.iter().all(|byte| *byte == 42));
                        length += data.len();
                        remote.data(&data[..]).await.unwrap();
                    }
                    ChannelMsg::Eof => break,
                    _ => {}
                }
            }
            remote.data(&b"tail"[..]).await.unwrap();
            remote.exit_status(7).await.unwrap();
            remote.close().await.unwrap();
            length
        });
        let (mut reader, mut writer, control) = channel.into_parts();
        let write = tokio::task::spawn_blocking(move || {
            for _ in 0..512 {
                writer.write_all(&[42; CHUNK]).unwrap();
            }
            writer.flush().unwrap();
            // Dropping only stdin preserves the final stdout and exit status.
        });
        let read = tokio::task::spawn_blocking(move || {
            let mut bytes = Vec::new();
            reader.read_to_end(&mut bytes).unwrap();
            let report = control.finish().unwrap();
            (bytes, report)
        });
        write.await.unwrap();
        let (bytes, report) = read.await.unwrap();
        assert_eq!(remote_task.await.unwrap(), 512 * CHUNK);
        assert_eq!(bytes.len(), 512 * CHUNK + 4);
        assert!(bytes[..512 * CHUNK].iter().all(|byte| *byte == 42));
        assert_eq!(&bytes[512 * CHUNK..], b"tail");
        assert_eq!(report.status, Some(7));
        assert!(report.failure().unwrap().contains("status 7"));
        assert_eq!(report.stderr.len(), STDERR_LIMIT);
        assert!(report.stderr.ends_with(b"diagnostic tail"));
    })
    .await;
}

#[tokio::test]
async fn cancel_unblocks_a_read_and_flush_when_remote_input_window_is_zero() {
    bounded(async {
        let mut fixture = Fixture::new(0).await;
        let (channel, _remote) = fixture.open().await;
        let (mut reader, mut writer, control) = channel.into_parts();
        let (unblocked, done) = std::sync::mpsc::channel();
        let read_done = unblocked.clone();
        let read = tokio::task::spawn_blocking(move || {
            let result = reader.read(&mut [0; 1]);
            read_done.send(()).unwrap();
            result
        });
        let (sent, submitted) = async_channel::bounded(1);
        let write = tokio::task::spawn_blocking(move || {
            writer.write_all(b"blocked behind remote window").unwrap();
            sent.send_blocking(()).unwrap();
            let result = writer.flush();
            unblocked.send(()).unwrap();
            result
        });
        submitted.recv().await.unwrap();
        // Give the forwarding future a chance to take the first data chunk.
        tokio::task::yield_now().await;
        control.cancel();
        // Intentionally don't advance the single-thread executor: cancellation
        // itself must wake blocked IO, without waiting for the relay to run.
        done.recv_timeout(Duration::from_secs(1)).unwrap();
        done.recv_timeout(Duration::from_secs(1)).unwrap();
        assert!(read.await.unwrap().is_err());
        assert!(write.await.unwrap().is_err());
        let report = tokio::task::spawn_blocking(move || control.finish())
            .await
            .unwrap()
            .unwrap();
        assert!(report.error.is_none(), "{report:?}");
    })
    .await;
}

#[tokio::test]
async fn cancel_unblocks_a_writer_with_a_full_bounded_queue() {
    bounded(async {
        let mut fixture = Fixture::new(0).await;
        let (channel, _remote) = fixture.open().await;
        let (_reader, mut writer, control) = channel.into_parts();
        let write = tokio::task::spawn_blocking(move || {
            for _ in 0..INPUT_CHUNKS + 3 {
                writer.write_all(&[1; CHUNK])?;
            }
            Ok::<_, io::Error>(())
        });
        while !control.0.input.is_full() {
            tokio::task::yield_now().await;
        }
        control.cancel();
        assert!(write.await.unwrap().is_err());
        tokio::task::spawn_blocking(move || control.finish())
            .await
            .unwrap()
            .unwrap();
    })
    .await;
}

#[tokio::test]
async fn a_stalled_decoder_is_bounded_and_other_ssh_channels_survive() {
    bounded(async {
        let mut fixture = Fixture::new(64 * 1024).await;
        let (channel, flood) = fixture.open().await;
        let (_reader, _writer, control) = channel.into_parts();
        let (other, mut peer) = fixture.open().await;
        let flood_task = tokio::spawn(async move {
            let (mut reader, writer) = flood.split();
            let write = async {
                for _ in 0..2048 {
                    if writer.data(&[42; CHUNK][..]).await.is_err() {
                        break;
                    }
                }
            };
            let closed = async {
                while let Some(message) = reader.wait().await {
                    if matches!(message, ChannelMsg::Close) {
                        break;
                    }
                }
            };
            tokio::select! { _ = write => {}, _ = closed => {} }
        });
        while !control.0.output.is_full() {
            tokio::task::yield_now().await;
        }
        let report = tokio::task::spawn_blocking(move || control.finish())
            .await
            .unwrap()
            .unwrap();
        assert!(report.error.unwrap().contains("consumer stalled"));
        let echo = tokio::spawn(async move {
            while let Some(message) = peer.wait().await {
                if let ChannelMsg::Data { data } = message {
                    peer.data(&data[..]).await.unwrap();
                    peer.exit_status(0).await.unwrap();
                    peer.close().await.unwrap();
                    break;
                }
            }
        });
        tokio::task::spawn_blocking(move || {
            let (mut reader, mut writer, control) = other.into_parts();
            writer.write_all(b"still connected").unwrap();
            writer.flush().unwrap();
            let mut bytes = Vec::new();
            reader.read_to_end(&mut bytes).unwrap();
            assert_eq!(bytes, b"still connected");
            assert!(control.finish().unwrap().failure().is_none());
        })
        .await
        .unwrap();
        echo.await.unwrap();
        flood_task.await.unwrap();
        assert!(!fixture.client.is_closed());
    })
    .await;
}

#[tokio::test]
async fn dropping_an_unclaimed_channel_closes_only_its_remote_worker() {
    bounded(async {
        let mut fixture = Fixture::new(64 * 1024).await;
        let (channel, mut remote) = fixture.open().await;
        drop(channel); // Must not block this single-thread executor.
        while let Some(message) = remote.wait().await {
            if matches!(message, ChannelMsg::Close) {
                break;
            }
        }
        let (_next, _peer) = fixture.open().await;
        assert!(!fixture.client.is_closed());
    })
    .await;
}

#[tokio::test]
async fn lost_transport_is_not_reported_as_success() {
    bounded(async {
        let mut fixture = Fixture::new(64 * 1024).await;
        let (channel, _remote) = fixture.open().await;
        let (mut reader, _writer, control) = channel.into_parts();
        fixture
            .client
            .disconnect(
                russh::Disconnect::ByApplication,
                "Fixture transport loss",
                "en",
            )
            .await
            .unwrap();
        let report = tokio::task::spawn_blocking(move || {
            assert_eq!(reader.read(&mut [0; 1]).unwrap(), 0);
            control.finish().unwrap()
        })
        .await
        .unwrap();
        assert!(report.failure().unwrap().contains("without an exit status"));
    })
    .await;
}

#[test]
fn runtime_shutdown_before_the_relay_is_polled_releases_blocking_halves() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let (channel, _fixture, _remote) = runtime.block_on(async {
        let mut fixture = Fixture::new(64 * 1024).await;
        let (channel, remote) = fixture.open().await;
        // No yield after Channel::attach, so its spawned future is unpolled.
        (channel, fixture, remote)
    });
    let (mut reader, mut writer, control) = channel.into_parts();
    drop(runtime);
    assert_eq!(reader.read(&mut [0; 1]).unwrap(), 0);
    assert!(writer.write_all(b"no runtime").is_err());
    let report = control.finish().unwrap();
    assert!(report.failure().unwrap().contains("before cleanup"));
    assert_eq!(control.finish().unwrap(), report);
}
