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
    /// Its own SSH port, which it forwards to as a jump host would.
    ssh_port: u16,
    home: PathBuf,
    runtime: PathBuf,
    agent: PathBuf,
    key: keys::PublicKey,
    sessions: Arc<Mutex<HashMap<uuid::Uuid, server::Handle>>>,
    disconnect_on_session: Arc<Mutex<HashSet<uuid::Uuid>>>,
    paused_auth: tokio::sync::watch::Sender<bool>,
    pool_auth_attempts: Arc<std::sync::atomic::AtomicUsize>,
    refused_auth_attempts: Arc<std::sync::atomic::AtomicUsize>,
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
            streamlocal: false,
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
    streamlocal: bool,
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
    async fn auth_publickey(
        &mut self,
        user: &str,
        verified: russh::server::VerifiedPublicKey<'_>,
    ) -> anyhow::Result<Auth> {
        let key = verified.public_key;
        Ok(
            if matches!(
                user,
                "fixture"
                    | "fixture-many-sessions"
                    | "fixture-one-session"
                    | "fixture-no-sessions"
                    | "fixture-paused-auth"
                    | "fixture-refused-secondary"
                    | "fixture-streamlocal"
            ) && key == &self.config.key
            {
                if user == "fixture-paused-auth"
                    && self
                        .config
                        .pool_auth_attempts
                        .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
                        > 0
                {
                    let mut paused = self.config.paused_auth.subscribe();
                    while *paused.borrow_and_update() {
                        paused.changed().await?;
                    }
                }
                // The runtime-admission test deliberately exceeds SSH's default
                // to exercise Needle's independent worker bound.
                self.max_sessions = match user {
                    "fixture-many-sessions" => 32,
                    "fixture-one-session" | "fixture-paused-auth" | "fixture-streamlocal" => 1,
                    "fixture-no-sessions" => 0,
                    "fixture-refused-secondary" => usize::from(
                        self.config
                            .refused_auth_attempts
                            .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
                            == 0,
                    ),
                    _ => 10,
                };
                self.streamlocal = user == "fixture-streamlocal";
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
        if self
            .config
            .disconnect_on_session
            .lock()
            .unwrap()
            .remove(&self.connection_id)
        {
            anyhow::bail!("Fixture disconnected during channel admission");
        }
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
    async fn channel_open_direct_streamlocal(
        &mut self,
        channel: Channel<Msg>,
        path: &str,
        reply: ChannelOpenHandle,
        _: &mut Session,
    ) -> anyhow::Result<()> {
        let path = std::path::Path::new(path);
        if !self.streamlocal
            || path.file_name().is_none_or(|name| name != "streams.sock")
            || !fixture_worker_socket(
                path.with_file_name("workers.sock").to_str().unwrap_or(""),
                &self.config.runtime,
            )
        {
            reply
                .reject(ChannelOpenFailure::AdministrativelyProhibited)
                .await;
            return Ok(());
        }
        match tokio::net::UnixStream::connect(path).await {
            Ok(mut socket) => {
                reply.accept().await;
                self.tasks.push(tokio::spawn(async move {
                    let _ = tokio::io::copy_bidirectional(&mut channel.into_stream(), &mut socket)
                        .await;
                }));
            }
            Err(_) => reply.reject(ChannelOpenFailure::ConnectFailed).await,
        }
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
        } else if let Some(port) = [self.config.http_port, self.config.ssh_port]
            .into_iter()
            .find(|&target| host == "127.0.0.1" && port == u32::from(target))
        {
            let mut socket = tokio::net::TcpStream::connect(("127.0.0.1", port)).await?;
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
            let handler = sftp::Sftp::new(
                self.config.home.clone(),
                SessionSlot(self.sessions.clone(), id),
            );
            // `run` spawns the serving task and returns immediately. Keep the
            // session slot in its handler until that actual stream ends.
            russh_sftp::server::run(stream, handler).await;
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
        let task = pty.start(
            self.channels.remove(&id).unwrap(),
            session.handle(),
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
        let script = posix_script(command);
        let command = script.as_ref().map_or(command, |script| script.as_bytes());
        if let Some((selector, terminal)) = terminal_identity(command)
            && let Some(pty) = self.ptys.get_mut(&id)
        {
            let task = pty.start(
                self.channels.remove(&id).unwrap(),
                session.handle(),
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
        let connection_id = self.connection_id;
        self.session_task(
            id,
            tokio::spawn(async move {
                if let Err(error) = execute(channel, &command, config, connection_id).await {
                    eprintln!("Fixture exec: {error}");
                }
            }),
        );
        Ok(())
    }
}
// Decode the transport envelope without executing a shell. The script still
// passes through the fixture's bounded command routes.
fn posix_script(command: &[u8]) -> Option<String> {
    // A terminal already owns its canonical shell script; preserve it for PTY routing.
    if terminal_identity(command).is_some() {
        return None;
    }
    let command = std::str::from_utf8(command).ok()?;
    let words = shlex::split(command)?;
    let [exec, shell, option, script] = words.as_slice() else {
        return None;
    };
    if exec != "exec" || shell != "sh" || option != "-c" {
        return None;
    }
    let expected = format!("exec sh -c '{}'", script.replace('\'', "'\\''"));
    (command == expected).then(|| script.clone())
}

/// Recognize the complete terminal command, including its optional tab identity.
fn terminal_identity(
    command: &[u8],
) -> Option<(uuid::Uuid, Option<newport_backend::ssh::TerminalIdentity>)> {
    let command = std::str::from_utf8(command).ok()?;
    let selector =
        uuid::Uuid::parse_str(command.split_once("NEWPORT_SESSION=")?.1.get(..36)?).ok()?;
    let terminal = if let Some((_, value)) = command.split_once("NEWPORT_TERMINAL=") {
        // The UUID is single-quoted inside the outer `sh -c` argument.
        let terminal = uuid::Uuid::parse_str(value.strip_prefix("'\\''")?.get(..36)?).ok()?;
        let epoch = command.split_once("NEWPORT_TERMINAL_EPOCH=")?.1;
        let epoch = uuid::Uuid::parse_str(epoch.strip_prefix("'\\''")?.get(..36)?).ok()?;
        Some(newport_backend::ssh::TerminalIdentity { terminal, epoch })
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

fn worker_arguments(command: &[u8], runtime: &std::path::Path) -> Option<Vec<String>> {
    let command = std::str::from_utf8(command).ok()?;
    let (build, arguments) = command
        .strip_prefix("exec \"$HOME/.local/share/newport/needle/builds/")?
        .split_once("/newport-agent\" worker ")?;
    let args = shlex::split(arguments)?;
    (args.len() >= 5
        && newport_backend::agent_runtime::protocol::valid_build(build)
        && args[0] == build
        && uuid::Uuid::parse_str(&args[1]).is_ok()
        && fixture_worker_socket(&args[2], runtime)
        && uuid::Uuid::parse_str(&args[3]).is_ok()
        && (args.len() == 5 && matches!(args[4].as_str(), "git" | "monitor")
            || args.len() == 6
                && args[4] == "integration"
                && uuid::Uuid::parse_str(&args[5]).is_ok()))
    .then_some(args)
}

async fn execute(
    channel: Channel<Msg>,
    command: &[u8],
    config: Config,
    connection_id: uuid::Uuid,
) -> anyhow::Result<()> {
    let worker_args = worker_arguments(command, &config.runtime);
    // Exercise the production managed service launcher with the locally built
    // agent. These are exact fixture routes, not arbitrary shell execution.
    // The runtime starts the desktop's own staged build, which the fixture
    // never has, or the entry point.
    let command = if command.starts_with(b"case \"$(uname -m)\" in ")
        && command.ends_with(b"; exec \"$HOME/.local/bin/newport-agent\" runtime --stdio")
    {
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
        command if command.starts_with(b"drop-next-session ") => {
            let id = uuid::Uuid::parse_str(std::str::from_utf8(&command[18..])?)?;
            config.disconnect_on_session.lock().unwrap().insert(id);
            0
        }
        b"printf ok" => {
            write.data(&b"ok"[..]).await?;
            0
        }
        b"fixture-connection-id" => {
            write.data(connection_id.to_string().as_bytes()).await?;
            0
        }
        b"pool-auth-attempts" => {
            write
                .data(
                    config
                        .pool_auth_attempts
                        .load(std::sync::atomic::Ordering::SeqCst)
                        .to_string()
                        .as_bytes(),
                )
                .await?;
            0
        }
        b"resume-pool-auth" => {
            config.paused_auth.send_replace(false);
            0
        }
        b"refused-auth-attempts" => {
            write
                .data(
                    config
                        .refused_auth_attempts
                        .load(std::sync::atomic::Ordering::SeqCst)
                        .to_string()
                        .as_bytes(),
                )
                .await?;
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
        ssh_port: port,
        home,
        runtime,
        agent: PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../tools/agent/target/debug/newport-agent"),
        key,
        sessions: Arc::default(),
        disconnect_on_session: Arc::default(),
        paused_auth: tokio::sync::watch::channel(true).0,
        pool_auth_attempts: Arc::default(),
        refused_auth_attempts: Arc::default(),
    });
    std::fs::write(directory.join("port"), port.to_string())?;
    server.run_on_socket(config, &listener).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{posix_script, terminal_identity, worker_arguments};
    use std::path::Path;

    fn command(runtime: &Path, service: &str) -> String {
        let build = "a".repeat(64);
        let runtime_id = uuid::Uuid::new_v4();
        let worker = uuid::Uuid::new_v4();
        let socket = runtime.join("newport-ipc/r-fixture/workers.sock");
        let quote = |value: &str| format!("'{}'", value.replace('\'', "'\"'\"'"));
        format!(
            "exec \"$HOME/.local/share/newport/needle/builds/{build}/newport-agent\" worker {build} {runtime_id} {} {worker} {service}",
            quote(socket.to_str().unwrap())
        )
    }

    fn posix(script: &str) -> String {
        format!("exec sh -c '{}'", script.replace('\'', "'\\''"))
    }

    #[test]
    fn posix_exec_envelope_preserves_the_literal_script() {
        let script = "printf '%s\\n' \"世界\"; printf '\\n'";
        assert_eq!(
            posix_script(posix(script).as_bytes()).as_deref(),
            Some(script)
        );
        assert_eq!(
            posix_script(posix("fixture-agent").as_bytes()).as_deref(),
            Some("fixture-agent")
        );
        let runtime = Path::new("/tmp/fixture's runtime");
        let client = uuid::Uuid::new_v4();
        let script = command(runtime, &format!("integration '{client}'"));
        let decoded = posix_script(posix(&script).as_bytes()).unwrap();
        assert_eq!(
            worker_arguments(decoded.as_bytes(), runtime).unwrap()[5],
            client.to_string()
        );
    }

    #[test]
    fn terminal_scripts_keep_their_selector_and_optional_tab_identity() {
        for terminal in [
            None,
            Some(newport_backend::ssh::TerminalIdentity {
                terminal: uuid::Uuid::new_v4(),
                epoch: uuid::Uuid::new_v4(),
            }),
        ] {
            let selector = uuid::Uuid::new_v4();
            let command = newport_backend::ssh::terminal_command(selector, terminal);
            assert!(posix_script(command.as_bytes()).is_none());
            assert_eq!(
                terminal_identity(command.as_bytes()),
                Some((selector, terminal))
            );
        }
    }

    #[test]
    fn only_the_exact_transport_envelope_is_decoded() {
        for script in [
            "fixture-agent",
            "exec sh -c 'fixture-agent' extra",
            "exec sh -c 'fixture-agent'; echo extra",
            "exec sh -c 'fixture-agent' # extra",
            "exec sh -c 'fixture-agent",
            "exec bash -c 'fixture-agent'",
            "exec sh -lc 'fixture-agent'",
            "exec sh -c $SCRIPT",
        ] {
            assert!(posix_script(script.as_bytes()).is_none(), "{script}");
        }
    }

    #[test]
    fn integration_worker_decodes_shell_quoted_literal_arguments() {
        let runtime = Path::new("/tmp/fixture's runtime");
        let client = uuid::Uuid::new_v4().to_string();
        let command = command(runtime, &format!("integration '{client}'"));
        let args = worker_arguments(command.as_bytes(), runtime).unwrap();
        assert_eq!(args.len(), 6);
        assert_eq!(
            args[2],
            "/tmp/fixture's runtime/newport-ipc/r-fixture/workers.sock"
        );
        assert_eq!(args[4], "integration");
        assert_eq!(args[5], client);
    }

    #[test]
    fn argument_free_workers_keep_their_registered_service() {
        let runtime = Path::new("/tmp/runtime");
        for service in ["git", "monitor"] {
            let command = command(runtime, service);
            let args = worker_arguments(command.as_bytes(), runtime).unwrap();
            assert_eq!(args.len(), 5);
            assert_eq!(args[4], service);
        }
    }

    #[test]
    fn malformed_or_unregistered_worker_commands_are_refused() {
        let runtime = Path::new("/tmp/runtime");
        let valid = command(runtime, "monitor");
        for command in [
            format!("{valid} extra"),
            format!("{valid}; echo unexpected"),
            format!("{valid} '"),
            valid.replacen(&"a".repeat(64), &"b".repeat(64), 1),
            valid.replace("/r-fixture/workers.sock", "/r-fixture/other.sock"),
            valid.replace("/tmp/runtime", "/tmp/foreign"),
            self::command(runtime, "integration 'not-a-uuid'"),
            self::command(runtime, "lsp"),
            String::from("exec unrelated"),
            valid.split(" worker ").next().unwrap().to_owned() + " worker",
            String::new(),
        ] {
            assert!(
                worker_arguments(command.as_bytes(), runtime).is_none(),
                "{command}"
            );
        }
    }
}
