//! Per-channel flow control for channels the server opens: a forwarded
//! channel nobody reads holds at most one window and pauses only itself.
use russh::{
    Channel, ChannelMsg, client, keys,
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
    sync::mpsc,
};

/// The server's key, from a fixed seed.
fn host_key() -> keys::PrivateKey {
    keys::ssh_key::private::Ed25519Keypair::from_seed(&[1; 32]).into()
}

/// Echoes sessions, and on `tcpip-forward` opens one forwarded channel and
/// writes into it as fast as the client's window lets it.
struct Flooding {
    /// Bytes the client's window has admitted on the forwarded channel.
    sent: Arc<AtomicUsize>,
}

impl server::Handler for Flooding {
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
        tokio::spawn(async move {
            let mut stream = channel.into_stream();
            let mut bytes = [0; 65536];
            while let Ok(n) = stream.read(&mut bytes).await {
                if n == 0 || stream.write_all(&bytes[..n]).await.is_err() {
                    break;
                }
            }
        });
        Ok(())
    }

    async fn tcpip_forward(
        &mut self,
        address: &str,
        port: &mut u32,
        session: &mut server::Session,
    ) -> Result<bool, Self::Error> {
        if *port == 0 {
            *port = 4242;
        }
        let (handle, address, port, sent) = (
            session.handle(),
            address.to_owned(),
            *port,
            self.sent.clone(),
        );
        // The open waits for the client's confirmation, which this session's
        // loop reads, so it can't be awaited here.
        tokio::spawn(async move {
            let channel = handle
                .channel_open_forwarded_tcpip(address, port, "127.0.0.1", 50000)
                .await
                .expect("forwarded channel");
            let mut writer = channel.make_writer();
            let chunk = vec![b'f'; 32768];
            while let Ok(n) = writer.write(&chunk).await {
                if n == 0 {
                    break;
                }
                sent.fetch_add(n, Ordering::SeqCst);
            }
        });
        Ok(true)
    }
}

/// Hands each forwarded channel to the test.
struct Forwarded(mpsc::UnboundedSender<Channel<client::Msg>>);

impl client::Handler for Forwarded {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        _: &keys::PublicKeyOrCertificate,
    ) -> Result<bool, Self::Error> {
        Ok(true)
    }

    async fn server_channel_open_forwarded_tcpip(
        &mut self,
        channel: Channel<client::Msg>,
        _: &str,
        _: u32,
        _: &str,
        _: u32,
        reply: client::ChannelOpenHandle,
        _: &mut client::Session,
    ) -> Result<(), Self::Error> {
        reply.accept().await;
        let _ = self.0.send(channel);
        Ok(())
    }
}

async fn flooding_server(sent: Arc<AtomicUsize>) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let config = Arc::new(server::Config {
        keys: vec![host_key()],
        auth_rejection_time: Duration::from_millis(1),
        ..Default::default()
    });
    tokio::spawn(async move {
        let (socket, _) = listener.accept().await.unwrap();
        if let Ok(session) = server::run_stream(config, socket, Flooding { sent }).await {
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

/// The next data on `channel`, or a failure after 3 s.
async fn next_data(channel: &mut Channel<client::Msg>) -> usize {
    loop {
        match tokio::time::timeout(Duration::from_secs(3), channel.wait()).await {
            Ok(Some(ChannelMsg::Data { data })) => return data.len(),
            Ok(Some(_)) => {}
            other => panic!("no data: {other:?}"),
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unread_forwarded_channel_pauses_only_itself() {
    let sent = Arc::new(AtomicUsize::new(0));
    let port = flooding_server(sent.clone()).await;
    let config = Arc::new(client::Config::default());
    let window = config.window_size as usize;
    let (forwarded_tx, mut forwarded_rx) = mpsc::unbounded_channel();
    let mut handle = client::connect(config, ("127.0.0.1", port), Forwarded(forwarded_tx))
        .await
        .unwrap();
    assert!(handle.authenticate_none("flow").await.unwrap().success());
    handle.tcpip_forward("127.0.0.1", 0).await.unwrap();
    let mut forwarded = tokio::time::timeout(Duration::from_secs(5), forwarded_rx.recv())
        .await
        .expect("forwarded channel opened")
        .unwrap();

    // Nobody reads the forwarded channel: the server fills one window and
    // stops there, since the window reopens only as the reader takes data.
    eventually("the server to fill the window", || {
        sent.load(Ordering::SeqCst) >= window
    })
    .await;

    // A session on the same connection still echoes.
    let mut session = handle.channel_open_session().await.unwrap();
    for _ in 0..50 {
        let message = [b'e'; 1024];
        session.data(&message[..]).await.unwrap();
        let mut echoed = 0;
        while echoed < message.len() {
            echoed += next_data(&mut session).await;
        }
    }

    // The unread queue never held more than one window.
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(sent.load(Ordering::SeqCst), window);

    // Reading reopens the window, so the server keeps sending.
    let mut read = 0;
    while read < 3 * window {
        read += next_data(&mut forwarded).await;
        assert!(sent.load(Ordering::SeqCst) <= read + window);
    }
}
