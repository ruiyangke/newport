//! A terminal is held until the connection that opened it ends.
use super::{Host, TerminalOpen};
use newport_platform::ipc::asynchronous::Stream;
use newport_platform::ipc::protocol::{Response, VERSION};
use serde_json::{Value, json};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

pub(super) async fn serve<H: Host>(stream: Stream, host: &H, args: Value) -> anyhow::Result<()> {
    hold(stream, async move {
        let request: TerminalOpen =
            serde_json::from_value(args).map_err(|error| error.to_string())?;
        host.open_terminal(request).await
    })
    .await
}

async fn hold<W, G>(
    mut stream: W,
    open: impl std::future::Future<Output = Result<G, String>>,
) -> anyhow::Result<()>
where
    W: AsyncRead + AsyncWrite + Unpin,
{
    let result = tokio::select! {
        result = open => result,
        _ = stream.read_u8() => return Ok(()),
    };
    let response = Response {
        version: VERSION,
        result: Value::Null,
        error: result.as_ref().err().map(|error| json!(error)),
    };
    // Keep the guard alive while acknowledging and waiting. Failed responses,
    // EOF, unexpected client data, or cancellation all release only this PTY.
    let guard = result.ok();
    let bytes = rmp_serde::to_vec_named(&response)?;
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        stream.write_u32(bytes.len() as u32).await?;
        stream.write_all(&bytes).await
    })
    .await??;
    if guard.is_some() {
        let _ = stream.read_u8().await;
    }
    drop(guard);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    use uuid::Uuid;
    struct Guard(Arc<Mutex<Vec<Uuid>>>, Uuid);
    impl Drop for Guard {
        fn drop(&mut self) {
            self.0.lock().unwrap().retain(|id| *id != self.1);
        }
    }
    async fn response(stream: &mut tokio::io::DuplexStream) -> Response {
        let len = stream.read_u32().await.unwrap();
        let mut data = vec![0; len as usize];
        stream.read_exact(&mut data).await.unwrap();
        rmp_serde::from_slice(&data).unwrap()
    }
    #[tokio::test]
    async fn disconnect_releases_only_the_owned_session_and_allows_repeated_opens() {
        let unrelated = Uuid::new_v4();
        let sessions = Arc::new(Mutex::new(vec![unrelated]));
        for _ in 0..16 {
            let (mut client, server) = tokio::io::duplex(1024);
            let state = sessions.clone();
            let id = Uuid::new_v4();
            let task = tokio::spawn(hold(server, async move {
                state.lock().unwrap().push(id);
                Ok(Guard(state, id))
            }));
            assert!(response(&mut client).await.error.is_none());
            assert_eq!(*sessions.lock().unwrap(), [unrelated, id]);
            drop(client); // Same EOF as client process termination, without a close RPC.
            tokio::time::timeout(std::time::Duration::from_secs(1), task)
                .await
                .unwrap()
                .unwrap()
                .unwrap();
            assert_eq!(*sessions.lock().unwrap(), [unrelated]);
        }
    }
    #[tokio::test]
    async fn lost_open_acknowledgement_releases_the_created_session() {
        let (client, server) = tokio::io::duplex(1);
        let id = Uuid::new_v4();
        let sessions = Arc::new(Mutex::new(vec![]));
        let state = sessions.clone();
        let (opened, ready) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(hold(server, async move {
            state.lock().unwrap().push(id);
            let _ = opened.send(());
            Ok(Guard(state, id))
        }));
        ready.await.unwrap();
        // The one-byte buffer cannot contain the full response header.
        drop(client);
        assert!(task.await.unwrap().is_err());
        assert!(sessions.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn failed_open_never_closes_an_existing_session() {
        let (mut client, server) = tokio::io::duplex(1024);
        let task = tokio::spawn(hold(server, async {
            Err::<Guard, _>("Terminal session already exists.".into())
        }));
        assert_eq!(
            response(&mut client).await.error,
            Some(json!("Terminal session already exists."))
        );
        task.await.unwrap().unwrap();
    }
    #[tokio::test]
    async fn disconnect_cancels_pending_open_and_task_cancellation_drops_guard() {
        let (client, server) = tokio::io::duplex(1024);
        let task = tokio::spawn(hold(
            server,
            std::future::pending::<Result<Guard, String>>(),
        ));
        drop(client);
        tokio::time::timeout(std::time::Duration::from_secs(1), task)
            .await
            .unwrap()
            .unwrap()
            .unwrap();

        let (mut client, server) = tokio::io::duplex(1024);
        let id = Uuid::new_v4();
        let sessions = Arc::new(Mutex::new(vec![id]));
        let state = sessions.clone();
        let task = tokio::spawn(hold(server, async move { Ok(Guard(state, id)) }));
        assert!(response(&mut client).await.error.is_none());
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        assert!(sessions.lock().unwrap().is_empty());
    }
}
