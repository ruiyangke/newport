//! A long-lived agent channel per server feature. Overview reads cannot queue
//! behind a service action, container prune, or Compose operation.
//! The agent samples on its own, so CPU figures are recent, not lifetime.
use crate::{
    agent,
    server_session::{Lease, ServerSession},
};
use serde_json::Value;
use std::{
    sync::{Arc, Weak},
    time::{Duration, Instant},
};
use tokio::{
    io::{AsyncBufRead, AsyncBufReadExt, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader},
    sync::Mutex,
};

const IDLE: Duration = Duration::from_secs(300);
// A server that cannot run the agent is not asked to upload it every sample;
// the reason is repeated until then, or until its connection settings change.
const UNAVAILABLE_FOR: Duration = Duration::from_secs(600);
const REPLY_TIMEOUT: Duration = Duration::from_secs(15);
const MAX_REPLY: u64 = 4 << 20;

type Channel = crate::agent_runtime::WorkerStream;

#[derive(Default)]
pub(crate) struct Channels([Arc<Mutex<Slot>>; 6]);
impl Channels {
    pub(crate) fn for_request(&self, request: &Value) -> Arc<Mutex<Slot>> {
        let lane = match request["op"].as_str() {
            Some("files_watch") => 5,
            Some("snapshot") => 0,
            Some("services" | "logs" | "unit" | "action") => 1,
            Some(
                "containers" | "container_inspect" | "container_logs" | "container_action"
                | "project_action" | "containers_remove" | "docker_disk" | "docker_prune",
            ) => 2,
            Some(
                "compose_project" | "compose_run" | "compose_job" | "compose_cancel"
                | "compose_jobs",
            ) => 3,
            _ => 4,
        };
        self.0[lane].clone()
    }
    pub(crate) async fn drain(&self) {
        for lane in &self.0 {
            lane.lock().await.drain().await;
        }
    }
}
struct Open {
    session: Lease,
    reader: BufReader<tokio::io::ReadHalf<Channel>>,
    writer: tokio::io::WriteHalf<Channel>,
}
struct Unavailable {
    until: Instant,
    reason: String,
    runtime: Option<String>,
}
#[derive(Default)]
pub(crate) struct Slot {
    open: Option<Open>,
    unavailable: Option<Unavailable>,
    used: Option<Instant>,
}
impl Slot {
    pub(crate) async fn drain(&mut self) {
        self.close().await;
        self.unavailable = None;
    }
    async fn close(&mut self) {
        if let Some(mut open) = self.open.take() {
            let _ = open.writer.shutdown().await;
        }
    }
}

/// Explicit retries clear this session's transport, runtime, and monitor failures.
pub async fn retry_installation(server: &Arc<ServerSession>) {
    server.retry_transport();
    server.agent_runtime.retry_failed().await;
    server.monitor.drain().await;
}

/// Overview retries affect its own channel; another feature may be executing
/// a mutation that must finish on its existing protocol stream.
pub(crate) async fn retry_overview(server: &Arc<ServerSession>) {
    server.retry_transport();
    server.agent_runtime.retry_failed().await;
    server
        .monitor
        .for_request(&serde_json::json!({"op":"snapshot"}))
        .lock()
        .await
        .drain()
        .await;
}

/// An Overview reading from the server's agent.
pub async fn snapshot(server: &Arc<ServerSession>) -> Result<Value, String> {
    ask(
        server,
        &serde_json::json!({"op": "snapshot"}),
        REPLY_TIMEOUT,
    )
    .await
}

/// Any request on the server's agent channel (Overview, Services), launching
/// using an already-installed agent. Requests to one server are
/// serialised within their feature channel; `timeout` bounds the reply.
pub async fn ask(
    server: &Arc<ServerSession>,
    request: &Value,
    timeout: Duration,
) -> Result<Value, String> {
    if server.is_revoked() {
        return Err("Server session changed. Reconnect before retrying.".into());
    }
    let shared = server.monitor.for_request(request);
    let mut slot = shared.lock().await;
    if slot
        .open
        .as_ref()
        .is_some_and(|open| open.session.is_closed() || open.session.is_draining())
    {
        slot.close().await;
        slot.unavailable = None;
    }
    if let Some(cached) = &slot.unavailable {
        let runtime = server.agent_runtime.snapshot().hello.map(|h| h.instance_id);
        if Instant::now() < cached.until && cached.runtime == runtime {
            return Err(unavailable(request, &cached.reason));
        }
        slot.unavailable = None;
    }
    slot.used = Some(Instant::now());
    // Only known read-only operations may be replayed after an incomplete reply.
    let attempts = if retryable_read(request) { 2 } else { 1 };
    for attempt in 0..attempts {
        if slot.open.is_none() {
            match open(server).await? {
                Ok(open) => {
                    let pressure = open.session.channel_pressure();
                    slot.open = Some(open);
                    close_when_idle(Arc::downgrade(&shared), pressure);
                }
                Err(reason) => {
                    slot.unavailable = Some(Unavailable {
                        until: Instant::now() + UNAVAILABLE_FOR,
                        reason: reason.clone(),
                        runtime: server.agent_runtime.snapshot().hello.map(|h| h.instance_id),
                    });
                    return Err(unavailable(request, &reason));
                }
            }
        }
        let result = exchange_session(&mut slot.open, |mut open| async move {
            match exchange(&mut open.reader, &mut open.writer, request, timeout).await {
                Ok(reply) => Ok((open, reply)),
                Err(error) => Err(error),
            }
        })
        .await;
        match result {
            Ok(reply) => {
                slot.open
                    .as_ref()
                    .expect("successful exchange")
                    .session
                    .validate()?;
                if slot
                    .open
                    .as_ref()
                    .is_some_and(|open| open.session.is_draining())
                {
                    slot.close().await;
                }
                return answer(reply);
            }
            Err(error) => {
                if attempt + 1 == attempts {
                    return Err(error);
                }
            }
        }
    }
    unreachable!("the second attempt returns")
}

fn retryable_read(request: &Value) -> bool {
    matches!(
        request["op"].as_str(),
        Some(
            "snapshot"
                | "ports"
                | "services"
                | "logs"
                | "unit"
                | "containers"
                | "container_inspect"
                | "container_logs"
                | "docker_disk"
                | "compose_project"
                | "compose_job"
                | "compose_jobs"
        )
    )
}

/// A cancelled or incomplete exchange must not leave a reusable stream behind:
/// its unread reply (or a partial frame) would belong to the next caller.
async fn exchange_session<S, T, F>(
    cached: &mut Option<S>,
    exchange: impl FnOnce(S) -> F,
) -> Result<T, String>
where
    F: std::future::Future<Output = Result<(S, T), String>>,
{
    let session = cached.take().expect("opened above");
    let (session, reply) = exchange(session).await?;
    *cached = Some(session);
    Ok(reply)
}

/// These features need the agent; say why it cannot run rather than show nothing.
fn unavailable(request: &Value, reason: &str) -> String {
    let feature = match request["op"].as_str() {
        Some("ports") => "Port discovery",
        _ => "Monitoring",
    };
    format!("{feature} needs the Newport agent, which can’t run on this server: {reason}")
}

/// Transport and runtime recovery remain eligible on the next request. Only
/// actionable capability/installation failures get the longer unavailable cache.
async fn open(server: &Arc<ServerSession>) -> Result<Result<Open, String>, String> {
    let session = server.acquire(server.server()).await?;
    match session.launch(agent::Service::Monitor).await {
        Ok((stream, _)) => {
            let (reader, writer) = tokio::io::split(stream);
            Ok(Ok(Open {
                session,
                reader: BufReader::new(reader),
                writer,
            }))
        }
        Err(error) => {
            server.agent_runtime.probe().await;
            let runtime = server.agent_runtime.snapshot();
            if runtime.retryable || connection_problem(&error) {
                Err(error)
            } else {
                Ok(Err(error))
            }
        }
    }
}
fn connection_problem(error: &str) -> bool {
    error == crate::agent_runtime::BUSY
        || error.starts_with("SSH session capacity unavailable")
        || error.contains("Needle runtime disconnected")
        || error.contains("Needle runtime changed before worker registration")
        || error.starts_with("SSH transport interrupted")
        || error.starts_with("SSH agent unavailable")
        || error.contains("timed out")
}

fn close_when_idle(slot: Weak<Mutex<Slot>>, mut pressure: tokio::sync::watch::Receiver<u64>) {
    tokio::spawn(async move {
        // A reclaim that finds the channel busy is retried shortly instead of
        // waiting for the next idle check.
        let mut pending = false;
        loop {
            let wait = Duration::from_secs(if pending { 1 } else { 60 });
            let reclaim = tokio::select! {
                _ = tokio::time::sleep(wait) => pending,
                changed = pressure.changed() => {
                    if changed.is_err() { return; }
                    true
                }
            };
            let Some(slot) = slot.upgrade() else { return };
            // The same lock protects the entire exchange. Never interrupt a
            // running monitor action to make room for another SSH channel.
            let Ok(mut slot) = slot.try_lock() else {
                pending = reclaim;
                continue;
            };
            pending = false;
            if slot.open.is_none() {
                return;
            }
            if reclaim || slot.used.is_some_and(|used| used.elapsed() > IDLE) {
                slot.close().await;
                return;
            }
        }
    });
}

async fn exchange(
    reader: &mut (impl AsyncBufRead + Unpin),
    writer: &mut (impl AsyncWrite + Unpin),
    request: &Value,
    timeout: Duration,
) -> Result<Value, String> {
    let mut message = serde_json::to_vec(request).map_err(|e| e.to_string())?;
    message.push(b'\n');
    let line = tokio::time::timeout(timeout, async {
        writer.write_all(&message).await?;
        writer.flush().await?;
        let mut line = Vec::new();
        (&mut *reader)
            .take(MAX_REPLY)
            .read_until(b'\n', &mut line)
            .await?;
        if !line.ends_with(b"\n") {
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                if line.is_empty() {
                    "the agent monitor closed"
                } else {
                    "the agent monitor reply was cut short"
                },
            ));
        }
        Ok(line)
    })
    .await
    .map_err(|_| {
        format!(
            "The agent monitor did not answer within {} seconds.",
            timeout.as_secs()
        )
    })?
    .map_err(|error| format!("SSH transport interrupted: {error}"))?;
    serde_json::from_slice(&line).map_err(|error| format!("Invalid agent monitor reply: {error}"))
}

fn answer(reply: Value) -> Result<Value, String> {
    match reply.get("error").and_then(Value::as_str) {
        Some(error) => Err(error.to_owned()),
        None => Ok(reply),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_slow_feature_does_not_block_other_channels_and_mutations_keep_their_feature_order() {
        let channels = Channels::default();
        let request = |op| serde_json::json!({"op":op});
        let services = channels.for_request(&request("action"));
        let busy = services.lock().await;
        let reads = channels.for_request(&request("services"));
        assert!(Arc::ptr_eq(&services, &reads));
        assert!(reads.try_lock().is_err());
        for op in ["snapshot", "containers", "compose_run", "future_operation"] {
            assert!(
                channels.for_request(&request(op)).try_lock().is_ok(),
                "{op}"
            );
        }
        for (read, write) in [
            ("containers", "docker_prune"),
            ("compose_jobs", "compose_cancel"),
        ] {
            assert!(Arc::ptr_eq(
                &channels.for_request(&request(read)),
                &channels.for_request(&request(write))
            ));
        }
        assert!(
            Channels::default()
                .for_request(&request("services"))
                .try_lock()
                .is_ok()
        );
        drop(busy);
        channels.drain().await;
    }

    #[test]
    fn reconnect_replays_reads_but_never_mutations_or_unknown_operations() {
        for op in ["snapshot", "services", "container_inspect", "compose_job"] {
            assert!(retryable_read(&serde_json::json!({"op": op})));
        }
        for op in [
            "action",
            "container_action",
            "project_action",
            "containers_remove",
            "docker_prune",
            "compose_run",
            "compose_cancel",
            "future_operation",
        ] {
            assert!(!retryable_read(&serde_json::json!({"op": op})));
        }
    }

    type FixtureSession = (
        BufReader<tokio::io::ReadHalf<tokio::io::DuplexStream>>,
        tokio::io::WriteHalf<tokio::io::DuplexStream>,
    );

    fn fixture_session() -> (FixtureSession, BufReader<tokio::io::DuplexStream>) {
        let (client, agent) = tokio::io::duplex(1024);
        let (reader, writer) = tokio::io::split(client);
        ((BufReader::new(reader), writer), BufReader::new(agent))
    }

    async fn exchange_cached(
        cached: &mut Option<FixtureSession>,
        op: &str,
    ) -> Result<Value, String> {
        let request = serde_json::json!({"op": op});
        exchange_session(cached, |(mut reader, mut writer)| async move {
            let reply = exchange(&mut reader, &mut writer, &request, REPLY_TIMEOUT).await?;
            Ok(((reader, writer), reply))
        })
        .await
    }

    #[tokio::test]
    async fn cancelled_exchange_discards_channel_before_the_next_request() {
        for partial_reply in [b"".as_slice(), b"{\"stale\":".as_slice()] {
            let (session, mut agent) = fixture_session();
            let mut cached = Some(session);
            {
                let request = exchange_cached(&mut cached, "cancelled");
                tokio::pin!(request);
                tokio::select! {
                    result = &mut request => panic!("The incomplete reply finished: {result:?}"),
                    _ = async {
                        let mut request = String::new();
                        agent.read_line(&mut request).await.unwrap();
                        assert_eq!(request, "{\"op\":\"cancelled\"}\n");
                        agent.get_mut().write_all(partial_reply).await.unwrap();
                        tokio::task::yield_now().await;
                    } => {},
                }
                // Dropping the in-flight request models native IPC disconnect.
            }
            assert!(cached.is_none(), "A cancelled stream must not be reused");
            let mut closed = Vec::new();
            tokio::time::timeout(Duration::from_secs(1), agent.read_to_end(&mut closed))
                .await
                .expect("Cancellation must close the old channel")
                .unwrap();
            assert!(closed.is_empty());

            // ask() now sees an empty slot and opens a fresh channel. Its reply
            // cannot be mistaken for the abandoned request's stale response.
            let (session, mut next_agent) = fixture_session();
            cached = Some(session);
            let next = tokio::spawn(async move {
                let mut request = String::new();
                next_agent.read_line(&mut request).await.unwrap();
                assert_eq!(request, "{\"op\":\"next\"}\n");
                next_agent
                    .get_mut()
                    .write_all(b"{\"fresh\":true}\n")
                    .await
                    .unwrap();
            });
            assert_eq!(
                exchange_cached(&mut cached, "next").await.unwrap()["fresh"],
                true
            );
            assert!(cached.is_some());
            next.await.unwrap();
        }
    }

    #[tokio::test]
    async fn complete_agent_error_preserves_the_channel_for_its_next_request() {
        let (session, mut agent) = fixture_session();
        let mut cached = Some(session);
        let task = tokio::spawn(async move {
            for (op, reply) in [
                ("first", b"{\"error\":\"not ready\"}\n".as_slice()),
                ("second", b"{\"fresh\":true}\n".as_slice()),
            ] {
                let mut request = String::new();
                agent.read_line(&mut request).await.unwrap();
                assert_eq!(request, format!("{{\"op\":\"{op}\"}}\n"));
                agent.get_mut().write_all(reply).await.unwrap();
            }
        });
        assert_eq!(
            answer(exchange_cached(&mut cached, "first").await.unwrap()).unwrap_err(),
            "not ready"
        );
        assert!(cached.is_some());
        assert_eq!(
            exchange_cached(&mut cached, "second").await.unwrap()["fresh"],
            true
        );
        task.await.unwrap();
    }

    async fn ask(reply: &'static [u8]) -> Result<Value, String> {
        let (client, mut agent) = tokio::io::duplex(1 << 16);
        let (reader, mut writer) = tokio::io::split(client);
        let mut reader = BufReader::new(reader);
        let answered = tokio::spawn(async move {
            let mut request = [0; 18];
            agent.read_exact(&mut request).await.unwrap();
            assert_eq!(&request, b"{\"op\":\"snapshot\"}\n");
            agent.write_all(reply).await.unwrap();
        });
        let result = exchange(
            &mut reader,
            &mut writer,
            &serde_json::json!({"op": "snapshot"}),
            REPLY_TIMEOUT,
        )
        .await;
        answered.await.unwrap();
        result
    }

    #[tokio::test]
    async fn one_request_one_line() {
        let reply = ask(b"{\"cpu\":12.5}\n").await.unwrap();
        assert_eq!(answer(reply).unwrap()["cpu"], 12.5);
    }

    #[tokio::test]
    async fn agent_errors_and_broken_channels_are_errors() {
        assert_eq!(
            answer(ask(b"{\"error\":\"not ready\"}\n").await.unwrap()).unwrap_err(),
            "not ready"
        );
        assert!(ask(b"").await.unwrap_err().contains("closed"));
        assert!(ask(b"{\"cpu\":1").await.unwrap_err().contains("cut short"));
        assert!(
            ask(b"not json\n")
                .await
                .unwrap_err()
                .starts_with("Invalid agent monitor reply")
        );
    }

    #[test]
    fn only_connection_failures_are_retried_at_once() {
        assert!(connection_problem(crate::agent_runtime::BUSY));
        assert!(connection_problem(
            "SSH session capacity unavailable. Retry shortly"
        ));
        assert!(connection_problem("Needle runtime disconnected"));
        assert!(connection_problem(
            "Needle runtime changed before worker registration"
        ));
        assert!(connection_problem("SSH transport interrupted: reset"));
        assert!(connection_problem("SSH command timed out starting agent"));
        assert!(!connection_problem("The Newport agent requires Linux."));
        assert!(!connection_problem(
            "The Newport agent supports x86_64 and ARM64 Linux servers."
        ));
        assert!(!connection_problem(
            "Remote command failed (status 1): noexec"
        ));
        assert_eq!(
            unavailable(
                &serde_json::json!({"op": "snapshot"}),
                "The Newport agent requires Linux."
            ),
            "Monitoring needs the Newport agent, which can’t run on this server: The Newport agent requires Linux."
        );
    }
}
