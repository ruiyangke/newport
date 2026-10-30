// Deployment and the single, bidirectional SSH connection to the Linux agent.
use crate::agent_runtime::Error;
use crate::desktop_integration::Desktop;
use crate::remote::command;
use newport_ssh::{Link, Open};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

pub use crate::agent_runtime::protocol::integration as control;
use crate::agent_runtime::protocol::{clipboard::ExportOffer, clipboard_export as exports};
#[path = "agent_events.rs"]
mod incoming;
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
    /// A language server in a worktree; `envrc` is false when the user chose
    /// to start without a blocked `.envrc`.
    Lsp {
        root: String,
        server: String,
        envrc: bool,
    },
    /// Private adapters provide literal GUI session arguments. The backend
    /// owns registration, the pinned build and the worker lifetime lease.
    RemoteApps {
        arguments: Vec<String>,
    },
}
impl Service {
    pub(crate) fn capability(&self) -> (&'static str, u32) {
        match self {
            Self::Git => ("git", crate::git::protocol::VERSION),
            Self::Monitor => ("monitor", 1),
            Self::Integration { .. } => ("integration", control::VERSION),
            Self::Lsp { .. } => ("lsp", 1),
            Self::RemoteApps { .. } => ("remote-apps", 1),
        }
    }
    #[cfg(test)]
    fn command(&self) -> String {
        match self {
            Self::Git => "exec \"$HOME/.local/bin/newport-agent\" git-rpc --stdio".into(),
            Self::Monitor => "exec \"$HOME/.local/bin/newport-agent\" monitor --stdio".into(),
            Self::Integration {
                client,
                clipboard,
                browser,
            } => format!(
                "exec \"$HOME/.local/bin/newport-agent\" serve {client} --managed {} {}",
                if *clipboard { "--clipboard" } else { "" },
                if *browser { "--browser" } else { "" },
            ),
            Self::Lsp { .. } => unreachable!("language servers start only as registered workers"),
            Self::RemoteApps { .. } => {
                unreachable!("GUI sessions start only as registered workers")
            }
        }
    }
}
#[cfg(test)]
pub async fn launch(
    link: &Link,
    service: Service,
) -> Result<(russh::ChannelStream<russh::client::Msg>, String), String> {
    require_installed(link).await?;
    let channel = command::stream(link, &service.command()).await?;
    Ok((channel.into_stream(), String::new()))
}

pub(crate) async fn launch_worker_channel(
    link: &Link,
    service: Service,
    hello: &crate::agent_runtime::protocol::Hello,
    worker: uuid::Uuid,
    endpoint: Option<&crate::agent_runtime::protocol::worker_stream::Endpoint>,
) -> Result<crate::service_channel::Channel, Error> {
    use crate::agent_runtime::protocol::worker_stream as wire;
    hello
        .validate()
        .map_err(|e| Error::Unavailable(e.to_string()))?;
    let build = &hello.build_sha256;
    let runtime = &hello.instance_id;
    let name = service.capability().0;
    let args = worker_literal_arguments(service).map_err(Error::Unavailable)?;
    if let Some(endpoint) = endpoint {
        let open = wire::Open {
            version: wire::VERSION,
            runtime_id: runtime.clone(),
            worker_id: worker.to_string(),
            build_sha256: build.clone(),
            ticket: endpoint.ticket().into(),
            service: name.into(),
            arguments: args.clone(),
        };
        open.validate()
            .map_err(|_| Error::Unavailable("Invalid worker stream admission".into()))?;
        let path = endpoint.path.clone();
        if let Ok(channel) = link.open(Open::DirectStreamLocal { path }).await {
            // From this point admission may have consumed the ticket and
            // spawned the process. Never replay a failed/ambiguous exchange.
            let fate = channel.fate();
            return crate::service_channel::Channel::forwarded(channel, &open)
                .await
                .map_err(|error| fate.error().map_or(Error::Interrupted(error), Error::Link));
        }
        // Opening the forwarding channel sent no worker admission bytes.
        // Older/restricted SSH servers may still run the registered worker
        // through exec; session capacity errors are surfaced to the caller.
    }
    let quote = |value: &str| format!("'{}'", value.replace('\'', "'\"'\"'"));
    let socket = quote(&hello.worker_socket);
    let arguments = worker_command(name, args);
    // Activation can change the entry point while this runtime is still serving
    // admitted work, so launch the runtime's own retained, immutable build.
    let command = format!(
        "exec \"$HOME/.local/share/newport/needle/builds/{build}/newport-agent\" worker {build} {runtime} {socket} {worker} {arguments}"
    );
    Ok(command::stream(link, &command).await?.into())
}

#[cfg(test)]
fn worker_arguments(service: Service) -> Result<String, String> {
    let name = service.capability().0;
    Ok(worker_command(name, worker_literal_arguments(service)?))
}
fn worker_command(name: &str, arguments: Vec<String>) -> String {
    let mut command = name.to_owned();
    for arg in arguments {
        command.push(' ');
        command.push_str(&crate::ssh::shell_quote(&arg));
    }
    command
}
fn worker_literal_arguments(service: Service) -> Result<Vec<String>, String> {
    Ok(match service {
        Service::Git | Service::Monitor => vec![],
        Service::Integration { client, .. } => vec![client.to_string()],
        Service::Lsp {
            root,
            server,
            envrc,
        } => vec![
            root,
            server,
            if envrc { "envrc" } else { "skip-envrc" }.into(),
        ],
        Service::RemoteApps { arguments } => {
            if arguments.len() > 128
                || arguments
                    .iter()
                    .any(|arg| arg.contains('\0') || arg.len() > 16384)
                || arguments.iter().map(String::len).sum::<usize>() > 65536
            {
                return Err("Remote Apps worker arguments exceed limit".into());
            }
            arguments
        }
    })
}

/// Explicit install and reinstall for remote tests. Feature startup uses
/// `launch`, which never calls these operations.
#[cfg(test)]
pub async fn install(link: &Link, server: &crate::model::Server) -> Result<String, String> {
    crate::agent_lifecycle::manage_test(link, server, false, true).await
}
#[cfg(test)]
pub async fn reinstall(link: &Link, server: &crate::model::Server) -> Result<String, String> {
    crate::agent_lifecycle::manage_test(link, server, true, true).await
}

pub(crate) async fn is_installed(link: &Link) -> Result<bool, command::Error> {
    let present = command::stdout(
        link,
        "test -x \"$HOME/.local/bin/newport-agent\" && printf installed || true",
        None,
    )
    .await?;
    Ok(present.trim() == "installed")
}

/// What a feature that needs Needle answers on a server without it.
pub(crate) const NOT_INSTALLED: &str =
    "Needle is not installed. Open Overview to set up this server.";

pub(crate) async fn require_installed(link: &Link) -> Result<(), Error> {
    if !is_installed(link).await? {
        return Err(Error::NotInstalled);
    }
    Ok(())
}

/// Why the integration agent's stream failed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Failure {
    /// The stream broke or timed out, or the agent said to try again.
    Interrupted(String),
    /// The agent refused, or spoke out of protocol; trying again won't help.
    Refused(String),
}
impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Failure::Interrupted(cause) => write!(f, "SSH transport interrupted: {cause}"),
            Failure::Refused(message) => f.write_str(message),
        }
    }
}
impl From<String> for Failure {
    fn from(message: String) -> Self {
        Failure::Refused(message)
    }
}
impl From<&str> for Failure {
    fn from(message: &str) -> Self {
        Failure::Refused(message.into())
    }
}
impl From<Failure> for String {
    fn from(failure: Failure) -> Self {
        failure.to_string()
    }
}

pub struct Agent<W> {
    writer: W,
    events: incoming::Events,
    reader: tokio::task::JoinHandle<()>,
    app: Desktop,
    permissions: std::sync::Arc<crate::integration_permissions::Gate>,
    callbacks: crate::ssh::callback::Callbacks,
}
impl<W> Drop for Agent<W> {
    fn drop(&mut self) {
        self.events.exports.disconnect();
        self.reader.abort();
    }
}
async fn read_event(input: &mut (impl AsyncRead + Unpin)) -> Result<(u8, Vec<u8>), Failure> {
    let mut header = [0; 5];
    input.read_exact(&mut header).await.map_err(transport)?;
    let len = wire::payload_length(&header, exports::MAX_FRAME).map_err(|e| e.to_string())?;
    let mut data = vec![0; len];
    input.read_exact(&mut data).await.map_err(transport)?;
    let event = wire::decode(&data, exports::MAX_FRAME).map_err(|e| e.to_string())?;
    if event.0 != exports::FRAME && event.1.len() > 8224 {
        return Err(Failure::Refused("Agent event exceeds limit".into()));
    }
    Ok(event)
}
fn transport(error: impl std::fmt::Display) -> Failure {
    Failure::Interrupted(error.to_string())
}

pub(crate) fn checked_event(kind: u8, data: Vec<u8>) -> Result<(u8, Vec<u8>), Failure> {
    match kind {
        b'E' => Err(Failure::Refused(format!(
            "Agent error: {}",
            String::from_utf8_lossy(&data)
        ))),
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
    ) -> Result<Agent<impl AsyncWrite + Unpin>, Failure> {
        let (reader, writer) = tokio::io::split(stream);
        let (events, reader) = incoming::Events::start(reader);
        let mut agent = Agent {
            writer,
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
            return Err(Failure::Refused(
                "Unsupported Needle integration handshake.".into(),
            ));
        }
        let hello: control::Hello = control::decode(&data)
            .map_err(|_| "Update Needle to use live integration permissions.".to_owned())?;
        hello.validate().map_err(|e| e.to_string())?;
        Ok(agent)
    }
}
impl<W: AsyncWrite + Unpin> Agent<W> {
    pub async fn event(&mut self) -> Result<(u8, Vec<u8>), Failure> {
        self.events.next().await
    }
    async fn receive(&mut self) -> Result<(u8, Vec<u8>), Failure> {
        self.events.receive().await
    }
    pub async fn open(&mut self, data: &[u8]) -> Result<(), Failure> {
        let (id, request) = wire::parse_browser_request(data).map_err(|e| e.to_string())?;
        let result = self.open_request(&request).await;
        let (success, message) = match result {
            Ok(warning) => (true, warning),
            Err(error) => (false, Some(error)),
        };
        let reply =
            wire::browser_reply(id, success, message.as_deref()).map_err(|e| e.to_string())?;
        self.write_unacknowledged(b'B', &reply, "browser reply")
            .await
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
    pub async fn request(&mut self, kind: u8, data: &[u8]) -> Result<String, Failure> {
        if kind == exports::FRAME {
            return Err(Failure::Refused(
                "Negotiate clipboard exports before requesting data".into(),
            ));
        }
        self.request_inner(kind, data).await
    }
    async fn request_inner(&mut self, kind: u8, data: &[u8]) -> Result<String, Failure> {
        let frame = wire::encode(kind, data).map_err(|e| e.to_string())?;
        let mut request = self.events.request_guard()?;
        let result = tokio::time::timeout(request_timeout(kind), async {
            self.writer.write_all(&frame).await.map_err(transport)?;
            self.writer.flush().await.map_err(transport)?;
            loop {
                let (kind, data) = self.receive().await?;
                match kind {
                    b'A' => return Ok(String::from_utf8_lossy(&data).into_owned()),
                    b'O' => self.open(&data).await?,
                    b'C' | b'G' | b'L' | b'l' | b'K' | b'a' | exports::FRAME => {
                        self.events.defer(kind, data)?
                    }
                    _ => return Err(Failure::Refused("Unexpected agent event.".into())),
                }
            }
        })
        .await
        .map_err(|_| transport("agent request timed out"))
        .and_then(|result| result);
        if result.is_err() {
            self.events.exports.disconnect();
        } else {
            request.complete();
        }
        result
    }
    /// Opt in only after the pinned runtime advertises the protocol. This
    /// grants no export/focus authority and never writes the native clipboard.
    pub async fn enable_clipboard_exports(
        &mut self,
        runtime: &crate::agent_runtime::protocol::Hello,
    ) -> Result<(), Failure> {
        self.enable_clipboard_export_version(runtime, exports::VERSION)
            .await
    }
    /// Explicit v2 negotiation. A v1 runtime cannot authorize committed-copy
    /// reads; do not silently fall back and lose clipboard data on focus change.
    pub async fn enable_clipboard_copies(
        &mut self,
        runtime: &crate::agent_runtime::protocol::Hello,
    ) -> Result<(), Failure> {
        self.enable_clipboard_export_version(runtime, exports::COPY_VERSION)
            .await
    }
    async fn enable_clipboard_export_version(
        &mut self,
        runtime: &crate::agent_runtime::protocol::Hello,
        version: u32,
    ) -> Result<(), Failure> {
        runtime.validate().map_err(|e| e.to_string())?;
        runtime
            .require(exports::CAPABILITY, version)
            .map_err(|e| e.to_string())?;
        self.events
            .exports
            .enable_version(&runtime.instance_id, version)?;
        self.export_command(exports::Control::Enable { version })
            .await
    }
    /// Propose the exact live offer. Await the returned acknowledgment beside
    /// this connection's event pump; it grants reads, not native publication.
    pub async fn commit_clipboard_copy(
        &mut self,
        proposal: &crate::clipboard_export::Lease,
    ) -> Result<crate::clipboard_export::Commit, Failure> {
        self.maintain_clipboard_exports().await?;
        let (commit, command) = self.events.exports.commit(proposal)?;
        self.export_command(command).await?;
        Ok(commit)
    }
    /// Fetch one format of the accepted copy without requiring continued focus.
    /// The event pump and normal maintenance must keep running during the read.
    pub async fn read_clipboard_copy(
        &mut self,
        copy: &crate::clipboard_export::CommittedCopy,
        format: &str,
    ) -> Result<crate::clipboard_export::Read, Failure> {
        self.maintain_clipboard_exports().await?;
        let (read, command) = self.events.exports.read_copy(copy, format)?;
        self.export_command(command).await?;
        Ok(read)
    }
    /// Fence the owned copy immediately and release it on the same connection.
    /// A stale handle cannot release a newer copy.
    pub async fn release_clipboard_copy(
        &mut self,
        copy: &crate::clipboard_export::CommittedCopy,
    ) -> Result<(), Failure> {
        self.events.exports.release_copy(copy)?;
        self.maintain_clipboard_exports().await
    }
    pub fn clipboard_export_offer(&self) -> Option<&ExportOffer> {
        self.events.exports.offer()
    }
    pub fn clipboard_export_lease(&self) -> Option<crate::clipboard_export::Lease> {
        self.events.exports.lease()
    }
    pub fn clipboard_export_connection(&self) -> Option<crate::clipboard_export::Connection> {
        self.events.exports.connection()
    }
    pub async fn read_clipboard_export(
        &mut self,
        offer: &ExportOffer,
        format: &str,
    ) -> Result<crate::clipboard_export::Read, Failure> {
        self.maintain_clipboard_exports().await?;
        let (read, command) = self.events.exports.read(offer, format)?;
        self.export_command(command).await?;
        Ok(read)
    }
    pub async fn cancel_clipboard_export(&mut self, id: u64) -> Result<(), Failure> {
        self.events.exports.cancel(id);
        self.maintain_clipboard_exports().await
    }
    pub async fn disable_clipboard_exports(&mut self) -> Result<(), Failure> {
        if self.events.exports.enabled() {
            self.events.exports.begin_disable();
            self.export_command(exports::Control::Disable).await?;
            self.events.exports.disable();
        }
        Ok(())
    }
    pub(crate) async fn maintain_clipboard_exports(&mut self) -> Result<(), Failure> {
        self.events.exports.expire();
        if self
            .events
            .exports
            .connection_fence()
            .load(std::sync::atomic::Ordering::Acquire)
        {
            return Err(transport("clipboard connection retired; reconnect"));
        }
        for id in self.events.exports.cancellations() {
            self.export_command(exports::Control::Cancel { id }).await?;
            self.events.exports.cancelled(id);
        }
        if let Some(exports::Control::Release { copy }) = self.events.exports.copy_release() {
            let id = copy.id.clone();
            self.export_command(exports::Control::Release { copy })
                .await?;
            self.events.exports.copy_release_sent(&id);
        }
        Ok(())
    }
    async fn export_command(&mut self, command: exports::Control) -> Result<(), Failure> {
        self.request_inner(
            exports::FRAME,
            &command.encode().map_err(|e| e.to_string())?,
        )
        .await?;
        Ok(())
    }
    pub async fn clipboard_reply(&mut self, data: &[u8]) -> Result<(), Failure> {
        self.write_unacknowledged(b'D', data, "clipboard reply")
            .await
    }
    async fn write_unacknowledged(
        &mut self,
        kind: u8,
        data: &[u8],
        operation: &str,
    ) -> Result<(), Failure> {
        write_reply(&mut self.writer, &mut self.events, kind, data, operation).await
    }
    pub async fn close(&mut self) {
        self.events.exports.disconnect();
        let _ = tokio::time::timeout(Duration::from_secs(2), async {
            self.writer
                .write_all(&wire::encode(b'Q', &[]).unwrap())
                .await?;
            self.writer.shutdown().await
        })
        .await;
    }
}

async fn write_reply(
    writer: &mut (impl AsyncWrite + Unpin),
    events: &mut incoming::Events,
    kind: u8,
    data: &[u8],
    operation: &str,
) -> Result<(), Failure> {
    let frame = wire::encode(kind, data).map_err(|e| e.to_string())?;
    let mut request = events.request_guard()?;
    let result = tokio::time::timeout(Duration::from_secs(5), async {
        writer.write_all(&frame).await.map_err(transport)?;
        writer.flush().await.map_err(transport)
    })
    .await
    .map_err(|_| transport(format!("{operation} timed out")))
    .and_then(|result| result);
    if result.is_err() {
        events.exports.disconnect();
    } else {
        request.complete();
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test(start_paused = true)]
    async fn interrupted_reply_writes_cannot_leave_a_reusable_partial_frame() {
        let (_send, input) = tokio::sync::mpsc::channel(8);
        let mut events = incoming::Events::new(input);
        write_reply(
            &mut tokio::io::sink(),
            &mut events,
            b'B',
            b"ok",
            "browser reply",
        )
        .await
        .unwrap();
        let (mut output, _unread) = tokio::io::duplex(16);
        assert!(
            tokio::time::timeout(
                Duration::from_millis(1),
                write_reply(
                    &mut output,
                    &mut events,
                    b'D',
                    &[0; 1024],
                    "clipboard reply"
                )
            )
            .await
            .is_err()
        );
        assert!(events.request_guard().is_err());
        assert!(
            events
                .next()
                .await
                .unwrap_err()
                .to_string()
                .starts_with("SSH transport interrupted:")
        );
    }
    #[cfg(unix)]
    #[test]
    fn gui_worker_arguments_preserve_literal_boundaries() {
        let args = vec![
            "--display",
            "private",
            "--",
            "/app/with space",
            "a'b",
            "$(printf expanded)",
            "",
        ];
        let command = worker_arguments(Service::RemoteApps {
            arguments: args.iter().map(|s| s.to_string()).collect(),
        })
        .unwrap();
        let output = std::process::Command::new("sh")
            .args(["-c", &format!("printf '%s\\0' {command}")])
            .output()
            .unwrap();
        assert!(output.status.success());
        let bytes = output.stdout.strip_suffix(&[0]).unwrap();
        let parsed: Vec<_> = bytes
            .split(|b| *b == 0)
            .map(|value| std::str::from_utf8(value).unwrap())
            .collect();
        assert_eq!(parsed[0], "remote-apps");
        assert_eq!(&parsed[1..], args.as_slice());
    }
    #[test]
    fn gui_worker_arguments_reject_oversized_and_invalid_values() {
        assert!(
            worker_arguments(Service::RemoteApps {
                arguments: vec!["literal argument".into()],
            })
            .is_ok()
        );
        for arguments in [
            vec!["arg".into(); 129],
            vec!["x".repeat(16385)],
            vec!["\0".into()],
            vec!["x".repeat(16384); 5],
        ] {
            assert!(worker_arguments(Service::RemoteApps { arguments }).is_err());
        }
    }
    #[test]
    fn agent_error_frames_preserve_retry_policy() {
        assert!(
            checked_event(b'T', b"previous agent is still closing".to_vec())
                .unwrap_err()
                .to_string()
                .starts_with("SSH transport interrupted:")
        );
        assert!(
            checked_event(
                b'E',
                b"SSH transport interrupted: permission denied".to_vec()
            )
            .unwrap_err()
            .to_string()
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
                .to_string()
                .starts_with("SSH transport interrupted:")
        );
        let mut oversized = vec![b'M'];
        oversized.extend(((exports::MAX_FRAME + 17) as u32).to_be_bytes());
        assert!(
            read_event(&mut oversized.as_slice())
                .await
                .unwrap_err()
                .to_string()
                .contains("limit")
        );
        let oversized_legacy = wire::encode(b'O', &vec![0; 8225]).unwrap();
        assert!(
            read_event(&mut oversized_legacy.as_slice())
                .await
                .unwrap_err()
                .to_string()
                .contains("limit")
        );
        let export = exports::Event::Data {
            id: 1,
            offset: 0,
            chunk: Some(crate::agent_runtime::protocol::clipboard::Chunk {
                bytes: vec![7; exports::CHUNK],
                eof: true,
            }),
        }
        .encode()
        .unwrap();
        let frame = wire::encode(exports::FRAME, &export).unwrap();
        assert_eq!(
            read_event(&mut frame.as_slice()).await.unwrap(),
            (exports::FRAME, export)
        );
    }
}
