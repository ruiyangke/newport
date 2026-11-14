//! A real holder, driven over its socket as the desktop drives it.
#![cfg(target_os = "linux")]

use newport_protocol::terminal::{
    Computer, Exit,
    holder::{self as protocol, Attach, ClientMessage, ServerMessage, wire},
};

use std::{
    io::Read,
    os::unix::{fs::PermissionsExt, net::UnixStream},
    path::PathBuf,
    process::Command,
    time::{Duration, Instant},
};

struct Server {
    _root: tempfile::TempDir,
    home: PathBuf,
    runtime: PathBuf,
}

impl Server {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("home");
        let runtime = root.path().join("run");
        for dir in [&home, &runtime] {
            std::fs::create_dir(dir).unwrap();
            std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        Server {
            _root: root,
            home,
            runtime,
        }
    }

    fn directory(&self) -> PathBuf {
        self.runtime.join("newport-ipc/terminals")
    }

    /// Starts a holder as the worker does: the command and creator in the
    /// start file, then `terminal holder`, which detaches.
    fn start(&self, command: &str) -> Holder {
        self.start_with(command, false)
    }

    /// Starts a holder whose creating Mac will show the stable display, or
    /// not.
    fn start_with(&self, command: &str, stable_display: bool) -> Holder {
        let id = uuid::Uuid::new_v4().simple().to_string();
        let directory = self.directory();
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::set_permissions(
            self.runtime.join("newport-ipc"),
            std::fs::Permissions::from_mode(0o700),
        )
        .unwrap();
        std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700)).unwrap();
        let start = serde_json::json!({
            "command": command,
            "creator": { "id": "studio", "name": "Studio" },
            "stable_display": stable_display,
        });
        std::fs::write(directory.join(format!("{id}.start")), start.to_string()).unwrap();
        let status = Command::new(env!("CARGO_BIN_EXE_newport-agent"))
            .args(["terminal", "holder", &id, "80", "24"])
            .env("HOME", &self.home)
            .env("XDG_RUNTIME_DIR", &self.runtime)
            .status()
            .unwrap();
        assert!(status.success());
        let socket = directory.join(format!("{id}.sock"));
        until(
            || UnixStream::connect(&socket).is_ok(),
            "the holder's socket",
        );
        Holder {
            id,
            socket,
            directory,
        }
    }
}

struct Holder {
    id: String,
    socket: PathBuf,
    directory: PathBuf,
}

impl Holder {
    fn attach(&self) -> Client {
        self.attach_as("Studio")
    }
    /// Attaches as the named Mac, at 80×24.
    fn attach_as(&self, mac: &str) -> Client {
        self.connect(Attach {
            version: protocol::VERSION,
            columns: 80,
            rows: 24,
            computer: Some(Computer {
                id: uuid::Uuid::from_bytes([mac.len() as u8; 16]).to_string(),
                name: mac.into(),
            }),
            ..Default::default()
        })
    }
    fn connect(&self, hello: Attach) -> Client {
        let mut client = Client {
            stream: UnixStream::connect(&self.socket).unwrap(),
        };
        let ServerMessage::Hello(greeting) = client.next().unwrap() else {
            panic!("holder greeting missing")
        };
        greeting.require(&self.id).unwrap();
        client.send(ClientMessage::Attach(hello));
        client
    }
    fn record(&self) -> serde_json::Value {
        std::fs::read(self.directory.join(format!("{}.json", self.id)))
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    }
    fn files(&self) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(&self.directory)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.starts_with(&self.id))
            .collect();
        names.sort();
        names
    }
}

struct Client {
    stream: UnixStream,
}
impl Client {
    fn send(&mut self, message: ClientMessage) {
        wire::write_client(&mut self.stream, &message).unwrap();
    }
    fn input(&mut self, bytes: &[u8]) {
        for chunk in bytes.chunks(protocol::MAX_DATA) {
            self.send(ClientMessage::Input(chunk.to_vec().into()));
        }
    }
    fn next(&mut self) -> Option<ServerMessage> {
        self.stream
            .set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        wire::read_server(&mut self.stream).ok()
    }
    fn ended(&mut self) {
        assert!(matches!(
            wire::read_server(&mut self.stream),
            Err(wire::Error::Ended)
        ));
    }
    fn greeting(&mut self) -> Vec<u8> {
        assert!(matches!(
            self.next().unwrap(),
            ServerMessage::Attached { .. }
        ));
        // The shell's size, which the screen is drawn at, comes first.
        assert!(matches!(self.next().unwrap(), ServerMessage::Size { .. }));
        assert!(matches!(self.next().unwrap(), ServerMessage::Screen(_)));
        let ServerMessage::Full(full) = self.next().unwrap() else {
            panic!("missing full snapshot")
        };
        let mut decoded = Vec::new();
        flate2::read::DeflateDecoder::new(&full[..])
            .read_to_end(&mut decoded)
            .unwrap();
        decoded
    }
    /// The next shell size the holder tells this client.
    fn shell_size(&mut self) -> Option<(u16, u16)> {
        loop {
            if let ServerMessage::Size { columns, rows } = self.next()? {
                return Some((columns, rows));
            }
        }
    }
    fn active(&mut self) -> Option<bool> {
        loop {
            if let ServerMessage::Active(active) = self.next()? {
                return Some(active);
            }
        }
    }
    fn output_until(&mut self, text: &str) -> String {
        let mut seen = String::new();
        while !seen.contains(text) {
            if let ServerMessage::Output(bytes) = self
                .next()
                .unwrap_or_else(|| panic!("closed before {text:?}: {seen:?}"))
            {
                seen.push_str(&String::from_utf8_lossy(&bytes));
            }
        }
        seen
    }
}

fn until(mut check: impl FnMut() -> bool, what: &str) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !check() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn contains(haystack: &[u8], needle: &str) -> bool {
    String::from_utf8_lossy(haystack).contains(needle)
}

/// Runs each line typed, with no line editing: an interactive shell queries
/// the cursor position at each prompt, which would race the test's typing.
const SHELL: &str = r#"exec sh -c 'while IFS= read -r line; do eval "$line"; done'"#;

#[test]
fn a_shell_keeps_running_between_clients_and_comes_back_with_its_screen() {
    let server = Server::new();
    let holder = server.start(SHELL);
    let mut first = holder.attach();
    first.greeting();
    first.input(b"echo alpha-$((6*7))\n");
    first.output_until("alpha-42");
    drop(first);

    // Output while nobody is attached still reaches the replica.
    let marker = server.home.join("detached");
    let mut second = holder.attach();
    second.greeting();
    second.input(
        format!(
            "(sleep 0.3; echo beta-$((7*6)); touch {}) &\n",
            marker.display()
        )
        .as_bytes(),
    );
    drop(second);
    until(|| marker.exists(), "output while detached");
    std::thread::sleep(Duration::from_millis(100));

    let mut third = holder.attach();
    let snapshot = third.greeting();
    assert!(contains(&snapshot, "alpha-42"), "history survives");
    assert!(contains(&snapshot, "beta-42"), "output while detached");
    third.send(ClientMessage::End);
}

#[test]
fn several_macs_share_a_shell_and_the_one_last_used_sets_its_size() {
    let server = Server::new();
    let holder = server.start(SHELL);
    let mut studio = holder.attach_as("Studio");
    studio.greeting();
    assert_eq!(studio.active(), Some(true), "the only Mac is active");
    let mut air = holder.attach_as("Air");
    air.greeting();
    // Until someone uses one, the latest to attach is active.
    assert_eq!(air.active(), Some(true));
    assert_eq!(studio.active(), Some(false));
    until(
        || {
            holder.record()["attached"]
                .as_array()
                .is_some_and(|a| a.len() == 2)
        },
        "both Macs in the record",
    );
    assert_eq!(holder.record()["creator"]["name"], "Studio");

    // Input from either Mac reaches the shell, and output reaches both.
    air.input(b"echo from-$((2+3))\n");
    studio.output_until("from-5");
    air.output_until("from-5");

    // A resize, or input, doesn't take the shell: a tab sends input by itself
    // too, such as focus and mouse motion reports.
    studio.send(ClientMessage::Resize {
        columns: 100,
        rows: 30,
    });
    studio.input(b"true\n");
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(holder.record()["columns"], 80, "the Air is still active");

    studio.send(ClientMessage::Activate);
    // Every Mac draws the shell at its one size, as tmux's clients do: the
    // Air is told the Studio's size before any output drawn at it.
    assert_eq!(studio.shell_size(), Some((100, 30)));
    assert_eq!(studio.active(), Some(true));
    assert_eq!(air.shell_size(), Some((100, 30)));
    assert_eq!(air.active(), Some(false));
    until(|| holder.record()["columns"] == 100, "the Studio's size");
    let names = |record: serde_json::Value| -> Vec<(String, bool)> {
        record["attached"]
            .as_array()
            .unwrap()
            .iter()
            .map(|mac| (mac["name"].as_str().unwrap().into(), mac["active"] == true))
            .collect()
    };
    assert_eq!(
        names(holder.record()),
        [("Studio".into(), true), ("Air".into(), false)],
        "most recently used first"
    );

    air.send(ClientMessage::Activate);
    assert_eq!(air.shell_size(), Some((80, 24)));
    assert_eq!(air.active(), Some(true));
    assert_eq!(studio.shell_size(), Some((80, 24)));
    assert_eq!(studio.active(), Some(false));
    air.input(b"stty size\n");
    air.output_until("24 80");
    assert_eq!(
        names(holder.record()),
        [("Air".into(), true), ("Studio".into(), false)]
    );

    drop(air);
    until(
        || holder.record()["columns"] == 100,
        "the Studio's size once the Air leaves",
    );
    assert_eq!(studio.active(), Some(true));
    assert_eq!(names(holder.record()), [("Studio".into(), true)]);
    studio.send(ClientMessage::End);
}

#[test]
fn a_tab_someone_opens_is_active_at_once_and_a_reattach_is_not() {
    let server = Server::new();
    let holder = server.start(SHELL);
    let mut studio = holder.connect(Attach {
        version: protocol::VERSION,
        columns: 100,
        rows: 30,
        opened: true,
        computer: Some(Computer {
            id: uuid::Uuid::from_bytes([1; 16]).to_string(),
            name: "Studio".into(),
        }),
        ..Default::default()
    });
    studio.greeting();
    assert_eq!(studio.active(), Some(true));
    // The Air reconnects after a wake: it attaches, but nobody used it.
    let mut air = holder.attach_as("Air");
    air.greeting();
    assert_eq!(air.active(), Some(false));
    assert_eq!(holder.record()["columns"], 100);
    studio.send(ClientMessage::End);
}

#[test]
fn a_mac_that_becomes_active_at_another_size_gets_the_screen_at_its_size() {
    let server = Server::new();
    let holder = server.start(SHELL);
    let mut studio = holder.connect(Attach {
        version: protocol::VERSION,
        columns: 100,
        rows: 30,
        opened: true,
        computer: Some(Computer {
            id: uuid::Uuid::from_bytes([1; 16]).to_string(),
            name: "Studio".into(),
        }),
        ..Default::default()
    });
    studio.greeting();
    assert_eq!(studio.active(), Some(true));
    studio.input(b"echo mark-$((6*7))\n");
    studio.output_until("mark-42");
    // The Air's tab showed the Studio's 100x30 screen at 80x24.
    let mut air = holder.attach_as("Air");
    air.greeting();
    assert_eq!(air.active(), Some(false));

    air.send(ClientMessage::Activate);
    let mut frames = Vec::new();
    while !(frames
        .iter()
        .any(|frame| matches!(frame, ServerMessage::Full(_)))
        && frames
            .iter()
            .any(|frame| matches!(frame, ServerMessage::Active(_))))
    {
        frames.push(air.next().expect("the Air's frames"));
    }
    assert!(
        frames
            .iter()
            .any(|frame| matches!(frame, ServerMessage::Screen(_))),
        "{frames:?}"
    );
    until(|| holder.record()["columns"] == 80, "the Air's size");

    assert_eq!(studio.active(), Some(false));

    // Using it again changes nothing, so it gets no second screen, and
    // neither Mac is told anything new.
    air.send(ClientMessage::Activate);
    air.input(b"echo again-$((1+1))\n");
    for mac in [&mut air, &mut studio] {
        let mut seen = String::new();
        while !seen.contains("again-2") {
            let frame = mac.next().expect("the shell's output");
            assert!(
                !matches!(frame, ServerMessage::Screen(_)),
                "no new screen without a new size"
            );
            assert!(
                !matches!(frame, ServerMessage::Active(_)),
                "still the same active Mac"
            );
            if let ServerMessage::Output(bytes) = frame {
                seen.push_str(&String::from_utf8_lossy(&bytes));
            }
        }
    }
    studio.send(ClientMessage::End);
}

#[test]
fn the_record_carries_the_title_for_tabs_not_attached_yet() {
    let server = Server::new();
    let holder = server.start(SHELL);
    let mut client = holder.attach();
    client.greeting();
    client.input(b"printf '\\033]2;dev@host: ~\\007'\n");
    drop(client);
    until(|| holder.record()["title"] == "dev@host: ~", "the title");
    let mut client = holder.attach();
    client.greeting();
    client.send(ClientMessage::End);
}

#[test]
fn a_status_lasts_only_as_long_as_the_program_that_reported_it() {
    let server = Server::new();
    // Reports that it is working, then dies without clearing it, as an
    // agent killed with SIGKILL does. The record follows within a tick, so
    // it runs long enough to still be running after the check below.
    let agent = server.home.join("agent.sh");
    std::fs::write(
        &agent,
        "printf '\\033]7501;state=working:app=claude-code\\033\\\\'\nsleep 5\nkill -9 $$\n",
    )
    .unwrap();
    // Job control runs each command in its own foreground process group,
    // as an interactive shell does.
    let holder =
        server.start(r#"exec sh -c 'set -m; while IFS= read -r line; do eval "$line"; done'"#);
    let mut client = holder.attach();
    client.greeting();
    client.input(format!("sh {}\n", agent.display()).as_bytes());
    until(
        || holder.record()["agent"] == "state=working:app=claude-code",
        "the status",
    );
    std::thread::sleep(Duration::from_millis(1200));
    assert_eq!(
        holder.record()["agent"],
        "state=working:app=claude-code",
        "it lasts while its program runs"
    );
    until(
        || holder.record()["agent"].is_null(),
        "the status to go with its program",
    );
    client.output_until("\x1b]7501;state=clear");
    client.send(ClientMessage::End);
}

#[test]
fn the_status_producer_finds_its_reports_supported() {
    let server = Server::new();
    let producer = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/agent-status/osc7501.sh");
    let holder = server.start(&format!("sh {} probe; sleep 1000", producer.display()));
    let mut client = holder.attach();
    client.greeting();
    client.output_until("supported: the terminal answered OSC 7501 before DA1");
    client.send(ClientMessage::End);
}

#[test]
fn a_control_client_renames_the_shell_without_attaching() {
    let server = Server::new();
    let holder = server.start(SHELL);
    for (requested, normalized) in [("  build server ", "build server"), ("", "")] {
        let mut control = holder.connect(Attach {
            version: protocol::VERSION,
            columns: 80,
            rows: 24,
            control: true,
            ..Default::default()
        });
        assert!(matches!(
            control.next().unwrap(),
            ServerMessage::Attached { .. }
        ));
        control.send(ClientMessage::Rename(requested.into()));
        assert_eq!(
            control.next().unwrap(),
            ServerMessage::Renamed(normalized.into())
        );
        control.ended();
        let record = holder.record();
        if normalized.is_empty() {
            assert!(record["name"].is_null());
        } else {
            assert_eq!(record["name"], normalized);
        }
        assert_eq!(record["attached"], serde_json::json!([]));
    }
    let mut control = holder.connect(Attach {
        version: protocol::VERSION,
        columns: 80,
        rows: 24,
        control: true,
        ..Default::default()
    });
    assert!(matches!(
        control.next().unwrap(),
        ServerMessage::Attached { .. }
    ));
    control.send(ClientMessage::End);
    assert!(matches!(control.next().unwrap(), ServerMessage::Exit(_)));
    control.ended();
}

#[test]
fn a_failed_rename_is_not_acknowledged_or_left_in_memory() {
    let server = Server::new();
    let holder = server.start(SHELL);
    let record = holder.directory.join(format!("{}.json", holder.id));
    until(|| record.is_file(), "the initial record");
    std::fs::remove_file(&record).unwrap();
    std::fs::create_dir(&record).unwrap();
    let mut control = holder.connect(Attach {
        version: protocol::VERSION,
        columns: 80,
        rows: 24,
        control: true,
        ..Default::default()
    });
    assert!(matches!(
        control.next().unwrap(),
        ServerMessage::Attached { .. }
    ));
    control.send(ClientMessage::Rename("must not persist".into()));
    assert!(
        matches!(control.next().unwrap(),ServerMessage::Failed(error) if error.code=="rename_failed")
    );
    control.ended();
    std::fs::remove_dir(&record).unwrap();
    let mut viewer = holder.attach();
    viewer.greeting();
    until(|| record.is_file(), "the restored record");
    assert!(holder.record()["name"].is_null());
    viewer.send(ClientMessage::End);
}

#[test]
fn the_holder_answers_colour_queries_with_the_active_macs_colours() {
    let server = Server::new();
    let holder = server.start(SHELL);
    let mut client = holder.connect(Attach {
        version: protocol::VERSION,
        columns: 80,
        rows: 24,
        computer: Some(Computer {
            id: uuid::Uuid::from_bytes([1; 16]).to_string(),
            name: "Studio".into(),
        }),
        colors: Some([[0xee, 0xee, 0xee], [0x12, 0x34, 0x56], [0xff, 0, 0]]),
        ..Default::default()
    });
    client.greeting();
    let answer = server.home.join("answer");
    client.input(format!(
            "stty raw -echo; printf '\\033]11;?\\007'; dd bs=1 count=18 of={} 2>/dev/null; stty sane\n",
            answer.display()
        )
        .as_bytes());
    until(
        || std::fs::read(&answer).is_ok_and(|a| a.len() == 18),
        "the holder to answer",
    );
    assert_eq!(
        String::from_utf8_lossy(&std::fs::read(&answer).unwrap()),
        "\x1b]11;rgb:1212/3434"
    );
    client.send(ClientMessage::End);
}

#[test]
fn end_stops_the_shell_and_removes_the_files() {
    let server = Server::new();
    let holder = server.start("trap '' HUP; sleep 1000");
    let mut client = holder.attach();
    client.greeting();
    client.send(ClientMessage::End);
    let mut exit = None;
    while let Some(frame) = client.next() {
        if let ServerMessage::Exit(value) = frame {
            exit = Some(value);
        }
    }
    // The shell ignored the hangup, so the grace period ended in a kill.
    assert_eq!(exit, Some(Exit::Signal(libc::SIGKILL as u32)));
    until(|| holder.files().is_empty(), "the holder's files to go");
}

/// A field of `/proc/<pid>/stat` after the command name, which may itself
/// hold spaces and parentheses: 1 is the parent, 2 the process group.
fn stat_field(pid: libc::pid_t, field: usize) -> libc::pid_t {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).unwrap();
    let fields = &stat[stat.rfind(')').unwrap() + 2..];
    fields
        .split_whitespace()
        .nth(field)
        .unwrap()
        .parse()
        .unwrap()
}

fn parent_of(pid: libc::pid_t) -> libc::pid_t {
    stat_field(pid, 1)
}

fn alive(pid: libc::pid_t) -> bool {
    // A collected process is gone; a zombie still has its stat, in state Z.
    std::fs::read_to_string(format!("/proc/{pid}/stat"))
        .is_ok_and(|stat| !stat[stat.rfind(')').unwrap() + 2..].starts_with('Z'))
}

fn send_signal(pid: libc::pid_t, signal: libc::c_int) {
    // SAFETY: kill takes scalar arguments; the pid is one this test started.
    assert_eq!(
        unsafe { libc::kill(pid, signal) },
        0,
        "signal {signal} to {pid}"
    );
}

/// The running reapers of the terminal `id`.
fn reapers(id: &str) -> Vec<libc::pid_t> {
    let needle = format!("terminal\0reaper\0{id}\0");
    std::fs::read_dir("/proc")
        .unwrap()
        .flatten()
        .filter_map(|entry| entry.file_name().to_str()?.parse::<libc::pid_t>().ok())
        .filter(|&pid| {
            alive(pid)
                && std::fs::read(format!("/proc/{pid}/cmdline"))
                    .is_ok_and(|line| String::from_utf8_lossy(&line).contains(&needle))
        })
        .collect()
}

impl Holder {
    fn pid(&self) -> libc::pid_t {
        self.record()["pid"].as_i64().unwrap() as libc::pid_t
    }
}

impl Client {
    /// How the shell ended, once the holder says so and closes.
    fn exit(&mut self) -> Option<Exit> {
        let mut exit = None;
        while let Some(frame) = self.next() {
            if let ServerMessage::Exit(value) = frame {
                exit = Some(value);
            }
        }
        exit
    }
}

/// Leaves a process whose parent has exited, as the `( … ) &` of a cleanup
/// that is itself killed does; returns its pid.
fn orphan(server: &Server, client: &mut Client) -> libc::pid_t {
    let pid_file = server.home.join("orphan");
    client.input(
        format!(
            "(sleep 1000 & echo $! > {}.new; mv {0}.new {0})\n",
            pid_file.display()
        )
        .as_bytes(),
    );
    until(|| pid_file.exists(), "the orphan's pid");
    std::fs::read_to_string(&pid_file)
        .unwrap()
        .trim()
        .parse()
        .unwrap()
}

#[test]
fn a_kill_aimed_at_an_orphans_parent_never_ends_the_terminal() {
    let server = Server::new();
    let holder = server.start(SHELL);
    let mut client = holder.attach();
    client.greeting();
    let orphan = orphan(&server, &mut client);
    // `kill $(ps -o ppid= -p <orphan>)`: the cleanup's guess at a parent.
    until(
        || parent_of(orphan) != holder.pid() && reapers(&holder.id).contains(&parent_of(orphan)),
        "the reaper to adopt it",
    );
    let parent = parent_of(orphan);
    // `kill -- -<its group>` reaches the reaper alone, never the holder.
    let group = stat_field(parent, 2);
    assert_ne!(group, stat_field(holder.pid(), 2));
    send_signal(-group, libc::SIGTERM);
    for signal in [
        libc::SIGTERM,
        libc::SIGINT,
        libc::SIGHUP,
        libc::SIGQUIT,
        libc::SIGUSR1,
    ] {
        send_signal(parent, signal);
    }
    client.input(b"echo still-$((40 + 2))\n");
    client.output_until("still-42");
    assert!(
        alive(parent),
        "the reaper ignores what a mistaken kill sends"
    );
    // A reaper killed outright leaves the terminal running too: the holder
    // takes the shell back and still learns how it ends.
    send_signal(parent, libc::SIGKILL);
    until(|| !alive(parent), "the reaper to go");
    client.input(b"echo again-$((40 + 2))\n");
    client.output_until("again-42");
    client.send(ClientMessage::End);
    assert_eq!(client.exit(), Some(Exit::Signal(libc::SIGHUP as u32)));
    until(|| holder.files().is_empty(), "the holder's files to go");
}

#[test]
fn a_holder_ignores_stray_signals_and_ends_its_shell_on_sigterm() {
    let server = Server::new();
    let holder = server.start(SHELL);
    let mut client = holder.attach();
    client.greeting();
    for signal in [
        libc::SIGHUP,
        libc::SIGQUIT,
        libc::SIGTSTP,
        libc::SIGUSR1,
        libc::SIGUSR2,
        libc::SIGPIPE,
    ] {
        send_signal(holder.pid(), signal);
    }
    client.input(b"echo still-$((40 + 2))\n");
    client.output_until("still-42");
    send_signal(holder.pid(), libc::SIGTERM);
    // Ended as Close ends it: the shell hung up, every client told.
    assert_eq!(client.exit(), Some(Exit::Signal(libc::SIGHUP as u32)));
    until(|| holder.files().is_empty(), "the holder's files to go");
    until(|| reapers(&holder.id).is_empty(), "the reaper to leave");
}

#[test]
fn a_shell_starts_with_the_signals_of_one_started_anywhere_else() {
    // Shells ignore some signals for what they run, so the shell is compared
    // with one this test starts the same way, not with all defaults.
    const PROBE: &str = "grep -E '^Sig(Ign|Blk)' /proc/self/status";
    let started_here = Command::new("/bin/sh")
        .args(["-c", &format!("exec sh -c \"{PROBE}; true\"")])
        .output()
        .unwrap();
    let expected = String::from_utf8(started_here.stdout).unwrap();
    let server = Server::new();
    let holder = server.start(SHELL);
    let mut client = holder.attach();
    client.greeting();
    client.input(format!("{PROBE}; echo done-$((40 + 2))\n").as_bytes());
    let seen = client.output_until("done-42").replace('\r', "");
    for line in expected.lines() {
        assert!(seen.contains(line), "{line:?} missing from {seen:?}");
    }
    client.send(ClientMessage::End);
}

#[test]
fn the_reaper_leaves_with_the_shell_and_its_orphans() {
    let server = Server::new();
    let holder = server.start(SHELL);
    let mut client = holder.attach();
    client.greeting();
    let orphan = orphan(&server, &mut client);
    assert_eq!(reapers(&holder.id).len(), 1);
    client.send(ClientMessage::End);
    client.exit();
    // The orphan is in the shell's process group, so the hangup reaches it.
    until(|| !alive(orphan), "the orphan to end");
    until(|| reapers(&holder.id).is_empty(), "the reaper to leave");
}

#[test]
fn a_shell_that_exits_while_detached_reports_on_the_next_attach() {
    let server = Server::new();
    let holder = server.start("sleep 0.3; echo goodbye; exit 3");
    std::thread::sleep(Duration::from_millis(800));
    assert!(!holder.files().is_empty(), "kept for the next attach");
    let mut client = holder.attach();
    let snapshot = client.greeting();
    assert!(contains(&snapshot, "goodbye"));
    let exit = client.next().unwrap();
    assert_eq!(exit, ServerMessage::Exit(Exit::Code(3)));
    assert!(
        client.next().is_none(),
        "the holder ends once the exit is seen"
    );
    until(|| holder.files().is_empty(), "the holder's files to go");
}

#[test]
fn a_client_that_stops_reading_never_stalls_the_shell() {
    let server = Server::new();
    let holder = server.start(SHELL);
    let mut client = holder.attach();
    client.greeting();
    let done = server.home.join("done");
    client.input(
        format!(
            "head -c 30000000 /dev/zero | od; touch {}\n",
            done.display()
        )
        .as_bytes(),
    );
    // Never read: the holder drops this client instead of blocking.
    until(
        || done.exists(),
        "the command to finish despite a stalled client",
    );
    let mut fresh = holder.attach();
    fresh.greeting();
    fresh.send(ClientMessage::End);
}

#[test]
fn a_shell_whose_history_outgrows_one_snapshot_can_still_be_opened() {
    use std::fmt::Write;
    let server = Server::new();
    // Attributes and three colours on every cell: kilobytes a row once
    // written back, so a full scrollback passes the snapshot limit.
    let mut heavy = String::new();
    for i in 0..newport_terminal::SCROLLBACK_ROWS + 24 {
        let _ = write!(heavy, "row-{i:05}");
        for x in 9..80 {
            let _ = write!(
                heavy,
                "\x1b[1;3;4;38;2;{};{};{};48;2;{};{};{};58;2;{};{};{}mx",
                (i + x) % 256,
                (i * 3 + x) % 256,
                (i * 7 + x) % 256,
                (i * 11 + x) % 256,
                (i * 13 + x) % 256,
                (i * 17 + x) % 256,
                (i * 19 + x) % 256,
                (i * 23 + x) % 256,
                (i * 29 + x) % 256,
            );
        }
        heavy.push_str("\x1b[m\r\n");
    }
    assert!(
        heavy.len() > protocol::MAX_SNAPSHOT,
        "written back whole, the history would not fit one snapshot"
    );
    let file = server.home.join("heavy");
    std::fs::write(&file, heavy).unwrap();

    let holder = server.start(SHELL);
    let mut client = holder.attach();
    client.greeting();
    client.input(format!("cat {}\n", file.display()).as_bytes());
    drop(client);

    let last = format!("row-{:05}", newport_terminal::SCROLLBACK_ROWS + 23);
    let check = |snapshot: &[u8]| {
        assert!(
            snapshot.len() <= protocol::MAX_SNAPSHOT,
            "{}",
            snapshot.len()
        );
        assert!(contains(snapshot, "row-04500"), "recent history is kept");
        assert!(
            !contains(snapshot, "row-01000"),
            "older history makes room before the row limit would drop it"
        );
    };
    // Each attach until the holder has read it all must succeed too.
    let deadline = Instant::now() + Duration::from_secs(120);
    loop {
        let snapshot = holder.attach().greeting();
        assert!(snapshot.len() <= protocol::MAX_SNAPSHOT);
        if contains(&snapshot, &last) {
            check(&snapshot);
            break;
        }
        assert!(Instant::now() < deadline, "timed out waiting for {last}");
        std::thread::sleep(Duration::from_millis(250));
    }
    let mut again = holder.attach();
    check(&again.greeting());
    again.send(ClientMessage::End);
}

#[test]
fn programs_get_answers_to_terminal_queries_while_detached() {
    let server = Server::new();
    // Asks for the cursor position and records the reply.
    let answer = server.home.join("answer");
    let holder = server.start(&format!(
        "sleep 0.3; stty raw -echo; printf '\\033[6n'; dd bs=1 count=6 of={} 2>/dev/null; sleep 1000",
        answer.display()
    ));
    until(
        || std::fs::read(&answer).is_ok_and(|a| a.starts_with(b"\x1b[")),
        "the holder to answer the query",
    );
    let mut client = holder.attach();
    client.greeting();
    client.send(ClientMessage::End);
}

#[test]
fn files_are_private_to_the_account() {
    let server = Server::new();
    let holder = server.start(SHELL);
    for name in holder.files() {
        let mode = std::fs::metadata(holder.directory.join(&name))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o077, 0, "{name} is {mode:o}");
    }
    let mut client = holder.attach();
    client.greeting();
    client.send(ClientMessage::End);
}

#[test]
fn a_shell_started_while_the_stable_display_runs_gets_its_endpoints() {
    let server = Server::new();
    // The full agent's stable display, as it publishes itself while it runs.
    let display = server.runtime.join("newport-display");
    std::fs::create_dir(&display).unwrap();
    std::fs::set_permissions(&display, std::fs::Permissions::from_mode(0o700)).unwrap();
    let _listener = std::os::unix::net::UnixListener::bind(display.join("display.sock")).unwrap();
    let authority = server.runtime.join("xauth_newport-display");
    let endpoints = serde_json::json!({
        "WAYLAND_DISPLAY": "newport-display/wayland-0",
        "DISPLAY": ":42",
        "XAUTHORITY": authority,
        "XDG_RUNTIME_DIR": server.runtime,
        "DBUS_SESSION_BUS_ADDRESS": "unix:path=/stable/bus",
    });
    std::fs::write(display.join("environment.json"), endpoints.to_string()).unwrap();
    let holder = server.start_with(SHELL, true);
    let mut client = holder.attach();
    client.greeting();
    client.input(b"[ \"${DBUS_SESSION_BUS_ADDRESS%%;*}\" = \"unix:path=$XDG_RUNTIME_DIR/newport-display/bus\" ] && b=alias || b=other; \
          echo \"w=$WAYLAND_DISPLAY d=$DISPLAY a=${XAUTHORITY#$XDG_RUNTIME_DIR/} f=${NEWPORT_DISPLAY-unset} b=$b m=${NEWPORT_GUI_WAYLAND_DISPLAY-unset}\"\n");
    // Not its folder or its markers: what runs in the terminal follows the
    // terminal, and opens on the display as it is. Its bus is the display's
    // alias first, not the bus the display was started with.
    client.output_until(&format!(
        "w={}/wayland-0 d=:42 a=xauth_newport-display f=unset b=alias m=unset",
        display.display()
    ));
    client.send(ClientMessage::End);
}

/// What a held shell's display variables say, as one line.
const DISPLAY_VARIABLES: &[u8] = b"echo \"w=$WAYLAND_DISPLAY d=$DISPLAY a=${XAUTHORITY#$XDG_RUNTIME_DIR/} m=${NEWPORT_GUI_WAYLAND_DISPLAY-unset}\"\n";

#[test]
fn a_shell_the_stable_display_will_show_on_gets_its_aliases_before_it_runs() {
    let server = Server::new();
    let holder = server.start_with(SHELL, true);
    let display = server.runtime.join("newport-display");
    let mode = std::fs::metadata(&display).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o700);
    // The number reserved for the display to take.
    let number = std::fs::read_to_string(display.join("x11-display")).unwrap();
    let mut client = holder.attach();
    client.greeting();
    client.input(DISPLAY_VARIABLES);
    client.output_until(&format!(
        "w={}/wayland-0 d=:{number} a=xauth_newport-display m=unset",
        display.display()
    ));
    // No clipboard display of its own.
    assert!(!holder.files().iter().any(|name| name.ends_with(".wl")));
    client.send(ClientMessage::End);
}

#[test]
fn a_shell_the_stable_display_will_show_on_takes_its_recorded_x11_display() {
    let server = Server::new();
    let display = server.runtime.join("newport-display");
    std::fs::create_dir(&display).unwrap();
    std::fs::set_permissions(&display, std::fs::Permissions::from_mode(0o700)).unwrap();
    std::fs::write(display.join("x11-display"), "57").unwrap();
    let holder = server.start_with(SHELL, true);
    let mut client = holder.attach();
    client.greeting();
    client.input(DISPLAY_VARIABLES);
    client.output_until(&format!(
        "w={}/wayland-0 d=:57 a=xauth_newport-display m=unset",
        display.display()
    ));
    client.send(ClientMessage::End);
}

#[test]
fn a_shell_whose_computer_may_not_show_the_running_stable_display_gets_a_clipboard_display() {
    let server = Server::new();
    // Another computer shows the account's display.
    let display = server.runtime.join("newport-display");
    std::fs::create_dir(&display).unwrap();
    std::fs::set_permissions(&display, std::fs::Permissions::from_mode(0o700)).unwrap();
    let _listener = std::os::unix::net::UnixListener::bind(display.join("display.sock")).unwrap();
    let endpoints = serde_json::json!({
        "WAYLAND_DISPLAY": "newport-display/wayland-0",
        "DISPLAY": ":42",
        "XAUTHORITY": server.runtime.join("xauth_newport-display"),
        "XDG_RUNTIME_DIR": server.runtime,
        "NEWPORT_DISPLAY": display,
    });
    std::fs::write(display.join("environment.json"), endpoints.to_string()).unwrap();
    let holder = server.start(SHELL);
    let mut client = holder.attach();
    client.greeting();
    // Its own clipboard display, not the running display's.
    client
        .input(b"[ \"$DISPLAY\" = :42 ] && w=shared || w=${WAYLAND_DISPLAY##*.}; echo \"w=$w\"\n");
    client.output_until("w=wl");
    client.send(ClientMessage::End);
}

#[test]
fn a_shell_without_the_stable_display_gets_a_clipboard_display() {
    let server = Server::new();
    let holder = server.start(SHELL);
    let mut client = holder.attach();
    client.greeting();
    client.input(b"echo \"w=${WAYLAND_DISPLAY##*.}\"\n");
    client.output_until("w=wl");
    assert!(!server.runtime.join("newport-display").exists());
    client.send(ClientMessage::End);
}

/// What `terminal display` makes a plain terminal's shell say, as
/// `DISPLAY_VARIABLES` does, with markers a login left behind.
fn plain_display_variables(server: &Server) -> String {
    let output = Command::new("/bin/sh")
        .args([
            "-c",
            &format!(
                "eval \"$(\"$AGENT\" terminal display)\" && {}",
                String::from_utf8_lossy(DISPLAY_VARIABLES)
            ),
        ])
        .env("AGENT", env!("CARGO_BIN_EXE_newport-agent"))
        .env("HOME", &server.home)
        .env("XDG_RUNTIME_DIR", &server.runtime)
        .env("NEWPORT_GUI_WAYLAND_DISPLAY", "elsewhere")
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

#[test]
fn a_plain_terminal_names_the_display_as_a_held_shell_does_before_and_while_it_runs() {
    let server = Server::new();
    // Before the display runs: its aliases, with the X11 number reserved
    // for it, which a held shell started next agrees on.
    let plain = plain_display_variables(&server);
    let display = server.runtime.join("newport-display");
    let number = std::fs::read_to_string(display.join("x11-display")).unwrap();
    assert_eq!(
        plain,
        format!(
            "w={}/wayland-0 d=:{number} a=xauth_newport-display m=unset",
            display.display()
        )
    );
    let holder = server.start_with(SHELL, true);
    let mut client = holder.attach();
    client.greeting();
    client.input(DISPLAY_VARIABLES);
    client.output_until(&plain);
    client.send(ClientMessage::End);

    // While it runs: the endpoints it publishes.
    let _listener = std::os::unix::net::UnixListener::bind(display.join("display.sock")).unwrap();
    let endpoints = serde_json::json!({
        "WAYLAND_DISPLAY": "newport-display/wayland-0",
        "DISPLAY": ":42",
        "XAUTHORITY": server.runtime.join("xauth_newport-display"),
        "XDG_RUNTIME_DIR": server.runtime,
        "DBUS_SESSION_BUS_ADDRESS": "unix:path=/stable/bus",
    });
    std::fs::write(display.join("environment.json"), endpoints.to_string()).unwrap();
    assert_eq!(
        plain_display_variables(&server),
        format!(
            "w={}/wayland-0 d=:42 a=xauth_newport-display m=unset",
            display.display()
        )
    );
}

#[test]
fn premature_input_cannot_change_the_held_shell_or_replace_its_viewer() {
    let server = Server::new();
    let holder = server.start(SHELL);
    let mut viewer = holder.attach();
    viewer.greeting();
    let mut intruder = UnixStream::connect(&holder.socket).unwrap();
    intruder
        .set_read_timeout(Some(Duration::from_secs(3)))
        .unwrap();
    assert!(matches!(
        wire::read_server(&mut intruder).unwrap(),
        ServerMessage::Hello(_)
    ));
    wire::write_client(
        &mut intruder,
        &ClientMessage::Input(b"echo forged-marker\n".to_vec().into()),
    )
    .unwrap();
    assert!(wire::read_server(&mut intruder).is_err());
    viewer.input(b"echo intact-$((2+2))\n");
    let output = viewer.output_until("intact-4");
    assert!(!output.contains("forged-marker"));
    viewer.send(ClientMessage::End);
}
