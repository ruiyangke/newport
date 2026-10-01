// Deployment and the single, bidirectional SSH connection to the Linux agent.
use crate::ssh::ExecSession;
use sha2::{Digest, Sha256};
use std::time::Duration;
use tauri_plugin_opener::OpenerExt;
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    sync::mpsc,
};

#[allow(dead_code)]
#[path = "../../tools/agent/src/wire.rs"]
pub(crate) mod wire;

/// All agent-backed features enter here before opening a protocol channel.
/// Recheck every new channel so removal or an external replacement is repaired.
pub enum Service {
    Git,
    Integration {
        client: uuid::Uuid,
        clipboard: bool,
        browser: bool,
    },
}
impl Service {
    fn command(&self) -> String {
        match self {
            Self::Git => "exec env NEWPORT_GIT_BACKEND=cli \"$HOME/.local/bin/newport-agent\" git-rpc --stdio".into(),
            Self::Integration { client, clipboard, browser } => format!(
                "exec \"$HOME/.local/bin/newport-agent\" serve {client} {} {}",
                if *clipboard { "--clipboard" } else { "" },
                if *browser { "--browser" } else { "" },
            ),
        }
    }
}
pub async fn launch(
    session: &ExecSession,
    service: Service,
) -> Result<(russh::ChannelStream<russh::client::Msg>, String), String> {
    let installed = install(session).await?;
    let stream = session.stream(&service.command()).await?;
    Ok((stream, installed))
}

pub async fn install(session: &ExecSession) -> Result<String, String> {
    deploy(session, false).await
}
pub async fn reinstall(session: &ExecSession) -> Result<String, String> {
    deploy(session, true).await
}
// Installation and explicit reinstalls share the same per-profile lock. Weak
// entries avoid retaining every server ever visited; failures are never cached.
fn installation_lock(server: uuid::Uuid) -> std::sync::Arc<tokio::sync::Mutex<()>> {
    use std::sync::{Arc, LazyLock, Mutex, Weak};
    static LOCKS: LazyLock<
        Mutex<std::collections::HashMap<uuid::Uuid, Weak<tokio::sync::Mutex<()>>>>,
    > = LazyLock::new(Mutex::default);
    let mut locks = LOCKS.lock().expect("agent installation locks");
    locks.retain(|_, lock| lock.strong_count() > 0);
    if let Some(lock) = locks.get(&server).and_then(Weak::upgrade) {
        return lock;
    }
    let lock = Arc::new(tokio::sync::Mutex::new(()));
    locks.insert(server, Arc::downgrade(&lock));
    lock
}
async fn deploy(session: &ExecSession, force: bool) -> Result<String, String> {
    let lock = installation_lock(session.server_id());
    let _installing = lock.lock().await;
    let platform = session.execute("uname -s; uname -m", None).await?;
    let mut lines = platform.lines();
    if lines.next() != Some("Linux") {
        return Err("The Newport agent requires Linux.".into());
    }
    let binary: &[u8] = match lines.next() {
        Some("x86_64") => include_bytes!("../agents/newport-agent-x86_64"),
        Some("aarch64" | "arm64") => include_bytes!("../agents/newport-agent-aarch64"),
        _ => return Err("The Newport agent supports x86_64 and ARM64 Linux servers.".into()),
    };
    let hash = Sha256::digest(binary)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>();
    if !force {
        let installed = session
            .execute(
                "test -x \"$HOME/.local/bin/newport-agent\" && sha256sum \"$HOME/.local/bin/newport-agent\" 2>/dev/null || true",
                None,
            )
            .await?;
        if installed.split_whitespace().next() == Some(hash.as_str()) {
            return session
                .execute("\"$HOME/.local/bin/newport-agent\" install", None)
                .await;
        }
    }
    let command = format!(
        "sh -c '{}' newport-install {hash}",
        include_str!("agent-install.sh").replace('\'', "'\"'\"'")
    );
    session.execute(&command, Some(binary)).await
}

pub struct Agent<W> {
    writer: W,
    deferred: std::collections::VecDeque<(u8, Vec<u8>)>,
    events: mpsc::Receiver<Result<(u8, Vec<u8>), String>>,
    reader: tokio::task::JoinHandle<()>,
    app: tauri::AppHandle,
    browser_enabled: bool,
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
        app: tauri::AppHandle,
        browser_enabled: bool,
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
            browser_enabled,
            callbacks,
        };
        let (kind, data) = tokio::time::timeout(Duration::from_secs(25), agent.event())
            .await
            .map_err(|_| transport("agent startup timed out"))??;
        if kind != b'R' || data != wire::VERSION.as_bytes() {
            return Err("Unsupported Newport agent protocol.".into());
        }
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
        if !self.browser_enabled {
            return Err("Browser sync is disabled.".into());
        }
        let url = wire::web_url(request).ok_or("Invalid browser URL.")?;
        let (prepared, warning) = match crate::ssh::callback::endpoint(&url) {
            Ok(Some(endpoint)) => match self.callbacks.prepare(endpoint).await {
                Ok(()) => (true, None),
                Err(error) => (false, Some(error)),
            },
            Ok(None) => (false, None),
            Err(error) => (false, Some(error)),
        };
        let app = self.app.clone();
        // Preserve the original URL, including its fragment and escaped parameters.
        let original = request.to_owned();
        let result =
            tokio::task::spawn_blocking(move || app.opener().open_url(original, None::<&str>))
                .await;
        if !matches!(result, Ok(Ok(()))) {
            if prepared {
                self.callbacks.rollback_last();
            }
            return Err("Could not open the browser on your Mac.".into());
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
        assert!(checked_event(
            b'E',
            b"SSH transport interrupted: permission denied".to_vec()
        )
        .unwrap_err()
        .starts_with("Agent error:"));
        assert_eq!(checked_event(b'A', vec![]).unwrap(), (b'A', vec![]));
    }
    #[tokio::test]
    async fn installations_share_a_server_lock_without_blocking_other_servers() {
        let server = uuid::Uuid::new_v4();
        let first = installation_lock(server);
        let second = installation_lock(server);
        let other = installation_lock(uuid::Uuid::new_v4());
        let held = first.lock().await;
        assert!(second.try_lock().is_err());
        assert!(other.try_lock().is_ok());
        drop(held);
        assert!(second.try_lock().is_ok());
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
        assert!(read_event(&mut destination)
            .await
            .unwrap_err()
            .starts_with("SSH transport interrupted:"));
        let oversized = [b'M', 0, 0, 34, 0];
        assert!(read_event(&mut oversized.as_slice())
            .await
            .unwrap_err()
            .contains("limit"));
    }
}
