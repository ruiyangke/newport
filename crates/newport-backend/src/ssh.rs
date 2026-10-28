//! SSH transport, forwarding and interactive shell lifecycle.
pub use crate::terminal::shell::Identity as TerminalIdentity;
mod auth;

/// Single-quote a word for a POSIX shell on the server.
pub(crate) fn shell_quote(word: &str) -> String {
    format!("'{}'", word.replace('\'', "'\\''"))
}
#[cfg(test)]
mod agent_request_tests;
pub mod callback;
mod exec;
mod health;
mod relay;
mod reverse;
mod session_pool;
pub mod sftp;
mod terminal_startup;
#[cfg(test)]
use exec::execute_result;
pub(crate) use exec::{ConnectFailure, REQUEST_TIMED_OUT, START_TIMED_OUT};
/// A connection that did not complete its handshake in time; integration retries it.
pub(crate) const CONNECT_TIMED_OUT: &str = "SSH socket connection or handshake timed out";
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
    sessions: Arc<session_pool::Pool>,
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
        self.sessions.stop();
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
    sessions: Arc<session_pool::Pool>,
    session_probe: Mutex<session_pool::Probe>,
    channel_pressure: tokio::sync::watch::Sender<u64>,
    sftp: tokio::sync::Mutex<std::sync::Weak<sftp::Session>>,
}
impl Drop for Connection {
    fn drop(&mut self) {
        self.abort();
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
            .context(CONNECT_TIMED_OUT)??;
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
        let sessions = Arc::new(session_pool::Pool::new(server));
        let handler = Handler {
            host: server.ssh_host.clone(),
            port: server.ssh_port,
            error: error.clone(),
            disconnected: disconnected.clone(),
            forwards: forwards.clone(),
            sessions: sessions.clone(),
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
            sessions,
            session_probe: Mutex::new(session_pool::Probe::default()),
            channel_pressure: tokio::sync::watch::channel(0).0,
            sftp: tokio::sync::Mutex::default(),
        })
    }
    async fn close(&self) {
        self.sessions.stop();
        let _ = tokio::time::timeout(
            Duration::from_secs(1),
            self.handle
                .disconnect(Disconnect::ByApplication, "Newport disconnected", "en"),
        )
        .await;
        let _ = self.transport.0.shutdown(Shutdown::Both);
    }
    fn abort(&self) {
        self.sessions.stop();
        let _ = self.transport.0.shutdown(Shutdown::Both);
    }
}
#[derive(Debug)]
struct SetupExited(String);
impl std::fmt::Display for SetupExited {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}
impl std::error::Error for SetupExited {}

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
                return Err(SetupExited(format!(
                    "Remote shell exited during setup (status {exit_status})."
                ))
                .into());
            }
            Some(ChannelMsg::ExitSignal { signal_name, .. }) => {
                return Err(SetupExited(format!(
                    "Remote shell ended during setup: {signal_name:?}"
                ))
                .into());
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
    terminal_with_session(
        owner,
        server,
        (cols, rows),
        input,
        output,
        Default::default(),
    )
    .await
}

pub(crate) async fn terminal_with_session(
    owner: Arc<crate::server_session::ServerSession>,
    server: &Server,
    mut size: (u32, u32),
    mut input: tokio::sync::mpsc::Receiver<crate::terminal::Input>,
    output: &tokio::sync::mpsc::Sender<crate::terminal::Event>,
    shell: crate::terminal::Shell,
) -> Result<()> {
    use crate::terminal::Event;
    if !owner.server().same_connection(server) || owner.is_revoked() {
        bail!("Server session changed. Reconnect before retrying.");
    }
    // The terminal task owns this supervisor. Exiting or cancelling the task
    // closes only this tab's socket, including a pending authentication attempt.
    let mut cancelled = owner.cancellation();
    let run = async {
        let selector = owner.integration_session_id();
        let gui = tokio::select! {
            result = shell.display.prepare(&owner) => result?,
            _ = discard_input(&mut input, &mut size) => return Ok(()),
        };
        // A shell's state owns its display, so a display it drops is released.
        let mut gui = Arc::new(gui);
        if let Some(shell) = &shell.terminal {
            shell.initialize(gui).map_err(anyhow::Error::msg)?;
            gui = Arc::new(crate::terminal::display::Prepared::None);
        }
        let transport = owner.terminal_transport();
        let _demand = transport.demand();
        loop {
            let acquired = tokio::select! {
                result = transport.acquire() => result,
                _ = discard_input(&mut input, &mut size) => return Ok(()),
            };
            let (result, lost) = match acquired {
                Ok((connection, _, _lease)) => {
                    owner.reachable();
                    let attempt = shell
                        .terminal
                        .as_ref()
                        .map(|shell| shell.begin())
                        .transpose()
                        .map_err(anyhow::Error::msg)?;
                    let current_gui = shell
                        .terminal
                        .as_ref()
                        .map(|shell| shell.display())
                        .transpose()
                        .map_err(anyhow::Error::msg)?
                        .flatten()
                        .unwrap_or_else(|| gui.clone());
                    let failed = Arc::downgrade(&current_gui);
                    let result = connection
                        .terminal(
                            TerminalIo {
                                size: &mut size,
                                input: &mut input,
                                output,
                            },
                            selector,
                            current_gui,
                            attempt.as_ref().map(|attempt| attempt.identity),
                            shell.terminal.as_deref(),
                        )
                        .await;
                    drop(attempt);
                    // A shared display that fails a shell's startup is
                    // dropped, and the shell starts again at once without it.
                    // The failure is logged, never reported to the terminal.
                    if let Err(error) = &result
                        && error.is::<crate::terminal::display::Unavailable>()
                    {
                        log::info!("Terminal starts again without the shared display: {error:#}");
                        match &shell.terminal {
                            Some(shell) => {
                                if let Some(failed) = failed.upgrade() {
                                    shell.forget_display(&failed);
                                }
                            }
                            None => gui = Arc::new(crate::terminal::display::Prepared::None),
                        }
                        continue;
                    }
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
struct PendingChannel<'a> {
    channel: Option<russh::Channel<client::Msg>>,
    connection: &'a Connection,
}
impl<'a> PendingChannel<'a> {
    async fn new(connection: &'a Connection) -> Result<Self> {
        Ok(Self {
            channel: Some(connection.open_session().await?),
            connection,
        })
    }
    fn take(mut self) -> russh::Channel<client::Msg> {
        self.channel.take().unwrap()
    }
}
impl std::ops::Deref for PendingChannel<'_> {
    type Target = russh::Channel<client::Msg>;
    fn deref(&self) -> &Self::Target {
        self.channel.as_ref().unwrap()
    }
}
impl Drop for PendingChannel<'_> {
    fn drop(&mut self) {
        if let Some(channel) = self.channel.take() {
            drop(SessionChannel {
                writer: Some(channel.split().1),
                connection: self.connection,
            });
        }
    }
}

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
/// What a Newport terminal runs: the login shell, started with Needle's
/// exports for this computer's session, so sharing works without any shell
/// setup. The person's startup files run after it and may add to them. `sh`
/// runs it whatever the login shell is, and the selector is a UUID from the
/// session owner, never shell input.
pub fn terminal_command(selector: uuid::Uuid, terminal: Option<TerminalIdentity>) -> String {
    terminal_command_with_identity(
        selector,
        &crate::terminal::display::Prepared::None,
        terminal,
    )
    .expect("A plain shell has no display to validate")
}
pub(crate) fn terminal_command_with_identity(
    selector: uuid::Uuid,
    gui: &crate::terminal::display::Prepared,
    terminal: Option<crate::terminal::shell::Identity>,
) -> std::result::Result<String, String> {
    terminal_command_preflight(selector, gui, terminal, None)
}
#[cfg(all(test, unix))]
pub(crate) fn checked_terminal_command(
    selector: uuid::Uuid,
    gui: &crate::terminal::display::Prepared,
) -> std::result::Result<String, String> {
    terminal_command_preflight(
        selector,
        gui,
        None,
        Some(&terminal_startup::Readiness::new()),
    )
}

fn terminal_command_preflight(
    selector: uuid::Uuid,
    gui: &crate::terminal::display::Prepared,
    terminal: Option<crate::terminal::shell::Identity>,
    readiness: Option<&terminal_startup::Readiness>,
) -> std::result::Result<String, String> {
    use crate::terminal::display::Prepared;
    use terminal_startup::Ack;
    let assignments = gui.assignments(selector)?;
    let mut setup = String::new();
    if let Some(identity) = terminal {
        setup.push_str(&format!(
            "export NEWPORT_TERMINAL='{}' NEWPORT_TERMINAL_EPOCH='{}'; ",
            identity.terminal, identity.epoch
        ));
    }
    let agent = "agent=\"$HOME/.local/bin/newport-agent\"; \
        if [ -x \"$agent\" ] && exports=$(\"$agent\" env --shell sh 2>/dev/null); then \
        eval \"$exports\"; fi; unset agent exports; ";
    let mut ready =
        readiness.map_or_else(String::new, |readiness| readiness.command(Ack::Unavailable));
    if let Prepared::Shared(lease) = gui {
        let value = |name: &str| {
            assignments
                .get(name)
                .and_then(Option::as_deref)
                .ok_or_else(|| format!("Missing shared display {name}"))
        };
        let socket = value("WAYLAND_DISPLAY")?;
        // GTK still requires a runtime directory when WAYLAND_DISPLAY is
        // absolute. Headless SSH logins may have none. Reuse the owned
        // socket's directory only in that case; preserve an existing user
        // runtime. Bus inheritance is selected separately below.
        let runtime = socket
            .rsplit_once('/')
            .map(|(directory, _)| directory)
            .filter(|directory| !directory.is_empty())
            .ok_or("Shared Wayland socket has no runtime directory")?;
        let environment = lease.environment()?;
        let terminal_bus = lease.terminal_bus()?;
        // Every owned endpoint is checked before any is exported. A missing
        // one leaves an ordinary shell; it never ends or delays the shell.
        let mut available = Vec::new();
        if let Some(authority) = assignments.get("XAUTHORITY").and_then(Option::as_deref) {
            available.push(format!("[ -r {} ]", shell_quote(authority)));
        }
        available.push(format!("[ -S {} ]", shell_quote(socket)));
        let mut exports = format!(
            "if [ -z \"${{XDG_RUNTIME_DIR:-}}\" ]; then export XDG_RUNTIME_DIR={}; fi; ",
            shell_quote(runtime)
        );
        if let Some(address) = terminal_bus.address() {
            // Explicit new-terminal policy, before the login profile runs.
            // Profiles and later command-level overrides remain final.
            available.push(format!(
                "[ -S {} ]",
                shell_quote(environment.fallback_bus_socket().unwrap())
            ));
            exports.push_str(&format!(
                "export DBUS_SESSION_BUS_ADDRESS={}; unset DBUS_STARTER_ADDRESS DBUS_STARTER_BUS_TYPE DBUS_SESSION_BUS_PID DBUS_SESSION_BUS_WINDOWID; ",
                shell_quote(address)
            ));
        } else if let Some(address) = &environment.fallback_bus {
            available.push(format!(
                "{{ [ -n \"${{DBUS_SESSION_BUS_ADDRESS:-}}\" ] || [ -n \"${{DBUS_STARTER_ADDRESS:-}}\" ] || [ -S {} ]; }}",
                shell_quote(environment.fallback_bus_socket().unwrap())
            ));
            exports.push_str(&format!(
                "if [ -z \"${{DBUS_SESSION_BUS_ADDRESS:-}}\" ] && [ -z \"${{DBUS_STARTER_ADDRESS:-}}\" ]; then export DBUS_SESSION_BUS_ADDRESS={}; unset DBUS_STARTER_ADDRESS DBUS_STARTER_BUS_TYPE; fi; ",
                shell_quote(address)
            ));
        }
        // An owned shared lease. Only fixed names are accepted; no PATH,
        // preload variables, or executable shell fragments cross this boundary.
        for (name, value) in &assignments {
            match value {
                Some(value) => exports.push_str(&format!("export {name}={}; ", shell_quote(value))),
                None => exports.push_str(&format!("unset {name}; ")),
            }
        }
        match readiness {
            None => setup.push_str(&format!(
                "if {}; then {exports}fi; ",
                available.join(" && ")
            )),
            Some(readiness) => {
                // A shell whose display fails the recheck below gets back exactly
                // the variables it started with.
                let mut names = assignments
                    .keys()
                    .map(String::as_str)
                    .collect::<std::collections::BTreeSet<_>>();
                names.extend([
                    "XDG_RUNTIME_DIR",
                    "DBUS_SESSION_BUS_ADDRESS",
                    "DBUS_STARTER_ADDRESS",
                    "DBUS_STARTER_BUS_TYPE",
                    "DBUS_SESSION_BUS_PID",
                    "DBUS_SESSION_BUS_WINDOWID",
                ]);
                let names = names.into_iter().collect::<Vec<_>>().join(" ");
                let save = format!(
                    r#"for newport_name in {names}; do eval "newport_set_$newport_name=\${{$newport_name+1}} newport_was_$newport_name=\${{$newport_name-}}"; done; "#
                );
                let restore = format!(
                    r#"for newport_name in {names}; do if eval "[ -n \"\${{newport_set_$newport_name-}}\" ]"; then eval "export $newport_name=\"\$newport_was_$newport_name\""; else unset "$newport_name"; fi; done; "#
                );
                setup.push_str(&format!(
                    "newport_gui=; if {}; then {save}{exports}newport_gui=1; fi; ",
                    available.join(" && ")
                ));
                // Recheck every owned endpoint immediately before acknowledgment,
                // after the agent environment command has completed. A native-only
                // display has no X11 listener or authority file to check.
                let mut checks = vec![format!("[ -S {} ]", shell_quote(socket))];
                if let Some(display) = assignments.get("DISPLAY").and_then(Option::as_deref) {
                    let display = display
                        .strip_prefix(':')
                        .ok_or("Invalid shared X11 display")?;
                    checks.push(format!("[ -r {} ]", shell_quote(value("XAUTHORITY")?)));
                    checks.push(format!(
                        "[ -S {} ]",
                        shell_quote(&format!("/tmp/.X11-unix/X{display}"))
                    ));
                }
                if let Some(address) = &environment.fallback_bus {
                    checks.push(format!(
                        "{{ [ \"${{DBUS_SESSION_BUS_ADDRESS-}}\" != {} ] || [ -S {} ]; }}",
                        shell_quote(address),
                        shell_quote(environment.fallback_bus_socket().unwrap())
                    ));
                }
                ready = format!(
                    "if [ -n \"$newport_gui\" ] && {}; then {}else if [ -n \"$newport_gui\" ]; then {restore}{agent}fi; {}fi; unset newport_gui newport_name; ",
                    checks.join(" && "),
                    readiness.command(Ack::Ready),
                    readiness.command(Ack::Unavailable),
                );
            }
        }
    }
    Ok(terminal_script(selector, &setup, agent, &ready))
}

fn terminal_script(selector: uuid::Uuid, setup: &str, agent: &str, ready: &str) -> String {
    let script = format!(
        "NEWPORT_SESSION={selector}; export NEWPORT_SESSION; {setup}{agent}{ready}exec \"$SHELL\" -l"
    );
    format!("exec sh -c {}", shell_quote(&script))
}

/// A terminal's local ends: its size, the user's input and the events it emits.
pub(crate) struct TerminalIo<'a> {
    pub(crate) size: &'a mut (u32, u32),
    pub(crate) input: &'a mut tokio::sync::mpsc::Receiver<crate::terminal::Input>,
    pub(crate) output: &'a tokio::sync::mpsc::Sender<crate::terminal::Event>,
}

async fn terminal_session(
    connection: &Arc<Connection>,
    TerminalIo {
        size,
        input,
        output,
    }: TerminalIo<'_>,
    integration: uuid::Uuid,
    gui: Arc<crate::terminal::display::Prepared>,
    terminal: Option<crate::terminal::shell::Identity>,
    shell: Option<&crate::terminal::shell::State>,
) -> Result<TerminalEnd> {
    use crate::terminal::{Event, Input};
    let (mut reader, writer) = connection.open_session().await?.split();
    let writer = SessionChannel {
        writer: Some(writer),
        connection,
    };
    let mut early_output = Vec::new();
    let mut readiness = matches!(*gui, crate::terminal::display::Prepared::Shared(_))
        .then(terminal_startup::Readiness::new);
    tokio::time::timeout(Duration::from_secs(10), async {
        writer
            .request_pty(true, "xterm-256color", size.0, size.1, 0, 0, &[])
            .await?;
        terminal_request_reply(&mut reader, "the terminal PTY", &mut early_output).await?;
        // An exec request avoids relying on sshd AcceptEnv configuration.
        writer
            .exec(
                true,
                terminal_command_preflight(integration, &gui, terminal, readiness.as_ref())
                    .map_err(|error| gui.failure(error))?,
            )
            .await?;
        if let Err(error) =
            terminal_request_reply(&mut reader, "an interactive shell", &mut early_output).await
        {
            // An early process exit may precede the exec acknowledgment.
            // Preserve transport/service failures, but classify shared-shell
            // preflight exits before readiness as GUI startup failures.
            if readiness.is_some() && error.is::<SetupExited>() {
                return Err(gui.failure(error.to_string()));
            }
            return Err(error);
        }
        Ok::<_, anyhow::Error>(())
    })
    .await
    .context("Terminal setup timed out")??;
    if let Some(readiness) = &mut readiness {
        let preflight = async {
            loop {
                match readiness
                    .strip(&mut early_output)
                    .map_err(|error| gui.failure(error))?
                {
                    Some(terminal_startup::Ack::Ready) => {
                        // The shell checked remote paths. Also fence the locally
                        // retained generation before releasing any queued input.
                        gui.assignments(integration)
                            .map_err(|error| gui.failure(error))?;
                        return Ok::<_, anyhow::Error>(terminal_startup::Ack::Ready);
                    }
                    Some(ack) => return Ok(ack),
                    None => {}
                }
                match reader.wait().await {
                    Some(ChannelMsg::Data { data } | ChannelMsg::ExtendedData { data, .. }) => {
                        early_output.extend_from_slice(&data);
                    }
                    Some(ChannelMsg::ExitStatus { exit_status }) => {
                        return Err(gui.failure(format!("Graphical terminal preflight failed (status {exit_status}). Reconnect this terminal.")));
                    }
                    Some(ChannelMsg::ExitSignal { signal_name, .. }) => {
                        return Err(gui.failure(format!("Graphical terminal preflight ended: {signal_name:?}. Reconnect this terminal.")));
                    }
                    Some(ChannelMsg::Close) | None => {
                        // A transport loss may close its channels just before
                        // the shared connection observes the disconnect.
                        if connection.handle.is_closed()
                            || tokio::time::timeout(
                                Duration::from_millis(500),
                                connection.disconnected.notified(),
                            )
                            .await
                            .is_ok()
                            || connection.handle.is_closed()
                        {
                            return Err(ConnectionLost.into());
                        }
                        return Err(gui.failure("Graphical terminal closed before display readiness was confirmed. Reconnect this terminal.".into()));
                    }
                    _ => {}
                }
            }
        };
        let ack = tokio::time::timeout(Duration::from_secs(10), preflight)
            .await
            .map_err(|_| {
                gui.failure(
                    "Graphical terminal readiness timed out. Reconnect this terminal.".into(),
                )
            })??;
        // The shell started without the display. Drop it, so a GUI app
        // launched later can start another.
        if ack == terminal_startup::Ack::Unavailable {
            log::info!(
                "Terminal started without the shared display: its endpoints were unavailable"
            );
            if let Some(shell) = shell {
                shell.forget_display(&gui);
            }
        }
    }
    // The shell's state keeps any display it uses from here on.
    drop(gui);
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
    let gui_notice = async {
        if let Some(shell) = shell {
            let error = shell.gui_failure().await;
            log::info!("A terminal's shared display ended; its shell stays connected: {error}");
        }
        // Its environment stays fenced; only a new shell may acquire a
        // replacement. Do not cancel healthy PTY IO.
        std::future::pending::<Result<()>>().await
    };
    // Read and write independently, so echoed input cannot deadlock a large paste.
    let completion = tokio::select! {
        result = receive => Completion::Receive(result),
        result = send => Completion::Send(result),
        result = gui_notice => Completion::Send(result),
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
            use crate::agent_payload::TransferEncoding;
            for encoding in [TransferEncoding::Gzip, TransferEncoding::Raw] {
                let progress = std::sync::atomic::AtomicUsize::new(0);
                assert_eq!(
                    session
                        .upload_resource("stdin", payload.stream(encoding).unwrap(), &|sent| {
                            progress.store(sent, std::sync::atomic::Ordering::Relaxed);
                        })
                        .await
                        .unwrap(),
                    payload.transfer_size(encoding).to_string()
                );
                assert_eq!(
                    progress.load(std::sync::atomic::Ordering::Relaxed),
                    payload.transfer_size(encoding)
                );
            }
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

#[cfg(all(test, unix))]
mod terminal_gui_tests {
    use super::*;
    #[test]
    fn terminal_identity_reaches_the_login_shell_without_gui_setup() {
        use std::os::unix::fs::PermissionsExt;
        let home = tempfile::tempdir().unwrap();
        let shell = home.path().join("login-shell");
        std::fs::write(
            &shell,
            "#!/bin/sh\nprintf '%s\\n' \"$NEWPORT_SESSION\" \"$NEWPORT_TERMINAL\" \"$NEWPORT_TERMINAL_EPOCH\" \"$1\"\n",
        )
        .unwrap();
        std::fs::set_permissions(&shell, std::fs::Permissions::from_mode(0o700)).unwrap();
        let routing = uuid::Uuid::new_v4();
        let terminal = uuid::Uuid::new_v4();
        let identity = crate::terminal::shell::Identity::new(terminal);
        let result = std::process::Command::new("sh")
            .args([
                "-c",
                &terminal_command_with_identity(
                    routing,
                    &crate::terminal::display::Prepared::None,
                    Some(identity),
                )
                .unwrap(),
            ])
            .env("HOME", home.path())
            .env("SHELL", shell)
            .output()
            .unwrap();
        assert!(result.status.success());
        assert_eq!(
            String::from_utf8(result.stdout).unwrap(),
            format!("{routing}\n{terminal}\n{}\n-l\n", identity.epoch)
        );
    }
}
