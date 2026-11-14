// Deployment and the single, bidirectional SSH connection to the Linux agent.
use crate::agent_runtime::Error;
use crate::desktop_integration::Desktop;
use crate::remote::command;
use newport_ssh::{Link, Open};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt};

use newport_protocol::clipboard::ExportOffer;
use newport_protocol::clipboard::exports;
use newport_protocol::encoding::protobuf::{FrameError, tokio::FrameReader};
use newport_protocol::integration::{
    self, BrowserReply, BrowserRequest, Call, ClientMessage, Reply, Request, Response,
    ServerMessage, wire::Message,
};
use newport_protocol::runtime::service;
#[path = "agent_events.rs"]
mod incoming;

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
    /// Starts, lists and ends the holders of persistent terminals; see
    /// crate::terminal::holder_channel. Each request is short.
    Terminal,
}
impl Service {
    fn transport(&self) -> newport_protocol::worker::Transport {
        use newport_protocol::worker::Transport;
        match self {
            Self::Git => Transport::GIT,
            Self::Monitor => Transport::MONITOR,
            Self::Integration { .. } => Transport::INTEGRATION,
            Self::Lsp { .. } => Transport::Opaque,
            Self::RemoteApps { .. } => Transport::REMOTE_APPS,
            Self::Terminal => Transport::TERMINAL,
        }
    }
    pub(crate) fn capability(&self) -> (&'static str, u32) {
        match self {
            Self::Git => (service::GIT, newport_protocol::git::VERSION),
            Self::Monitor => (service::MONITOR, newport_protocol::monitor::VERSION),
            Self::Integration { .. } => (service::INTEGRATION, integration::VERSION),
            Self::Lsp { .. } => (service::LSP, 1),
            Self::RemoteApps { .. } => (service::REMOTE_APPS, 1),
            Self::Terminal => (service::TERMINAL, newport_protocol::terminal::VERSION),
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
            Self::Terminal => unreachable!("terminal holders start only as registered workers"),
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
    hello: &newport_protocol::runtime::Hello,
    worker: uuid::Uuid,
    endpoint: Option<&newport_protocol::worker::admission::Endpoint>,
) -> Result<crate::service_channel::Channel, Error> {
    use newport_protocol::worker::admission as wire;
    hello
        .validate()
        .map_err(|e| Error::Unavailable(e.to_string()))?;
    let build = &hello.build_sha256;
    let runtime = &hello.instance_id;
    let (name, version) = service.capability();
    hello
        .require_contract(
            name,
            version,
            newport_protocol::runtime::service_contract_id(name),
        )
        .map_err(Error::Remote)?;
    let transport = service.transport();
    let args = worker_literal_arguments(service).map_err(Error::Unavailable)?;
    if let Some(endpoint) = endpoint {
        let open = wire::Open {
            version: wire::VERSION,
            runtime_id: runtime.clone(),
            worker_id: worker.to_string(),
            build_sha256: build.clone(),
            ticket: endpoint.ticket.clone(),
            service: name.into(),
            service_version: version,
            contract_id: newport_protocol::runtime::service_contract_id(name).map(str::to_owned),
            arguments: args.clone(),
        };
        open.validate()
            .map_err(|_| Error::Unavailable("Invalid worker stream admission".into()))?;
        let path = endpoint.path.clone();
        if let Ok(channel) = link.open(Open::DirectStreamLocal { path }).await {
            // From this point admission may have consumed the ticket and
            // spawned the process. Never replay a failed/ambiguous exchange.
            return crate::service_channel::Channel::forwarded(channel, &open, transport)
                .await
                .map_err(worker_admission_error);
        }
        // Opening the forwarding channel sent no worker admission bytes.
        // SSH servers that refuse stream-local forwarding still run the
        // registered worker through exec; session capacity errors are
        // surfaced to the caller.
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

fn worker_admission_error(error: crate::service_channel::AdmissionError) -> Error {
    match error {
        crate::service_channel::AdmissionError::Refused(error) => Error::Remote(error),
        error => Error::Admission(std::sync::Arc::new(error)),
    }
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
        Service::Git | Service::Monitor | Service::Terminal => vec![],
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    Response(Response),
    Browser(BrowserRequest),
    ClipboardRequest(newport_protocol::clipboard::imports::Request),
    GuiNotice,
    ExportsChanged,
}
pub struct Agent<W> {
    writer: W,
    request_id: u64,
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
async fn read_event(
    input: &mut FrameReader<impl AsyncRead + Unpin>,
) -> Result<ServerMessage, Failure> {
    let body = input
        .next()
        .await
        .map_err(|error| match error {
            FrameError::Io(_) | FrameError::Truncated => transport(error),
            _ => Failure::Refused(error.to_string()),
        })?
        .ok_or_else(|| transport("agent exited"))?;
    ServerMessage::decode(body).map_err(|error| Failure::Refused(error.to_string()))
}
fn transport(error: impl std::fmt::Display) -> Failure {
    Failure::Interrupted(error.to_string())
}
pub(crate) fn failure(error: newport_protocol::common::Error) -> Failure {
    match error.retry {
        newport_protocol::common::Retry::Yes => transport(error.message),
        _ => Failure::Refused(format!("Agent error: {}", error.message)),
    }
}
fn request_timeout(request: &Request) -> Duration {
    Duration::from_secs(if matches!(request, Request::Snapshot(_)) {
        120
    } else {
        25
    })
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
            request_id: 0,
            events,
            reader,
            app,
            permissions,
            callbacks,
        };
        agent.request(Request::Heartbeat).await?;
        Ok(agent)
    }
}
impl<W: AsyncWrite + Unpin> Agent<W> {
    pub async fn event(&mut self) -> Result<Event, Failure> {
        self.events.next().await
    }
    async fn receive(&mut self) -> Result<Event, Failure> {
        self.events.receive().await
    }
    pub async fn open(&mut self, request: BrowserRequest) -> Result<(), Failure> {
        request.validate().map_err(|error| error.to_string())?;
        let result = self.open_request(&request.url).await;
        let (success, message) = match result {
            Ok(warning) => (true, warning),
            Err(error) => (false, Some(error)),
        };
        self.write_unacknowledged(
            ClientMessage::Browser(BrowserReply {
                id: request.id,
                success,
                message,
            }),
            "browser reply",
        )
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
        let url = newport_protocol::browser::web_url(request).ok_or("Invalid browser URL.")?;
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
    pub async fn request(&mut self, request: Request) -> Result<Reply, Failure> {
        if matches!(request, Request::Export(_)) {
            return Err(Failure::Refused(
                "Negotiate clipboard exports before requesting data".into(),
            ));
        }
        self.request_inner(request).await
    }
    async fn request_inner(&mut self, request: Request) -> Result<Reply, Failure> {
        self.request_id = self
            .request_id
            .checked_add(1)
            .ok_or("Agent request IDs exhausted")?;
        let call = Call {
            id: self.request_id,
            request,
        };
        let frame = ClientMessage::Call(call.clone())
            .encode()
            .map_err(|error| error.to_string())?;
        let mut guard = self.events.request_guard()?;
        let result = tokio::time::timeout(request_timeout(&call.request), async {
            self.writer.write_all(&frame).await.map_err(transport)?;
            self.writer.flush().await.map_err(transport)?;
            loop {
                match self.receive().await? {
                    Event::Response(response) => {
                        response
                            .validate_for(&call)
                            .map_err(|error| error.to_string())?;
                        return match response.reply {
                            Reply::Failed(error) => Err(failure(error)),
                            reply => Ok(reply),
                        };
                    }
                    Event::Browser(request) => self.open(request).await?,
                    event => self.events.defer(event)?,
                }
            }
        })
        .await
        .map_err(|_| transport("agent request timed out"))
        .and_then(|result| result);
        if result.is_err() {
            self.events.exports.disconnect();
        } else {
            guard.complete();
        }
        result
    }
    /// Opt in only after the pinned runtime advertises the protocol. This
    /// grants no export/focus authority and never writes the native clipboard.
    pub async fn enable_clipboard_copies(
        &mut self,
        runtime: &newport_protocol::runtime::Hello,
    ) -> Result<(), Failure> {
        runtime.validate().map_err(|e| e.to_string())?;
        runtime
            .require(exports::CAPABILITY, exports::VERSION)
            .map_err(|e| e.to_string())?;
        self.events.exports.enable(&runtime.instance_id)?;
        self.export_command(exports::Control::Enable {
            version: exports::VERSION,
        })
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
        self.request_inner(Request::Export(command)).await?;
        Ok(())
    }
    pub async fn clipboard_reply(
        &mut self,
        reply: newport_protocol::clipboard::imports::Reply,
    ) -> Result<(), Failure> {
        self.write_unacknowledged(ClientMessage::Clipboard(reply), "clipboard reply")
            .await
    }
    async fn write_unacknowledged(
        &mut self,
        message: ClientMessage,
        operation: &str,
    ) -> Result<(), Failure> {
        write_reply(&mut self.writer, &mut self.events, message, operation).await
    }
    pub async fn close(&mut self) {
        self.events.exports.disconnect();
        let _ = tokio::time::timeout(Duration::from_secs(2), async {
            self.writer
                .write_all(&ClientMessage::Quit.encode().unwrap())
                .await?;
            self.writer.shutdown().await
        })
        .await;
    }
}

async fn write_reply(
    writer: &mut (impl AsyncWrite + Unpin),
    events: &mut incoming::Events,
    message: ClientMessage,
    operation: &str,
) -> Result<(), Failure> {
    let frame = message.encode().map_err(|e| e.to_string())?;
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
    #[test]
    fn worker_admission_keeps_remote_retry_and_uncertain_transport_causes() {
        use crate::service_channel::AdmissionError;
        use newport_protocol::common::{Error as RemoteError, Retry};
        use std::error::Error as _;

        let invalid = || {
            newport_protocol::runtime::Permissions {
                revision: 0,
                clipboard: false,
                browser: false,
            }
            .validate()
            .unwrap_err()
        };

        for retry in [Retry::No, Retry::Yes, Retry::Unknown] {
            let remote = RemoteError {
                code: "unavailable".into(),
                message: "Worker process could not start".into(),
                retry,
            };
            let error = worker_admission_error(AdmissionError::Refused(remote.clone()));
            assert_eq!(error, Error::Remote(remote.clone()));
            assert_eq!(error.transient(), retry == Retry::Yes);
            assert_eq!(
                error.source().unwrap().downcast_ref::<RemoteError>(),
                Some(&remote)
            );
            assert_eq!(crate::rpc::failure::runtime(error).error, remote);
        }

        let error = worker_admission_error(AdmissionError::Read(std::io::Error::from(
            std::io::ErrorKind::ConnectionReset,
        )));
        assert!(error.transient());
        assert_eq!(error, error.clone());
        let source = error
            .source()
            .unwrap()
            .downcast_ref::<AdmissionError>()
            .unwrap();
        assert_eq!(
            source
                .source()
                .unwrap()
                .downcast_ref::<std::io::Error>()
                .unwrap()
                .kind(),
            std::io::ErrorKind::ConnectionReset,
        );
        assert_eq!(
            crate::rpc::failure::runtime(error).error.retry,
            Retry::Unknown
        );

        for source in [
            AdmissionError::Frame(FrameError::Malformed),
            AdmissionError::Identity(invalid()),
        ] {
            let error = worker_admission_error(source);
            assert!(!error.transient());
            assert!(error.source().unwrap().source().is_some());
            assert_eq!(
                crate::rpc::failure::runtime(error).error.retry,
                Retry::Unknown
            );
        }
        let error = worker_admission_error(AdmissionError::Encode(
            newport_protocol::runtime::wire::Error::Invalid(invalid()),
        ));
        assert!(!error.transient());
        assert_eq!(crate::rpc::failure::runtime(error).error.retry, Retry::No);
    }

    #[tokio::test(start_paused = true)]
    async fn interrupted_reply_writes_cannot_leave_a_reusable_partial_frame() {
        let (_send, input) = tokio::sync::mpsc::channel(8);
        let mut events = incoming::Events::new(input);
        write_reply(
            &mut tokio::io::sink(),
            &mut events,
            ClientMessage::Browser(BrowserReply {
                id: 1,
                success: true,
                message: None,
            }),
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
                    ClientMessage::Clipboard(newport_protocol::clipboard::imports::Reply {
                        id: 1,
                        revision: 1,
                        encoding: newport_protocol::clipboard::imports::Encoding::Raw,
                        done: true,
                        bytes: vec![0; 1024],
                    }),
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
            "--attach",
            "--computer",
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
    fn agent_errors_preserve_explicit_retry_policy() {
        use newport_protocol::common::{Error, Retry};
        assert!(matches!(
            failure(Error {
                code: "closing".into(),
                message: "previous agent is still closing".into(),
                retry: Retry::Yes
            }),
            Failure::Interrupted(_)
        ));
        assert!(matches!(
            failure(Error {
                code: "permission".into(),
                message: "SSH transport interrupted: permission denied".into(),
                retry: Retry::No
            }),
            Failure::Refused(_)
        ));
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
    async fn agent_events_handle_fragmentation_read_ahead_and_oversized_frames() {
        let (mut source, destination) = tokio::io::duplex(16);
        let browser = ServerMessage::Browser(BrowserRequest {
            id: 1,
            url: "https://example.com".into(),
        });
        let frame = browser.encode().unwrap();
        tokio::spawn(async move {
            for byte in frame {
                source.write_all(&[byte]).await.unwrap();
            }
            source
                .write_all(&ServerMessage::GuiNotice.encode().unwrap())
                .await
                .unwrap();
        });
        let mut input = FrameReader::new(destination, integration::SERVER_LIMITS);
        assert_eq!(read_event(&mut input).await.unwrap(), browser);
        assert_eq!(
            read_event(&mut input).await.unwrap(),
            ServerMessage::GuiNotice
        );
        assert!(matches!(
            read_event(&mut input).await,
            Err(Failure::Interrupted(_))
        ));
        let mut oversized = vec![];
        newport_protocol::encoding::protobuf::write_frame(
            &mut oversized,
            &vec![0; integration::SERVER_LIMITS.message + 1],
        )
        .unwrap();
        let mut input = FrameReader::new(oversized.as_slice(), integration::SERVER_LIMITS);
        assert!(
            read_event(&mut input)
                .await
                .unwrap_err()
                .to_string()
                .contains("limit")
        );
        let export = ServerMessage::Export(exports::Event::Data {
            id: 1,
            offset: 0,
            chunk: Some(newport_protocol::clipboard::Chunk {
                bytes: vec![7; exports::CHUNK],
                eof: true,
            }),
        });
        let frame = export.encode().unwrap();
        let mut input = FrameReader::new(frame.as_slice(), integration::SERVER_LIMITS);
        assert_eq!(read_event(&mut input).await.unwrap(), export);
    }
}
