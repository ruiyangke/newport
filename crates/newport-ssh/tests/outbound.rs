//! Outbound flow on one connection: a channel whose data the server's window
//! no longer admits holds back only its own output.
use russh::{
    Channel, ChannelId, ChannelMsg, client, keys,
    server::{self, Auth},
};
use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};

/// The server's key, from a fixed seed.
fn host_key() -> keys::PrivateKey {
    keys::ssh_key::private::Ed25519Keypair::from_seed(&[1; 32]).into()
}

/// The server's window on each channel.
const WINDOW: u32 = 256 * 1024;

/// What the client writes to the stalled channel: well past its window.
const STALLED_WRITE: usize = 4 * WINDOW as usize;

/// The first session channel takes data but stops reopening its window;
/// every later one echoes.
struct Stalling {
    opened: Arc<AtomicUsize>,
    stalled_received: Arc<AtomicUsize>,
}

impl server::Handler for Stalling {
    type Error = russh::Error;

    async fn auth_none(&mut self, _: &str) -> Result<Auth, Self::Error> {
        Ok(Auth::Accept)
    }

    async fn channel_open_session(
        &mut self,
        channel: Channel<server::Msg>,
        reply: server::ChannelOpenHandle,
        _: &mut server::Session,
    ) -> Result<(), Self::Error> {
        reply.accept().await;
        if self.opened.fetch_add(1, Ordering::SeqCst) == 0 {
            let received = self.stalled_received.clone();
            tokio::spawn(async move {
                let mut stream = channel.into_stream();
                let mut bytes = [0; 65536];
                while let Ok(n) = stream.read(&mut bytes).await {
                    if n == 0 {
                        break;
                    }
                    received.fetch_add(n, Ordering::SeqCst);
                }
            });
        } else {
            tokio::spawn(async move {
                let mut stream = channel.into_stream();
                let mut bytes = [0; 65536];
                while let Ok(n) = stream.read(&mut bytes).await {
                    if n == 0 || stream.write_all(&bytes[..n]).await.is_err() {
                        break;
                    }
                }
            });
        }
        Ok(())
    }

    /// russh reopens a window as data arrives; the next target is the only
    /// say a handler has. A target of one byte never reopens it again.
    fn adjust_window(&mut self, _: ChannelId, _: u32) -> u32 {
        1
    }
}

struct Client;

impl client::Handler for Client {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        _: &keys::PublicKeyOrCertificate,
    ) -> Result<bool, Self::Error> {
        Ok(true)
    }
}

async fn stalling_server(stalled_received: Arc<AtomicUsize>) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let config = Arc::new(server::Config {
        keys: vec![host_key()],
        auth_rejection_time: Duration::from_millis(1),
        window_size: WINDOW,
        ..Default::default()
    });
    tokio::spawn(async move {
        let (socket, _) = listener.accept().await.unwrap();
        let handler = Stalling {
            opened: Arc::default(),
            stalled_received,
        };
        if let Ok(session) = server::run_stream(config, socket, handler).await {
            let _ = session.await;
        }
    });
    port
}

/// Wait until `condition` holds, or fail after 5 s.
async fn eventually(what: &str, mut condition: impl FnMut() -> bool) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while !condition() {
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting for {what}"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

/// Write `message` to `channel` and read its echo, failing after 3 s.
async fn echo(channel: &mut Channel<client::Msg>, message: &[u8]) {
    tokio::time::timeout(Duration::from_secs(3), async {
        channel.data(message).await.unwrap();
        let mut echoed = 0;
        while echoed < message.len() {
            match channel.wait().await {
                Some(ChannelMsg::Data { data }) => echoed += data.len(),
                Some(_) => {}
                None => panic!("channel ended"),
            }
        }
    })
    .await
    .expect("the echo");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stalled_channel_holds_back_only_itself() {
    let stalled_received = Arc::new(AtomicUsize::new(0));
    let port = stalling_server(stalled_received.clone()).await;
    let mut handle = client::connect(
        Arc::new(client::Config::default()),
        ("127.0.0.1", port),
        Client,
    )
    .await
    .unwrap();
    assert!(
        handle
            .authenticate_none("outbound")
            .await
            .unwrap()
            .success()
    );
    let handle = Arc::new(handle);
    let stalled = handle.channel_open_session().await.unwrap();
    let mut echoing = handle.channel_open_session().await.unwrap();

    // `Handle::data` doesn't wait for the window: what the server's window
    // doesn't admit stays queued in the client session.
    let writer = {
        let (handle, id) = (handle.clone(), stalled.id());
        tokio::spawn(async move {
            for _ in 0..STALLED_WRITE / 65536 {
                handle.data(id, vec![b's'; 65536]).await.unwrap();
            }
        })
    };
    eventually("the stalled channel's window to close", || {
        stalled_received.load(Ordering::SeqCst) >= WINDOW as usize
    })
    .await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    let received = stalled_received.load(Ordering::SeqCst);
    assert!(received < STALLED_WRITE, "the window never closed");

    // Another channel on the same connection keeps echoing.
    for _ in 0..50 {
        echo(&mut echoing, &[b'e'; 1024]).await;
    }

    // New channels still open, and work.
    let mut opened = tokio::time::timeout(Duration::from_secs(3), handle.channel_open_session())
        .await
        .expect("a channel open")
        .unwrap();
    echo(&mut opened, b"new").await;

    // The stalled channel's queue held its own writes without blocking them.
    tokio::time::timeout(Duration::from_secs(3), writer)
        .await
        .expect("the stalled writes queued")
        .unwrap();
    assert_eq!(stalled_received.load(Ordering::SeqCst), received);
    drop(stalled);
}
