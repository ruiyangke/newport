//! Persistent terminals on a real Linux server: Needle holds the shell, so
//! dropping the tab's connection only detaches it, and attaching again brings
//! back the same shell with its screen.
use super::{installed_server, owner};
use crate::terminal::{Event, Hold, Input, Io, Sessions};
use std::{sync::Arc, time::Duration};
use tokio::time::timeout;
use uuid::Uuid;

async fn held_server() -> crate::model::Server {
    let mut server =
        installed_server("NEWPORT_TEST_SSH_PORT", "Persistent terminals", "fixture").await;
    server.setup_complete = true;
    server
}

fn open(
    sessions: &Sessions,
    server: &crate::model::Server,
    owner: &Arc<crate::server_session::ServerSession>,
    id: Uuid,
    attach: bool,
) -> Io {
    sessions
        .open_bound_with_display(
            id,
            server.clone(),
            (100, 30),
            owner.clone(),
            None,
            if attach {
                Hold::Attach(None)
            } else {
                Hold::WhenAvailable(None)
            },
        )
        .unwrap()
}

async fn next(io: &mut Io) -> Event {
    timeout(Duration::from_secs(60), io.output.recv())
        .await
        .expect("a terminal event")
        .expect("the terminal's events continue")
}

/// Waits through the attach: persistent, the screen, ready. Returns what the
/// restored screen showed.
async fn attached(io: &mut Io) -> String {
    assert!(matches!(next(io).await, Event::Persistent), "a held shell");
    let mut screen = String::new();
    loop {
        match next(io).await {
            Event::Restore { state, .. } => screen.push_str(&String::from_utf8_lossy(&state)),
            Event::Ready => return screen,
            event => panic!("Unexpected {event:?} before ready"),
        }
    }
}

/// Output, and restored states, until `text` appears.
async fn until(io: &mut Io, text: &str) -> String {
    let mut seen = String::new();
    while !seen.contains(text) {
        match next(io).await {
            Event::Data(bytes) | Event::Restore { state: bytes, .. } => {
                seen.push_str(&String::from_utf8_lossy(&bytes))
            }
            Event::Active(_) => {}
            event => panic!("Unexpected {event:?} waiting for {text:?}: {seen}"),
        }
    }
    seen
}

/// Whether the holder next says this tab is the active one.
async fn active(io: &mut Io) -> bool {
    loop {
        match next(io).await {
            Event::Active(active) => return active,
            Event::Data(_) | Event::Restore { .. } => {}
            event => panic!("Unexpected {event:?} waiting to hear who is active"),
        }
    }
}

async fn type_in(io: &Io, text: &str) {
    io.input
        .send(Input::Data(text.as_bytes().to_vec()))
        .await
        .unwrap();
}

/// The shell's process id, as it reports it. The marker is split in the
/// command, so its echo can't match.
async fn shell_pid(io: &mut Io) -> String {
    type_in(io, "echo \"p\"\"id=$$=do\"\"ne\"\r").await;
    let text = until(io, "=done").await;
    let start = text.rfind("pid=").unwrap() + 4;
    let end = start + text[start..].find("=done").unwrap();
    text[start..end].to_string()
}

#[tokio::test]
#[ignore = "Requires cargo xtask test remote and real OpenSSH"]
async fn a_held_shell_outlives_its_tab_and_comes_back_with_its_screen() {
    let server = held_server().await;
    let owner = owner(&server);
    let sessions = Sessions::default();
    let id = Uuid::new_v4();

    let mut io = open(&sessions, &server, &owner, id, false);
    attached(&mut io).await;
    type_in(&io, "echo marker-$((40+2))\r").await;
    until(&mut io, "marker-42").await;
    let pid = shell_pid(&mut io).await;
    // Output while detached keeps reaching the holder.
    type_in(&io, "(sleep 2; echo while-$((6*7))-away) &\r").await;
    // Detach once the shell has the command: keystrokes still in flight on
    // a closing connection are lost, as on any terminal.
    until(&mut io, "-away) &").await;
    sessions.close(id).await;
    tokio::time::sleep(Duration::from_secs(4)).await;

    let mut io = open(&sessions, &server, &owner, id, true);
    let screen = attached(&mut io).await;
    assert!(
        screen.contains("marker-42"),
        "the screen comes back: {screen}"
    );
    let full = until(&mut io, "while-42-away").await;
    assert!(full.contains("marker-42"));
    assert_eq!(shell_pid(&mut io).await, pid, "the same shell");

    io.input.send(Input::End).await.unwrap();
    loop {
        match next(&mut io).await {
            Event::Exit(_) => break,
            Event::Data(_) | Event::Active(_) => {}
            event => panic!("Unexpected {event:?} after End"),
        }
    }
    let mut io = open(&sessions, &server, &owner, Uuid::new_v4(), true);
    assert!(
        matches!(next(&mut io).await, Event::Ended),
        "nothing to attach to"
    );
    sessions.close_server(None).await;
}

#[tokio::test]
#[ignore = "Requires cargo xtask test remote and real OpenSSH"]
async fn a_shell_that_exits_while_detached_reports_its_exit_on_attach() {
    let server = held_server().await;
    let owner = owner(&server);
    let sessions = Sessions::default();
    let id = Uuid::new_v4();
    let mut io = open(&sessions, &server, &owner, id, false);
    attached(&mut io).await;
    type_in(&io, "sleep 2; echo leav\"\"ing; exit 7\r").await;
    until(&mut io, "exit 7").await;
    sessions.close(id).await;
    tokio::time::sleep(Duration::from_secs(4)).await;

    let mut io = open(&sessions, &server, &owner, id, true);
    let screen = attached(&mut io).await;
    assert!(screen.contains("leaving"), "{screen}");
    loop {
        match next(&mut io).await {
            Event::Exit(code) => {
                assert_eq!(code, Some(7));
                break;
            }
            Event::Restore { .. } | Event::Data(_) | Event::Active(_) => {}
            event => panic!("Unexpected {event:?}"),
        }
    }
    sessions.close_server(None).await;
}

/// The server's held shells once `check` accepts them, as a desktop lists
/// them for its tabs.
async fn listed(
    owner: &Arc<crate::server_session::ServerSession>,
    check: impl Fn(&[newport_domain::terminal::HeldShell]) -> bool,
) -> Vec<newport_domain::terminal::HeldShell> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
    loop {
        let shells = crate::terminal::held_list::read(owner).shells;
        if let Some(shells) = shells.filter(|shells| check(shells)) {
            return shells;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the list never matched"
        );
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

#[tokio::test]
#[ignore = "Requires cargo xtask test remote and real OpenSSH"]
async fn two_desktops_share_a_held_shell_and_close_ends_it_for_both() {
    let server = held_server().await;
    let owner = owner(&server);
    // Another desktop has its own connection to the server.
    let elsewhere = crate::server_session::Registry::default();
    let other = elsewhere.session(&server, 0);
    let (first, second) = (Sessions::default(), Sessions::default());
    let id = Uuid::new_v4();
    let mut a = open(&first, &server, &owner, id, false);
    attached(&mut a).await;
    assert!(active(&mut a).await, "the desktop that opened it");

    // The other desktop finds the shell in its list and attaches to it.
    let shells = listed(&other, |shells| shells.iter().any(|s| s.terminal == id)).await;
    let shell = shells.iter().find(|s| s.terminal == id).unwrap();
    assert_eq!(
        shell.creator.name,
        *crate::terminal::holder_channel::COMPUTER
    );
    let mut b = open(&second, &server, &other, id, true);
    attached(&mut b).await;
    assert!(!active(&mut b).await, "attaching uses nothing");
    listed(&owner, |shells| {
        shells
            .iter()
            .any(|s| s.terminal == id && s.attached.len() == 2)
    })
    .await;

    // Using the terminal on the other desktop makes it the active one, and
    // the server lists it first by its routing id.
    b.input.send(Input::Activate).await.unwrap();
    assert!(active(&mut b).await);
    assert!(!active(&mut a).await);
    let routing = other.integration_session_id().to_string();
    listed(&owner, |shells| {
        shells.iter().any(|s| {
            s.terminal == id
                && s.attached
                    .first()
                    .is_some_and(|mac| mac.id == routing && mac.active)
        })
    })
    .await;

    // Either desktop's typing reaches the shell, and both see the output.
    type_in(&b, "echo both-$((1+1))\r").await;
    until(&mut b, "both-2").await;
    until(&mut a, "both-2").await;

    crate::terminal::holder_channel::rename(&other, id, "build")
        .await
        .unwrap();
    listed(&owner, |shells| {
        shells
            .iter()
            .any(|s| s.terminal == id && s.name.as_deref() == Some("build"))
    })
    .await;

    // Close on one desktop ends the shell on both.
    a.input.send(Input::End).await.unwrap();
    for io in [&mut a, &mut b] {
        loop {
            match next(io).await {
                Event::Exit(_) => break,
                Event::Data(_) | Event::Restore { .. } | Event::Active(_) => {}
                event => panic!("Unexpected {event:?} after End"),
            }
        }
    }
    listed(&other, |shells| shells.iter().all(|s| s.terminal != id)).await;
    first.close_server(None).await;
    second.close_server(None).await;
    elsewhere.shutdown().await;
}

#[tokio::test]
#[ignore = "Requires cargo xtask test remote and real OpenSSH"]
async fn a_server_without_needle_set_up_gets_an_ordinary_shell() {
    let mut server = held_server().await;
    server.setup_complete = false;
    let owner = owner(&server);
    let sessions = Sessions::default();
    let mut io = open(&sessions, &server, &owner, Uuid::new_v4(), false);
    assert!(matches!(next(&mut io).await, Event::Ready), "no held shell");
    sessions.close_server(None).await;
}

#[tokio::test]
#[ignore = "Requires cargo xtask test remote and real OpenSSH"]
async fn a_holder_that_cannot_start_still_leaves_a_shell() {
    let server = held_server().await;
    // Files where the holders' directories go: neither can be created.
    let blocked = r#"for root in "${XDG_RUNTIME_DIR:-/run/user/$(id -u)}/newport-ipc" "$HOME/.cache/newport/run"; do
        mkdir -p "$root" 2>/dev/null && chmod 700 "$root" && rm -rf "$root/terminals" && : > "$root/terminals"; done"#;
    let session = crate::remote::Shell::new(&server);
    session.execute(blocked, None).await.unwrap();
    let owner = owner(&server);
    let sessions = Sessions::default();
    let mut io = open(&sessions, &server, &owner, Uuid::new_v4(), false);
    let notice = next(&mut io).await;
    let first = next(&mut io).await;
    session
        .execute(
            r#"rm -f "${XDG_RUNTIME_DIR:-/run/user/$(id -u)}/newport-ipc/terminals" "$HOME/.cache/newport/run/terminals""#,
            None,
        )
        .await
        .unwrap();
    // The tab says why the shell won't keep running, with Needle's reason.
    assert!(
        matches!(&notice, Event::NotHeld(reason) if reason.contains("/terminals")),
        "a notice that the shell isn't held, not {notice:?}"
    );
    assert!(
        matches!(first, Event::Ready),
        "an ordinary shell, not {first:?}"
    );
    type_in(&io, "echo plain-$((2+2))\r").await;
    until(&mut io, "plain-4").await;
    sessions.close_server(None).await;
}

#[tokio::test]
#[ignore = "Requires cargo xtask test remote and real OpenSSH"]
async fn a_terminal_opened_after_setup_finished_is_held() {
    // The server's session began during setup, before Needle was approved
    // and setup finished, as when a server is added in the app.
    let mut server = installed_server("NEWPORT_TEST_SSH_PORT", "Set up later", "fixture").await;
    server.setup_complete = false;
    let session = owner(&server);
    server.setup_complete = true;
    let current = owner(&server);
    assert!(Arc::ptr_eq(&session, &current), "setup keeps the session");
    assert!(
        !session.server().setup_complete,
        "the session's own copy is older"
    );
    let sessions = Sessions::default();
    let stale = session.server().clone();
    let mut io = open(&sessions, &stale, &session, Uuid::new_v4(), false);
    attached(&mut io).await;
    io.input.send(Input::End).await.unwrap();
    sessions.close_server(None).await;
}
