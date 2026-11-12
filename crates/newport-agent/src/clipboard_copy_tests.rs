//! Programs' copies on a clipboard display that takes them.
use crate::clipboard_source::Source;
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::Path,
    sync::{Arc, mpsc},
    time::Duration,
};

/// The computer's clipboard holds `text`.
fn clipboard(path: &Path, text: &[u8]) {
    let mut tar = tar::Builder::new(fs::File::create(path).unwrap());
    let mut header = tar::Header::new_gnu();
    header.set_size(text.len() as u64);
    header.set_mode(0o600);
    header.set_cksum();
    tar.append_data(&mut header, "text/plain", text).unwrap();
    tar.finish().unwrap();
}

/// A display source reading the computer's clipboard, and what its programs
/// copied for the computer.
fn copying(dir: &Path) -> (Source, mpsc::Receiver<String>) {
    let path = dir.join("snapshot.tar");
    clipboard(&path, b"from the Mac");
    let (sent, copied) = mpsc::channel();
    let source = Source::new(&path).with_copies(Arc::new(move |text| {
        let _ = sent.send(text);
    }));
    (source, copied)
}

mod x11 {
    use super::*;
    use std::os::unix::net::UnixStream;
    use x11rb::{
        CURRENT_TIME,
        connection::Connection,
        protocol::{Event, xproto::*},
        rust_connection::{DefaultStream, RustConnection},
        wrapper::ConnectionExt as _,
    };

    fn connect(server: &crate::x_server::Server) -> RustConnection {
        let socket =
            UnixStream::connect(format!("/tmp/.X11-unix/X{}", &server.display[1..])).unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let stream = DefaultStream::from_unix_stream(socket).unwrap().0;
        RustConnection::connect_to_stream(stream, 0).unwrap()
    }
    fn atom(c: &RustConnection, name: &str) -> u32 {
        c.intern_atom(false, name.as_bytes())
            .unwrap()
            .reply()
            .unwrap()
            .atom
    }
    fn window(c: &RustConnection) -> u32 {
        let id = c.generate_id().unwrap();
        c.create_window(
            x11rb::COPY_DEPTH_FROM_PARENT,
            id,
            c.setup().roots[0].root,
            0,
            0,
            1,
            1,
            0,
            WindowClass::COPY_FROM_PARENT,
            x11rb::COPY_FROM_PARENT,
            &CreateWindowAux::new(),
        )
        .unwrap()
        .check()
        .unwrap();
        id
    }
    fn owner(c: &RustConnection, selection: u32) -> u32 {
        c.get_selection_owner(selection)
            .unwrap()
            .reply()
            .unwrap()
            .owner
    }
    fn notify(c: &RustConnection, request: &SelectionRequestEvent, property: u32) {
        let event = SelectionNotifyEvent {
            response_type: SELECTION_NOTIFY_EVENT,
            sequence: 0,
            time: request.time,
            requestor: request.requestor,
            selection: request.selection,
            target: request.target,
            property,
        };
        c.send_event(false, request.requestor, EventMask::NO_EVENT, event)
            .unwrap()
            .check()
            .unwrap();
    }

    /// Owns `selection` as xclip does, serving `text` as UTF8_STRING (in
    /// parts of `part` bytes when given) until the server takes it. Returns
    /// the targets it was asked for.
    fn serve(c: &RustConnection, selection: &str, text: &[u8], part: Option<usize>) -> Vec<String> {
        let w = window(c);
        let selection = atom(c, selection);
        let (targets, utf8, incr) = (atom(c, "TARGETS"), atom(c, "UTF8_STRING"), atom(c, "INCR"));
        c.set_selection_owner(w, selection, CURRENT_TIME)
            .unwrap()
            .check()
            .unwrap();
        // xsel checks it owns the selection before serving it.
        assert_eq!(owner(c, selection), w);
        let mut asked = Vec::new();
        let mut sending: Option<(u32, u32, std::slice::Chunks<u8>, bool)> = None;
        loop {
            match c.wait_for_event().unwrap() {
                Event::SelectionRequest(request) => {
                    let name = c
                        .get_atom_name(request.target)
                        .unwrap()
                        .reply()
                        .unwrap()
                        .name;
                    asked.push(String::from_utf8(name).unwrap());
                    if request.target == targets {
                        c.change_property32(
                            PropMode::REPLACE,
                            request.requestor,
                            request.property,
                            AtomEnum::ATOM,
                            &[targets, utf8],
                        )
                        .unwrap();
                        notify(c, &request, request.property);
                    } else if request.target != utf8 {
                        notify(c, &request, x11rb::NONE);
                    } else if let Some(part) = part {
                        c.change_property32(
                            PropMode::REPLACE,
                            request.requestor,
                            request.property,
                            incr,
                            &[text.len() as u32],
                        )
                        .unwrap();
                        c.change_window_attributes(
                            request.requestor,
                            &ChangeWindowAttributesAux::new()
                                .event_mask(EventMask::PROPERTY_CHANGE),
                        )
                        .unwrap();
                        notify(c, &request, request.property);
                        sending = Some((
                            request.requestor,
                            request.property,
                            text.chunks(part),
                            false,
                        ));
                    } else {
                        c.change_property8(
                            PropMode::REPLACE,
                            request.requestor,
                            request.property,
                            utf8,
                            text,
                        )
                        .unwrap();
                        notify(c, &request, request.property);
                    }
                }
                Event::PropertyNotify(event) if event.state == Property::DELETE => {
                    let Some((requestor, property, parts, done)) = &mut sending else {
                        continue;
                    };
                    if *done || event.window != *requestor || event.atom != *property {
                        continue;
                    }
                    let part = parts.next().unwrap_or_default();
                    *done = part.is_empty();
                    c.change_property8(PropMode::REPLACE, *requestor, *property, utf8, part)
                        .unwrap()
                        .check()
                        .unwrap();
                }
                Event::SelectionClear(event) => {
                    assert_eq!((event.owner, event.selection), (w, selection));
                    return asked;
                }
                _ => (),
            }
        }
    }

    #[test]
    fn a_programs_clipboard_goes_to_the_computer_and_the_selection_back_to_its_clipboard() {
        let dir = tempfile::tempdir().unwrap();
        let (source, copied) = copying(dir.path());
        let server = crate::x_server::Server::start_source(source, None).unwrap();
        let c = connect(&server);
        let asked = serve(
            &c,
            "CLIPBOARD",
            "copied on the server, 世界".as_bytes(),
            None,
        );
        assert_eq!(asked, ["TARGETS", "UTF8_STRING"]);
        assert_eq!(
            copied.recv_timeout(Duration::from_secs(5)).unwrap(),
            "copied on the server, 世界"
        );
        // The selection is the computer's clipboard again.
        assert_eq!(owner(&c, atom(&c, "CLIPBOARD")), 2);
    }

    /// What `selection` reads as `target`, as a requestor's window `w` asks.
    fn read(c: &RustConnection, w: u32, selection: &str, target: &str) -> Option<Vec<u8>> {
        let property = atom(c, "TEST_READ");
        c.convert_selection(
            w,
            atom(c, selection),
            atom(c, target),
            property,
            CURRENT_TIME,
        )
        .unwrap()
        .check()
        .unwrap();
        loop {
            if let Event::SelectionNotify(event) = c.wait_for_event().unwrap() {
                if event.property == x11rb::NONE {
                    return None;
                }
                break;
            }
        }
        Some(
            c.get_property(true, w, property, AtomEnum::ANY, 0, u32::MAX / 4)
                .unwrap()
                .reply()
                .unwrap()
                .value,
        )
    }

    #[test]
    fn a_copy_reads_back_at_once_until_the_computers_clipboard_changes() {
        let dir = tempfile::tempdir().unwrap();
        let (source, copied) = copying(dir.path());
        let server = crate::x_server::Server::start_source(source, None).unwrap();
        let c = connect(&server);
        let w = window(&c);
        serve(&c, "CLIPBOARD", "copied, 世界".as_bytes(), None);
        copied.recv_timeout(Duration::from_secs(5)).unwrap();
        // Before the computer has it, as `xclip -sel c; xclip -o -sel c` reads.
        for target in ["UTF8_STRING", "TEXT", "text/plain"] {
            assert_eq!(
                read(&c, w, "CLIPBOARD", target).as_deref(),
                Some("copied, 世界".as_bytes()),
                "{target}"
            );
        }
        assert_eq!(
            read(&c, w, "CLIPBOARD", "STRING").as_deref(),
            Some(b"copied, ??".as_slice())
        );
        // The computer's next clipboard replaces it, as a new snapshot.
        let next = dir.path().join("next.tar");
        clipboard(&next, b"copied later on the Mac");
        fs::rename(&next, dir.path().join("snapshot.tar")).unwrap();
        assert_eq!(
            read(&c, w, "CLIPBOARD", "UTF8_STRING").as_deref(),
            Some(b"copied later on the Mac".as_slice())
        );
    }

    #[test]
    fn a_program_that_takes_the_selection_back_sends_its_copy_once() {
        let dir = tempfile::tempdir().unwrap();
        let (source, copied) = copying(dir.path());
        let server = crate::x_server::Server::start_source(source, None).unwrap();
        let c = connect(&server);
        serve(&c, "CLIPBOARD", b"kept by a clipboard manager", None);
        copied.recv_timeout(Duration::from_secs(5)).unwrap();
        // It owns the selection again, as clipboard managers do once they
        // lose it, with the same text: the server takes it and keeps it there.
        let w = window(&c);
        let clipboard = atom(&c, "CLIPBOARD");
        let (targets, utf8) = (atom(&c, "TARGETS"), atom(&c, "UTF8_STRING"));
        c.set_selection_owner(w, clipboard, CURRENT_TIME)
            .unwrap()
            .check()
            .unwrap();
        let mut answered = 0;
        let started = std::time::Instant::now();
        while started.elapsed() < Duration::from_millis(500) {
            match c.poll_for_event().unwrap() {
                Some(Event::SelectionRequest(request)) if request.target == targets => {
                    c.change_property32(
                        PropMode::REPLACE,
                        request.requestor,
                        request.property,
                        AtomEnum::ATOM,
                        &[targets, utf8],
                    )
                    .unwrap();
                    notify(&c, &request, request.property);
                }
                Some(Event::SelectionRequest(request)) => {
                    c.change_property8(
                        PropMode::REPLACE,
                        request.requestor,
                        request.property,
                        utf8,
                        b"kept by a clipboard manager",
                    )
                    .unwrap();
                    notify(&c, &request, request.property);
                    answered += 1;
                }
                Some(Event::SelectionClear(_)) => panic!("the repeated copy was given back"),
                Some(_) => (),
                None => std::thread::sleep(Duration::from_millis(10)),
            }
        }
        assert_eq!(answered, 1, "the server took the repeated copy once");
        assert!(copied.recv_timeout(Duration::from_millis(200)).is_err());
    }

    #[test]
    fn a_second_program_copying_the_same_text_is_told_it_lost_the_selection() {
        let dir = tempfile::tempdir().unwrap();
        let (source, copied) = copying(dir.path());
        let server = crate::x_server::Server::start_source(source, None).unwrap();
        // `yy` twice in an editor that copies with `xclip -i`: two programs,
        // the same text, before the computer's clipboard has it.
        let first = connect(&server);
        serve(&first, "CLIPBOARD", b"yanked twice", None);
        copied.recv_timeout(Duration::from_secs(5)).unwrap();
        // The second is told it lost the selection, so it exits rather than
        // waiting for the terminal's end.
        let c = connect(&server);
        let w = window(&c);
        let (targets, utf8) = (atom(&c, "TARGETS"), atom(&c, "UTF8_STRING"));
        c.set_selection_owner(w, atom(&c, "CLIPBOARD"), CURRENT_TIME)
            .unwrap()
            .check()
            .unwrap();
        let started = std::time::Instant::now();
        let cleared = loop {
            if started.elapsed() > Duration::from_secs(2) {
                break false;
            }
            match c.poll_for_event().unwrap() {
                Some(Event::SelectionRequest(request)) => {
                    if request.target == targets {
                        c.change_property32(
                            PropMode::REPLACE,
                            request.requestor,
                            request.property,
                            AtomEnum::ATOM,
                            &[targets, utf8],
                        )
                        .unwrap();
                    } else {
                        c.change_property8(
                            PropMode::REPLACE,
                            request.requestor,
                            request.property,
                            utf8,
                            b"yanked twice",
                        )
                        .unwrap();
                    }
                    notify(&c, &request, request.property);
                }
                Some(Event::SelectionClear(_)) => break true,
                Some(_) => (),
                None => std::thread::sleep(Duration::from_millis(10)),
            }
        };
        assert!(cleared, "the second program still owns the selection");
        assert!(copied.recv_timeout(Duration::from_millis(200)).is_err());
    }

    #[test]
    fn a_copy_sent_in_parts_arrives_whole() {
        let dir = tempfile::tempdir().unwrap();
        let (source, copied) = copying(dir.path());
        let server = crate::x_server::Server::start_source(source, None).unwrap();
        let c = connect(&server);
        let text = "a long copy, sent in parts\n".repeat(20_000);
        serve(&c, "CLIPBOARD", text.as_bytes(), Some(16 * 1024));
        assert_eq!(copied.recv_timeout(Duration::from_secs(5)).unwrap(), text);
    }

    #[test]
    fn a_programs_primary_selection_never_leaves_the_server() {
        let dir = tempfile::tempdir().unwrap();
        let (source, copied) = copying(dir.path());
        let server = crate::x_server::Server::start_source(source, None).unwrap();
        let c = connect(&server);
        let asked = serve(&c, "PRIMARY", b"selected on the server", None);
        assert_eq!(asked, ["TARGETS"]);
        assert!(copied.recv_timeout(Duration::from_millis(300)).is_err());
        assert_eq!(owner(&c, atom(&c, "PRIMARY")), 2);
        // CLIPBOARD still reaches the computer afterwards.
        serve(&c, "CLIPBOARD", b"copied", None);
        assert_eq!(
            copied.recv_timeout(Duration::from_secs(5)).unwrap(),
            "copied"
        );
    }

    #[test]
    fn an_owner_without_text_is_given_the_selection_back_and_nothing_is_sent() {
        let dir = tempfile::tempdir().unwrap();
        let (source, copied) = copying(dir.path());
        let server = crate::x_server::Server::start_source(source, None).unwrap();
        let c = connect(&server);
        let w = window(&c);
        let clipboard = atom(&c, "CLIPBOARD");
        c.set_selection_owner(w, clipboard, CURRENT_TIME)
            .unwrap()
            .check()
            .unwrap();
        let mut asked = Vec::new();
        loop {
            match c.wait_for_event().unwrap() {
                Event::SelectionRequest(request) => {
                    asked.push(request.target);
                    notify(&c, &request, x11rb::NONE);
                }
                Event::SelectionClear(_) => break,
                _ => (),
            }
        }
        // It lists no targets, so text is asked for by name.
        assert_eq!(
            asked,
            [
                atom(&c, "TARGETS"),
                atom(&c, "UTF8_STRING"),
                atom(&c, "STRING")
            ]
        );
        assert!(copied.recv_timeout(Duration::from_millis(300)).is_err());
    }

    #[cfg(target_os = "linux")]
    fn native(
        server: &crate::x_server::Server,
        program: &str,
        args: &[&str],
        input: &[u8],
    ) -> std::process::Output {
        use std::io::Write;
        let binary = match program {
            "xclip" => {
                std::env::var("NEWPORT_TEST_XCLIP").unwrap_or_else(|_| "/usr/bin/xclip".into())
            }
            _ => std::env::var("NEWPORT_TEST_XSEL").unwrap_or_else(|_| "/usr/bin/xsel".into()),
        };
        let mut child = std::process::Command::new("timeout")
            .arg("8")
            .arg(binary)
            .args(args)
            .env("DISPLAY", &server.display)
            .env("XAUTHORITY", server.authority())
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap_or_else(|error| panic!("Install {program}: {error}"));
        child.stdin.take().unwrap().write_all(input).unwrap();
        child.wait_with_output().unwrap()
    }

    /// Run in the foreground, the system's xclip exits once the server has
    /// taken its selection: copies leave nothing behind.
    #[cfg(target_os = "linux")]
    #[test]
    fn native_xclip_copies_reach_the_computer_and_its_primary_does_not() {
        let dir = tempfile::tempdir().unwrap();
        let (source, copied) = copying(dir.path());
        let server = crate::x_server::Server::start_source(source, None).unwrap();
        let large = "xclip sends this in parts\n".repeat(4_000);
        for text in ["copied by xclip, 世界", large.as_str()] {
            let output = native(
                &server,
                "xclip",
                &["-quiet", "-selection", "clipboard"],
                text.as_bytes(),
            );
            assert!(output.status.success(), "{output:?}");
            assert_eq!(copied.recv_timeout(Duration::from_secs(5)).unwrap(), text);
        }
        let output = native(&server, "xclip", &["-quiet"], b"selected");
        assert!(output.status.success(), "{output:?}");
        assert!(copied.recv_timeout(Duration::from_millis(300)).is_err());
        // Selection paste reads the last copy until the computer has it.
        let output = native(&server, "xclip", &["-o"], b"");
        assert_eq!(output.stdout, large.as_bytes());
    }

    /// xsel serves a copy in the foreground and exits once it is taken.
    #[cfg(target_os = "linux")]
    #[test]
    fn native_xsel_copies_reach_the_computer_and_its_primary_does_not() {
        let dir = tempfile::tempdir().unwrap();
        let (source, copied) = copying(dir.path());
        let server = crate::x_server::Server::start_source(source, None).unwrap();
        let output = native(
            &server,
            "xsel",
            &["-n", "-ib"],
            "copied by xsel, 世界".as_bytes(),
        );
        assert!(output.status.success(), "{output:?}");
        assert_eq!(
            copied.recv_timeout(Duration::from_secs(5)).unwrap(),
            "copied by xsel, 世界"
        );
        let output = native(&server, "xsel", &["-n", "-i"], b"selected");
        assert!(output.status.success(), "{output:?}");
        assert!(copied.recv_timeout(Duration::from_millis(300)).is_err());
        // Both read the last copy until the computer has it.
        for args in [&["-o"][..], &["-bo"]] {
            let output = native(&server, "xsel", args, b"");
            assert_eq!(output.stdout, "copied by xsel, 世界".as_bytes(), "{args:?}");
        }
    }
}

mod wayland {
    use super::*;
    use std::os::{fd::AsFd, unix::net::UnixStream};
    use wayland_client::{
        Connection, Dispatch, QueueHandle,
        protocol::{wl_compositor, wl_registry, wl_seat, wl_surface},
    };
    use wayland_protocols_wlr::data_control::v1::client::{
        zwlr_data_control_device_v1::{self as device, ZwlrDataControlDeviceV1 as Device},
        zwlr_data_control_manager_v1::ZwlrDataControlManagerV1 as Manager,
        zwlr_data_control_offer_v1::ZwlrDataControlOfferV1 as Offer,
        zwlr_data_control_source_v1::{self as source, ZwlrDataControlSourceV1 as DataSource},
    };

    /// A program that copies as wl-copy does.
    #[derive(Default)]
    struct Copier {
        manager: Option<Manager>,
        seat: Option<wl_seat::WlSeat>,
        probe_compositor: bool,
        compositor: Option<wl_compositor::WlCompositor>,
        text: Vec<u8>,
        asked: Vec<String>,
        cancelled: bool,
        /// Keeps the pipe it is asked to write to instead of writing, as a
        /// program slow to send its copy.
        hold: bool,
        held: Option<std::os::fd::OwnedFd>,
    }
    impl Dispatch<wl_registry::WlRegistry, ()> for Copier {
        fn event(
            state: &mut Self,
            registry: &wl_registry::WlRegistry,
            event: wl_registry::Event,
            _: &(),
            _: &Connection,
            q: &QueueHandle<Self>,
        ) {
            if let wl_registry::Event::Global {
                name,
                interface,
                version,
            } = event
            {
                match interface.as_str() {
                    "wl_seat" => state.seat = Some(registry.bind(name, version.min(7), q, ())),
                    "zwlr_data_control_manager_v1" => {
                        state.manager = Some(registry.bind(name, version.min(2), q, ()))
                    }
                    "wl_compositor" if state.probe_compositor => {
                        state.compositor = Some(registry.bind(name, 2, q, ()))
                    }
                    _ => (),
                }
            }
        }
    }
    wayland_client::delegate_noop!(Copier: ignore wl_seat::WlSeat);
    wayland_client::delegate_noop!(Copier: ignore wl_compositor::WlCompositor);
    wayland_client::delegate_noop!(Copier: ignore wl_surface::WlSurface);
    wayland_client::delegate_noop!(Copier: ignore Manager);
    wayland_client::delegate_noop!(Copier: ignore Offer);
    impl Dispatch<Device, ()> for Copier {
        fn event(
            _: &mut Self,
            _: &Device,
            _: device::Event,
            _: &(),
            _: &Connection,
            _: &QueueHandle<Self>,
        ) {
        }
        wayland_client::event_created_child!(Copier, Device, [0 => (Offer, ())]);
    }
    impl Dispatch<DataSource, ()> for Copier {
        fn event(
            state: &mut Self,
            _: &DataSource,
            event: source::Event,
            _: &(),
            _: &Connection,
            _: &QueueHandle<Self>,
        ) {
            match event {
                source::Event::Send { mime_type, fd } if state.hold => {
                    state.asked.push(mime_type);
                    state.held = Some(fd);
                }
                source::Event::Send { mime_type, fd } => {
                    use std::io::Write;
                    state.asked.push(mime_type);
                    let _ = fs::File::from(fd).write_all(&state.text);
                }
                source::Event::Cancelled => state.cancelled = true,
                _ => (),
            }
        }
    }

    /// Copies `text` to the selection, or the primary one, and waits until
    /// the server replaces it. Returns the types it was asked for.
    fn copy(server: &crate::wl_server::Server, primary: bool, text: &[u8]) -> Vec<String> {
        copier(server, primary, text, false).asked
    }
    fn copier(server: &crate::wl_server::Server, primary: bool, text: &[u8], hold: bool) -> Copier {
        let socket = UnixStream::connect(server.socket()).unwrap();
        let conn = Connection::from_socket(socket).unwrap();
        let mut q = conn.new_event_queue();
        let mut state = Copier {
            text: text.to_vec(),
            hold,
            ..Default::default()
        };
        let _registry = conn.display().get_registry(&q.handle(), ());
        q.roundtrip(&mut state).unwrap();
        let manager = state.manager.clone().unwrap();
        let device = manager.get_data_device(state.seat.as_ref().unwrap(), &q.handle(), ());
        let source = manager.create_data_source(&q.handle(), ());
        for mime in ["text/plain;charset=utf-8", "text/plain", "UTF8_STRING"] {
            source.offer(mime.into());
        }
        if primary {
            device.set_primary_selection(Some(&source));
        } else {
            device.set_selection(Some(&source));
        }
        let started = std::time::Instant::now();
        while !state.cancelled {
            assert!(started.elapsed() < Duration::from_secs(5), "never replaced");
            q.roundtrip(&mut state).unwrap();
            std::thread::sleep(Duration::from_millis(10));
        }
        state
    }

    /// A program that pastes as wl-paste does.
    #[derive(Default)]
    struct Paster {
        manager: Option<Manager>,
        seat: Option<wl_seat::WlSeat>,
        selection: Option<Offer>,
    }
    impl Dispatch<wl_registry::WlRegistry, ()> for Paster {
        fn event(
            state: &mut Self,
            registry: &wl_registry::WlRegistry,
            event: wl_registry::Event,
            _: &(),
            _: &Connection,
            q: &QueueHandle<Self>,
        ) {
            if let wl_registry::Event::Global {
                name,
                interface,
                version,
            } = event
            {
                match interface.as_str() {
                    "wl_seat" => state.seat = Some(registry.bind(name, version.min(7), q, ())),
                    "zwlr_data_control_manager_v1" => {
                        state.manager = Some(registry.bind(name, version.min(2), q, ()))
                    }
                    _ => (),
                }
            }
        }
    }
    wayland_client::delegate_noop!(Paster: ignore wl_seat::WlSeat);
    wayland_client::delegate_noop!(Paster: ignore Manager);
    wayland_client::delegate_noop!(Paster: ignore Offer);
    impl Dispatch<Device, ()> for Paster {
        fn event(
            state: &mut Self,
            _: &Device,
            event: device::Event,
            _: &(),
            _: &Connection,
            _: &QueueHandle<Self>,
        ) {
            if let device::Event::Selection { id } = event {
                state.selection = id;
            }
        }
        wayland_client::event_created_child!(Paster, Device, [0 => (Offer, ())]);
    }

    /// Asks for the selection's text; the server has the request once this
    /// returns. Read the pipe for it.
    /// A program that pastes as wl-paste does, offered the selection.
    struct Pasting {
        conn: Connection,
        q: wayland_client::EventQueue<Paster>,
        state: Paster,
        offer: Offer,
    }
    fn offered(server: &crate::wl_server::Server) -> Pasting {
        let socket = UnixStream::connect(server.socket()).unwrap();
        let conn = Connection::from_socket(socket).unwrap();
        let mut q = conn.new_event_queue();
        let mut state = Paster::default();
        let _registry = conn.display().get_registry(&q.handle(), ());
        q.roundtrip(&mut state).unwrap();
        let manager = state.manager.clone().unwrap();
        let _device = manager.get_data_device(state.seat.as_ref().unwrap(), &q.handle(), ());
        q.roundtrip(&mut state).unwrap();
        let offer = state.selection.clone().expect("a selection");
        Pasting {
            conn,
            q,
            state,
            offer,
        }
    }
    /// Reads what it was offered.
    fn receive(mut pasting: Pasting) -> (Connection, fs::File) {
        let (reader, writer) = std::io::pipe().unwrap();
        pasting
            .offer
            .receive("text/plain;charset=utf-8".into(), writer.as_fd());
        drop(writer);
        pasting.q.roundtrip(&mut pasting.state).unwrap();
        (
            pasting.conn,
            fs::File::from(std::os::fd::OwnedFd::from(reader)),
        )
    }
    fn paste(server: &crate::wl_server::Server) -> (Connection, fs::File) {
        receive(offered(server))
    }
    fn read_all(mut file: fs::File) -> Vec<u8> {
        use std::io::Read;
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes).unwrap();
        bytes
    }

    #[test]
    fn a_copy_reads_back_even_while_it_is_still_being_sent() {
        use std::io::Write;
        let dir = tempfile::tempdir().unwrap();
        let (source, copied) = copying(dir.path());
        let server = server(dir.path(), source);
        // The program is told its selection was replaced before it sent the
        // text: `wl-copy x; wl-paste` asks in between.
        let held = copier(&server, false, b"", true)
            .held
            .expect("asked to send");
        let (_paster, pasted) = paste(&server);
        fs::File::from(held)
            .write_all(b"copied, then pasted")
            .unwrap();
        assert_eq!(read_all(pasted), b"copied, then pasted");
        assert_eq!(
            copied.recv_timeout(Duration::from_secs(5)).unwrap(),
            "copied, then pasted"
        );
        // Until the computer's clipboard changes.
        let next = dir.path().join("next.tar");
        clipboard(&next, b"copied later on the Mac");
        fs::rename(&next, dir.path().join("snapshot.tar")).unwrap();
        let (_paster, pasted) = paste(&server);
        assert_eq!(read_all(pasted), b"copied later on the Mac");
    }

    #[test]
    fn a_read_offered_before_a_copy_finished_returns_the_copy() {
        let dir = tempfile::tempdir().unwrap();
        let (source, copied) = copying(dir.path());
        let server = server(dir.path(), source);
        // wl-paste is offered the computer's clipboard, and a program's copy
        // finishes before it reads.
        let pasting = offered(&server);
        copy(&server, false, b"copied before the read");
        assert_eq!(
            copied.recv_timeout(Duration::from_secs(5)).unwrap(),
            "copied before the read"
        );
        let (_paster, pasted) = receive(pasting);
        assert_eq!(read_all(pasted), b"copied before the read");
        // An offer from before the computer's clipboard changed reads nothing.
        let pasting = offered(&server);
        let next = dir.path().join("next.tar");
        clipboard(&next, b"copied later on the Mac");
        fs::rename(&next, dir.path().join("snapshot.tar")).unwrap();
        let (_paster, pasted) = receive(pasting);
        assert_eq!(read_all(pasted), b"");
    }

    #[test]
    fn a_read_waiting_on_a_copy_never_returns_a_later_computer_clipboard() {
        let dir = tempfile::tempdir().unwrap();
        let (source, copied) = copying(dir.path());
        let server = server(dir.path(), source);
        // wl-paste is offered the computer's clipboard while a program's copy
        // is in flight. The computer's clipboard changes, and the copy ends
        // with nothing to send.
        let held = copier(&server, false, b"", true)
            .held
            .expect("asked to send");
        let (_paster, pasted) = paste(&server);
        let next = dir.path().join("next.tar");
        clipboard(&next, b"copied later on the Mac");
        fs::rename(&next, dir.path().join("snapshot.tar")).unwrap();
        drop(held);
        assert_eq!(read_all(pasted), b"");
        assert!(copied.recv_timeout(Duration::from_millis(300)).is_err());
    }

    fn server(dir: &Path, source: Source) -> crate::wl_server::Server {
        fs::set_permissions(dir, fs::Permissions::from_mode(0o700)).unwrap();
        crate::wl_server::Server::start_source(source, &dir.join("t.wl")).unwrap()
    }

    #[test]
    fn a_programs_selection_goes_to_the_computer_and_its_primary_does_not() {
        let dir = tempfile::tempdir().unwrap();
        let (source, copied) = copying(dir.path());
        let server = server(dir.path(), source);
        assert_eq!(
            copy(&server, false, "copied on the server, 世界".as_bytes()),
            ["text/plain;charset=utf-8"]
        );
        assert_eq!(
            copied.recv_timeout(Duration::from_secs(5)).unwrap(),
            "copied on the server, 世界"
        );
        assert!(copy(&server, true, b"selected on the server").is_empty());
        assert!(copied.recv_timeout(Duration::from_millis(300)).is_err());
    }

    #[test]
    fn a_display_that_takes_no_copies_refuses_them() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("snapshot.tar");
        clipboard(&path, b"from the Mac");
        let server = server(dir.path(), Source::new(&path));
        let socket = UnixStream::connect(server.socket()).unwrap();
        let conn = Connection::from_socket(socket).unwrap();
        let mut q = conn.new_event_queue();
        let mut state = Copier::default();
        let _registry = conn.display().get_registry(&q.handle(), ());
        q.roundtrip(&mut state).unwrap();
        state
            .manager
            .clone()
            .unwrap()
            .create_data_source(&q.handle(), ());
        assert!(q.roundtrip(&mut state).is_err());
    }

    #[test]
    fn clipboard_compositor_probing_is_quiet_but_surface_creation_requests_a_gui() {
        let dir = tempfile::tempdir().unwrap();
        let (source, _copied) = copying(dir.path());
        let notices = source.gui.clone();
        notices.enable();
        let server = server(dir.path(), source);
        let conn = Connection::from_socket(UnixStream::connect(server.socket()).unwrap()).unwrap();
        let mut queue = conn.new_event_queue();
        let mut state = Copier {
            probe_compositor: true,
            ..Default::default()
        };
        let _registry = conn.display().get_registry(&queue.handle(), ());
        queue.roundtrip(&mut state).unwrap();
        queue.roundtrip(&mut state).unwrap();
        assert!(!notices.take());
        let _surface = state
            .compositor
            .as_ref()
            .unwrap()
            .create_surface(&queue.handle(), ());
        assert!(queue.roundtrip(&mut state).is_err());
        assert!(notices.take());
    }

    /// The system's wl-copy, in the foreground, exits once the server has
    /// taken its selection.
    #[cfg(target_os = "linux")]
    #[test]
    fn native_wl_copy_reaches_the_computer_and_its_primary_does_not() {
        use std::io::Write;
        let dir = tempfile::tempdir().unwrap();
        let (source, copied) = copying(dir.path());
        let notices = source.gui.clone();
        notices.enable();
        let server = server(dir.path(), source);
        let directory =
            std::env::var("NEWPORT_TEST_WL_CLIPBOARD").unwrap_or_else(|_| "/usr/bin".into());
        let run = |args: &[&str], input: &[u8]| {
            let mut child = std::process::Command::new("timeout")
                .arg("8")
                .arg(Path::new(&directory).join("wl-copy"))
                .arg("--foreground")
                .args(args)
                .env("WAYLAND_DISPLAY", server.socket())
                .env_remove("WAYLAND_SOCKET")
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn()
                .expect("Install wl-clipboard or set NEWPORT_TEST_WL_CLIPBOARD to its directory");
            child.stdin.take().unwrap().write_all(input).unwrap();
            child.wait_with_output().unwrap()
        };
        let output = run(&[], "copied by wl-copy, 世界".as_bytes());
        assert!(output.status.success(), "{output:?}");
        assert_eq!(
            copied
                .recv_timeout(Duration::from_secs(5))
                .unwrap_or_else(|error| panic!("{error}: {output:?}")),
            "copied by wl-copy, 世界"
        );
        assert!(!notices.take());
        let output = run(&["--primary"], b"selected");
        assert!(output.status.success(), "{output:?}");
        assert!(copied.recv_timeout(Duration::from_millis(300)).is_err());
        assert!(!notices.take());
    }
}
