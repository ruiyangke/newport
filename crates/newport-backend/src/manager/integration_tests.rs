//! Tunnels against the SSH fixture, through a proxy that counts logins and
//! cuts connections.
use super::*;
use std::sync::atomic::AtomicUsize;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
};

/// Every SSH connection to the fixture passes through here: each one is a
/// login. While cut, connections are dropped as they arrive.
struct Proxy {
    port: u16,
    logins: Arc<AtomicUsize>,
    cut: watch::Sender<bool>,
    task: JoinHandle<()>,
}
impl Proxy {
    async fn start() -> Self {
        let target: u16 = std::env::var("NEWPORT_TEST_SSH_PORT")
            .unwrap()
            .parse()
            .unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let logins = Arc::new(AtomicUsize::new(0));
        let (cut, _) = watch::channel(false);
        let task = tokio::spawn({
            let logins = logins.clone();
            let cut = cut.clone();
            async move {
                while let Ok((mut socket, _)) = listener.accept().await {
                    let mut cutting = cut.subscribe();
                    if *cutting.borrow_and_update() {
                        continue;
                    }
                    logins.fetch_add(1, Ordering::SeqCst);
                    tokio::spawn(async move {
                        let Ok(mut upstream) = TcpStream::connect(("127.0.0.1", target)).await
                        else {
                            return;
                        };
                        tokio::select! {
                            _ = tokio::io::copy_bidirectional(&mut socket, &mut upstream) => {}
                            _ = cutting.wait_for(|cut| *cut) => {}
                        }
                    });
                }
            }
        });
        Self {
            port,
            logins,
            cut,
            task,
        }
    }
    fn logins(&self) -> usize {
        self.logins.load(Ordering::SeqCst)
    }
    /// Closes every connection and refuses new ones until restored.
    fn cut(&self) {
        self.cut.send_replace(true);
    }
    fn restore(&self) {
        self.cut.send_replace(false);
    }
}
impl Drop for Proxy {
    fn drop(&mut self) {
        self.cut();
        self.task.abort();
    }
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}
fn server(ssh_port: u16) -> Server {
    Server {
        id: Uuid::new_v4(),
        name: "Fixture".into(),
        ssh_user: "fixture".into(),
        ssh_host: "127.0.0.1".into(),
        ssh_port,
        identity_file: Some(std::env::var("NEWPORT_TEST_IDENTITY").unwrap()),
        agent_source: None,
        agent_key_fingerprint: None,
        auth_method: AuthMethod::PublicKey,
        clipboard_enabled: false,
        browser_enabled: false,
        agent_install_approved: true,
        setup_complete: true,
        proxy: None,
    }
}
fn fixture_port() -> u16 {
    std::env::var("NEWPORT_TEST_SSH_PORT")
        .unwrap()
        .parse()
        .unwrap()
}
/// A local forward to a port the fixture echoes.
fn local(server: &Server, port: u16) -> Tunnel {
    Tunnel {
        id: Uuid::new_v4(),
        name: format!("Echo {port}"),
        server_id: server.id,
        local_port: port,
        local_port_end: None,
        remote_host: "127.0.0.1".into(),
        remote_port: 12345,
        remote_port_end: None,
        auto_connect: false,
        auto_reconnect: true,
        direction: Direction::Local,
        project: None,
    }
}
/// A remote forward of a free server port to `target` on this Mac.
fn remote(server: &Server, target: u16) -> Tunnel {
    Tunnel {
        name: "Reverse".into(),
        local_port: target,
        remote_port: free_port(),
        direction: Direction::Remote,
        ..local(server, target)
    }
}
async fn manager(server: &Server, tunnels: &[Tunnel]) -> (tempfile::TempDir, Manager) {
    let directory = tempfile::tempdir().unwrap();
    let mut manager = Manager::new(Store::for_test(directory.path().into()));
    manager
        .save(Config {
            servers: vec![server.clone()],
            tunnels: tunnels.to_vec(),
        })
        .await
        .unwrap();
    (directory, manager)
}
fn state(m: &Manager, id: Uuid) -> ConnectionState {
    m.runtime
        .lock()
        .unwrap()
        .tunnels
        .get(&id)
        .cloned()
        .unwrap_or_default()
}
async fn wait_status(m: &Manager, id: Uuid, status: Status) -> ConnectionState {
    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            let current = state(m, id);
            if current.status == status {
                return current;
            }
            if current.status == Status::Error && status != Status::Error {
                panic!("Unexpected tunnel error: {:?}", current.error_message);
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("Timed out waiting for {status:?}"))
}
async fn wait_health(m: &Manager, id: Uuid, status: DestinationStatus) {
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            let health = m.runtime.lock().unwrap().tunnel_health.get(&id).cloned();
            if health.is_some_and(|health| health.iter().all(|h| h.status == status)) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("Timed out waiting for destinations to be {status:?}"));
}
async fn roundtrip(port: u16, payload: &[u8]) {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    stream.write_all(payload).await.unwrap();
    let mut received = vec![0; payload.len()];
    tokio::time::timeout(Duration::from_secs(5), stream.read_exact(&mut received))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(received, payload);
}
/// Echoes every connection to the returned port on this Mac.
async fn echo() -> (u16, JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let task = tokio::spawn(async move {
        while let Ok((mut socket, _)) = listener.accept().await {
            tokio::spawn(async move {
                let (mut read, mut write) = socket.split();
                let _ = tokio::io::copy(&mut read, &mut write).await;
            });
        }
    });
    (port, task)
}
fn connections(m: &Manager, server: &Server) -> Option<usize> {
    let owner = m.sessions.get(server.id).unwrap();
    match owner.link().status().borrow().reach {
        newport_ssh::Reach::Ready { connections } => Some(connections),
        _ => None,
    }
}

#[tokio::test]
#[ignore = "Run with cargo xtask test ssh and its ephemeral SSH fixture"]
async fn a_local_forward_relays_and_checks_its_destination() {
    let server = server(fixture_port());
    let tunnel = local(&server, free_port());
    let (_directory, mut m) = manager(&server, std::slice::from_ref(&tunnel)).await;
    m.connect(tunnel.id).unwrap();
    wait_status(&m, tunnel.id, Status::Connected).await;
    roundtrip(tunnel.local_port, b"newport roundtrip").await;
    wait_health(&m, tunnel.id, DestinationStatus::Reachable).await;
    // A second tunnel can't claim a live local port.
    let mut overlap = tunnel.clone();
    overlap.id = Uuid::new_v4();
    m.config.tunnels.push(overlap.clone());
    assert!(m.connect(overlap.id).unwrap_err().contains("overlap"));
    m.disconnect(tunnel.id).await;
    assert_eq!(state(&m, tunnel.id).status, Status::Disconnected);
    std::net::TcpListener::bind(("127.0.0.1", tunnel.local_port))
        .expect("stopping releases the local port");
    m.shutdown().await;
}

#[tokio::test]
#[ignore = "Run with cargo xtask test ssh and its ephemeral SSH fixture"]
async fn tunnels_to_one_server_share_its_login() {
    let proxy = Proxy::start().await;
    let server = server(proxy.port);
    let (target, echoing) = echo().await;
    let tunnels = [
        local(&server, free_port()),
        local(&server, free_port()),
        remote(&server, target),
    ];
    let (_directory, mut m) = manager(&server, &tunnels).await;
    for tunnel in &tunnels {
        m.connect(tunnel.id).unwrap();
    }
    for tunnel in &tunnels {
        wait_status(&m, tunnel.id, Status::Connected).await;
    }
    roundtrip(tunnels[0].local_port, b"first").await;
    roundtrip(tunnels[1].local_port, b"second").await;
    roundtrip(tunnels[2].remote_port, b"reverse").await;
    assert_eq!(proxy.logins(), 1);
    assert_eq!(connections(&m, &server), Some(1));
    m.shutdown().await;
    echoing.abort();
}

#[tokio::test]
#[ignore = "Run with cargo xtask test ssh and its ephemeral SSH fixture"]
async fn a_remote_forward_relays_a_server_port_to_this_mac() {
    let server = server(fixture_port());
    let (target, echoing) = echo().await;
    let tunnel = remote(&server, target);
    let (_directory, mut m) = manager(&server, std::slice::from_ref(&tunnel)).await;
    m.connect(tunnel.id).unwrap();
    wait_status(&m, tunnel.id, Status::Connected).await;
    roundtrip(tunnel.remote_port, b"over ssh").await;
    wait_health(&m, tunnel.id, DestinationStatus::Reachable).await;
    m.disconnect(tunnel.id).await;
    tokio::time::timeout(Duration::from_secs(5), async {
        while TcpStream::connect(("127.0.0.1", tunnel.remote_port))
            .await
            .is_ok()
        {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("stopping releases the server's port");
    m.shutdown().await;
    echoing.abort();
}

#[tokio::test]
#[ignore = "Run with cargo xtask test ssh and its ephemeral SSH fixture"]
async fn a_lost_connection_reconnects_tunnels_through_the_link() {
    let proxy = Proxy::start().await;
    let server = server(proxy.port);
    let (target, echoing) = echo().await;
    let tunnels = [local(&server, free_port()), remote(&server, target)];
    let (_directory, mut m) = manager(&server, &tunnels).await;
    for tunnel in &tunnels {
        m.connect(tunnel.id).unwrap();
    }
    for tunnel in &tunnels {
        wait_status(&m, tunnel.id, Status::Connected).await;
    }
    assert_eq!(proxy.logins(), 1);
    proxy.cut();
    for tunnel in &tunnels {
        wait_status(&m, tunnel.id, Status::Reconnecting).await;
    }
    // While the link is down the local port stays open.
    TcpStream::connect(("127.0.0.1", tunnels[0].local_port))
        .await
        .expect("the local listener stays open while the link is down");
    proxy.restore();
    for tunnel in &tunnels {
        wait_status(&m, tunnel.id, Status::Connected).await;
    }
    roundtrip(tunnels[0].local_port, b"after the loss").await;
    roundtrip(tunnels[1].remote_port, b"reverse again").await;
    assert_eq!(proxy.logins(), 2, "one login for the link, none per tunnel");
    m.shutdown().await;
    echoing.abort();
}

#[tokio::test]
#[ignore = "Run with cargo xtask test ssh and its ephemeral SSH fixture"]
async fn a_busy_local_port_fails_the_tunnel_without_a_login() {
    let proxy = Proxy::start().await;
    let server = server(proxy.port);
    let occupied = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let tunnel = local(&server, occupied.local_addr().unwrap().port());
    let (_directory, mut m) = manager(&server, std::slice::from_ref(&tunnel)).await;
    m.connect(tunnel.id).unwrap();
    let failed = wait_status(&m, tunnel.id, Status::Error).await;
    assert!(
        failed
            .error_message
            .as_deref()
            .is_some_and(|message| message.contains("Cannot bind local port")),
        "{:?}",
        failed.error_message
    );
    assert_eq!(proxy.logins(), 0);
    assert!(m.active_tunnels(server.id).is_empty());
    m.shutdown().await;
}

#[tokio::test]
#[ignore = "Run with cargo xtask test ssh and its ephemeral SSH fixture"]
async fn without_auto_reconnect_a_lost_connection_stops_the_tunnel() {
    let proxy = Proxy::start().await;
    let server = server(proxy.port);
    let mut tunnel = local(&server, free_port());
    tunnel.auto_reconnect = false;
    let (_directory, mut m) = manager(&server, std::slice::from_ref(&tunnel)).await;
    m.connect(tunnel.id).unwrap();
    wait_status(&m, tunnel.id, Status::Connected).await;
    roundtrip(tunnel.local_port, b"before the loss").await;
    proxy.cut();
    let failed = wait_status(&m, tunnel.id, Status::Error).await;
    assert!(failed.error_message.is_some());
    tokio::time::timeout(Duration::from_secs(5), async {
        while !m.active_tunnels(server.id).is_empty() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("the tunnel stops");
    std::net::TcpListener::bind(("127.0.0.1", tunnel.local_port))
        .expect("a stopped tunnel releases its local port");
    proxy.restore();
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert_eq!(state(&m, tunnel.id).status, Status::Error);
    m.shutdown().await;
}
