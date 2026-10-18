//! Preserve a native request's timeout budget while its server awaits consent.
use newport_platform::ipc::asynchronous::Stream;
use newport_platform::ipc::protocol::{AgentApproval, VERSION};
use serde_json::Value;
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use uuid::Uuid;

pub(super) async fn dispatch(
    stream: &mut Stream,
    method: &str,
    args: &Value,
    operation: impl std::future::Future<Output = Result<Value, Value>>,
) -> anyhow::Result<Result<Value, Value>> {
    let server = args
        .get("serverId")
        .or_else(|| args.get("id"))
        .or_else(|| {
            args.get("expectedConnection")
                .and_then(|server| server.get("id"))
        })
        .or_else(|| args.get("server").and_then(|server| server.get("id")))
        .and_then(Value::as_str)
        .and_then(|id| Uuid::parse_str(id).ok());
    with_progress(stream, server, cancel_on_disconnect(method), operation).await
}

// Drop abandoned reads before another Retry can queue behind their auth wait.
// File transfers already support cancellation by dropping their future: they
// publish only completed staging files, and their guards remove unfinished
// staging files and release operation slots. Tie those transfers to the caller
// as well, including a crashed client whose Drop handler cannot send files_cancel.
// Git and other writes retain their outcome/recovery semantics on disconnection.
fn cancel_on_disconnect(method: &str) -> bool {
    matches!(
        method,
        newport_platform::ipc::protocol::ACTIVATE_WINDOW
            | "test_connection"
            | "setup_inspect"
            | "discover_ports"
            | "cockpit_collect"
            | "services_snapshot"
            | "service_logs"
            | "service_unit_file"
            | "containers_snapshot"
            | "container_inspect"
            | "container_logs"
            | "docker_disk"
            | "compose_project"
            | "compose_job"
            | "compose_jobs"
            | "files_watch"
            | "files_list"
            | "files_preview"
            | "files_read_text"
            | "files_project_index"
            | "files_write_text"
            | "files_upload"
            | "files_download"
            | "native.files_upload"
            | "native.files_download"
    )
}

async fn with_progress<W: AsyncRead + AsyncWrite + Unpin>(
    stream: &mut W,
    server: Option<Uuid>,
    cancel_on_disconnect: bool,
    operation: impl std::future::Future<Output = Result<Value, Value>>,
) -> anyhow::Result<Result<Value, Value>> {
    let mut approvals = crate::agent_approval::subscribe();
    let mut awaiting = false;
    let mut heartbeat = tokio::time::interval(Duration::from_secs(5));
    heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let (mut reader, mut writer) = tokio::io::split(stream);
    tokio::pin!(operation);
    loop {
        let current = server.is_some_and(|id| approvals.borrow_and_update().contains_key(&id));
        if current != awaiting {
            if let Err(error) = write_progress(&mut writer, current).await {
                return if cancel_on_disconnect {
                    Err(error)
                } else {
                    Ok(operation.await)
                };
            }
            awaiting = current;
            heartbeat.reset();
        }
        tokio::select! {
            result = &mut operation => return Ok(result),
            _ = reader.read_u8(), if cancel_on_disconnect => {
                anyhow::bail!("Native request disconnected");
            },
            _ = approvals.changed() => {},
            _ = heartbeat.tick(), if awaiting => {
                if let Err(error) = write_progress(&mut writer, true).await {
                    return if cancel_on_disconnect { Err(error) } else { Ok(operation.await) };
                }
            },
        }
    }
}

async fn write_progress<W: AsyncWrite + Unpin>(
    stream: &mut W,
    awaiting: bool,
) -> anyhow::Result<()> {
    let data = rmp_serde::to_vec_named(&AgentApproval {
        version: VERSION,
        awaiting_agent: awaiting,
    })?;
    tokio::time::timeout(Duration::from_secs(5), async {
        stream.write_u32(data.len() as u32).await?;
        stream.write_all(&data).await
    })
    .await??;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncReadExt;

    #[tokio::test(start_paused = true)]
    async fn queued_request_receives_scoped_consent_heartbeats_and_resumes() {
        let server = Uuid::new_v4();
        let (mut writer, mut reader) = tokio::io::duplex(1024);
        let wait = crate::agent_approval::Wait::begin(server);
        let task = tokio::spawn(async move {
            with_progress(&mut writer, Some(server), true, async {
                tokio::time::sleep(Duration::from_secs(65)).await;
                drop(wait);
                tokio::time::sleep(Duration::from_secs(1)).await;
                Ok(Value::Bool(true))
            })
            .await
            .unwrap()
        });
        let mut pulses = 0;
        while let Ok(len) = reader.read_u32().await {
            let mut data = vec![0; len as usize];
            reader.read_exact(&mut data).await.unwrap();
            let progress: AgentApproval = rmp_serde::from_slice(&data).unwrap();
            assert_eq!(progress.version, VERSION);
            if !progress.awaiting_agent {
                break;
            }
            pulses += 1;
        }
        assert!(pulses >= 12);
        assert_eq!(task.await.unwrap().unwrap(), Value::Bool(true));
    }

    #[tokio::test(start_paused = true)]
    async fn unrelated_server_wait_does_not_extend_a_request() {
        let _wait = crate::agent_approval::Wait::begin(Uuid::new_v4());
        let (mut writer, mut reader) = tokio::io::duplex(1024);
        with_progress(&mut writer, Some(Uuid::new_v4()), true, async {
            tokio::time::sleep(Duration::from_secs(10)).await;
            Ok(Value::Null)
        })
        .await
        .unwrap()
        .unwrap();
        drop(writer);
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes).await.unwrap();
        assert!(bytes.is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn disconnect_cancels_reads_but_never_cancels_a_write() {
        for read in [true, false] {
            let server = Uuid::new_v4();
            let (mut writer, reader) = tokio::io::duplex(1024);
            drop(reader);
            let wait = crate::agent_approval::Wait::begin(server);
            let completed = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
            let flag = completed.clone();
            let result = with_progress(&mut writer, Some(server), read, async move {
                let _wait = wait;
                tokio::time::sleep(Duration::from_secs(60)).await;
                flag.store(true, std::sync::atomic::Ordering::SeqCst);
                Ok(Value::Null)
            })
            .await;
            assert_eq!(result.is_err(), read);
            assert_eq!(completed.load(std::sync::atomic::Ordering::SeqCst), !read);
            assert!(
                !crate::agent_approval::subscribe()
                    .borrow()
                    .contains_key(&server)
            );
        }
    }

    #[test]
    fn mutation_methods_never_use_disconnect_cancellation() {
        for method in [
            "compose_run",
            "container_action",
            "git_request",
            "service_action",
            "terminal_open",
            "unknown",
        ] {
            assert!(!cancel_on_disconnect(method));
        }
        assert!(cancel_on_disconnect("containers_snapshot"));
        assert!(cancel_on_disconnect("test_connection"));
        assert!(cancel_on_disconnect(
            newport_platform::ipc::protocol::ACTIVATE_WINDOW
        ));
    }

    #[tokio::test]
    async fn abandoned_owned_requests_drop_work_without_waiting_for_agent_consent() {
        use std::sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        };
        struct Cleanup(Arc<AtomicBool>);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                self.0.store(true, Ordering::SeqCst);
            }
        }
        for method in [
            newport_platform::ipc::protocol::ACTIVATE_WINDOW,
            "files_list",
            "files_preview",
            "files_upload",
            "files_download",
            "native.files_upload",
            "native.files_download",
        ] {
            let (mut stream, client) = newport_platform::ipc::asynchronous::pair().await.unwrap();
            let dropped = Arc::new(AtomicBool::new(false));
            let cleanup = Cleanup(dropped.clone());
            let (started, ready) = tokio::sync::oneshot::channel();
            let task = tokio::spawn(async move {
                dispatch(&mut stream, method, &serde_json::json!({}), async move {
                    let _cleanup = cleanup;
                    started.send(()).unwrap();
                    std::future::pending().await
                })
                .await
            });
            ready.await.unwrap();
            assert!(!dropped.load(Ordering::SeqCst));
            drop(client);
            let result = tokio::time::timeout(Duration::from_secs(2), task)
                .await
                .expect("A crashed client must not retain its owned operation")
                .unwrap();
            assert!(result.is_err(), "{method}");
            assert!(
                dropped.load(Ordering::SeqCst),
                "{method} did not run cleanup"
            );
        }
    }
}
