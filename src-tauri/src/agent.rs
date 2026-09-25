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
mod wire;

pub async fn install(session: &ExecSession) -> Result<String, String> {
    deploy(session, false).await
}
pub async fn reinstall(session: &ExecSession) -> Result<String, String> {
    deploy(session, true).await
}
async fn deploy(session: &ExecSession, force: bool) -> Result<String, String> {
    let platform = session.execute("uname -s; uname -m", None).await?;
    let mut lines = platform.lines();
    if lines.next() != Some("Linux") {
        return Err("The Porthop agent requires Linux.".into());
    }
    let binary: &[u8] = match lines.next() {
        Some("x86_64") => include_bytes!("../agents/porthop-agent-x86_64"),
        Some("aarch64" | "arm64") => include_bytes!("../agents/porthop-agent-aarch64"),
        _ => return Err("The Porthop agent supports x86_64 and ARM64 Linux servers.".into()),
    };
    let hash = Sha256::digest(binary)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>();
    if !force {
        let installed = session
            .execute(
                "sha256sum \"$HOME/.local/bin/porthop-agent\" 2>/dev/null || true",
                None,
            )
            .await?;
        if installed.split_whitespace().next() == Some(hash.as_str()) {
            return session
                .execute("\"$HOME/.local/bin/porthop-agent\" install", None)
                .await;
        }
    }
    let command = format!(
        "sh -c '{}' porthop-install {hash}",
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
    let len = u32::from_be_bytes(header[1..].try_into().unwrap()) as usize;
    if len > 8224 {
        return Err("Invalid agent event length.".into());
    }
    let mut data = vec![0; len];
    input.read_exact(&mut data).await.map_err(transport)?;
    Ok((header[0], data))
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
            return Err("Unsupported Porthop agent protocol.".into());
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
        let (id, request) = std::str::from_utf8(data)
            .ok()
            .and_then(|v| v.split_once('\n'))
            .filter(|(id, _)| id.parse::<u64>().is_ok())
            .ok_or("Invalid browser request.")?;
        let result = self.open_request(request).await;
        let reply = match result {
            Ok(Some(warning)) => format!("ok\n{warning}"),
            Ok(None) => "ok".to_owned(),
            Err(error) => error,
        };
        let frame =
            wire::encode(b'B', format!("{id}\n{reply}").as_bytes()).map_err(|e| e.to_string())?;
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
        let oversized = [b'O', 0, 0, 33, 0];
        assert!(read_event(&mut oversized.as_slice())
            .await
            .unwrap_err()
            .contains("length"));
    }
}
