//! Native input -> retained terminal -> real framed IPC, with no SSH or OS clipboard.
use super::{Session, TerminalView, selection_tests};
use gpui_kit::component::WindowExt;
use gpui_kit::{
    self as gpui, AppContext, ClipboardItem, Entity, EntityInputHandler, TestAppContext,
    VisualTestContext,
};
use newport_client::backend::Backend;
use newport_platform::ipc::protocol::terminal::{self as wire, ToApp, ToBackend};
use newport_platform::ipc::protocol::{Request, Response, TERMINAL_OPEN_LEASE, VERSION};
use newport_platform::ipc::{Listener, Stream};
use serde_json::{Value, json};
use std::{
    collections::VecDeque,
    io::{Read, Write},
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

pub(crate) struct Peer {
    path: PathBuf,
    events: Arc<Mutex<VecDeque<Value>>>,
    writes: Arc<Mutex<Vec<Vec<u8>>>>,
    stopped: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
}

impl Peer {
    pub(crate) fn new() -> Self {
        let path = std::env::temp_dir().join(format!("np-terminal-{}.sock", uuid::Uuid::new_v4()));
        let listener = Listener::bind(&path).unwrap();
        listener.set_nonblocking(true).unwrap();
        let events = Arc::new(Mutex::new(VecDeque::from([json!({"type":"ready"})])));
        let writes = Arc::new(Mutex::new(Vec::new()));
        let stopped = Arc::new(AtomicBool::new(false));
        let (output, input, stop) = (events.clone(), writes.clone(), stopped.clone());
        let worker = thread::spawn(move || {
            let mut lease: Option<Stream> = None;
            let mut reader = None;
            while !stop.load(Ordering::Acquire) {
                if let Some(stream) = &mut lease {
                    while let Some(event) = output.lock().unwrap().pop_front() {
                        // A view can close while its final event is in flight.
                        let _ = stream.write_all(&frame(event).encode());
                    }
                }
                let (mut stream, _) = match listener.accept() {
                    Ok(pair) => pair,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(1));
                        continue;
                    }
                    Err(error) => panic!("{error}"),
                };
                stream.set_nonblocking(false).unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(2)))
                    .unwrap();
                stream
                    .set_write_timeout(Some(Duration::from_secs(2)))
                    .unwrap();
                let request = request(&mut stream);
                assert_eq!(request.version, VERSION);
                assert_eq!(request.method, TERMINAL_OPEN_LEASE);
                assert!(lease.is_none(), "one owned session per fixture");
                reply(&mut stream, Value::Null);
                stream.set_read_timeout(None).unwrap();
                let mut incoming = stream.try_clone().unwrap();
                let input = input.clone();
                reader = Some(thread::spawn(move || {
                    while let Ok(Some((kind, payload))) = wire::read(&mut incoming) {
                        if let Ok(ToBackend::Input { data, .. }) = ToBackend::decode(kind, payload)
                        {
                            input.lock().unwrap().push(data);
                        }
                    }
                }));
                lease = Some(stream);
            }
            if let Some(stream) = lease {
                let _ = stream.shutdown(std::net::Shutdown::Both);
            }
            if let Some(reader) = reader {
                reader.join().unwrap();
            }
        });
        Self {
            path,
            events,
            writes,
            stopped,
            worker: Some(worker),
        }
    }

    fn event(&self, event: Value) {
        self.events.lock().unwrap().push_back(event);
    }

    pub(crate) fn written(&self) -> Vec<u8> {
        self.writes
            .lock()
            .unwrap()
            .iter()
            .flatten()
            .copied()
            .collect()
    }
}

impl Drop for Peer {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::Release);
        self.worker.take().unwrap().join().unwrap();
        #[cfg(unix)]
        let _ = std::fs::remove_file(&self.path);
    }
}

fn request(stream: &mut Stream) -> Request {
    let mut header = [0; 4];
    stream.read_exact(&mut header).unwrap();
    let length = u32::from_be_bytes(header) as usize;
    assert!(length <= newport_platform::ipc::protocol::MAX_REQUEST);
    let mut bytes = vec![0; length];
    stream.read_exact(&mut bytes).unwrap();
    rmp_serde::from_slice(&bytes).unwrap()
}

/// A fixture event, written as the terminal stream carries it.
fn frame(event: Value) -> ToApp {
    if event["type"] == "data" {
        ToApp::Output(serde_json::from_value(event["data"].clone()).unwrap())
    } else {
        ToApp::Event(serde_json::from_value::<wire::Event>(event).unwrap())
    }
}

fn reply(stream: &mut Stream, result: Value) {
    let bytes = rmp_serde::to_vec_named(&Response {
        version: VERSION,
        result,
        error: None,
    })
    .unwrap();
    // A view can close before its open completes.
    let _ = stream
        .write_all(&(bytes.len() as u32).to_be_bytes())
        .and_then(|_| stream.write_all(&bytes));
}

/// A terminal for `server` attached to `peer`. It never opens a session of
/// its own, so drawing it reaches no real backend.
pub(crate) fn attached(
    peer: &Peer,
    server: &str,
    cx: &mut gpui::Context<TerminalView>,
) -> TerminalView {
    let mut view = TerminalView::new(server.into(), cx);
    view.close();
    view.session = Some(
        Session::open_with_backend(
            Backend::at_path(peer.path.clone()),
            &uuid::Uuid::new_v4().to_string(),
            80,
            24,
        )
        .unwrap(),
    );
    view
}

pub(crate) fn wait(
    view: &Entity<TerminalView>,
    cx: &mut VisualTestContext,
    done: impl Fn(&TerminalView) -> bool,
) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if view.update(cx, |view, cx| {
            view.tick(cx);
            done(view)
        }) {
            return;
        }
        assert!(Instant::now() < deadline, "terminal IPC fixture timed out");
        thread::sleep(Duration::from_millis(2));
    }
}

fn connect<'a>(
    peer: &Peer,
    cx: &'a mut TestAppContext,
) -> (Entity<TerminalView>, &'a mut VisualTestContext) {
    let (view, cx) = selection_tests::fixture(cx, "");
    view.update(cx, |view, _| {
        view.session = Some(
            Session::open_with_backend(
                Backend::at_path(peer.path.clone()),
                &uuid::Uuid::new_v4().to_string(),
                view.dimensions.0,
                view.dimensions.1,
            )
            .unwrap(),
        );
    });
    wait(&view, cx, |view| view.ready());
    (view, cx)
}

// Keep this entity unmounted: drawing a TerminalView makes it visible again.
fn hidden(peer: &Peer, cx: &mut TestAppContext) -> Entity<TerminalView> {
    cx.update(|cx| {
        gpui::component::init(cx);
        crate::theme::install(false, cx);
        cx.new(|cx| {
            let mut view = TerminalView::new(uuid::Uuid::new_v4().to_string(), cx);
            view.session = Some(
                Session::open_with_backend(
                    Backend::at_path(peer.path.clone()),
                    &view.server,
                    80,
                    24,
                )
                .unwrap(),
            );
            view
        })
    })
}

fn poll_hidden(
    view: &Entity<TerminalView>,
    cx: &mut TestAppContext,
    done: impl Fn(&mut TerminalView) -> bool,
) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let finished = view.update(cx, |view, cx| {
            view.tick(cx);
            done(view)
        });
        cx.run_until_parked();
        if finished {
            return;
        }
        assert!(Instant::now() < deadline, "hidden terminal IPC timed out");
        thread::sleep(Duration::from_millis(2));
    }
}

#[gpui::test]
fn terminal_events_cannot_overwrite_global_connectivity(cx: &mut TestAppContext) {
    use crate::connectivity::{self, Phase, ServerState, Snapshot};
    let peer = Peer::new();
    let (view, cx) = connect(&peer, cx);
    let id = uuid::Uuid::new_v4();
    let state = Snapshot {
        instance_id: uuid::Uuid::new_v4(),
        revision: 1,
        servers: [(
            id,
            ServerState {
                phase: Phase::AwaitingApproval,
                ..Default::default()
            },
        )]
        .into(),
    };
    cx.update(|_, cx| connectivity::publish(state, cx));
    let phase = |cx: &gpui_kit::App| connectivity::server(&id.to_string(), cx).unwrap().phase;
    assert!(view.read_with(cx, |view, _| view.ready()));
    assert_eq!(
        view.read_with(cx, |_, cx| phase(cx)),
        Phase::AwaitingApproval
    );
    peer.event(json!({"type":"reconnecting", "data":{"attempt":1,"message":"fixture reconnect"}}));
    wait(&view, cx, |view| !view.ready() && view.observed_epoch == 1);
    peer.event(json!({"type":"ready"}));
    wait(&view, cx, |view| view.ready());
    assert_eq!(
        view.read_with(cx, |_, cx| phase(cx)),
        Phase::AwaitingApproval
    );
    view.update(cx, |view, _| view.close());
}

#[gpui::test]
fn hidden_output_is_retained_without_notifications_and_visible_output_notifies(
    cx: &mut TestAppContext,
) {
    use std::{cell::Cell, rc::Rc};
    let peer = Peer::new();
    let view = hidden(&peer, cx);
    poll_hidden(&view, cx, |view| view.ready());
    let notifications = Rc::new(Cell::new(0));
    let observed = notifications.clone();
    let _subscription =
        cx.update(|cx| cx.observe(&view, move |_, _| observed.set(observed.get() + 1)));
    for line in ["first", "second", "third"] {
        peer.event(json!({"type":"data", "data":format!("{line}\r\n").into_bytes()}));
        poll_hidden(&view, cx, |view| {
            view.engine
                .frame()
                .unwrap()
                .visible_lines()
                .iter()
                .any(|text| text.trim() == line)
        });
    }
    assert_eq!(
        notifications.get(),
        0,
        "hidden output must not invalidate UI"
    );

    // Device replies still reach the remote program while its tab is hidden.
    peer.event(json!({"type":"data", "data":b"\x1b[6n".to_vec()}));
    poll_hidden(&view, cx, |_| !peer.written().is_empty());
    assert_eq!(peer.written(), b"\x1b[4;1R");
    assert_eq!(notifications.get(), 0);

    view.update(cx, |view, _| view.visible = true);
    peer.event(json!({"type":"data", "data":b"visible".to_vec()}));
    poll_hidden(&view, cx, |view| {
        view.engine
            .frame()
            .unwrap()
            .visible_lines()
            .iter()
            .any(|text| text.trim() == "visible")
    });
    assert_eq!(notifications.get(), 1);
    let lines = view.update(cx, |view, _| view.engine.frame().unwrap().visible_lines());
    assert_eq!(
        lines[..4]
            .iter()
            .map(|line| line.trim())
            .collect::<Vec<_>>(),
        ["first", "second", "third", "visible"]
    );
}

#[gpui::test]
fn hidden_terminal_still_publishes_titles_reconnection_and_exit(cx: &mut TestAppContext) {
    use newport_gpui::terminal_status::Phase;
    use std::{cell::Cell, rc::Rc};
    let peer = Peer::new();
    let view = hidden(&peer, cx);
    poll_hidden(&view, cx, |view| view.ready());
    let notifications = Rc::new(Cell::new(0));
    let observed = notifications.clone();
    let _subscription =
        cx.update(|cx| cx.observe(&view, move |_, _| observed.set(observed.get() + 1)));
    peer.event(json!({"type":"data", "data":b"\x1b]2;Background build\x07".to_vec()}));
    poll_hidden(&view, cx, |view| view.pending_title.is_some());
    assert_eq!(notifications.get(), 0, "title debounce should stay quiet");
    view.update(cx, |view, _| {
        view.pending_title.as_mut().unwrap().1 = Instant::now() - Duration::from_millis(151);
    });
    poll_hidden(&view, cx, |view| view.title == "Background build");
    assert_eq!(notifications.get(), 1);
    peer.event(json!({"type":"reconnecting", "data":{"attempt":1,"message":"fixture"}}));
    poll_hidden(&view, cx, |view| {
        matches!(view.status.phase, Phase::Reconnecting { attempt: 1 })
    });
    assert_eq!(notifications.get(), 2);
    assert_eq!(view.read_with(cx, |view, _| view.title.clone()), "Terminal");
    peer.event(json!({"type":"ready"}));
    poll_hidden(&view, cx, |view| view.ready());
    assert_eq!(notifications.get(), 3);
    peer.event(json!({"type":"exit", "data":17}));
    poll_hidden(&view, cx, |view| {
        matches!(view.status.phase, Phase::Exited(Some(17)))
    });
    assert_eq!(notifications.get(), 4);
    assert!(!view.read_with(cx, |view, _| view.active()));
}

#[gpui::test]
fn hidden_terminal_still_notifies_when_input_warning_clears_or_transport_fails(
    cx: &mut TestAppContext,
) {
    use std::{cell::Cell, rc::Rc};
    let peer = Peer::new();
    let view = hidden(&peer, cx);
    poll_hidden(&view, cx, |view| view.ready());
    view.update(cx, |view, _| {
        view.input_error = Some("previous input warning".into())
    });
    let notifications = Rc::new(Cell::new(0));
    let observed = notifications.clone();
    let _subscription =
        cx.update(|cx| cx.observe(&view, move |_, _| observed.set(observed.get() + 1)));
    peer.event(json!({"type":"data", "data":b"\x1b[6n".to_vec()}));
    poll_hidden(&view, cx, |view| {
        view.input_error.is_none() && !peer.written().is_empty()
    });
    assert_eq!(notifications.get(), 1);
    peer.event(json!({"type":"error", "data":"SSH transport lost"}));
    poll_hidden(&view, cx, |view| {
        view.status.error() == Some("SSH transport lost")
    });
    assert_eq!(notifications.get(), 2);
    assert!(!view.read_with(cx, |view, _| view.active()));
}

#[gpui::test]
fn workspace_bindings_preserve_terminal_flow_control_and_history(cx: &mut TestAppContext) {
    cx.update(|cx| {
        crate::navigation::install_desktop_commands(cx);
        cx.on_action(|_: &crate::Quit, _| panic!("Ctrl+Q must reach the terminal"));
        cx.on_action(|_: &crate::Refresh, _| panic!("Terminal input must not refresh the app"));
        cx.on_action(|_: &crate::FindInView, _| panic!("Ctrl+F must reach the terminal"));
    });
    let peer = Peer::new();
    let (view, cx) = connect(&peer, cx);
    cx.simulate_keystrokes("ctrl-q ctrl-r ctrl-f ctrl-n ctrl-p ctrl-s ctrl-w ctrl-x ctrl-s ctrl-x ctrl-w ctrl-2 ctrl-3 ctrl-6 ctrl-7 ctrl-8 ctrl-tab ctrl-shift-tab f5 f11 ctrl-insert shift-insert ctrl-alt-f ctrl-alt-b");
    // Every chord reaches the terminal instead of an app action.
    let expected = b"\x11\x12\x06\x0e\x10\x13\x17\x18\x13\x18\x17\x00\x1b\x1e\x1f\x7f\x1b[27;5;9~\x1b[27;6;9~\x1b[15~\x1b[23~\x1b[2;5~\x1b[2;2~\x1b\x06\x1b\x02";
    wait(&view, cx, |_| peer.written().len() >= expected.len());
    assert_eq!(peer.written(), expected);
}

#[gpui::test]
fn tab_number_shortcuts_select_tabs_from_a_focused_terminal(cx: &mut TestAppContext) {
    let selected = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    cx.update(|cx| {
        crate::navigation::install_desktop_commands(cx);
        let selected = selected.clone();
        cx.on_action(move |action: &crate::navigation::SelectTab, _| {
            selected.borrow_mut().push(action.number)
        });
    });
    let peer = Peer::new();
    let (view, cx) = connect(&peer, cx);
    let tabs = if cfg!(target_os = "macos") {
        "cmd"
    } else {
        "ctrl-alt"
    };
    cx.simulate_keystrokes(&format!("{tabs}-2 {tabs}-9 ctrl-a"));
    wait(&view, cx, |_| !peer.written().is_empty());
    assert_eq!(*selected.borrow(), [2, 9]);
    assert_eq!(peer.written(), b"\x01", "only Ctrl+A reached the shell");
}

#[cfg(target_os = "macos")]
#[gpui::test]
fn cmd_t_opens_a_terminal_from_a_focused_terminal(cx: &mut TestAppContext) {
    let opened = std::rc::Rc::new(std::cell::Cell::new(0));
    cx.update(|cx| {
        crate::navigation::install_desktop_commands(cx);
        let opened = opened.clone();
        cx.on_action(move |_: &crate::navigation::NewTerminal, _| opened.set(opened.get() + 1));
    });
    let peer = Peer::new();
    let (view, cx) = connect(&peer, cx);
    cx.simulate_keystrokes("cmd-t ctrl-a");
    wait(&view, cx, |_| !peer.written().is_empty());
    assert_eq!(opened.get(), 1);
    assert_eq!(peer.written(), b"\x01", "only Ctrl+A reached the shell");
}

#[gpui::test]
fn modified_page_keys_reach_fullscreen_apps_over_ipc(cx: &mut TestAppContext) {
    let peer = Peer::new();
    let (view, cx) = connect(&peer, cx);
    let mut expected = Vec::new();
    for alternate in [false, true] {
        view.update(cx, |view, _| {
            view.engine
                .advance(if alternate {
                    b"\x1b[?1049h"
                } else {
                    b"\x1b[?1049l"
                })
                .unwrap();
        });
        cx.simulate_keystrokes("shift-pageup shift-pagedown ctrl-shift-pageup alt-shift-pagedown");
        expected.extend_from_slice(b"\x1b[5;2~\x1b[6;2~\x1b[5;6~\x1b[6;4~");
        wait(&view, cx, |_| peer.written().len() >= expected.len());
        assert_eq!(peer.written(), expected);
    }
}

#[gpui::test]
fn super_keys_reach_kitty_apps_but_do_not_become_legacy_text(cx: &mut TestAppContext) {
    let peer = Peer::new();
    let (view, cx) = connect(&peer, cx);
    cx.simulate_keystrokes("cmd-n ctrl-a");
    wait(&view, cx, |_| !peer.written().is_empty());
    assert_eq!(peer.written(), b"\x01");
    let mut expected = b"\x01".to_vec();
    for (mode, bytes) in [
        (b"\x1b[>1u", b"\x1b[110;9u\x1b[105;11u".as_slice()),
        (
            b"\x1b[>3u",
            b"\x1b[110;9u\x1b[110;9:3u\x1b[105;11u\x1b[105;11:3u",
        ),
    ] {
        view.update(cx, |view, _| view.engine.advance(mode).unwrap());
        for key in ["cmd-n", "cmd-alt-i"] {
            cx.simulate_event(gpui::KeyDownEvent {
                keystroke: gpui::Keystroke::parse(key).unwrap(),
                physical_key: None,
                key_metadata: None,
                is_held: false,
                prefer_character_input: false,
            });
            cx.simulate_event(gpui::KeyUpEvent {
                keystroke: gpui::Keystroke::parse(key).unwrap(),
                physical_key: None,
                key_metadata: None,
            });
        }
        expected.extend_from_slice(bytes);
        wait(&view, cx, |_| peer.written().len() >= expected.len());
        assert_eq!(peer.written(), expected);
    }
}

#[gpui::test]
fn altgr_text_preference_bypasses_shortcuts_and_terminal_encoding(cx: &mut TestAppContext) {
    let peer = Peer::new();
    let (view, cx) = connect(&peer, cx);
    view.update(cx, |view, _| view.engine.advance(b"\x1b[>31u").unwrap());
    for is_held in [false, true] {
        cx.simulate_event(gpui::KeyDownEvent {
            keystroke: gpui::Keystroke {
                key: "e".into(),
                key_char: Some("€".into()),
                modifiers: gpui::Modifiers {
                    control: true,
                    alt: true,
                    ..Default::default()
                },
            },
            physical_key: None,
            key_metadata: None,
            is_held,
            prefer_character_input: true,
        });
        cx.simulate_input("€");
    }
    // Native commits still follow the application's negotiated text protocol.
    let expected = b"\x1b[8364;;8364u\x1b[8364;;8364u";
    wait(&view, cx, |_| peer.written().len() >= expected.len());
    assert_eq!(peer.written(), expected);
}

#[gpui::test]
fn tab_reaches_the_shell_instead_of_moving_focus(cx: &mut TestAppContext) {
    cx.update(crate::navigation::install_workspace_shortcuts);
    let peer = Peer::new();
    let (view, cx) = connect(&peer, cx);
    cx.simulate_keystrokes("tab shift-tab");
    let expected = b"\t\x1b[Z";
    wait(&view, cx, |_| peer.written().len() >= expected.len());
    assert_eq!(peer.written(), expected);
    cx.update(|window, cx| assert!(view.read(cx).focus.is_focused(window)));
}

#[cfg(target_os = "macos")]
#[gpui::test]
fn option_follows_the_option_as_meta_setting(cx: &mut TestAppContext) {
    let peer = Peer::new();
    let (view, cx) = connect(&peer, cx);
    // Without a saved choice, Option is Meta.
    cx.simulate_keystrokes("alt-f alt-b");
    let mut expected = b"\x1bf\x1bb".to_vec();
    wait(&view, cx, |_| peer.written().len() >= expected.len());
    assert_eq!(peer.written(), expected);
    cx.update(|_, cx| cx.set_global(super::OptionAsMeta(false)));
    cx.simulate_keystrokes("alt-f ctrl-a");
    expected.push(0x01);
    wait(&view, cx, |_| peer.written().len() >= expected.len());
    assert_eq!(
        peer.written(),
        expected,
        "Option composes text when it is not Meta"
    );
}

#[gpui::test]
fn paste_shortcuts_deliver_exact_bytes_over_owned_terminal_ipc(cx: &mut TestAppContext) {
    let peer = Peer::new();
    let (view, cx) = connect(&peer, cx);
    let shortcuts: &[&str] = if cfg!(target_os = "macos") {
        &["cmd-v"]
    } else {
        &["ctrl-shift-v"]
    };
    let mut expected = Vec::new();
    for bracketed in [false, true] {
        view.update(cx, |view, _| {
            view.engine
                .advance(if bracketed {
                    b"\x1b[?2004h"
                } else {
                    b"\x1b[?2004l"
                })
                .unwrap();
        });
        for shortcut in shortcuts {
            cx.update(|_, cx| {
                cx.write_to_clipboard(ClipboardItem::new_string(
                    "one\r\n服务🙂\ntwo\rthree".into(),
                ))
            });
            cx.simulate_keystrokes(shortcut);
            if bracketed {
                expected.extend_from_slice(b"\x1b[200~");
            }
            expected.extend_from_slice(if bracketed {
                "one\r\n服务🙂\ntwo\rthree".as_bytes()
            } else {
                "one\r服务🙂\rtwo\rthree".as_bytes()
            });
            if bracketed {
                expected.extend_from_slice(b"\x1b[201~");
            }
            wait(&view, cx, |_| peer.written().len() >= expected.len());
            assert_eq!(peer.written(), expected);
        }
    }
    cx.simulate_keystrokes("ctrl-c ctrl-v");
    expected.extend_from_slice(&[3, 22]);
    wait(&view, cx, |_| peer.written().len() >= expected.len());
    assert_eq!(
        peer.written(),
        expected,
        "shell control keys are not clipboard gestures"
    );
    view.update(cx, |view, _| view.close());
}

#[gpui::test]
fn preedit_and_disconnected_paste_do_not_leak_into_a_replacement_shell(cx: &mut TestAppContext) {
    let peer = Peer::new();
    let (view, cx) = connect(&peer, cx);
    cx.update(|window, cx| {
        view.update(cx, |view, cx| {
            view.replace_and_mark_text_in_range(None, "服务🙂", Some(2..4), window, cx);
            assert_eq!(view.marked_text_range(window, cx), Some(0..4));
            assert_eq!(
                view.selected_text_range(false, window, cx).unwrap().range,
                2..4
            );
        })
    });
    assert!(peer.written().is_empty(), "IME preedit must remain local");
    peer.event(json!({"type":"reconnecting", "data":{"attempt":1,"message":"fixture reconnect"}}));
    wait(&view, cx, |view| !view.ready() && view.observed_epoch == 1);
    assert!(view.read_with(cx, |view, _| view.composition.is_empty()));
    cx.update(|_, cx| cx.write_to_clipboard(ClipboardItem::new_string("must not replay".into())));
    cx.simulate_keystrokes(if cfg!(target_os = "macos") {
        "cmd-v"
    } else {
        "ctrl-shift-v"
    });
    peer.event(json!({"type":"ready"}));
    wait(&view, cx, |view| view.ready());
    cx.update(|window, cx| {
        view.update(cx, |view, cx| {
            view.replace_and_mark_text_in_range(None, "にほん", Some(3..3), window, cx);
            view.replace_and_mark_text_in_range(None, "日本", Some(2..2), window, cx);
            view.replace_text_in_range(None, "日本", window, cx);
            assert!(view.marked_text_range(window, cx).is_none());
        })
    });
    wait(&view, cx, |_| !peer.written().is_empty());
    assert_eq!(
        peer.written(),
        "日本".as_bytes(),
        "only committed text reaches the replacement session"
    );
    view.update(cx, |view, _| view.close());
}

#[gpui::test]
fn text_input_callbacks_obey_focus_visibility_and_dialog_ownership(cx: &mut TestAppContext) {
    let peer = Peer::new();
    let (view, cx) = connect(&peer, cx);
    let other_focus = cx.update(|_, cx| cx.focus_handle());
    for owner in ["another control", "another tab", "modal"] {
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                view.replace_and_mark_text_in_range(None, "unfinished", Some(10..10), window, cx);
                assert!(!view.composition.is_empty());
            });
            match owner {
                "another control" => other_focus.focus(window, cx),
                "another tab" => view.update(cx, |view, cx| view.deactivate(window, cx)),
                "modal" => window.open_dialog(cx, |dialog, _, _| dialog.title("Local dialog")),
                _ => unreachable!(),
            }
            // Deliver the late callback before the next frame can register an
            // input handler. This single-view fixture renders the terminal
            // again on that frame; a real workspace renders the new tab.
            view.update(cx, |view, cx| {
                assert!(!view.accepts_text_input(window, cx));
                view.replace_and_mark_text_in_range(None, "late preedit", None, window, cx);
                view.replace_text_in_range(None, "late commit", window, cx);
            })
        });
        cx.update(|window, cx| {
            if window.has_active_dialog(cx) {
                window.close_dialog(cx);
            }
            view.read(cx).focus.clone().focus(window, cx);
            window.draw(cx).clear(cx);
        });
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                view.replace_text_in_range(None, "sentinel", window, cx);
            })
        });
        let expected = "sentinel".repeat(match owner {
            "another control" => 1,
            "another tab" => 2,
            _ => 3,
        });
        wait(&view, cx, |_| peer.written().len() >= expected.len());
        assert_eq!(
            peer.written(),
            expected.as_bytes(),
            "late input from {owner} reached the terminal"
        );
    }
    view.update(cx, |view, _| view.close());
}

#[gpui::test]
fn changing_terminal_tabs_cancels_preedit_without_closing_the_session(cx: &mut TestAppContext) {
    let peer = Peer::new();
    let (view, cx) = connect(&peer, cx);
    let session = view.read_with(cx, |view, _| view.session.as_ref().unwrap().id());
    cx.update(|window, cx| {
        view.update(cx, |view, cx| {
            view.replace_and_mark_text_in_range(None, "にほん", Some(3..3), window, cx);
            assert!(!view.composition.is_empty());
            view.deactivate(window, cx);
            assert!(
                view.composition.is_empty(),
                "hidden preedit must not block keys when the tab returns"
            );
            assert_eq!(view.composition_selection, 0..0);
            assert_eq!(view.session.as_ref().unwrap().id(), session);
            assert!(view.ready());
        })
    });
    assert!(peer.written().is_empty());
    view.update(cx, |view, _| view.close());
}

#[gpui::test]
fn kitty_key_events_deliver_one_press_repeat_and_release_over_ipc(cx: &mut TestAppContext) {
    let peer = Peer::new();
    let (view, cx) = connect(&peer, cx);
    view.update(cx, |view, _| {
        view.engine.advance(b"\x1b[>31u").unwrap();
    });
    let mut keystroke = gpui::Keystroke::parse("a").unwrap();
    keystroke.key_char = Some("a".into());
    for is_held in [false, true] {
        cx.simulate_event(gpui::KeyDownEvent {
            physical_key: None,
            key_metadata: None,
            keystroke: keystroke.clone(),
            is_held,
            prefer_character_input: false,
        });
    }
    cx.simulate_event(gpui::KeyUpEvent {
        physical_key: None,
        key_metadata: None,
        keystroke,
    });
    let expected = b"\x1b[97;;97u\x1b[97;1:2;97u\x1b[97;1:3u";
    wait(&view, cx, |_| peer.written().len() >= expected.len());
    assert_eq!(peer.written(), expected);
    view.update(cx, |view, _| view.close());
}

#[gpui::test]
fn losing_focus_releases_forwarded_kitty_keys_once(cx: &mut TestAppContext) {
    let peer = Peer::new();
    let (view, cx) = connect(&peer, cx);
    view.update(cx, |view, _| {
        view.engine.advance(b"\x1b[>3u").unwrap();
    });
    let keystroke = gpui::Keystroke::parse("up").unwrap();
    cx.simulate_event(gpui::KeyDownEvent {
        physical_key: None,
        key_metadata: None,
        keystroke: keystroke.clone(),
        is_held: false,
        prefer_character_input: false,
    });
    cx.update(|window, cx| {
        view.update(cx, |view, cx| view.deactivate(window, cx));
    });
    // A late key-up after switching tabs cannot duplicate the synthesized
    // release or deliver input to a different session.
    cx.simulate_event(gpui::KeyUpEvent {
        physical_key: None,
        key_metadata: None,
        keystroke,
    });
    let expected = b"\x1b[A\x1b[1;1:3A";
    wait(&view, cx, |_| peer.written().len() >= expected.len());
    assert_eq!(peer.written(), expected);
    view.update(cx, |view, _| view.close());
}

#[gpui::test]
fn focus_loss_releases_keys_with_current_modifiers_over_ipc(cx: &mut TestAppContext) {
    for deactivate in [false, true] {
        let peer = Peer::new();
        let (view, cx) = connect(&peer, cx);
        view.update(cx, |view, _| {
            view.engine.advance(b"\x1b[>3u").unwrap();
        });
        cx.update(|window, _| window.activate_window());
        cx.run_until_parked();
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                view.sync_focus(window, cx);
                assert!(view.focused, "test terminal starts focused");
            });
        });
        let keystroke = gpui::Keystroke::parse("up").unwrap();
        cx.simulate_event(gpui::KeyDownEvent {
            physical_key: None,
            key_metadata: None,
            keystroke: keystroke.clone(),
            is_held: false,
            prefer_character_input: false,
        });
        let other_focus = cx.update(|_, cx| cx.focus_handle());
        // Modifiers can change after the forwarded press. Both changing tabs
        // and moving focus must use that current state for the release.
        cx.simulate_event(gpui::ModifiersChangedEvent {
            modifiers: gpui::Modifiers {
                shift: true,
                alt: true,
                control: true,
                ..Default::default()
            },
            ..Default::default()
        });
        cx.update(|window, cx| {
            if !deactivate {
                other_focus.focus(window, cx);
            }
            view.update(cx, |view, cx| {
                if deactivate {
                    view.deactivate(window, cx);
                } else {
                    view.sync_focus(window, cx);
                }
                assert!(view.forwarded_keys.is_empty());
            });
        });
        cx.simulate_event(gpui::KeyUpEvent {
            physical_key: None,
            key_metadata: None,
            keystroke,
        });
        let expected = b"\x1b[A\x1b[1;8:3A";
        wait(&view, cx, |_| peer.written().len() >= expected.len());
        assert_eq!(peer.written(), expected, "tab deactivation: {deactivate}");
        view.update(cx, |view, _| view.close());
    }
}

fn physical_press(
    cx: &mut VisualTestContext,
    physical_key: Option<gpui::PhysicalKey>,
    key: &str,
    text: &str,
    is_held: bool,
) {
    cx.simulate_event(gpui::KeyDownEvent {
        physical_key,
        key_metadata: None,
        keystroke: gpui::Keystroke {
            key: key.into(),
            key_char: Some(text.into()),
            modifiers: Default::default(),
        },
        is_held,
        prefer_character_input: false,
    });
}

fn physical_release(
    cx: &mut VisualTestContext,
    physical_key: Option<gpui::PhysicalKey>,
    key: &str,
) {
    cx.simulate_event(gpui::KeyUpEvent {
        physical_key,
        key_metadata: None,
        keystroke: gpui::Keystroke {
            key: key.into(),
            key_char: None,
            modifiers: Default::default(),
        },
    });
}

#[gpui::test]
fn physical_key_shifted_punctuation_releases_original_identity_over_ipc(cx: &mut TestAppContext) {
    let peer = Peer::new();
    let (view, cx) = connect(&peer, cx);
    view.update(cx, |view, _| view.engine.advance(b"\x1b[>31u").unwrap());
    // macOS reports Shift+1 as logical "!" with Shift consumed, then "1"
    // on key-up if Shift was released first. Native keyCode remains 18.
    let physical = Some(gpui::PhysicalKey::Native(18));
    physical_press(cx, physical.clone(), "!", "!", false);
    physical_release(cx, physical, "1");
    let expected = b"\x1b[33;;33u\x1b[33;1:3u";
    wait(&view, cx, |_| peer.written().len() >= expected.len());
    assert_eq!(peer.written(), expected);
    view.update(cx, |view, _| {
        assert!(view.forwarded_keys.is_empty());
        view.close();
    });
}

#[gpui::test]
fn physical_key_non_us_repeat_preserves_identity_and_current_text_over_ipc(
    cx: &mut TestAppContext,
) {
    let peer = Peer::new();
    let (view, cx) = connect(&peer, cx);
    view.update(cx, |view, _| view.engine.advance(b"\x1b[>31u").unwrap());
    // Identity must not depend on a US punctuation table, ASCII case folding,
    // or current text. Web code is opaque in exactly the same way as Native.
    let physical = Some(gpui::PhysicalKey::Web("Semicolon".into()));
    physical_press(cx, physical.clone(), "ö", "ö", false);
    physical_press(cx, physical.clone(), "Ö", "Ö", true);
    physical_release(cx, physical, "Ö");
    let expected = b"\x1b[246;;246u\x1b[246;1:2;214u\x1b[246;1:3u";
    wait(&view, cx, |_| peer.written().len() >= expected.len());
    assert_eq!(peer.written(), expected);
    view.update(cx, |view, _| {
        assert!(view.forwarded_keys.is_empty());
        view.close();
    });
}

#[gpui::test]
fn physical_key_collisions_and_synthetic_fallback_are_independent_over_ipc(
    cx: &mut TestAppContext,
) {
    let peer = Peer::new();
    let (view, cx) = connect(&peer, cx);
    view.update(cx, |view, _| view.engine.advance(b"\x1b[>31u").unwrap());
    let first = Some(gpui::PhysicalKey::Native(20));
    let second = Some(gpui::PhysicalKey::Native(21));
    for physical in [first.clone(), second.clone(), None] {
        physical_press(cx, physical, "a", "a", false);
    }
    view.update(cx, |view, _| assert_eq!(view.forwarded_keys.len(), 3));
    // A release with an unknown physical ID must not consume a native or
    // synthetic press just because its logical character happens to match.
    physical_release(cx, Some(gpui::PhysicalKey::Native(22)), "a");
    view.update(cx, |view, _| assert_eq!(view.forwarded_keys.len(), 3));
    for (remaining, physical) in [(2, first), (1, None), (0, second)] {
        physical_release(cx, physical, "a");
        view.update(cx, |view, _| {
            assert_eq!(view.forwarded_keys.len(), remaining)
        });
    }
    // Duplicate key-up cannot emit an extra release.
    physical_release(cx, None, "a");
    let expected = b"\x1b[97;;97u\x1b[97;;97u\x1b[97;;97u\x1b[97;1:3u\x1b[97;1:3u\x1b[97;1:3u";
    wait(&view, cx, |_| peer.written().len() >= expected.len());
    assert_eq!(peer.written(), expected);
    view.update(cx, |view, _| view.close());
}

#[cfg(target_os = "macos")]
#[gpui::test]
fn native_metadata_kitty_layout_identity_over_ipc(cx: &mut TestAppContext) {
    // These are native Mac key positions and independently specified Kitty
    // protocol results. Logical shortcut names deliberately omit raw Shift.
    for (code, base, logical, text, shift, press, release) in [
        (18, "1", "!", "!", true, "\x1b[49:33;2;33u", "\x1b[49;1:3u"),
        (
            41,
            "ö",
            "Ö",
            "Ö",
            true,
            "\x1b[246:214:59;2;214u",
            "\x1b[246::59;1:3u",
        ),
        (
            41,
            "ö",
            "ö",
            "ö",
            false,
            "\x1b[246::59;;246u",
            "\x1b[246::59;1:3u",
        ),
        (
            16,
            "z",
            "z",
            "z",
            false,
            "\x1b[122::121;;122u",
            "\x1b[122::121;1:3u",
        ),
    ] {
        let peer = Peer::new();
        let (view, cx) = connect(&peer, cx);
        view.update(cx, |view, _| view.engine.advance(b"\x1b[>31u").unwrap());
        let metadata = gpui::KeyEventMetadata {
            unmodified_text: Some(base.into()),
            modifiers: gpui::Modifiers {
                shift,
                ..Default::default()
            },
            caps_lock: false,
            num_lock: false,
        };
        cx.simulate_event(gpui::KeyDownEvent {
            physical_key: Some(gpui::PhysicalKey::Native(code)),
            key_metadata: Some(metadata),
            keystroke: gpui::Keystroke {
                key: logical.into(),
                key_char: Some(text.into()),
                modifiers: Default::default(),
            },
            is_held: false,
            prefer_character_input: false,
        });
        // A layout change while held must not change the original identity.
        // Shift is up now: release uses current raw modifiers, not press flags.
        cx.simulate_event(gpui::KeyUpEvent {
            physical_key: Some(gpui::PhysicalKey::Native(code)),
            key_metadata: Some(gpui::KeyEventMetadata {
                unmodified_text: Some("changed layout".into()),
                modifiers: Default::default(),
                caps_lock: false,
                num_lock: false,
            }),
            keystroke: gpui::Keystroke {
                key: base.into(),
                key_char: None,
                modifiers: Default::default(),
            },
        });
        let expected = format!("{press}{release}");
        wait(&view, cx, |_| peer.written().len() >= expected.len());
        assert_eq!(
            peer.written(),
            expected.as_bytes(),
            "native code {code}, base {base}"
        );
        view.update(cx, |view, _| {
            assert!(view.forwarded_keys.is_empty());
            view.close();
        });
    }
}

#[cfg(target_os = "macos")]
#[gpui::test]
fn native_metadata_kitty_repeat_keeps_identity_with_current_modifiers_and_text(
    cx: &mut TestAppContext,
) {
    let peer = Peer::new();
    let (view, cx) = connect(&peer, cx);
    view.update(cx, |view, _| view.engine.advance(b"\x1b[>31u").unwrap());
    for (held, base, text, shift) in [
        (false, "ö", "ö", false),
        (true, "changed layout", "Ö", true),
    ] {
        cx.simulate_event(gpui::KeyDownEvent {
            physical_key: Some(gpui::PhysicalKey::Native(41)),
            key_metadata: Some(gpui::KeyEventMetadata {
                unmodified_text: Some(base.into()),
                modifiers: gpui::Modifiers {
                    shift,
                    ..Default::default()
                },
                caps_lock: false,
                num_lock: false,
            }),
            keystroke: gpui::Keystroke {
                key: text.into(),
                key_char: Some(text.into()),
                modifiers: Default::default(),
            },
            is_held: held,
            prefer_character_input: false,
        });
    }
    cx.simulate_event(gpui::KeyUpEvent {
        physical_key: Some(gpui::PhysicalKey::Native(41)),
        key_metadata: Some(gpui::KeyEventMetadata {
            unmodified_text: Some("wrong".into()),
            modifiers: Default::default(),
            caps_lock: false,
            num_lock: false,
        }),
        keystroke: gpui::Keystroke {
            key: "Ö".into(),
            key_char: None,
            modifiers: Default::default(),
        },
    });
    let expected = b"\x1b[246::59;;246u\x1b[246:214:59;2:2;214u\x1b[246::59;1:3u";
    wait(&view, cx, |_| peer.written().len() >= expected.len());
    assert_eq!(peer.written(), expected);
    view.update(cx, |view, _| view.close());
}

#[gpui::test]
fn focus_loss_releases_keys_with_current_lock_state_over_ipc(cx: &mut TestAppContext) {
    for (deactivate, num_lock) in [(false, true), (true, true), (false, false), (true, false)] {
        let peer = Peer::new();
        let (view, cx) = connect(&peer, cx);
        view.update(cx, |view, _| view.engine.advance(b"\x1b[>3u").unwrap());
        cx.update(|window, _| window.activate_window());
        cx.run_until_parked();
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                view.sync_focus(window, cx);
                assert!(view.focused);
            })
        });
        cx.simulate_numlock_change(!num_lock);
        let keystroke = gpui::Keystroke::parse("up").unwrap();
        cx.simulate_event(gpui::KeyDownEvent {
            physical_key: None,
            key_metadata: Some(gpui::KeyEventMetadata {
                unmodified_text: None,
                modifiers: Default::default(),
                caps_lock: false,
                num_lock: !num_lock,
            }),
            keystroke: keystroke.clone(),
            is_held: false,
            prefer_character_input: false,
        });
        let other_focus = cx.update(|_, cx| cx.focus_handle());
        // Lock state changes after the press. Query the current platform state,
        // never a retained press or metadata from an earlier key event.
        cx.simulate_numlock_change(num_lock);
        cx.simulate_event(gpui::ModifiersChangedEvent {
            modifiers: gpui::Modifiers {
                shift: true,
                alt: true,
                control: true,
                ..Default::default()
            },
            capslock: gpui::Capslock { on: true },
        });
        cx.update(|window, cx| {
            assert_eq!(window.numlock(), Some(num_lock));
            if !deactivate {
                other_focus.focus(window, cx);
            }
            view.update(cx, |view, cx| {
                if deactivate {
                    view.deactivate(window, cx);
                } else {
                    view.sync_focus(window, cx);
                }
                assert!(view.forwarded_keys.is_empty());
            });
        });
        // An OS key-up arriving after the synthetic release cannot release twice.
        cx.simulate_event(gpui::KeyUpEvent {
            physical_key: None,
            key_metadata: None,
            keystroke,
        });
        let expected: &[u8] = if num_lock {
            b"\x1b[A\x1b[1;200:3A"
        } else {
            b"\x1b[1;129A\x1b[1;72:3A"
        };
        wait(&view, cx, |_| peer.written().len() >= expected.len());
        assert_eq!(
            peer.written(),
            expected,
            "deactivate={deactivate}, num_lock={num_lock}"
        );
        view.update(cx, |view, _| view.close());
    }
}
