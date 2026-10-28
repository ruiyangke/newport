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

/// The fixture key in Credential Manager, for a packaged app that reads only
/// the OS store. Removed when the fixture ends.
#[cfg(windows)]
struct SeededKey(newport_platform::secrets::SystemStorage);
#[cfg(windows)]
impl Drop for SeededKey {
    fn drop(&mut self) {
        let _ = self.0.delete();
    }
}

struct Fixture {
    _directory: tempfile::TempDir,
    binary: PathBuf,
    profile: PathBuf,
    /// A test build of the app reads the key from this file.
    key: Option<PathBuf>,
    #[cfg(windows)]
    _seeded: Option<SeededKey>,
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
        let packaged = std::env::var_os("NEWPORT_TEST_APP").map(PathBuf::from);
        let key = packaged.is_none().then(|| {
            let key = directory.path().join("fixture-key");
            fs::write(&key, [7; 32]).unwrap();
            key
        });
        #[cfg(windows)]
        let seeded = packaged.is_some().then(|| {
            use newport_platform::secrets::KeyStorage;
            let storage = newport_backend::profile::credentials::system_storage(
                &profile.canonicalize().unwrap(),
            );
            storage.create(&[7; 32]).unwrap();
            SeededKey(storage)
        });
        #[cfg(not(windows))]
        assert!(
            packaged.is_none(),
            "A packaged app reads its vault key only from the OS store, which this test seeds on Windows"
        );
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
            key,
            #[cfg(windows)]
            _seeded: seeded,
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
        if let Some(key) = &self.key {
            command.env("NEWPORT_TEST_VAULT_KEY_FILE", key);
        }
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

    fn vault(&self) -> Vec<u8> {
        fs::read(self.profile.join("profiles.stronghold")).unwrap()
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
#[ignore = "Starts the real native executable; requires staged grammars and Windows PDFium"]
fn standalone_health_preserves_profile_and_releases_ownership() {
    let fixture = Fixture::new();
    let before = fixture.vault();
    for _ in 0..2 {
        fixture.health();
        assert_eq!(fixture.vault(), before);
    }
    let owner = Instance::acquire(&fixture.profile).unwrap().unwrap();
    let refused = fixture.run(&["--check-backend"]);
    assert!(!refused.status.success());
    assert!(String::from_utf8_lossy(&refused.stderr).contains("owns this profile"));
    assert_eq!(fixture.vault(), before);
    drop(owner);
    fixture.health();

    fs::write(
        fixture.profile.join("profiles.stronghold"),
        b"corrupt fixture",
    )
    .unwrap();
    let refused = fixture.run(&["--check-update-health"]);
    assert!(!refused.status.success());
    assert!(
        refused.stdout.is_empty(),
        "Unreadable profiles must not report ready"
    );
    assert_eq!(fixture.vault(), b"corrupt fixture");
    assert!(Instance::acquire(&fixture.profile).unwrap().is_some());
}

#[test]
#[ignore = "Starts the real native executable; requires staged grammars and Windows PDFium"]
fn standalone_upgrade_backs_up_the_profile_and_refuses_a_failed_backup() {
    let fixture = Fixture::new();
    let vault = fixture.vault();
    let preferences = br#"{"sidebarWidth":224,"futureSetting":{"keep":true}}"#;
    fs::write(fixture.profile.join("preferences.json"), preferences).unwrap();
    // A backup failure must happen before the vault key, metrics database or
    // workers are opened. Retry only after fixing the fixture blocker.
    fs::write(fixture.profile.join("upgrade-backups"), b"blocked").unwrap();
    let refused = fixture.run(&["--check-backend"]);
    assert!(!refused.status.success());
    assert_eq!(fixture.vault(), vault);
    assert!(!fixture.profile.join("metrics.sqlite3").exists());
    fs::remove_file(fixture.profile.join("upgrade-backups")).unwrap();
    fixture.health();
    let recovery = fixture.profile.join(format!(
        "upgrade-backups/native-{}/data",
        env!("CARGO_PKG_VERSION")
    ));
    assert_eq!(
        fs::read(recovery.join("profiles.stronghold")).unwrap(),
        vault
    );
    assert_eq!(
        fs::read(recovery.join("preferences.json")).unwrap(),
        preferences
    );
    assert_eq!(
        fs::read(fixture.profile.join("preferences.json")).unwrap(),
        preferences
    );
    fixture.health();
    assert_eq!(fixture.vault(), vault);
    assert_eq!(
        fs::read(recovery.join("profiles.stronghold")).unwrap(),
        vault
    );
}

#[cfg(target_os = "macos")]
#[test]
#[ignore = "Opens a disposable native window; requires an interactive macOS desktop"]
fn standalone_gui_activation_and_graceful_shutdown() {
    let fixture = Fixture::new();
    let before = fixture.vault();
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
            if let Ok(snapshot) =
                backend.call_with_timeout("snapshot", json!({}), Duration::from_secs(1))
            {
                break snapshot;
            }
            assert!(Instant::now() < deadline, "Backend did not become ready");
            thread::sleep(Duration::from_millis(50));
        };
        assert_eq!(snapshot["loadError"], Value::Null);
        assert_eq!(snapshot["config"], fixture.config);
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
        let after = backend.call("snapshot", json!({})).unwrap();
        assert_eq!(after["instanceId"], snapshot["instanceId"]);
        assert!(!fixture.run(&["--check-backend"]).status.success());
        assert!(fixture.run(&["--autostart"]).status.success());
        assert!(child.0.try_wait().unwrap().is_none());
        assert_eq!(unsafe { libc::kill(child.0.id() as i32, libc::SIGTERM) }, 0);
        assert!(child.wait().success(), "Graceful shutdown failed");
        assert!(Instance::acquire(&fixture.profile).unwrap().is_some());
        assert_eq!(fixture.vault(), before);
        assert!(fixture.profile.join("ui-state.json").is_file());
        fixture.health();
    }
}
