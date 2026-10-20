// Deployment and the single, bidirectional SSH connection to the Linux agent.
use crate::desktop_integration::Desktop;
use crate::ssh::ExecSession;
use std::time::Duration;
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    sync::mpsc,
};

pub use crate::agent_runtime::protocol::integration as control;
#[allow(dead_code)]
#[path = "../../../tools/agent/src/wire.rs"]
pub mod wire;

/// All agent-backed features enter here before opening a protocol channel.
/// Read-only startup: deployment belongs exclusively to agent_lifecycle.
pub enum Service {
    Git,
    /// Overview metrics, sampled on the server; see crate::monitor.
    Monitor,
    Integration {
        client: uuid::Uuid,
        clipboard: bool,
        browser: bool,
    },
}
impl Service {
    pub(crate) fn capability(&self) -> (&'static str, u32) {
        match self {
            Self::Git => ("git", crate::git::protocol::VERSION),
            Self::Monitor => ("monitor", 1),
            Self::Integration { .. } => ("integration", control::VERSION),
        }
    }
    #[cfg(test)]
    fn command(&self) -> String {
        match self {
            Self::Git => "exec env NEWPORT_GIT_BACKEND=cli \"$HOME/.local/bin/newport-agent\" git-rpc --stdio".into(),
            Self::Monitor => "exec \"$HOME/.local/bin/newport-agent\" monitor --stdio".into(),
            Self::Integration { client, clipboard, browser } => format!(
                "exec \"$HOME/.local/bin/newport-agent\" serve {client} --managed {} {}",
                if *clipboard { "--clipboard" } else { "" },
                if *browser { "--browser" } else { "" },
            ),
        }
    }
}
#[cfg(test)]
pub async fn launch(
    session: &ExecSession,
    service: Service,
) -> Result<(russh::ChannelStream<russh::client::Msg>, String), String> {
    require_installed(session).await?;
    let stream = session.stream(&service.command()).await?;
    Ok((stream, String::new()))
}

pub(crate) async fn launch_worker(
    session: &ExecSession,
    service: Service,
    hello: &crate::agent_runtime::protocol::Hello,
    worker: uuid::Uuid,
) -> Result<(russh::ChannelStream<russh::client::Msg>, String), String> {
    hello.validate().map_err(|e| e.to_string())?;
    let build = &hello.build_sha256;
    let runtime = &hello.instance_id;
    let socket = format!("'{}'", hello.worker_socket.replace('\'', "'\"'\"'"));
    let arguments = match service {
        Service::Git => "git".into(),
        Service::Monitor => "monitor".into(),
        Service::Integration { client, .. } => format!("integration {client}"),
    };
    // Activation can change the entry point while this runtime is still serving
    // admitted work, so launch the runtime's own retained, immutable build.
    let stream = session.stream(&format!("exec env NEWPORT_GIT_BACKEND=cli \"$HOME/.local/share/newport/needle/builds/{build}/newport-agent\" worker {build} {runtime} {socket} {worker} {arguments}")).await?;
    Ok((stream, String::new()))
}

/// Explicit install and reinstall for remote tests. Feature startup uses
/// `launch`, which never calls these operations.
#[cfg(test)]
pub async fn install(session: &ExecSession) -> Result<String, String> {
    crate::agent_lifecycle::manage_test(session, false, true).await
}
#[cfg(test)]
pub async fn reinstall(session: &ExecSession) -> Result<String, String> {
    crate::agent_lifecycle::manage_test(session, true, true).await
}

pub(crate) async fn is_installed(session: &ExecSession) -> Result<bool, String> {
    let present = session
        .execute(
            "test -x \"$HOME/.local/bin/newport-agent\" && printf installed || true",
            None,
        )
        .await?;
    Ok(present.trim() == "installed")
}

pub(crate) async fn require_installed(session: &ExecSession) -> Result<(), String> {
    if !is_installed(session).await? {
        return Err("Needle is not installed. Open Overview to set up this server.".into());
    }
    Ok(())
}

pub struct Agent<W> {
    writer: W,
    deferred: std::collections::VecDeque<(u8, Vec<u8>)>,
    events: mpsc::Receiver<Result<(u8, Vec<u8>), String>>,
    reader: tokio::task::JoinHandle<()>,
    app: Desktop,
    permissions: std::sync::Arc<crate::integration_permissions::Gate>,
    callbacks: crate::ssh::callback::Callbacks,
}
impl<W> Drop for Agent<W> {
    fn drop(&mut self) {
        self.reader.abort();
    }
}
async fn read_event(input: &mut (impl AsyncRead + Unpin)) -> Result<(u8, Vec<u8>), String> {
    let mut header = [0; 5];
    input.read_exact(&mut header).await.map_err(transport)?;
    let len = wire::payload_length(&header, 8224).map_err(|e| e.to_string())?;
    let mut data = vec![0; len];
    input.read_exact(&mut data).await.map_err(transport)?;
    wire::decode(&data, 8224).map_err(|e| e.to_string())
}
fn transport(error: impl std::fmt::Display) -> String {
    format!("SSH transport interrupted: {error}")
}

pub(crate) fn checked_event(kind: u8, data: Vec<u8>) -> Result<(u8, Vec<u8>), String> {
    match kind {
        b'E' => Err(format!("Agent error: {}", String::from_utf8_lossy(&data))),
        b'T' => Err(transport(String::from_utf8_lossy(&data))),
        _ => Ok((kind, data)),
    }
}

fn request_timeout(kind: u8) -> Duration {
    Duration::from_secs(if kind == b'S' { 120 } else { 25 })
}

impl Agent<tokio::io::Sink> {
    pub async fn start(
        stream: impl AsyncRead + AsyncWrite + Unpin + Send + 'static,
        app: Desktop,
        permissions: std::sync::Arc<crate::integration_permissions::Gate>,
        callbacks: crate::ssh::callback::Callbacks,
    ) -> Result<Agent<impl AsyncWrite + Unpin>, String> {
        let (mut reader, writer) = tokio::io::split(stream);
        let (tx, events) = mpsc::channel(8);
        let reader = tokio::spawn(async move {
            loop {
                let event = read_event(&mut reader).await;
                let failed = event.is_err();
                if tx.send(event).await.is_err() || failed {
                    break;
                }
            }
        });
        let mut agent = Agent {
            writer,
            deferred: Default::default(),
            events,
            reader,
            app,
            permissions,
            callbacks,
        };
        let (kind, data) = tokio::time::timeout(Duration::from_secs(25), agent.event())
            .await
            .map_err(|_| transport("agent startup timed out"))??;
        if kind != b'R' {
            return Err("Unsupported Needle integration handshake.".into());
        }
        let hello: control::Hello = control::decode(&data)
            .map_err(|_| "Update Needle to use live integration permissions.".to_owned())?;
        hello.validate().map_err(|e| e.to_string())?;
        Ok(agent)
    }
}
impl<W: AsyncWrite + Unpin> Agent<W> {
    pub async fn event(&mut self) -> Result<(u8, Vec<u8>), String> {
        if let Some(event) = self.deferred.pop_front() {
            return Ok(event);
        }
        self.receive().await
    }
    async fn receive(&mut self) -> Result<(u8, Vec<u8>), String> {
        let (kind, data) = self
            .events
            .recv()
            .await
            .ok_or_else(|| transport("agent exited"))??;
        checked_event(kind, data)
    }
    pub async fn open(&mut self, data: &[u8]) -> Result<(), String> {
        let (id, request) = wire::parse_browser_request(data).map_err(|e| e.to_string())?;
        let result = self.open_request(&request).await;
        let (success, message) = match result {
            Ok(warning) => (true, warning),
            Err(error) => (false, Some(error)),
        };
        let reply =
            wire::browser_reply(id, success, message.as_deref()).map_err(|e| e.to_string())?;
        let frame = wire::encode(b'B', &reply).map_err(|e| e.to_string())?;
        tokio::time::timeout(Duration::from_secs(5), async {
            self.writer.write_all(&frame).await.map_err(transport)?;
            self.writer.flush().await.map_err(transport)
        })
        .await
        .map_err(|_| transport("browser reply timed out"))?
    }
    async fn open_request(&mut self, request: &str) -> Result<Option<String>, String> {
        if !self.permissions.browser_enabled() {
            self.callbacks.clear();
            return Err("Browser sync is disabled.".into());
        }
        let ticket = self
            .permissions
            .browser_ticket()
            .ok_or("Browser access was disabled.")?;
        let url = wire::web_url(request).ok_or("Invalid browser URL.")?;
        let (prepared, warning) = match crate::ssh::callback::endpoint(&url) {
            Ok(Some(endpoint)) => match self
                .callbacks
                .prepare_guarded(endpoint, self.permissions.clone())
                .await
            {
                Ok(()) => (true, None),
                Err(error) => (false, Some(error)),
            },
            Ok(None) => (false, None),
            Err(error) => (false, Some(error)),
        };
        let app = self.app.clone();
        // Preserve the original URL, including its fragment and escaped parameters.
        let original = request.to_owned();
        let permissions = self.permissions.clone();
        let result = tokio::task::spawn_blocking(move || {
            permissions
                .browser_at(ticket, || app.open_url(&original))
                .unwrap_or_else(|| Err("Browser access was disabled.".into()))
        })
        .await;
        if !matches!(result, Ok(Ok(()))) {
            if prepared {
                self.callbacks.rollback_last();
            }
            return Err("Could not open the browser on your computer.".into());
        }
        Ok(warning.map(|reason| format!("Browser opened without callback forwarding. {reason}")))
    }
    pub async fn request(&mut self, kind: u8, data: &[u8]) -> Result<String, String> {
        let frame = wire::encode(kind, data).map_err(|e| e.to_string())?;
        tokio::time::timeout(request_timeout(kind), async {
            self.writer.write_all(&frame).await.map_err(transport)?;
            self.writer.flush().await.map_err(transport)?;
            loop {
                let (kind, data) = self.receive().await?;
                match kind {
                    b'A' => return Ok(String::from_utf8_lossy(&data).into_owned()),
                    b'O' => self.open(&data).await?,
                    b'C' if self.deferred.len() < 8 => self.deferred.push_back((kind, data)),
                    _ => return Err("Unexpected agent event.".into()),
                }
            }
        })
        .await
        .map_err(|_| transport("agent request timed out"))?
    }
    pub async fn clipboard_reply(&mut self, data: &[u8]) -> Result<(), String> {
        let frame = wire::encode(b'D', data).map_err(|e| e.to_string())?;
        tokio::time::timeout(Duration::from_secs(5), async {
            self.writer.write_all(&frame).await.map_err(transport)?;
            self.writer.flush().await.map_err(transport)
        })
        .await
        .map_err(|_| transport("clipboard reply timed out"))?
    }
    pub async fn close(&mut self) {
        let _ = tokio::time::timeout(Duration::from_secs(2), async {
            self.writer
                .write_all(&wire::encode(b'Q', &[]).unwrap())
                .await?;
            self.writer.shutdown().await
        })
        .await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn agent_error_frames_preserve_retry_policy() {
        assert!(
            checked_event(b'T', b"previous agent is still closing".to_vec())
                .unwrap_err()
                .starts_with("SSH transport interrupted:")
        );
        assert!(
            checked_event(
                b'E',
                b"SSH transport interrupted: permission denied".to_vec()
            )
            .unwrap_err()
            .starts_with("Agent error:")
        );
        assert_eq!(checked_event(b'A', vec![]).unwrap(), (b'A', vec![]));
    }
    #[test]
    fn agent_services_keep_browser_and_clipboard_preferences_independent() {
        for (clipboard, browser) in [(true, false), (false, true), (true, true), (false, false)] {
            let command = Service::Integration {
                client: uuid::Uuid::nil(),
                clipboard,
                browser,
            }
            .command();
            assert_eq!(command.contains("--clipboard"), clipboard);
            assert_eq!(command.contains("--browser"), browser);
        }
        assert!(Service::Git.command().ends_with("git-rpc --stdio"));
    }
    #[tokio::test]
    async fn agent_events_handle_fragmentation_and_reject_oversized_frames() {
        let (mut source, mut destination) = tokio::io::duplex(16);
        let frame = wire::encode(b'O', b"https://example.com").unwrap();
        tokio::spawn(async move {
            for byte in frame {
                source.write_all(&[byte]).await.unwrap();
            }
        });
        let (kind, data) = read_event(&mut destination).await.unwrap();
        assert_eq!(kind, b'O');
        assert_eq!(data, b"https://example.com");
        assert!(
            read_event(&mut destination)
                .await
                .unwrap_err()
                .starts_with("SSH transport interrupted:")
        );
        let oversized = [b'M', 0, 0, 34, 0];
        assert!(
            read_event(&mut oversized.as_slice())
                .await
                .unwrap_err()
                .contains("limit")
        );
    }
}
