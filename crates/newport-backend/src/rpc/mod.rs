//! Bounded local RPC transport between the UI and the backend. This module
//! owns framing, consent progress and connection-held resources, never UI state.
pub mod account;
mod arguments;
pub mod control;
pub mod features;
pub mod git;
pub mod graphical;
mod lsp_stream;
pub mod preferences;
mod progress;
mod remote_stream;
pub mod services;
mod terminal_lease;
use newport_platform::ipc::asynchronous::{Listener, Stream};
use newport_platform::ipc::protocol;
use protocol::{MAX_REQUEST, MAX_RESPONSE, Request, Response, VERSION};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    collections::VecDeque,
    future::Future,
    path::Path,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    sync::Semaphore,
};
use uuid::Uuid;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TerminalOpen {
    pub id: Uuid,
    pub session: Uuid,
    pub cols: u32,
    pub rows: u32,
}

/// The application supplies the services. Returning the terminal guard
/// transfers ownership atomically after a successful open.
pub trait Host: Send + Sync + 'static {
    fn dispatch(
        &self,
        method: &str,
        args: Value,
    ) -> impl Future<Output = Result<Value, Value>> + Send;
    fn open_terminal(
        &self,
        request: TerminalOpen,
    ) -> impl Future<Output = Result<TerminalChannel, String>> + Send;
    /// Start a command on a server over its existing connection.
    fn open_remote_stream(
        &self,
        _server: Uuid,
        _command: String,
    ) -> impl Future<Output = Result<RemoteChannel, String>> + Send {
        async { Err("Remote streams aren’t available.".to_owned()) }
    }
    /// Start a registered GUI worker and retain its runtime/connection guard.
    fn open_remote_apps(
        &self,
        _server: Uuid,
        _arguments: Vec<String>,
    ) -> impl Future<Output = Result<RemoteChannel, String>> + Send {
        async { Err("Remote Apps workers aren’t available.".to_owned()) }
    }
    /// The status of a server's link, without opening it, for requests that
    /// wait while its login waits for approval.
    fn link_status(
        &self,
        _server: Uuid,
    ) -> Option<tokio::sync::watch::Receiver<newport_ssh::Status>> {
        None
    }
    /// Start a language server for a worktree through a Needle worker.
    fn open_lsp(
        &self,
        _request: LspOpen,
    ) -> impl Future<Output = Result<LspChannel, String>> + Send {
        async { Err("Language servers aren’t available.".to_owned()) }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LspOpen {
    pub server_id: Uuid,
    /// The connection the editor was opened for; a changed server is refused.
    pub expected_connection: Value,
    pub root: String,
    pub server: String,
    pub envrc: bool,
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
    rows: VecDeque<(u64, Value)>,
}
#[derive(Clone, Default)]
pub struct EventLog(Arc<Mutex<Events>>);
impl EventLog {
    pub fn record(&self, value: Value) {
        let mut events = self.0.lock().unwrap_or_else(|e| e.into_inner());
        events.next += 1;
        let id = events.next;
        events.rows.push_back((id, value));
        while events.rows.len() > 100 {
            events.rows.pop_front();
        }
    }
    fn after(&self, cursor: u64) -> Value {
        let events = self.0.lock().unwrap_or_else(|e| e.into_inner());
        json!({"cursor":events.next,"entries":events.rows.iter().filter(|(id,_)| *id > cursor).map(|(_,value)| value).collect::<Vec<_>>()})
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
        let slots = Arc::new(Semaphore::new(CONNECTIONS));
        let admission = Admission {
            commands: Arc::new(Semaphore::new(COMMANDS)),
            revocations: Arc::new(Semaphore::new(4)),
            large_payloads: Arc::new(Semaphore::new(2)),
        };
        // Over capacity, a connection still learns it was refused.
        let refusals = Arc::new(Semaphore::new(8));
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

/// Open connections, including long-lived terminal leases and streams.
const CONNECTIONS: usize = 256;
/// Ordinary commands running at once.
const COMMANDS: usize = 32;

/// Separate capacity per kind of request, so one kind cannot starve another.
#[derive(Clone)]
struct Admission {
    commands: Arc<Semaphore>,
    revocations: Arc<Semaphore>,
    large_payloads: Arc<Semaphore>,
}

async fn serve<H: Host>(
    mut stream: Stream,
    host: Arc<H>,
    events: EventLog,
    admission: Admission,
) -> anyhow::Result<()> {
    let mut _large_permit = None;
    let request = tokio::time::timeout(Duration::from_secs(10), async {
        let len = stream.read_u32().await? as usize;
        anyhow::ensure!((1..=MAX_REQUEST).contains(&len), "Invalid request size");
        if len > protocol::DEFAULT_MAX_REQUEST {
            _large_permit = Some(admission.large_payloads.clone().try_acquire_owned()?);
        }
        let mut data = vec![0; len];
        stream.read_exact(&mut data).await?;
        let request: Request = rmp_serde::from_slice(&data)?;
        anyhow::ensure!(
            len <= protocol::request_limit(&request.method),
            "Request exceeds method size limit"
        );
        anyhow::ensure!(request.version == VERSION, "Unsupported protocol version");
        Ok::<_, anyhow::Error>(request)
    })
    .await??;
    if request.method == protocol::TERMINAL_OPEN_LEASE {
        return terminal_lease::serve(stream, host.as_ref(), request.args).await;
    }
    if request.method == protocol::REMOTE_STREAM_OPEN {
        return remote_stream::serve(stream, host.as_ref(), request.args, false).await;
    }
    if request.method == protocol::REMOTE_APPS_OPEN {
        return remote_stream::serve(stream, host.as_ref(), request.args, true).await;
    }
    if request.method == protocol::LSP_OPEN {
        return lsp_stream::serve(stream, host.as_ref(), request.args).await;
    }
    // Feature calls and consent waits cannot consume revocation admission.
    // This reserves capacity only; the host still validates scope and permissions.
    let pool = if protocol::is_integration_revocation(&request.method, &request.args) {
        admission.revocations
    } else {
        admission.commands
    };
    let Ok(_command) = pool.try_acquire_owned() else {
        return reply_busy(stream).await;
    };
    let result = if request.method == "$events" {
        Ok(events.after(request.args["after"].as_u64().unwrap_or(0)))
    } else {
        let status = progress::server(&request.args).and_then(|server| host.link_status(server));
        progress::dispatch(
            &mut stream,
            &request.method,
            status,
            host.dispatch(&request.method, request.args.clone()),
        )
        .await?
    };
    let response = match result {
        Ok(result) => Response {
            version: VERSION,
            result,
            error: None,
        },
        Err(error) => Response {
            version: VERSION,
            result: Value::Null,
            error: Some(error),
        },
    };
    let mut bytes = rmp_serde::to_vec_named(&response)?;
    if bytes.len() > MAX_RESPONSE {
        log::warn!(
            "Response for {} exceeds transport limit ({} bytes)",
            request.method,
            bytes.len()
        );
        bytes = rmp_serde::to_vec_named(&Response {
            version: VERSION,
            result: Value::Null,
            error: Some(
                json!({"code":"RESPONSE_TOO_LARGE", "message":"Backend response is too large to display."}),
            ),
        })?;
    }
    tokio::time::timeout(Duration::from_secs(30), async {
        stream.write_u32(bytes.len() as u32).await?;
        stream.write_all(&bytes).await
    })
    .await??;
    Ok(())
}

/// Refuses a connection over capacity. Its request is read first: a peer
/// still writing it would otherwise see the write fail (on Windows pipes in
/// particular) instead of the refusal it can repeat.
async fn refuse(mut stream: Stream) -> anyhow::Result<()> {
    tokio::time::timeout(Duration::from_secs(5), async {
        let len = stream.read_u32().await? as usize;
        if (1..=protocol::DEFAULT_MAX_REQUEST).contains(&len) {
            let mut request = (&mut stream).take(len as u64);
            tokio::io::copy(&mut request, &mut tokio::io::sink()).await?;
        }
        anyhow::Ok(())
    })
    .await??;
    reply_busy(stream).await
}

/// Refuses a request without running it. The client may repeat it.
async fn reply_busy(mut stream: Stream) -> anyhow::Result<()> {
    let bytes = rmp_serde::to_vec_named(&Response {
        version: VERSION,
        result: Value::Null,
        error: Some(json!({
            "code": protocol::BUSY,
            "message": "Newport is busy. Try again in a moment.",
        })),
    })?;
    tokio::time::timeout(Duration::from_secs(5), async {
        stream.write_u32(bytes.len() as u32).await?;
        stream.write_all(&bytes).await
    })
    .await??;
    Ok(())
}

#[cfg(test)]
mod tests;
