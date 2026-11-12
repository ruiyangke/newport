use super::*;

#[test]
fn an_exec_bridge_flushes_binary_greeting_and_reply_while_the_holder_stays_open() {
    let (mut holder, mut input) = UnixStream::pair().unwrap();
    let (output, mut desktop) = UnixStream::pair().unwrap();
    input
        .set_read_timeout(Some(Duration::from_secs(3)))
        .unwrap();
    desktop
        .set_read_timeout(Some(Duration::from_secs(1)))
        .unwrap();
    let bridge =
        std::thread::spawn(move || copy_live(&mut input, &mut io::LineWriter::new(output)));
    let hello = protocol::wire::encode_server(&protocol::ServerMessage::Hello(protocol::Hello {
        version: protocol::VERSION,
        terminal_id: "0123456789abcdef0123456789abcdef".into(),
    }))
    .unwrap();
    assert_ne!(hello.last(), Some(&b'\n'));
    let attached =
        protocol::wire::encode_server(&protocol::ServerMessage::Attached { prompted: false })
            .unwrap();
    let transferred = (|| -> io::Result<()> {
        for message in [&hello, &attached] {
            holder.write_all(message)?;
            let mut received = vec![0; message.len()];
            desktop.read_exact(&mut received)?;
            assert_eq!(&received, message);
            assert!(!bridge.is_finished(), "the holder still owns its socket");
        }
        Ok(())
    })();
    // Closing the owned source also bounds cleanup when an assertion fails.
    drop(holder);
    let completed = bridge.join().unwrap();
    transferred.unwrap();
    assert_eq!(completed.unwrap(), (hello.len() + attached.len()) as u64);
}

#[test]
fn an_exec_bridge_propagates_a_flush_failure_after_copying_bytes() {
    struct FailsFlush(Vec<u8>);
    impl Write for FailsFlush {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.0.extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "fixture flush failure",
            ))
        }
    }
    let mut output = FailsFlush(Vec::new());
    let error = copy_live(&mut b"binary greeting".as_slice(), &mut output).unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::BrokenPipe);
    assert_eq!(error.to_string(), "fixture flush failure");
    assert_eq!(output.0, b"binary greeting");
}

#[test]
fn ids_are_dashless_hex_uuids() {
    let id = uuid::Uuid::new_v4().simple().to_string();
    assert_eq!(valid_id(&id).unwrap(), id);
    assert_eq!(valid_id(&id.to_uppercase()).unwrap(), id);
    for bad in [
        "",
        "../etc",
        &format!("{id}0"),
        &uuid::Uuid::new_v4().to_string(),
    ] {
        assert!(valid_id(bad).is_err(), "{bad}");
    }
}

#[test]
fn a_reserved_x11_display_is_free_and_kept() {
    use std::os::unix::fs::symlink;
    let root = tempfile::tempdir().unwrap();
    let folder = tempfile::tempdir().unwrap();
    // Taken: a server's socket, a lock, and a lock that is a broken link.
    fs::create_dir(root.path().join(".X11-unix")).unwrap();
    File::create(root.path().join(".X11-unix/X0")).unwrap();
    File::create(root.path().join(".X1-lock")).unwrap();
    symlink("/nonexistent", root.path().join(".X2-lock")).unwrap();
    assert_eq!(display::x11_display(folder.path(), root.path()).unwrap(), 3);
    assert_eq!(
        fs::read_to_string(folder.path().join("x11-display")).unwrap(),
        "3"
    );
    // Recorded, it holds even once taken: the display runs on it.
    File::create(root.path().join(".X3-lock")).unwrap();
    assert_eq!(display::x11_display(folder.path(), root.path()).unwrap(), 3);
    // A number the display recorded is taken as it is.
    fs::write(folder.path().join("x11-display"), "57\n").unwrap();
    assert_eq!(
        display::x11_display(folder.path(), root.path()).unwrap(),
        57
    );
}

#[test]
fn a_holder_is_alive_only_while_it_holds_its_lock() {
    let directory = tempfile::tempdir().unwrap();
    let files = Files {
        directory: directory.path().into(),
        id: uuid::Uuid::new_v4().simple().to_string(),
    };
    assert!(!files.alive());
    let lock = File::create(files.lock()).unwrap();
    assert!(!files.alive());
    assert_eq!(
        // SAFETY: flock takes a live descriptor and a scalar operation.
        unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) },
        0
    );
    // flock is per open file description: another open sees it held.
    assert!(files.alive());
    drop(lock);
    // A child another test is forking shares the description until it execs.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while files.alive() {
        assert!(
            std::time::Instant::now() < deadline,
            "the lock was released"
        );
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

/// Complete messages emitted by the watch.
struct Frames(std::sync::mpsc::Sender<Reply>, Vec<u8>);
impl Write for Frames {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.1.extend_from_slice(bytes);
        let mut input = std::io::Cursor::new(&self.1);
        if let Ok(reply) = wire::read_reply(&mut input) {
            let consumed = input.position() as usize;
            self.1.drain(..consumed);
            let _ = self.0.send(reply);
        }
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[test]
fn watch_reports_the_list_when_records_change_until_its_input_closes() {
    let directory = tempfile::tempdir().unwrap();
    let directories = vec![directory.path().canonicalize().unwrap()];
    let (lines, received) = std::sync::mpsc::channel();
    let (input, closer) = UnixStream::pair().unwrap();
    let watching = directories.clone();
    let watch = std::thread::spawn(move || {
        watch(
            &watching,
            input,
            Frames(lines, Vec::new()),
            &Cancellation::default(),
        )
    });
    let next = || received.recv_timeout(Duration::from_secs(10)).unwrap();
    assert_eq!(next(), Reply::Listed(Vec::new()));

    let files = Files {
        directory: directories[0].clone(),
        id: uuid::Uuid::new_v4().simple().to_string(),
    };
    let lock = File::create(files.lock()).unwrap();
    assert_eq!(
        // SAFETY: flock takes a live descriptor and a scalar operation.
        unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) },
        0
    );
    let record = |name: Option<&str>| {
        serde_json::json!({
            "version": 1, "pid": 1, "columns": 80, "rows": 24, "started": 5,
            "name": name, "creator": { "id": uuid::Uuid::new_v4(), "name": "Studio" },
            "attached": [{ "id": uuid::Uuid::new_v4(), "name": "Air", "active": true }], "exited": null,
        })
        .to_string()
    };
    fs::write(files.record(), record(None)).unwrap();
    let Reply::Listed(listed) = next() else {
        panic!("expected list")
    };
    assert_eq!(listed[0].id, files.id);
    assert_eq!(listed[0].creator.name, "Studio");
    assert!(listed[0].attached[0].active);

    fs::write(files.record(), record(Some("build"))).unwrap();
    let Reply::Listed(listed) = next() else {
        panic!("expected list")
    };
    assert_eq!(listed[0].name.as_deref(), Some("build"));

    drop(closer);
    watch.join().unwrap().unwrap();
}

#[test]
fn a_route_is_read_only_when_private_and_the_least_recently_used_go_first() {
    use std::os::unix::fs::PermissionsExt;
    let directory = tempfile::tempdir().unwrap();
    let routes = crate::paths::private_home_directory(directory.path(), ROUTES).unwrap();
    let mac = Computer {
        id: uuid::Uuid::new_v4().to_string(),
        name: "Studio".into(),
    };
    let ids: Vec<_> = (0..4)
        .map(|_| uuid::Uuid::new_v4().simple().to_string())
        .collect();
    let started = std::time::SystemTime::now();
    let age = |path: &Path, minutes: u64| {
        File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_modified(started - Duration::from_secs(60 * minutes))
            .unwrap();
    };
    for (index, id) in ids.iter().enumerate() {
        let path = routes.join(format!("{id}.json"));
        crate::paths::private_write(&path, &serde_json::to_vec(&mac).unwrap()).unwrap();
        age(&path, 10 - index as u64);
    }
    // A route in use, as a long-running tmux session's is, stays.
    assert_eq!(read_route(&routes, &ids[0]), Some(mac.clone()));
    // Writes that never finished leave temporary files; only old ones go.
    let orphan = routes.join(".tmpOrphan");
    let writing = routes.join(".tmpWriting");
    for path in [&orphan, &writing] {
        fs::write(path, "{").unwrap();
    }
    age(&orphan, 10);
    prune_routes(&routes, 2);
    assert!(!orphan.exists());
    assert!(writing.exists());
    let kept: Vec<_> = ids
        .iter()
        .map(|id| routes.join(format!("{id}.json")).exists())
        .collect();
    assert_eq!(kept, [true, false, false, true]);
    // One another account could change is no route.
    let path = routes.join(format!("{}.json", ids[3]));
    fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
    assert_eq!(read_route(&routes, &ids[3]), None);
}

#[test]
fn a_route_names_a_terminal_and_a_computer_as_newport_does() {
    let terminal = uuid::Uuid::new_v4().to_string();
    let mac = |id: &str, name: &str| Computer {
        id: id.into(),
        name: name.into(),
    };
    let id = uuid::Uuid::new_v4().to_string();
    assert!(route("../x", mac(&id, "Studio")).is_err());
    assert!(route(&terminal, mac("studio", "Studio")).is_err());
    assert!(route(&terminal, mac(&id, "Studio\n")).is_err());
    // A name of any length passes through hex, however many bytes its
    // characters take.
    let wide: String = "工作室\u{1F5A5}"
        .chars()
        .cycle()
        .take(protocol::MAX_NAME + 1)
        .collect();
    let hex: String = wide.bytes().map(|b| format!("{b:02x}")).collect();
    assert_eq!(name_from_hex(&hex).unwrap(), wide);
    assert_eq!(name_from_hex(&hex.to_uppercase()).unwrap(), wide);
    for invalid in ["4", "+f", "zz", "ff"] {
        assert!(name_from_hex(invalid).is_err(), "{invalid}");
    }
    // A longer name than holders take is cut as they cut it, never refused.
    let terminals = tempfile::tempdir().unwrap();
    let simple = uuid::Uuid::parse_str(&terminal)
        .unwrap()
        .simple()
        .to_string();
    route_in(terminals.path(), &simple, &mac(&id, &wide)).unwrap();
    let cut: String = wide.chars().take(protocol::MAX_NAME).collect();
    assert_eq!(
        read_route(&terminals.path().join(ROUTES), &simple),
        Some(mac(&id, &cut))
    );
}

#[test]
fn a_holder_without_a_supported_greeting_is_not_sent_control_bytes() {
    use std::os::unix::net::UnixListener;
    let root = tempfile::tempdir().unwrap();
    let files = Files {
        directory: root.path().into(),
        id: "0123456789abcdef0123456789abcdef".into(),
    };
    let listener = UnixListener::bind(files.socket()).unwrap();
    let peer = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(4)))
            .unwrap();
        let mut bytes = Vec::new();
        stream.read_to_end(&mut bytes).unwrap();
        assert!(bytes.is_empty());
    });
    let error = connect_holder(&files).expect_err("unsupported holder refused");
    assert!(
        matches!(holder_failure(error),Reply::Failed(error) if error.code=="holder_greeting_incomplete" && error.retry==newport_protocol::common::Retry::No)
    );
    peer.join().unwrap();
}

#[test]
fn a_start_that_launched_nothing_says_so_with_its_reason() {
    let reply = holder_failure(not_started(io::Error::other(
        "Needle directory is not safely owned by this account: /home/a/terminals",
    )));
    let Reply::Failed(error) = reply else {
        panic!("a failure, not {reply:?}");
    };
    assert_eq!(error.code, newport_protocol::terminal::NOT_STARTED);
    assert_eq!(error.retry, newport_protocol::common::Retry::No);
    assert_eq!(
        error.message,
        "Needle directory is not safely owned by this account: /home/a/terminals"
    );
}

#[test]
fn a_live_holder_lock_reserves_its_identity_without_a_socket() {
    let root = tempfile::tempdir().unwrap();
    let id = uuid::Uuid::new_v4().simple().to_string();
    let files = Files {
        directory: root.path().into(),
        id: id.clone(),
    };
    let lock = File::create(files.lock()).unwrap();
    assert_eq!(
        // SAFETY: flock takes a live descriptor and a scalar operation.
        unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) },
        0
    );
    assert!(!files.socket().exists());
    assert!(occupied_in(&id, [root.path().into()]).is_some());
    // SAFETY: flock takes a live descriptor and a scalar operation.
    assert_eq!(unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_UN) }, 0);
    assert!(occupied_in(&id, [root.path().into()]).is_none());
    assert!(!files.start().exists());
}
#[test]
fn terminal_control_completion_requires_a_correlated_outcome_and_clean_eof() {
    for (name, messages, accepted) in [
        (
            None,
            vec![protocol::ServerMessage::Exit(Exit::Code(0))],
            true,
        ),
        (
            Some("build"),
            vec![protocol::ServerMessage::Renamed("build".into())],
            true,
        ),
        (
            Some("build"),
            vec![protocol::ServerMessage::Renamed("other".into())],
            false,
        ),
        (
            Some("build"),
            vec![protocol::ServerMessage::Exit(Exit::Code(0))],
            false,
        ),
        (None, vec![protocol::ServerMessage::Active(true)], false),
        (None, vec![], false),
        (Some("build"), vec![], false),
        (
            None,
            vec![
                protocol::ServerMessage::Exit(Exit::Code(0)),
                protocol::ServerMessage::Exit(Exit::Code(0)),
            ],
            false,
        ),
    ] {
        let (mut client, mut server) = UnixStream::pair().unwrap();
        for message in messages {
            protocol::wire::write_server(&mut server, &message).unwrap();
        }
        server.shutdown(std::net::Shutdown::Write).unwrap();
        assert_eq!(
            control_completed_until(&mut client, name, Instant::now() + Duration::from_secs(1))
                .is_ok(),
            accepted
        );
    }
    let (mut client, mut server) = UnixStream::pair().unwrap();
    server.write_all(&[0x80]).unwrap();
    let error = control_completed_until(
        &mut client,
        None,
        Instant::now() + Duration::from_millis(30),
    )
    .unwrap_err();
    assert!(matches!(
        error.kind(),
        io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock
    ));
}
#[test]
fn an_unconfirmed_mutation_retains_the_cause_and_unknown_outcome() {
    #[derive(Debug)]
    struct Cause;
    impl std::fmt::Display for Cause {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("peer reset")
        }
    }
    impl std::error::Error for Cause {}
    let error = outcome_failed(io::Error::new(io::ErrorKind::ConnectionReset, Cause));
    assert_eq!(error.kind(), io::ErrorKind::ConnectionReset);
    let cause = error
        .get_ref()
        .unwrap()
        .downcast_ref::<HolderOutcome>()
        .unwrap();
    assert!(cause.0.get_ref().unwrap().is::<Cause>());
    assert!(
        matches!(holder_failure(error),Reply::Failed(error) if error.code=="holder_outcome_unknown" && error.retry==newport_protocol::common::Retry::Unknown)
    );
}
