use porthop_agent::wire;
use std::{
    fs,
    io::Write,
    process::{Child, ChildStdin, Command, Stdio},
    sync::mpsc,
    time::Duration,
};
const BIN: &str = env!("CARGO_BIN_EXE_porthop-agent");
struct Agent {
    child: Child,
    input: Option<ChildStdin>,
    events: mpsc::Receiver<(u8, Vec<u8>)>,
}
impl Drop for Agent {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
impl Agent {
    fn start(home: &std::path::Path) -> Self {
        Self::start_with(home, false)
    }
    fn start_with(home: &std::path::Path, native: bool) -> Self {
        Self::features(home, native, &["--clipboard", "--browser"])
    }
    fn features(home: &std::path::Path, native: bool, features: &[&str]) -> Self {
        let mut child = Command::new(BIN)
            .args(["serve", "test-client"])
            .args(features)
            .env("HOME", home)
            .env("PORTHOP_CLIPBOARD_NATIVE", if native { "1" } else { "0" })
            .env(
                "PATH",
                format!("{}:/usr/bin:/bin", home.join("bin").display()),
            )
            .env("WAYLAND_DISPLAY", "wayland-test")
            .env_remove("DISPLAY")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let input = child.stdin.take();
        let mut output = child.stdout.take().unwrap();
        let (tx, events) = mpsc::channel();
        std::thread::spawn(move || {
            while let Ok(event) = wire::read(&mut output) {
                if tx.send(event).is_err() {
                    break;
                }
            }
        });
        Self {
            child,
            input,
            events,
        }
    }
    fn event(&self, expected: u8) -> Vec<u8> {
        let (kind, data) = self
            .events
            .recv_timeout(Duration::from_secs(5))
            .expect("agent event");
        assert_eq!(kind, expected, "{}", String::from_utf8_lossy(&data));
        data
    }
    fn send(&mut self, kind: u8, data: &[u8]) {
        wire::write(self.input.as_mut().unwrap(), kind, data).unwrap();
    }
    fn stopped(&mut self) {
        for _ in 0..200 {
            if self.child.try_wait().unwrap().is_some() {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("agent did not stop");
    }
}
fn archive() -> Vec<u8> {
    let mut tar = tar::Builder::new(Vec::new());
    for (name, data) in [
        ("text/plain", b"hello from Mac".as_slice()),
        ("image/png", include_bytes!("pixel.png").as_slice()),
    ] {
        let mut header = tar::Header::new_gnu();
        header.set_size(data.len() as u64);
        header.set_mode(0o600);
        header.set_cksum();
        tar.append_data(&mut header, name, data).unwrap();
    }
    tar.into_inner().unwrap()
}
fn cmd(home: &std::path::Path, args: &[&str]) -> std::process::Output {
    Command::new(BIN)
        .args(args)
        .env("HOME", home)
        .output()
        .unwrap()
}
#[test]
fn single_agent_clipboard_browser_displays_and_disconnect_cleanup() {
    let home = tempfile::tempdir().unwrap();
    let mut agent = Agent::start(home.path());
    assert_eq!(agent.event(b'R'), wire::VERSION.as_bytes());
    agent.send(b'S', &archive());
    agent.event(b'A');
    let output = cmd(home.path(), &["clipboard", "-selection", "clipboard", "-o"]);
    assert!(output.status.success());
    assert_eq!(output.stdout, b"hello from Mac");
    let root = home.path().join(".cache/porthop/clipboard");
    assert!(root.join("Xauthority").is_file());
    assert!(root.join("display").is_file());
    std::env::set_var("WAYLAND_DISPLAY", root.join("wayland.sock"));
    std::env::remove_var("WAYLAND_SOCKET");
    let (mut png, _) = wl_clipboard_rs::paste::get_contents(
        wl_clipboard_rs::paste::ClipboardType::Regular,
        wl_clipboard_rs::paste::Seat::Unspecified,
        wl_clipboard_rs::paste::MimeType::Specific("image/png"),
    )
    .unwrap();
    let mut pixels = Vec::new();
    std::io::Read::read_to_end(&mut png, &mut pixels).unwrap();
    assert_eq!(pixels, include_bytes!("pixel.png"));
    let mut opener = Command::new(BIN)
        .args(["open", "https://example.com/login?code=123"])
        .env("HOME", home.path())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    assert_eq!(agent.event(b'O'), b"1\nhttps://example.com/login?code=123");
    assert!(opener.try_wait().unwrap().is_none());
    agent.send(b'B', b"1\nok");
    let opened = opener.wait_with_output().unwrap();
    assert!(
        opened.status.success(),
        "{}",
        String::from_utf8_lossy(&opened.stderr)
    );
    assert!(!cmd(home.path(), &["open", "file:///tmp/private"])
        .status
        .success());
    agent.send(b'H', &[]);
    agent.event(b'A');
    drop(agent.input.take());
    agent.stopped();
    assert!(!root.join("snapshot.tar").exists());
    assert!(!root.join("agent.sock").exists());
    assert!(!root.join("wayland.sock").exists());
    assert!(!cmd(home.path(), &["open", "https://example.com"])
        .status
        .success());
    let mut restarted = Agent::start(home.path());
    restarted.event(b'R');
    restarted.send(b'Q', &[]);
    restarted.stopped();
}
#[test]
fn duplicate_agent_cannot_take_over_or_delete_owner_snapshot() {
    let home = tempfile::tempdir().unwrap();
    let mut first = Agent::start(home.path());
    first.event(b'R');
    first.send(b'S', &archive());
    first.event(b'A');
    let mut second = Agent::start(home.path());
    second.event(b'T');
    second.stopped();
    assert!(cmd(home.path(), &["clipboard", "-o"]).status.success());
    let foreign = Command::new(BIN)
        .args(["serve", "different-client", "--clipboard"])
        .env("HOME", home.path())
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(!foreign.status.success());
    assert_eq!(wire::read(&mut foreign.stdout.as_slice()).unwrap().0, b'E');
    assert!(cmd(home.path(), &["clipboard", "-o"]).status.success());
    first.send(b'H', &[]);
    first.event(b'A');
}
#[test]
fn invalid_and_oversized_frames_stop_without_publishing() {
    for oversized in [false, true] {
        let home = tempfile::tempdir().unwrap();
        let mut agent = Agent::start(home.path());
        agent.event(b'R');
        if oversized {
            agent
                .input
                .as_mut()
                .unwrap()
                .write_all(&[b'S', 255, 255, 255, 255])
                .unwrap();
        } else {
            agent.send(b'S', b"not a tar archive");
        }
        agent.event(b'E');
        agent.stopped();
        assert!(!home
            .path()
            .join(".cache/porthop/clipboard/snapshot.tar")
            .exists());
    }
}
#[test]
fn installer_replaces_our_shims_but_preserves_unrelated_commands() {
    use std::os::unix::fs::symlink;
    let home = tempfile::tempdir().unwrap();
    let bin = home.path().join(".local/bin");
    fs::create_dir_all(&bin).unwrap();
    fs::write(
        bin.join("porthop-clip"),
        "#!/usr/bin/env bash\n# Porthop clipboard helper v5\n",
    )
    .unwrap();
    symlink("porthop-clip", bin.join("xclip")).unwrap();
    fs::write(bin.join("xdg-open"), "unrelated").unwrap();
    assert!(cmd(home.path(), &["install"]).status.success());
    assert_eq!(
        fs::read_link(bin.join("xclip")).unwrap().to_str(),
        Some("porthop-agent")
    );
    assert_eq!(
        fs::read_to_string(bin.join("xdg-open")).unwrap(),
        "unrelated"
    );
    assert!(!bin.join("porthop-clip").exists());
}
#[test]
fn web_requests_reject_unsafe_schemes_and_frames_preserve_binary_data() {
    for value in [
        "file:///tmp/a",
        "javascript:alert(1)",
        "https://u:p@host",
        "https://x\nhttps://y",
        "",
    ] {
        assert!(wire::web_url(value).is_none());
    }
    let data = [0, 255, 13, 10, 5];
    let frame = wire::encode(b'S', &data).unwrap();
    assert_eq!(
        wire::read(&mut frame.as_slice()).unwrap(),
        (b'S', data.to_vec())
    );
}

#[test]
fn native_publishing_prefers_wayland_then_x11_then_snapshot() {
    use std::os::unix::fs::PermissionsExt;
    for (wl, x, expected) in [(true, true, "wl"), (false, true, "x"), (false, false, "")] {
        let home = tempfile::tempdir().unwrap();
        let bin = home.path().join("bin");
        fs::create_dir(&bin).unwrap();
        for (name, works, label) in [("wl-copy", wl, "wl"), ("xclip", x, "x")] {
            let script = if works {
                format!(
                    "#!/bin/sh\ncat > \"$HOME/published\"\nprintf {label} > \"$HOME/backend\"\n"
                )
            } else {
                "#!/bin/sh\nexit 1\n".into()
            };
            let path = bin.join(name);
            fs::write(&path, script).unwrap();
            fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
        }
        let mut agent = Agent::start_with(home.path(), true);
        agent.event(b'R');
        agent.send(b'S', &archive());
        agent.event(b'A');
        assert_eq!(
            fs::read_to_string(home.path().join("backend")).unwrap_or_default(),
            expected
        );
        if !expected.is_empty() {
            assert_eq!(
                fs::read(home.path().join("published")).unwrap(),
                b"hello from Mac"
            );
        }
        assert_eq!(
            cmd(home.path(), &["clipboard", "-o"]).stdout,
            b"hello from Mac"
        );
        agent.send(b'Q', &[]);
        agent.stopped();
    }
}

#[test]
fn upload_installer_verifies_hash_and_preserves_unrelated_binary() {
    use std::os::unix::fs::PermissionsExt;
    let home = tempfile::tempdir().unwrap();
    let bin = home.path().join("bin");
    fs::create_dir(&bin).unwrap();
    // macOS lacks sha256sum; keep the production Linux installer unchanged.
    let shim = bin.join("sha256sum");
    fs::write(&shim, "#!/bin/sh\nexec shasum -a 256 \"$@\"\n").unwrap();
    fs::set_permissions(shim, fs::Permissions::from_mode(0o700)).unwrap();
    let digest = Command::new("shasum")
        .args(["-a", "256", BIN])
        .output()
        .unwrap();
    assert!(digest.status.success());
    let hash = String::from_utf8(digest.stdout)
        .unwrap()
        .split_whitespace()
        .next()
        .unwrap()
        .to_owned();
    let install = |hash: &str| {
        let mut process = Command::new("sh")
            .args([
                "-c",
                include_str!("../../../src-tauri/src/agent-install.sh"),
                "install",
                hash,
            ])
            .env("HOME", home.path())
            .env(
                "PATH",
                format!("{}:{}", bin.display(), std::env::var("PATH").unwrap()),
            )
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        process
            .stdin
            .take()
            .unwrap()
            .write_all(&fs::read(BIN).unwrap())
            .ok();
        process.wait().unwrap().success()
    };
    assert!(!install(&"0".repeat(64)));
    assert!(!home.path().join(".local/bin/porthop-agent").exists());
    assert!(install(&hash));
    assert_eq!(
        fs::read(home.path().join(".local/bin/porthop-agent")).unwrap(),
        fs::read(BIN).unwrap()
    );
    // Reinstall replaces an identical binary and repairs missing aliases.
    use std::os::unix::fs::MetadataExt;
    let agent_path = home.path().join(".local/bin/porthop-agent");
    let before = fs::metadata(&agent_path).unwrap().ino();
    fs::remove_file(home.path().join(".local/bin/porthop-browser")).unwrap();
    assert!(install(&hash));
    assert_ne!(fs::metadata(&agent_path).unwrap().ino(), before);
    assert_eq!(
        fs::read_link(home.path().join(".local/bin/porthop-browser")).unwrap(),
        std::path::Path::new("porthop-agent")
    );
    assert!(!install(&"0".repeat(64)));
    assert_eq!(fs::read(&agent_path).unwrap(), fs::read(BIN).unwrap());
    fs::write(
        home.path().join(".local/bin/porthop-agent"),
        b"unrelated command",
    )
    .unwrap();
    assert!(!install(&hash));
    assert_eq!(
        fs::read(home.path().join(".local/bin/porthop-agent")).unwrap(),
        b"unrelated command"
    );
}

#[test]
fn unrelated_socket_file_is_preserved_and_not_treated_as_reconnect() {
    use std::os::unix::fs::PermissionsExt;
    let home = tempfile::tempdir().unwrap();
    let root = home.path().join(".cache/porthop/clipboard");
    fs::create_dir_all(&root).unwrap();
    fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
    fs::write(root.join("agent-client"), "test-client").unwrap();
    fs::write(root.join("agent.sock"), "unrelated").unwrap();
    let mut agent = Agent::start(home.path());
    let error = String::from_utf8(agent.event(b'E')).unwrap();
    assert!(!error.contains("SSH transport interrupted"));
    agent.stopped();
    assert_eq!(
        fs::read_to_string(root.join("agent.sock")).unwrap(),
        "unrelated"
    );
}

#[test]
fn browser_only_does_not_create_clipboard_services_or_accept_snapshots() {
    let home = tempfile::tempdir().unwrap();
    let mut agent = Agent::features(home.path(), false, &["--browser"]);
    agent.event(b'R');
    let root = home.path().join(".cache/porthop/clipboard");
    for name in ["snapshot.tar", "wayland.sock", "display", "Xauthority"] {
        assert!(!root.join(name).exists(), "{name}");
    }
    let mut opener = Command::new(BIN)
        .args(["open", "https://example.com/#fragment"])
        .env("HOME", home.path())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    assert_eq!(agent.event(b'O'), b"1\nhttps://example.com/#fragment");
    agent.send(b'B', b"999\nok");
    agent.send(b'H', b"");
    agent.event(b'A');
    assert!(opener.try_wait().unwrap().is_none());
    agent.send(b'B', b"1\nCannot listen on callback port");
    let result = opener.wait_with_output().unwrap();
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("Cannot listen on callback port"));
    let env = Command::new(BIN)
        .arg("env")
        .env("HOME", home.path())
        .output()
        .unwrap();
    let env = String::from_utf8(env.stdout).unwrap();
    assert!(env.contains("porthop-browser"));
    assert!(!env.contains("DISPLAY"));
    agent.send(b'S', &archive());
    agent.event(b'E');
    agent.stopped();
    assert!(!root.join("snapshot.tar").exists());
}

#[test]
fn clipboard_only_rejects_browser_requests_without_interrupting_clipboard() {
    let home = tempfile::tempdir().unwrap();
    let mut agent = Agent::features(home.path(), false, &["--clipboard"]);
    agent.event(b'R');
    let opener = Command::new(BIN)
        .args(["open", "https://example.com"])
        .env("HOME", home.path())
        .output()
        .unwrap();
    assert!(!opener.status.success());
    agent.send(b'S', &archive());
    agent.event(b'A');
    let env = Command::new(BIN)
        .arg("env")
        .env("HOME", home.path())
        .output()
        .unwrap();
    let env = String::from_utf8(env.stdout).unwrap();
    assert!(env.contains("DISPLAY"));
    assert!(!env.contains("BROWSER"));
    agent.send(b'Q', &[]);
    agent.stopped();
}

#[test]
fn browser_warning_is_nonfatal_and_original_url_is_preserved() {
    let home = tempfile::tempdir().unwrap();
    let mut agent = Agent::features(home.path(), false, &["--browser"]);
    agent.event(b'R');
    let url =
        "https://example.com/login?redirect_uri=http%3A%2F%2Flocalhost%3A1455%2Fcallback#keep";
    let opener = Command::new(BIN)
        .args(["open", url])
        .env("HOME", home.path())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    assert_eq!(agent.event(b'O'), format!("1\n{url}").as_bytes());
    agent.send(b'B', b"1\nok\nBrowser opened without callback forwarding.");
    let output = opener.wait_with_output().unwrap();
    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("without callback forwarding"));
    agent.send(b'H', b"");
    agent.event(b'A');
}
