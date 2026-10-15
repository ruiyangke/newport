//! Opens the SSH agent socket in the visible native application's process so
//! agent consent belongs to that application, rather than its background host.
//! Only identity listing and signing are forwarded; no private key is exported.
use newport_platform::ipc::Stream;
use newport_platform::ipc::{agent_relay as relay, protocol as desktop};
use serde::{Serialize, de::DeserializeOwned};
use std::{
    collections::HashMap,
    io::{self, Read, Write},
    net::Shutdown,
    path::{Path, PathBuf},
    sync::{
        Arc, Condvar, Mutex, Weak,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
use uuid::Uuid;

type Connections = HashMap<Uuid, [Stream; 2]>;
struct Session {
    control: Stream,
    connections: Mutex<Connections>,
    stopped: AtomicBool,
}
impl Session {
    fn stop(&self) {
        self.stopped.store(true, Ordering::Release);
        let _ = self.control.shutdown(Shutdown::Both);
        let connections = self.connections.lock().unwrap_or_else(|e| e.into_inner());
        for streams in connections.values() {
            for stream in streams {
                let _ = stream.shutdown(Shutdown::Both);
            }
        }
    }
}
struct Lifetime {
    stopped: AtomicBool,
    session: Mutex<Option<Arc<Session>>>,
    wake: Condvar,
}
type ActiveRelay = Option<(PathBuf, Weak<Lifetime>)>;
static ACTIVE_RELAY: Mutex<ActiveRelay> = Mutex::new(None);

/// Called by desktop request workers, never the UI render thread. It prevents
/// the first remote request racing ahead of relay registration on startup.
pub fn await_registration(path: &Path) -> Result<(), String> {
    let lifetime = ACTIVE_RELAY
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
        .filter(|(registered_path, _)| registered_path == path)
        .and_then(|(_, lifetime)| lifetime.upgrade());
    let Some(lifetime) = lifetime else {
        return Ok(());
    };
    let session = lifetime.session.lock().unwrap_or_else(|e| e.into_inner());
    let (session, _) = lifetime
        .wake
        .wait_timeout_while(session, Duration::from_secs(10), |session| {
            session.is_none() && !lifetime.stopped.load(Ordering::Acquire)
        })
        .unwrap_or_else(|e| e.into_inner());
    if lifetime.stopped.load(Ordering::Acquire) {
        return Err("Native SSH agent relay has stopped".into());
    }
    if session.is_none() {
        return Err("Native SSH agent relay is not connected; check that the Newport backend is running and up to date".into());
    }
    Ok(())
}

/// Keep this guard alive for the native application's lifetime. Reconnection
/// registers a new control channel, but never repeats an agent request.
pub struct AgentRelay {
    lifetime: Arc<Lifetime>,
}
impl Drop for AgentRelay {
    fn drop(&mut self) {
        self.lifetime.stopped.store(true, Ordering::Release);
        let session = self
            .lifetime
            .session
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if let Some(session) = session.as_ref() {
            session.stop();
        }
        self.lifetime.wake.notify_all();
    }
}

pub fn start(backend: &crate::backend::Backend) -> io::Result<AgentRelay> {
    let path = backend.socket_path().to_owned();
    let lifetime = Arc::new(Lifetime {
        stopped: AtomicBool::new(false),
        session: Mutex::new(None),
        wake: Condvar::new(),
    });
    let worker = lifetime.clone();
    *ACTIVE_RELAY.lock().unwrap_or_else(|e| e.into_inner()) =
        Some((path.clone(), Arc::downgrade(&lifetime)));
    std::thread::Builder::new()
        .name("ssh-agent-relay".into())
        .spawn(move || {
            while !worker.stopped.load(Ordering::Acquire) {
                let _ = run_session(&path, &worker);
                let mut current = worker.session.lock().unwrap_or_else(|e| e.into_inner());
                if let Some(session) = current.take() {
                    session.stop();
                }
                if worker.stopped.load(Ordering::Acquire) {
                    break;
                }
                let _ = worker.wake.wait_timeout(current, Duration::from_secs(1));
            }
        })?;
    Ok(AgentRelay { lifetime })
}

fn run_session(path: &Path, lifetime: &Arc<Lifetime>) -> io::Result<()> {
    let mut control = bridge(path, relay::REGISTER, serde_json::json!({}))?;
    control.set_read_timeout(None)?;
    let session = Arc::new(Session {
        control: control.try_clone()?,
        connections: Mutex::new(HashMap::new()),
        stopped: AtomicBool::new(false),
    });
    {
        let mut current = lifetime.session.lock().unwrap_or_else(|e| e.into_inner());
        if lifetime.stopped.load(Ordering::Acquire) {
            return Ok(());
        }
        *current = Some(session.clone());
        lifetime.wake.notify_all();
    }
    loop {
        let open: relay::Open = read_message(&mut control, relay::MAX_CONTROL_FRAME)?;
        // Connection attempts are local and do not prompt; signing is performed
        // by independent bounded workers after the one-use attachment succeeds.
        let available = {
            let connections = session
                .connections
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            connections.len() < relay::MAX_CONNECTIONS && !connections.contains_key(&open.ticket)
        };
        let agent = if available {
            source_path(open.source).and_then(newport_platform::ipc::connect_agent)
        } else {
            Err(io::Error::other("Native SSH agent relay is busy"))
        };
        let attach = relay::Attach {
            ticket: open.ticket,
            error: agent.as_ref().err().map(|error| error.to_string()),
        };
        let attached = bridge(path, relay::ATTACH, serde_json::to_value(attach)?);
        let (Ok(agent), Ok(backend)) = (agent, attached) else {
            continue;
        };
        agent.set_read_timeout(Some(Duration::from_secs(180)))?;
        agent.set_write_timeout(Some(Duration::from_secs(10)))?;
        backend.set_read_timeout(Some(Duration::from_secs(180)))?;
        backend.set_write_timeout(Some(Duration::from_secs(10)))?;
        let sockets = [agent.try_clone()?, backend.try_clone()?];
        {
            let mut connections = session
                .connections
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            if session.stopped.load(Ordering::Acquire) {
                return Ok(());
            }
            connections.insert(open.ticket, sockets);
        }
        let worker_session = session.clone();
        let spawned = std::thread::Builder::new()
            .name("ssh-agent-sign".into())
            .spawn(move || {
                let _ = relay_packets(backend, agent);
                worker_session
                    .connections
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .remove(&open.ticket);
            });
        if let Err(error) = spawned {
            session
                .connections
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remove(&open.ticket);
            return Err(error);
        }
    }
}

use newport_platform::ssh_agent::source_path;

fn bridge(path: &Path, method: &str, args: serde_json::Value) -> io::Result<Stream> {
    let mut stream = newport_platform::ipc::connect_private(path)?;
    stream.set_read_timeout(Some(Duration::from_secs(10)))?;
    stream.set_write_timeout(Some(Duration::from_secs(10)))?;
    write_message(
        &mut stream,
        &desktop::Request {
            version: desktop::VERSION,
            method: method.into(),
            args,
        },
    )?;
    let response: desktop::Response = read_message(&mut stream, relay::MAX_CONTROL_FRAME)?;
    if response.version != desktop::VERSION {
        return Err(io::Error::other(
            "Unsupported native SSH agent relay version",
        ));
    }
    if let Some(error) = response.error {
        return Err(io::Error::other(crate::backend::error_text(&error)));
    }
    Ok(stream)
}

fn read_message<T: DeserializeOwned>(stream: &mut Stream, limit: usize) -> io::Result<T> {
    let data = read_packet(stream, limit)?;
    rmp_serde::from_slice(&data).map_err(io::Error::other)
}
fn write_message(stream: &mut Stream, value: &impl Serialize) -> io::Result<()> {
    let data = rmp_serde::to_vec_named(value).map_err(io::Error::other)?;
    write_packet(stream, &data)
}
fn read_packet(stream: &mut Stream, limit: usize) -> io::Result<Vec<u8>> {
    let mut prefix = [0; 4];
    stream.read_exact(&mut prefix)?;
    let length = u32::from_be_bytes(prefix) as usize;
    if !(1..=limit).contains(&length) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Invalid SSH agent frame size",
        ));
    }
    let mut packet = vec![0; length];
    stream.read_exact(&mut packet)?;
    Ok(packet)
}
fn write_packet(stream: &mut Stream, packet: &[u8]) -> io::Result<()> {
    stream.write_all(&(packet.len() as u32).to_be_bytes())?;
    stream.write_all(packet)
}

fn relay_packets(mut backend: Stream, mut agent: Stream) -> io::Result<()> {
    loop {
        let request = read_packet(&mut backend, relay::MAX_AGENT_FRAME)?;
        if !relay::request_allowed(&request) {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "SSH agent request is not permitted",
            ));
        }
        write_packet(&mut agent, &request)?;
        let response = read_agent_response(&mut backend, &mut agent)?;
        if !relay::response_allowed(request[0], &response) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Unexpected SSH agent response",
            ));
        }
        write_packet(&mut backend, &response)?;
    }
}

/// A russh agent session is request/reply, without pipelining. While consent is
/// pending, detect the requesting connection closing and withdraw that exact
/// agent request. Partial response frames survive the short polling timeout.
fn read_agent_response(backend: &mut Stream, agent: &mut Stream) -> io::Result<Vec<u8>> {
    backend.set_nonblocking(true)?;
    agent.set_read_timeout(Some(Duration::from_millis(200)))?;
    let deadline = Instant::now() + Duration::from_secs(180);
    let mut read = |mut output: &mut [u8]| -> io::Result<()> {
        while !output.is_empty() {
            match backend.read(&mut [0]) {
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
                Ok(0) => {
                    return Err(io::Error::new(
                        io::ErrorKind::ConnectionAborted,
                        "SSH agent request was cancelled",
                    ));
                }
                Ok(_) => {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "Pipelined SSH agent requests are not supported",
                    ));
                }
                Err(error) => return Err(error),
            }
            if Instant::now() >= deadline {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "SSH agent approval timed out",
                ));
            }
            match agent.read(output) {
                Ok(0) => {
                    return Err(io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        "SSH agent disconnected",
                    ));
                }
                Ok(count) => output = &mut output[count..],
                Err(error)
                    if matches!(
                        error.kind(),
                        io::ErrorKind::WouldBlock
                            | io::ErrorKind::TimedOut
                            | io::ErrorKind::Interrupted
                    ) => {}
                Err(error) => return Err(error),
            }
        }
        Ok(())
    };
    let mut prefix = [0; 4];
    read(&mut prefix)?;
    let length = u32::from_be_bytes(prefix) as usize;
    if !(1..=relay::MAX_AGENT_FRAME).contains(&length) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Invalid SSH agent response size",
        ));
    }
    let mut response = vec![0; length];
    read(&mut response)?;
    backend.set_nonblocking(false)?;
    Ok(response)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Executed as a separate native process by the ownership test. It never
    // touches a real agent: the subprocess's SSH_AUTH_SOCK is a test listener.
    #[test]
    fn native_relay_process_fixture() {
        let Some(path) = std::env::var_os("NEWPORT_TEST_AGENT_RELAY_BRIDGE") else {
            return;
        };
        let _relay = start(&crate::backend::Backend::at_path(path.into())).unwrap();
        std::thread::sleep(Duration::from_secs(30));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn actual_agent_socket_belongs_to_native_process_and_reconnect_does_not_replay() {
        use newport_platform::ipc::Listener;
        use std::{
            os::fd::AsRawFd,
            process::{Child, Command, Stdio},
        };
        struct Fixture {
            child: Child,
            directory: PathBuf,
        }
        impl Drop for Fixture {
            fn drop(&mut self) {
                let _ = self.child.kill();
                let _ = self.child.wait();
                let _ = std::fs::remove_dir_all(&self.directory);
            }
        }
        fn accept(listener: &Listener) -> Stream {
            let started = Instant::now();
            loop {
                match listener.accept() {
                    Ok((stream, _)) => {
                        stream.set_nonblocking(false).unwrap();
                        stream
                            .set_read_timeout(Some(Duration::from_secs(3)))
                            .unwrap();
                        return stream;
                    }
                    Err(error)
                        if error.kind() == io::ErrorKind::WouldBlock
                            && started.elapsed() < Duration::from_secs(10) =>
                    {
                        std::thread::sleep(Duration::from_millis(10))
                    }
                    Err(error) => panic!("accept: {error}"),
                }
            }
        }
        fn peer_pid(stream: &Stream) -> u32 {
            unsafe extern "C" {
                fn getsockopt(
                    fd: i32,
                    level: i32,
                    option: i32,
                    value: *mut std::ffi::c_void,
                    size: *mut u32,
                ) -> i32;
            }
            let mut pid: i32 = 0;
            let mut size = std::mem::size_of_val(&pid) as u32;
            // macOS SOL_LOCAL / LOCAL_PEERPID returns the connecting process
            // from the kernel; no PID supplied by the relay is trusted.
            let result = unsafe {
                getsockopt(
                    stream.as_raw_fd(),
                    0,
                    2,
                    (&mut pid as *mut i32).cast(),
                    &mut size,
                )
            };
            assert_eq!(result, 0, "{}", io::Error::last_os_error());
            assert!(pid > 0);
            pid as u32
        }
        fn ready(stream: &mut Stream) {
            write_message(
                stream,
                &desktop::Response {
                    version: desktop::VERSION,
                    result: serde_json::Value::Null,
                    error: None,
                },
            )
            .unwrap();
        }
        let directory = PathBuf::from(format!("/tmp/np-agent-relay-{}", Uuid::new_v4()));
        std::fs::create_dir(&directory).unwrap();
        let bridge_path = directory.join("bridge");
        let agent_path = directory.join("agent");
        let bridge = Listener::bind(&bridge_path).unwrap();
        let agents = Listener::bind(&agent_path).unwrap();
        bridge.set_nonblocking(true).unwrap();
        agents.set_nonblocking(true).unwrap();
        let child = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "agent_relay::tests::native_relay_process_fixture",
                "--nocapture",
            ])
            .env("NEWPORT_TEST_AGENT_RELAY_BRIDGE", &bridge_path)
            .env("SSH_AUTH_SOCK", &agent_path)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let fixture = Fixture { child, directory };
        let mut control = accept(&bridge);
        let request: desktop::Request =
            read_message(&mut control, relay::MAX_CONTROL_FRAME).unwrap();
        assert_eq!(request.method, relay::REGISTER);
        assert_eq!(peer_pid(&control), fixture.child.id());
        ready(&mut control);
        let ticket = Uuid::new_v4();
        write_message(
            &mut control,
            &relay::Open {
                ticket,
                source: relay::Source::System,
            },
        )
        .unwrap();
        let mut actual_agent = accept(&agents);
        assert_eq!(peer_pid(&actual_agent), fixture.child.id());
        assert_ne!(peer_pid(&actual_agent), std::process::id());
        let mut attached = accept(&bridge);
        let request: desktop::Request =
            read_message(&mut attached, relay::MAX_CONTROL_FRAME).unwrap();
        assert_eq!(request.method, relay::ATTACH);
        let attachment: relay::Attach = serde_json::from_value(request.args).unwrap();
        assert_eq!(attachment.ticket, ticket);
        assert!(attachment.error.is_none());
        assert_eq!(peer_pid(&attached), fixture.child.id());
        ready(&mut attached);
        write_packet(&mut attached, &[11]).unwrap();
        assert_eq!(read_packet(&mut actual_agent, 1024).unwrap(), [11]);
        write_packet(&mut actual_agent, &[12, 0, 0, 0, 0]).unwrap();
        assert_eq!(read_packet(&mut attached, 1024).unwrap(), [12, 0, 0, 0, 0]);
        let sign = [13, 0, 0, 0, 1, 42, 0, 0, 0, 1, 43, 0, 0, 0, 0];
        write_packet(&mut attached, &sign).unwrap();
        assert_eq!(read_packet(&mut actual_agent, 1024).unwrap(), sign);
        drop(control);
        assert!(read_packet(&mut actual_agent, 1024).is_err());
        assert!(read_packet(&mut attached, 1024).is_err());
        let mut reconnected = accept(&bridge);
        let request: desktop::Request =
            read_message(&mut reconnected, relay::MAX_CONTROL_FRAME).unwrap();
        assert_eq!(request.method, relay::REGISTER);
        ready(&mut reconnected);
        assert_eq!(
            agents.accept().unwrap_err().kind(),
            io::ErrorKind::WouldBlock
        );
    }

    #[test]
    fn forwards_identities_and_signatures_but_refuses_agent_mutation() {
        let (mut backend, relay_backend) = Stream::pair().unwrap();
        let (relay_agent, mut agent) = Stream::pair().unwrap();
        let worker = std::thread::spawn(move || relay_packets(relay_backend, relay_agent));
        write_packet(&mut backend, &[11]).unwrap();
        assert_eq!(read_packet(&mut agent, 1024).unwrap(), [11]);
        write_packet(&mut agent, &[12, 0, 0, 0, 0]).unwrap();
        assert_eq!(read_packet(&mut backend, 1024).unwrap(), [12, 0, 0, 0, 0]);
        let sign = [13, 0, 0, 0, 1, 42, 0, 0, 0, 1, 43, 0, 0, 0, 0];
        write_packet(&mut backend, &sign).unwrap();
        assert_eq!(read_packet(&mut agent, 1024).unwrap(), sign);
        write_packet(&mut agent, &[14, 0, 0, 0, 1, 99]).unwrap();
        assert_eq!(
            read_packet(&mut backend, 1024).unwrap(),
            [14, 0, 0, 0, 1, 99]
        );
        write_packet(&mut backend, &[17]).unwrap();
        assert_eq!(
            worker.join().unwrap().unwrap_err().kind(),
            io::ErrorKind::PermissionDenied
        );
        assert!(read_packet(&mut agent, 1024).is_err());
    }

    #[test]
    fn oversized_frames_are_rejected_before_allocation_or_forwarding() {
        let (mut input, mut reader) = Stream::pair().unwrap();
        input.write_all(&u32::MAX.to_be_bytes()).unwrap();
        assert_eq!(
            read_packet(&mut reader, relay::MAX_AGENT_FRAME)
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidData
        );
    }

    #[test]
    fn disconnect_closes_pending_sign_without_replay() {
        let (mut backend, relay_backend) = Stream::pair().unwrap();
        let (relay_agent, mut agent) = Stream::pair().unwrap();
        let session = Session {
            control: backend.try_clone().unwrap(),
            connections: Mutex::new(HashMap::from([(
                Uuid::new_v4(),
                [
                    relay_backend.try_clone().unwrap(),
                    relay_agent.try_clone().unwrap(),
                ],
            )])),
            stopped: AtomicBool::new(false),
        };
        let worker = std::thread::spawn(move || relay_packets(relay_backend, relay_agent));
        write_packet(&mut backend, &[11]).unwrap();
        assert_eq!(read_packet(&mut agent, 1024).unwrap(), [11]);
        session.stop();
        assert!(worker.join().unwrap().is_err());
        assert!(read_packet(&mut agent, 1024).is_err());
    }

    #[test]
    fn individual_auth_cancellation_withdraws_pending_agent_request() {
        let (mut backend, relay_backend) = Stream::pair().unwrap();
        let (relay_agent, mut agent) = Stream::pair().unwrap();
        let worker = std::thread::spawn(move || relay_packets(relay_backend, relay_agent));
        write_packet(&mut backend, &[11]).unwrap();
        assert_eq!(read_packet(&mut agent, 1024).unwrap(), [11]);
        drop(backend);
        let started = Instant::now();
        assert_eq!(
            worker.join().unwrap().unwrap_err().kind(),
            io::ErrorKind::ConnectionAborted
        );
        assert!(started.elapsed() < Duration::from_secs(2));
        assert!(read_packet(&mut agent, 1024).is_err());
    }

    #[test]
    fn partial_agent_frames_survive_approval_poll_timeouts() {
        let (mut backend, relay_backend) = Stream::pair().unwrap();
        let (relay_agent, mut agent) = Stream::pair().unwrap();
        let worker = std::thread::spawn(move || relay_packets(relay_backend, relay_agent));
        write_packet(&mut backend, &[11]).unwrap();
        assert_eq!(read_packet(&mut agent, 1024).unwrap(), [11]);
        agent.write_all(&[0, 0]).unwrap();
        std::thread::sleep(Duration::from_millis(250));
        agent.write_all(&[0, 5, 12, 0]).unwrap();
        std::thread::sleep(Duration::from_millis(250));
        agent.write_all(&[0, 0, 0]).unwrap();
        assert_eq!(read_packet(&mut backend, 1024).unwrap(), [12, 0, 0, 0, 0]);
        drop(backend);
        assert!(worker.join().unwrap().is_err());
    }
}
