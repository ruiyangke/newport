use crate::model::Server;
use serde::Serialize;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};
use tokio::sync::mpsc;
use uuid::Uuid;
pub(crate) mod display;
pub(crate) mod held_list;
pub(crate) mod holder_channel;
pub(crate) mod shell_channel;

/// What a tab's shell may use besides its server: the account's display,
/// where this computer may show it.
#[derive(Clone, Default)]
pub(crate) struct Shell {
    pub display: Option<display::Access>,
}

#[derive(Debug, Serialize)]
#[serde(tag = "type", content = "data", rename_all = "camelCase")]
pub enum Event {
    Ready,
    Data(Vec<u8>),
    Reconnecting {
        attempt: u32,
        message: String,
    },
    Exit(Option<u32>),
    Error(newport_protocol::common::Error),
    /// The shell's whole state, as output to replay into a reset view, and
    /// whether the shell has prompted (see `Replica::prompted`).
    Restore {
        state: Vec<u8>,
        prompted: bool,
    },
    /// The shell runs on the server apart from this connection.
    Persistent,
    /// The shell to attach to is gone.
    Ended,
    /// Whether this tab is its shell's active one: only that tab acts on
    /// what goes to one Mac, such as a program's clipboard copy. A shell that
    /// isn't held has only this Mac, and is always active.
    Active(bool),
    /// The account's display stopped showing on this Mac while the tab is
    /// its shell's active one: the tab's next use shows it again.
    DisplayLost,
    /// The held shell's size, which every tab with it open draws it at:
    /// the size of the computer last used, as tmux's `window-size latest`.
    Size {
        columns: u16,
        rows: u16,
    },
    /// Needle couldn't hold a new tab's shell, for this reason: an ordinary
    /// shell follows, and ends when the connection does.
    NotHeld(String),
}
/// A tab's foreground, background and cursor colours, which a holder
/// answers the program's colour queries with.
pub type Colors = Option<[[u8; 3]; 3]>;

/// Whether a terminal's shell is held on its server (`holder_channel`).
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Hold {
    /// The connection's own shell, which ends with it.
    No,
    /// Held when the server's Needle can hold it, else the connection's own.
    WhenAvailable(Colors),
    /// The terminal's held shell, never a new one: a listed shell, or a tab
    /// Newport restored.
    Attach(Colors),
}
pub enum Input {
    Data(Vec<u8>),
    Resize(u32, u32),
    /// End the shell. Closing the input only detaches from a persistent one.
    End,
    /// Someone used the terminal on this Mac: it becomes its held shell's
    /// active one, and shows the account's display here.
    Activate,
}
/// A terminal's input and output, carried to the app by its lease. Dropping
/// the output ends the shell.
pub struct Io {
    pub input: mpsc::Sender<Input>,
    pub output: mpsc::Receiver<Event>,
}
struct Session {
    server: Uuid,
    owner: Arc<crate::server_session::ServerSession>,
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
        let sessions = self
            .inner
            .entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
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
        if let Some(session) = self
            .inner
            .entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&id)
        {
            session.task.abort();
            let mut cancelled = self
                .inner
                .cancelled
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
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
    ) -> Result<Io, String> {
        self.open_bound_with_display(id, server, (cols, rows), owner, None, Hold::No)
    }
    pub(crate) fn open_bound_with_display(
        &self,
        id: Uuid,
        server: Server,
        (cols, rows): (u32, u32),
        owner: Arc<crate::server_session::ServerSession>,
        display: Option<display::Access>,
        hold: Hold,
    ) -> Result<Io, String> {
        size(cols, rows)?;
        let mut sessions = self
            .inner
            .entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if sessions.contains_key(&id) {
            return Err("Terminal session already exists.".into());
        }
        // The limit is each server's, as the app's tabs are.
        let open = sessions.values().filter(|s| s.server == server.id).count();
        if open >= newport_domain::terminal::MAX_TERMINALS {
            return Err("Close an existing terminal before opening another.".into());
        }
        let (input, rx) = mpsc::channel(32);
        let (tx, output) = mpsc::channel(32);
        let server_id = server.id;
        let task_owner = owner.clone();
        let task = tokio::spawn(async move {
            let shell = Shell { display };
            // A shell Needle can hold outlives this connection; otherwise it
            // is the connection's own, as on servers without Needle.
            let result = match hold {
                Hold::No => {
                    shell_channel::run(task_owner, &server, id, (cols, rows), rx, &tx, shell).await
                }
                Hold::WhenAvailable(colors) | Hold::Attach(colors) => {
                    let held = holder_channel::run(
                        task_owner.clone(),
                        &server,
                        id,
                        (cols, rows),
                        rx,
                        &tx,
                        shell,
                        matches!(hold, Hold::Attach(_)),
                        colors,
                    )
                    .await;
                    match held {
                        Ok(Some(fallback)) => {
                            shell_channel::run(
                                task_owner,
                                &server,
                                id,
                                fallback.size,
                                fallback.input,
                                &tx,
                                fallback.shell,
                            )
                            .await
                        }
                        Ok(None) => Ok(()),
                        Err(error) => Err(error),
                    }
                }
            };
            if let Err(error) = result {
                let _ = tx
                    .send(Event::Error(crate::rpc::failure::terminal(&error)))
                    .await;
            }
        });
        sessions.insert(
            id,
            Session {
                server: server_id,
                owner,
                task,
            },
        );
        Ok(Io { input, output })
    }
    pub async fn close(&self, id: Uuid) {
        let session = self
            .inner
            .entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&id);
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
        let mut sessions = self
            .inner
            .entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let ids: Vec<_> = sessions
            .iter()
            .filter(|(_, s)| matches(s))
            .map(|(id, _)| *id)
            .collect();
        let mut cancelled = if join_cancelled {
            std::mem::take(
                &mut *self
                    .inner
                    .cancelled
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner),
            )
        } else {
            Vec::new()
        };
        for id in ids {
            if let Some(session) = sessions.remove(&id) {
                session.task.abort();
                cancelled.push(session.task);
            }
        }
        Closing(cancelled)
    }
    pub async fn close_server(&self, server: Option<Uuid>) {
        self.begin_close_server(server).wait().await;
    }
}
impl Drop for SessionRegistry {
    fn drop(&mut self) {
        for (_, session) in self
            .entries
            .get_mut()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .drain()
        {
            session.task.abort();
        }
        for task in self
            .cancelled
            .get_mut()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .drain(..)
        {
            task.abort();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server_session::ServerSession;
    use newport_ssh::Reach;
    use std::time::Duration;

    fn test_owner(id: Uuid, revision: u64) -> Arc<crate::server_session::ServerSession> {
        let server: Server = serde_json::from_value(serde_json::json!({
            "id":id,"name":"Terminal fixture","sshHost":"unconnected.invalid","sshUser":"fixture","sshPort":22
        })).unwrap();
        crate::server_session::Registry::default().session(&server, revision)
    }

    fn insert(
        sessions: &Sessions,
        id: Uuid,
        server: Uuid,
        revision: u64,
        task: tokio::task::JoinHandle<()>,
    ) {
        sessions.inner.entries.lock().unwrap().insert(
            id,
            Session {
                server,
                owner: test_owner(server, revision),
                task,
            },
        );
    }

    #[tokio::test]
    async fn each_server_has_its_own_terminal_limit() {
        let sessions = Sessions::default();
        let full = Uuid::new_v4();
        for _ in 0..newport_domain::terminal::MAX_TERMINALS {
            let task = tokio::spawn(std::future::pending());
            insert(&sessions, Uuid::new_v4(), full, 0, task);
        }
        let open = |server: Uuid| {
            let owner = test_owner(server, 0);
            let config = owner.server().clone();
            sessions.open_bound(Uuid::new_v4(), config, 80, 24, owner)
        };
        // Another server's tabs don't count against this one.
        assert!(open(Uuid::new_v4()).is_ok());
        assert_eq!(
            open(full).err().as_deref(),
            Some("Close an existing terminal before opening another.")
        );
        sessions.close_server(None).await;
    }

    fn queued_session(task: tokio::task::JoinHandle<()>) -> (Sessions, Uuid) {
        let sessions = Sessions::default();
        let id = Uuid::new_v4();
        insert(&sessions, id, Uuid::nil(), 0, task);
        (sessions, id)
    }

    #[tokio::test]
    async fn the_display_is_never_on_the_way_to_the_ssh_dial() {
        // Checks that never answer or refuse, and a display that never
        // starts, don't hold up the shell: it dials at once.
        for refuses in [false, true] {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let server: Server = serde_json::from_value(serde_json::json!({
                "id":Uuid::new_v4(),"name":"Display access","sshHost":"127.0.0.1",
                "sshUser":"fixture","sshPort":listener.local_addr().unwrap().port()
            }))
            .unwrap();
            let registry = crate::server_session::Registry::default();
            let owner = registry.session(&server, 0);
            let sessions = Sessions::default();
            let checks: display::Check = Arc::new(move || {
                if refuses {
                    Box::pin(async { Err("Remote apps isn’t set up for this server".into()) })
                } else {
                    Box::pin(std::future::pending())
                }
            });
            let factory: display::Source = Arc::new(|| Box::pin(std::future::pending()));
            let _io = sessions
                .open_bound_with_display(
                    Uuid::new_v4(),
                    server,
                    (80, 24),
                    owner.clone(),
                    Some(display::Access::new(checks, factory)),
                    Hold::No,
                )
                .unwrap();
            tokio::time::timeout(Duration::from_secs(2), listener.accept())
                .await
                .expect("the shell must dial without waiting for a display")
                .unwrap();
            assert!(owner.graphical_sessions().is_empty());
            sessions.close_server(None).await;
            registry.shutdown().await;
        }
    }

    #[tokio::test]
    async fn retired_owner_cleanup_preserves_replacement_tabs_already_open() {
        let (sessions, old_id) = queued_session(tokio::spawn(std::future::pending()));
        let owner = sessions.inner.entries.lock().unwrap()[&old_id]
            .owner
            .clone();
        let server = owner.server().id;
        let replacement_id = Uuid::new_v4();
        let replacement = tokio::spawn(std::future::pending());
        let active = replacement.abort_handle();
        insert(&sessions, replacement_id, server, 1, replacement);
        let cleanup_handle = sessions.clone();
        drop(sessions);
        cleanup_handle.begin_close_owner(&owner).wait().await;
        let entries = |id| {
            cleanup_handle
                .inner
                .entries
                .lock()
                .unwrap()
                .contains_key(&id)
        };
        assert!(!entries(old_id));
        assert!(entries(replacement_id));
        assert!(!active.is_finished());
        cleanup_handle.close_server(None).await;
        assert!(active.is_finished());
    }

    #[tokio::test]
    async fn captured_cleanup_does_not_close_a_replacement_terminal() {
        let old = tokio::spawn(std::future::pending());
        let (sessions, id) = queued_session(old);
        let server = sessions.inner.entries.lock().unwrap()[&id].server;
        let cleanup = sessions.begin_close_server(Some(server));
        assert!(!sessions.inner.entries.lock().unwrap().contains_key(&id));

        let replacement = tokio::spawn(std::future::pending());
        let active = replacement.abort_handle();
        insert(&sessions, id, server, 1, replacement);
        cleanup.wait().await;
        assert!(!active.is_finished());
        assert!(sessions.inner.entries.lock().unwrap().contains_key(&id));
        sessions.close_server(None).await;
        assert!(active.is_finished());
    }

    #[tokio::test]
    async fn lease_cancellation_is_synchronous_and_shutdown_joins_the_task() {
        let (ready, started) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(async move {
            ready.send(()).unwrap();
            std::future::pending::<()>().await;
        });
        let abort = task.abort_handle();
        started.await.unwrap();
        let (sessions, id) = queued_session(task);
        sessions.cancel(id);
        assert!(matches!(
            sessions.link_owner(id),
            Err(error) if error.contains("Terminal is closed")
        ));
        sessions.close_server(None).await;
        assert!(abort.is_finished());
        assert!(sessions.inner.cancelled.lock().unwrap().is_empty());
        sessions.cancel(id);
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
            proxy: None,
        }
    }
    #[tokio::test]
    async fn tabs_waiting_for_a_login_share_it_and_the_last_to_close_cancels_it() {
        use tokio::io::AsyncReadExt;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let server: Server = serde_json::from_value(serde_json::json!({
            "id":Uuid::new_v4(),"name":"Pending terminal","sshHost":"127.0.0.1",
            "sshUser":"fixture","sshPort":listener.local_addr().unwrap().port()
        }))
        .unwrap();
        let registry = crate::server_session::Registry::default();
        let owner = registry.session(&server, 0);
        let sessions = Sessions::default();
        let first = Uuid::new_v4();
        let _first = sessions
            .open_bound(first, server.clone(), 80, 24, owner.clone())
            .unwrap();
        let (mut socket, _) = tokio::time::timeout(Duration::from_secs(3), listener.accept())
            .await
            .unwrap()
            .unwrap();
        let second = Uuid::new_v4();
        let _second = sessions
            .open_bound(second, server, 80, 24, owner.clone())
            .unwrap();
        assert!(
            tokio::time::timeout(Duration::from_millis(200), listener.accept())
                .await
                .is_err(),
            "the second tab must wait for the login in progress"
        );
        sessions.close(first).await;
        // The client's banner arrives at once; only the end of the socket
        // tells that the login stopped.
        let mut received = Vec::new();
        assert!(
            tokio::time::timeout(
                Duration::from_millis(200),
                socket.read_to_end(&mut received)
            )
            .await
            .is_err(),
            "the login must go on while a tab still waits for it"
        );
        sessions.close(second).await;
        tokio::time::timeout(Duration::from_secs(3), socket.read_to_end(&mut received))
            .await
            .expect("the login must stop once no tab waits for it")
            .unwrap();
        registry.shutdown().await;
    }

    /// The next event; a closed output reads as an exit.
    async fn next(io: &mut Io) -> Event {
        io.output.recv().await.unwrap_or(Event::Exit(None))
    }
    async fn type_in(io: &Io, input: impl Into<Vec<u8>>) {
        io.input.send(Input::Data(input.into())).await.unwrap();
    }
    async fn until(io: &mut Io, needle: &str) -> String {
        tokio::time::timeout(Duration::from_secs(8), async {
            let mut text = String::new();
            loop {
                match next(io).await {
                    Event::Data(bytes) => {
                        text.push_str(&String::from_utf8_lossy(&bytes));
                        if text.contains(needle) {
                            return text;
                        }
                    }
                    Event::Error(error) => panic!("{error}"),
                    Event::Exit(code) => panic!("Unexpected exit {code:?}: {text}"),
                    _ => {}
                }
            }
        })
        .await
        .unwrap_or_else(|_| panic!("missing {needle:?}"))
    }
    async fn refused(io: &mut Io) {
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                match next(io).await {
                    Event::Error(error) => {
                        assert!(error.message.contains("refused"));
                        break;
                    }
                    Event::Reconnecting { .. } | Event::Ready => panic!("refused PTY retried"),
                    _ => {}
                }
            }
        })
        .await
        .unwrap();
    }
    #[tokio::test]
    #[ignore = "Run with cargo xtask test ssh; runs a real Bash PTY in an isolated HOME"]
    async fn interactive_pty_input_resize_interrupt_exit_and_close() {
        let sessions = Sessions::default();
        let server = fixture();
        let id = Uuid::new_v4();
        let registry = crate::server_session::Registry::default();
        let owner = registry.session(&server, 0);
        let mut io = sessions
            .open_bound(id, server.clone(), 80, 24, owner.clone())
            .unwrap();
        until(&mut io, "early shell output").await;
        until(&mut io, "fixture> ").await;
        type_in(&io, b"export NEWPORT_VALUE=persistent\r").await;
        until(&mut io, "fixture> ").await;
        type_in(&io, b"printf 'value=%s\\n' \"$NEWPORT_VALUE\"\r").await;
        until(&mut io, "value=persistent").await;
        type_in(&io, b"printf 'terminal=%s\\n' \"$NEWPORT_TERMINAL\"\r").await;
        until(&mut io, &format!("terminal={id}")).await;
        io.input.send(Input::Resize(100, 40)).await.unwrap();
        type_in(&io, b"stty size\r").await;
        until(&mut io, "40 100").await;
        type_in(&io, b"sleep 60\r").await;
        tokio::time::sleep(Duration::from_millis(150)).await;
        type_in(&io, [3]).await;
        until(&mut io, "^C").await;
        type_in(&io, b"exit 7\r").await;
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                match next(&mut io).await {
                    Event::Exit(code) => {
                        assert_eq!(code, Some(7));
                        break;
                    }
                    Event::Error(error) => panic!("{error}"),
                    _ => {}
                }
            }
        })
        .await
        .unwrap();
        sessions.close(id).await;
        assert!(sessions.link_owner(id).is_err());
        let rejected = Uuid::new_v4();
        let mut io = sessions
            .open_bound(rejected, server.clone(), 13, 24, owner.clone())
            .unwrap();
        refused(&mut io).await;
        sessions.close_server(Some(server.id)).await;
        assert!(sessions.inner.entries.lock().unwrap().is_empty());
        let cancelled = Uuid::new_v4();
        let _io = sessions
            .open_bound(cancelled, server, 80, 24, owner)
            .unwrap();
        sessions.close(cancelled).await;
        assert!(sessions.inner.entries.lock().unwrap().is_empty());
    }
    async fn fixture_connection(io: &mut Io) -> Uuid {
        type_in(
            io,
            b"printf '\\nconnection=%s\\n' \"$NEWPORT_FIXTURE_CONNECTION\"\r",
        )
        .await;
        let output = until(io, "fixture> ").await;
        output
            .lines()
            .find_map(|line| {
                line.trim()
                    .strip_prefix("connection=")
                    .and_then(|value| Uuid::parse_str(value).ok())
            })
            .unwrap_or_else(|| panic!("Missing fixture connection ID: {output}"))
    }

    fn open(sessions: &Sessions, server: &Server, owner: &Arc<ServerSession>) -> Io {
        sessions
            .open_bound(Uuid::new_v4(), server.clone(), 80, 24, owner.clone())
            .unwrap()
    }

    /// Waits for the tab's next shell after its connection was lost.
    async fn reconnects(io: &mut Io) {
        tokio::time::timeout(Duration::from_secs(8), async {
            loop {
                match next(io).await {
                    Event::Reconnecting { attempt, .. } => {
                        assert!(attempt >= 1);
                        break;
                    }
                    Event::Error(error) => panic!("{error}"),
                    Event::Exit(code) => panic!("Unexpected exit {code:?}"),
                    _ => {}
                }
            }
        })
        .await
        .unwrap();
        until(io, "fixture> ").await;
    }

    #[tokio::test]
    #[ignore = "Run with cargo xtask test ssh -- terminal::tests"]
    async fn two_tabs_on_one_server_share_one_login() {
        let sessions = Sessions::default();
        let server = fixture();
        let registry = crate::server_session::Registry::default();
        let owner = registry.session(&server, 0);
        let mut a = open(&sessions, &server, &owner);
        let mut b = open(&sessions, &server, &owner);
        until(&mut a, "fixture> ").await;
        until(&mut b, "fixture> ").await;
        let connection = fixture_connection(&mut a).await;
        assert_eq!(fixture_connection(&mut b).await, connection);
        assert_eq!(
            crate::remote::command::stdout(owner.link(), "fixture-connection-id", None)
                .await
                .unwrap(),
            connection.to_string()
        );
        assert_eq!(
            owner.link().status().borrow().reach,
            Reach::Ready { connections: 1 }
        );
        // A refused PTY ends only its own tab, and nothing reconnects.
        let refused_id = Uuid::new_v4();
        let mut io = sessions
            .open_bound(refused_id, server, 13, 24, owner.clone())
            .unwrap();
        refused(&mut io).await;
        sessions.close(refused_id).await;
        assert_eq!(fixture_connection(&mut a).await, connection);
        sessions.close_server(None).await;
        registry.shutdown().await;
    }

    #[tokio::test]
    #[ignore = "Run with cargo xtask test ssh -- terminal::tests"]
    async fn a_stopped_link_keeps_the_tab_waiting_and_no_sessions_ends_it() {
        // A refused login waits for something to retry the link, and the tab
        // waits with it; a server without sessions can't run a shell at all.
        for (user, error, waits) in [
            ("nobody", newport_ssh::Error::AuthRefused, true),
            (
                "fixture-no-sessions",
                newport_ssh::Error::SessionsDisabled,
                false,
            ),
        ] {
            let sessions = Sessions::default();
            let mut server = fixture();
            server.ssh_user = user.into();
            let registry = crate::server_session::Registry::default();
            let owner = registry.session(&server, 0);
            let mut io = open(&sessions, &server, &owner);
            match tokio::time::timeout(Duration::from_secs(8), next(&mut io))
                .await
                .unwrap()
            {
                Event::Reconnecting { message, .. } if waits => {
                    assert_eq!(message, error.to_string())
                }
                Event::Error(message) if !waits => assert_eq!(message.message, error.to_string()),
                event => panic!("Unexpected {event:?}"),
            }
            sessions.close_server(None).await;
            registry.shutdown().await;
        }
    }

    #[tokio::test]
    #[ignore = "Run with cargo xtask test ssh -- terminal::tests"]
    async fn a_tab_opened_on_a_link_that_stopped_trying_logs_in_again() {
        let directory = tempfile::tempdir().unwrap();
        let identity = directory.path().join("identity");
        let sessions = Sessions::default();
        let mut server = fixture();
        server.identity_file = Some(identity.to_string_lossy().into_owned());
        let registry = crate::server_session::Registry::default();
        let owner = registry.session(&server, 0);
        // A key file that isn't there stops the link until Retry.
        let mut stopped = open(&sessions, &server, &owner);
        match tokio::time::timeout(Duration::from_secs(8), next(&mut stopped))
            .await
            .unwrap()
        {
            Event::Error(message) => assert!(
                message.message.contains("Cannot read identity"),
                "{message}"
            ),
            event => panic!("Unexpected {event:?}"),
        }
        assert!(matches!(
            owner.link().status().borrow().reach,
            Reach::Failed(_)
        ));
        std::fs::copy(std::env::var("NEWPORT_TEST_IDENTITY").unwrap(), &identity).unwrap();
        let mut io = open(&sessions, &server, &owner);
        until(&mut io, "fixture> ").await;
        sessions.close_server(None).await;
        registry.shutdown().await;
    }

    #[tokio::test]
    #[ignore = "Run with cargo xtask test ssh -- terminal::tests"]
    async fn a_lost_connection_reconnects_every_tab_on_one_new_login() {
        let sessions = Sessions::default();
        let server = fixture();
        let registry = crate::server_session::Registry::default();
        let owner = registry.session(&server, 0);
        let mut a = open(&sessions, &server, &owner);
        let mut b = open(&sessions, &server, &owner);
        until(&mut a, "fixture> ").await;
        until(&mut b, "fixture> ").await;
        let lost = fixture_connection(&mut a).await;
        assert_eq!(fixture_connection(&mut b).await, lost);
        // The command runs on the connection it drops, so its own result is lost.
        let _ = crate::remote::command::run(owner.link(), &format!("drop-connection {lost}"), None)
            .await;
        reconnects(&mut a).await;
        reconnects(&mut b).await;
        let replacement = fixture_connection(&mut a).await;
        assert_ne!(replacement, lost);
        assert_eq!(fixture_connection(&mut b).await, replacement);
        assert_eq!(
            owner.link().status().borrow().reach,
            Reach::Ready { connections: 1 }
        );
        sessions.close_server(None).await;
        registry.shutdown().await;
    }

    #[tokio::test]
    #[ignore = "Run with cargo xtask test ssh -- terminal::tests"]
    async fn heavy_output_in_one_tab_does_not_delay_echo_in_another() {
        let sessions = Sessions::default();
        let server = fixture();
        let registry = crate::server_session::Registry::default();
        let owner = registry.session(&server, 0);
        let flooding = Uuid::new_v4();
        let mut flood = sessions
            .open_bound(flooding, server.clone(), 80, 24, owner.clone())
            .unwrap();
        let mut quiet = open(&sessions, &server, &owner);
        until(&mut flood, "fixture> ").await;
        until(&mut quiet, "fixture> ").await;
        let connection = fixture_connection(&mut flood).await;
        assert_eq!(fixture_connection(&mut quiet).await, connection);
        type_in(&flood, b"yes newport-flood\r").await;
        until(&mut flood, "newport-flood\r\nnewport-flood\r\n").await;
        // From here the flooding tab is never read: its shell fills its
        // channel's window and waits.
        tokio::time::sleep(Duration::from_secs(1)).await;
        let mut slowest = Duration::ZERO;
        for index in 0..20 {
            let started = std::time::Instant::now();
            type_in(&quiet, format!("echo-{index}-")).await;
            until(&mut quiet, &format!("echo-{index}-")).await;
            slowest = slowest.max(started.elapsed());
            type_in(&quiet, [3]).await;
            until(&mut quiet, "fixture> ").await;
        }
        assert!(
            slowest < Duration::from_secs(1),
            "keystroke echo took {slowest:?} beside a flooding tab"
        );
        sessions.close(flooding).await;
        assert_eq!(fixture_connection(&mut quiet).await, connection);
        sessions.close_server(None).await;
        registry.shutdown().await;
    }

    #[tokio::test]
    #[ignore = "Run with cargo xtask test ssh -- terminal::tests"]
    async fn terminals_need_no_needle_and_closing_one_ends_only_its_shell() {
        let sessions = Sessions::default();
        let mut server = fixture();
        server.agent_install_approved = false;
        server.setup_complete = false;
        let registry = crate::server_session::Registry::default();
        let owner = registry.session(&server, 0);
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        let mut io_a = sessions
            .open_bound(a, server.clone(), 80, 24, owner.clone())
            .unwrap();
        let mut io_b = sessions
            .open_bound(b, server.clone(), 80, 24, owner.clone())
            .unwrap();
        until(&mut io_a, "fixture> ").await;
        until(&mut io_b, "fixture> ").await;
        assert!(owner.agent_runtime.snapshot().hello.is_none());
        let pid_file = format!("terminal-{a}.pid");
        type_in(&io_a, format!("echo $$ > \"$HOME/{pid_file}\"\r")).await;
        until(&mut io_a, "fixture> ").await;
        sessions.close(a).await;
        // The surviving shell sees that the closed tab's shell was reaped,
        // while its own channel and an ordinary command still work.
        type_in(&io_b, format!(
            "for i in {{1..100}}; do kill -0 \"$(cat \"$HOME/{pid_file}\")\" 2>/dev/null || {{ printf 'channel-%s\\n' released; break; }}; sleep 0.03; done; rm -f \"$HOME/{pid_file}\"\r"
        )).await;
        until(&mut io_b, "channel-released").await;
        assert_eq!(
            crate::remote::command::stdout(owner.link(), "printf ok", None)
                .await
                .unwrap(),
            "ok"
        );
        // Changing the profile ends an active terminal without moving it to
        // the new session.
        registry.invalidate(server.id);
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if let Event::Error(error) = next(&mut io_b).await {
                    assert!(error.message.contains("Server session changed"));
                    break;
                }
            }
        })
        .await
        .unwrap();
        sessions.close(b).await;
        registry.shutdown().await;
    }

    #[tokio::test]
    #[ignore = "Run with cargo xtask test ssh -- terminal::tests"]
    async fn a_plain_tab_names_the_display_and_shows_it_only_when_used() {
        use display::FakeView;
        let sessions = Sessions::default();
        let server = fixture();
        let registry = crate::server_session::Registry::default();
        let owner = registry.session(&server, 0);
        let fake = FakeView::new();
        let id = Uuid::new_v4();
        let mut io = sessions
            .open_bound_with_display(
                id,
                server.clone(),
                (80, 24),
                owner.clone(),
                Some(display::Access::showing(fake.view())),
                Hold::No,
            )
            .unwrap();
        // Opening the tab alone, as Newport restoring it does, never takes
        // the display; the shell names it all the same.
        until(&mut io, "fixture> ").await;
        fake.stays((0, 0)).await;
        type_in(&io, b"printf 'named=%s\\n' \"$WAYLAND_DISPLAY\"\r").await;
        until(&mut io, "named=newport-display/wayland-0").await;
        fake.stays((0, 0)).await;
        // Someone using it shows the display here, starting it if needed.
        io.input.send(Input::Activate).await.unwrap();
        fake.until((1, 1)).await;
        // Using it while it shows here asks nothing more; the display takes
        // copies.
        io.input.send(Input::Activate).await.unwrap();
        fake.stays((1, 1)).await;
        assert_eq!(fake.stood.load(std::sync::atomic::Ordering::Acquire), 1);
        // Another computer takes it over: the tab hears so, and its next use
        // takes it back.
        fake.taken.notify_one();
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                match next(&mut io).await {
                    Event::DisplayLost => break,
                    Event::Error(error) => panic!("{error}"),
                    _ => {}
                }
            }
        })
        .await
        .expect("the tab hears the display was taken");
        fake.until((1, 0)).await;
        io.input.send(Input::Activate).await.unwrap();
        fake.until((2, 1)).await;
        // A reconnect alone never takes it.
        fake.taken.notify_one();
        fake.until((2, 0)).await;
        let lost = fixture_connection(&mut io).await;
        let _ = crate::remote::command::run(owner.link(), &format!("drop-connection {lost}"), None)
            .await;
        reconnects(&mut io).await;
        fake.stays((2, 0)).await;
        sessions.close(id).await;
        fake.until((2, 0)).await;

        // Where this computer may not use Remote apps, nothing is named.
        let id = Uuid::new_v4();
        let mut io = sessions
            .open_bound_with_display(id, server, (80, 24), owner, None, Hold::No)
            .unwrap();
        until(&mut io, "fixture> ").await;
        type_in(&io, b"printf 'named=%s\\n' \"${WAYLAND_DISPLAY-none}\"\r").await;
        until(&mut io, "named=none").await;
        sessions.close_server(None).await;
        registry.shutdown().await;
    }

    #[tokio::test]
    #[ignore = "Run with cargo xtask test ssh; runs a real Bash PTY in an isolated HOME"]
    async fn interrupted_terminal_reconnects_with_a_fresh_shell() {
        let sessions = Sessions::default();
        let server = fixture();
        let id = Uuid::new_v4();
        let registry = crate::server_session::Registry::default();
        let owner = registry.session(&server, 0);
        let mut io = sessions
            .open_bound(id, server.clone(), 80, 24, owner.clone())
            .unwrap();
        until(&mut io, "fixture> ").await;
        let lost = fixture_connection(&mut io).await;
        // The fixture drops every SSH connection, including this shell's.
        let _ = crate::remote::command::run(owner.link(), "drop", None).await;
        tokio::time::timeout(Duration::from_secs(8), async {
            loop {
                match next(&mut io).await {
                    Event::Reconnecting { attempt, .. } => {
                        assert!(attempt >= 1);
                        break;
                    }
                    Event::Error(error) => panic!("{error}"),
                    Event::Exit(code) => panic!("Unexpected exit {code:?}"),
                    _ => {}
                }
            }
        })
        .await
        .unwrap();
        // A fresh shell starts without user action.
        let replay_file = format!("terminal-replay-{id}");
        type_in(&io, format!("touch \"$HOME/{replay_file}\"\r")).await;
        io.input.send(Input::Resize(110, 42)).await.unwrap();
        until(&mut io, "fixture> ").await;
        assert_ne!(fixture_connection(&mut io).await, lost);
        type_in(
            &io,
            format!(
                "stty size; test ! -e \"$HOME/{replay_file}\" && printf 'replay-%s\\n' absent\r"
            ),
        )
        .await;
        let recovered = until(&mut io, "replay-absent").await;
        assert!(
            recovered.contains("42 110"),
            "resize lost during recovery: {recovered}"
        );
        type_in(&io, b"printf 'new-shell\\n'\r").await;
        until(&mut io, "new-shell").await;
        sessions.close(id).await;
        registry.shutdown().await;
    }
}
