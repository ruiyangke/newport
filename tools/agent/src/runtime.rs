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
/// As [`protocol::Hello::require`], but Git installed since Needle started
/// counts: the person installs it after Projects says it's missing.
fn require(hello: &protocol::Hello, service: &str, version: u32) -> io::Result<()> {
    hello.require(service, version).or_else(|error| {
        let installed_since = service == "git"
            && hello
                .capabilities
                .get(service)
                .is_some_and(|c| !c.available && c.versions.contains(&version))
            && usable("git");
        if installed_since { Ok(()) } else { Err(error) }
    })
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
pub fn compiled_product(
    variant: crate::product::Variant,
    services: &[&dyn crate::composition::Service],
) -> io::Result<crate::product::Product> {
    let mut compiled: std::collections::BTreeMap<_, _> =
        crate::composition::capabilities(services)?
            .into_iter()
            .map(|(name, capability)| (name, capability.versions))
            .collect();
    for (name, version) in [
        ("git", crate::git::protocol::VERSION),
        ("monitor", 1),
        ("integration", crate::integration_control::VERSION),
        ("gui-notices", 1),
        (
            protocol::worker_stream::CAPABILITY,
            protocol::worker_stream::VERSION,
        ),
        ("lsp", crate::lsp::VERSION),
    ] {
        compiled.insert(name.into(), vec![version]);
    }
    compiled.insert("gui-launch".into(), crate::gui_protocol::VERSIONS.to_vec());
    compiled.insert(
        crate::gui_protocol::attachment::CAPABILITY.into(),
        vec![crate::gui_protocol::attachment::VERSION],
    );
    compiled.insert(
        protocol::clipboard::SERVICE_CAPABILITY.into(),
        protocol::clipboard::SERVICE_VERSIONS.to_vec(),
    );
    compiled.insert(
        protocol::clipboard_export::CAPABILITY.into(),
        protocol::clipboard_export::VERSIONS.to_vec(),
    );
    let product = crate::product::Product {
        variant,
        control_versions: vec![protocol::VERSION],
        services: compiled,
    };
    product.validate().map_err(io::Error::other)?;
    Ok(product)
}
fn describe(
    variant: crate::product::Variant,
    services: &[&dyn crate::composition::Service],
) -> io::Result<protocol::Hello> {
    compiled_product(variant, services)?;
    let git = usable("git");
    let mut capabilities = crate::composition::capabilities(services)?;
    capabilities.insert(
        variant.marker().into(),
        protocol::Capability {
            versions: vec![crate::product::VERSION],
            available: true,
            reason: None,
        },
    );
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
        ("gui-notices", 1, true, ""),
        (
            protocol::worker_stream::CAPABILITY,
            protocol::worker_stream::VERSION,
            true,
            "",
        ),
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
    capabilities.insert(
        protocol::clipboard::SERVICE_CAPABILITY.into(),
        protocol::Capability {
            versions: protocol::clipboard::SERVICE_VERSIONS.to_vec(),
            available: true,
            reason: None,
        },
    );
    capabilities.insert(
        "gui-launch".into(),
        protocol::Capability {
            versions: crate::gui_protocol::VERSIONS.to_vec(),
            available: true,
            reason: None,
        },
    );
    capabilities.insert(
        protocol::clipboard_export::CAPABILITY.into(),
        protocol::Capability {
            versions: protocol::clipboard_export::VERSIONS.to_vec(),
            available: true,
            reason: None,
        },
    );
    capabilities.insert(
        crate::gui_protocol::attachment::CAPABILITY.into(),
        protocol::Capability {
            versions: vec![crate::gui_protocol::attachment::VERSION],
            available: true,
            reason: None,
        },
    );
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
    let mut worker_stream = None;
    let mut clipboard_route = None;
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
                } if *initialized => require(hello, &service, version)
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
                protocol::Operation::PrepareWorkerStream {
                    worker_id,
                    service,
                    version,
                } if *initialized => match require(hello, &service, version)
                    .and_then(|_| broker.prepare_stream(&worker_id, &service))
                {
                    Ok(endpoint) => {
                        worker_stream = Some(endpoint);
                        None
                    }
                    Err(error) => Some(error.to_string()),
                },
                protocol::Operation::Ping if *initialized => None,
                protocol::Operation::ClipboardRoute { route } if *initialized => broker
                    .clipboard_route(route)
                    .err()
                    .map(|error| error.to_string()),
                protocol::Operation::ClipboardRouteV2 { route } if *initialized => match hello
                    .require(
                        protocol::clipboard::SERVICE_CAPABILITY,
                        protocol::clipboard::ROUTE_RESULT_VERSION,
                    )
                    .and_then(|_| broker.clipboard_route_v2(route))
                {
                    Ok(reply) => {
                        clipboard_route = Some(reply);
                        None
                    }
                    Err(error) => Some(error.to_string()),
                },
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
            clipboard_route,
            worker_stream,
            applied,
            id: request.id,
            runtime_id: hello.instance_id.clone(),
            error,
        },
        stop,
    )
}
pub fn serve() -> io::Result<()> {
    serve_with(&[])
}

pub fn serve_with(services: &[&dyn crate::composition::Service]) -> io::Result<()> {
    serve_product(crate::product::Variant::Core, services)
}

pub fn serve_product(
    variant: crate::product::Variant,
    services: &[&dyn crate::composition::Service],
) -> io::Result<()> {
    crate::composition::capabilities(services)?;
    // Recover external display resources even if this connection never enables
    // Clipboard. Live displays retain their directory lease and are untouched.
    crate::x_endpoint::sweep();
    crate::owned_directory::sweep(&crate::paths::runtime()?, "wl-");
    // Builds a finished update replaced while they still ran go now, in the
    // background so the handshake isn't delayed. This build counts as used.
    if let Some(home) = std::env::var_os("HOME") {
        std::thread::spawn(move || {
            crate::deployment::mark_used();
            crate::deployment::prune_retained(std::path::Path::new(&home));
            // Shells opened before Needle was reinstalled keep the launcher
            // directory on PATH; give them their launchers back.
            let _ = crate::gui_launch::prepare();
        });
    }
    let mut hello = describe(variant, services)?;
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
                ));
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
    worker_with(expected_build, args, &[])
}

pub fn worker_with(
    expected_build: &str,
    args: &[String],
    services: &[&dyn crate::composition::Service],
) -> io::Result<()> {
    crate::composition::capabilities(services)?;
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
            lease.clipboard()?,
        ),
        name => match services.iter().find(|service| service.name() == name) {
            Some(service) => service.run(
                crate::composition::WorkerContext::new(lease, &args[2], expected_build),
                &args[4..],
            ),
            None => Err(io::Error::other("Invalid Needle worker")),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn git_installed_after_startup_is_accepted_and_other_services_stay_refused() {
        assert!(usable("git"), "the test needs git on PATH");
        let unavailable = |reason: &str| protocol::Capability {
            versions: vec![1],
            available: false,
            reason: Some(reason.into()),
        };
        let hello = protocol::Hello {
            protocol: protocol::PROTOCOL.into(),
            versions: vec![protocol::VERSION],
            instance_id: uuid::Uuid::new_v4().to_string(),
            build_sha256: "0".repeat(64),
            worker_socket: "/tmp/workers.sock".into(),
            capabilities: [
                (
                    "git".into(),
                    unavailable("Git is not available on this server."),
                ),
                ("monitor".into(), unavailable("Monitoring requires Linux.")),
            ]
            .into(),
            dependencies: [("git".into(), false)].into(),
            limits: protocol::Limits {
                max_control_bytes: protocol::MAX_FRAME,
                max_inflight: 1,
                max_workers: protocol::MAX_WORKERS,
                max_registrations: protocol::MAX_REGISTRATIONS,
            },
        };
        assert!(require(&hello, "git", 1).is_ok());
        assert!(require(&hello, "git", 2).is_err(), "versions still count");
        assert_eq!(
            require(&hello, "monitor", 1).unwrap_err().to_string(),
            "Monitoring requires Linux."
        );
    }
}
