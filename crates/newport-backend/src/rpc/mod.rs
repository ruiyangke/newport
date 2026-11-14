//! Bounded local RPC transport between the UI and the backend. This module
//! owns framing, consent progress and connection-held resources, never UI state.
pub mod account;
pub(crate) mod configuration;
pub mod control;
pub(crate) mod failure;
pub mod features;
mod files;
pub mod git;
pub mod graphical;
mod lsp_stream;
mod metrics;
pub mod preferences;
mod progress;
mod projection;
mod remote_stream;
pub mod services;
mod terminal;
mod terminal_lease;
mod transport;
use newport_platform::ipc::asynchronous::{Listener, Stream};
use newport_protocol::{
    common::Retry,
    desktop::{
        self, ClientMessage, Exchange, Failure, Operation, Outcome, Ready, Reply, Request,
        Response, ServerMessage, wire,
    },
};
use std::{
    collections::VecDeque,
    future::Future,
    io,
    path::Path,
    sync::{Arc, Mutex},
};
use tokio::{
    io::{AsyncWrite, AsyncWriteExt},
    sync::Semaphore,
    time::Instant,
};
use uuid::Uuid;

pub type TerminalOpen = desktop::stream::TerminalOpenArgs;
pub type LspOpen = desktop::stream::LspOpenArgs;

/// Application-owned services; only the explicitly typed desktop contract
/// crosses this boundary. Opening transfers the guard into one connection.
pub trait Host: Send + Sync + 'static {
    fn instance_id(&self) -> Uuid;
    fn dispatch(
        &self,
        operation: &Operation,
    ) -> impl Future<Output = Result<Reply, Failure>> + Send;
    fn open_terminal(
        &self,
        request: TerminalOpen,
    ) -> impl Future<Output = Result<TerminalChannel, Failure>> + Send;
    fn open_remote_stream(
        &self,
        _server: Uuid,
        _command: String,
    ) -> impl Future<Output = Result<RemoteChannel, Failure>> + Send {
        async { Err(failure::operation("Remote streams aren’t available.")) }
    }
    fn open_remote_apps(
        &self,
        _server: Uuid,
        _arguments: Vec<String>,
    ) -> impl Future<Output = Result<RemoteChannel, Failure>> + Send {
        async { Err(failure::operation("Remote Apps workers aren’t available.")) }
    }
    fn link_status(
        &self,
        _server: Uuid,
    ) -> Option<tokio::sync::watch::Receiver<newport_ssh::Status>> {
        None
    }
    fn open_lsp(
        &self,
        _request: LspOpen,
    ) -> impl Future<Output = Result<LspChannel, Failure>> + Send {
        async { Err(failure::operation("Language servers aren’t available.")) }
    }
}

pub trait LspIo: tokio::io::AsyncRead + tokio::io::AsyncWrite + Send + Unpin {}
impl<T: tokio::io::AsyncRead + tokio::io::AsyncWrite + Send + Unpin> LspIo for T {}

/// A started language server worker, and whatever keeps its connection open.
pub struct LspChannel {
    pub stream: Box<dyn LspIo>,
    pub guard: Box<dyn Send>,
}

/// An open terminal's I/O, and the guard that closes it when dropped.
pub struct TerminalChannel {
    pub io: crate::terminal::Io,
    pub guard: Box<dyn Send>,
}

/// A started remote command, and whatever keeps its connection open.
pub struct RemoteChannel {
    pub channel: crate::service_channel::Channel,
    pub guard: Box<dyn Send>,
}

#[derive(Default)]
struct Events {
    next: u64,
    rows: VecDeque<(u64, desktop::events::Event)>,
}
#[derive(Clone, Default)]
pub struct EventLog(Arc<Mutex<Events>>);
impl EventLog {
    pub fn record(&self, value: desktop::events::Event) {
        let mut events = self.0.lock().unwrap_or_else(|e| e.into_inner());
        // Exhaustion must never reuse a cursor and misidentify a later event.
        let Some(id) = events.next.checked_add(1) else {
            return;
        };
        events.next = id;
        events.rows.push_back((id, value));
        while events.rows.len() > 100 {
            events.rows.pop_front();
        }
    }
    fn after(&self, cursor: u64) -> desktop::events::Batch {
        let events = self.0.lock().unwrap_or_else(|e| e.into_inner());
        desktop::events::Batch {
            cursor: events.next,
            entries: events
                .rows
                .iter()
                .filter(|(id, _)| *id > cursor)
                .map(|(_, value)| value.clone())
                .collect(),
        }
    }
}

pub struct Server {
    listener: Listener,
    events: EventLog,
}
impl Server {
    /// Caller owns the profile instance lock. Must run inside its Tokio runtime.
    pub fn bind_owned(endpoint: impl AsRef<Path>) -> std::io::Result<Self> {
        Ok(Self {
            listener: Listener::bind_owned(endpoint)?,
            events: EventLog::default(),
        })
    }
    /// Refuse to replace an existing endpoint (for isolated test hosts).
    pub fn bind(endpoint: impl AsRef<Path>) -> std::io::Result<Self> {
        Ok(Self {
            listener: Listener::bind(endpoint)?,
            events: EventLog::default(),
        })
    }
    pub fn events(&self) -> EventLog {
        self.events.clone()
    }
    pub async fn run<H: Host>(self, host: H) -> std::io::Result<()> {
        self.run_until(host, std::future::pending()).await
    }

    /// Stop accepting requests, cancel connection-held work, and wait for its
    /// guards to drop before the application closes the underlying services.
    pub async fn run_until<H: Host>(
        mut self,
        host: H,
        shutdown: impl Future<Output = ()> + Send,
    ) -> std::io::Result<()> {
        let host = Arc::new(host);
        // Keep attachments available while ordinary calls wait for consent.
        let slots = Arc::new(Semaphore::new(desktop::CONNECTION_SLOTS));
        let admission = Admission {
            commands: Arc::new(Semaphore::new(desktop::OPERATION_SLOTS)),
            revocations: Arc::new(Semaphore::new(desktop::REVOCATION_SLOTS)),
            large_payloads: Arc::new(Semaphore::new(desktop::LARGE_REQUEST_SLOTS)),
        };
        // Over capacity, a connection still learns it was refused.
        let refusals = Arc::new(Semaphore::new(desktop::REFUSAL_SLOTS));
        let mut connections = tokio::task::JoinSet::new();
        tokio::pin!(shutdown);
        let result = loop {
            let stream = tokio::select! {
                biased;
                _ = &mut shutdown => break Ok(()),
                _ = connections.join_next(), if !connections.is_empty() => continue,
                result = self.listener.accept() => match result {
                    Ok(stream) => stream,
                    Err(error) => break Err(error),
                },
            };
            let Ok(permit) = slots.clone().try_acquire_owned() else {
                if let Ok(permit) = refusals.clone().try_acquire_owned() {
                    connections.spawn(async move {
                        let _permit = permit;
                        if let Err(error) = refuse(stream).await {
                            log::debug!("Client disconnected: {error}");
                        }
                    });
                }
                continue;
            };
            let host = host.clone();
            let events = self.events.clone();
            let admission = admission.clone();
            connections.spawn(async move {
                let _permit = permit;
                if let Err(error) = serve(stream, host, events, admission).await {
                    log::debug!("Client disconnected: {error}");
                }
            });
        };
        connections.shutdown().await;
        result
    }
}

/// Separate capacity per kind of request, so one kind cannot starve another.
#[derive(Clone)]
struct Admission {
    commands: Arc<Semaphore>,
    revocations: Arc<Semaphore>,
    large_payloads: Arc<Semaphore>,
}

async fn serve<H: Host>(
    stream: Stream,
    host: Arc<H>,
    events: EventLog,
    admission: Admission,
) -> anyhow::Result<()> {
    let deadline = Instant::now() + desktop::ADMISSION_TIMEOUT;
    let (input, mut writer) = tokio::io::split(stream);
    let mut reader = transport::Reader::new(input);
    let mut exchange = Exchange::default();
    let hello = frame_until(
        &mut reader,
        desktop::HANDSHAKE_LIMITS.message,
        None,
        deadline,
    )
    .await?;
    let hello = wire::decode_hello(hello.frame.body())?;
    let compatible = desktop::require_contract_id(hello.contract_id.as_deref());
    exchange.client(&ClientMessage::Hello(Box::new(hello)))?;
    if let Err(error) = compatible {
        let refused = ServerMessage::Refused(Box::new(error));
        exchange.server(&refused)?;
        transport::write_until(&mut writer, &wire::encode_server(&refused)?, deadline).await?;
        tokio::time::timeout_at(deadline, writer.shutdown()).await??;
        return Ok(());
    }
    let ready = ServerMessage::Ready(Box::new(Ready {
        version: desktop::VERSION,
        backend_instance: host.instance_id(),
        contract_id: Some(desktop::CONTRACT_ID.into()),
    }));
    exchange.server(&ready)?;
    transport::write_until(&mut writer, &wire::encode_server(&ready)?, deadline).await?;
    let received = frame_until(
        &mut reader,
        desktop::MAX_SAVE_REQUEST,
        Some(admission.large_payloads.clone()),
        deadline,
    )
    .await?;
    let _large_permit = received.large;
    let message = wire::decode_client(received.frame.body(), _large_permit.is_some())?;
    drop(received.frame); // Decoded DTOs own their payload; release the framing allocation.
    exchange.client(&message)?;
    let ClientMessage::Request(request) = message else {
        anyhow::bail!("Expected desktop request");
    };
    if reader.has_buffered() {
        anyhow::bail!("Data preceded desktop response");
    }
    if let Operation::Stream(operation) = &request.operation {
        return match operation.as_ref() {
            desktop::stream::Operation::TerminalOpen(_) => {
                terminal_lease::serve(reader, writer, host.as_ref(), &request).await
            }
            desktop::stream::Operation::CommandOpen(_)
            | desktop::stream::Operation::RemoteAppsOpen(_) => {
                remote_stream::serve(reader, writer, host.as_ref(), &request).await
            }
            desktop::stream::Operation::LspOpen(_) => {
                lsp_stream::serve(reader, writer, host.as_ref(), &request).await
            }
        };
    }
    let pool = match request.operation.admission() {
        desktop::Admission::IntegrationOff => admission.revocations,
        desktop::Admission::Ordinary => admission.commands,
        desktop::Admission::Stream => anyhow::bail!("Unrouted desktop stream"),
    };
    let Ok(_command) = pool.try_acquire_owned() else {
        return write_response(
            &mut writer,
            &request,
            Outcome::Failed(Box::new(busy())),
            desktop::REFUSAL_TIMEOUT,
        )
        .await;
    };
    let status = request
        .operation
        .server_id()
        .and_then(|server| host.link_status(server));
    let result = progress::with_progress(&mut reader, &mut writer, &request, status, async {
        if let Operation::Events(operation) = &request.operation {
            match operation.as_ref() {
                desktop::events::Operation::Poll(args) => Ok(Reply::Events(Box::new(
                    desktop::events::Reply::Poll(Box::new(events.after(args.after))),
                ))),
            }
        } else {
            host.dispatch(&request.operation).await
        }
    })
    .await?;
    let outcome = match result {
        Ok(reply) => Outcome::Completed(Box::new(reply)),
        Err(error) => Outcome::Failed(Box::new(error)),
    };
    write_response(
        &mut writer,
        &request,
        outcome,
        desktop::RESPONSE_WRITE_TIMEOUT,
    )
    .await
}

async fn frame_until<R: tokio::io::AsyncRead + Unpin>(
    reader: &mut transport::Reader<R>,
    limit: usize,
    large: Option<Arc<Semaphore>>,
    deadline: Instant,
) -> io::Result<transport::Received> {
    if Instant::now() >= deadline {
        return Err(io::ErrorKind::TimedOut.into());
    }
    let result = tokio::time::timeout_at(deadline, reader.frame(limit, large))
        .await
        .map_err(|error| io::Error::new(io::ErrorKind::TimedOut, error))??;
    if Instant::now() >= deadline {
        return Err(io::ErrorKind::TimedOut.into());
    }
    result.ok_or_else(|| io::ErrorKind::UnexpectedEof.into())
}

pub(super) async fn write_response<W: AsyncWrite + Unpin>(
    writer: &mut W,
    request: &Request,
    outcome: Outcome,
    budget: std::time::Duration,
) -> anyhow::Result<()> {
    let opened = matches!(&outcome, Outcome::Opened(_));
    let response = ServerMessage::Response(Box::new(Response {
        id: request.id,
        outcome,
    }));
    let data = match wire::encode_server_for(&response, request) {
        Ok(data) => data,
        Err(error) if !opened => {
            // The operation has already run; never advertise that retry is safe.
            let message = ServerMessage::Response(Box::new(Response {
                id: request.id,
                outcome: Outcome::Failed(Box::new(failure::failure(
                    "invalid_result",
                    error.to_string(),
                    Retry::Unknown,
                ))),
            }));
            wire::encode_server_for(&message, request)?
        }
        Err(error) => return Err(error.into()),
    };
    let deadline = Instant::now() + budget;
    transport::write_until(writer, &data, deadline).await?;
    if !opened {
        // Windows pipes expose full shutdown only. The client validates actual
        // transport EOF after the outcome; the server never waits for client EOF.
        tokio::time::timeout_at(deadline, writer.shutdown()).await??;
        if Instant::now() >= deadline {
            return Err(io::Error::from(io::ErrorKind::TimedOut).into());
        }
    }
    Ok(())
}

fn busy() -> Failure {
    failure::failure(
        newport_protocol::common::BUSY,
        "Newport is busy. Try again in a moment.",
        Retry::Yes,
    )
}

/// Refusal is credential-free: the peer sends only Hello until Ready.
async fn refuse(stream: Stream) -> anyhow::Result<()> {
    let deadline = Instant::now() + desktop::REFUSAL_TIMEOUT;
    let (input, mut writer) = tokio::io::split(stream);
    let mut reader = transport::Reader::new(input);
    let hello = frame_until(
        &mut reader,
        desktop::HANDSHAKE_LIMITS.message,
        None,
        deadline,
    )
    .await?;
    wire::decode_hello(hello.frame.body())?;
    let data = wire::encode_server(&ServerMessage::Refused(Box::new(busy().error)))?;
    transport::write_until(&mut writer, &data, deadline).await?;
    tokio::time::timeout_at(deadline, writer.shutdown()).await??;
    Ok(())
}

#[cfg(test)]
mod tests;

#[cfg(test)]
pub(crate) mod test_fixture;
