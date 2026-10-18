//! Bounded local RPC transport shared by both application hosts. This module
//! owns framing, consent progress and connection-held resources, never UI state.
mod arguments;
pub mod control;
pub mod features;
pub mod git;
pub mod preferences;
mod progress;
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

/// A host supplies explicit services while extraction is in progress. Returning
/// the terminal guard transfers ownership atomically after a successful open.
pub trait Host: Send + Sync + 'static {
    fn dispatch(
        &self,
        method: &str,
        args: Value,
    ) -> impl Future<Output = Result<Value, Value>> + Send;
    fn open_terminal(
        &self,
        request: TerminalOpen,
    ) -> impl Future<Output = Result<Box<dyn Send>, String>> + Send;
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
        let slots = Arc::new(Semaphore::new(64));
        let commands = Arc::new(Semaphore::new(32));
        let revocations = Arc::new(Semaphore::new(4));
        let large_payloads = Arc::new(Semaphore::new(2));
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
                continue;
            };
            let host = host.clone();
            let events = self.events.clone();
            let commands = commands.clone();
            let revocations = revocations.clone();
            let large_payloads = large_payloads.clone();
            connections.spawn(async move {
                let _permit = permit;
                if let Err(error) =
                    serve(stream, host, events, commands, revocations, large_payloads).await
                {
                    log::debug!("Native client disconnected: {error}");
                }
            });
        };
        connections.shutdown().await;
        result
    }
}

async fn serve<H: Host>(
    mut stream: Stream,
    host: Arc<H>,
    events: EventLog,
    commands: Arc<Semaphore>,
    revocations: Arc<Semaphore>,
    large_payloads: Arc<Semaphore>,
) -> anyhow::Result<()> {
    let mut _large_permit = None;
    let request = tokio::time::timeout(Duration::from_secs(10), async {
        let len = stream.read_u32().await? as usize;
        anyhow::ensure!(
            (1..=MAX_REQUEST).contains(&len),
            "Invalid native request size"
        );
        if len > protocol::DEFAULT_MAX_REQUEST {
            _large_permit = Some(large_payloads.clone().try_acquire_owned()?);
        }
        let mut data = vec![0; len];
        stream.read_exact(&mut data).await?;
        let request: Request = rmp_serde::from_slice(&data)?;
        anyhow::ensure!(
            len <= protocol::request_limit(&request.method),
            "Native request exceeds method size limit"
        );
        anyhow::ensure!(
            request.version == VERSION,
            "Unsupported native protocol version"
        );
        Ok::<_, anyhow::Error>(request)
    })
    .await??;
    if matches!(
        request.method.as_str(),
        newport_platform::ipc::agent_relay::REGISTER | newport_platform::ipc::agent_relay::ATTACH
    ) {
        return crate::agent_relay::serve(stream, &request.method, request.args).await;
    }
    if request.method == protocol::TERMINAL_OPEN_LEASE {
        return terminal_lease::serve(stream, host.as_ref(), request.args).await;
    }
    // Feature calls and consent waits cannot consume revocation admission.
    // This reserves capacity only; the host still validates scope and permissions.
    let admission = if protocol::is_integration_revocation(&request.method, &request.args) {
        revocations
    } else {
        commands
    };
    let _command = admission
        .try_acquire_owned()
        .map_err(|_| anyhow::anyhow!("Native backend is busy"))?;
    let result = if request.method == "$events" {
        Ok(events.after(request.args["after"].as_u64().unwrap_or(0)))
    } else {
        progress::dispatch(
            &mut stream,
            &request.method,
            &request.args,
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
            "Native response for {} exceeds transport limit ({} bytes)",
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

#[cfg(test)]
mod tests;
