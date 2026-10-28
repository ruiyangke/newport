use newport_agent::{gui_protocol as protocol, wire};
use std::{
    fs,
    io::Write,
    os::unix::{fs::PermissionsExt, net::UnixStream},
    path::Path,
    process::{Child, ChildStdin, Command, Stdio},
    sync::mpsc,
    time::Duration,
};

/// Newport waits this long for Needle's startup and replies; a slow runner
/// can exceed a tighter limit without anything being wrong.
const REPLY: Duration = Duration::from_secs(25);
const BIN: &str = env!("CARGO_BIN_EXE_newport-agent");
struct Agent {
    child: Child,
    input: ChildStdin,
    events: mpsc::Receiver<(u8, Vec<u8>)>,
}
impl Agent {
    fn start(home: &Path) -> Self {
        let mut child = Command::new(BIN)
            .args(["serve", "gui-test", "--managed"])
            .env("HOME", home)
            .env_remove("NEWPORT_SESSION")
            .env_remove("NEWPORT_TERMINAL")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let input = child.stdin.take().unwrap();
        let mut stdout = child.stdout.take().unwrap();
        let (send, events) = mpsc::channel();
        std::thread::spawn(move || {
            while let Ok(event) = wire::read(&mut stdout) {
                if send.send(event).is_err() {
                    break;
                }
            }
        });
        let mut agent = Self {
            child,
            input,
            events,
        };
        agent.event(b'R');
        agent.send(b'L', &[]);
        agent.event(b'A');
        agent
    }
    fn event(&self, kind: u8) -> Vec<u8> {
        let (got, bytes) = self.events.recv_timeout(REPLY).unwrap();
        assert_eq!(got, kind, "{}", String::from_utf8_lossy(&bytes));
        bytes
    }
    fn send(&mut self, kind: u8, bytes: &[u8]) {
        wire::write(&mut self.input, kind, bytes).unwrap();
    }
}
impl Drop for Agent {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
fn fixture(home: &Path) -> std::path::PathBuf {
    fs::create_dir_all(home.join("bin")).unwrap();
    fs::create_dir_all(home.join(".local/share/applications")).unwrap();
    let program = home.join("bin/gui-fixture");
    fs::write(&program,"#!/bin/sh\nprintf '%s\\0' \"$PWD\" \"$DISPLAY\" \"$XAUTHORITY\" \"${WAYLAND_DISPLAY-unset}\" \"$@\"\nprintf 'executed\\n' >> \"$HOME/executed\"\nexit 17\n").unwrap();
    fs::set_permissions(&program, fs::Permissions::from_mode(0o700)).unwrap();
    fs::write(
        home.join(".local/share/applications/gui-fixture.desktop"),
        format!(
            "[Desktop Entry]\nType=Application\nName=Fixture\nExec={}\nTerminal=false\n",
            program.display()
        ),
    )
    .unwrap();
    program
}
fn exports(home: &Path, id: uuid::Uuid) -> String {
    let output = Command::new(BIN)
        .args(["env", "--shell", "sh"])
        .env("HOME", home)
        .env("NEWPORT_TERMINAL", id.to_string())
        .env_remove("NEWPORT_SESSION")
        .env_remove("XDG_DATA_HOME")
        .env("XDG_DATA_DIRS", home.join("empty"))
        .env(
            "PATH",
            format!("{}:/usr/bin:/bin", home.join("bin").display()),
        )
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}
fn launch(home: &Path, id: uuid::Uuid, args: &[&str]) -> Child {
    let source = exports(home, id);
    let cwd = home.join("working directory");
    fs::create_dir_all(&cwd).unwrap();
    Command::new("sh")
        .arg("-c")
        .arg(format!("{source}\nexec gui-fixture \"$@\""))
        .arg("fixture-shell")
        .args(args)
        .env("HOME", home)
        .env("NEWPORT_TERMINAL", id.to_string())
        .env("XDG_SESSION_TYPE", "tty")
        .env_remove("NEWPORT_SESSION")
        .env_remove("XDG_DATA_HOME")
        .env("XDG_DATA_DIRS", home.join("empty"))
        .env(
            "PATH",
            format!("{}:/usr/bin:/bin", home.join("bin").display()),
        )
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap()
}
fn refusal(id: uuid::Uuid) -> Vec<u8> {
    protocol::encode(&protocol::Reply {
        id,
        environment: None,
        error: Some("fixture refused".into()),
    })
    .unwrap()
}
fn display_reply(id: uuid::Uuid) -> protocol::Reply {
    protocol::Reply {
        id,
        error: None,
        environment: Some(
            [
                ("DISPLAY", Some(":75")),
                ("XAUTHORITY", Some("/tmp/private/Xauthority")),
                ("WAYLAND_DISPLAY", None),
                ("WAYLAND_SOCKET", None),
            ]
            .into_iter()
            .map(|(k, v)| (k.to_owned(), v.map(str::to_owned)))
            .collect(),
        ),
    }
}
#[test]
fn fragmented_gui_admission_keeps_heartbeats_and_prompt_helpers_running() {
    let home = tempfile::tempdir_in("/tmp").unwrap();
    let mut agent = Agent::start(home.path());
    let path = home.path().join(".cache/newport/clipboard/gui.sock");
    let mut delayed = UnixStream::connect(&path).unwrap();
    delayed
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    let request = protocol::Request {
        id: uuid::Uuid::new_v4(),
        terminal: uuid::Uuid::new_v4(),
        application: "delayed".into(),
    };
    let frame = wire::encode(b'L', &protocol::encode(&request).unwrap()).unwrap();
    delayed.write_all(&frame[..2]).unwrap();
    // Each acknowledgment precedes the next broker poll. The second round
    // therefore fences a poll with only a partial header, without timing sleeps.
    for _ in 0..2 {
        agent.send(b'H', &[]);
        agent.event(b'A');
    }
    let mut prompt = UnixStream::connect(path).unwrap();
    prompt
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    let ready = protocol::Request {
        id: uuid::Uuid::new_v4(),
        terminal: uuid::Uuid::new_v4(),
        application: "prompt".into(),
    };
    wire::write(&mut prompt, b'L', &protocol::encode(&ready).unwrap()).unwrap();
    assert_eq!(
        protocol::decode::<protocol::Request>(&agent.event(b'L'))
            .unwrap()
            .id,
        ready.id
    );
    agent.send(b'J', &refusal(ready.id));
    agent.event(b'A');
    let (_, bytes) = wire::read(&mut prompt).unwrap();
    assert_eq!(
        protocol::decode::<protocol::Reply>(&bytes).unwrap().id,
        ready.id
    );
    delayed.write_all(&frame[2..8]).unwrap();
    for _ in 0..2 {
        agent.send(b'H', &[]);
        agent.event(b'A');
    }
    delayed.write_all(&frame[8..]).unwrap();
    assert_eq!(
        protocol::decode::<protocol::Request>(&agent.event(b'L'))
            .unwrap()
            .id,
        request.id
    );
    agent.send(b'J', &refusal(request.id));
    agent.event(b'A');
    let (_, bytes) = wire::read(&mut delayed).unwrap();
    assert_eq!(
        protocol::decode::<protocol::Reply>(&bytes).unwrap().id,
        request.id
    );
    agent.send(b'H', &[]);
    agent.event(b'A');
    assert!(agent.events.try_recv().is_err());
}
#[test]
fn waiting_launch_reports_the_peer_refusal_and_never_executes() {
    let home = tempfile::tempdir().unwrap();
    fixture(home.path());
    let mut agent = Agent::start(home.path());
    let id = uuid::Uuid::new_v4();
    let process = launch(home.path(), id, &["$(touch injected)"]);
    let request: protocol::Request = protocol::decode(&agent.event(b'L')).unwrap();
    assert_eq!(request.terminal, id);
    assert_eq!(request.application, "gui-fixture");
    assert!(!home.path().join("executed").exists());
    // Account/setup waiting never blocks integration heartbeats.
    agent.send(b'H', &[]);
    agent.event(b'A');
    agent.send(b'J', &refusal(request.id));
    agent.event(b'A');
    let output = process.wait_with_output().unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("fixture refused"));
    agent.send(b'J', &refusal(request.id));
    agent.event(b'A');
    assert!(!home.path().join("executed").exists());
    assert!(!home.path().join("working directory/injected").exists());
}
#[test]
fn closing_a_waiting_process_cancels_its_request() {
    let home = tempfile::tempdir().unwrap();
    fixture(home.path());
    let mut agent = Agent::start(home.path());
    let mut process = launch(home.path(), uuid::Uuid::new_v4(), &[]);
    let request: protocol::Request = protocol::decode(&agent.event(b'L')).unwrap();
    process.kill().unwrap();
    process.wait().unwrap();
    assert_eq!(
        protocol::decode::<uuid::Uuid>(&agent.event(b'K')).unwrap(),
        request.id
    );
    agent.send(b'J', &refusal(request.id));
    agent.event(b'A');
    assert!(!home.path().join("executed").exists());
}
#[test]
fn cli_invocations_do_not_request_gui_access() {
    let home = tempfile::tempdir().unwrap();
    fixture(home.path());
    let agent = Agent::start(home.path());
    for arg in ["--version", "--help", "--headless", "--headless=new"] {
        let output = launch(home.path(), uuid::Uuid::new_v4(), &[arg])
            .wait_with_output()
            .unwrap();
        assert_eq!(
            output.status.code(),
            Some(17),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(agent.events.try_recv().is_err());
    }
}
#[test]
fn a_display_reply_cannot_change_executable_or_environment() {
    let home = tempfile::tempdir().unwrap();
    fixture(home.path());
    let mut agent = Agent::start(home.path());
    let process = launch(home.path(), uuid::Uuid::new_v4(), &[]);
    let request: protocol::Request = protocol::decode(&agent.event(b'L')).unwrap();
    agent.send(b'J', &protocol::encode(&display_reply(request.id)).unwrap());
    agent.event(b'A');
    let output = process.wait_with_output().unwrap();
    assert!(!output.status.success());
    assert!(!home.path().join("executed").exists());
    assert!(String::from_utf8_lossy(&output.stderr).contains("Invalid GUI launch reply"));
}

#[test]
fn terminal_started_before_integrations_still_reaches_them_with_its_first_app() {
    let home = tempfile::tempdir().unwrap();
    fixture(home.path());
    let terminal = uuid::Uuid::new_v4();
    let process = launch(home.path(), terminal, &[]);
    assert!(
        home.path()
            .join(".cache/newport/gui-bin/gui-fixture")
            .is_file()
    );
    let mut agent = Agent::start(home.path());
    let request: protocol::Request = protocol::decode(&agent.event(b'L')).unwrap();
    assert_eq!(request.terminal, terminal);
    agent.send(b'J', &refusal(request.id));
    agent.event(b'A');
    let output = process.wait_with_output().unwrap();
    assert!(String::from_utf8_lossy(&output.stderr).contains("fixture refused"));
    assert!(!home.path().join("executed").exists());
}

fn owned_display(command: &mut Command, home: &Path, x11: bool, wayland: bool) {
    command
        .env_remove("DISPLAY")
        .env_remove("XAUTHORITY")
        .env_remove("NEWPORT_GUI_DISPLAY")
        .env_remove("NEWPORT_GUI_AUTHORITY")
        .env_remove("WAYLAND_DISPLAY")
        .env_remove("WAYLAND_SOCKET")
        .env_remove("NEWPORT_GUI_WAYLAND_DISPLAY")
        .env_remove("NEWPORT_GUI_RUNTIME_DIR")
        .env("XDG_RUNTIME_DIR", home.join("user-runtime"));
    if x11 {
        command
            .env("DISPLAY", ":75")
            .env("NEWPORT_GUI_DISPLAY", ":75")
            .env("XAUTHORITY", home.join("Xauthority"))
            .env("NEWPORT_GUI_AUTHORITY", home.join("Xauthority"));
    }
    if wayland {
        command
            .env("WAYLAND_DISPLAY", home.join("wayland.sock"))
            .env("NEWPORT_GUI_WAYLAND_DISPLAY", home.join("wayland.sock"));
    }
}
fn finish(mut child: Child) -> std::process::Output {
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    while child.try_wait().unwrap().is_none() {
        if std::time::Instant::now() >= deadline {
            let _ = child.kill();
            let output = child.wait_with_output().unwrap();
            panic!("Ready display launch waited for another handoff: {output:?}");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    child.wait_with_output().unwrap()
}
#[test]
fn ready_display_claims_need_a_peer_with_attachment_checks() {
    for (x11, wayland) in [(true, false), (false, true), (true, true)] {
        let home = tempfile::tempdir().unwrap();
        let program = fixture(home.path());
        let mut agent = Agent::start(home.path());
        let cwd = home.path().join("working directory");
        fs::create_dir(&cwd).unwrap();
        let mut command = Command::new(BIN);
        command
            .args(["gui", "--program"])
            .arg(&program)
            .args(["--", "literal $(touch INJECTED)"])
            .env("HOME", home.path())
            .env("NEWPORT_TERMINAL", uuid::Uuid::new_v4().to_string())
            .env_remove("NEWPORT_SESSION")
            .current_dir(&cwd)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        owned_display(&mut command, home.path(), x11, wayland);
        let output = finish(command.spawn().unwrap());
        assert!(!output.status.success(), "{output:?}");
        assert!(output.stdout.is_empty());
        assert!(String::from_utf8_lossy(&output.stderr).contains("does not support"));
        assert!(!home.path().join("executed").exists());
        assert!(!cwd.join("INJECTED").exists());
        agent.send(b'H', &[]);
        agent.event(b'A'); // A GUI handoff would be an unexpected L event here.
        assert!(agent.events.try_recv().is_err());
    }
}

#[path = "support/gui_attachment.rs"]
mod attachment_checks;
#[test]
fn ready_shell_refresh_resolves_native_commands_and_keeps_other_terminals_shims() {
    let home = tempfile::tempdir().unwrap();
    let native = fixture(home.path());
    let mut agent = Agent::start(home.path());
    let terminal = uuid::Uuid::new_v4();
    let ordinary = exports(home.path(), terminal);
    let shim = home.path().join(".cache/newport/gui-bin/gui-fixture");
    assert!(shim.is_file());
    let old_shim = fs::read(&shim).unwrap();
    let mut prepare = Command::new(BIN);
    prepare
        .args(["env", "--shell", "sh"])
        .env("HOME", home.path())
        .env("NEWPORT_TERMINAL", terminal.to_string())
        .env_remove("NEWPORT_SESSION");
    owned_display(&mut prepare, home.path(), true, true);
    let output = prepare.output().unwrap();
    assert!(output.status.success(), "{output:?}");
    let refresh = String::from_utf8(output.stdout).unwrap();
    let mut command = Command::new("sh");
    command.args(["-c", &format!("{ordinary}\ngui-fixture --version >/dev/null\n{refresh}\ncommand -v gui-fixture\nexec gui-fixture 'new command'" )])
        .env("HOME", home.path()).env("NEWPORT_TERMINAL", terminal.to_string())
        .env_remove("NEWPORT_SESSION")
        .env("PATH", format!("{}:/usr/bin:/bin", home.path().join("bin").display()))
        .current_dir(home.path()).stdout(Stdio::piped()).stderr(Stdio::piped());
    owned_display(&mut command, home.path(), true, true);
    let output = finish(command.spawn().unwrap());
    assert_eq!(output.status.code(), Some(17), "{output:?}");
    assert!(
        output
            .stdout
            .starts_with(format!("{}\n", native.display()).as_bytes()),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    assert_eq!(
        fs::read(&shim).unwrap(),
        old_shim,
        "Another ordinary terminal may still use this shim"
    );
    agent.send(b'H', &[]);
    agent.event(b'A');
    assert!(agent.events.try_recv().is_err());
}
