//! Real composed runtime + integration broker + waiting command. These test
//! protocol adoption without installing an agent or changing a native display.
use super::{BIN, Runtime, Worker, fixture_home};
use newport_agent::{gui_protocol as gui, integration_control, wire};
use std::{
    fs,
    os::unix::{fs::PermissionsExt, net::UnixListener},
    process::{Child, Command, Output, Stdio},
    sync::mpsc,
    time::{Duration, Instant},
};
use uuid::Uuid;

struct Fixture {
    worker: Worker,
    runtime: Runtime,
    events: mpsc::Receiver<(u8, Vec<u8>)>,
    terminal: Uuid,
    home: tempfile::TempDir,
}
impl Fixture {
    fn new() -> Self {
        let home = fixture_home();
        let mut runtime = Runtime::start(home.path(), "");
        runtime.hello.require("gui-launch", 1).unwrap();
        runtime.hello.require("gui-launch", 2).unwrap();
        runtime.initialize();
        let registration = runtime.register("integration", integration_control::VERSION);
        let mut worker = runtime.worker(&registration, "integration", home.path());
        let mut output = worker.stdout.take().unwrap();
        let (send, events) = mpsc::sync_channel(64);
        std::thread::spawn(move || {
            while let Ok(event) = wire::read(&mut output) {
                if send.send(event).is_err() {
                    break;
                }
            }
        });
        let mut fixture = Self {
            worker,
            runtime,
            events,
            terminal: Uuid::new_v4(),
            home,
        };
        fixture.event(b'R');
        fixture.send(b'L', &[]);
        fixture.event(b'A');
        let root = fixture.runtime.integration_root(fixture.home.path());
        assert!(
            !fs::read_to_string(root.join("features"))
                .unwrap()
                .contains("gui-launch-v2")
        );
        fs::create_dir(fixture.home.path().join("working directory")).unwrap();
        let program = fixture.home.path().join("gui-fixture");
        fs::write(&program, r#"#!/bin/sh
printf '%s\0' "$PWD" "${DISPLAY-unset}" "${XAUTHORITY-unset}" "${WAYLAND_DISPLAY-unset}" "${WAYLAND_SOCKET-unset}" "${NEWPORT_GUI_DISPLAY-unset}" "${NEWPORT_GUI_AUTHORITY-unset}" "$NEWPORT_GUI_WAYLAND_DISPLAY" "${NEWPORT_GUI_RUNTIME_DIR-unset}" "$NEWPORT_SESSION" "$DBUS_SESSION_BUS_ADDRESS" "$XDG_RUNTIME_DIR" "$@"
printf 'executed\n' >> "$HOME/executed"
exit 17
"#).unwrap();
        fs::set_permissions(program, fs::Permissions::from_mode(0o700)).unwrap();
        fixture
    }
    fn send(&mut self, kind: u8, bytes: &[u8]) {
        wire::write(self.worker.stdin.as_mut().unwrap(), kind, bytes).unwrap();
    }
    fn event(&self, kind: u8) -> Vec<u8> {
        let (got, data) = self.events.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(got, kind, "{}", String::from_utf8_lossy(&data));
        data
    }
    fn enable_shared(&mut self) {
        self.send(b'L', b"2");
        self.event(b'A');
        let root = self.runtime.integration_root(self.home.path());
        assert!(
            fs::read_to_string(root.join("features"))
                .unwrap()
                .contains("gui-launch-v2")
        );
    }
    fn launch(&self, epoch: Option<Uuid>, runtime: Option<&str>) -> Process {
        self.launch_with_bus(epoch, runtime, Some("unix:path=/user/bus"))
    }
    fn launch_with_bus(
        &self,
        epoch: Option<Uuid>,
        runtime: Option<&str>,
        bus: Option<&str>,
    ) -> Process {
        let mut command = Command::new(BIN);
        command
            .args(["gui", "--program"])
            .arg(self.home.path().join("gui-fixture"))
            .args([
                "--",
                "space argument",
                "$(touch injected)",
                "",
                "--flag=value",
            ])
            .env("HOME", self.home.path())
            .env("PATH", "/usr/bin:/bin")
            .env("NEWPORT_SESSION", &self.runtime.session)
            .env("NEWPORT_TERMINAL", self.terminal.to_string())
            .env("WAYLAND_SOCKET", "99")
            .env_remove("DBUS_SESSION_BUS_ADDRESS")
            .env_remove("DBUS_STARTER_ADDRESS")
            .env("DISPLAY", ":999")
            .env("XAUTHORITY", "/foreign/Xauthority")
            .env_remove("WAYLAND_DISPLAY")
            .env_remove("NEWPORT_GUI_DISPLAY")
            .env_remove("NEWPORT_GUI_AUTHORITY")
            .env_remove("NEWPORT_GUI_WAYLAND_DISPLAY")
            .env_remove("NEWPORT_GUI_RUNTIME_DIR")
            .env_remove("XDG_CURRENT_DESKTOP")
            .env_remove("GSETTINGS_SCHEMA_DIR")
            .env_remove("GTK_A11Y")
            .env_remove("NO_AT_BRIDGE")
            .current_dir(self.home.path().join("working directory"))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(bus) = bus {
            command.env("DBUS_SESSION_BUS_ADDRESS", bus);
        }
        match epoch {
            Some(epoch) => {
                command.env("NEWPORT_TERMINAL_EPOCH", epoch.to_string());
            }
            None => {
                command.env_remove("NEWPORT_TERMINAL_EPOCH");
            }
        }
        match runtime {
            Some(runtime) => {
                command.env("XDG_RUNTIME_DIR", runtime);
            }
            None => {
                command.env_remove("XDG_RUNTIME_DIR");
            }
        }
        Process(Some(command.spawn().unwrap()))
    }
    fn display(&self) -> (gui::SharedGuiEnvironment, UnixListener) {
        let socket = self.home.path().join("wayland ' socket");
        let listener = UnixListener::bind(&socket).unwrap();
        let authority = self.home.path().join("authority ' $(touch injected)");
        fs::write(&authority, "fixture").unwrap();
        (
            gui::SharedGuiEnvironment {
                session: self.runtime.session.parse().unwrap(),
                display: Some(":75".into()),
                authority: Some(authority.to_str().unwrap().into()),
                wayland_socket: socket.to_str().unwrap().into(),
                fallback_bus: None,
                toolkit: [("GDK_BACKEND".into(), Some("wayland,x11".into()))].into(),
                application: Default::default(),
            },
            listener,
        )
    }
    fn reply(&mut self, request: &gui::SharedRequest, environment: gui::SharedGuiEnvironment) {
        self.send(
            b'j',
            &gui::encode(&gui::SharedReply {
                id: request.id,
                epoch: request.epoch,
                environment: Some(environment),
                error: None,
            })
            .unwrap(),
        );
        self.event(b'A');
    }
}
struct Process(Option<Child>);
impl Process {
    fn live(&mut self) -> bool {
        self.0.as_mut().unwrap().try_wait().unwrap().is_none()
    }
    fn finish(mut self) -> Output {
        let until = Instant::now() + Duration::from_secs(5);
        while self.live() {
            assert!(Instant::now() < until, "GUI helper did not finish");
            std::thread::sleep(Duration::from_millis(10));
        }
        self.0.take().unwrap().wait_with_output().unwrap()
    }
}
impl Drop for Process {
    fn drop(&mut self) {
        if let Some(child) = &mut self.0 {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}
fn fields(output: &Output) -> Vec<String> {
    output
        .stdout
        .split(|b| *b == 0)
        .map(|b| String::from_utf8(b.into()).unwrap())
        .collect()
}

#[test]
fn attachment_check_is_independently_negotiated_and_never_executes_an_app() {
    use gui::attachment as check;
    let mut fixture = Fixture::new();
    fixture
        .runtime
        .hello
        .require(check::CAPABILITY, check::VERSION)
        .unwrap();
    let root = fixture.runtime.integration_root(fixture.home.path());
    assert!(
        !fs::read_to_string(root.join("features"))
            .unwrap()
            .contains(check::FEATURE)
    );
    let (environment, _display) = fixture.display();
    let request = check::Check {
        id: Uuid::new_v4(),
        session: environment.session,
        terminal: fixture.terminal,
        epoch: Uuid::new_v4(),
        endpoints: check::Endpoints {
            display: environment.display,
            authority: environment.authority,
            wayland_socket: environment.wayland_socket,
        },
    };
    let connect = || {
        let mut socket = std::os::unix::net::UnixStream::connect(root.join("gui.sock")).unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        wire::write(
            &mut socket,
            check::REQUEST,
            &check::encode(&request).unwrap(),
        )
        .unwrap();
        socket
    };
    let mut old = connect();
    assert!(wire::read(&mut old).is_err());
    fixture.send(check::ENABLE, b"1");
    fixture.event(b'A');
    assert!(
        fs::read_to_string(root.join("features"))
            .unwrap()
            .contains(check::FEATURE)
    );
    let mut socket = connect();
    assert_eq!(
        check::decode::<check::Check>(&fixture.event(check::REQUEST)).unwrap(),
        request
    );
    let mut reply = check::Checked::new(&request, check::Outcome::Current {});
    reply.epoch = Uuid::new_v4();
    fixture.send(check::REPLY, &check::encode(&reply).unwrap());
    fixture.event(b'A');
    socket.set_nonblocking(true).unwrap();
    use std::io::Read;
    assert_eq!(
        socket.read(&mut [0]).unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock
    );
    socket.set_nonblocking(false).unwrap();
    let reply = check::Checked::error(&request, "permission denied");
    fixture.send(check::REPLY, &check::encode(&reply).unwrap());
    fixture.event(b'A');
    let (kind, bytes) = wire::read(&mut socket).unwrap();
    assert_eq!(kind, check::REPLY);
    assert_eq!(
        check::decode::<check::Checked>(&bytes)
            .unwrap()
            .accept(&request)
            .unwrap(),
        &reply.outcome
    );
    fixture.send(b'H', &[]);
    fixture.event(b'A');
    assert!(!fixture.home.path().join("executed").exists());
}

#[test]
fn shared_gui_owned_bus_fallback_requires_negotiation_and_preserves_explicit_bus() {
    for inherited in [None, Some(""), Some("unix:path=/user/bus")] {
        let mut fixture = Fixture::new();
        fixture.send(b'L', b"3");
        fixture.event(b'A');
        let epoch = Uuid::new_v4();
        let process = fixture.launch_with_bus(Some(epoch), None, inherited);
        let request: gui::SharedRequest = gui::decode(&fixture.event(b'l')).unwrap();
        assert!(request.session_bus);
        let (mut environment, _display) = fixture.display();
        let socket = fixture.home.path().join("bus");
        let _bus = UnixListener::bind(&socket).unwrap();
        let address = format!("unix:path={}", socket.display());
        environment.fallback_bus = Some(address.clone());
        fixture.reply(&request, environment.clone());
        let result = process.finish();
        assert_eq!(result.status.code(), Some(17), "{result:?}");
        assert_eq!(
            fields(&result)[10],
            inherited.filter(|v| !v.is_empty()).unwrap_or(&address)
        );

        fs::remove_file(socket).unwrap();
        let process = fixture.launch_with_bus(Some(epoch), None, None);
        let request: gui::SharedRequest = gui::decode(&fixture.event(b'l')).unwrap();
        fixture.reply(&request, environment);
        let result = process.finish();
        assert!(!result.status.success());
        assert!(result.stdout.is_empty());
        assert!(String::from_utf8_lossy(&result.stderr).contains("session bus ended"));
        assert_eq!(
            fs::read_to_string(fixture.home.path().join("executed")).unwrap(),
            "executed\n"
        );
    }
}

#[test]
fn shared_gui_preserves_command_and_user_environment_and_executes_once() {
    for native in [false, true] {
        for runtime_dir in [None, Some(""), Some("/user/runtime")] {
            let mut fixture = Fixture::new();
            fixture.send(b'L', if native { b"4" } else { b"2" });
            fixture.event(b'A');
            let epoch = Uuid::new_v4();
            let mut process = fixture.launch(Some(epoch), runtime_dir);
            let request: gui::SharedRequest = gui::decode(&fixture.event(b'l')).unwrap();
            assert_eq!((request.terminal, request.epoch), (fixture.terminal, epoch));
            assert_eq!(request.application, "gui-fixture");
            assert!(process.live());
            fixture.send(b'H', &[]);
            fixture.event(b'A');
            assert_eq!(request.native_wayland, native);
            assert_eq!(request.session_bus, native);
            let (mut environment, _listener) = fixture.display();
            if native {
                fs::remove_file(environment.authority.take().unwrap()).unwrap();
                environment.display = None;
            }
            fixture.reply(&request, environment.clone());
            let result = process.finish();
            assert_eq!(result.status.code(), Some(17), "{result:?}");
            let home = fixture.home.path().canonicalize().unwrap();
            let expected_runtime = runtime_dir
                .filter(|s| !s.is_empty())
                .unwrap_or(environment.wayland_socket.rsplit_once('/').unwrap().0);
            assert_eq!(
                fields(&result),
                vec![
                    home.join("working directory").display().to_string(),
                    environment
                        .display
                        .clone()
                        .unwrap_or_else(|| "unset".into()),
                    environment
                        .authority
                        .clone()
                        .unwrap_or_else(|| "unset".into()),
                    environment.wayland_socket.clone(),
                    "unset".into(),
                    environment
                        .display
                        .clone()
                        .unwrap_or_else(|| "unset".into()),
                    environment
                        .authority
                        .clone()
                        .unwrap_or_else(|| "unset".into()),
                    environment.wayland_socket.clone(),
                    "unset".into(),
                    fixture.runtime.session.clone(),
                    "unix:path=/user/bus".into(),
                    expected_runtime.into(),
                    "space argument".into(),
                    "$(touch injected)".into(),
                    "".into(),
                    "--flag=value".into(),
                    "".into(),
                ]
            );
            fixture.reply(&request, environment);
            assert_eq!(
                fs::read_to_string(home.join("executed")).unwrap(),
                "executed\n"
            );
            assert!(!home.join("working directory/injected").exists());
        }
    }
}

#[test]
fn shared_gui_rejects_wrong_shell_without_consuming_request_and_cancels_on_exit() {
    let mut fixture = Fixture::new();
    fixture.enable_shared();
    let mut process = fixture.launch(Some(Uuid::new_v4()), None);
    let request: gui::SharedRequest = gui::decode(&fixture.event(b'l')).unwrap();
    let (environment, _listener) = fixture.display();
    let mut wrong = request.clone();
    wrong.epoch = Uuid::new_v4();
    fixture.reply(&wrong, environment.clone());
    fixture.send(
        b'J',
        &gui::encode(&gui::Reply {
            id: request.id,
            environment: None,
            error: Some("old mode".into()),
        })
        .unwrap(),
    );
    fixture.event(b'A');
    assert!(process.live());
    drop(process);
    assert_eq!(
        gui::decode::<Uuid>(&fixture.event(b'K')).unwrap(),
        request.id
    );
    fixture.reply(&request, environment);
    assert!(!fixture.home.path().join("executed").exists());
}

#[test]
fn an_unrequested_reply_is_dropped_without_ending_the_connection() {
    let mut fixture = Fixture::new();
    fixture.enable_shared();
    let process = fixture.launch(Some(Uuid::new_v4()), None);
    let request: gui::SharedRequest = gui::decode(&fixture.event(b'l')).unwrap();
    assert!(!request.session_bus);
    let (mut environment, _listener) = fixture.display();
    environment.fallback_bus = Some("unix:path=/unrequested/bus".into());
    fixture.reply(&request, environment.clone());
    fixture.send(b'j', b"not a reply");
    fixture.event(b'A');
    fixture.send(b'H', &[]);
    fixture.event(b'A');
    environment.fallback_bus = None;
    fixture.reply(&request, environment);
    let result = process.finish();
    assert_eq!(result.status.code(), Some(17), "{result:?}");
}

#[test]
fn shared_gui_rejects_foreign_or_invalid_or_missing_endpoints_without_execution() {
    for case in [
        "foreign",
        "toolkit",
        "socket",
        "authority",
        "ambiguous",
        "native_socket",
    ] {
        let mut fixture = Fixture::new();
        fixture.send(b'L', if case == "native_socket" { b"4" } else { b"2" });
        fixture.event(b'A');
        let process = fixture.launch(Some(Uuid::new_v4()), None);
        let request: gui::SharedRequest = gui::decode(&fixture.event(b'l')).unwrap();
        let (mut environment, _listener) = fixture.display();
        match case {
            "foreign" => environment.session = Uuid::new_v4(),
            "toolkit" => {
                environment
                    .toolkit
                    .insert("LD_PRELOAD".into(), Some("/bad.so".into()));
            }
            "native_socket" => {
                environment.display = None;
                environment.authority = None;
                fs::remove_file(&environment.wayland_socket).unwrap();
            }
            "socket" => {
                fs::remove_file(&environment.wayland_socket).unwrap();
            }
            "authority" => {
                fs::remove_file(environment.authority.as_ref().unwrap()).unwrap();
            }
            _ => {}
        }
        fixture.send(
            b'j',
            &gui::encode(&gui::SharedReply {
                id: request.id,
                epoch: request.epoch,
                environment: Some(environment),
                error: (case == "ambiguous").then(|| "inconsistent".into()),
            })
            .unwrap(),
        );
        fixture.event(b'A');
        let result = process.finish();
        assert!(!result.status.success(), "{case}: {result:?}");
        assert_ne!(result.status.code(), Some(17), "{case}");
        assert!(!fixture.home.path().join("executed").exists());
    }
}

#[test]
fn shared_gui_negotiation_preserves_waiting_v1_and_refuses_missing_shell_identity() {
    let mut fixture = Fixture::new();
    let legacy = fixture.launch(None, None);
    let old: gui::Request = gui::decode(&fixture.event(b'L')).unwrap();
    fixture.enable_shared();
    let process = fixture.launch(None, None);
    let result = process.finish();
    assert!(String::from_utf8_lossy(&result.stderr).contains("Reconnect this terminal"));
    fixture.send(
        b'J',
        &gui::encode(&gui::Reply {
            id: old.id,
            error: Some("old request cancelled".into()),
            environment: None,
        })
        .unwrap(),
    );
    fixture.event(b'A');
    assert!(String::from_utf8_lossy(&legacy.finish().stderr).contains("old request cancelled"));
    assert!(!fixture.home.path().join("executed").exists());
    // Even a helper that connects and exits before sending a request must not
    // kill the integration broker or lose another waiting launch.
    fixture.send(b'H', &[]);
    fixture.event(b'A');
}

#[test]
fn shared_gui_pending_bound_is_shared_with_legacy_and_slots_are_reusable() {
    use std::os::unix::net::UnixStream;
    let mut fixture = Fixture::new();
    fixture.enable_shared();
    let endpoint = fixture
        .runtime
        .integration_root(fixture.home.path())
        .join("gui.sock");
    let mut pending = Vec::new();
    for i in 0..gui::MAX_PENDING {
        let mut socket = UnixStream::connect(&endpoint).unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        let request = gui::SharedRequest {
            id: Uuid::new_v4(),
            terminal: fixture.terminal,
            epoch: Uuid::new_v4(),
            application: "fixture".into(),
            session_bus: false,
            native_wayland: false,
            application_environment: false,
            application_environment_v2: false,
        };
        let kind = if i % 2 == 0 { b'l' } else { b'L' };
        let bytes = if kind == b'l' {
            gui::encode(&request).unwrap()
        } else {
            gui::encode(&gui::Request {
                id: request.id,
                terminal: request.terminal,
                application: request.application.clone(),
            })
            .unwrap()
        };
        wire::write(&mut socket, kind, &bytes).unwrap();
        fixture.event(kind);
        pending.push((socket, kind, bytes, request));
    }
    let mut refused = UnixStream::connect(&endpoint).unwrap();
    refused
        .set_read_timeout(Some(Duration::from_secs(3)))
        .unwrap();
    let request = gui::SharedRequest {
        id: Uuid::new_v4(),
        terminal: fixture.terminal,
        epoch: Uuid::new_v4(),
        application: "one more".into(),
        session_bus: false,
        native_wayland: false,
        application_environment: false,
        application_environment_v2: false,
    };
    wire::write(&mut refused, b'l', &gui::encode(&request).unwrap()).unwrap();
    let (kind, bytes) = wire::read(&mut refused).unwrap();
    assert_eq!(kind, b'j');
    let reply: gui::SharedReply = gui::decode(&bytes).unwrap();
    assert_eq!((reply.id, reply.epoch), (request.id, request.epoch));
    assert!(reply.error.unwrap().contains("Too many"));
    let (socket, _, _, old) = pending.remove(0);
    drop(socket);
    assert_eq!(gui::decode::<Uuid>(&fixture.event(b'K')).unwrap(), old.id);
    // Capacity is available, but a duplicate ID cannot replace a live peer.
    let (original, duplicate_kind, duplicate_bytes, duplicate_request) = &mut pending[0];
    let mut duplicate = UnixStream::connect(&endpoint).unwrap();
    duplicate
        .set_read_timeout(Some(Duration::from_secs(3)))
        .unwrap();
    wire::write(&mut duplicate, *duplicate_kind, duplicate_bytes).unwrap();
    let (got, _) = wire::read(&mut duplicate).unwrap();
    assert_eq!(got, b'J');
    fixture.send(
        b'J',
        &gui::encode(&gui::Reply {
            id: duplicate_request.id,
            environment: None,
            error: Some("original still waiting".into()),
        })
        .unwrap(),
    );
    fixture.event(b'A');
    let (kind, bytes) = wire::read(original).unwrap();
    assert_eq!(kind, b'J');
    assert_eq!(
        gui::decode::<gui::Reply>(&bytes).unwrap().error.as_deref(),
        Some("original still waiting")
    );
    let mut replacement = UnixStream::connect(&endpoint).unwrap();
    replacement
        .set_read_timeout(Some(Duration::from_secs(3)))
        .unwrap();
    wire::write(&mut replacement, b'l', &gui::encode(&request).unwrap()).unwrap();
    fixture.event(b'l');
    fixture.send(
        b'j',
        &gui::encode(&gui::SharedReply {
            id: request.id,
            epoch: request.epoch,
            environment: None,
            error: Some("cancelled".into()),
        })
        .unwrap(),
    );
    fixture.event(b'A');
    assert_eq!(wire::read(&mut replacement).unwrap().0, b'j');
    fixture.send(b'H', &[]);
    fixture.event(b'A');
}

#[test]
fn helpers_negotiate_application_profiles_before_executing_once() {
    for version in [4, 5, 6] {
        let mut fixture = Fixture::new();
        fixture.send(
            b'L',
            match version {
                4 => b"4",
                5 => b"5",
                _ => b"6",
            },
        );
        fixture.event(b'A');
        let root = fixture.runtime.integration_root(fixture.home.path());
        assert_eq!(
            fs::read_to_string(root.join("features"))
                .unwrap()
                .split_whitespace()
                .any(|name| name == "gui-launch-v6"),
            version == 6,
        );
        fs::write(fixture.home.path().join("gui-fixture"), r#"#!/bin/sh
printf '%s\0' "${XDG_CURRENT_DESKTOP-unset}" "${GSETTINGS_SCHEMA_DIR-unset}" "${GTK_A11Y-unset}" "${NO_AT_BRIDGE-unset}"
printf 'executed\n' >> "$HOME/executed"
exit 17
"#).unwrap();
        let process = fixture.launch(Some(Uuid::new_v4()), None);
        let bytes = fixture.event(b'l');
        let wire: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        let request: gui::SharedRequest = gui::decode(&bytes).unwrap();
        assert_eq!(request.application_environment, version >= 5);
        assert_eq!(request.application_environment_v2, version == 6);
        assert_eq!(wire.get("applicationEnvironmentV2").is_some(), version == 6);
        let (mut environment, _display) = fixture.display();
        environment.application = [
            ("XDG_CURRENT_DESKTOP".into(), Some("X-Newport".into())),
            (
                "GSETTINGS_SCHEMA_DIR".into(),
                Some("/owned/share/glib-2.0/schemas".into()),
            ),
            ("GTK_A11Y".into(), Some("none".into())),
            ("NO_AT_BRIDGE".into(), Some("1".into())),
        ]
        .into();
        environment.validate().unwrap();
        request.project_application(&mut environment);
        fixture.reply(&request, environment);
        let result = process.finish();
        assert_eq!(result.status.code(), Some(17), "{result:?}");
        assert_eq!(
            fields(&result),
            match version {
                4 => vec!["unset", "unset", "unset", "unset", ""],
                5 => vec!["X-Newport", "unset", "unset", "unset", ""],
                _ => vec![
                    "X-Newport",
                    "/owned/share/glib-2.0/schemas",
                    "none",
                    "1",
                    ""
                ],
            }
        );
        assert_eq!(
            fs::read_to_string(fixture.home.path().join("executed")).unwrap(),
            "executed\n"
        );
    }
}
