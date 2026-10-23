use newport_agent::{gui_protocol as protocol, wire};
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::Path,
    process::{Child, ChildStdin, Command, Stdio},
    sync::mpsc,
    time::Duration,
};
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
        let (got, bytes) = self.events.recv_timeout(Duration::from_secs(5)).unwrap();
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
fn reply(id: uuid::Uuid) -> protocol::Reply {
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
fn waiting_launch_preserves_arguments_cwd_and_exit_code_and_executes_once() {
    let home = tempfile::tempdir().unwrap();
    fixture(home.path());
    let mut agent = Agent::start(home.path());
    let id = uuid::Uuid::new_v4();
    let mut process = launch(
        home.path(),
        id,
        &["space argument", "$(touch injected)", "", "--flag=value"],
    );
    let request: protocol::Request = protocol::decode(&agent.event(b'L')).unwrap();
    assert_eq!(request.terminal, id);
    assert_eq!(request.application, "gui-fixture");
    assert!(process.try_wait().unwrap().is_none());
    assert!(!home.path().join("executed").exists());
    // Account/setup waiting never blocks integration heartbeats.
    agent.send(b'H', &[]);
    agent.event(b'A');
    let bytes = protocol::encode(&reply(request.id)).unwrap();
    agent.send(b'J', &bytes);
    agent.event(b'A');
    let output = process.wait_with_output().unwrap();
    assert_eq!(output.status.code(), Some(17));
    let fields: Vec<_> = output
        .stdout
        .split(|b| *b == 0)
        .map(|s| String::from_utf8_lossy(s).to_string())
        .collect();
    assert_eq!(
        fields,
        vec![
            home.path()
                .join("working directory")
                .canonicalize()
                .unwrap()
                .display()
                .to_string(),
            ":75".into(),
            "/tmp/private/Xauthority".into(),
            "unset".into(),
            "space argument".into(),
            "$(touch injected)".into(),
            "".into(),
            "--flag=value".into(),
            "".into()
        ]
    );
    agent.send(b'J', &bytes);
    agent.event(b'A');
    assert_eq!(
        fs::read_to_string(home.path().join("executed")).unwrap(),
        "executed\n"
    );
    assert!(!home.path().join("working directory/injected").exists());
}
#[test]
fn closing_a_waiting_process_cancels_and_late_authorization_does_not_execute() {
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
    agent.send(b'J', &protocol::encode(&reply(request.id)).unwrap());
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
fn invalid_display_reply_cannot_change_executable_or_environment() {
    let home = tempfile::tempdir().unwrap();
    fixture(home.path());
    let mut agent = Agent::start(home.path());
    let process = launch(home.path(), uuid::Uuid::new_v4(), &[]);
    let request: protocol::Request = protocol::decode(&agent.event(b'L')).unwrap();
    let mut reply = reply(request.id);
    reply
        .environment
        .as_mut()
        .unwrap()
        .insert("LD_PRELOAD".into(), Some("/tmp/evil".into()));
    agent.send(b'J', &protocol::encode(&reply).unwrap());
    agent.event(b'A');
    let output = process.wait_with_output().unwrap();
    assert!(!output.status.success());
    assert!(!home.path().join("executed").exists());
    assert!(String::from_utf8_lossy(&output.stderr).contains("Invalid GUI display environment"));
}

#[test]
fn terminal_started_before_integrations_can_still_handoff_its_first_app() {
    let home = tempfile::tempdir().unwrap();
    fixture(home.path());
    let terminal = uuid::Uuid::new_v4();
    let process = launch(home.path(), terminal, &[]);
    assert!(home
        .path()
        .join(".cache/newport/gui-bin/gui-fixture")
        .is_file());
    let mut agent = Agent::start(home.path());
    let request: protocol::Request = protocol::decode(&agent.event(b'L')).unwrap();
    assert_eq!(request.terminal, terminal);
    agent.send(b'J', &protocol::encode(&reply(request.id)).unwrap());
    agent.event(b'A');
    assert_eq!(process.wait_with_output().unwrap().status.code(), Some(17));
}
