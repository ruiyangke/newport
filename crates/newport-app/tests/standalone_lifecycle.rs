//! Opt-in checks against the actual standalone executable and a disposable profile.
use newport_backend::profile::{Instance, Store};
use serde_json::{Value, json};
use std::{
    fs,
    path::PathBuf,
    process::{Child, Command, Output, Stdio},
    thread,
    time::{Duration, Instant},
};

struct Process(Child);
impl Process {
    fn wait(&mut self) -> std::process::ExitStatus {
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            if let Some(status) = self.0.try_wait().unwrap() {
                return status;
            }
            assert!(Instant::now() < deadline, "Standalone process timed out");
            thread::sleep(Duration::from_millis(25));
        }
    }
}
impl Drop for Process {
    fn drop(&mut self) {
        // Only this fixture's child; always reap before its profile is removed.
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

struct Fixture {
    _directory: tempfile::TempDir,
    binary: PathBuf,
    profile: PathBuf,
    config: Value,
}
impl Fixture {
    fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        // Exercise the short-endpoint fallback through a real executable on Unix.
        #[cfg(unix)]
        let profile = directory.path().join("isolated profile ".repeat(8));
        #[cfg(not(unix))]
        let profile = directory.path().join("isolated profile");
        fs::create_dir(&profile).unwrap();
        newport_platform::filesystem::protect_directory(directory.path()).unwrap();
        newport_platform::filesystem::protect_directory(&profile).unwrap();
        // A profile without passwords or a sign-in needs no credential storage,
        // so a packaged app runs against it too.
        let packaged = std::env::var_os("NEWPORT_TEST_APP").map(PathBuf::from);
        let binary = packaged.unwrap_or_else(|| PathBuf::from(env!("CARGO_BIN_EXE_newport")));
        assert!(binary.is_absolute() && binary.is_file());
        let config = json!({"servers":[{
            "id":"8d43df10-73f7-4358-b818-c41dddc2006e",
            "name":"Lifecycle fixture", "sshHost":"127.0.0.1", "sshPort":1,
            "sshUser":"fixture", "clipboardEnabled":false, "browserEnabled":false
        }], "tunnels":[]});
        let store = Store::for_test(profile.clone());
        let parsed = serde_json::from_value(config).unwrap();
        store.save(&parsed).unwrap();
        let fixture = Self {
            _directory: directory,
            binary,
            profile,
            config: serde_json::to_value(parsed).unwrap(),
        };
        // Fail before launching a GUI if an override names a different app.
        let output = fixture.run(&["--build-info"]);
        assert!(output.status.success());
        let info: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(info["identifier"], "app.newport");
        assert_eq!(info["version"], env!("CARGO_PKG_VERSION"));
        assert_eq!(info["os"], std::env::consts::OS);
        fixture
    }

    fn command(&self) -> Command {
        let mut command = Command::new(&self.binary);
        command
            .env("NEWPORT_DATA_DIR", &self.profile)
            .stdin(Stdio::null());
        command
    }

    fn run(&self, args: &[&str]) -> Output {
        // Files avoid pipe-capacity deadlocks if startup diagnostics are verbose.
        let stdout = tempfile::tempfile().unwrap();
        let stderr = tempfile::tempfile().unwrap();
        let mut child = Process(
            self.command()
                .args(args)
                .stdout(stdout.try_clone().unwrap())
                .stderr(stderr.try_clone().unwrap())
                .spawn()
                .unwrap(),
        );
        let status = child.wait();
        fn read(mut file: fs::File) -> Vec<u8> {
            use std::io::{Read, Seek};
            file.rewind().unwrap();
            let mut bytes = Vec::new();
            file.take(1024 * 1024).read_to_end(&mut bytes).unwrap();
            bytes
        }
        Output {
            status,
            stdout: read(stdout),
            stderr: read(stderr),
        }
    }

    fn profiles(&self) -> Vec<u8> {
        fs::read(self.profile.join("profiles.json")).unwrap()
    }

    fn health(&self) {
        let output = self.run(&["--check-update-health"]);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let receipt: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(
            receipt,
            json!({"protocol":"newport-update-health", "schema":1,
            "version":env!("CARGO_PKG_VERSION"), "status":"ready"})
        );
        assert!(Instance::acquire(&self.profile).unwrap().is_some());
        assert_eq!(
            serde_json::to_value(Store::for_test(self.profile.clone()).load().unwrap()).unwrap(),
            self.config
        );
    }
}

#[test]
#[ignore = "Starts the real native executable; requires staged grammars"]
fn standalone_health_preserves_profile_and_releases_ownership() {
    let fixture = Fixture::new();
    let before = fixture.profiles();
    for _ in 0..2 {
        fixture.health();
        assert_eq!(fixture.profiles(), before);
    }
    let owner = Instance::acquire(&fixture.profile).unwrap().unwrap();
    let refused = fixture.run(&["--check-backend"]);
    assert!(!refused.status.success());
    assert!(String::from_utf8_lossy(&refused.stderr).contains("owns this profile"));
    assert_eq!(fixture.profiles(), before);
    drop(owner);
    fixture.health();

    fs::write(fixture.profile.join("profiles.json"), b"corrupt fixture").unwrap();
    let refused = fixture.run(&["--check-update-health"]);
    assert!(!refused.status.success());
    assert!(
        refused.stdout.is_empty(),
        "Unreadable profiles must not report ready"
    );
    assert_eq!(fixture.profiles(), b"corrupt fixture");
    assert!(Instance::acquire(&fixture.profile).unwrap().is_some());
}

#[test]
#[ignore = "Starts the real native executable; requires staged grammars"]
fn standalone_upgrade_backs_up_the_profile_and_refuses_a_failed_backup() {
    let fixture = Fixture::new();
    let profiles = fixture.profiles();
    let preferences = br#"{"sidebarWidth":224,"futureSetting":{"keep":true}}"#;
    fs::write(fixture.profile.join("preferences.json"), preferences).unwrap();
    // A backup failure must happen before credential storage, the metrics
    // database or workers are opened. Retry only after fixing the fixture blocker.
    fs::write(fixture.profile.join("upgrade-backups"), b"blocked").unwrap();
    let refused = fixture.run(&["--check-backend"]);
    assert!(!refused.status.success());
    assert_eq!(fixture.profiles(), profiles);
    assert!(!fixture.profile.join("metrics.sqlite3").exists());
    fs::remove_file(fixture.profile.join("upgrade-backups")).unwrap();
    fixture.health();
    let recovery = fixture.profile.join(format!(
        "upgrade-backups/native-{}/data",
        env!("CARGO_PKG_VERSION")
    ));
    assert_eq!(fs::read(recovery.join("profiles.json")).unwrap(), profiles);
    assert_eq!(
        fs::read(recovery.join("preferences.json")).unwrap(),
        preferences
    );
    assert_eq!(
        fs::read(fixture.profile.join("preferences.json")).unwrap(),
        preferences
    );
    fixture.health();
    assert_eq!(fixture.profiles(), profiles);
    assert_eq!(fs::read(recovery.join("profiles.json")).unwrap(), profiles);
}

#[cfg(target_os = "macos")]
#[test]
#[ignore = "Opens a disposable native window; requires an interactive macOS desktop"]
fn standalone_gui_activation_and_graceful_shutdown() {
    use newport_protocol::{
        common::Present,
        desktop::{Operation, Reply, configuration},
    };
    fn read_snapshot(
        backend: &newport_client::backend::Backend,
        timeout: Duration,
    ) -> Result<configuration::Snapshot, String> {
        let Reply::Configuration(reply) = backend
            .execute_with_timeout(
                Operation::Configuration(Box::new(configuration::Operation::Snapshot(Box::new(
                    Present {},
                )))),
                timeout,
            )
            .map_err(|error| error.to_string())?
        else {
            return Err("Expected snapshot".into());
        };
        let configuration::Reply::Snapshot(snapshot) = *reply else {
            return Err("Expected snapshot".into());
        };
        Ok(*snapshot)
    }
    let fixture = Fixture::new();
    let before = fixture.profiles();
    let backend = newport_client::backend::Backend::at_path(
        newport_platform::ipc::profile_endpoint(&fixture.profile),
    );
    for args in [vec![], vec!["--autostart"]] {
        let mut child = Process(fixture.command().args(args).spawn().unwrap());
        let deadline = Instant::now() + Duration::from_secs(30);
        let snapshot = loop {
            assert!(
                child.0.try_wait().unwrap().is_none(),
                "App exited before readiness"
            );
            if let Ok(snapshot) = read_snapshot(&backend, Duration::from_secs(1)) {
                break snapshot;
            }
            assert!(Instant::now() < deadline, "Backend did not become ready");
            thread::sleep(Duration::from_millis(50));
        };
        assert!(snapshot.load_error.is_none());
        assert_eq!(
            snapshot.config.servers.len(),
            fixture.config["servers"].as_array().unwrap().len()
        );
        assert_eq!(
            snapshot.config.tunnels.len(),
            fixture.config["tunnels"].as_array().unwrap().len()
        );
        for (actual, expected) in snapshot
            .config
            .servers
            .iter()
            .zip(fixture.config["servers"].as_array().unwrap())
        {
            assert_eq!(actual.id.to_string(), expected["id"].as_str().unwrap());
            assert_eq!(actual.name, expected["name"].as_str().unwrap());
            assert_eq!(actual.ssh_host, expected["sshHost"].as_str().unwrap());
            assert_eq!(actual.ssh_user, expected["sshUser"].as_str().unwrap());
            assert_eq!(
                u64::from(actual.ssh_port),
                expected["sshPort"].as_u64().unwrap()
            );
        }
        assert!(Instance::acquire(&fixture.profile).unwrap().is_none());
        // ACK requires dispatch through the real GPUI main thread, not just RPC startup.
        let activation = fixture.run(&[]);
        assert!(
            activation.status.success(),
            "{}",
            String::from_utf8_lossy(&activation.stderr)
        );
        // Activation is acknowledged before the next frame is painted. Give
        // the interactive acceptance window time to render before shutdown.
        thread::sleep(Duration::from_millis(1500));
        let after = read_snapshot(&backend, Duration::from_secs(10)).unwrap();
        assert_eq!(after.instance_id, snapshot.instance_id);
        assert!(!fixture.run(&["--check-backend"]).status.success());
        assert!(fixture.run(&["--autostart"]).status.success());
        assert!(child.0.try_wait().unwrap().is_none());
        assert_eq!(unsafe { libc::kill(child.0.id() as i32, libc::SIGTERM) }, 0);
        assert!(child.wait().success(), "Graceful shutdown failed");
        assert!(Instance::acquire(&fixture.profile).unwrap().is_some());
        assert_eq!(fixture.profiles(), before);
        assert!(fixture.profile.join("ui-state.json").is_file());
        fixture.health();
    }
}

#[cfg(windows)]
#[test]
#[ignore = "Opens a disposable native window; requires an interactive Windows desktop"]
fn standalone_gui_closing_hides_the_window_and_a_second_launch_reopens_it() {
    use windows_sys::Win32::{
        Foundation::{HWND, LPARAM},
        UI::WindowsAndMessaging::{
            EnumWindows, GetWindowTextW, GetWindowThreadProcessId, IsWindowVisible, PostMessageW,
            WM_CLOSE,
        },
    };

    /// The visible top-level window titled "Newport" that `pid` owns.
    fn window(pid: u32) -> Option<HWND> {
        struct Search {
            pid: u32,
            found: Option<HWND>,
        }
        unsafe extern "system" fn visit(window: HWND, search: LPARAM) -> i32 {
            // SAFETY: `search` is the `Search` that `window()` passes to
            // EnumWindows, which outlives the enumeration.
            let search = unsafe { &mut *(search as *mut Search) };
            let mut owner = 0;
            let mut title = [0u16; 256];
            // SAFETY: `window` comes from EnumWindows, and the buffer's length
            // is the one passed.
            let (length, visible) = unsafe {
                GetWindowThreadProcessId(window, &mut owner);
                (
                    GetWindowTextW(window, title.as_mut_ptr(), title.len() as i32),
                    IsWindowVisible(window) != 0,
                )
            };
            let title = String::from_utf16_lossy(&title[..length.max(0) as usize]);
            if owner == search.pid && visible && title == "Newport" {
                search.found = Some(window);
                return 0;
            }
            1
        }
        let mut search = Search { pid, found: None };
        // SAFETY: `visit` only reads `search` during this call.
        unsafe { EnumWindows(Some(visit), &mut search as *mut Search as LPARAM) };
        search.found
    }
    fn visible(window: HWND) -> bool {
        // SAFETY: IsWindowVisible accepts any handle, including a destroyed one.
        unsafe { IsWindowVisible(window) != 0 }
    }
    /// Polls `probe` until it holds or `timeout` passes.
    fn eventually(timeout: Duration, mut probe: impl FnMut() -> bool) -> bool {
        let deadline = Instant::now() + timeout;
        loop {
            if probe() {
                return true;
            }
            if Instant::now() >= deadline {
                return false;
            }
            thread::sleep(Duration::from_millis(100));
        }
    }

    let fixture = Fixture::new();
    let mut first = Process(
        fixture
            .command()
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let pid = first.0.id();
    assert!(
        eventually(Duration::from_secs(30), || window(pid).is_some()),
        "The first window did not appear"
    );
    // Let startup finish before closing, as a person would.
    thread::sleep(Duration::from_secs(3));
    let handle = window(pid).expect("The first window closed during startup");
    // SAFETY: `handle` is a live top-level window of the child process.
    unsafe { PostMessageW(handle, WM_CLOSE, 0, 0) };
    assert!(
        eventually(Duration::from_secs(5), || !visible(handle)),
        "Closing the window did not hide it"
    );
    assert!(
        first.0.try_wait().unwrap().is_none(),
        "Closing the window quit the app"
    );

    let second = fixture.run(&[]);
    assert!(
        second.status.success(),
        "{}",
        String::from_utf8_lossy(&second.stderr)
    );
    assert!(
        eventually(Duration::from_secs(5), || visible(handle)),
        "A second launch did not reopen the hidden window"
    );
    assert!(first.0.try_wait().unwrap().is_none());
}
