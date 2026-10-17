use crate::{model::Server, ssh};
use serde::Serialize;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::sync::mpsc;
use uuid::Uuid;

#[derive(Debug, Serialize)]
#[serde(tag = "type", content = "data", rename_all = "camelCase")]
pub enum Event {
    Ready,
    Data(Vec<u8>),
    Reconnecting { attempt: u32, message: String },
    Exit(Option<u32>),
    Error(String),
}
pub enum Input {
    Data(Vec<u8>),
    Resize(u32, u32),
}
struct Session {
    server: Uuid,
    owner: Arc<crate::server_session::ServerSession>,
    input: mpsc::Sender<Input>,
    output: Arc<tokio::sync::Mutex<mpsc::Receiver<Event>>>,
    task: tokio::task::JoinHandle<()>,
}
#[derive(Clone, Default)]
pub struct Sessions {
    inner: Arc<SessionRegistry>,
}
#[derive(Default)]
struct SessionRegistry {
    entries: Mutex<HashMap<Uuid, Session>>,
    cancelled: Mutex<Vec<tokio::task::JoinHandle<()>>>,
}

pub(crate) struct Closing(Vec<tokio::task::JoinHandle<()>>);
impl Closing {
    pub(crate) async fn wait(mut self) {
        for task in &mut self.0 {
            let _ = task.await;
        }
    }
}
impl Drop for Closing {
    fn drop(&mut self) {
        for task in &self.0 {
            task.abort();
        }
    }
}

pub fn size(cols: u32, rows: u32) -> Result<(), String> {
    if !(2..=500).contains(&cols) || !(1..=300).contains(&rows) {
        return Err("Invalid terminal dimensions.".into());
    }
    Ok(())
}
impl Sessions {
    pub(crate) fn link_owner(
        &self,
        id: Uuid,
    ) -> Result<Arc<crate::server_session::ServerSession>, String> {
        let sessions = self.inner.entries.lock().unwrap();
        let session = sessions
            .get(&id)
            .ok_or("Terminal is closed. Reconnect before opening a server link.")?;
        if session.owner.is_revoked() {
            return Err(
                "The terminal's server connection changed. Reconnect before opening its links."
                    .into(),
            );
        }
        Ok(session.owner.clone())
    }
    /// Lease drop cannot await. Abort synchronously and retain the task so the
    /// application can join it before releasing profile ownership.
    pub fn cancel(&self, id: Uuid) {
        if let Some(session) = self.inner.entries.lock().unwrap().remove(&id) {
            session.task.abort();
            let mut cancelled = self.inner.cancelled.lock().unwrap();
            cancelled.retain(|task| !task.is_finished());
            cancelled.push(session.task);
        }
    }
    pub fn open_bound(
        &self,
        id: Uuid,
        server: Server,
        cols: u32,
        rows: u32,
        owner: Arc<crate::server_session::ServerSession>,
    ) -> Result<(), String> {
        size(cols, rows)?;
        let mut sessions = self.inner.entries.lock().unwrap();
        if sessions.contains_key(&id) {
            return Err("Terminal session already exists.".into());
        }
        if sessions.len() >= 8 {
            return Err("Close an existing terminal before opening another.".into());
        }
        let (input, rx) = mpsc::channel(32);
        let (tx, output) = mpsc::channel(32);
        let server_id = server.id;
        // Opening/reopening a tab is an explicit retry after a permanent error.
        owner.retry_transport();
        let task_owner = owner.clone();
        let task = tokio::spawn(async move {
            if let Err(error) =
                ssh::terminal_with_session(task_owner, &server, cols, rows, rx, &tx).await
            {
                let _ = tx.send(Event::Error(format!("{error:#}"))).await;
            }
        });
        sessions.insert(
            id,
            Session {
                server: server_id,
                owner,
                input,
                output: Arc::new(tokio::sync::Mutex::new(output)),
                task,
            },
        );
        Ok(())
    }
    pub async fn read(&self, id: Uuid) -> Result<Option<Event>, String> {
        let output = self
            .inner
            .entries
            .lock()
            .unwrap()
            .get(&id)
            .map(|s| s.output.clone())
            .ok_or("Terminal is closed.")?;
        let mut output = output
            .try_lock()
            .map_err(|_| "Terminal already has a reader.")?;
        match tokio::time::timeout(Duration::from_millis(500), output.recv()).await {
            Ok(Some(event)) => Ok(Some(event)),
            Ok(None) => Ok(Some(Event::Exit(None))),
            Err(_) => Ok(None),
        }
    }
    pub async fn send(&self, id: Uuid, input: Input) -> Result<(), String> {
        let sender = self
            .inner
            .entries
            .lock()
            .unwrap()
            .get(&id)
            .map(|session| session.input.clone())
            .ok_or("Terminal is closed.")?;
        // Fast typing can outrun the SSH channel briefly. Apply backpressure
        // instead of treating a full bounded queue as a failed connection.
        // Never hold the session map lock while waiting: close must cancel us.
        sender
            .send(input)
            .await
            .map_err(|_| "Terminal is disconnected. Reconnect if needed.".into())
    }
    pub async fn close(&self, id: Uuid) {
        let session = self.inner.entries.lock().unwrap().remove(&id);
        if let Some(session) = session {
            session.task.abort();
            let _ = session.task.await;
        }
    }
    /// Retired-owner cleanup must not close tabs opened for its replacement,
    /// even when both connections use the same saved server ID.
    pub(crate) fn begin_close_owner(
        &self,
        owner: &Arc<crate::server_session::ServerSession>,
    ) -> Closing {
        self.begin_close_matching(|session| Arc::ptr_eq(&session.owner, owner), false)
    }

    pub(crate) fn begin_close_server(&self, server: Option<Uuid>) -> Closing {
        self.begin_close_matching(|session| server.is_none_or(|id| id == session.server), true)
    }

    fn begin_close_matching(
        &self,
        matches: impl Fn(&Session) -> bool,
        join_cancelled: bool,
    ) -> Closing {
        let mut sessions = self.inner.entries.lock().unwrap();
        let ids: Vec<_> = sessions
            .iter()
            .filter(|(_, s)| matches(s))
            .map(|(id, _)| *id)
            .collect();
        let mut cancelled = if join_cancelled {
            std::mem::take(&mut *self.inner.cancelled.lock().unwrap())
        } else {
            Vec::new()
        };
        for id in ids {
            let session = sessions.remove(&id).unwrap();
            session.task.abort();
            cancelled.push(session.task);
        }
        Closing(cancelled)
    }
    pub async fn close_server(&self, server: Option<Uuid>) {
        self.begin_close_server(server).wait().await;
    }
}
impl Drop for SessionRegistry {
    fn drop(&mut self) {
        for (_, session) in self.entries.get_mut().unwrap().drain() {
            session.task.abort();
        }
        for task in self.cancelled.get_mut().unwrap().drain(..) {
            task.abort();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_owner(id: Uuid, revision: u64) -> Arc<crate::server_session::ServerSession> {
        let server: Server = serde_json::from_value(serde_json::json!({
            "id":id,"name":"Terminal fixture","sshHost":"unconnected.invalid","sshUser":"fixture","sshPort":22
        })).unwrap();
        crate::server_session::Registry::default().session(&server, revision)
    }

    fn queued_session(
        input: mpsc::Sender<Input>,
        task: tokio::task::JoinHandle<()>,
    ) -> (Sessions, Uuid) {
        let sessions = Sessions::default();
        let id = Uuid::new_v4();
        let (_, output) = mpsc::channel(1);
        sessions.inner.entries.lock().unwrap().insert(
            id,
            Session {
                server: Uuid::nil(),
                owner: test_owner(Uuid::nil(), 0),
                input,
                output: Arc::new(tokio::sync::Mutex::new(output)),
                task,
            },
        );
        (sessions, id)
    }

    #[tokio::test]
    async fn retired_owner_cleanup_preserves_replacement_tabs_already_open() {
        let (input, _rx) = mpsc::channel(1);
        let (sessions, old_id) = queued_session(input, tokio::spawn(std::future::pending()));
        let owner = sessions.inner.entries.lock().unwrap()[&old_id]
            .owner
            .clone();
        let server = owner.server().id;
        let replacement_id = Uuid::new_v4();
        let (input, mut received) = mpsc::channel(1);
        let (_output, output) = mpsc::channel(1);
        let replacement = tokio::spawn(std::future::pending());
        let active = replacement.abort_handle();
        sessions.inner.entries.lock().unwrap().insert(
            replacement_id,
            Session {
                server,
                owner: test_owner(server, 1),
                input,
                output: Arc::new(tokio::sync::Mutex::new(output)),
                task: replacement,
            },
        );
        let cleanup_handle = sessions.clone();
        drop(sessions);
        cleanup_handle.begin_close_owner(&owner).wait().await;
        assert!(
            !cleanup_handle
                .inner
                .entries
                .lock()
                .unwrap()
                .contains_key(&old_id)
        );
        assert!(!active.is_finished());
        cleanup_handle
            .send(replacement_id, Input::Data(vec![42]))
            .await
            .unwrap();
        assert!(matches!(received.recv().await, Some(Input::Data(bytes)) if bytes == [42]));
        cleanup_handle.close_server(None).await;
        assert!(active.is_finished());
    }

    #[tokio::test]
    async fn captured_cleanup_does_not_close_a_replacement_terminal() {
        let (input, _rx) = mpsc::channel(1);
        let old = tokio::spawn(std::future::pending());
        let (sessions, id) = queued_session(input, old);
        let server = sessions.inner.entries.lock().unwrap()[&id].server;
        let cleanup = sessions.begin_close_server(Some(server));
        assert!(!sessions.inner.entries.lock().unwrap().contains_key(&id));

        let (input, mut received) = mpsc::channel(1);
        let (_output, output) = mpsc::channel(1);
        let replacement = tokio::spawn(std::future::pending());
        let active = replacement.abort_handle();
        sessions.inner.entries.lock().unwrap().insert(
            id,
            Session {
                server,
                owner: test_owner(server, 1),
                input,
                output: Arc::new(tokio::sync::Mutex::new(output)),
                task: replacement,
            },
        );
        cleanup.wait().await;
        assert!(!active.is_finished());
        sessions.send(id, Input::Data(vec![42])).await.unwrap();
        assert!(matches!(received.recv().await, Some(Input::Data(bytes)) if bytes == [42]));
        sessions.close_server(None).await;
        assert!(active.is_finished());
    }

    #[tokio::test]
    async fn lease_cancellation_is_synchronous_and_shutdown_joins_the_task() {
        let (input, _rx) = mpsc::channel(1);
        let (ready, started) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(async move {
            ready.send(()).unwrap();
            std::future::pending::<()>().await;
        });
        let abort = task.abort_handle();
        started.await.unwrap();
        let (sessions, id) = queued_session(input, task);
        sessions.cancel(id);
        assert!(
            sessions
                .read(id)
                .await
                .unwrap_err()
                .contains("Terminal is closed")
        );
        sessions.close_server(None).await;
        assert!(abort.is_finished());
        assert!(sessions.inner.cancelled.lock().unwrap().is_empty());
        sessions.cancel(id);
    }

    #[tokio::test]
    async fn input_waits_for_capacity_without_holding_the_session_map() {
        let (input, mut rx) = mpsc::channel(1);
        let (sessions, id) = queued_session(input, tokio::spawn(std::future::pending()));
        sessions
            .send(id, Input::Data(b"first".to_vec()))
            .await
            .unwrap();
        let waiting = sessions.send(id, Input::Resize(100, 40));
        tokio::pin!(waiting);
        tokio::select! {
            biased;
            result = &mut waiting => panic!("full input queue should wait, got {result:?}"),
            _ = tokio::time::sleep(Duration::from_millis(10)) => {},
        }
        assert!(
            sessions.inner.entries.try_lock().is_ok(),
            "pending input held the session map"
        );
        assert!(matches!(rx.recv().await, Some(Input::Data(bytes)) if bytes == b"first"));
        tokio::time::timeout(Duration::from_secs(1), waiting)
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(rx.recv().await, Some(Input::Resize(100, 40))));
        sessions.close(id).await;
    }

    #[tokio::test]
    async fn closing_cancels_input_waiting_for_capacity() {
        let (input, rx) = mpsc::channel(1);
        let task = tokio::spawn(async move {
            std::future::pending::<()>().await;
            drop(rx);
        });
        let (sessions, id) = queued_session(input, task);
        sessions
            .send(id, Input::Data(b"first".to_vec()))
            .await
            .unwrap();
        let waiting = sessions.send(id, Input::Data(b"second".to_vec()));
        tokio::pin!(waiting);
        tokio::select! {
            biased;
            result = &mut waiting => panic!("full input queue should wait, got {result:?}"),
            _ = tokio::time::sleep(Duration::from_millis(10)) => {},
        }
        tokio::time::timeout(Duration::from_secs(1), sessions.close(id))
            .await
            .unwrap();
        assert!(
            tokio::time::timeout(Duration::from_secs(1), waiting)
                .await
                .unwrap()
                .is_err()
        );
    }

    fn fixture() -> Server {
        Server {
            id: Uuid::new_v4(),
            name: "Terminal fixture".into(),
            ssh_user: "fixture".into(),
            ssh_host: "127.0.0.1".into(),
            ssh_port: std::env::var("NEWPORT_TEST_SSH_PORT")
                .unwrap()
                .parse()
                .unwrap(),
            identity_file: Some(std::env::var("NEWPORT_TEST_IDENTITY").unwrap()),
            agent_source: None,
            agent_key_fingerprint: None,
            auth_method: crate::model::AuthMethod::PublicKey,
            clipboard_enabled: false,
            browser_enabled: false,
            agent_install_approved: true,
            setup_complete: true,
        }
    }
    async fn until(sessions: &Sessions, id: Uuid, needle: &str) -> String {
        tokio::time::timeout(Duration::from_secs(8), async {
            let mut text = String::new();
            loop {
                match sessions.read(id).await.unwrap() {
                    Some(Event::Data(bytes)) => {
                        text.push_str(&String::from_utf8_lossy(&bytes));
                        if text.contains(needle) {
                            return text;
                        }
                    }
                    Some(Event::Error(error)) => panic!("{error}"),
                    Some(Event::Exit(code)) => panic!("Unexpected exit {code:?}: {text}"),
                    _ => {}
                }
            }
        })
        .await
        .unwrap()
    }
    #[tokio::test]
    #[ignore = "Run with scripts/test-ssh.sh; runs a real Bash PTY in an isolated HOME"]
    async fn interactive_pty_input_resize_interrupt_exit_and_close() {
        let sessions = Sessions::default();
        let server = fixture();
        let id = Uuid::new_v4();
        let registry = crate::server_session::Registry::default();
        let owner = registry.session(&server, 0);
        let integration = owner.integration_session_id();
        sessions
            .open_bound(id, server.clone(), 80, 24, owner.clone())
            .unwrap();
        until(&sessions, id, "early shell output").await;
        until(&sessions, id, "fixture> ").await;
        sessions
            .send(
                id,
                Input::Data(b"export NEWPORT_VALUE=persistent\r".to_vec()),
            )
            .await
            .unwrap();
        until(&sessions, id, "fixture> ").await;
        sessions
            .send(
                id,
                Input::Data(b"printf 'value=%s\\n' \"$NEWPORT_VALUE\"\r".to_vec()),
            )
            .await
            .unwrap();
        until(&sessions, id, "value=persistent").await;
        sessions
            .send(
                id,
                Input::Data(b"printf 'session=%s\\n' \"$NEWPORT_SESSION\"\r".to_vec()),
            )
            .await
            .unwrap();
        until(&sessions, id, &format!("session={integration}")).await;
        sessions.send(id, Input::Resize(100, 40)).await.unwrap();
        sessions
            .send(id, Input::Data(b"stty size\r".to_vec()))
            .await
            .unwrap();
        until(&sessions, id, "40 100").await;
        sessions
            .send(id, Input::Data(b"sleep 60\r".to_vec()))
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(150)).await;
        sessions.send(id, Input::Data(vec![3])).await.unwrap();
        until(&sessions, id, "^C").await;
        sessions
            .send(id, Input::Data(b"exit 7\r".to_vec()))
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                match sessions.read(id).await.unwrap() {
                    Some(Event::Exit(code)) => {
                        assert_eq!(code, Some(7));
                        break;
                    }
                    Some(Event::Data(_)) => {}
                    Some(Event::Error(error)) => panic!("{error}"),
                    _ => {}
                }
            }
        })
        .await
        .unwrap();
        sessions.close(id).await;
        assert!(sessions.read(id).await.is_err());
        let rejected = Uuid::new_v4();
        sessions
            .open_bound(rejected, server.clone(), 13, 24, owner.clone())
            .unwrap();
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if let Some(Event::Error(error)) = sessions.read(rejected).await.unwrap() {
                    assert!(error.contains("refused"));
                    break;
                }
            }
        })
        .await
        .unwrap();
        sessions.close_server(Some(server.id)).await;
        assert!(sessions.inner.entries.lock().unwrap().is_empty());
        let cancelled = Uuid::new_v4();
        sessions
            .open_bound(cancelled, server, 80, 24, owner)
            .unwrap();
        sessions.close(cancelled).await;
        assert!(sessions.inner.entries.lock().unwrap().is_empty());
    }
    #[tokio::test]
    #[ignore = "Run with scripts/test-ssh.sh terminal::tests"]
    async fn terminals_share_transport_without_needle_and_close_only_their_shell() {
        let sessions = Sessions::default();
        let mut server = fixture();
        server.agent_install_approved = false;
        server.setup_complete = false;
        let registry = crate::server_session::Registry::default();
        let owner = registry.session(&server, 0);
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        sessions
            .open_bound(a, server.clone(), 80, 24, owner.clone())
            .unwrap();
        sessions
            .open_bound(b, server.clone(), 80, 24, owner.clone())
            .unwrap();
        until(&sessions, a, "fixture> ").await;
        until(&sessions, b, "fixture> ").await;
        let lease = owner.acquire(&server).await.unwrap();
        assert_eq!(owner.transport_snapshot().generation, 1);
        assert!(owner.agent_runtime.snapshot().hello.is_none());
        let pid_file = format!("terminal-{a}.pid");
        sessions
            .send(
                a,
                Input::Data(format!("echo $$ > \"$HOME/{pid_file}\"\r").into_bytes()),
            )
            .await
            .unwrap();
        until(&sessions, a, "fixture> ").await;
        sessions.close(a).await;
        // The surviving shell observes that the closed tab's shell was reaped,
        // while its own channel and an ordinary exec lease remain usable.
        sessions.send(b, Input::Data(format!(
            "for i in {{1..100}}; do kill -0 \"$(cat \"$HOME/{pid_file}\")\" 2>/dev/null || {{ printf 'channel-%s\\n' released; break; }}; sleep 0.03; done; rm -f \"$HOME/{pid_file}\"\r"
        ).into_bytes())).await.unwrap();
        until(&sessions, b, "channel-released").await;
        assert_eq!(lease.execute("printf ok", None).await.unwrap(), "ok");
        assert_eq!(owner.transport_snapshot().generation, 1);
        // Changing the profile revokes an active terminal without moving it to
        // the new connection or leaking its demand.
        registry.invalidate(server.id);
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if let Some(Event::Error(error)) = sessions.read(b).await.unwrap() {
                    assert!(error.contains("Server session changed"));
                    break;
                }
            }
        })
        .await
        .unwrap();
        sessions.close(b).await;
        assert_eq!(owner.transport_snapshot().demand, 1); // retained exec lease
        drop(lease);
        assert_eq!(owner.transport_snapshot().demand, 0);
        registry.shutdown().await;
    }
    #[tokio::test]
    #[ignore = "Run with scripts/test-ssh.sh; runs a real Bash PTY in an isolated HOME"]
    async fn interrupted_terminal_reconnects_with_a_fresh_shell() {
        let sessions = Sessions::default();
        let server = fixture();
        let id = Uuid::new_v4();
        let registry = crate::server_session::Registry::default();
        let owner = registry.session(&server, 0);
        sessions
            .open_bound(id, server.clone(), 80, 24, owner.clone())
            .unwrap();
        until(&sessions, id, "fixture> ").await;
        // The fixture drops every SSH connection, including this PTY's transport.
        let _ = ssh::execute(&server, "drop", None).await;
        tokio::time::timeout(Duration::from_secs(8), async {
            loop {
                match sessions.read(id).await.unwrap() {
                    Some(Event::Reconnecting { attempt, .. }) => {
                        assert!(attempt >= 1);
                        break;
                    }
                    Some(Event::Error(error)) => panic!("{error}"),
                    Some(Event::Exit(code)) => panic!("Unexpected exit {code:?}"),
                    _ => {}
                }
            }
        })
        .await
        .unwrap();
        // A fresh shell starts without user action.
        let replay_file = format!("terminal-replay-{id}");
        sessions
            .send(
                id,
                Input::Data(format!("touch \"$HOME/{replay_file}\"\r").into_bytes()),
            )
            .await
            .unwrap();
        sessions.send(id, Input::Resize(110, 42)).await.unwrap();
        until(&sessions, id, "fixture> ").await;
        sessions.send(id, Input::Data(format!("stty size; test ! -e \"$HOME/{replay_file}\" && printf 'replay-%s\\n' absent\r").into_bytes())).await.unwrap();
        let recovered = until(&sessions, id, "replay-absent").await;
        assert!(
            recovered.contains("42 110"),
            "resize lost during recovery: {recovered}"
        );
        sessions
            .send(id, Input::Data(b"printf 'new-shell\\n'\r".to_vec()))
            .await
            .unwrap();
        until(&sessions, id, "new-shell").await;
        assert_eq!(owner.transport_snapshot().generation, 2);
        sessions.close(id).await;
        registry.shutdown().await;
    }
}
