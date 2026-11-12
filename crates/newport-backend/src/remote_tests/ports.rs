//! Port discovery and forwarding on a real Linux server: what runs where (Git
//! checkouts and worktrees, plain projects, vanished folders), and traffic in
//! both forwarding directions.
use super::server;
use crate::{
    model::{DestinationStatus, Direction, Server, Status, Tunnel},
    ports::discover,
    ssh::{self, forward::Forward},
};
use newport_protocol::monitor::ports::{
    Project,
    discovery::{Details, Snapshot, Socket},
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    time::{Duration, sleep, timeout},
};
use uuid::Uuid;

/// A forward on its own link to `server`, once it is connected.
async fn started(server: &Server, tunnel: &Tunnel) -> Forward {
    let forward = Forward::start(ssh::link(server, crate::ssh::no_jump_hosts), tunnel, true)
        .await
        .unwrap();
    let mut state = forward.state();
    let connected = timeout(
        Duration::from_secs(15),
        state.wait_for(|state| matches!(state.status, Status::Connected | Status::Error)),
    )
    .await
    .expect("the forward connects")
    .unwrap()
    .clone();
    assert_eq!(
        connected.status,
        Status::Connected,
        "{:?}",
        connected.error_message
    );
    forward
}

async fn run(script: &str) -> String {
    crate::remote::Shell::new(&server())
        .execute(script, None)
        .await
        .unwrap_or_else(|error| panic!("{error}\n--- script ---\n{script}"))
}

/// One port per layout below; a disposable container, so they are fixed.
const LAYOUT: &str = r#"
set -eu
base=$(mktemp -d "$HOME/ports.XXXXXX")
cd "$base"
git init -q -b main shop
git -C shop -c user.name=Fixture -c user.email=fixture@example.invalid commit -q --allow-empty -m init
mkdir shop/web
git -C shop worktree add -q -b feat/search ../shop-search
git -C shop worktree add -q --detach .worktrees/hotfix
mkdir plain gone
touch plain/package.json
# `setsid -f` fully detaches; a backgrounded child would hold the SSH channel.
serve() { (cd "$1" && setsid -f newport-test-fixture http "$2" </dev/null >/dev/null 2>&1); }
serve shop/web 18101
serve shop-search 18102
serve shop/.worktrees/hotfix 18103
serve plain 18104
serve gone 18105
for port in 18101 18102 18103 18104 18105; do
    tries=0
    until ss -ltn | grep -q ":$port "; do
        tries=$((tries + 1)); [ "$tries" -lt 50 ] || exit 1; sleep 0.1
    done
done
# A dev server whose checkout is removed while it keeps running.
rm -rf gone
printf '%s' "$base"
"#;
const CLEANUP: &str =
    r#"pkill -f '^newport-test-fixture http 181' || true; rm -rf "$HOME"/ports.*"#;

/// Discovery runs in the agent, deployed explicitly as every feature test does.
async fn discovered() -> (crate::model::Server, Snapshot) {
    let server =
        super::installed_server("NEWPORT_TEST_SSH_PORT", "Disposable ports", "fixture").await;
    let ports = discover(&super::owner(&server)).await.unwrap();
    (server, ports)
}

fn on(ports: &Snapshot, port: u16) -> &Socket {
    ports
        .sockets
        .iter()
        .find(|p| p.endpoint.port == port)
        .unwrap_or_else(|| panic!("port {port} was not discovered: {ports:#?}"))
}
fn details(ports: &Snapshot, port: u16) -> &Details {
    let socket = on(ports, port);
    ports
        .processes
        .iter()
        .filter(|p| socket.holders.contains(&p.id))
        .find_map(|p| p.details.as_ref())
        .expect("verified fixture holder details")
}
fn project(ports: &Snapshot, port: u16) -> &Project {
    details(ports, port)
        .project
        .as_ref()
        .expect("fixture project")
}

#[tokio::test]
#[ignore = "Requires cargo xtask test remote and real OpenSSH"]
async fn discovery_attributes_listeners_to_projects_and_worktrees() {
    let base = run(LAYOUT).await;
    let shop = format!("{base}/shop");
    let (_, ports) = discovered().await;
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let main = details(&ports, 18101);
        assert_eq!(
            main.directory.as_ref().unwrap().value,
            format!("{shop}/web")
        );
        let main_project = project(&ports, 18101);
        assert_eq!(main_project.root, shop);
        assert_eq!(main_project.marker, ".git");
        assert_eq!(main_project.worktree.as_deref(), Some(&*shop));
        assert_eq!(main_project.branch.as_deref(), Some("main"));
        assert_eq!(main_project.main, Some(true));
        assert!(!on(&ports, 18101).holders.is_empty() && main.executable.is_some());
        let search = project(&ports, 18102);
        assert_eq!(search.root, shop);
        assert_eq!(
            search.worktree.as_deref(),
            Some(&*format!("{base}/shop-search"))
        );
        assert_eq!(search.branch.as_deref(), Some("feat/search"));
        assert_eq!(search.main, Some(false));
        let hotfix = project(&ports, 18103);
        assert_eq!(hotfix.root, shop);
        assert_eq!(hotfix.main, Some(false));
        let commit = hotfix.branch.as_deref().unwrap();
        assert!(
            commit.len() == 7 && commit.chars().all(|c| c.is_ascii_hexdigit()),
            "{commit}"
        );
        let plain = project(&ports, 18104);
        assert_eq!(plain.root, format!("{base}/plain"));
        assert_eq!(plain.marker, "package.json");
        assert_eq!(plain.worktree, None);
        let gone = details(&ports, 18105);
        assert_eq!(
            gone.directory.as_ref().unwrap().value,
            format!("{base}/gone")
        );
        assert!(gone.directory.as_ref().unwrap().deleted);
        assert_eq!(gone.project, None);
    }));
    run(CLEANUP).await;
    if let Err(panic) = result {
        std::panic::resume_unwind(panic);
    }
}

#[tokio::test]
#[ignore = "Requires cargo xtask test remote and real OpenSSH"]
async fn reverse_forward_serves_this_machine_on_the_server() {
    // The service on "this Mac": echoes what it receives.
    let target = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let local = target.local_addr().unwrap().port();
    let echo = tokio::spawn(async move {
        while let Ok((mut socket, _)) = target.accept().await {
            tokio::spawn(async move {
                let (mut read, mut write) = socket.split();
                let _ = tokio::io::copy(&mut read, &mut write).await;
            });
        }
    });
    let server = server();
    let tunnel = Tunnel {
        id: Uuid::new_v4(),
        name: "Reverse".into(),
        server_id: server.id,
        local_port: local,
        local_port_end: None,
        remote_host: "127.0.0.1".into(),
        remote_port: 18200,
        remote_port_end: None,
        auto_connect: false,
        auto_reconnect: false,
        direction: Direction::Remote,
        project: None,
    };
    let forwarding = started(&server, &tunnel).await;
    // A client on the server reaches this machine through the forwarded port.
    let reply = run(&format!(
        "bash -c {}",
        ssh::shell_quote("exec 3<>/dev/tcp/127.0.0.1/18200; printf 'over ssh' >&3; head -c 8 <&3")
    ))
    .await;
    assert_eq!(reply, "over ssh");
    timeout(Duration::from_secs(15), async {
        while forwarding.destination_health()[0].status != DestinationStatus::Reachable {
            sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .expect("the local target is reported reachable");
    assert_eq!(forwarding.state().borrow().status, Status::Connected);

    // Without the service here, the server's client is closed at once rather
    // than left hanging (a hang would exit 124 from `timeout`).
    echo.abort();
    sleep(Duration::from_millis(200)).await;
    let refused = run(&format!(
        "bash -c {}",
        ssh::shell_quote(
            "exec 3<>/dev/tcp/127.0.0.1/18200 && timeout 5 head -c 1 <&3; echo \"read:$?\""
        )
    ))
    .await;
    assert_eq!(refused.trim(), "read:0");

    forwarding.shutdown().await;
    let closed = run(
        "for i in $(seq 50); do ss -ltn | grep -q ':18200 ' || { echo closed; exit 0; }; sleep 0.1; done; echo open",
    )
    .await;
    assert_eq!(closed.trim(), "closed");
}

#[tokio::test]
#[ignore = "Requires cargo xtask test remote and real OpenSSH"]
async fn local_forward_reaches_a_discovered_port() {
    // Discover a listener, then forward to exactly what discovery reported.
    run(
        r#"(cd /srv/fixture && setsid -f newport-test-fixture http 18300 </dev/null >/dev/null 2>&1)
for i in $(seq 50); do ss -ltn | grep -q ':18300 ' && exit 0; sleep 0.1; done; exit 1"#,
    )
    .await;
    let (server, ports) = discovered().await;
    let found = on(&ports, 18300).clone();
    let reservation = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let local = reservation.local_addr().unwrap().port();
    drop(reservation);
    let tunnel = Tunnel {
        id: Uuid::new_v4(),
        name: "Discovered".into(),
        server_id: server.id,
        local_port: local,
        local_port_end: None,
        remote_host: found.endpoint.address.to_string(),
        remote_port: found.endpoint.port,
        remote_port_end: None,
        auto_connect: false,
        auto_reconnect: false,
        direction: Direction::Local,
        project: None,
    };
    let forwarding = started(&server, &tunnel).await;
    let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", local))
        .await
        .unwrap();
    stream
        .write_all(b"GET / HTTP/1.0\r\nHost: localhost\r\n\r\n")
        .await
        .unwrap();
    let mut response = String::new();
    timeout(
        Duration::from_secs(10),
        stream.read_to_string(&mut response),
    )
    .await
    .unwrap()
    .unwrap();
    forwarding.shutdown().await;
    run("pkill -f '^newport-test-fixture http 18300' || true").await;
    assert!(response.contains("200 OK"), "{response}");
}

/// A local forward from a free port on this machine to `remote_host` on `server`.
fn local_forward(
    server: &Server,
    name: &str,
    remote_host: &str,
    remote_port: u16,
) -> (Tunnel, u16) {
    let local = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let tunnel = Tunnel {
        id: Uuid::new_v4(),
        name: name.into(),
        server_id: server.id,
        local_port: local,
        local_port_end: None,
        remote_host: remote_host.into(),
        remote_port,
        remote_port_end: None,
        auto_connect: false,
        auto_reconnect: false,
        direction: Direction::Local,
        project: None,
    };
    (tunnel, local)
}

/// The first destination check's outcome once it has run.
async fn checked_destination(forward: &Forward) -> crate::model::DestinationHealth {
    timeout(Duration::from_secs(15), async {
        loop {
            let health = forward.destination_health().remove(0);
            if health.status != DestinationStatus::Checking {
                return health;
            }
            sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("the destination check finishes")
}

/// An sshd that listens on IPv4 only, as on a server with IPv6 disabled, and
/// a dual-stack listener behind it. Started by the fixture user, so it serves
/// that account only.
const IPV4_ONLY_SSHD: &str = r#"
set -eu
dir="$HOME/ipv4-only-sshd"
rm -rf "$dir"
mkdir "$dir"
ssh-keygen -q -t ed25519 -N '' -f "$dir/host_key"
cat > "$dir/sshd_config" <<CONFIG
Port 18410
AddressFamily inet
ListenAddress 127.0.0.1
HostKey $dir/host_key
PidFile $dir/sshd.pid
PasswordAuthentication no
PubkeyAuthentication yes
UsePAM no
AllowTcpForwarding yes
CONFIG
setsid -f /usr/sbin/sshd -f "$dir/sshd_config" -E "$dir/sshd.log" </dev/null >/dev/null 2>&1
# Bun binds `[::]` with IPv4-mapped addresses accepted, like a dual-stack web server.
setsid -f bun -e 'Bun.listen({ hostname: "::", port: 18411, socket: { data(socket, data) { socket.write(data); } } }); setInterval(() => {}, 1 << 30)' </dev/null >/dev/null 2>&1
for port in 18410 18411; do
    tries=0
    until ss -ltn | grep -q ":$port "; do
        tries=$((tries + 1)); [ "$tries" -lt 100 ] || { cat "$dir/sshd.log" >&2; exit 1; }; sleep 0.1
    done
done
cat "$dir/host_key.pub"
"#;
const IPV4_ONLY_SSHD_CLEANUP: &str = r#"pkill -f '[i]pv4-only-sshd/sshd_config' || true
pkill -f '[B]un.listen.*18411' || true
rm -rf "$HOME/ipv4-only-sshd""#;

#[tokio::test]
#[ignore = "Requires cargo xtask test remote and real OpenSSH"]
async fn dual_stack_listener_is_forwarded_through_an_ipv4_only_sshd() {
    struct Restore(std::path::PathBuf, Vec<u8>);
    impl Drop for Restore {
        fn drop(&mut self) {
            std::fs::write(&self.0, &self.1).unwrap();
        }
    }
    struct Cleanup;
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = std::thread::spawn(|| {
                tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .unwrap()
                    .block_on(run(IPV4_ONLY_SSHD_CLEANUP))
            })
            .join();
        }
    }
    let _cleanup = Cleanup;
    let host_key = run(IPV4_ONLY_SSHD).await;

    // The container publishes only its main sshd, so reach the IPv4-only one
    // through a forward and trust the host key it generated.
    let main = super::server();
    let (bridge_tunnel, bridge_port) = local_forward(&main, "IPv4-only sshd", "127.0.0.1", 18410);
    let bridge = started(&main, &bridge_tunnel).await;
    let known_hosts = std::path::PathBuf::from(std::env::var("NEWPORT_TEST_KNOWN_HOSTS").unwrap());
    let _restore = Restore(known_hosts.clone(), std::fs::read(&known_hosts).unwrap());
    let mut trusted = std::fs::read_to_string(&known_hosts).unwrap();
    trusted += &format!("[127.0.0.1]:{bridge_port} {host_key}\n");
    std::fs::write(&known_hosts, trusted).unwrap();
    let mut ipv4_only = super::server();
    ipv4_only.id = Uuid::new_v4();
    ipv4_only.ssh_port = bridge_port;

    // A server that accepts IPv4 and IPv6 is reached as `localhost`, which
    // falls back to IPv4 where the sshd cannot connect to `::1`.
    let (tunnel, local) = local_forward(&ipv4_only, "Dual-stack", "localhost", 18411);
    let forward = started(&ipv4_only, &tunnel).await;
    let health = checked_destination(&forward).await;
    assert_eq!(
        health.status,
        DestinationStatus::Reachable,
        "{:?}",
        health.message
    );
    let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", local))
        .await
        .unwrap();
    stream.write_all(b"through the forward").await.unwrap();
    let mut echoed = [0; 19];
    timeout(Duration::from_secs(10), stream.read_exact(&mut echoed))
        .await
        .expect("the listener answers")
        .unwrap();
    assert_eq!(&echoed, b"through the forward");
    forward.shutdown().await;

    // `::1` alone is what the server refuses.
    let (tunnel, _) = local_forward(&ipv4_only, "IPv6 loopback", "::1", 18411);
    let forward = started(&ipv4_only, &tunnel).await;
    let health = checked_destination(&forward).await;
    assert_eq!(health.status, DestinationStatus::Unavailable);
    assert!(
        health
            .message
            .as_deref()
            .is_some_and(|message| message.contains("couldn’t connect")),
        "{:?}",
        health.message
    );
    assert_eq!(forward.state().borrow().status, Status::Connected);
    forward.shutdown().await;
    bridge.shutdown().await;
}
