use super::*;
use std::{
    os::unix::{ffi::OsStringExt, net::UnixListener},
    thread,
};

#[test]
fn feature_read_failures_are_not_legacy_negotiation() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("features");
    assert!(read_features(directory.path()).is_err());
    fs::write(&path, "gui-launch").unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    assert_eq!(read_features(directory.path()).unwrap(), "gui-launch");
    fs::write(&path, vec![b' '; 4097]).unwrap();
    assert!(read_features(directory.path()).is_err());
    fs::write(&path, "gui-launch").unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
    assert!(read_features(directory.path()).is_err());
    fs::remove_file(&path).unwrap();
    symlink(directory.path().join("missing"), &path).unwrap();
    assert!(read_features(directory.path()).is_err());
}

fn values() -> BTreeMap<&'static str, OsString> {
    [
        ("NEWPORT_SESSION", Uuid::new_v4().to_string()),
        ("NEWPORT_TERMINAL", Uuid::new_v4().to_string()),
        ("NEWPORT_TERMINAL_EPOCH", Uuid::new_v4().to_string()),
        ("DISPLAY", ":75".into()),
        ("NEWPORT_GUI_DISPLAY", ":75".into()),
        ("XAUTHORITY", "/tmp/owned/Xauthority".into()),
        ("NEWPORT_GUI_AUTHORITY", "/tmp/owned/Xauthority".into()),
        ("WAYLAND_DISPLAY", "wayland-0".into()),
        ("NEWPORT_GUI_WAYLAND_DISPLAY", "wayland-0".into()),
        ("XDG_RUNTIME_DIR", "/tmp/owned".into()),
        ("NEWPORT_GUI_RUNTIME_DIR", "/tmp/owned".into()),
    ]
    .into_iter()
    .map(|(k, v)| (k, v.into()))
    .collect()
}
fn claim(values: &BTreeMap<&'static str, OsString>) -> Claim {
    Claim::capture(|name| values.get(name).cloned())
}
#[test]
fn original_identity_exact_pairs_and_relative_wayland_are_not_reinterpreted() {
    let mut values = values();
    let snapshot = claim(&values);
    let first = snapshot.request().unwrap();
    values.insert("NEWPORT_SESSION", Uuid::new_v4().to_string().into());
    assert_eq!(snapshot.request().unwrap().session, first.session);
    assert_ne!(claim(&values).request().unwrap().session, first.session);
    assert_eq!(first.endpoints.wayland_socket, "/tmp/owned/wayland-0");
    for key in [
        "DISPLAY",
        "XAUTHORITY",
        "NEWPORT_GUI_DISPLAY",
        "NEWPORT_GUI_AUTHORITY",
        "WAYLAND_DISPLAY",
        "NEWPORT_GUI_WAYLAND_DISPLAY",
        "XDG_RUNTIME_DIR",
        "NEWPORT_GUI_RUNTIME_DIR",
    ] {
        let mut changed = values.clone();
        changed.insert(key, "/foreign/value".into());
        assert!(claim(&changed).request().is_err(), "{key}");
    }
    for value in ["", "99"] {
        let mut changed = values.clone();
        changed.insert("WAYLAND_SOCKET", value.into());
        assert!(claim(&changed).request().is_err());
    }
    for name in [
        "NEWPORT_SESSION",
        "NEWPORT_TERMINAL",
        "NEWPORT_TERMINAL_EPOCH",
    ] {
        for value in [
            OsString::new(),
            "invalid".into(),
            Uuid::nil().to_string().into(),
            OsString::from_vec(vec![255]),
        ] {
            let mut changed = values.clone();
            changed.insert(name, value);
            assert!(claim(&changed).request().is_err(), "{name}");
        }
    }
    for name in [
        "DISPLAY",
        "XAUTHORITY",
        "NEWPORT_GUI_DISPLAY",
        "NEWPORT_GUI_AUTHORITY",
    ] {
        values.remove(name);
    }
    let native = claim(&values).request().unwrap();
    assert_eq!(
        (native.endpoints.display, native.endpoints.authority),
        (None, None)
    );
    values.insert("DISPLAY", OsString::new());
    assert!(claim(&values).request().is_err(), "empty is not absent");
    values.remove("DISPLAY");
    values.insert("WAYLAND_DISPLAY", "/tmp/owned/socket".into());
    values.insert("NEWPORT_GUI_WAYLAND_DISPLAY", "/tmp/owned/socket".into());
    values.remove("XDG_RUNTIME_DIR");
    values.remove("NEWPORT_GUI_RUNTIME_DIR");
    assert_eq!(
        claim(&values).request().unwrap().endpoints.wayland_socket,
        "/tmp/owned/socket"
    );
}

fn with_peer(
    action: impl FnOnce(UnixStream, protocol::Check) + Send + 'static,
    timeout: Duration,
) -> io::Result<protocol::Checked> {
    let directory = tempfile::tempdir_in("/tmp").unwrap();
    let path = directory.path().join("gui.sock");
    let listener = UnixListener::bind(&path).unwrap();
    let request = claim(&values()).request().unwrap();
    let expected = request.clone();
    let peer = thread::spawn(move || {
        let (mut socket, _) = listener.accept().unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let (kind, bytes) = wire::read_limited(&mut socket, protocol::MAX_MESSAGE).unwrap();
        assert_eq!(kind, protocol::REQUEST);
        let got: protocol::Check = protocol::decode(&bytes).unwrap();
        assert_eq!(got, expected);
        action(socket, got);
    });
    let result = exchange(&path, &request, Instant::now() + timeout);
    peer.join().unwrap();
    result
}
#[test]
fn fragmented_replies_preserve_exact_identity_and_refuse_malformed_frames() {
    let current = with_peer(
        |mut socket, request| {
            let frame = wire::encode(
                protocol::REPLY,
                &protocol::encode(&protocol::Checked::new(
                    &request,
                    protocol::Outcome::Current {},
                ))
                .unwrap(),
            )
            .unwrap();
            for part in frame.chunks(3) {
                socket.write_all(part).unwrap();
            }
        },
        Duration::from_secs(2),
    )
    .unwrap();
    assert_eq!(current.outcome, protocol::Outcome::Current {});
    for mode in 0..8 {
        let result = with_peer(
            move |mut socket, request| {
                let mut reply = protocol::Checked::new(&request, protocol::Outcome::Current {});
                match mode {
                    0 => reply.id = Uuid::new_v4(),
                    1 => reply.session = Uuid::new_v4(),
                    2 => reply.terminal = Uuid::new_v4(),
                    3 => reply.epoch = Uuid::new_v4(),
                    _ => (),
                }
                let data = if mode == 6 {
                    vec![b' '; protocol::MAX_MESSAGE + 1]
                } else if mode == 7 {
                    br#"{"unknown":true}"#.to_vec()
                } else {
                    protocol::encode(&reply).unwrap()
                };
                let frame =
                    wire::encode(if mode == 4 { b'j' } else { protocol::REPLY }, &data).unwrap();
                let _ = socket.write_all(if mode == 5 { &frame[..7] } else { &frame });
            },
            Duration::from_secs(2),
        );
        assert!(result.is_err(), "mode={mode}");
    }
}
#[test]
fn absolute_deadline_fences_silent_and_trickling_peers() {
    for trickle in [false, true] {
        let before = Instant::now();
        let error = with_peer(
            move |mut socket, request| {
                let frame = wire::encode(
                    protocol::REPLY,
                    &protocol::encode(&protocol::Checked::new(
                        &request,
                        protocol::Outcome::Current {},
                    ))
                    .unwrap(),
                )
                .unwrap();
                for byte in frame.into_iter().take(12) {
                    thread::sleep(Duration::from_millis(15));
                    if trickle && socket.write_all(&[byte]).is_err() {
                        break;
                    }
                }
            },
            Duration::from_millis(70),
        )
        .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
        assert!(before.elapsed() < Duration::from_secs(1));
    }
    // Expired work does not even connect, and thus cannot send/replay a request.
    let directory = tempfile::tempdir_in("/tmp").unwrap();
    let path = directory.path().join("gui.sock");
    let listener = UnixListener::bind(&path).unwrap();
    listener.set_nonblocking(true).unwrap();
    assert_eq!(
        exchange(&path, &claim(&values()).request().unwrap(), Instant::now())
            .unwrap_err()
            .kind(),
        io::ErrorKind::TimedOut
    );
    assert_eq!(
        listener.accept().unwrap_err().kind(),
        io::ErrorKind::WouldBlock
    );
}
