//! Demand-started control process. No installation or shell changes on startup.
use crate::runtime_protocol as protocol;
use sha2::{Digest, Sha256};
use std::{
    fs::File,
    io::{self, Read},
    os::fd::{AsFd, AsRawFd},
    process::{Command, Stdio},
    sync::mpsc,
    time::{Duration, Instant},
};

pub fn build_identity() -> io::Result<String> {
    // On Linux this names the running inode even if another client activates a
    // different executable at the original path while this process is alive.
    #[cfg(target_os = "linux")]
    let mut executable = File::open("/proc/self/exe")?;
    #[cfg(not(target_os = "linux"))]
    let mut executable = File::open(std::env::current_exe()?)?;
    let mut hash = Sha256::new();
    let mut buffer = [0; 65536];
    loop {
        let count = executable.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hash.update(&buffer[..count]);
    }
    Ok(hash.finalize().iter().map(|b| format!("{b:02x}")).collect())
}
fn usable(command: &str) -> bool {
    let Ok(mut child) = Command::new(command)
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    else {
        return false;
    };
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return status.success(),
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(10)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return false;
            }
        }
    }
}
fn describe() -> io::Result<protocol::Hello> {
    let git = usable("git");
    let mut capabilities = std::collections::BTreeMap::new();
    for (name, version, available, reason) in [
        (
            "git",
            crate::git::protocol::VERSION,
            git,
            "Git is not available on this server.",
        ),
        (
            "monitor",
            1,
            cfg!(target_os = "linux"),
            "Monitoring requires Linux.",
        ),
        ("integration", crate::integration_control::VERSION, true, ""),
        ("lsp", crate::lsp::VERSION, true, ""),
    ] {
        capabilities.insert(
            name.into(),
            protocol::Capability {
                versions: vec![version],
                available,
                reason: (!available).then(|| reason.into()),
            },
        );
    }
    Ok(protocol::Hello {
        protocol: protocol::PROTOCOL.into(),
        versions: vec![protocol::VERSION],
        instance_id: uuid::Uuid::new_v4().to_string(),
        build_sha256: build_identity()?,
        worker_socket: String::new(),
        capabilities,
        dependencies: [("git".into(), git)].into(),
        limits: protocol::Limits {
            max_control_bytes: protocol::MAX_FRAME,
            max_inflight: 1,
            max_workers: protocol::MAX_WORKERS,
            max_registrations: protocol::MAX_REGISTRATIONS,
        },
    })
}
fn respond(
    hello: &protocol::Hello,
    initialized: &mut bool,
    broker: &crate::worker_session::Broker,
    request: protocol::Request,
) -> (protocol::Reply, bool) {
    let mut stop = false;
    let mut applied = None;
    let error =
        if uuid::Uuid::parse_str(&request.id).is_err() || request.runtime_id != hello.instance_id {
            stop = true;
            Some("Invalid request or obsolete runtime identity.".into())
        } else {
            match request.operation {
                protocol::Operation::Initialize {
                    version,
                    session_id,
                    transport_generation,
                } if !*initialized
                    && version == protocol::VERSION
                    && transport_generation > 0
                    && uuid::Uuid::parse_str(&session_id).is_ok() =>
                {
                    broker.initialize(session_id);
                    *initialized = true;
                    None
                }
                protocol::Operation::Register {
                    worker_id,
                    service,
                    version,
                } if *initialized => hello
                    .require(&service, version)
                    .and_then(|_| broker.register(&worker_id, &service))
                    .err()
                    .map(|e| e.to_string()),
                protocol::Operation::Permissions {
                    worker_id,
                    permissions,
                } if *initialized => match broker.permissions(worker_id, permissions) {
                    Ok(value) => {
                        applied = Some(value);
                        None
                    }
                    Err(error) => Some(error.to_string()),
                },
                protocol::Operation::Ping if *initialized => None,
                protocol::Operation::Shutdown if *initialized => {
                    stop = true;
                    None
                }
                _ => {
                    stop = true;
                    Some("Invalid runtime initialization or operation.".into())
                }
            }
        };
    (
        protocol::Reply {
            applied,
            id: request.id,
            runtime_id: hello.instance_id.clone(),
            error,
        },
        stop,
    )
}
pub fn serve() -> io::Result<()> {
    // Recover external display resources even if this connection never enables
    // Clipboard. Live displays retain their directory lease and are untouched.
    crate::x_endpoint::sweep();
    crate::owned_directory::sweep(&crate::paths::runtime()?, "wl-");
    let mut hello = describe()?;
    let broker = crate::worker_session::Broker::new(&hello.instance_id, &hello.build_sha256)?;
    hello.worker_socket = broker
        .path()
        .to_str()
        .ok_or_else(|| io::Error::other("Invalid worker socket path"))?
        .into();
    let stdout = File::from(io::stdout().as_fd().try_clone_to_owned()?);
    let flags = unsafe { libc::fcntl(stdout.as_raw_fd(), libc::F_GETFL) };
    if flags < 0
        || unsafe { libc::fcntl(stdout.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0
    {
        return Err(io::Error::last_os_error());
    }
    let mut output = crate::transport::DeadlineWriter {
        inner: stdout,
        timeout: Duration::from_secs(5),
        cancellation: crate::transport::Cancellation::default(),
    };
    protocol::write(&mut output, &hello)?;
    let (send, receive) = mpsc::sync_channel(1);
    std::thread::spawn(move || {
        let mut input = io::stdin().lock();
        loop {
            let frame = protocol::read::<protocol::Request>(&mut input);
            let failed = frame.is_err();
            if send.send(frame).is_err() || failed {
                break;
            }
        }
    });
    let mut initialized = false;
    loop {
        let limit = Duration::from_secs(if initialized { 45 } else { 5 });
        let request = match receive.recv_timeout(limit) {
            Ok(Ok(request)) => request,
            Ok(Err(error)) if error.kind() == io::ErrorKind::UnexpectedEof => return Ok(()),
            Ok(Err(error)) => return Err(error),
            Err(_) => {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "Runtime heartbeat expired",
                ))
            }
        };
        let (reply, stop) = respond(&hello, &mut initialized, &broker, request);
        protocol::write(&mut output, &reply)?;
        if stop {
            return Ok(());
        }
    }
}
/// Check the executable before a feature consumes its stdin protocol. Replacing
/// the active binary cannot silently mix an old negotiated runtime with new workers.
pub fn worker(expected_build: &str, args: &[String]) -> io::Result<()> {
    if !protocol::valid_build(expected_build) || build_identity()? != expected_build {
        return Err(io::Error::other(
            "Needle build changed. Reconnect before starting this service.",
        ));
    }
    if args.len() < 4 {
        return Err(io::Error::other("Needle worker registration is required"));
    }
    let mut lease = crate::worker_session::Lease::attach(
        std::path::Path::new(&args[1]),
        protocol::Attach {
            runtime_id: args[0].clone(),
            worker_id: args[2].clone(),
            build_sha256: expected_build.into(),
            service: args[3].clone(),
        },
    )?;
    let cancellation = lease.cancellation();
    match args[3].as_str() {
        "git" if args.len() == 4 => crate::git::serve_bound(cancellation),
        "monitor" if args.len() == 4 => crate::monitor::serve_bound(cancellation),
        "lsp" if args.len() == 7 => crate::lsp::serve_bound(&args[4..], cancellation),
        "integration" if args.len() == 5 => crate::agent::serve_bound(
            &args[4],
            cancellation,
            lease.take_permissions(),
            lease.scope(),
        ),
        _ => Err(io::Error::other("Invalid Needle worker")),
    }
}
