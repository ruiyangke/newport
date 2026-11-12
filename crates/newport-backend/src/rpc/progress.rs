//! Correlated consent progress, while retaining each operation's disconnect policy.
use super::transport;
use newport_protocol::desktop::{
    self, AgentApproval, DisconnectPolicy, Progress, ProgressState, Request, ServerMessage, wire,
};
use newport_ssh::{Login, Status};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite},
    sync::watch,
    time::Instant,
};

pub(super) async fn with_progress<T>(
    reader: &mut (impl AsyncRead + Unpin),
    writer: &mut (impl AsyncWrite + Unpin),
    request: &Request,
    status: Option<watch::Receiver<Status>>,
    operation: impl std::future::Future<Output = T>,
) -> anyhow::Result<T> {
    with_policy(
        reader,
        writer,
        request,
        status,
        request.operation.disconnect_policy(),
        operation,
    )
    .await
}

pub(super) async fn with_policy<T>(
    reader: &mut (impl AsyncRead + Unpin),
    writer: &mut (impl AsyncWrite + Unpin),
    request: &Request,
    mut status: Option<watch::Receiver<Status>>,
    policy: DisconnectPolicy,
    operation: impl std::future::Future<Output = T>,
) -> anyhow::Result<T> {
    let mut awaiting = false;
    let mut heartbeat = tokio::time::interval(desktop::CONSENT_HEARTBEAT);
    heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    tokio::pin!(operation);
    loop {
        let current = status.as_mut().is_some_and(|status| {
            matches!(
                status.borrow_and_update().login,
                Some(Login::WaitingForApproval)
            )
        });
        if current != awaiting {
            if let Err(error) = write_progress(writer, request, current).await {
                if policy == DisconnectPolicy::Complete {
                    let _ = operation.await;
                }
                return Err(error);
            }
            awaiting = current;
            heartbeat.reset();
        }
        tokio::select! {
            // A byte after the request is a protocol violation; EOF is
            // cancellation. Neither can dispatch a second operation.
            biased;
            _ = reader.read_u8() => {
                if policy == DisconnectPolicy::Complete { let _ = operation.await; }
                anyhow::bail!("Desktop request disconnected or sent trailing data");
            }
            result = &mut operation => return Ok(result),
            changed = async {
                match status.as_mut() {
                    Some(status) => status.changed().await,
                    None => std::future::pending().await,
                }
            } => { if changed.is_err() { status = None; } }
            _ = heartbeat.tick(), if awaiting => {
                if let Err(error) = write_progress(writer, request, true).await {
                    if policy == DisconnectPolicy::Complete { let _ = operation.await; }
                    return Err(error);
                }
            }
        }
    }
}

async fn write_progress(
    writer: &mut (impl AsyncWrite + Unpin),
    request: &Request,
    awaiting: bool,
) -> anyhow::Result<()> {
    let server = request
        .operation
        .server_id()
        .ok_or_else(|| anyhow::anyhow!("Consent progress has no server"))?;
    let message = ServerMessage::Progress(Box::new(Progress {
        id: request.id,
        state: ProgressState::AgentApproval(Box::new(AgentApproval { server, awaiting })),
    }));
    let data = wire::encode_server_for(&message, request)?;
    transport::write_until(writer, &data, Instant::now() + desktop::REFUSAL_TIMEOUT).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use newport_protocol::desktop::{Operation, RequestId, connection};
    use std::{
        sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        },
        time::Duration,
    };
    use tokio::io::AsyncReadExt;
    fn approving() -> (watch::Sender<Status>, watch::Receiver<Status>) {
        watch::channel(Status {
            reach: newport_ssh::Reach::Connecting,
            login: Some(Login::WaitingForApproval),
            waiting: 1,
        })
    }
    fn request() -> Request {
        Request {
            id: RequestId::new(17).unwrap(),
            operation: Operation::Connection(Box::new(connection::Operation::TestConnection(
                Box::new(connection::TestConnectionArgs {
                    server: uuid::Uuid::from_u128(5),
                }),
            ))),
        }
    }
    #[tokio::test(start_paused = true)]
    async fn queued_request_receives_correlated_consent_heartbeats_and_resumes() {
        let (server, client) = tokio::io::duplex(4096);
        let (mut input, mut output) = tokio::io::split(server);
        let (link, status) = approving();
        let task = tokio::spawn(async move {
            with_progress(&mut input, &mut output, &request(), Some(status), async {
                tokio::time::sleep(Duration::from_secs(65)).await;
                link.send_modify(|status| status.login = Some(Login::InProgress));
                tokio::time::sleep(Duration::from_secs(1)).await;
                true
            })
            .await
            .unwrap()
        });
        let mut frames = transport::Reader::new(client);
        let mut pulses = 0;
        while let Some(frame) = frames
            .frame(desktop::CONTROL_LIMITS.message, None)
            .await
            .unwrap()
        {
            let ServerMessage::Progress(progress) =
                wire::decode_server_for(frame.frame.body(), &request()).unwrap()
            else {
                panic!("consent progress")
            };
            assert_eq!(progress.id, request().id);
            let ProgressState::AgentApproval(approval) = progress.state;
            assert_eq!(Some(approval.server), request().operation.server_id());
            if !approval.awaiting {
                break;
            }
            pulses += 1;
        }
        assert!(pulses >= 12);
        assert!(task.await.unwrap());
    }
    #[tokio::test(start_paused = true)]
    async fn a_link_that_is_not_waiting_for_approval_sends_no_progress() {
        let (server, mut client) = tokio::io::duplex(1024);
        let (mut input, mut output) = tokio::io::split(server);
        let (_link, status) = watch::channel(Status::default());
        with_progress(&mut input, &mut output, &request(), Some(status), async {
            tokio::time::sleep(Duration::from_secs(10)).await;
        })
        .await
        .unwrap();
        drop(input);
        drop(output);
        let mut bytes = Vec::new();
        client.read_to_end(&mut bytes).await.unwrap();
        assert!(bytes.is_empty());
    }
    #[tokio::test(start_paused = true)]
    async fn disconnect_cancels_reads_but_never_cancels_a_write() {
        for policy in [DisconnectPolicy::Cancel, DisconnectPolicy::Complete] {
            let (server, client) = tokio::io::duplex(1024);
            let (mut input, mut output) = tokio::io::split(server);
            drop(client);
            let (_link, status) = approving();
            let completed = Arc::new(AtomicBool::new(false));
            let flag = completed.clone();
            assert!(
                with_policy(
                    &mut input,
                    &mut output,
                    &request(),
                    Some(status),
                    policy,
                    async move {
                        tokio::time::sleep(Duration::from_secs(60)).await;
                        flag.store(true, Ordering::SeqCst);
                    }
                )
                .await
                .is_err()
            );
            assert_eq!(
                completed.load(Ordering::SeqCst),
                policy == DisconnectPolicy::Complete
            );
        }
    }
    #[test]
    fn mutation_operations_never_use_disconnect_cancellation() {
        use newport_protocol::{
            common::Present,
            desktop::{configuration, host},
        };
        assert_eq!(
            request().operation.disconnect_policy(),
            DisconnectPolicy::Cancel
        );
        let mutation = Operation::Configuration(Box::new(configuration::Operation::DeleteServer(
            Box::new(configuration::DeleteServerArgs {
                server: uuid::Uuid::from_u128(1),
            }),
        )));
        assert_eq!(mutation.disconnect_policy(), DisconnectPolicy::Complete);
        let activate = Operation::Host(Box::new(host::Operation::Activate(Box::new(Present {}))));
        assert_eq!(activate.disconnect_policy(), DisconnectPolicy::Cancel);
    }
    #[tokio::test]
    async fn abandoned_owned_requests_drop_work_without_waiting_for_agent_consent() {
        struct Cleanup(Arc<AtomicBool>);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                self.0.store(true, Ordering::SeqCst);
            }
        }
        let (server, client) = tokio::io::duplex(1024);
        let dropped = Arc::new(AtomicBool::new(false));
        let cleanup = Cleanup(dropped.clone());
        let (started, ready) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(async move {
            let (mut input, mut output) = tokio::io::split(server);
            with_progress(&mut input, &mut output, &request(), None, async move {
                let _cleanup = cleanup;
                started.send(()).unwrap();
                std::future::pending::<()>().await
            })
            .await
        });
        ready.await.unwrap();
        assert!(!dropped.load(Ordering::SeqCst));
        drop(client);
        assert!(
            tokio::time::timeout(Duration::from_secs(1), task)
                .await
                .unwrap()
                .unwrap()
                .is_err()
        );
        assert!(dropped.load(Ordering::SeqCst));
    }
}
