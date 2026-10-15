//! Bounded, one-use attachments to a same-user native client's agent relay.
//! Registration and attachments must have the same kernel-reported peer PID.
use newport_platform::ipc::asynchronous::{self, Stream};
use newport_platform::ipc::{agent_relay as relay, protocol as desktop};
use serde_json::Value;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex, OnceLock},
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    sync::{mpsc, oneshot},
};
use uuid::Uuid;

type Reply = oneshot::Sender<Result<Stream, String>>;
#[derive(Default)]
struct Registry(Mutex<Option<Arc<Owner>>>);
struct Owner {
    id: Uuid,
    pid: u32,
    requests: mpsc::Sender<relay::Open>,
    pending: Mutex<HashMap<Uuid, Reply>>,
}
static REGISTRY: OnceLock<Registry> = OnceLock::new();
fn registry() -> &'static Registry {
    REGISTRY.get_or_init(Registry::default)
}

impl Registry {
    fn register(&self, pid: u32) -> Result<(Arc<Owner>, mpsc::Receiver<relay::Open>), String> {
        let mut current = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if current.is_some() {
            return Err("A native SSH agent relay is already connected".into());
        }
        let (requests, receiver) = mpsc::channel(relay::MAX_CONNECTIONS);
        let owner = Arc::new(Owner {
            id: Uuid::new_v4(),
            pid,
            requests,
            pending: Mutex::new(HashMap::new()),
        });
        *current = Some(owner.clone());
        Ok((owner, receiver))
    }
    fn disconnect(&self, owner: &Owner) {
        let mut current = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if current.as_ref().is_some_and(|value| value.id == owner.id) {
            *current = None;
            log::info!("Native SSH agent relay disconnected (PID {})", owner.pid);
        }
        let pending = std::mem::take(&mut *owner.pending.lock().unwrap_or_else(|e| e.into_inner()));
        for (_, sender) in pending {
            let _ = sender.send(Err(
                "Native SSH agent relay disconnected; retry with Newport open".into(),
            ));
        }
    }
    fn ticket(&self, pid: u32, ticket: Uuid) -> Result<Reply, String> {
        let current = self.0.lock().unwrap_or_else(|e| e.into_inner());
        let owner = current
            .as_ref()
            .ok_or("Native SSH agent relay is not registered")?;
        if owner.pid != pid {
            return Err("SSH agent attachment does not belong to the registered process".into());
        }
        let sender = owner
            .pending
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&ticket)
            .ok_or("SSH agent attachment expired or was already consumed")?;
        if sender.is_closed() {
            return Err("SSH agent attachment was cancelled".into());
        }
        Ok(sender)
    }
    fn owner(&self) -> Option<Arc<Owner>> {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
}

struct Registration<'a> {
    registry: &'a Registry,
    owner: Arc<Owner>,
}
impl Drop for Registration<'_> {
    fn drop(&mut self) {
        self.registry.disconnect(&self.owner);
    }
}
struct Pending {
    owner: Arc<Owner>,
    ticket: Uuid,
}
impl Drop for Pending {
    fn drop(&mut self) {
        self.owner
            .pending
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&self.ticket);
    }
}

/// None means no native UI is connected, so the host may use its own agent.
/// Once a relay is selected its errors remain errors, never a silent reroute.
pub async fn connect(source: relay::Source) -> Option<Result<Stream, String>> {
    let owner = registry().owner()?;
    Some(open(owner, source).await)
}
async fn open(owner: Arc<Owner>, source: relay::Source) -> Result<Stream, String> {
    let ticket = Uuid::new_v4();
    let (sender, receiver) = oneshot::channel();
    {
        let mut pending = owner.pending.lock().unwrap_or_else(|e| e.into_inner());
        if pending.len() >= relay::MAX_CONNECTIONS {
            return Err("Native SSH agent relay is busy; retry shortly".into());
        }
        pending.insert(ticket, sender);
    }
    let _pending = Pending {
        owner: owner.clone(),
        ticket,
    };
    owner
        .requests
        .try_send(relay::Open { ticket, source })
        .map_err(|_| {
            "Native SSH agent relay is unavailable; retry with Newport open".to_string()
        })?;
    tokio::time::timeout(Duration::from_secs(15), receiver)
        .await
        .map_err(|_| {
            "Native SSH agent relay did not open the agent; retry with Newport open".to_string()
        })?
        .map_err(|_| "Native SSH agent relay disconnected; retry with Newport open".to_string())?
}

pub async fn serve(mut stream: Stream, method: &str, args: Value) -> anyhow::Result<()> {
    let pid = asynchronous::peer_process_id(&stream)?;
    if method == relay::REGISTER {
        if args != serde_json::json!({}) {
            return acknowledgement(&mut stream, Some("Invalid agent relay registration".into()))
                .await;
        }
        let (owner, mut receiver) = match registry().register(pid) {
            Ok(value) => value,
            Err(error) => return acknowledgement(&mut stream, Some(error)).await,
        };
        let _registration = Registration {
            registry: registry(),
            owner,
        };
        acknowledgement(&mut stream, None).await?;
        log::info!("Native SSH agent relay registered (PID {pid})");
        let mut probe = [0; 1];
        loop {
            tokio::select! {
                _ = stream.read(&mut probe) => break,
                request = receiver.recv() => {
                    let Some(request) = request else { break };
                    let data = rmp_serde::to_vec_named(&request)?;
                    anyhow::ensure!(data.len() <= relay::MAX_CONTROL_FRAME, "Agent control frame is too large");
                    tokio::time::timeout(Duration::from_secs(5), async {
                        stream.write_u32(data.len() as u32).await?;
                        stream.write_all(&data).await
                    }).await??;
                }
            }
        }
        Ok(())
    } else {
        let attachment: relay::Attach = serde_json::from_value(args)?;
        anyhow::ensure!(
            attachment
                .error
                .as_ref()
                .is_none_or(|error| error.len() <= 512),
            "Agent error is too large"
        );
        let sender = match registry().ticket(pid, attachment.ticket) {
            Ok(sender) => sender,
            Err(error) => return acknowledgement(&mut stream, Some(error)).await,
        };
        if let Some(error) = attachment.error {
            let _ = sender.send(Err(format!("Native SSH agent: {error}")));
            return acknowledgement(&mut stream, None).await;
        }
        if let Err(error) = acknowledgement(&mut stream, None).await {
            let _ = sender.send(Err("Native SSH agent attachment disconnected".into()));
            return Err(error);
        }
        // Hand off only after the normal bridge acknowledgement; subsequent
        // bytes are the filtered agent protocol, not MessagePack responses.
        let _ = sender.send(Ok(stream));
        Ok(())
    }
}
async fn acknowledgement(stream: &mut Stream, error: Option<String>) -> anyhow::Result<()> {
    let response = desktop::Response {
        version: desktop::VERSION,
        result: Value::Null,
        error: error.map(Value::String),
    };
    let bytes = rmp_serde::to_vec_named(&response)?;
    tokio::time::timeout(Duration::from_secs(5), async {
        stream.write_u32(bytes.len() as u32).await?;
        stream.write_all(&bytes).await
    })
    .await??;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn tickets_are_owned_by_peer_pid_and_consumed_once() {
        let registry = Registry::default();
        let (owner, mut requests) = registry.register(123).unwrap();
        let worker = tokio::spawn(open(owner.clone(), relay::Source::OnePassword));
        let request = requests.recv().await.unwrap();
        assert!(registry.ticket(456, request.ticket).is_err());
        let sender = registry.ticket(123, request.ticket).unwrap();
        assert!(registry.ticket(123, request.ticket).is_err());
        let (stream, _peer) = asynchronous::pair().await.unwrap();
        sender.send(Ok(stream)).unwrap();
        assert!(worker.await.unwrap().is_ok());
        assert!(owner.pending.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn disconnect_cancels_pending_connections_and_allows_new_registration() {
        let registry = Registry::default();
        let (owner, mut requests) = registry.register(123).unwrap();
        assert!(registry.register(456).is_err());
        let worker = tokio::spawn(open(owner.clone(), relay::Source::System));
        let ticket = requests.recv().await.unwrap().ticket;
        registry.disconnect(&owner);
        assert!(worker.await.unwrap().unwrap_err().contains("disconnected"));
        let (replacement, _requests) = registry.register(456).unwrap();
        assert!(registry.ticket(456, ticket).is_err());
        registry.disconnect(&owner);
        assert_eq!(registry.owner().unwrap().id, replacement.id);
    }

    #[tokio::test]
    async fn cancelled_authentication_removes_its_ticket() {
        let registry = Registry::default();
        let (owner, mut requests) = registry.register(123).unwrap();
        let worker = tokio::spawn(open(owner.clone(), relay::Source::System));
        let ticket = requests.recv().await.unwrap().ticket;
        worker.abort();
        assert!(worker.await.unwrap_err().is_cancelled());
        assert!(registry.ticket(123, ticket).is_err());
        assert!(owner.pending.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn outstanding_connections_are_bounded() {
        let registry = Registry::default();
        let (owner, mut requests) = registry.register(123).unwrap();
        let mut workers = Vec::new();
        for _ in 0..relay::MAX_CONNECTIONS {
            workers.push(tokio::spawn(open(owner.clone(), relay::Source::System)));
            requests.recv().await.unwrap();
        }
        assert!(
            open(owner.clone(), relay::Source::System)
                .await
                .unwrap_err()
                .contains("busy")
        );
        registry.disconnect(&owner);
        for worker in workers {
            assert!(worker.await.unwrap().is_err());
        }
    }
}
