use porthop_agent::x_server::Server;
use std::{fs, io::Write, os::unix::net::UnixStream, path::Path};
use x11rb::{
    connection::Connection,
    protocol::{xproto::*, Event},
    rust_connection::{DefaultStream, RustConnection},
    wrapper::ConnectionExt as _,
    COPY_DEPTH_FROM_PARENT, COPY_FROM_PARENT, CURRENT_TIME,
};

fn snapshot(path: &Path, entries: &[(&str, &[u8])]) {
    let tmp = path.with_extension("new");
    let mut archive = tar::Builder::new(fs::File::create(&tmp).unwrap());
    for (name, bytes) in entries {
        let mut h = tar::Header::new_gnu();
        h.set_size(bytes.len() as u64);
        h.set_mode(0o600);
        h.set_cksum();
        archive.append_data(&mut h, *name, *bytes).unwrap();
    }
    archive.into_inner().unwrap().flush().unwrap();
    fs::rename(tmp, path).unwrap();
}
fn connect(server: &Server) -> RustConnection {
    let socket = UnixStream::connect(format!("/tmp/.X11-unix/X{}", &server.display[1..])).unwrap();
    socket
        .set_read_timeout(Some(std::time::Duration::from_secs(5)))
        .unwrap();
    let stream = DefaultStream::from_unix_stream(socket).unwrap().0;
    let auth = fs::read(server.authority()).unwrap();
    RustConnection::connect_to_stream_with_auth_info(
        stream,
        0,
        b"MIT-MAGIC-COOKIE-1".to_vec(),
        auth[auth.len() - 16..].to_vec(),
    )
    .unwrap()
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
        COPY_DEPTH_FROM_PARENT,
        id,
        c.setup().roots[0].root,
        0,
        0,
        1,
        1,
        0,
        WindowClass::COPY_FROM_PARENT,
        COPY_FROM_PARENT,
        &CreateWindowAux::new()
            .event_mask(EventMask::PROPERTY_CHANGE | EventMask::STRUCTURE_NOTIFY),
    )
    .unwrap()
    .check()
    .unwrap();
    id
}
fn read(c: &RustConnection, w: u32, kind: &str) -> Option<Vec<u8>> {
    let selection = atom(c, "CLIPBOARD");
    let target = atom(c, kind);
    let property = atom(c, "TEST_PROPERTY");
    c.delete_property(w, property).unwrap();
    c.convert_selection(w, selection, target, property, CURRENT_TIME)
        .unwrap();
    c.flush().unwrap();
    loop {
        if let Event::SelectionNotify(e) = c.wait_for_event().unwrap() {
            if e.property == 0 {
                return None;
            }
            break;
        }
    }
    let first = c
        .get_property(false, w, property, AtomEnum::ANY, 0, u32::MAX / 4)
        .unwrap()
        .reply()
        .unwrap();
    if first.type_ != atom(c, "INCR") {
        c.delete_property(w, property).unwrap();
        c.flush().unwrap();
        return Some(first.value);
    }
    // Drain the initial property's queued NewValue event before acknowledging INCR.
    c.sync().unwrap();
    while c.poll_for_event().unwrap().is_some() {}
    c.delete_property(w, property).unwrap();
    c.flush().unwrap();
    let mut result = Vec::new();
    loop {
        if let Event::PropertyNotify(e) = c.wait_for_event().unwrap() {
            if e.atom == property && e.state == Property::NEW_VALUE {
                let p = c
                    .get_property(true, w, property, target, 0, u32::MAX / 4)
                    .unwrap()
                    .reply()
                    .unwrap();
                if p.value.is_empty() {
                    return Some(result);
                }
                result.extend(p.value);
            }
        }
    }
}

#[test]
fn reads_formats_and_large_image_across_connections() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("snapshot.tar");
    let image: Vec<_> = (0..300_001).map(|i| (i % 251) as u8).collect();
    snapshot(
        &path,
        &[
            ("text/plain", b"hello \xf0\x9f\x8c\x8d"),
            ("image/png", &image),
        ],
    );
    let server = Server::start(&path, None).unwrap();
    let c = connect(&server);
    let w = window(&c);
    assert_eq!(
        read(&c, w, "UTF8_STRING").unwrap(),
        b"hello \xf0\x9f\x8c\x8d"
    );
    assert!(read(&c, w, "text/uri-list").is_none());
    assert_eq!(read(&c, w, "image/png").unwrap(), image);
    let targets = read(&c, w, "TARGETS").unwrap();
    assert!(targets
        .chunks_exact(4)
        .any(|v| u32::from_ne_bytes(v.try_into().unwrap()) == atom(&c, "image/png")));
    let second = connect(&server);
    assert_eq!(atom(&second, "image/png"), atom(&c, "image/png"));
    let w2 = window(&second);
    assert_eq!(
        read(&second, w2, "UTF8_STRING").unwrap(),
        b"hello \xf0\x9f\x8c\x8d"
    );
    snapshot(&path, &[("text/plain", b"replacement")]);
    assert_eq!(read(&c, w, "UTF8_STRING").unwrap(), b"replacement");
    assert!(read(&c, w, "image/png").is_none());
    fs::remove_file(&path).unwrap();
    assert!(read(&c, w, "UTF8_STRING").is_none());
    snapshot(&path, &[("text/plain", b"expired")]);
    fs::File::open(&path)
        .unwrap()
        .set_times(
            fs::FileTimes::new()
                .set_modified(std::time::SystemTime::now() - std::time::Duration::from_secs(121)),
        )
        .unwrap();
    assert!(read(&c, w, "UTF8_STRING").is_none());
}

#[test]
fn rejects_unauthenticated_clients_and_clipboard_writes() {
    let dir = tempfile::tempdir().unwrap();
    let server = Server::start(&dir.path().join("missing"), None).unwrap();
    let socket = UnixStream::connect(format!("/tmp/.X11-unix/X{}", &server.display[1..])).unwrap();
    assert!(RustConnection::connect_to_stream(
        DefaultStream::from_unix_stream(socket).unwrap().0,
        0
    )
    .is_err());
    let c = connect(&server);
    let w = window(&c);
    assert!(c
        .set_selection_owner(w, atom(&c, "CLIPBOARD"), CURRENT_TIME)
        .unwrap()
        .check()
        .is_err());
    assert_eq!(
        c.get_selection_owner(atom(&c, "CLIPBOARD"))
            .unwrap()
            .reply()
            .unwrap()
            .owner,
        0
    );
    assert!(
        !c.query_extension(b"BIG-REQUESTS")
            .unwrap()
            .reply()
            .unwrap()
            .present
    );
    let path = format!("/tmp/.X11-unix/X{}", &server.display[1..]);
    let authority = server.authority().to_owned();
    drop(server);
    assert!(!Path::new(&path).exists());
    assert!(!authority.exists());
}

#[test]
fn property_type_mismatch_does_not_delete_data() {
    let dir = tempfile::tempdir().unwrap();
    let server = Server::start(&dir.path().join("missing"), None).unwrap();
    let c = connect(&server);
    let w = window(&c);
    let p = atom(&c, "TEST");
    c.change_property8(PropMode::REPLACE, w, p, AtomEnum::STRING, b"abcdef")
        .unwrap()
        .check()
        .unwrap();
    let mismatch = c
        .get_property(true, w, p, AtomEnum::ATOM, 0, 100)
        .unwrap()
        .reply()
        .unwrap();
    assert_eq!(mismatch.bytes_after, 6);
    assert!(mismatch.value.is_empty());
    let part = c
        .get_property(true, w, p, AtomEnum::STRING, 0, 1)
        .unwrap()
        .reply()
        .unwrap();
    assert_eq!(part.value, b"abcd");
    assert_eq!(part.bytes_after, 2);
    let end = c
        .get_property(true, w, p, AtomEnum::STRING, 1, 1)
        .unwrap()
        .reply()
        .unwrap();
    assert_eq!(end.value, b"ef");
    assert_eq!(
        c.get_property(false, w, p, AtomEnum::ANY, 0, 10)
            .unwrap()
            .reply()
            .unwrap()
            .type_,
        0
    );
}

// Runs against the actual arboard Linux backend on Linux; macOS uses NSPasteboard.
#[cfg(target_os = "linux")]
#[test]
fn unmodified_arboard_reads_text_and_png() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("snapshot.tar");
    // 1x1 transparent PNG, with a valid CRC and zlib stream.
    let png = include_bytes!("pixel.png");
    snapshot(
        &path,
        &[("text/plain", b"arboard bridge"), ("image/png", png)],
    );
    let server = Server::start(&path, None).unwrap();
    std::env::set_var("DISPLAY", &server.display);
    std::env::set_var("XAUTHORITY", server.authority());
    std::env::remove_var("WAYLAND_DISPLAY");
    let mut clipboard = arboard::Clipboard::new().unwrap();
    assert_eq!(clipboard.get_text().unwrap(), "arboard bridge");
    let image = clipboard.get_image().unwrap();
    assert_eq!((image.width, image.height), (1, 1));
    // Incompressible pixels force arboard's real INCR reader, not just our test client.
    use image::ImageEncoder;
    let mut seed = 123456789u32;
    let pixels: Vec<u8> = (0..256 * 256 * 4)
        .map(|_| {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            seed as u8
        })
        .collect();
    let mut png = Vec::new();
    image::codecs::png::PngEncoder::new(&mut png)
        .write_image(&pixels, 256, 256, image::ExtendedColorType::Rgba8)
        .unwrap();
    assert!(png.len() > 64 * 1024);
    snapshot(&path, &[("image/png", &png)]);
    let image = clipboard.get_image().unwrap();
    assert_eq!((image.width, image.height), (256, 256));
    assert_eq!(image.bytes.as_ref(), pixels);
    snapshot(&path, &[("text/plain", b"changed")]);
    assert_eq!(clipboard.get_text().unwrap(), "changed");
    fs::remove_file(path).unwrap();
    assert!(clipboard.get_image().is_err());
}

#[test]
fn wrapper_sets_environment_and_cleans_up() {
    let dir = tempfile::tempdir().unwrap();
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_porthop-agent")).args(["display", "--backend", "x11"])
        .args(["--snapshot", dir.path().join("snapshot.tar").to_str().unwrap(), "--", "sh", "-c",
            "test -S /tmp/.X11-unix/X${DISPLAY#:} && test -f \"$XAUTHORITY\" && test -z \"${WAYLAND_DISPLAY-}${WAYLAND_SOCKET-}\" || exit 2; printf '%s\\n%s\\n' \"$DISPLAY\" \"$XAUTHORITY\"; exit 7"])
        .env("WAYLAND_DISPLAY", "existing-display")
        .output().unwrap();
    assert_eq!(output.status.code(), Some(7));
    let text = String::from_utf8(output.stdout).unwrap();
    let lines: Vec<_> = text.lines().collect();
    assert!(!Path::new(&format!("/tmp/.X11-unix/X{}", &lines[0][1..])).exists());
    assert!(!Path::new(lines[1]).exists());
}

#[test]
fn big_endian_setup_and_malformed_request() {
    use std::io::Read;
    let dir = tempfile::tempdir().unwrap();
    let server = Server::start(&dir.path().join("snapshot.tar"), None).unwrap();
    let mut socket =
        UnixStream::connect(format!("/tmp/.X11-unix/X{}", &server.display[1..])).unwrap();
    socket
        .set_read_timeout(Some(std::time::Duration::from_secs(2)))
        .unwrap();
    let auth = fs::read(server.authority()).unwrap();
    let mut setup = vec![b'B', 0, 0, 11, 0, 0, 0, 18, 0, 16, 0, 0];
    setup.extend_from_slice(b"MIT-MAGIC-COOKIE-1\0\0");
    setup.extend_from_slice(&auth[auth.len() - 16..]);
    socket.write_all(&setup).unwrap();
    let mut reply = [0; 8];
    socket.read_exact(&mut reply).unwrap();
    assert_eq!(reply[0], 1);
    let mut rest = vec![0; u16::from_be_bytes([reply[6], reply[7]]) as usize * 4];
    socket.read_exact(&mut rest).unwrap();
    // InternAtom: 9-character CLIPBOARD, padded to 12 bytes.
    socket
        .write_all(&[
            16, 0, 0, 5, 0, 9, 0, 0, b'C', b'L', b'I', b'P', b'B', b'O', b'A', b'R', b'D', 0, 0, 0,
        ])
        .unwrap();
    let mut response = [0; 32];
    socket.read_exact(&mut response).unwrap();
    assert_eq!(&response[..4], &[1, 0, 0, 1]);
    assert!(u32::from_be_bytes(response[8..12].try_into().unwrap()) >= 69);
    socket.write_all(&[24, 0, 0, 1]).unwrap(); // Truncated ConvertSelection.
    socket.read_exact(&mut response).unwrap();
    assert_eq!(&response[..4], &[0, 16, 0, 2]);
    socket.write_all(&[127, 0, 0, 0]).unwrap(); // Unsupported extended request framing closes safely.
    let mut byte = [0];
    assert_eq!(socket.read(&mut byte).unwrap(), 0);
}

#[test]
fn xlib_default_graphics_context_lifecycle_is_bounded_and_validated() {
    let dir = tempfile::tempdir().unwrap();
    let server = Server::start(&dir.path().join("snapshot.tar"), None).unwrap();
    let c = connect(&server);
    let root = c.setup().roots[0].root;
    // Xlib also reads RESOURCE_MANAGER from the logical root during setup.
    let resources = c
        .get_property(
            false,
            root,
            AtomEnum::RESOURCE_MANAGER,
            AtomEnum::STRING,
            0,
            1024,
        )
        .unwrap()
        .reply()
        .unwrap();
    assert_eq!(resources.type_, 0);
    assert!(resources.value.is_empty());
    let gc = c.generate_id().unwrap();
    c.create_gc(
        gc,
        root,
        &CreateGCAux::new().foreground(0).background(0xffffff),
    )
    .unwrap()
    .check()
    .unwrap();
    c.change_gc(gc, &ChangeGCAux::new().foreground(1))
        .unwrap()
        .check()
        .unwrap();
    assert!(c
        .create_gc(gc, root, &CreateGCAux::new())
        .unwrap()
        .check()
        .is_err());
    let win = window(&c);
    assert!(c
        .create_gc(win, root, &CreateGCAux::new())
        .unwrap()
        .check()
        .is_err());
    let other = connect(&server);
    assert!(other.free_gc(gc).unwrap().check().is_err());
    c.free_gc(gc).unwrap().check().unwrap();
    assert!(c.free_gc(gc).unwrap().check().is_err());
    assert!(c
        .change_gc(gc, &ChangeGCAux::new())
        .unwrap()
        .check()
        .is_err());
    for _ in 0..64 {
        c.create_gc(c.generate_id().unwrap(), root, &CreateGCAux::new())
            .unwrap()
            .check()
            .unwrap();
    }
    assert!(c
        .create_gc(c.generate_id().unwrap(), root, &CreateGCAux::new())
        .unwrap()
        .check()
        .is_err());
}

#[cfg(target_os = "linux")]
#[test]
fn native_xclip_reads_formats_text_and_png_through_xlib() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("snapshot.tar");
    let text = b"native xclip INCR transfer\n".repeat(8192);
    let png = include_bytes!("pixel.png");
    snapshot(&path, &[("text/plain", &text), ("image/png", png)]);
    let server = Server::start(&path, None).unwrap();
    let binary = std::env::var("PORTHOP_TEST_XCLIP").unwrap_or_else(|_| "/usr/bin/xclip".into());
    let read = |target: &str| {
        let output = std::process::Command::new("timeout")
            .args(["8", &binary, "-selection", "clipboard", "-o", "-t", target])
            .env("DISPLAY", &server.display)
            .env("XAUTHORITY", server.authority())
            .output()
            .expect("Install xclip or set PORTHOP_TEST_XCLIP to the native executable");
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        output.stdout
    };
    let targets = String::from_utf8(read("TARGETS")).unwrap();
    assert!(targets.lines().any(|line| line == "image/png"));
    assert_eq!(read("UTF8_STRING"), text);
    assert_eq!(read("image/png"), png);
}
