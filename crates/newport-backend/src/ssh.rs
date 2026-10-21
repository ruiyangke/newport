//! SSH transport, forwarding and interactive shell lifecycle.
mod auth;

/// Single-quote a word for a POSIX shell on the server.
pub(crate) fn shell_quote(word: &str) -> String {
    format!("'{}'", word.replace('\'', "'\\''"))
}
pub mod callback;
mod exec;
mod health;
mod relay;
mod reverse;
pub mod sftp;
pub(crate) use exec::ConnectFailure;
#[cfg(test)]
use exec::execute_result;
pub use exec::{ExecSession, execute};

use crate::{
    known_hosts,
    model::{Direction, Server, Tunnel},
};
use anyhow::{Context, Result, bail};
use russh::{ChannelMsg, Disconnect, client, keys::PublicKeyOrCertificate};
use std::{
    net::Shutdown,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{
    net::{TcpListener, TcpStream},
    task::JoinSet,
};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(20);
const MAX_OUTPUT: usize = 4 * 1024 * 1024;
// Closing this duplicate shuts down the underlying socket even if a handshake,
// authentication, or channel-open future is cancelled before returning a handle.
struct Transport(std::net::TcpStream);
impl Drop for Transport {
    fn drop(&mut self) {
        let _ = self.0.shutdown(Shutdown::Both);
    }
}
struct Handler {
    host: String,
    port: u16,
    error: Arc<Mutex<Option<String>>>,
    disconnected: Arc<tokio::sync::Notify>,
    forwards: Arc<reverse::RemoteForwards>,
}
impl client::Handler for Handler {
    type Error = anyhow::Error;
    async fn check_server_key(&mut self, key: &PublicKeyOrCertificate) -> Result<bool> {
        if key.certificate().is_some() {
            bail!("SSH host certificates are not supported; configure a pinned host key.");
        }
        let host = self.host.clone();
        let port = self.port;
        let key = key.public_key();
        tokio::task::spawn_blocking(move || {
            let path = known_hosts::user_path()?;
            #[cfg(any(test, feature = "test-support"))]
            let system: Option<std::path::PathBuf> = None;
            #[cfg(not(any(test, feature = "test-support")))]
            let system = newport_platform::ssh_agent::system_known_hosts();
            known_hosts::verify(&host, port, &key, &path, system.as_deref())
        })
        .await??;
        Ok(true)
    }
    async fn disconnected(&mut self, reason: client::DisconnectReason<Self::Error>) -> Result<()> {
        *self.error.lock().unwrap() = Some(format!("SSH connection closed: {reason:?}"));
        self.disconnected.notify_one();
        Ok(())
    }
    async fn server_channel_open_forwarded_tcpip(
        &mut self,
        channel: russh::Channel<client::Msg>,
        _connected_address: &str,
        connected_port: u32,
        _originator_address: &str,
        _originator_port: u32,
        reply: client::ChannelOpenHandle,
        _session: &mut client::Session,
    ) -> Result<()> {
        self.forwards.accept(channel, connected_port, reply);
        Ok(())
    }
}
struct Connection {
    handle: client::Handle<Handler>,
    transport: Transport,
    error: Arc<Mutex<Option<String>>>,
    disconnected: Arc<tokio::sync::Notify>,
    forwards: Arc<reverse::RemoteForwards>,
    channel_cleanup: Mutex<JoinSet<()>>,
    session_open: tokio::sync::Mutex<()>,
    channel_pressure: tokio::sync::watch::Sender<u64>,
    sftp: tokio::sync::Mutex<std::sync::Weak<sftp::Session>>,
}
impl Drop for Connection {
    fn drop(&mut self) {
        let _ = self.transport.0.shutdown(Shutdown::Both);
    }
}
pub fn is_transient_connection_error(error: &anyhow::Error) -> bool {
    if error.is::<auth::AgentApprovalRequired>() {
        return false;
    }
    if error.is::<auth::SelectedAgentUnavailable>() {
        return true;
    }
    error.chain().any(|cause| {
        // russh's transparent IO wrapper omits the inner error from source(),
        // so anyhow's chain alone cannot expose its ErrorKind.
        let io = cause.downcast_ref::<std::io::Error>().or_else(|| {
            match cause.downcast_ref::<russh::Error>() {
                Some(russh::Error::IO(error)) => Some(error),
                _ => None,
            }
        });
        if let Some(error) = io {
            return matches!(
                error.kind(),
                std::io::ErrorKind::ConnectionRefused
                    | std::io::ErrorKind::ConnectionReset
                    | std::io::ErrorKind::ConnectionAborted
                    | std::io::ErrorKind::NotConnected
                    | std::io::ErrorKind::BrokenPipe
                    | std::io::ErrorKind::TimedOut
                    | std::io::ErrorKind::UnexpectedEof
            );
        }
        cause.is::<tokio::time::error::Elapsed>()
            || matches!(
                cause.downcast_ref::<russh::Error>(),
                Some(
                    russh::Error::Disconnect
                        | russh::Error::HUP
                        | russh::Error::ConnectionTimeout
                        | russh::Error::KeepaliveTimeout
                        | russh::Error::InactivityTimeout
                        | russh::Error::SendError
                )
            )
            || cause.is::<russh::SendError>()
    }) || error
        .chain()
        .any(|cause| cause.to_string() == "Cannot connect to SSH server")
}

async fn retry_connection<F, Fut, T>(mut attempt: F, delay: Duration) -> Result<T>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<T>>,
{
    for retry in 0..3 {
        match attempt().await {
            Ok(connection) => return Ok(connection),
            Err(error) if retry < 2 && is_transient_connection_error(&error) => {
                log::warn!(
                    "SSH connection interrupted; retrying setup ({}/3)",
                    retry + 2
                );
                tokio::time::sleep(delay * (1 << retry)).await;
            }
            Err(error) => return Err(error),
        }
    }
    unreachable!()
}

#[derive(Clone, Copy)]
enum ConnectionPolicy {
    Request,
    Integration,
}

impl ConnectionPolicy {
    async fn establish<T, F, Fut>(self, mut attempt: F) -> Result<T>
    where
        F: FnMut() -> Fut,
        Fut: std::future::Future<Output = Result<T>>,
    {
        match self {
            // Each attempt bounds the handshake and network authentication
            // phases. An outer deadline would also cancel local agent consent.
            Self::Request => retry_connection(attempt, Duration::from_secs(2)).await,
            // Integration recovery owns backoff and cancellation.
            Self::Integration => attempt().await,
        }
    }
}

impl Connection {
    async fn connect(server: &Server) -> Result<Self> {
        Self::connect_with_policy(server, ConnectionPolicy::Request).await
    }
    async fn connect_with_policy(server: &Server, policy: ConnectionPolicy) -> Result<Self> {
        server.validate().map_err(anyhow::Error::msg)?;
        let started = std::time::Instant::now();
        let result = policy.establish(|| Self::connect_once(server)).await;
        crate::connectivity::report(
            server,
            started,
            result.as_ref().map(|_| ()).map_err(|e| format!("{e:#}")),
        );
        result
    }
    async fn connect_once(server: &Server) -> Result<Self> {
        let mut connection = tokio::time::timeout(CONNECT_TIMEOUT, Self::handshake(server))
            .await
            .context("SSH socket connection or handshake timed out")??;
        let (approval, waiting) = tokio::sync::watch::channel(false);
        auth::with_network_deadline(
            auth::authenticate(&mut connection.handle, server, approval),
            waiting,
            &connection.disconnected,
        )
        .await?;
        Ok(connection)
    }
    async fn handshake(server: &Server) -> Result<Self> {
        let socket = TcpStream::connect((server.ssh_host.as_str(), server.ssh_port))
            .await
            .context("Cannot connect to SSH server")?;
        socket.set_nodelay(true)?;
        let socket = socket.into_std()?;
        let transport = Transport(socket.try_clone()?);
        let error = Arc::new(Mutex::new(None));
        let disconnected = Arc::new(tokio::sync::Notify::new());
        let forwards = Arc::new(reverse::RemoteForwards::default());
        let handler = Handler {
            host: server.ssh_host.clone(),
            port: server.ssh_port,
            error: error.clone(),
            disconnected: disconnected.clone(),
            forwards: forwards.clone(),
        };
        let config = Arc::new(client::Config {
            keepalive_interval: Some(Duration::from_secs(15)),
            keepalive_max: 2,
            nodelay: true,
            ..Default::default()
        });
        let handle = client::connect_stream(config, TcpStream::from_std(socket)?, handler)
            .await
            .context("SSH handshake or host-key verification failed")?;
        Ok::<_, anyhow::Error>(Self {
            handle,
            transport,
            error,
            disconnected,
            forwards,
            channel_cleanup: Mutex::new(JoinSet::new()),
            session_open: tokio::sync::Mutex::new(()),
            channel_pressure: tokio::sync::watch::channel(0).0,
            sftp: tokio::sync::Mutex::default(),
        })
    }
    async fn close(&self) {
        let _ = tokio::time::timeout(
            Duration::from_secs(1),
            self.handle
                .disconnect(Disconnect::ByApplication, "Newport disconnected", "en"),
        )
        .await;
        let _ = self.transport.0.shutdown(Shutdown::Both);
    }
}
async fn terminal_request_reply(
    channel: &mut russh::ChannelReadHalf,
    request: &str,
    early_output: &mut Vec<u8>,
) -> Result<()> {
    loop {
        match channel.wait().await {
            Some(ChannelMsg::Success) => return Ok(()),
            Some(ChannelMsg::Failure) => bail!("Server refused {request}."),
            Some(ChannelMsg::Data { data } | ChannelMsg::ExtendedData { data, .. }) => {
                if early_output.len() + data.len() > MAX_OUTPUT {
                    bail!("Terminal setup output exceeded 4 MiB");
                }
                early_output.extend_from_slice(&data);
            }
            Some(ChannelMsg::Eof | ChannelMsg::Close) | None => {
                bail!("SSH channel closed while requesting {request}.");
            }
            Some(ChannelMsg::ExitStatus { exit_status }) => {
                bail!("Remote shell exited during setup (status {exit_status}).");
            }
            Some(ChannelMsg::ExitSignal { signal_name, .. }) => {
                bail!("Remote shell ended during setup: {signal_name:?}");
            }
            // Flow-control messages can arrive before the request acknowledgement.
            _ => {}
        }
    }
}

#[derive(Debug)]
struct ConnectionLost;
impl std::fmt::Display for ConnectionLost {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SSH connection lost.")
    }
}
impl std::error::Error for ConnectionLost {}

pub(crate) enum TerminalEnd {
    Exited(Option<u32>),
    Closed,
}

/// Discard keystrokes for an unavailable shell, but preserve window size.
async fn discard_input(
    input: &mut tokio::sync::mpsc::Receiver<crate::terminal::Input>,
    size: &mut (u32, u32),
) {
    while let Some(command) = input.recv().await {
        if let crate::terminal::Input::Resize(cols, rows) = command {
            *size = (cols, rows);
        }
    }
}

#[cfg(test)]
pub async fn terminal(
    server: &Server,
    cols: u32,
    rows: u32,
    input: tokio::sync::mpsc::Receiver<crate::terminal::Input>,
    output: &tokio::sync::mpsc::Sender<crate::terminal::Event>,
) -> Result<()> {
    let registry = crate::server_session::Registry::default();
    let owner = registry.session(server, 0);
    terminal_with_session(owner, server, cols, rows, input, output).await
}

pub(crate) async fn terminal_with_session(
    owner: Arc<crate::server_session::ServerSession>,
    server: &Server,
    cols: u32,
    rows: u32,
    mut input: tokio::sync::mpsc::Receiver<crate::terminal::Input>,
    output: &tokio::sync::mpsc::Sender<crate::terminal::Event>,
) -> Result<()> {
    use crate::terminal::Event;
    if !owner.server().same_connection(server) || owner.is_revoked() {
        bail!("Server session changed. Reconnect before retrying.");
    }
    // The terminal task owns this supervisor. Exiting or cancelling the task
    // closes only this tab's socket, including a pending authentication attempt.
    let transport = owner.terminal_transport();
    let _demand = transport.demand();
    let mut cancelled = owner.cancellation();
    let run = async {
        let selector = owner.integration_session_id().to_string();
        let mut size = (cols, rows);
        loop {
            let acquired = tokio::select! {
                result = transport.acquire() => result,
                _ = discard_input(&mut input, &mut size) => return Ok(()),
            };
            let (result, lost) = match acquired {
                Ok((connection, _, _lease)) => {
                    let result = connection
                        .terminal(&mut size, &mut input, output, &selector)
                        .await;
                    let lost = connection.is_closed()
                        || result.as_ref().is_err_and(|e| e.is::<ConnectionLost>());
                    (result, lost)
                }
                Err(error) => (Err(anyhow::Error::msg(error)), false),
            };
            match result {
                Ok(TerminalEnd::Exited(exit)) => {
                    output.send(Event::Exit(exit)).await?;
                    return Ok(());
                }
                Ok(TerminalEnd::Closed) => return Ok(()),
                Err(error) => {
                    let snapshot = transport.snapshot();
                    // A refused PTY or shell is a service failure. Reconnecting
                    // a healthy transport cannot repair it.
                    if (!lost && (transport.observe().is_some() || !snapshot.retryable))
                        || (!snapshot.retryable && snapshot.error.is_some())
                    {
                        return Err(error);
                    }
                    output
                        .send(Event::Reconnecting {
                            attempt: snapshot.attempt.max(1),
                            message: format!("{error:#}"),
                        })
                        .await?;
                    tokio::select! {
                        result = transport.ready() => result.map_err(anyhow::Error::msg)?,
                        _ = discard_input(&mut input, &mut size) => return Ok(()),
                    }
                }
            }
        }
    };
    tokio::select! {
        biased;
        _ = async { if !*cancelled.borrow() { let _ = cancelled.changed().await; } } => {
            bail!("Server session changed. Reconnect before retrying.")
        },
        result = run => result,
    }
}

/// Cancelling a channel closes that channel. The transport's owner decides
/// whether the underlying connection should also close.
struct SessionChannel<'a> {
    writer: Option<russh::ChannelWriteHalf<client::Msg>>,
    connection: &'a Connection,
}
impl std::ops::Deref for SessionChannel<'_> {
    type Target = russh::ChannelWriteHalf<client::Msg>;
    fn deref(&self) -> &Self::Target {
        self.writer.as_ref().unwrap()
    }
}
impl SessionChannel<'_> {
    /// Close in place; the drop cleanup then has nothing left to close.
    async fn close(mut self) -> Result<(), russh::Error> {
        let writer = self.writer.take().unwrap();
        writer.close().await
    }
}
impl Drop for SessionChannel<'_> {
    fn drop(&mut self) {
        let Some(writer) = self.writer.take() else {
            return;
        };
        let mut cleanup = self.connection.channel_cleanup.lock().unwrap();
        while cleanup.try_join_next().is_some() {}
        cleanup.spawn(async move {
            let _ = tokio::time::timeout(Duration::from_secs(3), writer.close()).await;
        });
    }
}
async fn terminal_session(
    connection: &Arc<Connection>,
    size: &mut (u32, u32),
    input: &mut tokio::sync::mpsc::Receiver<crate::terminal::Input>,
    output: &tokio::sync::mpsc::Sender<crate::terminal::Event>,
    integration: Option<&str>,
) -> Result<TerminalEnd> {
    use crate::terminal::{Event, Input};
    let (mut reader, writer) =
        tokio::time::timeout(Duration::from_secs(10), connection.open_session())
            .await
            .context("Terminal channel timed out")??
            .split();
    let writer = SessionChannel {
        writer: Some(writer),
        connection,
    };
    let mut early_output = Vec::new();
    tokio::time::timeout(Duration::from_secs(10), async {
        writer
            .request_pty(true, "xterm-256color", size.0, size.1, 0, 0, &[])
            .await?;
        terminal_request_reply(&mut reader, "the terminal PTY", &mut early_output).await?;
        if let Some(selector) = integration {
            // An exec request avoids relying on sshd AcceptEnv configuration.
            // The UUID comes from the session owner, never shell input.
            writer
                .exec(
                    true,
                    format!("exec env NEWPORT_SESSION='{selector}' \"$SHELL\" -l"),
                )
                .await?;
        } else {
            writer.request_shell(true).await?;
        }
        terminal_request_reply(&mut reader, "an interactive shell", &mut early_output).await?;
        Ok::<_, anyhow::Error>(())
    })
    .await
    .context("Terminal setup timed out")??;
    // Only input for this shell is forwarded; earlier keystrokes are discarded.
    while let Ok(command) = input.try_recv() {
        if let Input::Resize(cols, rows) = command {
            *size = (cols, rows);
        }
    }
    writer.window_change(size.0, size.1, 0, 0).await?;
    output.send(Event::Ready).await?;
    for bytes in early_output.chunks(32768) {
        output.send(Event::Data(bytes.to_vec())).await?;
    }
    let receive = async {
        let mut exit = None;
        while let Some(message) = reader.wait().await {
            match message {
                ChannelMsg::Data { data } | ChannelMsg::ExtendedData { data, .. } => {
                    for bytes in data.chunks(32768) {
                        output.send(Event::Data(bytes.to_vec())).await?;
                    }
                }
                ChannelMsg::ExitStatus { exit_status } => exit = Some(exit_status),
                ChannelMsg::ExitSignal { signal_name, .. } => {
                    bail!("Remote shell ended: {signal_name:?}")
                }
                ChannelMsg::Close => break,
                _ => {}
            }
        }
        Ok::<_, anyhow::Error>(exit)
    };
    let send = async {
        while let Some(command) = input.recv().await {
            match command {
                Input::Data(bytes) => {
                    tokio::time::timeout(Duration::from_secs(10), writer.data(bytes.as_slice()))
                        .await
                        .context("Terminal input timed out")??;
                }
                Input::Resize(cols, rows) => {
                    *size = (cols, rows);
                    writer.window_change(cols, rows, 0, 0).await?;
                }
            }
        }
        Ok::<_, anyhow::Error>(())
    };
    enum Completion {
        Receive(Result<Option<u32>>),
        Send(Result<()>),
    }
    // Read and write independently, so echoed input cannot deadlock a large paste.
    let completion = tokio::select! {
        result = receive => Completion::Receive(result),
        result = send => Completion::Send(result),
    };
    let mut closed = connection.handle.is_closed();
    // A disconnect closes channels just before the session task closes its
    // handle; give the transport a moment to confirm the loss.
    if !closed
        && matches!(
            &completion,
            Completion::Receive(Ok(None)) | Completion::Send(Err(_))
        )
    {
        closed = tokio::time::timeout(
            Duration::from_millis(500),
            connection.disconnected.notified(),
        )
        .await
        .is_ok()
            || connection.handle.is_closed();
    }
    match completion {
        // An observed exit status is authoritative, even if the transport
        // closed right after it. A closed transport without one is a loss.
        Completion::Receive(Ok(Some(exit))) => Ok(TerminalEnd::Exited(Some(exit))),
        Completion::Receive(Ok(None)) if !closed => Ok(TerminalEnd::Exited(None)),
        Completion::Send(Ok(())) => Ok(TerminalEnd::Closed),
        Completion::Receive(Err(error)) | Completion::Send(Err(error)) if !closed => Err(error),
        Completion::Receive(_) | Completion::Send(_) => Err(ConnectionLost.into()),
    }
}

pub struct Forwarding {
    connection: Arc<Connection>,
    tasks: JoinSet<()>,
    failure: Arc<Mutex<Option<String>>>,
    health: health::Health,
    /// Server listeners opened by a remote forward, released on shutdown.
    exposed: Vec<(String, u32)>,
}
impl Forwarding {
    pub async fn start(server: &Server, tunnel: Option<&Tunnel>) -> Result<Self> {
        // Acquire the whole range before connecting; a partial bind rolls back by drop.
        let mut listeners = Vec::new();
        if let Some(t) = tunnel.filter(|t| t.direction == Direction::Local) {
            for (local, remote) in t.pairs().map_err(anyhow::Error::msg)? {
                let listener = TcpListener::bind(("127.0.0.1", local))
                    .await
                    .with_context(|| format!("Cannot bind local port {local}"))?;
                listeners.push((listener, t.remote_host.clone(), remote));
            }
        }
        let connection = Arc::new(Connection::connect(server).await?);
        let failure = Arc::new(Mutex::new(None));
        let mut tasks = JoinSet::new();
        let health: health::Health = Arc::new(Mutex::new(Vec::new()));
        let slots = Arc::new(tokio::sync::Semaphore::new(4));
        for (listener, host, remote) in listeners {
            health
                .lock()
                .unwrap()
                .push(crate::model::DestinationHealth {
                    local_port: listener.local_addr()?.port(),
                    remote_port: remote,
                    status: crate::model::DestinationStatus::Checking,
                    message: None,
                    checked_at: None,
                });
            tasks.spawn(health::monitor(
                connection.clone(),
                host.clone(),
                remote,
                health.clone(),
                slots.clone(),
            ));
            let conn = connection.clone();
            let failure = failure.clone();
            tasks.spawn(async move {
                if let Err(error) =
                    relay::serve(listener, conn, host, remote, relay::Policy::TUNNEL).await
                {
                    *failure.lock().unwrap() = Some(format!("Local listener failed: {error}"));
                }
            });
        }
        let mut exposed = Vec::new();
        if let Some(t) = tunnel.filter(|t| t.direction == Direction::Remote) {
            for (local, remote) in t.pairs().map_err(anyhow::Error::msg)? {
                // Register first: the server may open a channel before replying.
                connection.forwards.expose(u32::from(remote), local);
                tokio::time::timeout(
                    Duration::from_secs(10),
                    connection
                        .handle
                        .tcpip_forward(t.remote_host.clone(), u32::from(remote)),
                )
                .await
                .map_err(anyhow::Error::from)
                .and_then(|result| result.map_err(anyhow::Error::from))
                .with_context(|| format!("Cannot open port {remote} on the server"))?;
                exposed.push((t.remote_host.clone(), u32::from(remote)));
                health
                    .lock()
                    .unwrap()
                    .push(crate::model::DestinationHealth {
                        local_port: local,
                        remote_port: remote,
                        status: crate::model::DestinationStatus::Checking,
                        message: None,
                        checked_at: None,
                    });
                tasks.spawn(health::monitor_local(local, remote, health.clone()));
            }
        }
        Ok(Self {
            connection,
            tasks,
            failure,
            health,
            exposed,
        })
    }
    pub fn destination_health(&self) -> Vec<crate::model::DestinationHealth> {
        self.health.lock().unwrap().clone()
    }
    pub fn error(&self) -> Option<String> {
        self.failure.lock().unwrap().clone().or_else(|| {
            if self.connection.handle.is_closed() {
                Some(
                    self.connection
                        .error
                        .lock()
                        .unwrap()
                        .clone()
                        .unwrap_or("SSH connection lost".into()),
                )
            } else {
                None
            }
        })
    }
    pub async fn shutdown(mut self) {
        self.connection.forwards.stop();
        for (host, port) in std::mem::take(&mut self.exposed) {
            let cancel = self.connection.handle.cancel_tcpip_forward(host, port);
            let _ = tokio::time::timeout(Duration::from_secs(1), cancel).await;
        }
        self.tasks.shutdown().await;
        self.connection.close().await;
    }
}
impl Drop for Forwarding {
    fn drop(&mut self) {
        self.tasks.abort_all();
        self.connection.forwards.stop();
        let _ = self.connection.transport.0.shutdown(Shutdown::Both);
    }
}

#[cfg(test)]
mod integration_tests {
    use super::*;
    use crate::model::AuthMethod;
    use tokio::io::AsyncReadExt;
    use uuid::Uuid;
    pub(super) fn server() -> Server {
        Server {
            id: Uuid::new_v4(),
            name: "Fixture".into(),
            ssh_user: "fixture".into(),
            ssh_host: "127.0.0.1".into(),
            ssh_port: std::env::var("NEWPORT_TEST_SSH_PORT")
                .unwrap()
                .parse()
                .unwrap(),
            identity_file: Some(std::env::var("NEWPORT_TEST_IDENTITY").unwrap()),
            agent_source: None,
            agent_key_fingerprint: None,
            auth_method: AuthMethod::PublicKey,
            clipboard_enabled: false,
            browser_enabled: false,
            agent_install_approved: true,
            setup_complete: true,
        }
    }
    #[tokio::test]
    #[ignore = "Run with cargo xtask test ssh"]
    async fn remote_forward_relays_server_port_to_this_mac() {
        use crate::model::DestinationStatus;
        use tokio::io::AsyncWriteExt;
        let target = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let local = target.local_addr().unwrap().port();
        // Health probes connect here too, so echo every connection.
        let echo = tokio::spawn(async move {
            while let Ok((mut socket, _)) = target.accept().await {
                tokio::spawn(async move {
                    let (mut read, mut write) = socket.split();
                    let _ = tokio::io::copy(&mut read, &mut write).await;
                });
            }
        });
        let reservation = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let exposed = reservation.local_addr().unwrap().port();
        drop(reservation);
        let s = server();
        let t = Tunnel {
            id: Uuid::new_v4(),
            name: "reverse".into(),
            server_id: s.id,
            local_port: local,
            local_port_end: None,
            remote_host: "127.0.0.1".into(),
            remote_port: exposed,
            remote_port_end: None,
            auto_connect: false,
            auto_reconnect: false,
            direction: Direction::Remote,
            project: None,
        };
        let forwarding = Forwarding::start(&s, Some(&t)).await.unwrap();
        let mut client = TcpStream::connect(("127.0.0.1", exposed)).await.unwrap();
        client.write_all(b"ping").await.unwrap();
        let mut reply = [0; 4];
        tokio::time::timeout(Duration::from_secs(5), client.read_exact(&mut reply))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(&reply, b"ping");
        drop(client);
        tokio::time::timeout(Duration::from_secs(5), async {
            while forwarding.destination_health()[0].status == DestinationStatus::Checking {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap();
        let health = forwarding.destination_health();
        assert_eq!(health[0].status, DestinationStatus::Reachable);
        assert_eq!(health[0].remote_port, exposed);
        assert_eq!(health[0].local_port, local);
        assert!(forwarding.error().is_none());
        echo.abort();
        forwarding.shutdown().await;
        tokio::time::timeout(Duration::from_secs(5), async {
            while TcpStream::connect(("127.0.0.1", exposed)).await.is_ok() {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("closing the SSH session releases the server listener");
    }
    #[tokio::test]
    #[ignore = "Run with cargo xtask test ssh"]
    async fn destination_checks_preserve_transport_and_release_ports() {
        use crate::model::DestinationStatus;
        for (remote, expected) in [
            (1, DestinationStatus::Unavailable),
            (2, DestinationStatus::Blocked),
            (3, DestinationStatus::Reachable),
        ] {
            let reservation = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let local = reservation.local_addr().unwrap().port();
            drop(reservation);
            let s = server();
            let t = Tunnel {
                id: Uuid::new_v4(),
                name: "health".into(),
                server_id: s.id,
                local_port: local,
                local_port_end: None,
                remote_host: "health.fixture".into(),
                remote_port: remote,
                remote_port_end: None,
                auto_connect: false,
                auto_reconnect: false,
                direction: Default::default(),
                project: None,
            };
            let forwarding = Forwarding::start(&s, Some(&t)).await.unwrap();
            tokio::time::timeout(Duration::from_secs(5), async {
                loop {
                    let health = forwarding.destination_health();
                    if health[0].status != DestinationStatus::Checking {
                        assert_eq!(health[0].status, expected);
                        assert!(health[0].checked_at.is_some());
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
            })
            .await
            .unwrap();
            assert!(
                forwarding.error().is_none(),
                "Destination failure must not reconnect SSH"
            );
            forwarding.shutdown().await;
            let _released = TcpListener::bind(("127.0.0.1", local)).await.unwrap();
        }
    }
    #[tokio::test]
    #[ignore = "Run with cargo xtask test ssh"]
    async fn embedded_auth_and_persistent_exec() {
        let _owner = crate::credentials::TEST_OWNER.lock().await;
        let mut s = server();
        assert_eq!(
            execute(&s, "stdin", Some(&"a".repeat(512 * 1024)))
                .await
                .unwrap(),
            (512 * 1024).to_string()
        );
        let failed = execute_result(&s, "fail", None).await.unwrap();
        assert_eq!(failed.exit_code, 7);
        assert_eq!(failed.stderr, "fixture failure");
        assert!(
            execute(&s, "fail", None)
                .await
                .unwrap_err()
                .contains("fixture failure")
        );
        assert!(
            execute(&s, "large", None)
                .await
                .unwrap_err()
                .contains("exceeded 4 MiB")
        );
        // This uses the fixture's real SSH-agent Unix socket, not a mocked russh API.
        s.identity_file = Some(format!(
            "{}.encrypted",
            std::env::var("NEWPORT_TEST_IDENTITY").unwrap()
        ));
        assert_eq!(execute(&s, "printf ok", None).await.unwrap(), "ok");
        s.identity_file = None;
        assert_eq!(execute(&s, "printf ok", None).await.unwrap(), "ok");
        let mut agent = crate::agent_keys::connect(Some("system")).await.unwrap();
        let keys = agent.request_identities().await.unwrap();
        let fingerprint = keys[0]
            .public_key()
            .fingerprint(russh::keys::ssh_key::HashAlg::Sha256)
            .to_string();
        s.agent_source = Some("system".into());
        s.agent_key_fingerprint = Some(fingerprint);
        assert_eq!(execute(&s, "printf ok", None).await.unwrap(), "ok");
        s.agent_key_fingerprint = Some(format!("SHA256:{}", "A".repeat(43)));
        assert!(
            execute(&s, "printf ok", None)
                .await
                .unwrap_err()
                .contains("selected SSH agent key")
        );
        s.agent_key_fingerprint = None;
        s.agent_source = None;
        s.auth_method = AuthMethod::Password;
        // Exercise the production credential reader against an encrypted fixture vault.
        let vault_directory = tempfile::tempdir().unwrap();
        let store = Arc::new(crate::profile::Store::for_test(
            vault_directory.path().into(),
        ));
        let config = crate::model::Config {
            servers: vec![s.clone()],
            tunnels: vec![],
        };
        let password = zeroize::Zeroizing::new(std::env::var("NEWPORT_TEST_PASSWORD").unwrap());
        store
            .save_with_password(&config, Some((s.id, &password)))
            .unwrap();
        let _credentials = crate::credentials::register(store).unwrap();
        assert_eq!(execute(&s, "printf ok", None).await.unwrap(), "ok");
        s.ssh_user = "wrong".into();
        assert!(execute(&s, "printf ok", None).await.is_err());
        s = server();
        let session = ExecSession::connect(&s).await.unwrap();
        let bytes: Vec<u8> = (0..=255).cycle().take(5 * 1024 * 1024).collect();
        assert_eq!(
            session.execute("stdin", Some(&bytes)).await.unwrap(),
            bytes.len().to_string()
        );
        for arch in ["x86_64", "aarch64"] {
            let payload = crate::agent_payload::payload(arch).unwrap();
            let progress = std::sync::atomic::AtomicUsize::new(0);
            assert_eq!(
                session
                    .upload_resource("stdin", payload.stream().unwrap(), &|sent| {
                        progress.store(sent, std::sync::atomic::Ordering::Relaxed);
                    })
                    .await
                    .unwrap(),
                payload.size.to_string()
            );
            assert_eq!(
                progress.load(std::sync::atomic::Ordering::Relaxed),
                payload.size
            );
        }
        assert_eq!(session.execute("printf ok", None).await.unwrap(), "ok");
        assert!(
            session
                .execute("fail", None)
                .await
                .unwrap_err()
                .contains("fixture failure")
        );
        session.close().await;
    }
    #[tokio::test]
    async fn cancelling_handshake_releases_transport_and_listener() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let ssh_port = listener.local_addr().unwrap().port();
        let target = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let local = target.local_addr().unwrap().port();
        drop(target);
        let s = Server {
            id: Uuid::new_v4(),
            name: "stall".into(),
            ssh_user: "fixture".into(),
            ssh_host: "127.0.0.1".into(),
            ssh_port,
            identity_file: None,
            agent_source: None,
            agent_key_fingerprint: None,
            auth_method: AuthMethod::PublicKey,
            clipboard_enabled: false,
            browser_enabled: false,
            agent_install_approved: true,
            setup_complete: true,
        };
        let t = Tunnel {
            id: Uuid::new_v4(),
            name: "stall".into(),
            server_id: s.id,
            local_port: local,
            local_port_end: None,
            remote_host: "127.0.0.1".into(),
            remote_port: 80,
            remote_port_end: None,
            auto_connect: false,
            auto_reconnect: false,
            direction: Default::default(),
            project: None,
        };
        let peer = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut bytes = Vec::new();
            stream.read_to_end(&mut bytes).await.unwrap();
        });
        assert!(
            tokio::time::timeout(Duration::from_millis(200), Forwarding::start(&s, Some(&t)))
                .await
                .is_err()
        );
        tokio::time::timeout(Duration::from_secs(2), peer)
            .await
            .expect("Cancelled handshake must close SSH transport")
            .unwrap();

        // The port was free when chosen, so a parallel test may briefly hold
        // it; only a listener that never goes away fails this.
        let released = tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if TcpListener::bind(("127.0.0.1", local)).await.is_ok() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await;
        assert!(
            released.is_ok(),
            "Cancelled handshake must release listener"
        );
    }
}

#[cfg(test)]
mod connection_retry_tests {
    use super::*;

    #[tokio::test]
    async fn agent_approval_eof_requires_user_retry_even_with_transient_io_cause() {
        let mut attempts = 0;
        let result: Result<()> = retry_connection(
            || {
                attempts += 1;
                std::future::ready(Err(anyhow::Error::new(std::io::Error::from(
                    std::io::ErrorKind::UnexpectedEof,
                ))
                .context(auth::AgentApprovalRequired)))
            },
            Duration::ZERO,
        )
        .await;
        assert!(result.is_err());
        assert_eq!(attempts, 1);
    }

    #[test]
    fn wrapped_io_errors_keep_their_retry_classification() {
        for (kind, retryable) in [
            (std::io::ErrorKind::ConnectionReset, true),
            (std::io::ErrorKind::BrokenPipe, true),
            (std::io::ErrorKind::UnexpectedEof, true),
            (std::io::ErrorKind::TimedOut, true),
            (std::io::ErrorKind::PermissionDenied, false),
            (std::io::ErrorKind::InvalidData, false),
        ] {
            let error = anyhow::Error::new(russh::Error::IO(std::io::Error::from(kind)))
                .context("SSH handshake or host-key verification failed");
            assert_eq!(is_transient_connection_error(&error), retryable, "{kind:?}");
        }
    }

    #[tokio::test]
    async fn selected_agent_recovers_and_exhaustion_is_bounded() {
        for available_after in [2, 4] {
            let mut attempts = 0;
            let result = retry_connection(
                || {
                    attempts += 1;
                    std::future::ready(if attempts >= available_after {
                        Ok(())
                    } else {
                        Err(anyhow::Error::new(auth::SelectedAgentUnavailable)
                            .context("SSH authentication"))
                    })
                },
                Duration::ZERO,
            )
            .await;
            assert_eq!(attempts, available_after.min(3));
            assert_eq!(result.is_ok(), available_after <= 3);
        }
    }

    #[tokio::test]
    async fn handshake_reset_recovers_before_any_command_runs() {
        let mut attempts = 0;
        let result = retry_connection(
            || {
                attempts += 1;
                std::future::ready(if attempts == 1 {
                    Err(anyhow::Error::new(russh::Error::IO(std::io::Error::from(
                        std::io::ErrorKind::ConnectionReset,
                    )))
                    .context("SSH handshake or host-key verification failed"))
                } else {
                    Ok("connected")
                })
            },
            Duration::ZERO,
        )
        .await
        .unwrap();
        assert_eq!(result, "connected");
        assert_eq!(attempts, 2);
    }

    #[tokio::test]
    async fn permanent_failures_are_not_retried_and_resets_are_bounded() {
        for (message, expected) in [
            ("Host key changed", 1),
            ("Authentication rejected", 1),
            ("Permission denied", 1),
            ("reset", 3),
        ] {
            let mut attempts = 0;
            let result: Result<()> = retry_connection(
                || {
                    attempts += 1;
                    std::future::ready(Err(if message == "reset" {
                        anyhow::Error::new(std::io::Error::from(
                            std::io::ErrorKind::ConnectionReset,
                        ))
                    } else {
                        anyhow::anyhow!(message)
                    }))
                },
                Duration::ZERO,
            )
            .await;
            assert!(result.is_err());
            assert_eq!(attempts, expected);
        }
    }

    #[tokio::test]
    async fn connection_deadline_cancels_backoff() {
        let mut attempts = 0;
        let result = tokio::time::timeout(
            Duration::from_millis(10),
            retry_connection(
                || {
                    attempts += 1;
                    std::future::ready(Err::<(), _>(anyhow::Error::new(std::io::Error::from(
                        std::io::ErrorKind::ConnectionReset,
                    ))))
                },
                Duration::from_secs(60),
            ),
        )
        .await;
        assert!(result.is_err());
        assert_eq!(attempts, 1);
    }
}
