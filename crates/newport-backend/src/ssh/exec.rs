//! Bounded remote command execution and reusable authenticated sessions.
use super::{Connection, MAX_OUTPUT};
use crate::model::Server;
use anyhow::{Context as _, Result, bail};
use russh::ChannelMsg;
use serde::Serialize;
use std::time::Duration;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandResult {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: u32,
}

const COMMAND_TIMEOUT: Duration = Duration::from_secs(25);

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
    execute_after_connect(Connection::connect(server), |connection| async move {
        let result = connection.command(command, input.map(str::as_bytes)).await;
        connection.close().await;
        result
    })
    .await
    .map_err(|error| format!("{error:#}"))
}

// Connection setup owns its network deadlines and may await local agent
// consent. Only start the remote command's deadline after authentication.
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
        .map_err(|_| anyhow::anyhow!("SSH request timed out after 25 seconds"))?
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
        self.command_with_progress(command, input, None, None).await
    }
    async fn command_with_progress(
        &self,
        command: &str,
        input: Option<&[u8]>,
        progress: Option<&(dyn Fn(usize) + Send + Sync)>,
        mut resource: Option<crate::agent_payload::Upload>,
    ) -> Result<CommandResult> {
        let channel = self.handle.channel_open_session().await?;
        channel.exec(true, command).await?;
        let (mut reader, writer) = channel.split();
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
    if error.is::<super::auth::SelectedAgentUnavailable>() {
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
    pub(crate) async fn terminal(
        &self,
        size: &mut (u32, u32),
        input: &mut tokio::sync::mpsc::Receiver<crate::terminal::Input>,
        output: &tokio::sync::mpsc::Sender<crate::terminal::Event>,
        selector: &str,
    ) -> Result<super::TerminalEnd> {
        super::terminal_session(&self.0, size, input, output, Some(selector)).await
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
        tokio::time::timeout(
            Duration::from_secs(900),
            self.0
                .command_with_progress(command, Some(input), Some(progress), None),
        )
        .await
        .map_err(|_| "Agent upload exceeded the 15-minute limit".to_owned())?
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
        tokio::time::timeout(
            Duration::from_secs(900),
            self.0
                .command_with_progress(command, None, Some(progress), Some(input)),
        )
        .await
        .map_err(|_| "Agent upload exceeded the 15-minute limit".to_owned())?
        .map_err(session_error)?
        .into_stdout()
    }
    pub async fn execute(&self, command: &str, input: Option<&[u8]>) -> Result<String, String> {
        let result = tokio::time::timeout(COMMAND_TIMEOUT, self.0.command(command, input))
            .await
            .map_err(|_| "SSH command timed out after 25 seconds".to_owned())?
            .map_err(session_error)?;
        result.into_stdout()
    }

    pub async fn stream(
        &self,
        command: &str,
    ) -> Result<russh::ChannelStream<russh::client::Msg>, String> {
        tokio::time::timeout(COMMAND_TIMEOUT, async {
            let channel = self.0.handle.channel_open_session().await?;
            channel.exec(true, command).await?;
            Ok::<_, russh::Error>(channel.into_stream())
        })
        .await
        .map_err(|_| "SSH command timed out starting agent".to_owned())?
        .map_err(|e| session_error(e.into()))
    }

    pub fn is_closed(&self) -> bool {
        self.0.handle.is_closed()
    }
    pub(crate) fn abort_transport(&self) {
        let _ = self.0.transport.0.shutdown(std::net::Shutdown::Both);
    }

    pub fn callbacks(&self) -> super::callback::Callbacks {
        super::callback::Callbacks::new(self.0.clone())
    }

    pub async fn close(&self) {
        self.0.close().await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

        assert_eq!(error.to_string(), "SSH request timed out after 25 seconds");
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
