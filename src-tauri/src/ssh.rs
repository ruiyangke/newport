//! SSH transport, forwarding and interactive shell lifecycle.
mod auth;
pub mod callback;
mod exec;
mod health;
mod relay;
pub mod sftp;
#[cfg(test)]
use exec::execute_result;
pub use exec::{execute, ExecSession};

use crate::{
    known_hosts,
    model::{Server, Tunnel},
};
use anyhow::{bail, Context, Result};
use russh::{client, keys::PublicKeyOrCertificate, ChannelMsg, Disconnect};
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
            #[cfg(test)]
            let system = None;
            #[cfg(not(test))]
            let system = crate::platform::ssh_agent::system_known_hosts();
            known_hosts::verify(&host, port, &key, &path, system)
        })
        .await??;
        Ok(true)
    }
    async fn disconnected(&mut self, reason: client::DisconnectReason<Self::Error>) -> Result<()> {
        *self.error.lock().unwrap() = Some(format!("SSH connection closed: {reason:?}"));
        Ok(())
    }
}
struct Connection {
    handle: client::Handle<Handler>,
    transport: Transport,
    error: Arc<Mutex<Option<String>>>,
}
impl Drop for Connection {
    fn drop(&mut self) {
        let _ = self.transport.0.shutdown(Shutdown::Both);
    }
}
pub(crate) fn is_transient_connection_error(error: &anyhow::Error) -> bool {
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

impl Connection {
    async fn connect(server: &Server) -> Result<Self> {
        server.validate().map_err(anyhow::Error::msg)?;
        let started = std::time::Instant::now();
        let result = tokio::time::timeout(
            CONNECT_TIMEOUT,
            retry_connection(|| Self::connect_once(server), Duration::from_secs(2)),
        )
        .await
        .context("SSH connection or authentication timed out after 20 seconds")
        .and_then(|result| result);
        crate::connectivity::report(
            server,
            started,
            result.as_ref().map(|_| ()).map_err(|e| format!("{e:#}")),
        );
        result
    }
    async fn connect_once(server: &Server) -> Result<Self> {
        let socket = TcpStream::connect((server.ssh_host.as_str(), server.ssh_port))
            .await
            .context("Cannot connect to SSH server")?;
        socket.set_nodelay(true)?;
        let socket = socket.into_std()?;
        let transport = Transport(socket.try_clone()?);
        let error = Arc::new(Mutex::new(None));
        let handler = Handler {
            host: server.ssh_host.clone(),
            port: server.ssh_port,
            error: error.clone(),
        };
        let config = Arc::new(client::Config {
            keepalive_interval: Some(Duration::from_secs(15)),
            keepalive_max: 2,
            nodelay: true,
            ..Default::default()
        });
        let mut handle = client::connect_stream(config, TcpStream::from_std(socket)?, handler)
            .await
            .context("SSH handshake or host-key verification failed")?;
        auth::authenticate(&mut handle, server).await?;
        Ok::<_, anyhow::Error>(Self {
            handle,
            transport,
            error,
        })
    }
    async fn close(&self) {
        let _ = tokio::time::timeout(
            Duration::from_secs(1),
            self.handle
                .disconnect(Disconnect::ByApplication, "Porthop disconnected", "en"),
        )
        .await;
        let _ = self.transport.0.shutdown(Shutdown::Both);
    }
}
async fn terminal_request_reply(
    channel: &mut russh::Channel<client::Msg>,
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

/// A remote PTY owns shell state; only bytes and window sizes cross IPC.
pub async fn terminal(
    server: &Server,
    cols: u32,
    rows: u32,
    mut input: tokio::sync::mpsc::Receiver<crate::terminal::Input>,
    output: &tokio::sync::mpsc::Sender<crate::terminal::Event>,
) -> Result<()> {
    use crate::terminal::{Event, Input};
    let connection = Connection::connect(server).await?;
    let mut early_output = Vec::new();
    let channel = tokio::time::timeout(Duration::from_secs(10), async {
        let mut channel = connection.handle.channel_open_session().await?;
        channel
            .request_pty(true, "xterm-256color", cols, rows, 0, 0, &[])
            .await?;
        terminal_request_reply(&mut channel, "the terminal PTY", &mut early_output).await?;
        channel.request_shell(true).await?;
        terminal_request_reply(&mut channel, "an interactive shell", &mut early_output).await?;
        Ok::<_, anyhow::Error>(channel)
    })
    .await
    .context("Terminal setup timed out")??;
    output.send(Event::Ready).await?;
    for bytes in early_output.chunks(32768) {
        output.send(Event::Data(bytes.to_vec())).await?;
    }
    let (mut reader, writer) = channel.split();
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
                Input::Resize(cols, rows) => writer.window_change(cols, rows, 0, 0).await?,
            }
        }
        Ok::<_, anyhow::Error>(None)
    };
    // Read and write independently, so echoed input cannot deadlock a large paste.
    let exit = tokio::select! { result = receive => result?, result = send => result? };
    let lost = connection.handle.is_closed() && exit.is_none();
    connection.close().await;
    if lost {
        bail!("SSH connection lost. Reconnect to start a new shell.");
    }
    output.send(Event::Exit(exit)).await?;
    Ok(())
}

pub struct Forwarding {
    connection: Arc<Connection>,
    tasks: JoinSet<()>,
    failure: Arc<Mutex<Option<String>>>,
    health: health::Health,
}
impl Forwarding {
    pub async fn start(server: &Server, tunnel: Option<&Tunnel>) -> Result<Self> {
        // Acquire the whole range before connecting; a partial bind rolls back by drop.
        let mut listeners = Vec::new();
        if let Some(t) = tunnel {
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
        Ok(Self {
            connection,
            tasks,
            failure,
            health,
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
        self.tasks.shutdown().await;
        self.connection.close().await;
    }
}
impl Drop for Forwarding {
    fn drop(&mut self) {
        self.tasks.abort_all();
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
            ssh_port: std::env::var("PORTHOP_TEST_SSH_PORT")
                .unwrap()
                .parse()
                .unwrap(),
            identity_file: Some(std::env::var("PORTHOP_TEST_IDENTITY").unwrap()),
            agent_source: None,
            agent_key_fingerprint: None,
            auth_method: AuthMethod::PublicKey,
            clipboard_enabled: false,
            browser_enabled: false,
        }
    }
    #[tokio::test]
    #[ignore = "Run with scripts/test-ssh.sh"]
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
    #[ignore = "Run with scripts/test-ssh.sh"]
    async fn embedded_auth_and_persistent_exec() {
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
        assert!(execute(&s, "fail", None)
            .await
            .unwrap_err()
            .contains("fixture failure"));
        assert!(execute(&s, "large", None)
            .await
            .unwrap_err()
            .contains("exceeded 4 MiB"));
        // This uses the fixture's real SSH-agent Unix socket, not a mocked russh API.
        s.identity_file = Some(format!(
            "{}.encrypted",
            std::env::var("PORTHOP_TEST_IDENTITY").unwrap()
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
        assert!(execute(&s, "printf ok", None)
            .await
            .unwrap_err()
            .contains("selected agent key"));
        s.agent_key_fingerprint = None;
        s.agent_source = None;
        s.auth_method = AuthMethod::Password;
        // Exercise the production credential reader against an encrypted fixture vault.
        let vault_directory = tempfile::tempdir().unwrap();
        let store = Arc::new(crate::config::Store::for_test(
            vault_directory.path().into(),
        ));
        let config = crate::model::Config {
            servers: vec![s.clone()],
            tunnels: vec![],
        };
        let password = zeroize::Zeroizing::new(std::env::var("PORTHOP_TEST_PASSWORD").unwrap());
        store
            .save_with_password(&config, Some((s.id, &password)))
            .unwrap();
        crate::credentials::initialize(store);
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
        assert_eq!(session.execute("printf ok", None).await.unwrap(), "ok");
        assert!(session
            .execute("fail", None)
            .await
            .unwrap_err()
            .contains("fixture failure"));
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
        let _port = TcpListener::bind(("127.0.0.1", local))
            .await
            .expect("Cancelled handshake must release listener");
        tokio::time::timeout(Duration::from_secs(2), peer)
            .await
            .expect("Cancelled handshake must close SSH transport")
            .unwrap();
    }
}

#[cfg(test)]
mod connection_retry_tests {
    use super::*;

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
