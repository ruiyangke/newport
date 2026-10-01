use russh::{
    keys,
    server::{self, Auth, ChannelOpenHandle, Msg, Server as _, Session},
    Channel, ChannelId, ChannelOpenFailure,
};
use std::{
    collections::HashMap,
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
    home: PathBuf,
    agent: PathBuf,
    key: keys::PublicKey,
    sessions: Arc<Mutex<Vec<server::Handle>>>,
}
struct Listener(Config);
impl server::Server for Listener {
    type Handler = Client;
    fn new_client(&mut self, _: Option<std::net::SocketAddr>) -> Client {
        Client {
            config: self.0.clone(),
            channels: HashMap::new(),
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
    config: Config,
    channels: HashMap<ChannelId, Channel<Msg>>,
    ptys: HashMap<ChannelId, pty::Pty>,
    tasks: Vec<JoinHandle<()>>,
}
impl Drop for Client {
    fn drop(&mut self) {
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
        Ok(if user == "fixture" && key == &self.config.key {
            Auth::Accept
        } else {
            Auth::reject()
        })
    }
    async fn auth_succeeded(&mut self, session: &mut Session) -> anyhow::Result<()> {
        self.config.sessions.lock().unwrap().push(session.handle());
        Ok(())
    }
    async fn channel_open_session(
        &mut self,
        channel: Channel<Msg>,
        reply: ChannelOpenHandle,
        _: &mut Session,
    ) -> anyhow::Result<()> {
        self.channels.insert(channel.id(), channel);
        reply.accept().await;
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
            self.tasks.push(tokio::spawn(async move {
                russh_sftp::server::run(stream, handler).await;
            }));
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
        self.tasks
            .push(pty.start(self.channels.remove(&id).unwrap(), self.config.home.clone())?);
        Ok(())
    }
    async fn exec_request(
        &mut self,
        id: ChannelId,
        command: &[u8],
        session: &mut Session,
    ) -> anyhow::Result<()> {
        session.channel_success(id)?;
        let channel = self.channels.remove(&id).unwrap();
        let command = command.to_vec();
        let config = self.config.clone();
        self.tasks.push(tokio::spawn(async move {
            if let Err(error) = execute(channel, &command, config).await {
                eprintln!("Fixture exec: {error}");
            }
        }));
        Ok(())
    }
}
async fn execute(channel: Channel<Msg>, command: &[u8], config: Config) -> anyhow::Result<()> {
    let (mut read, write) = channel.split();
    let code = match command {
        b"drop" => {
            write.exit_status(0).await?;
            tokio::time::sleep(Duration::from_millis(100)).await;
            let handles = config.sessions.lock().unwrap().clone();
            for handle in handles {
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
        b"printf ok" => {
            write.data(&b"ok"[..]).await?;
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
        b"fixture-agent" | b"fixture-clipboard" | b"fixture-open" => {
            let args: &[&str] = match command {
                b"fixture-agent" => &["serve", "ssh-fixture", "--clipboard", "--browser"],
                b"fixture-clipboard" => &["clipboard", "-o"],
                _ => &["open", "https://example.com/login?code=fixture"],
            };
            let mut child = tokio::process::Command::new(config.agent)
                .args(args)
                .env("HOME", config.home)
                .env("NEWPORT_CLIPBOARD_NATIVE", "0")
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::null())
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
    std::fs::write(
        home.join("Documents/hello 世界.txt"),
        "Hello over SFTP!\n<script>inert text</script>\n",
    )?;
    std::fs::write(home.join("binary.dat"), (0..=255u8).collect::<Vec<_>>())?;
    std::fs::write(home.join("large.txt"), vec![b'x'; 17 * 1024 * 1024])?;
    let key = keys::load_public_key(directory.join("client.pub"))?;
    let host = keys::load_secret_key(directory.join("host"), None)?;
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
        home,
        agent: PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../tools/agent/target/debug/newport-agent"),
        key,
        sessions: Arc::default(),
    });
    std::fs::write(directory.join("port"), port.to_string())?;
    server.run_on_socket(config, &listener).await?;
    Ok(())
}
