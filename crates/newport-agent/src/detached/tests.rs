use super::*;
use crate::linger::Linger;
use std::{
    fs,
    os::unix::{fs::PermissionsExt, process::ExitStatusExt},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

/// Stand-ins for `loginctl`, `systemctl` and `systemd-run` that record their
/// arguments in `log`. `loginctl show-user` reports the `linger` file, and
/// `enable-linger` fails while `refuse` exists; `systemctl` lists `scopes`.
struct Server {
    directory: tempfile::TempDir,
    state: PathBuf,
}

impl Server {
    fn new(lingers: bool) -> Self {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let state = root.join("state");
        fs::create_dir(&state).unwrap();
        fs::write(root.join("linger"), if lingers { "yes" } else { "no" }).unwrap();
        fs::write(root.join("scopes"), "").unwrap();
        let log = root.join("log").display().to_string();
        let dir = root.display().to_string();
        tool(
            root,
            "loginctl",
            &format!(
                r#"echo "loginctl $*" >> '{log}'
case "$1 $2" in
  show-user*) echo "Linger=$(cat '{dir}/linger')" ;;
  "--no-ask-password enable-linger") [ -e '{dir}/refuse' ] && exit 1; echo yes > '{dir}/linger' ;;
  "--no-ask-password disable-linger") echo no > '{dir}/linger' ;;
esac"#
            ),
        );
        tool(
            root,
            "systemctl",
            &format!("echo \"systemctl $*\" >> '{log}'\ncat '{dir}/scopes'"),
        );
        // With `slow`, it says it started, waits, and only then is its scope
        // listed, as a scope appears once systemd-run has run.
        tool(
            root,
            "systemd-run",
            &format!(
                r#"echo "systemd-run $*" >> '{log}'
if [ -e '{dir}/slow' ]; then
  touch '{dir}/started'; sleep 1
  echo "newport-display.scope loaded active running display" > '{dir}/scopes'
fi"#
            ),
        );
        Self { directory, state }
    }

    fn linger(&self) -> Linger {
        Linger {
            loginctl: self.path("loginctl").into(),
            systemctl: self.path("systemctl").into(),
            state: Some(self.state.clone()),
            uid: 1000,
        }
    }

    fn path(&self, name: &str) -> PathBuf {
        self.directory.path().join(name)
    }

    fn launch(&self) {
        launch_with(
            &self.linger(),
            self.path("systemd-run").as_os_str(),
            "newport-display",
            &["--display-process".to_owned()],
        )
        .unwrap();
    }

    fn launch_until(&self, deadline: Instant) -> io::Result<()> {
        launch_until_with(
            &self.linger(),
            self.path("systemd-run").as_os_str(),
            &self.path("direct"),
            "newport-display",
            &["--display-process".into()],
            deadline,
        )
    }

    fn direct(&self, body: &str) {
        tool(
            self.directory.path(),
            "direct",
            &format!("echo direct >> '{}';\n{body}", self.path("log").display(),),
        );
    }

    fn stalled(&self, name: &str) {
        tool(
            self.directory.path(),
            name,
            &format!(
                "echo '{name} $*' >> '{}'; echo $$ > '{}'; exec sleep 3",
                self.path("log").display(),
                self.path("pid").display(),
            ),
        );
    }

    fn lock_is_free(&self) {
        let file = fs::File::options()
            .write(true)
            .open(self.state.join("linger.lock"))
            .unwrap();
        fs2::FileExt::try_lock_exclusive(&file).expect("launch retained its linger lock");
    }

    fn child_was_reaped(&self) {
        let pid: libc::pid_t = fs::read_to_string(self.path("pid"))
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        // SAFETY: the fixture recorded the specific command this launch owned.
        assert_eq!(
            unsafe { libc::waitpid(pid, std::ptr::null_mut(), libc::WNOHANG) },
            -1
        );
        assert_eq!(
            io::Error::last_os_error().raw_os_error(),
            Some(libc::ECHILD)
        );
        // SAFETY: signal zero checks only whether that command is still present.
        assert_eq!(unsafe { libc::kill(pid, 0) }, -1, "launch command survived");
    }

    /// The commands run since the last call, with the arguments that matter.
    fn calls(&self) -> Vec<String> {
        let log = self.path("log");
        let calls = fs::read_to_string(&log).unwrap_or_default();
        let _ = fs::remove_file(&log);
        calls
            .lines()
            .map(|line| {
                if line.starts_with("systemd-run ") {
                    let unit = line
                        .split_whitespace()
                        .find(|a| a.starts_with("--unit="))
                        .unwrap();
                    format!("systemd-run {unit}")
                } else if line.starts_with("systemctl ") {
                    "systemctl list-units".to_owned()
                } else {
                    line.to_owned()
                }
            })
            .collect()
    }

    fn lingers(&self) -> bool {
        fs::read_to_string(self.path("linger")).unwrap().trim() == "yes"
    }

    fn marked(&self) -> bool {
        self.state.join("linger").exists()
    }
}

fn tool(directory: &Path, name: &str, body: &str) {
    let path = directory.join(name);
    fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
}

#[test]
fn a_launch_turns_lingering_on_and_marks_it_before_its_scope_starts() {
    let server = Server::new(false);
    server.launch();
    assert_eq!(
        server.calls(),
        [
            "loginctl show-user 1000 -p Linger",
            "loginctl --no-ask-password enable-linger 1000",
            "systemd-run --unit=newport-display",
        ]
    );
    assert!(server.lingers() && server.marked());
}

#[test]
fn an_account_that_lingered_already_is_left_alone() {
    let server = Server::new(true);
    server.launch();
    assert_eq!(
        server.calls(),
        [
            "loginctl show-user 1000 -p Linger",
            "systemd-run --unit=newport-display",
        ]
    );
    assert!(!server.marked());
    server.linger().reconcile_with(None);
    assert!(server.lingers(), "never turned off without the marker");
}

#[test]
fn an_account_that_may_not_linger_still_starts_its_scope_unmarked() {
    let server = Server::new(false);
    fs::write(server.path("refuse"), "").unwrap();
    server.launch();
    assert_eq!(
        server.calls(),
        [
            "loginctl show-user 1000 -p Linger",
            "loginctl --no-ask-password enable-linger 1000",
            "systemd-run --unit=newport-display",
        ]
    );
    assert!(!server.lingers() && !server.marked());
}

#[test]
fn lingering_newport_turned_on_stays_while_a_display_or_terminal_runs() {
    let server = Server::new(false);
    server.launch();
    server.calls();
    fs::write(
        server.path("scopes"),
        "newport-terminal-0123.scope loaded active running /newport-agent terminal holder\n",
    )
    .unwrap();
    server.linger().reconcile_with(None);
    assert_eq!(server.calls(), ["systemctl list-units"]);
    assert!(server.lingers() && server.marked());
}

#[test]
fn a_start_with_nothing_running_turns_off_the_lingering_newport_turned_on() {
    let server = Server::new(false);
    server.launch();
    server.calls();
    server.linger().reconcile_with(None);
    assert_eq!(
        server.calls(),
        [
            "systemctl list-units",
            "loginctl --no-ask-password disable-linger 1000",
        ]
    );
    assert!(!server.lingers() && !server.marked());
    server.linger().reconcile_with(None);
    assert!(
        server.calls().is_empty(),
        "nothing to do without the marker"
    );
}

#[test]
fn the_last_display_ending_turns_lingering_off() {
    let server = Server::new(false);
    server.launch();
    fs::write(
        server.path("scopes"),
        "newport-display.scope loaded active running /newport-agent --display-process\n",
    )
    .unwrap();
    server
        .linger()
        .reconcile_with(Some("newport-display.scope"));
    assert!(!server.lingers() && !server.marked());
}

#[test]
fn without_state_lingering_is_never_turned_on() {
    let server = Server::new(false);
    let linger = Linger {
        state: None,
        ..server.linger()
    };
    launch_with(
        &linger,
        server.path("systemd-run").as_os_str(),
        "newport-display",
        &[],
    )
    .unwrap();
    assert_eq!(server.calls(), ["systemd-run --unit=newport-display"]);
    assert!(!server.lingers());
}

#[test]
fn a_reconcile_while_a_display_starts_waits_for_its_scope() {
    let server = Server::new(false);
    server.launch();
    // Lingering is on and marked, from an earlier display that has ended.
    assert!(server.lingers() && server.marked());
    fs::write(server.path("slow"), "").unwrap();
    std::thread::scope(|scope| {
        let starting = scope.spawn(|| server.launch());
        while !server.path("started").exists() {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        // A held terminal ends while the display's scope doesn't exist yet.
        server.linger().reconcile_with(None);
        starting.join().unwrap();
    });
    assert!(server.lingers() && server.marked(), "{:?}", server.calls());
}

#[test]
fn display_launch_lock_contention_is_bounded_and_does_not_start_any_command() {
    let server = Server::new(false);
    server.direct("");
    let lock = fs::File::options()
        .create(true)
        .truncate(false)
        .write(true)
        .open(server.state.join("linger.lock"))
        .unwrap();
    fs2::FileExt::lock_exclusive(&lock).unwrap();
    let started = Instant::now();
    let error = server
        .launch_until(started + Duration::from_millis(100))
        .unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::TimedOut);
    assert!(started.elapsed() < Duration::from_secs(2));
    assert!(server.calls().is_empty());
    drop(lock);
    server
        .launch_until(Instant::now() + Duration::from_secs(2))
        .unwrap();
    server.lock_is_free();
}

#[test]
fn a_stalled_linger_query_is_reaped_without_launch_or_lock_leak() {
    let server = Server::new(false);
    server.direct("");
    server.stalled("loginctl");
    let started = Instant::now();
    let error = server
        .launch_until(started + Duration::from_secs(1))
        .unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::TimedOut);
    assert!(started.elapsed() < Duration::from_secs(2));
    assert_eq!(server.calls(), ["loginctl $*"]);
    assert!(!server.marked());
    server.child_was_reaped();
    server.lock_is_free();
}

#[test]
fn a_timed_out_linger_enable_keeps_its_marker_for_reconciliation() {
    let server = Server::new(false);
    server.direct("");
    let root = server.directory.path().display();
    tool(
        server.directory.path(),
        "loginctl",
        &format!(
            r#"
echo "loginctl $*" >> '{root}/log'
case "$1 $2" in
  show-user*) echo 'Linger=no' ;;
  '--no-ask-password enable-linger') echo yes > '{root}/linger'; echo $$ > '{root}/pid'; exec sleep 3 ;;
  '--no-ask-password disable-linger') echo no > '{root}/linger' ;;
esac"#
        ),
    );
    let error = server
        .launch_until(Instant::now() + Duration::from_secs(1))
        .unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::TimedOut);
    assert_eq!(
        server.calls(),
        [
            "loginctl show-user 1000 -p Linger",
            "loginctl --no-ask-password enable-linger 1000"
        ]
    );
    assert!(server.lingers() && server.marked());
    server.child_was_reaped();
    server.lock_is_free();
    server.linger().reconcile_with(None);
    assert!(!server.lingers() && !server.marked());
}

#[test]
fn a_timed_out_scope_is_not_replayed_directly_or_unlingered() {
    let server = Server::new(false);
    server.direct("");
    tool(
        server.directory.path(),
        "systemd-run",
        &format!(
            "echo \"systemd-run $*\" >> '{}'; echo $$ > '{}'; exec sleep 3",
            server.path("log").display(),
            server.path("pid").display(),
        ),
    );
    let started = Instant::now();
    // Allow the linger prerequisites to finish before the scope stalls.
    let error = server
        .launch_until(started + Duration::from_secs(1))
        .unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::TimedOut);
    assert!(started.elapsed() < Duration::from_secs(2));
    assert_eq!(
        server.calls(),
        [
            "loginctl show-user 1000 -p Linger",
            "loginctl --no-ask-password enable-linger 1000",
            "systemd-run --unit=newport-display"
        ]
    );
    assert!(server.lingers() && server.marked());
    server.child_was_reaped();
    server.lock_is_free();
}

#[test]
fn a_stalled_direct_launcher_uses_the_remaining_budget_and_is_reaped() {
    let server = Server::new(true);
    tool(server.directory.path(), "systemd-run", "sleep 0.1; exit 1");
    server.direct(&format!(
        "echo $$ > '{}'; exec sleep 3",
        server.path("pid").display()
    ));
    let started = Instant::now();
    // Allow the prerequisite processes to finish before the direct child stalls.
    let error = server
        .launch_until(started + Duration::from_secs(1))
        .unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::TimedOut);
    assert!(started.elapsed() < Duration::from_secs(2));
    assert_eq!(
        server.calls(),
        ["loginctl show-user 1000 -p Linger", "direct"]
    );
    server.child_was_reaped();
    server.lock_is_free();
}

#[test]
fn a_definitively_failed_or_unavailable_scope_can_launch_directly() {
    for missing in [false, true] {
        let server = Server::new(true);
        server.direct("");
        if missing {
            fs::remove_file(server.path("systemd-run")).unwrap();
        } else {
            tool(server.directory.path(), "systemd-run", "exit 1");
        }
        server
            .launch_until(Instant::now() + Duration::from_secs(2))
            .unwrap();
        assert_eq!(
            server.calls(),
            ["loginctl show-user 1000 -p Linger", "direct"]
        );
        server.lock_is_free();
    }
}

#[test]
fn an_interrupted_scope_is_uncertain_and_never_replayed() {
    let server = Server::new(true);
    server.direct("");
    tool(server.directory.path(), "systemd-run", "kill -TERM $$");
    let error = server
        .launch_until(Instant::now() + Duration::from_secs(2))
        .unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::Other);
    let exit = error
        .get_ref()
        .unwrap()
        .downcast_ref::<LaunchExit>()
        .unwrap();
    assert_eq!(exit.status().signal(), Some(libc::SIGTERM));
    assert!(error.to_string().contains(&exit.status().to_string()));
    assert_eq!(server.calls(), ["loginctl show-user 1000 -p Linger"]);
    server.lock_is_free();
}

#[test]
fn a_failed_direct_launcher_preserves_its_exit_code_or_signal() {
    for (script, code, signal) in [
        ("exit 7", Some(7), None),
        ("kill -TERM $$", None, Some(libc::SIGTERM)),
    ] {
        let server = Server::new(true);
        tool(server.directory.path(), "systemd-run", "exit 1");
        server.direct(script);
        let error = server
            .launch_until(Instant::now() + Duration::from_secs(2))
            .unwrap_err();
        let exit = error
            .get_ref()
            .unwrap()
            .downcast_ref::<LaunchExit>()
            .unwrap();
        assert_eq!(exit.status().code(), code);
        assert_eq!(exit.status().signal(), signal);
        assert!(error.to_string().contains(&exit.status().to_string()));
        assert_eq!(
            server.calls(),
            ["loginctl show-user 1000 -p Linger", "direct"]
        );
        server.lock_is_free();
    }
}

#[test]
fn a_chatty_linger_query_is_bounded_and_never_starts_the_display() {
    let server = Server::new(true);
    server.direct("");
    tool(
        server.directory.path(),
        "loginctl",
        &format!("echo $$ > '{}'; exec yes", server.path("pid").display(),),
    );
    let error = server
        .launch_until(Instant::now() + Duration::from_secs(2))
        .unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    assert!(server.calls().is_empty());
    server.child_was_reaped();
    server.lock_is_free();
}
