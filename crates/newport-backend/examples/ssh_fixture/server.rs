use russh::{
    Channel, ChannelId, ChannelOpenFailure, keys,
    server::{self, Auth, ChannelOpenHandle, Msg, Server as _, Session},
};
use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    task::JoinHandle,
};
#[path = "pty.rs"]
mod pty;
#[path = "sftp.rs"]
mod sftp;

#[derive(Clone)]
struct Config {
    http_port: u16,
    home: PathBuf,
    runtime: PathBuf,
    agent: PathBuf,
    key: keys::PublicKey,
    sessions: Arc<Mutex<HashMap<uuid::Uuid, server::Handle>>>,
}
struct Listener(Config);
impl server::Server for Listener {
    type Handler = Client;
    fn new_client(&mut self, _: Option<std::net::SocketAddr>) -> Client {
        Client {
            connection_id: uuid::Uuid::new_v4(),
            config: self.0.clone(),
            channels: HashMap::new(),
            sessions: Arc::default(),
            max_sessions: 10,
            ptys: HashMap::new(),
            tasks: Vec::new(),
        }
    }
    fn handle_session_error(&mut self, error: anyhow::Error) {
        eprintln!("Fixture session: {error}");
    }
}
struct AbortTask(JoinHandle<()>);
impl Drop for AbortTask {
    fn drop(&mut self) {
        self.0.abort();
    }
}
struct Client {
    connection_id: uuid::Uuid,
    config: Config,
    channels: HashMap<ChannelId, Channel<Msg>>,
    sessions: Arc<Mutex<HashSet<ChannelId>>>,
    max_sessions: usize,
    ptys: HashMap<ChannelId, pty::Pty>,
    tasks: Vec<JoinHandle<()>>,
}
struct SessionSlot(Arc<Mutex<HashSet<ChannelId>>>, ChannelId);
impl Drop for SessionSlot {
    fn drop(&mut self) {
        self.0.lock().unwrap().remove(&self.1);
    }
}
impl Client {
    fn session_task(&mut self, id: ChannelId, task: JoinHandle<()>) {
        let slot = SessionSlot(self.sessions.clone(), id);
        let mut task = AbortTask(task);
        self.tasks.push(tokio::spawn(async move {
            let _slot = slot;
            let _ = (&mut task.0).await;
        }));
    }
}
impl Drop for Client {
    fn drop(&mut self) {
        self.config
            .sessions
            .lock()
            .unwrap()
            .remove(&self.connection_id);
        for task in &self.tasks {
            task.abort();
        }
    }
}
impl server::Handler for Client {
    type Error = anyhow::Error;
    async fn auth_password(&mut self, user: &str, password: &str) -> anyhow::Result<Auth> {
        Ok(if user == "fixture" && password == "fixture-password" {
            Auth::Accept
        } else {
            Auth::reject()
        })
    }
    async fn auth_publickey(&mut self, user: &str, key: &keys::PublicKey) -> anyhow::Result<Auth> {
        Ok(
            if matches!(user, "fixture" | "fixture-many-sessions") && key == &self.config.key {
                // The runtime-admission test deliberately exceeds SSH's default
                // to exercise Needle's independent 16-worker bound.
                self.max_sessions = if user == "fixture-many-sessions" {
                    32
                } else {
                    10
                };
                Auth::Accept
            } else {
                Auth::reject()
            },
        )
    }
    async fn auth_succeeded(&mut self, session: &mut Session) -> anyhow::Result<()> {
        self.config
            .sessions
            .lock()
            .unwrap()
            .insert(self.connection_id, session.handle());
        Ok(())
    }
    async fn channel_open_session(
        &mut self,
        channel: Channel<Msg>,
        reply: ChannelOpenHandle,
        _: &mut Session,
    ) -> anyhow::Result<()> {
        // Match OpenSSH's default per-connection session limit. Forwarded TCP
        // channels do not consume a session slot.
        if self.sessions.lock().unwrap().len() >= self.max_sessions {
            reply.reject(ChannelOpenFailure::ConnectFailed).await;
            return Ok(());
        }
        self.sessions.lock().unwrap().insert(channel.id());
        self.channels.insert(channel.id(), channel);
        reply.accept().await;
        Ok(())
    }
    async fn channel_close(&mut self, id: ChannelId, _: &mut Session) -> anyhow::Result<()> {
        self.sessions.lock().unwrap().remove(&id);
        self.channels.remove(&id);
        self.ptys.remove(&id);
        Ok(())
    }
    async fn channel_open_direct_tcpip(
        &mut self,
        channel: Channel<Msg>,
        host: &str,
        port: u32,
        _: &str,
        _: u32,
        reply: ChannelOpenHandle,
        _: &mut Session,
    ) -> anyhow::Result<()> {
        if host == "health.fixture" && (port == 1 || port == 2) {
            reply
                .reject(if port == 1 {
                    ChannelOpenFailure::ConnectFailed
                } else {
                    ChannelOpenFailure::AdministrativelyProhibited
                })
                .await;
        } else if host == "127.0.0.1" && port == u32::from(self.config.http_port) {
            let mut socket =
                tokio::net::TcpStream::connect(("127.0.0.1", self.config.http_port)).await?;
            reply.accept().await;
            self.tasks.push(tokio::spawn(async move {
                let _ =
                    tokio::io::copy_bidirectional(&mut channel.into_stream(), &mut socket).await;
            }));
        } else {
            reply.accept().await;
            self.tasks.push(tokio::spawn(async move {
                let mut stream = channel.into_stream();
                let mut bytes = [0; 65536];
                while let Ok(n) = stream.read(&mut bytes).await {
                    if n == 0 || stream.write_all(&bytes[..n]).await.is_err() {
                        break;
                    }
                }
            }));
        }
        Ok(())
    }
    async fn tcpip_forward(
        &mut self,
        address: &str,
        port: &mut u32,
        session: &mut Session,
    ) -> anyhow::Result<bool> {
        if address != "127.0.0.1" {
            return Ok(false);
        }
        let Ok(listener) = TcpListener::bind((address, *port as u16)).await else {
            return Ok(false);
        };
        *port = listener.local_addr()?.port().into();
        let port = *port;
        let handle = session.handle();
        let address = address.to_owned();
        self.tasks.push(tokio::spawn(async move {
            while let Ok((mut socket, peer)) = listener.accept().await {
                let Ok(channel) = handle
                    .channel_open_forwarded_tcpip(
                        &address,
                        port,
                        peer.ip().to_string(),
                        peer.port().into(),
                    )
                    .await
                else {
                    break;
                };
                tokio::spawn(async move {
                    let _ = tokio::io::copy_bidirectional(&mut socket, &mut channel.into_stream())
                        .await;
                });
            }
        }));
        Ok(true)
    }
    async fn subsystem_request(
        &mut self,
        id: ChannelId,
        name: &str,
        session: &mut Session,
    ) -> anyhow::Result<()> {
        if name == "sftp" {
            session.channel_success(id)?;
            let stream = self.channels.remove(&id).unwrap().into_stream();
            let handler = sftp::Sftp::new(self.config.home.clone());
            self.session_task(
                id,
                tokio::spawn(async move {
                    russh_sftp::server::run(stream, handler).await;
                }),
            );
        } else {
            session.channel_failure(id)?;
        }
        Ok(())
    }
    async fn pty_request(
        &mut self,
        id: ChannelId,
        _: &str,
        cols: u32,
        rows: u32,
        _: u32,
        _: u32,
        _: &[(russh::Pty, u32)],
        session: &mut Session,
    ) -> anyhow::Result<()> {
        if cols == 13 {
            session.channel_failure(id)?;
        } else {
            self.ptys.insert(id, pty::Pty::new(cols, rows)?);
            session.channel_success(id)?;
        }
        Ok(())
    }
    async fn window_change_request(
        &mut self,
        id: ChannelId,
        cols: u32,
        rows: u32,
        _: u32,
        _: u32,
        _: &mut Session,
    ) -> anyhow::Result<()> {
        if let Some(pty) = self.ptys.get(&id) {
            pty.resize(cols, rows)?;
        }
        Ok(())
    }
    async fn shell_request(&mut self, id: ChannelId, session: &mut Session) -> anyhow::Result<()> {
        let Some(pty) = self.ptys.get_mut(&id) else {
            session.channel_failure(id)?;
            return Ok(());
        };
        session.data(id, b"early shell output\r\n".to_vec())?;
        session.channel_success(id)?;
        let task = pty.start(
            self.channels.remove(&id).unwrap(),
            self.config.home.clone(),
            None,
            None,
            self.connection_id,
        )?;
        self.session_task(id, task);
        Ok(())
    }
    async fn exec_request(
        &mut self,
        id: ChannelId,
        command: &[u8],
        session: &mut Session,
    ) -> anyhow::Result<()> {
        if let Some((selector, terminal)) = terminal_identity(command)
            && let Some(pty) = self.ptys.get_mut(&id)
        {
            session.data(id, b"early shell output\r\n".to_vec())?;
            session.channel_success(id)?;
            let task = pty.start(
                self.channels.remove(&id).unwrap(),
                self.config.home.clone(),
                Some(selector),
                terminal,
                self.connection_id,
            )?;
            self.session_task(id, task);
            return Ok(());
        }
        session.channel_success(id)?;
        let channel = self.channels.remove(&id).unwrap();
        let command = command.to_vec();
        let config = self.config.clone();
        self.session_task(
            id,
            tokio::spawn(async move {
                if let Err(error) = execute(channel, &command, config).await {
                    eprintln!("Fixture exec: {error}");
                }
            }),
        );
        Ok(())
    }
}
/// Recognize the complete terminal command, including its optional tab identity.
fn terminal_identity(command: &[u8]) -> Option<(uuid::Uuid, Option<uuid::Uuid>)> {
    let command = std::str::from_utf8(command).ok()?;
    let selector =
        uuid::Uuid::parse_str(command.split_once("NEWPORT_SESSION=")?.1.get(..36)?).ok()?;
    let terminal = if let Some((_, value)) = command.split_once("NEWPORT_TERMINAL=") {
        // The UUID is single-quoted inside the outer `sh -c` argument.
        Some(uuid::Uuid::parse_str(value.strip_prefix("'\\''")?.get(..36)?).ok()?)
    } else {
        None
    };
    (command == newport_backend::ssh::terminal_command(selector, terminal))
        .then_some((selector, terminal))
}

fn fixture_worker_socket(socket: &str, runtime: &std::path::Path) -> bool {
    let path = std::path::Path::new(socket);
    path.file_name().is_some_and(|name| name == "workers.sock")
        && path.parent().is_some_and(|parent| {
            parent.parent() == Some(runtime.join("newport-ipc").as_path())
                && parent
                    .file_name()
                    .and_then(|s| s.to_str())
                    .is_some_and(|name| name.starts_with("r-"))
        })
}

async fn execute(channel: Channel<Msg>, command: &[u8], config: Config) -> anyhow::Result<()> {
    let worker_args = std::str::from_utf8(command)
        .ok()
        .and_then(|s| {
            s.strip_prefix("exec \"$HOME/.local/share/newport/needle/builds/")?
                .split_once("/newport-agent\" worker ")
                .map(|(_, args)| args)
        })
        .map(|s| s.split_whitespace().map(str::to_owned).collect::<Vec<_>>())
        .map(|mut args| {
            if args.len() >= 3 {
                args[2] = args[2].trim_matches('\'').to_owned();
            }
            args
        })
        .filter(|args| {
            args.len() >= 5
                && newport_backend::agent_runtime::protocol::valid_build(&args[0])
                && uuid::Uuid::parse_str(&args[1]).is_ok()
                && fixture_worker_socket(&args[2], &config.runtime)
                && uuid::Uuid::parse_str(&args[3]).is_ok()
                && (args.len() == 5 && matches!(args[4].as_str(), "git" | "monitor")
                    || args.len() == 6
                        && args[4] == "integration"
                        && uuid::Uuid::parse_str(&args[5]).is_ok())
        });
    // Exercise the production managed service launcher with the locally built
    // agent. These are exact fixture routes, not arbitrary shell execution.
    let command = if command == b"exec \"$HOME/.local/bin/newport-agent\" runtime --stdio" {
        b"fixture-runtime".as_slice()
    } else if worker_args.is_some() {
        b"fixture-worker".as_slice()
    } else if command.starts_with(b"exec \"$HOME/.local/bin/newport-agent\" serve ")
        && command
            .windows(b"--managed".len())
            .any(|w| w == b"--managed")
    {
        b"fixture-managed-agent".as_slice()
    } else {
        command
    };
    let (mut read, write) = channel.split();
    let code = match command {
        command if command == include_bytes!("../../src/agent-release-check.sh") => {
            write
                .data(format!("Linux\nx86_64\n{}\n", "0".repeat(64)).as_bytes())
                .await?;
            0
        }
        b"test -x \"$HOME/.local/bin/newport-agent\" && printf installed || true" => {
            write.data(&b"installed"[..]).await?;
            0
        }
        b"drop" => {
            write.exit_status(0).await?;
            tokio::time::sleep(Duration::from_millis(100)).await;
            let handles = config.sessions.lock().unwrap().clone();
            for handle in handles.into_values() {
                let _ = handle
                    .disconnect(
                        russh::Disconnect::ByApplication,
                        "fixture disconnect".into(),
                        "".into(),
                    )
                    .await;
            }
            return Ok(());
        }
        command if command.starts_with(b"drop-connection ") => {
            let id = uuid::Uuid::parse_str(std::str::from_utf8(&command[16..])?)?;
            let handle = config.sessions.lock().unwrap().get(&id).cloned();
            if let Some(handle) = handle {
                handle
                    .disconnect(
                        russh::Disconnect::ByApplication,
                        "fixture disconnect".into(),
                        "".into(),
                    )
                    .await?;
            }
            0
        }
        b"printf ok" => {
            write.data(&b"ok"[..]).await?;
            0
        }
        b"printf connected" | b"printf still-connected" | b"printf shared" => {
            write.data(&command[b"printf ".len()..]).await?;
            0
        }
        b"fail" => {
            write.extended_data(1, &b"fixture failure"[..]).await?;
            7
        }
        b"large" => {
            write.data(&vec![b'x'; 5 * 1024 * 1024][..]).await?;
            0
        }
        b"stdin" => {
            let mut count = 0;
            while let Some(msg) = read.wait().await {
                match msg {
                    russh::ChannelMsg::Data { data } => count += data.len(),
                    russh::ChannelMsg::Eof | russh::ChannelMsg::Close => break,
                    _ => {}
                }
            }
            write.data(count.to_string().as_bytes()).await?;
            0
        }
        b"stall-until-close" => {
            while let Some(message) = read.wait().await {
                if matches!(message, russh::ChannelMsg::Close) {
                    break;
                }
            }
            return Ok(());
        }
        b"fixture-agent"
        | b"fixture-runtime"
        | b"fixture-worker"
        | b"fixture-managed-agent"
        | b"fixture-clipboard"
        | b"fixture-open" => {
            let args: Vec<&str> = match command {
                b"fixture-runtime" => vec!["runtime", "--stdio"],
                b"fixture-worker" => std::iter::once("worker")
                    .chain(worker_args.as_ref().unwrap().iter().map(String::as_str))
                    .collect(),
                b"fixture-agent" => vec!["serve", "ssh-fixture", "--clipboard", "--browser"],
                b"fixture-managed-agent" => vec!["serve", "ssh-fixture", "--managed"],
                b"fixture-clipboard" => vec!["clipboard", "-o"],
                _ => vec!["open", "https://example.com/login?code=fixture"],
            };
            let mut child = tokio::process::Command::new(config.agent)
                .args(args)
                .env("HOME", config.home)
                .env("XDG_RUNTIME_DIR", config.runtime)
                .env("NEWPORT_CLIPBOARD_NATIVE", "0")
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::inherit())
                .kill_on_drop(true)
                .spawn()?;
            let mut input = child.stdin.take().unwrap();
            let mut output = child.stdout.take().unwrap();
            let feed = AbortTask(tokio::spawn(async move {
                while let Some(msg) = read.wait().await {
                    match msg {
                        russh::ChannelMsg::Data { data } => {
                            if input.write_all(&data).await.is_err() {
                                break;
                            }
                        }
                        russh::ChannelMsg::Eof | russh::ChannelMsg::Close => break,
                        _ => {}
                    }
                }
            }));
            let mut bytes = [0; 32768];
            loop {
                let n = output.read(&mut bytes).await?;
                if n == 0 {
                    break;
                }
                if write.data(&bytes[..n]).await.is_err() {
                    feed.0.abort();
                    return Ok(());
                }
            }
            feed.0.abort();
            child.wait().await?.code().unwrap_or(1) as u32
        }
        _ => {
            write
                .data(
                    &b"LISTEN 0 128 127.0.0.1:5432 0.0.0.0:* users:((\"postgres\",pid=812,fd=3))\n"
                        [..],
                )
                .await?;
            0
        }
    };
    write.exit_status(code).await?;
    write.eof().await?;
    write.close().await?;
    Ok(())
}
pub async fn run() -> anyhow::Result<()> {
    let directory = PathBuf::from(std::env::args().nth(1).expect("fixture directory"));
    let home = directory.join("remote-home");
    std::fs::create_dir_all(home.join("Documents"))?;
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&home, std::fs::Permissions::from_mode(0o700))?;
    let home = home.canonicalize()?;
    let runtime = directory.join("ipc");
    std::fs::create_dir(&runtime)?;
    std::fs::set_permissions(&runtime, std::fs::Permissions::from_mode(0o700))?;
    let runtime = runtime.canonicalize()?;
    std::fs::write(
        home.join("Documents/hello 世界.txt"),
        "Hello over SFTP!\n<script>inert text</script>\n",
    )?;
    std::fs::write(home.join("binary.dat"), (0..=255u8).collect::<Vec<_>>())?;
    std::fs::write(home.join("large.txt"), vec![b'x'; 17 * 1024 * 1024])?;
    let key = keys::load_public_key(directory.join("client.pub"))?;
    let host = keys::load_secret_key(directory.join("host"), None)?;
    // A separate real HTTP destination exercises forwarding through the SSH
    // channel without changing the echo protocol used by the transport tests.
    let http = TcpListener::bind(("127.0.0.1", 0)).await?;
    let http_port = http.local_addr()?.port();
    let _http = AbortTask(tokio::spawn(async move {
        while let Ok((mut socket, _)) = http.accept().await {
            tokio::spawn(async move {
                let _ = tokio::time::timeout(Duration::from_secs(5), async {
                    let mut request = Vec::new();
                    let mut bytes = [0; 1024];
                    while !request.windows(4).any(|v| v == b"\r\n\r\n") {
                        let n = socket.read(&mut bytes).await?;
                        if n == 0 || request.len() + n > 8192 {
                            return Ok::<_, std::io::Error>(());
                        }
                        request.extend_from_slice(&bytes[..n]);
                    }
                    if request.starts_with(b"GET / HTTP/1.0\r\n") {
                        socket.write_all(b"HTTP/1.0 200 OK\r\nContent-Length: 21\r\nConnection: close\r\n\r\nNewport HTTP fixture\r\n").await?;
                    }
                    socket.shutdown().await
                })
                .await;
            });
        }
    }));
    std::fs::write(directory.join("http-port"), http_port.to_string())?;
    let listener = TcpListener::bind(("127.0.0.1", 0)).await?;
    let port = listener.local_addr()?.port();
    std::fs::write(
        directory.join("known_hosts"),
        format!("[127.0.0.1]:{port} {}\n", host.public_key().to_openssh()?),
    )?;
    let config = Arc::new(server::Config {
        keys: vec![host],
        auth_rejection_time: Duration::from_millis(10),
        ..Default::default()
    });
    let mut server = Listener(Config {
        http_port,
        home,
        runtime,
        agent: PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../tools/agent/target/debug/newport-agent"),
        key,
        sessions: Arc::default(),
    });
    std::fs::write(directory.join("port"), port.to_string())?;
    server.run_on_socket(config, &listener).await?;
    Ok(())
}
