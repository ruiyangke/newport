//! One long-lived agent monitor per server: every Overview reading comes from
//! it, as a request on an open channel rather than a new SSH login each time.
//! The agent samples on its own, so CPU figures are recent, not lifetime.
use crate::{agent, model::Server, ssh::ExecSession};
use serde_json::Value;
use std::{
    collections::HashMap,
    sync::{Arc, LazyLock, Mutex as StdMutex, Weak},
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

type Channel = russh::ChannelStream<russh::client::Msg>;
struct Open {
    session: ExecSession,
    reader: BufReader<tokio::io::ReadHalf<Channel>>,
    writer: tokio::io::WriteHalf<Channel>,
}
#[derive(Default)]
struct Slot {
    server: Option<Server>,
    open: Option<Open>,
    unavailable: Option<(Instant, String)>,
    used: Option<Instant>,
}
impl Slot {
    async fn close(&mut self) {
        if let Some(mut open) = self.open.take() {
            let _ = open.writer.shutdown().await;
            open.session.close().await;
        }
    }
}

static SLOTS: LazyLock<StdMutex<HashMap<uuid::Uuid, Arc<Mutex<Slot>>>>> =
    LazyLock::new(Default::default);

/// An Overview reading from the server's agent.
pub async fn snapshot(server: &Server) -> Result<Value, String> {
    ask(
        server,
        &serde_json::json!({"op": "snapshot"}),
        REPLY_TIMEOUT,
    )
    .await
}

/// Any request on the server's agent channel (Overview, Services), launching
/// and if needed installing the agent first. Requests to one server are
/// serialised on its channel; `timeout` bounds the reply.
pub async fn ask(server: &Server, request: &Value, timeout: Duration) -> Result<Value, String> {
    let shared = SLOTS.lock().unwrap().entry(server.id).or_default().clone();
    let mut slot = shared.lock().await;
    // A changed host or login gets a new channel and a fresh chance.
    if !slot
        .server
        .as_ref()
        .is_some_and(|known| known.same_connection(server))
    {
        slot.close().await;
        slot.unavailable = None;
        slot.server = Some(server.clone());
    }
    if let Some((until, reason)) = &slot.unavailable {
        if Instant::now() < *until {
            return Err(unavailable(reason));
        }
        slot.unavailable = None;
    }
    slot.used = Some(Instant::now());
    // One retry covers a channel that dropped while idle.
    for attempt in 0..2 {
        if slot.open.is_none() {
            match open(server).await? {
                Ok(open) => {
                    slot.open = Some(open);
                    close_when_idle(Arc::downgrade(&shared));
                }
                Err(reason) => {
                    slot.unavailable = Some((Instant::now() + UNAVAILABLE_FOR, reason.clone()));
                    return Err(unavailable(&reason));
                }
            }
        }
        let open = slot.open.as_mut().expect("opened above");
        match exchange(&mut open.reader, &mut open.writer, request, timeout).await {
            Ok(reply) => return answer(reply),
            Err(error) => {
                slot.close().await;
                if attempt == 1 {
                    return Err(error);
                }
            }
        }
    }
    unreachable!("the second attempt returns")
}

/// Monitoring needs the agent; say why it cannot run rather than show nothing.
fn unavailable(reason: &str) -> String {
    format!("Monitoring needs the Newport agent, which can’t run on this server: {reason}")
}

/// Connection failures are retried on the next request; anything else
/// stopping the agent from starting means this server cannot run it.
async fn open(server: &Server) -> Result<Result<Open, String>, String> {
    let session = ExecSession::connect(server).await?;
    match agent::launch(&session, agent::Service::Monitor).await {
        Ok((stream, _)) => {
            let (reader, writer) = tokio::io::split(stream);
            Ok(Ok(Open {
                session,
                reader: BufReader::new(reader),
                writer,
            }))
        }
        Err(error) => {
            session.close().await;
            if connection_problem(&error) {
                Err(error)
            } else {
                Ok(Err(error))
            }
        }
    }
}
fn connection_problem(error: &str) -> bool {
    error.starts_with("SSH transport interrupted")
        || error.starts_with("SSH agent unavailable")
        || error.contains("timed out")
}

fn close_when_idle(slot: Weak<Mutex<Slot>>) {
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(60)).await;
            let Some(slot) = slot.upgrade() else { return };
            let mut slot = slot.lock().await;
            if slot.open.is_none() {
                return;
            }
            if slot.used.is_some_and(|used| used.elapsed() > IDLE) {
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
        assert!(ask(b"not json\n")
            .await
            .unwrap_err()
            .starts_with("Invalid agent monitor reply"));
    }

    #[test]
    fn only_connection_failures_are_retried_at_once() {
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
            unavailable("The Newport agent requires Linux."),
            "Monitoring needs the Newport agent, which can’t run on this server: The Newport agent requires Linux."
        );
    }
}
