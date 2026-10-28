//! Real helper processes and managed broker. The fixture is the backend peer:
//! it supplies explicit validation outcomes, never launches or acquires a GUI.
use super::*;
use protocol::attachment as check;
use std::{os::unix::net::UnixListener, time::Instant};
use uuid::Uuid;

#[path = "gui_attachment/fish.rs"]
mod fish;

struct Fixture {
    agent: Agent,
    home: tempfile::TempDir,
    session: Uuid,
    terminal: Uuid,
    epoch: Uuid,
    program: std::path::PathBuf,
    _socket: UnixListener,
}
impl Fixture {
    fn new(enabled: bool) -> Self {
        let home = tempfile::tempdir_in("/tmp").unwrap();
        let program = fixture(home.path());
        fs::create_dir(home.path().join("working directory")).unwrap();
        fs::write(home.path().join("Xauthority"), "authority").unwrap();
        let socket = UnixListener::bind(home.path().join("wayland.sock")).unwrap();
        let mut agent = Agent::start(home.path());
        if enabled {
            agent.send(check::ENABLE, b"1");
            agent.event(b'A');
        }
        let fixture = Self {
            agent,
            home,
            session: Uuid::new_v4(),
            terminal: Uuid::new_v4(),
            epoch: Uuid::new_v4(),
            program,
            _socket: socket,
        };
        fixture.route(fixture.session, None);
        fixture
    }
    fn route(&self, current: Uuid, alias: Option<Uuid>) {
        let records = self.home.path().join(".cache/newport/sessions");
        fs::create_dir_all(&records).unwrap();
        fs::set_permissions(&records, fs::Permissions::from_mode(0o700)).unwrap();
        let record = serde_json::json!({"session":current,"runtime":Uuid::new_v4(),"client":"attachment-test","root":self.home.path().join(".cache/newport/clipboard")});
        let path = records.join("session-fixture.json");
        fs::write(&path, serde_json::to_vec(&record).unwrap()).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
        if let Some(alias) = alias {
            let dir = self.home.path().join(".cache/newport/routes");
            fs::create_dir_all(&dir).unwrap();
            fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
            let path = dir.join("attachment-test.json");
            fs::write(
                &path,
                serde_json::to_vec(&serde_json::json!({"current":record,"aliases":[alias]}))
                    .unwrap(),
            )
            .unwrap();
            fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
        }
    }
    fn environment(&self, command: &mut Command, native: bool) {
        command
            .env("HOME", self.home.path())
            .env_remove("ENV")
            .env_remove("BASH_ENV")
            .env_remove("ZDOTDIR")
            .env("NEWPORT_SESSION", self.session.to_string())
            .env("NEWPORT_TERMINAL", self.terminal.to_string())
            .env("NEWPORT_TERMINAL_EPOCH", self.epoch.to_string())
            .env("PATH", "/usr/bin:/bin")
            .env("DBUS_SESSION_BUS_ADDRESS", "")
            .env_remove("DBUS_STARTER_ADDRESS")
            .env("GDK_BACKEND", "wayland,x11")
            .current_dir(self.home.path().join("working directory"))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        owned_display(command, self.home.path(), !native, true);
    }
    fn command(&self, native: bool, explicit: bool) -> Command {
        let mut command = Command::new(BIN);
        if explicit {
            command.args(["env", "--check-gui", "--shell", "sh"]);
        } else {
            command.args(["gui", "--program"]).arg(&self.program).args([
                "--",
                "literal $(touch INJECTED)",
                "",
                "space argument",
            ]);
        }
        self.environment(&mut command, native);
        command
    }
    fn request(&self, native: bool) -> check::Check {
        let request: check::Check = check::decode(&self.agent.event(check::REQUEST)).unwrap();
        assert_eq!(
            (request.session, request.terminal, request.epoch),
            (self.session, self.terminal, self.epoch)
        );
        assert_eq!(
            request.endpoints.wayland_socket,
            self.home.path().join("wayland.sock").to_str().unwrap()
        );
        assert_eq!(
            request.endpoints.display.as_deref(),
            (!native).then_some(":75")
        );
        assert_eq!(
            request.endpoints.authority,
            (!native).then(|| self.home.path().join("Xauthority").to_str().unwrap().into())
        );
        request
    }
    fn reply(&mut self, request: &check::Check, outcome: check::Outcome) {
        self.agent.send(
            check::REPLY,
            &check::encode(&check::Checked::new(request, outcome)).unwrap(),
        );
        self.agent.event(b'A');
    }
    fn idle(&mut self) {
        self.agent.send(b'H', &[]);
        self.agent.event(b'A');
        assert!(
            self.agent.events.try_recv().is_err(),
            "No launch/replay/demand request is allowed"
        );
    }
}

#[test]
#[ignore = "Requires a cached local tmux executable; no packages are installed"]
fn existing_tmux_pane_keeps_old_identity_and_only_explicit_validation_refuses_it() {
    let executable = [
        "/opt/homebrew/bin/tmux",
        "/usr/bin/tmux",
        "/usr/local/bin/tmux",
    ]
    .into_iter()
    .find(|path| Path::new(path).is_file())
    .expect("tmux is required");
    struct Server {
        executable: &'static str,
        socket: std::path::PathBuf,
    }
    impl Server {
        fn command(&self) -> Command {
            let mut c = Command::new(self.executable);
            c.args(["-S"]).arg(&self.socket);
            c
        }
        fn run(&self, args: &[&str]) {
            let output = self.command().args(args).output().unwrap();
            assert!(output.status.success(), "tmux fixture: {output:?}");
        }
        fn inspect(&self, target: &str, name: &str) {
            let script = format!(
                r#""$AGENT" env --check-gui --shell sh > "$HOME/{name}.out" 2> "$HOME/{name}.err"; printf '%s\n' "$?" > "$HOME/{name}.status"; printf '%s\n' "$NEWPORT_SESSION" "$NEWPORT_TERMINAL" "$NEWPORT_TERMINAL_EPOCH" "$WAYLAND_DISPLAY" > "$HOME/{name}.ids""#
            );
            self.run(&["send-keys", "-t", target, "-l", &script]);
            self.run(&["send-keys", "-t", target, "Enter"]);
        }
    }
    impl Drop for Server {
        fn drop(&mut self) {
            let _ = self.command().args(["kill-server"]).output();
        }
    }
    fn completed(home: &Path, name: &str) -> String {
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            if let Ok(value) = fs::read_to_string(home.join(format!("{name}.ids"))) {
                return value;
            }
            assert!(
                Instant::now() < deadline,
                "tmux fixture command did not finish"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    let mut f = Fixture::new(true);
    let server = Server {
        executable,
        socket: f.home.path().join("tmux.sock"),
    };
    let mut create = server.command();
    create
        .args([
            "-f",
            "/dev/null",
            "new-session",
            "-d",
            "-s",
            "attachment",
            "-n",
            "old",
            "/bin/sh",
        ])
        .env("AGENT", BIN);
    create
        .env_remove("TMUX")
        .env_remove("TMUX_PANE")
        .env("TERM", "xterm");
    f.environment(&mut create, false);
    let output = create.output().unwrap();
    assert!(output.status.success(), "{output:?}");
    let next = (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
    f.route(next.0, Some(f.session));
    for (name, id) in [
        ("NEWPORT_SESSION", next.0),
        ("NEWPORT_TERMINAL", next.1),
        ("NEWPORT_TERMINAL_EPOCH", next.2),
    ] {
        server.run(&["set-environment", "-g", name, &id.to_string()]);
    }
    server.inspect("attachment:old", "old");
    let old = f.request(false);
    f.reply(
        &old,
        check::Outcome::NotCurrent {
            reason: check::Reason::StaleShell,
        },
    );
    assert_eq!(
        completed(f.home.path(), "old"),
        format!(
            "{}\n{}\n{}\n{}\n",
            f.session,
            f.terminal,
            f.epoch,
            f.home.path().join("wayland.sock").display()
        )
    );
    assert_ne!(
        fs::read_to_string(f.home.path().join("old.status")).unwrap(),
        "0\n"
    );
    assert!(fs::read(f.home.path().join("old.out")).unwrap().is_empty());
    server.run(&[
        "new-window",
        "-d",
        "-t",
        "attachment",
        "-n",
        "fresh",
        "/bin/sh",
    ]);
    server.inspect("attachment:fresh", "fresh");
    let new: check::Check = check::decode(&f.agent.event(check::REQUEST)).unwrap();
    assert_eq!((new.session, new.terminal, new.epoch), next);
    f.reply(&new, check::Outcome::Current {});
    assert_eq!(
        completed(f.home.path(), "fresh"),
        format!(
            "{}\n{}\n{}\n{}\n",
            next.0,
            next.1,
            next.2,
            f.home.path().join("wayland.sock").display()
        )
    );
    assert_eq!(
        fs::read_to_string(f.home.path().join("fresh.status")).unwrap(),
        "0\n"
    );
    assert!(
        !fs::read(f.home.path().join("fresh.out"))
            .unwrap()
            .is_empty()
    );
    assert!(!f.home.path().join("executed").exists());
    f.idle();
}

#[test]
fn current_mixed_and_native_claims_execute_once_with_literal_process_state() {
    for native in [false, true] {
        let mut f = Fixture::new(true);
        let process = f.command(native, false).spawn().unwrap();
        let request = f.request(native);
        assert!(!f.home.path().join("executed").exists());
        f.reply(&request, check::Outcome::Current {});
        let output = finish(process);
        assert_eq!(output.status.code(), Some(17), "{output:?}");
        let fields = output
            .stdout
            .split(|b| *b == 0)
            .map(|b| String::from_utf8_lossy(b).into_owned())
            .collect::<Vec<_>>();
        assert_eq!(
            fields,
            vec![
                f.home
                    .path()
                    .join("working directory")
                    .canonicalize()
                    .unwrap()
                    .to_str()
                    .unwrap()
                    .to_owned(),
                if native { "" } else { ":75" }.into(),
                if native {
                    String::new()
                } else {
                    f.home.path().join("Xauthority").to_str().unwrap().into()
                },
                f.home.path().join("wayland.sock").to_str().unwrap().into(),
                "literal $(touch INJECTED)".into(),
                "".into(),
                "space argument".into(),
                "".into()
            ]
        );
        assert_eq!(
            fs::read_to_string(f.home.path().join("executed")).unwrap(),
            "executed\n"
        );
        assert!(!f.home.path().join("working directory/INJECTED").exists());
        f.idle();
    }
}
#[test]
fn not_current_and_permission_failures_preserve_existing_endpoints_and_never_execute() {
    for outcome in [
        check::Outcome::NotCurrent {
            reason: check::Reason::StaleShell,
        },
        check::Outcome::NotCurrent {
            reason: check::Reason::EndpointMismatch,
        },
        check::Outcome::NotCurrent {
            reason: check::Reason::NoSharedDisplay,
        },
        check::Outcome::Error {
            message: "permission revoked".into(),
        },
        check::Outcome::Error {
            message: "provider transport closed".into(),
        },
    ] {
        let mut f = Fixture::new(true);
        for explicit in [false, true] {
            let process = f.command(false, explicit).spawn().unwrap();
            let request = f.request(false);
            f.reply(&request, outcome.clone());
            let output = finish(process);
            assert!(!output.status.success());
            assert!(output.stdout.is_empty());
            let error = String::from_utf8_lossy(&output.stderr);
            assert!(error.contains("current Newport terminal"), "{error}");
            if let check::Outcome::Error { message } = &outcome {
                assert!(error.contains(message));
            }
            assert!(!f.home.path().join("executed").exists());
            assert!(f.home.path().join("wayland.sock").exists());
            assert_eq!(
                fs::read_to_string(f.home.path().join("Xauthority")).unwrap(),
                "authority"
            );
            f.idle();
        }
    }
}
#[test]
fn historical_routing_preserves_original_claim_and_cannot_authorize_a_new_session() {
    let mut f = Fixture::new(true);
    f.route(Uuid::new_v4(), Some(f.session));
    for outcome in [
        check::Outcome::NotCurrent {
            reason: check::Reason::SessionMismatch,
        },
        check::Outcome::Current {},
    ] {
        let process = f.command(false, false).spawn().unwrap();
        let request = f.request(false); // Request still contains original session, terminal and epoch.
        f.reply(&request, outcome);
        let output = finish(process);
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        assert!(!f.home.path().join("executed").exists());
        f.idle();
    }
}
#[test]
fn explicit_legacy_check_and_transport_loss_do_not_downgrade() {
    for enabled in [false, true] {
        let mut f = Fixture::new(enabled);
        if enabled {
            fs::remove_file(f.home.path().join(".cache/newport/clipboard/gui.sock")).unwrap();
        }
        let before = Instant::now();
        let output = f.command(false, true).output().unwrap();
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        assert!(before.elapsed() < Duration::from_secs(2));
        if !enabled {
            assert!(String::from_utf8_lossy(&output.stderr).contains("does not support"));
        }
        if enabled {
            let output = f.command(false, false).output().unwrap();
            assert!(!output.status.success());
            assert!(output.stdout.is_empty());
        }
        assert!(!f.home.path().join("executed").exists());
        f.idle();
    }
}
#[test]
fn malformed_inherited_native_gui_helper_cannot_reacquire_a_display() {
    for enabled in [true, false] {
        let mut f = Fixture::new(enabled);
        for (name, value) in [
            ("WAYLAND_SOCKET", Some("")),
            ("WAYLAND_SOCKET", Some("99")),
            ("WAYLAND_DISPLAY", Some("")),
            ("WAYLAND_DISPLAY", Some("/host/wayland")),
            ("WAYLAND_DISPLAY", None),
            ("NEWPORT_GUI_WAYLAND_DISPLAY", Some("")),
            ("NEWPORT_GUI_WAYLAND_DISPLAY", Some("/different/claim")),
        ] {
            let mut command = f.command(true, false);
            if let Some(value) = value {
                command.env(name, value);
            } else {
                command.env_remove(name);
            }
            let output = finish(command.spawn().unwrap());
            assert!(
                !output.status.success(),
                "enabled={enabled} {name}={value:?}: {output:?}"
            );
            assert!(output.stdout.is_empty());
            assert!(!f.home.path().join("executed").exists());
            assert!(f.home.path().join("wayland.sock").exists());
            f.idle(); // An a/l/L request is unexpected: these claims are malformed.
        }
    }
}

#[test]
fn incomplete_inherited_claims_refuse_even_when_terminal_identity_is_missing() {
    let mut f = Fixture::new(true);
    for native in [true, false] {
        for name in [
            "NEWPORT_SESSION",
            "NEWPORT_TERMINAL",
            "NEWPORT_TERMINAL_EPOCH",
        ] {
            for empty in [true, false] {
                let mut command = f.command(native, false);
                if empty {
                    command.env(name, "");
                } else {
                    command.env_remove(name);
                }
                let output = finish(command.spawn().unwrap());
                assert!(
                    !output.status.success(),
                    "native={native} {name}: {output:?}"
                );
                assert!(output.stdout.is_empty());
                f.idle();
            }
        }
    }
    // Only a leftover runtime marker also indicates an inherited claim. It
    // cannot become a fresh handoff just because all display markers vanished.
    let mut command = f.command(true, false);
    command.env_remove("NEWPORT_GUI_WAYLAND_DISPLAY").env(
        "NEWPORT_GUI_RUNTIME_DIR",
        f.home.path().join("user-runtime"),
    );
    let output = finish(command.spawn().unwrap());
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    f.idle();
    assert!(!f.home.path().join("executed").exists());
}

#[test]
fn malformed_partial_overrides_refuse_without_touching_host_environment_or_broker() {
    let mut f = Fixture::new(true);
    for (name, value) in [
        ("DISPLAY", ":0"),
        ("XAUTHORITY", "/host/authority"),
        ("WAYLAND_SOCKET", ""),
        ("NEWPORT_TERMINAL_EPOCH", ""),
    ] {
        let output = f.command(false, true).env(name, value).output().unwrap();
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        f.idle();
    }
    // Help/headless executions need no GUI validation, even if routing is gone.
    for flag in ["--help", "--version", "--headless"] {
        let mut command = Command::new(BIN);
        command
            .args(["gui", "--program"])
            .arg(&f.program)
            .args(["--", flag]);
        f.environment(&mut command, false);
        command.env("NEWPORT_SESSION", Uuid::new_v4().to_string());
        assert_eq!(command.output().unwrap().status.code(), Some(17));
        f.idle();
    }
}

#[test]
fn explicit_check_refreshes_only_after_current_and_preserves_shell_state() {
    for shell in ["sh", "bash", "zsh"] {
        let mut f = Fixture::new(true);
        let script = r#"set -- 'space argument' '' literal
before=$PATH
code=$("$AGENT" env --check-gui --shell sh) || exit $?
eval "$code"
printf '%s\0' "$#" "$1" "$2" "$3" "${DBUS_SESSION_BUS_ADDRESS-unset}" "${DBUS_STARTER_ADDRESS-unset}" "$WAYLAND_DISPLAY" "$GDK_BACKEND" "$PATH"
"#;
        let mut command = Command::new(shell);
        command.args(["-f", "-c", script]).env("AGENT", BIN);
        f.environment(&mut command, false);
        let process = command.spawn().unwrap();
        let request = f.request(false);
        f.reply(&request, check::Outcome::Current {});
        let output = finish(process);
        assert!(output.status.success(), "{shell}: {output:?}");
        let fields = output
            .stdout
            .split(|b| *b == 0)
            .map(|b| String::from_utf8_lossy(b).into_owned())
            .collect::<Vec<_>>();
        assert_eq!(
            &fields[..8],
            &[
                "3",
                "space argument",
                "",
                "literal",
                "",
                "unset",
                f.home.path().join("wayland.sock").to_str().unwrap(),
                "wayland,x11"
            ]
        );
        assert_eq!(
            fields[8],
            format!(
                "{}:/usr/bin:/bin",
                f.home.path().join(".local/bin").display()
            )
        );
        assert!(!f.home.path().join("executed").exists());
        f.idle();
    }
}
