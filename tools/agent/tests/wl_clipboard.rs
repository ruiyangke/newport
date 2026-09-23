use porthop_agent::wl_server::Server;
use std::{
    fs,
    io::Read,
    os::{
        fd::AsFd,
        unix::{fs::PermissionsExt, net::UnixStream},
    },
    path::Path,
    sync::Mutex,
    time::{Duration, Instant},
};
use wayland_client::{
    protocol::{wl_registry, wl_seat},
    Connection, Dispatch, EventQueue, QueueHandle,
};
use wayland_protocols_wlr::data_control::v1::client::{
    zwlr_data_control_device_v1::{self as device, ZwlrDataControlDeviceV1 as Device},
    zwlr_data_control_manager_v1::ZwlrDataControlManagerV1 as Manager,
    zwlr_data_control_offer_v1::{self as offer, ZwlrDataControlOfferV1 as Offer},
};

fn snapshot(path: &Path, entries: &[(&str, &[u8])]) {
    let tmp = path.with_extension("new");
    let mut tar = tar::Builder::new(fs::File::create(&tmp).unwrap());
    for (mime, data) in entries {
        let mut header = tar::Header::new_gnu();
        header.set_size(data.len() as u64);
        header.set_mode(0o600);
        header.set_cksum();
        tar.append_data(&mut header, mime, *data).unwrap();
    }
    tar.finish().unwrap();
    drop(tar);
    fs::rename(tmp, path).unwrap();
}

#[derive(Default)]
struct Probe {
    manager: Option<Manager>,
    seat: Option<wl_seat::WlSeat>,
    selected: Option<Offer>,
    changes: usize,
    mime_types: Vec<String>,
}
impl Dispatch<wl_registry::WlRegistry, ()> for Probe {
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
impl Dispatch<wl_seat::WlSeat, ()> for Probe {
    fn event(
        _: &mut Self,
        _: &wl_seat::WlSeat,
        _: wl_seat::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}
impl Dispatch<Manager, ()> for Probe {
    fn event(
        _: &mut Self,
        _: &Manager,
        _: <Manager as wayland_client::Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}
impl Dispatch<Device, ()> for Probe {
    fn event(
        state: &mut Self,
        _: &Device,
        event: device::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let device::Event::Selection { id } = event {
            state.selected = id;
            state.changes += 1;
        }
    }
    wayland_client::event_created_child!(Probe, Device, [0 => (Offer, ())]);
}
impl Dispatch<Offer, ()> for Probe {
    fn event(
        state: &mut Self,
        _: &Offer,
        event: offer::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let offer::Event::Offer { mime_type } = event {
            state.mime_types.push(mime_type);
        }
    }
}
fn connect(server: &Server) -> (Connection, EventQueue<Probe>, Probe, Device) {
    let conn = Connection::from_socket(UnixStream::connect(server.socket()).unwrap()).unwrap();
    let mut q = conn.new_event_queue();
    let mut state = Probe::default();
    let _registry = conn.display().get_registry(&q.handle(), ());
    q.roundtrip(&mut state).unwrap();
    let device = state.manager.as_ref().unwrap().get_data_device(
        state.seat.as_ref().unwrap(),
        &q.handle(),
        (),
    );
    q.roundtrip(&mut state).unwrap();
    (conn, q, state, device)
}
fn read(offer: &Offer, mime: &str, q: &mut EventQueue<Probe>, state: &mut Probe) -> Vec<u8> {
    let (mut reader, writer) = UnixStream::pair().unwrap();
    reader
        .set_read_timeout(Some(Duration::from_secs(3)))
        .unwrap();
    offer.receive(mime.into(), writer.as_fd());
    drop(writer);
    q.roundtrip(state).unwrap();
    let mut bytes = Vec::new();
    reader.read_to_end(&mut bytes).unwrap();
    bytes
}
fn wait_for_change(q: &mut EventQueue<Probe>, state: &mut Probe, previous: usize) {
    let started = Instant::now();
    while state.changes == previous {
        assert!(
            started.elapsed() < Duration::from_secs(3),
            "no selection update"
        );
        q.roundtrip(state).unwrap();
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn transfers_formats_notifies_changes_and_rejects_stale_offers() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("snapshot.tar");
    let image: Vec<u8> = (0..300_123).map(|i| (i % 251) as u8).collect();
    snapshot(&path, &[("text/plain", b"hello"), ("image/png", &image)]);
    let server = Server::start(&path).unwrap();
    assert_eq!(
        fs::metadata(server.socket()).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert_eq!(
        fs::metadata(server.socket().parent().unwrap())
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o700
    );
    let (_conn, mut q, mut state, _device) = connect(&server);
    assert!(state
        .mime_types
        .contains(&"text/plain;charset=utf-8".into()));
    let original = state.selected.clone().unwrap();
    assert_eq!(
        read(&original, "text/plain;charset=utf-8", &mut q, &mut state),
        b"hello"
    );
    assert_eq!(read(&original, "image/png", &mut q, &mut state), image);
    assert!(read(&original, "unknown/format", &mut q, &mut state).is_empty());

    let changes = state.changes;
    snapshot(&path, &[("text/plain", b"updated")]);
    wait_for_change(&mut q, &mut state, changes);
    assert!(read(&original, "text/plain", &mut q, &mut state).is_empty());
    let updated = state.selected.clone().unwrap();
    assert_eq!(read(&updated, "text/plain", &mut q, &mut state), b"updated");

    let changes = state.changes;
    fs::File::open(&path)
        .unwrap()
        .set_times(
            fs::FileTimes::new()
                .set_modified(std::time::SystemTime::now() - Duration::from_secs(121)),
        )
        .unwrap();
    wait_for_change(&mut q, &mut state, changes);
    assert!(state.selected.is_none());
    assert!(read(&updated, "text/plain", &mut q, &mut state).is_empty());

    let changes = state.changes;
    snapshot(&path, &[("text/plain", b"resumed")]);
    wait_for_change(&mut q, &mut state, changes);
    let changes = state.changes;
    fs::remove_file(path).unwrap();
    wait_for_change(&mut q, &mut state, changes);
    assert!(state.selected.is_none());
    let socket = server.socket();
    drop(server);
    assert!(!socket.exists());
}

#[test]
fn refuses_writes_without_changing_snapshot() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("snapshot.tar");
    snapshot(&path, &[("text/plain", b"original")]);
    let bytes = fs::read(&path).unwrap();
    let server = Server::start(&path).unwrap();
    let (_conn, mut q, mut state, device) = connect(&server);
    device.set_selection(None);
    assert!(q.roundtrip(&mut state).is_err());
    assert_eq!(fs::read(path).unwrap(), bytes);
}

#[test]
fn stalled_transfer_does_not_block_other_clients_or_shutdown() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("snapshot.tar");
    snapshot(
        &path,
        &[
            ("image/png", &vec![42; 4 * 1024 * 1024]),
            ("text/plain", b"responsive"),
        ],
    );
    let server = Server::start(&path).unwrap();
    let (_conn, mut queue, mut probe, _device) = connect(&server);
    let (mut reader, writer) = UnixStream::pair().unwrap();
    probe
        .selected
        .as_ref()
        .unwrap()
        .receive("image/png".into(), writer.as_fd());
    drop(writer);
    queue.roundtrip(&mut probe).unwrap();
    // Do not read the image pipe: its writer must not block protocol dispatch.
    let (_other, mut q, mut state, _device) = connect(&server);
    let offer = state.selected.clone().unwrap();
    assert_eq!(
        read(&offer, "text/plain", &mut q, &mut state),
        b"responsive"
    );
    let started = Instant::now();
    drop(server);
    assert!(started.elapsed() < Duration::from_secs(1));
    reader
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    let mut remaining = Vec::new();
    reader.read_to_end(&mut remaining).unwrap();
}

#[test]
fn standalone_termination_removes_private_socket() {
    use std::io::BufRead;
    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_porthop-agent"))
        .args(["display", "--backend", "wayland"])
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let mut output = std::io::BufReader::new(child.stdout.take().unwrap());
    let mut line = String::new();
    output.read_line(&mut line).unwrap();
    let path = line
        .trim()
        .strip_prefix("export WAYLAND_DISPLAY='")
        .unwrap()
        .strip_suffix('\'')
        .unwrap();
    assert!(Path::new(path).exists());
    assert_eq!(unsafe { libc::kill(child.id() as i32, libc::SIGTERM) }, 0);
    let started = Instant::now();
    while child.try_wait().unwrap().is_none() {
        if started.elapsed() > Duration::from_secs(3) {
            let _ = child.kill();
            panic!("bridge did not stop");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(!Path::new(path).exists());
    assert!(!Path::new(path).parent().unwrap().exists());
}

// Only these tests change environment, serialized within this integration-test process.
static ENVIRONMENT: Mutex<()> = Mutex::new(());
struct Environment(Vec<(&'static str, Option<std::ffi::OsString>)>);
impl Environment {
    fn new(server: &Server) -> Self {
        let old = ["WAYLAND_DISPLAY", "WAYLAND_SOCKET", "DISPLAY"]
            .map(|k| (k, std::env::var_os(k)))
            .to_vec();
        std::env::set_var("WAYLAND_DISPLAY", server.socket());
        std::env::remove_var("WAYLAND_SOCKET");
        std::env::remove_var("DISPLAY");
        Self(old)
    }
}
impl Drop for Environment {
    fn drop(&mut self) {
        for (k, v) in &self.0 {
            if let Some(v) = v {
                std::env::set_var(k, v);
            } else {
                std::env::remove_var(k);
            }
        }
    }
}
#[test]
fn wl_clipboard_library_reads_text_and_large_images() {
    let _lock = ENVIRONMENT.lock().unwrap();
    use wl_clipboard_rs::{
        paste::{self, ClipboardType, MimeType, Seat},
        utils,
    };
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("snapshot.tar");
    let image = vec![123; 300_000];
    snapshot(&path, &[("text/plain", b"library"), ("image/png", &image)]);
    let server = Server::start(&path).unwrap();
    let _env = Environment::new(&server);
    assert!(!utils::is_primary_selection_supported().unwrap());
    let formats = paste::get_mime_types(ClipboardType::Regular, Seat::Unspecified).unwrap();
    assert!(formats.contains("image/png"));
    for (mime, expected) in [
        (MimeType::Text, b"library".as_slice()),
        (MimeType::Specific("image/png"), image.as_slice()),
    ] {
        let (mut pipe, _) =
            paste::get_contents(ClipboardType::Regular, Seat::Unspecified, mime).unwrap();
        let mut bytes = Vec::new();
        pipe.read_to_end(&mut bytes).unwrap();
        assert_eq!(bytes, expected);
    }
    fs::remove_file(path).unwrap();
    assert!(matches!(
        paste::get_contents(ClipboardType::Regular, Seat::Unspecified, MimeType::Text),
        Err(paste::Error::ClipboardEmpty)
    ));
}

#[test]
fn wrapper_sets_wayland_environment_and_cleans_up() {
    let output=std::process::Command::new(env!("CARGO_BIN_EXE_porthop-agent")).args(["display", "--backend", "wayland"])
        .args(["--", "sh", "-c", "test -S \"$WAYLAND_DISPLAY\" && test -z \"${DISPLAY-}${XAUTHORITY-}${WAYLAND_SOCKET-}\" || exit 2; printf '%s\\n' \"$WAYLAND_DISPLAY\"; exit 7"])
        .env("DISPLAY", ":99").env("XAUTHORITY", "/existing").env("WAYLAND_SOCKET", "17").output().unwrap();
    assert_eq!(output.status.code(), Some(7));
    let socket = String::from_utf8(output.stdout).unwrap();
    assert!(!Path::new(socket.trim()).exists());
}

#[test]
fn fixed_socket_serves_without_wrapper_and_blocks_duplicate_start() {
    let _lock = ENVIRONMENT.lock().unwrap();
    let tmp = tempfile::Builder::new()
        .prefix("wl-fixed-")
        .tempdir_in("/tmp")
        .unwrap();
    let path = tmp.path().join("snapshot.tar");
    let socket = tmp.path().join("private/wayland.sock");
    snapshot(&path, &[("text/plain", b"no wrapper")]);
    let server = Server::start_fixed(&path, &socket).unwrap();
    assert_eq!(server.socket(), socket);
    assert!(
        matches!(Server::start_fixed(&path, &socket), Err(e) if e.kind()==std::io::ErrorKind::AddrInUse)
    );
    let _env = Environment::new(&server);
    use wl_clipboard_rs::paste::{get_contents, ClipboardType, MimeType, Seat};
    let (mut pipe, _) =
        get_contents(ClipboardType::Regular, Seat::Unspecified, MimeType::Text).unwrap();
    let mut text = String::new();
    pipe.read_to_string(&mut text).unwrap();
    assert_eq!(text, "no wrapper");
    drop(server);
    assert!(!socket.exists());
    // The lock inode stays put; deleting it could split the lock between two processes.
    assert!(socket.with_file_name("wayland.sock.lock").exists());
    let restarted = Server::start_fixed(&path, &socket).unwrap();
    drop(restarted);
}

#[test]
fn fixed_socket_preserves_live_listener_and_unrelated_files() {
    use std::os::unix::{fs::symlink, net::UnixListener};
    let tmp = tempfile::Builder::new()
        .prefix("wl-safe-")
        .permissions(fs::Permissions::from_mode(0o700))
        .tempdir_in("/tmp")
        .unwrap();
    let path = tmp.path().join("snapshot.tar");
    let socket = tmp.path().join("wayland.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    assert!(Server::start_fixed(&path, &socket).is_err());
    assert!(UnixStream::connect(&socket).is_ok());
    listener.set_nonblocking(true).unwrap();
    while let Ok((pending, _)) = listener.accept() {
        drop(pending);
    }
    drop(listener);
    // A closed listener leaves a stale socket; only that case may be replaced.
    let server = Server::start_fixed(&path, &socket).unwrap();
    drop(server);
    fs::write(&socket, b"keep this file").unwrap();
    assert!(Server::start_fixed(&path, &socket).is_err());
    assert_eq!(fs::read(&socket).unwrap(), b"keep this file");
    fs::remove_file(&socket).unwrap();
    symlink(&path, &socket).unwrap();
    assert!(Server::start_fixed(&path, &socket).is_err());
    assert!(fs::symlink_metadata(&socket)
        .unwrap()
        .file_type()
        .is_symlink());
    fs::remove_file(&socket).unwrap();
    let lock = socket.with_file_name("wayland.sock.lock");
    fs::remove_file(&lock).unwrap();
    symlink(&path, &lock).unwrap();
    assert!(Server::start_fixed(&path, &socket).is_err());
    fs::remove_file(&lock).unwrap();
    fs::set_permissions(tmp.path(), fs::Permissions::from_mode(0o755)).unwrap();
    assert!(Server::start_fixed(&path, &socket).is_err());
    assert_eq!(
        fs::metadata(tmp.path()).unwrap().permissions().mode() & 0o777,
        0o755
    );
}

#[test]
fn fixed_socket_survives_crash_and_restarts_at_default_path() {
    use std::{
        io::BufRead,
        process::{Command, Stdio},
    };
    let home = tempfile::Builder::new()
        .prefix("wl-home-")
        .permissions(fs::Permissions::from_mode(0o700))
        .tempdir_in("/tmp")
        .unwrap();
    let socket = home.path().join(".cache/porthop/clipboard/wayland.sock");
    let mut child = Command::new(env!("CARGO_BIN_EXE_porthop-agent"))
        .args(["display", "--backend", "wayland"])
        .arg("--service")
        .env("HOME", home.path())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut output = std::io::BufReader::new(child.stdout.take().unwrap());
    let mut line = String::new();
    output.read_line(&mut line).unwrap();
    assert!(line.contains(socket.to_str().unwrap()));
    assert!(socket.exists());
    child.kill().unwrap();
    child.wait().unwrap();
    assert!(
        socket.exists(),
        "SIGKILL must exercise stale-socket recovery"
    );
    let path = socket.with_file_name("snapshot.tar");
    snapshot(&path, &[("text/plain", b"recovered")]);
    let server = Server::start_fixed(&path, &socket).unwrap();
    let (_conn, mut q, mut probe, _device) = connect(&server);
    let offer = probe.selected.clone().unwrap();
    assert_eq!(read(&offer, "text/plain", &mut q, &mut probe), b"recovered");
    drop(server);
    assert!(!socket.exists());
}

#[test]
fn fixed_socket_concurrent_starts_have_one_owner() {
    let tmp = tempfile::Builder::new()
        .prefix("wl-race-")
        .tempdir_in("/tmp")
        .unwrap();
    let path = tmp.path().join("snapshot.tar");
    let socket = tmp.path().join("private/wayland.sock");
    let barrier = std::sync::Barrier::new(2);
    std::thread::scope(|scope| {
        let a = scope.spawn(|| {
            barrier.wait();
            Server::start_fixed(&path, &socket)
        });
        let b = scope.spawn(|| {
            barrier.wait();
            Server::start_fixed(&path, &socket)
        });
        let a = a.join().unwrap();
        let b = b.join().unwrap();
        assert_eq!(usize::from(a.is_ok()) + usize::from(b.is_ok()), 1);
        assert!(socket.exists());
    });
    assert!(!socket.exists());
}

#[cfg(target_os = "linux")]
#[test]
fn arboard_wayland_reads_text_and_png() {
    let _lock = ENVIRONMENT.lock().unwrap();
    let tmp = tempfile::Builder::new()
        .prefix("wl-arboard-")
        .tempdir_in("/tmp")
        .unwrap();
    let path = tmp.path().join("snapshot.tar");
    snapshot(
        &path,
        &[
            ("text/plain", b"arboard wayland"),
            ("image/png", include_bytes!("pixel.png")),
        ],
    );
    let server = Server::start_fixed(&path, &tmp.path().join("private/wayland.sock")).unwrap();
    let _env = Environment::new(&server);
    let mut clipboard = arboard::Clipboard::new().unwrap();
    assert_eq!(clipboard.get_text().unwrap(), "arboard wayland");
    let image = clipboard.get_image().unwrap();
    assert_eq!((image.width, image.height), (1, 1));
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
    assert_eq!(image.bytes.as_ref(), pixels);
    fs::remove_file(path).unwrap();
    assert!(clipboard.get_image().is_err());
}
