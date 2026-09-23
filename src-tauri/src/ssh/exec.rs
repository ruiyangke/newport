//! Bounded remote command execution and reusable authenticated sessions.
use super::{Connection, MAX_OUTPUT};
use crate::model::Server;
use anyhow::{bail, Result};
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
    let result = tokio::time::timeout(COMMAND_TIMEOUT, async {
        let connection = Connection::connect(server).await?;
        let result = connection.command(command, input.map(str::as_bytes)).await;
        connection.close().await;
        result
    })
    .await;
    match result {
        Ok(r) => r.map_err(|e: anyhow::Error| format!("{e:#}")),
        Err(_) => Err("SSH request timed out after 25 seconds".into()),
    }
}

impl Connection {
    async fn command(&self, command: &str, input: Option<&[u8]>) -> Result<CommandResult> {
        let channel = self.handle.channel_open_session().await?;
        channel.exec(true, command).await?;
        let (mut reader, writer) = channel.split();
        let send = async {
            if let Some(input) = input {
                writer.data(input).await?;
            }
            writer.eof().await?;
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
        let result = tokio::try_join!(send, receive);
        let _ = writer.close().await;
        result.map(|(_, result)| result)
    }
}

// Preserve transport failure identity before turning errors into UI strings.
fn session_error(error: anyhow::Error) -> String {
    let transient = super::is_transient_connection_error(&error);
    if transient {
        format!("SSH transport interrupted: {error:#}")
    } else {
        format!("{error:#}")
    }
}

/// Reuse an authenticated transport for setup commands and streaming channels.
pub struct ExecSession(std::sync::Arc<Connection>);
impl ExecSession {
    pub async fn connect(server: &Server) -> Result<Self, String> {
        Connection::connect(server)
            .await
            .map(|connection| Self(std::sync::Arc::new(connection)))
            .map_err(session_error)
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
    #[test]
    fn session_errors_distinguish_transport_from_permissions() {
        assert!(session_error(anyhow::Error::new(std::io::Error::from(
            std::io::ErrorKind::ConnectionReset
        )))
        .starts_with("SSH transport interrupted:"));
        assert!(session_error(anyhow::Error::new(russh::Error::HUP))
            .starts_with("SSH transport interrupted:"));
        assert!(!session_error(anyhow::Error::new(std::io::Error::from(
            std::io::ErrorKind::PermissionDenied
        )))
        .starts_with("SSH transport interrupted:"));
        assert!(!session_error(anyhow::anyhow!("Host key changed"))
            .starts_with("SSH transport interrupted:"));
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
}
