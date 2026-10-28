//! Bounded remote command execution and reusable authenticated sessions.
use super::{Connection, MAX_OUTPUT};
use crate::model::Server;
use anyhow::{Context as _, Result, bail};
use russh::ChannelMsg;
use serde::Serialize;
use std::time::Duration;

/// sshd runs an exec request through the account's login shell, which may be
/// zsh or fish. Newport's commands are POSIX sh, so they run under sh: zsh
/// aborts on a glob that matches nothing, and fish can't parse them at all.
/// The login shell's environment still reaches the command.
fn posix(command: &str) -> String {
    format!("exec sh -c {}", super::shell_quote(command))
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandResult {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: u32,
}

const COMMAND_TIMEOUT: Duration = Duration::from_secs(25);
/// How a command fails when it outlives `COMMAND_TIMEOUT`, after it started
/// or while starting; callers that retry timeouts match these.
pub(crate) const REQUEST_TIMED_OUT: &str = "SSH request timed out after 25 seconds";
pub(crate) const START_TIMED_OUT: &str = "SSH command timed out starting";

impl CommandResult {
    fn into_stdout(self) -> Result<String, String> {
        if self.exit_code == 0 {
            return Ok(self.stdout);
        }
        let details = [self.stderr.trim(), self.stdout.trim()]
            .into_iter()
            .filter(|text| !text.is_empty())
            .collect::<Vec<_>>()
            .join("\n");
        Err(format!(
            "Remote command failed (status {}): {details}",
            self.exit_code
        ))
    }
}

pub async fn execute(
    server: &Server,
    command: &str,
    input: Option<&str>,
) -> Result<String, String> {
    execute_result(server, command, input).await?.into_stdout()
}

pub async fn execute_result(
    server: &Server,
    command: &str,
    input: Option<&str>,
) -> Result<CommandResult, String> {
    let connection = Connection::connect(server)
        .await
        .map_err(|error| format!("{error:#}"))?;
    let result = connection.command(command, input.map(str::as_bytes)).await;
    connection.close().await;
    result.map_err(|error| format!("{error:#}"))
}

// Admission may authenticate an additional pooled transport. It owns its
// network deadlines and local consent wait, just like the primary connection.
// Only start the remote command's deadline once admission has completed.
async fn execute_after_connect<C, T, F>(
    connection: impl std::future::Future<Output = Result<C>>,
    command: impl FnOnce(C) -> F,
) -> Result<T>
where
    F: std::future::Future<Output = Result<T>>,
{
    let connection = connection.await?;
    tokio::time::timeout(COMMAND_TIMEOUT, command(connection))
        .await
        .map_err(|_| anyhow::Error::msg(REQUEST_TIMED_OUT))?
}

async fn send_upload<'a, F, Fut>(
    input: &'a [u8],
    mut send: F,
    progress: &(dyn Fn(usize) + Send + Sync),
) -> Result<()>
where
    F: FnMut(&'a [u8]) -> Fut,
    Fut: std::future::Future<Output = Result<(), russh::Error>>,
{
    let mut sent = 0;
    for chunk in input.chunks(32 * 1024) {
        tokio::time::timeout(Duration::from_secs(45), send(chunk))
            .await
            .context("Agent upload stalled for 45 seconds")??;
        sent += chunk.len();
        progress(sent);
    }
    Ok(())
}

impl Connection {
    async fn command(&self, command: &str, input: Option<&[u8]>) -> Result<CommandResult> {
        execute_after_connect(self.open_session(), |channel| {
            self.command_on_channel(channel, command, input, None, None)
        })
        .await
    }
    async fn command_with_progress(
        &self,
        command: &str,
        input: Option<&[u8]>,
        progress: Option<&(dyn Fn(usize) + Send + Sync)>,
        resource: Option<crate::agent_payload::Upload>,
    ) -> Result<CommandResult> {
        let channel = self.open_session().await?;
        tokio::time::timeout(
            Duration::from_secs(900),
            self.command_on_channel(channel, command, input, progress, resource),
        )
        .await
        .context("Agent upload exceeded the 15-minute limit")?
    }
    async fn command_on_channel(
        &self,
        channel: russh::Channel<russh::client::Msg>,
        command: &str,
        input: Option<&[u8]>,
        progress: Option<&(dyn Fn(usize) + Send + Sync)>,
        mut resource: Option<crate::agent_payload::Upload>,
    ) -> Result<CommandResult> {
        let (mut reader, writer) = channel.split();
        // Deadlines and caller cancellation must release the session slot too.
        let writer = super::SessionChannel {
            writer: Some(writer),
            connection: self,
        };
        writer.exec(true, posix(command)).await?;
        let upload_done = tokio::sync::Notify::new();
        let send = async {
            if let Some(resource) = &mut resource {
                let mut sent = 0;
                while let Some(chunk) = resource.next().await.map_err(anyhow::Error::msg)? {
                    tokio::time::timeout(Duration::from_secs(45), writer.data(chunk.as_slice()))
                        .await
                        .context("Agent upload stalled for 45 seconds")??;
                    sent += chunk.len();
                    if let Some(progress) = progress {
                        progress(sent);
                    }
                }
            }
            if let Some(input) = input {
                if let Some(progress) = progress {
                    send_upload(input, |chunk| writer.data(chunk), progress).await?;
                } else {
                    writer.data(input).await?;
                }
            }
            writer.eof().await?;
            upload_done.notify_one();
            Ok::<_, anyhow::Error>(())
        };
        let receive = async {
            let (mut out, mut err, mut status) = (Vec::new(), Vec::new(), None);
            while let Some(message) = reader.wait().await {
                match message {
                    ChannelMsg::Data { data } => {
                        if out.len() + data.len() > MAX_OUTPUT {
                            bail!("Remote output exceeded 4 MiB");
                        }
                        out.extend_from_slice(&data);
                    }
                    ChannelMsg::ExtendedData { data, .. } => {
                        if err.len() + data.len() > MAX_OUTPUT {
                            bail!("Remote error output exceeded 4 MiB");
                        }
                        err.extend_from_slice(&data);
                    }
                    ChannelMsg::ExitStatus { exit_status } => status = Some(exit_status),
                    ChannelMsg::Failure => bail!("Server refused the SSH exec request"),
                    ChannelMsg::ExitSignal { .. } => bail!("Remote command terminated by a signal"),
                    _ => {}
                }
            }
            if status.is_none() && (self.handle.is_closed() || self.error.lock().unwrap().is_some())
            {
                bail!(russh::Error::Disconnect);
            }
            let exit_code = status
                .ok_or_else(|| anyhow::anyhow!("Remote command ended without an exit status"))?;
            Ok::<_, anyhow::Error>(CommandResult {
                stdout: String::from_utf8_lossy(&out).into_owned(),
                stderr: String::from_utf8_lossy(&err).into_owned(),
                exit_code,
            })
        };
        let bounded_receive = async {
            tokio::pin!(receive);
            tokio::select! {
                result = &mut receive => result,
                _ = upload_done.notified(), if progress.is_some() => {
                    tokio::time::timeout(Duration::from_secs(60), receive).await
                        .context("Agent verification or installation timed out after 60 seconds")?
                }
            }
        };
        let result = tokio::try_join!(send, bounded_receive);
        let _ = writer.close().await;
        result.map(|(_, result)| result)
    }
}

// Preserve retryable setup failures before turning errors into UI strings.
fn session_error(error: anyhow::Error) -> String {
    let transient = super::is_transient_connection_error(&error);
    if error.is::<super::auth::AgentApprovalRequired>() {
        format!("SSH approval required: {error:#}")
    } else if error.is::<super::auth::SelectedAgentUnavailable>() {
        format!("SSH agent unavailable: {error:#}")
    } else if transient {
        format!("SSH transport interrupted: {error:#}")
    } else {
        format!("{error:#}")
    }
}

/// Reuse an authenticated transport for setup commands and streaming channels.
pub struct ExecSession(std::sync::Arc<Connection>, Server);
pub(crate) struct ConnectFailure {
    pub message: String,
    pub retryable: bool,
}
impl ExecSession {
    pub(crate) fn retry_session_admission(&self) {
        self.0.sessions.retry();
    }
    pub(crate) fn channel_pressure(&self) -> tokio::sync::watch::Receiver<u64> {
        self.0.channel_pressure.subscribe()
    }
    pub(crate) async fn sftp(&self) -> Result<std::sync::Arc<super::sftp::Session>> {
        let mut cached = self.0.sftp.lock().await;
        if let Some(session) = cached.upgrade().filter(|s| !s.is_closed()) {
            return Ok(session);
        }
        let session = std::sync::Arc::new(super::sftp::Session::open(&self.0).await?);
        *cached = std::sync::Arc::downgrade(&session);
        Ok(session)
    }
    pub(crate) async fn terminal(
        &self,
        io: super::TerminalIo<'_>,
        selector: uuid::Uuid,
        gui: std::sync::Arc<crate::terminal::display::Prepared>,
        terminal: Option<crate::terminal::shell::Identity>,
        shell: Option<&crate::terminal::shell::State>,
    ) -> Result<super::TerminalEnd> {
        super::terminal_session(&self.0, io, selector, gui, terminal, shell).await
    }
    /// Exactly one attempt. The server-session supervisor owns retry policy.
    pub(crate) async fn connect_session(server: &Server) -> Result<Self, ConnectFailure> {
        Connection::connect_with_policy(server, super::ConnectionPolicy::Integration)
            .await
            .map(|connection| Self(std::sync::Arc::new(connection), server.clone()))
            .map_err(|error| ConnectFailure {
                retryable: super::is_transient_connection_error(&error),
                message: session_error(error),
            })
    }
    /// Protocol round trip only: no shell command and no new authentication.
    pub(crate) async fn responsive(&self) -> bool {
        self.0.handle.send_ping().await.is_ok() && !self.is_closed()
    }
    pub(crate) async fn disconnected(&self) {
        while !self.is_closed() {
            // The SSH callback can notify before the channel sender closes.
            // A bounded fallback avoids losing that ordering edge.
            tokio::select! {
                _ = self.0.disconnected.notified() => {},
                _ = tokio::time::sleep(Duration::from_millis(250)) => {},
            }
        }
    }
    /// Refresh non-connection metadata without opening another transport.
    pub(crate) fn with_server(&self, server: Server) -> Self {
        debug_assert!(self.1.same_connection(&server));
        Self(self.0.clone(), server)
    }
    pub async fn connect(server: &Server) -> Result<Self, String> {
        Connection::connect(server)
            .await
            .map(|connection| Self(std::sync::Arc::new(connection), server.clone()))
            .map_err(session_error)
    }
    /// Long-lived sync waits for agent approval without a request deadline.
    /// The caller must own cancellation and reconnect backoff.
    pub async fn connect_integration(server: &Server) -> Result<Self, String> {
        Connection::connect_with_policy(server, super::ConnectionPolicy::Integration)
            .await
            .map(|connection| Self(std::sync::Arc::new(connection), server.clone()))
            .map_err(session_error)
    }
    pub fn server_id(&self) -> uuid::Uuid {
        self.1.id
    }
    pub fn server(&self) -> &Server {
        &self.1
    }
    /// Uploads have an inactivity deadline per chunk and a bounded overall budget.
    pub async fn upload(
        &self,
        command: &str,
        input: &[u8],
        progress: &(dyn Fn(usize) + Send + Sync),
    ) -> Result<String, String> {
        self.0
            .command_with_progress(command, Some(input), Some(progress), None)
            .await
            .map_err(session_error)?
            .into_stdout()
    }
    /// Decode and upload a packaged payload with bounded buffers and the same deadlines.
    pub(crate) async fn upload_resource(
        &self,
        command: &str,
        input: crate::agent_payload::Upload,
        progress: &(dyn Fn(usize) + Send + Sync),
    ) -> Result<String, String> {
        self.0
            .command_with_progress(command, None, Some(progress), Some(input))
            .await
            .map_err(session_error)?
            .into_stdout()
    }
    pub async fn execute(&self, command: &str, input: Option<&[u8]>) -> Result<String, String> {
        let result = self
            .0
            .command(command, input)
            .await
            .map_err(session_error)?;
        result.into_stdout()
    }

    pub async fn stream(
        &self,
        command: &str,
    ) -> Result<russh::ChannelStream<russh::client::Msg>, String> {
        self.exec_channel(command)
            .await
            .map(|channel| channel.into_stream())
    }

    /// A started command's channel, for callers that need its error output
    /// and exit status as well as its stdout.
    pub(crate) async fn exec_channel(
        &self,
        command: &str,
    ) -> Result<russh::Channel<russh::client::Msg>, String> {
        let channel = super::PendingChannel::new(&self.0)
            .await
            .map_err(session_error)?;
        tokio::time::timeout(COMMAND_TIMEOUT, channel.exec(true, posix(command)))
            .await
            .map_err(|_| START_TIMED_OUT.to_owned())?
            .map_err(|error| session_error(error.into()))?;
        Ok(channel.take())
    }

    /// This is a forwarding channel, not an SSH exec/session slot. Before any
    /// admission bytes are sent, a refused open is safe to fall back from.
    pub(crate) async fn direct_streamlocal(
        &self,
        path: &str,
    ) -> Result<russh::Channel<russh::client::Msg>, String> {
        tokio::time::timeout(
            Duration::from_secs(5),
            self.0.handle.channel_open_direct_streamlocal(path),
        )
        .await
        .map_err(|_| "SSH worker forwarding timed out".to_owned())?
        .map_err(|error| error.to_string())
    }

    pub fn is_closed(&self) -> bool {
        self.0.handle.is_closed() || self.0.sessions.is_stopped()
    }
    pub(crate) fn abort_transport(&self) {
        self.0.abort();
    }

    pub fn callbacks(&self) -> super::callback::Callbacks {
        super::callback::Callbacks::new(self.0.clone())
    }

    pub async fn close(&self) {
        self.0.close().await;
    }
}

#[cfg(test)]
#[path = "session_pool_tests.rs"]
mod session_pool_tests;

#[cfg(test)]
mod tests {
    use super::*;

    /// As sshd runs it for an account whose login shell is `shell`. Servers are
    /// Linux; on Windows `bash` may be the WSL launcher rather than a shell.
    #[cfg(unix)]
    fn through_login_shell(shell: &str, command: &str) -> Option<std::process::Output> {
        std::process::Command::new(shell)
            .arg("-c")
            .arg(posix(command))
            .output()
            .ok()
    }

    #[cfg(unix)]
    #[test]
    fn commands_keep_posix_meaning_under_any_login_shell() {
        let empty = tempfile::tempdir().unwrap();
        let script = format!(
            "set -eu; n=0; for f in {}/*; do test -e \"$f\" && n=$((n+1)); done; printf '%s' \"$n\"",
            super::super::shell_quote(&empty.path().display().to_string())
        );
        for shell in ["sh", "bash", "zsh", "fish"] {
            let Some(output) = through_login_shell(shell, &script) else {
                continue;
            };
            assert!(output.status.success(), "{shell}: {output:?}");
            assert_eq!(output.stdout, b"0", "{shell}");
        }
    }

    #[tokio::test]
    #[ignore = "Run with cargo xtask test ssh -- ssh::exec::tests::session_capacity"]
    async fn session_capacity_concurrent_sftp_uses_one_slot_and_replaces_closed_stream() {
        let session = std::sync::Arc::new(
            ExecSession::connect(&super::super::integration_tests::server())
                .await
                .unwrap(),
        );
        let mut held = Vec::new();
        for _ in 0..9 {
            held.push(
                session
                    .0
                    .handle
                    .channel_open_session()
                    .await
                    .unwrap()
                    .into_stream(),
            );
        }
        let mut requests = tokio::task::JoinSet::new();
        for _ in 0..16 {
            let session = session.clone();
            requests.spawn(async move {
                let sftp = session.sftp().await.unwrap();
                assert!(!sftp.realpath(".").await.unwrap().files.is_empty());
                sftp
            });
        }
        let mut clients = Vec::new();
        while let Some(result) = requests.join_next().await {
            clients.push(result.unwrap());
        }
        assert!(
            clients
                .iter()
                .all(|client| std::sync::Arc::ptr_eq(client, &clients[0]))
        );
        assert!(!session.is_closed());
        clients[0].close_session().unwrap();
        tokio::time::timeout(Duration::from_secs(3), async {
            while !clients[0].is_closed() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        let replacement = session.sftp().await.unwrap();
        assert!(!std::sync::Arc::ptr_eq(&replacement, &clients[0]));
        replacement.realpath(".").await.unwrap();
        drop(replacement);
        drop(clients);
        assert_eq!(session.execute("printf ok", None).await.unwrap(), "ok");
        drop(held);
        session.close().await;
    }

    #[tokio::test]
    #[ignore = "Run with cargo xtask test ssh -- ssh::exec::tests::session_capacity"]
    async fn session_capacity_waits_for_release_and_cancellation_preserves_transport() {
        let session = std::sync::Arc::new(
            ExecSession::connect(&super::super::integration_tests::server())
                .await
                .unwrap(),
        );
        let mut held = Vec::new();
        for _ in 0..10 {
            held.push(
                session
                    .0
                    .handle
                    .channel_open_session()
                    .await
                    .unwrap()
                    .into_stream(),
            );
        }
        // A cancelled request must release channel admission without closing
        // any active channel or re-authenticating the shared connection.
        let mut pressure = session.channel_pressure();
        let pending = {
            let session = session.clone();
            tokio::spawn(async move { session.execute("printf ok", None).await })
        };
        tokio::time::timeout(Duration::from_secs(2), pressure.changed())
            .await
            .unwrap()
            .unwrap();
        pending.abort();
        assert!(pending.await.unwrap_err().is_cancelled());
        let pending = {
            let session = session.clone();
            tokio::spawn(async move { session.execute("printf ok", None).await })
        };
        tokio::time::timeout(Duration::from_secs(2), pressure.changed())
            .await
            .unwrap()
            .unwrap();
        drop(held.pop());
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(3), pending)
                .await
                .unwrap()
                .unwrap()
                .unwrap(),
            "ok"
        );
        assert!(session.responsive().await);
        drop(held);
        session.close().await;
    }

    #[tokio::test]
    #[ignore = "Run with cargo xtask test ssh -- session_capacity"]
    async fn session_capacity_cancelled_exec_releases_its_slot() {
        let session = std::sync::Arc::new(
            ExecSession::connect(&super::super::integration_tests::server())
                .await
                .unwrap(),
        );
        let mut held = Vec::new();
        for _ in 0..9 {
            held.push(
                session
                    .0
                    .handle
                    .channel_open_session()
                    .await
                    .unwrap()
                    .into_stream(),
            );
        }
        let started = std::sync::Arc::new(tokio::sync::Notify::new());
        let command = {
            let session = session.clone();
            let started = started.clone();
            tokio::spawn(async move {
                let progress = |_| started.notify_one();
                session
                    .0
                    .command_with_progress(
                        "stall-until-close",
                        Some(b"input"),
                        Some(&progress),
                        None,
                    )
                    .await
            })
        };
        // The upload callback proves exec and input were sent. Cancellation
        // now tests an in-flight command, not just waiting for admission.
        tokio::time::timeout(Duration::from_secs(2), started.notified())
            .await
            .unwrap();
        command.abort();
        assert!(command.await.unwrap_err().is_cancelled());
        assert_eq!(session.execute("printf ok", None).await.unwrap(), "ok");
        assert!(session.responsive().await);
        drop(held);
        session.close().await;
    }

    #[tokio::test]
    #[ignore = "Run with cargo xtask test ssh -- ssh::exec::tests::session_capacity"]
    async fn session_capacity_reclaims_idle_git_and_keeps_active_workers() {
        struct Identity;
        impl crate::git::ClientIdentity for Identity {
            fn git_client_id(&self) -> crate::git::IdentityFuture {
                Box::pin(std::future::ready(Ok(uuid::Uuid::new_v4().to_string())))
            }
        }
        let server = super::super::integration_tests::server();
        let directory = tempfile::tempdir().unwrap();
        let mut manager =
            crate::manager::Manager::new(crate::profile::Store::for_test(directory.path().into()));
        manager.config.servers.push(server.clone());
        let owner = manager.sessions.session(&server, 0);
        let state = crate::manager::share(manager);
        crate::git::git_connect(&Identity, &state, server.id)
            .await
            .unwrap();
        let lease = owner.acquire(&server).await.unwrap();
        let generation = owner.transport_snapshot().generation;
        let mut held = Vec::new();
        for _ in 0..8 {
            let stream = lease.launch(crate::agent::Service::Git).await.unwrap().0;
            held.push(crate::git::client::Client::start(stream).await.unwrap().0);
        }
        assert_eq!(lease.execute("printf ok", None).await.unwrap(), "ok");
        assert_eq!(owner.transport_snapshot().generation, generation);
        // These uncached workers cannot be reclaimed. They remain usable after
        // the idle Git lane makes room for the command on the same transport.
        for mut client in held {
            client.ping().await.unwrap();
        }
        crate::git::git_connect(&Identity, &state, server.id)
            .await
            .unwrap();
        state.sessions.shutdown().await;
    }

    #[tokio::test]
    #[ignore = "Run with the loopback SSH fixture and file keys"]
    async fn session_capacity_preserves_forwarded_optional_workers() {
        struct Identity;
        impl crate::git::ClientIdentity for Identity {
            fn git_client_id(&self) -> crate::git::IdentityFuture {
                Box::pin(std::future::ready(Ok(uuid::Uuid::new_v4().to_string())))
            }
        }
        let mut server = super::super::integration_tests::server();
        server.ssh_user = "fixture-streamlocal".into();
        let directory = tempfile::tempdir().unwrap();
        let mut manager =
            crate::manager::Manager::new(crate::profile::Store::for_test(directory.path().into()));
        manager.config.servers.push(server.clone());
        let owner = manager.sessions.session(&server, 0);
        let state = crate::manager::share(manager);
        crate::git::git_connect(&Identity, &state, server.id)
            .await
            .unwrap();
        #[cfg(target_os = "linux")]
        crate::monitor::snapshot(&owner).await.unwrap();
        let lease = owner.acquire(&server).await.unwrap();
        let before = owner.agent_runtime.snapshot().instances.remove(0);
        assert_eq!(
            before.worker_leases,
            if cfg!(target_os = "linux") { 2 } else { 1 }
        );
        for _ in 0..3 {
            // The primary has MaxSessions=1, occupied by runtime control.
            // Pressure must leave the forwarded monitor and Git lanes alive.
            assert_eq!(lease.execute("printf ok", None).await.unwrap(), "ok");
            #[cfg(target_os = "linux")]
            crate::monitor::snapshot(&owner).await.unwrap();
            let after = owner.agent_runtime.snapshot().instances.remove(0);
            assert_eq!(after.instance_id, before.instance_id);
            assert_eq!(after.registrations, before.registrations);
            assert_eq!(after.worker_leases, before.worker_leases);
        }
        state.sessions.shutdown().await;
    }

    #[tokio::test(start_paused = true)]
    async fn command_deadline_starts_after_slow_agent_approval() {
        let started = tokio::time::Instant::now();
        let output = execute_after_connect(
            async {
                // The production request policy must also preserve this wait.
                super::super::ConnectionPolicy::Request
                    .establish(|| async {
                        tokio::time::sleep(Duration::from_secs(60)).await;
                        Ok("authenticated")
                    })
                    .await
            },
            |connection| async move {
                assert_eq!(connection, "authenticated");
                tokio::time::sleep(Duration::from_secs(24)).await;
                Ok("ok")
            },
        )
        .await
        .unwrap();

        assert_eq!(output, "ok");
        assert_eq!(started.elapsed(), Duration::from_secs(84));
    }

    #[tokio::test(start_paused = true)]
    async fn stalled_command_is_cancelled_and_releases_its_connection() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicBool, Ordering};

        struct ConnectionGuard(Arc<AtomicBool>);
        impl Drop for ConnectionGuard {
            fn drop(&mut self) {
                self.0.store(true, Ordering::SeqCst);
            }
        }
        let released = Arc::new(AtomicBool::new(false));
        let started = tokio::time::Instant::now();
        let error = execute_after_connect(
            async {
                tokio::time::sleep(Duration::from_secs(60)).await;
                Ok(ConnectionGuard(released.clone()))
            },
            |connection| async move {
                let _connection = connection;
                std::future::pending::<Result<()>>().await
            },
        )
        .await
        .unwrap_err();

        assert_eq!(error.to_string(), REQUEST_TIMED_OUT);
        assert_eq!(started.elapsed(), Duration::from_secs(85));
        assert!(released.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn failed_authentication_never_executes_command() {
        let executed = std::cell::Cell::new(false);
        let error = execute_after_connect(
            std::future::ready(Err::<(), _>(anyhow::anyhow!("Authentication rejected"))),
            |_| {
                executed.set(true);
                std::future::ready(Ok(()))
            },
        )
        .await
        .unwrap_err();

        assert_eq!(error.to_string(), "Authentication rejected");
        assert!(!executed.get());
    }

    #[test]
    fn session_errors_distinguish_transport_from_permissions() {
        assert!(
            session_error(anyhow::Error::new(std::io::Error::from(
                std::io::ErrorKind::ConnectionReset
            )))
            .starts_with("SSH transport interrupted:")
        );
        assert!(
            session_error(anyhow::Error::new(russh::Error::HUP))
                .starts_with("SSH transport interrupted:")
        );
        assert!(
            !session_error(anyhow::Error::new(std::io::Error::from(
                std::io::ErrorKind::PermissionDenied
            )))
            .starts_with("SSH transport interrupted:")
        );
        assert!(
            !session_error(anyhow::anyhow!("Host key changed"))
                .starts_with("SSH transport interrupted:")
        );
    }

    #[test]
    fn selected_agent_error_survives_session_string_conversion() {
        let error = anyhow::Error::new(super::super::auth::SelectedAgentUnavailable)
            .context("Authentication failed");
        assert!(session_error(error).starts_with("SSH agent unavailable:"));
    }

    #[test]
    fn failure_keeps_both_streams_separate() {
        let result = CommandResult {
            stdout: "partial output\n".into(),
            stderr: "denied\n".into(),
            exit_code: 1,
        };
        assert_eq!(
            result.into_stdout().unwrap_err(),
            "Remote command failed (status 1): denied\npartial output"
        );
    }
    #[test]
    fn success_preserves_output_bytes_and_ignores_warning_stream() {
        let result = CommandResult {
            stdout: "  output\n".into(),
            stderr: "warning".into(),
            exit_code: 0,
        };
        assert_eq!(result.into_stdout().unwrap(), "  output\n");
    }
    #[tokio::test(start_paused = true)]
    async fn progressing_upload_can_exceed_command_deadline_but_stalls_are_bounded() {
        use std::sync::Mutex;
        let input = vec![0; 3 * 32 * 1024];
        let seen = Mutex::new(Vec::new());
        let started = tokio::time::Instant::now();
        super::send_upload(
            &input,
            |_| async {
                tokio::time::sleep(Duration::from_secs(20)).await;
                Ok(())
            },
            &|sent| seen.lock().unwrap().push(sent),
        )
        .await
        .unwrap();
        assert_eq!(started.elapsed(), Duration::from_secs(60));
        assert_eq!(*seen.lock().unwrap(), vec![32768, 65536, 98304]);
        let started = tokio::time::Instant::now();
        let error = super::send_upload(&input, |_| std::future::pending(), &|_| {
            panic!("stalled bytes must not be counted")
        })
        .await
        .unwrap_err();
        assert!(error.to_string().contains("stalled"));
        assert_eq!(started.elapsed(), Duration::from_secs(45));
    }
}
