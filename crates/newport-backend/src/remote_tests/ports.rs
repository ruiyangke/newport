//! Port discovery and forwarding on a real Linux server: what runs where (Git
//! checkouts and worktrees, plain projects, vanished folders), and traffic in
//! both forwarding directions.
use super::server;
use crate::{
    model::{DestinationStatus, Direction, Tunnel},
    ports::{DiscoveredPort, discover},
    ssh::{self, Forwarding},
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    time::{Duration, sleep, timeout},
};
use uuid::Uuid;

async fn run(script: &str) -> String {
    ssh::execute(&server(), script, None)
        .await
        .unwrap_or_else(|error| panic!("{error}\n--- script ---\n{script}"))
}

/// Ports 18101-18105, one per layout below; a disposable container, so fixed.
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
async fn discovered() -> (crate::model::Server, Vec<DiscoveredPort>) {
    let server =
        super::installed_server("NEWPORT_TEST_SSH_PORT", "Disposable ports", "fixture").await;
    let ports = discover(&super::owner(&server)).await.unwrap();
    (server, ports)
}

fn on(ports: &[DiscoveredPort], port: u16) -> &DiscoveredPort {
    ports
        .iter()
        .find(|p| p.port == port)
        .unwrap_or_else(|| panic!("port {port} was not discovered: {ports:#?}"))
}

#[tokio::test]
#[ignore = "Requires npm run test:remote and real OpenSSH"]
async fn discovery_attributes_listeners_to_projects_and_worktrees() {
    let base = run(LAYOUT).await;
    let shop = format!("{base}/shop");
    let (_, ports) = discovered().await;
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        // The main checkout: its repository is the project.
        let main = on(&ports, 18101);
        assert_eq!(
            main.working_directory.as_deref(),
            Some(&*format!("{shop}/web"))
        );
        assert_eq!(main.project_root.as_deref(), Some(&*shop));
        assert_eq!(main.project_marker.as_deref(), Some(".git"));
        assert_eq!(main.worktree_path.as_deref(), Some(&*shop));
        assert_eq!(main.worktree_branch.as_deref(), Some("main"));
        assert_eq!(main.worktree_main, Some(true));
        assert!(main.pid.is_some() && main.executable.is_some());

        // A linked worktree beside the repository belongs to the same project.
        let search = on(&ports, 18102);
        assert_eq!(search.project_root.as_deref(), Some(&*shop));
        assert_eq!(
            search.worktree_path.as_deref(),
            Some(&*format!("{base}/shop-search"))
        );
        assert_eq!(search.worktree_branch.as_deref(), Some("feat/search"));
        assert_eq!(search.worktree_main, Some(false));

        // A detached worktree inside it reports its commit, not a branch.
        let hotfix = on(&ports, 18103);
        assert_eq!(hotfix.project_root.as_deref(), Some(&*shop));
        assert_eq!(hotfix.worktree_main, Some(false));
        let commit = hotfix.worktree_branch.as_deref().unwrap();
        assert!(
            commit.len() == 7 && commit.chars().all(|c| c.is_ascii_hexdigit()),
            "{commit}"
        );

        // A project without version control is found by its project file.
        let plain = on(&ports, 18104);
        assert_eq!(
            plain.project_root.as_deref(),
            Some(&*format!("{base}/plain"))
        );
        assert_eq!(plain.project_marker.as_deref(), Some("package.json"));
        assert_eq!(plain.worktree_path, None);

        // A vanished folder is reported as such and credited to no project.
        let gone = on(&ports, 18105);
        assert_eq!(
            gone.working_directory.as_deref(),
            Some(&*format!("{base}/gone"))
        );
        assert!(gone.working_directory_deleted);
        assert_eq!(gone.project_root, None);
    }));
    run(CLEANUP).await;
    if let Err(panic) = result {
        std::panic::resume_unwind(panic);
    }
}

#[tokio::test]
#[ignore = "Requires npm run test:remote and real OpenSSH"]
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
    let forwarding = Forwarding::start(&server, Some(&tunnel)).await.unwrap();
    // A client on the server reaches this machine through the forwarded port.
    let reply = run("exec 3<>/dev/tcp/127.0.0.1/18200; printf 'over ssh' >&3; head -c 8 <&3").await;
    assert_eq!(reply, "over ssh");
    timeout(Duration::from_secs(15), async {
        while forwarding.destination_health()[0].status != DestinationStatus::Reachable {
            sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .expect("the local target is reported reachable");
    assert!(forwarding.error().is_none());

    // Without the service here, the server's client is closed at once rather
    // than left hanging (a hang would exit 124 from `timeout`).
    echo.abort();
    sleep(Duration::from_millis(200)).await;
    let refused =
        run("exec 3<>/dev/tcp/127.0.0.1/18200 && timeout 5 head -c 1 <&3; echo \"read:$?\"").await;
    assert_eq!(refused.trim(), "read:0");

    forwarding.shutdown().await;
    let closed = run(
        "for i in $(seq 50); do ss -ltn | grep -q ':18200 ' || { echo closed; exit 0; }; sleep 0.1; done; echo open",
    )
    .await;
    assert_eq!(closed.trim(), "closed");
}

#[tokio::test]
#[ignore = "Requires npm run test:remote and real OpenSSH"]
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
        remote_host: found.address.clone(),
        remote_port: found.port,
        remote_port_end: None,
        auto_connect: false,
        auto_reconnect: false,
        direction: Direction::Local,
        project: None,
    };
    let forwarding = Forwarding::start(&server, Some(&tunnel)).await.unwrap();
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
