//! Opt-in real-backend lifecycle acceptance; only opens disposable loopback PTYs.
use newport_client::{
    backend::Backend,
    terminal_transport::{Event, Session},
};
use serde_json::json;
use std::{
    io::{BufRead, Write},
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};

fn ready(session: &Session) -> Result<(), String> {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        match session.try_read() {
            Ok(Event::Ready) => return Ok(()),
            Ok(Event::Error(error)) => return Err(error),
            Ok(Event::Exit(_)) => return Err("Terminal exited before ready".into()),
            _ => {}
        }
        if Instant::now() >= deadline {
            return Err("Terminal did not become ready".into());
        }
        thread::sleep(Duration::from_millis(5));
    }
}
#[test]
#[ignore = "child process helper for the explicitly opted-in lifecycle test"]
fn terminal_lease_child() -> Result<(), String> {
    let server = std::env::var("NEWPORT_TEST_LEASE_SERVER").map_err(|e| e.to_string())?;
    let session = Session::open(&server, 80, 24)?;
    ready(&session)?;
    println!("LEASE_READY {}", session.id());
    std::io::stdout().flush().unwrap();
    loop {
        thread::park();
    }
}
struct OwnedChild(Child);
impl Drop for OwnedChild {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
#[test]
#[ignore = "requires running backend and explicit disposable loopback SSH fixture"]
fn crashed_native_clients_release_ptys_without_closing_other_sessions() -> Result<(), String> {
    let user =
        std::env::var("NEWPORT_TEST_LEASE_USER").map_err(|_| "Set NEWPORT_TEST_LEASE_USER")?;
    let port: u16 = std::env::var("NEWPORT_TEST_LEASE_PORT")
        .map_err(|_| "Set NEWPORT_TEST_LEASE_PORT")?
        .parse()
        .map_err(|_| "Invalid fixture port")?;
    let backend = Backend::discover();
    let snapshot = backend.call("snapshot", json!({}))?;
    let matching: Vec<_> = snapshot["config"]["servers"]
        .as_array()
        .ok_or("Missing servers")?
        .iter()
        .filter(|s| s["sshHost"] == "127.0.0.1" && s["sshUser"] == user && s["sshPort"] == port)
        .collect();
    if matching.len() != 1 {
        return Err("Expected one exact disposable loopback SSH fixture".into());
    }
    let server = matching[0]["id"].as_str().ok_or("Missing server ID")?;
    let survivor = Session::open(server, 80, 24)?;
    ready(&survivor)?;
    // Exceeds the backend's eight-session cap without touching existing PTYs.
    for round in 0..12 {
        let mut child = OwnedChild(
            Command::new(std::env::current_exe().map_err(|e| e.to_string())?)
                .args([
                    "--ignored",
                    "--exact",
                    "terminal_lease_child",
                    "--nocapture",
                ])
                .env("NEWPORT_TEST_LEASE_SERVER", server)
                .stdout(Stdio::piped())
                .spawn()
                .map_err(|e| e.to_string())?,
        );
        let output = child.0.stdout.take().unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        thread::spawn(move || {
            for line in std::io::BufReader::new(output)
                .lines()
                .map_while(Result::ok)
            {
                if let Some(id) = line.strip_prefix("LEASE_READY ") {
                    let _ = tx.send(id.to_owned());
                    break;
                }
            }
        });
        let id = rx
            .recv_timeout(Duration::from_secs(20))
            .map_err(|e| format!("Child {round} did not open: {e}"))?;
        child.0.kill().map_err(|e| e.to_string())?;
        child.0.wait().map_err(|e| e.to_string())?;
        // The crashed child's session ID opens again only once its PTY is gone.
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            match backend.open_terminal_lease(json!({"id":server,"session":id,"cols":80,"rows":24}))
            {
                Ok(_lease) => break,
                Err(error) if error == "Terminal session already exists." => {}
                Err(error) => return Err(error),
            }
            if Instant::now() >= deadline {
                return Err(format!("Child {round} leaked its PTY"));
            }
            thread::sleep(Duration::from_millis(10));
        }
        // The unrelated owned terminal must still accept input after each kill.
        let marker = format!("lease-survivor-{round}");
        survivor.send(format!("printf '{marker}\\n'\n").into_bytes())?;
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut output = Vec::new();
        loop {
            match survivor.try_read() {
                Ok(Event::Data(bytes)) => output.extend(bytes),
                Ok(Event::Error(error)) => return Err(error),
                Ok(Event::Exit(_)) => return Err("Unrelated terminal was closed".into()),
                _ => {}
            }
            if String::from_utf8_lossy(&output).contains(&format!("\r{marker}\r\n")) {
                break;
            }
            if Instant::now() >= deadline {
                return Err("Surviving terminal did not return output".into());
            }
            thread::sleep(Duration::from_millis(5));
        }
        println!(
            "round {}: crashed PTY released; unrelated PTY still works",
            round + 1
        );
    }
    Ok(())
}
