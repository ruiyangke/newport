//! Demand-started control process. No installation or shell changes on startup.
use newport_protocol::{
    common,
    runtime::{self as protocol, wire},
};
use sha2::{Digest, Sha256};
use std::{
    fs::File,
    io::{self, Read},
    os::fd::AsFd,
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
fn require(hello: &protocol::Hello, service: &str, version: u32) -> Result<(), common::Error> {
    hello.require(service, version).or_else(|error| {
        let installed_since = service == "git"
            && hello
                .services
                .get(service)
                .is_some_and(|c| c.unavailable.is_some() && c.versions.contains(&version))
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
    // A first run can be slow, as macOS's xcrun shim is on a busy host. Stay
    // well inside the desktop's waits for a reply and for the greeting.
    let deadline = Instant::now() + Duration::from_secs(5);
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
    variant: newport_protocol::product::Variant,
    services: &[&dyn crate::composition::Service],
) -> io::Result<newport_protocol::product::Product> {
    let mut compiled: std::collections::BTreeMap<_, _> =
        crate::composition::capabilities(services)?
            .into_iter()
            .map(|(name, capability)| (name, capability.versions))
            .collect();
    for (name, version) in [
        ("git", newport_protocol::git::VERSION),
        ("monitor", newport_protocol::monitor::VERSION),
        ("integration", newport_protocol::integration::VERSION),
        (
            newport_protocol::browser::CAPABILITY,
            newport_protocol::browser::VERSION,
        ),
        (
            newport_protocol::terminal::holder::CAPABILITY,
            newport_protocol::terminal::holder::VERSION,
        ),
        ("gui-notices", 1),
        (
            newport_protocol::worker::admission::CAPABILITY,
            newport_protocol::worker::admission::VERSION,
        ),
        ("lsp", crate::lsp::VERSION),
    ] {
        compiled.insert(name.into(), vec![version]);
    }
    compiled.insert("terminal".into(), crate::terminal::VERSIONS.to_vec());
    compiled.insert(
        newport_protocol::clipboard::SERVICE_CAPABILITY.into(),
        newport_protocol::clipboard::SERVICE_VERSIONS.to_vec(),
    );
    compiled.insert(
        newport_protocol::clipboard::exports::CAPABILITY.into(),
        newport_protocol::clipboard::exports::VERSIONS.to_vec(),
    );
    let product = newport_protocol::product::Product {
        variant,
        control_versions: vec![protocol::VERSION],
        services: compiled,
    };
    product.validate().map_err(io::Error::other)?;
    Ok(product)
}
fn describe(
    variant: newport_protocol::product::Variant,
    services: &[&dyn crate::composition::Service],
) -> io::Result<protocol::Hello> {
    compiled_product(variant, services)?;
    let git = usable("git");
    let mut capabilities = crate::composition::capabilities(services)?;
    for (name, version, available, reason) in [
        (
            "git",
            newport_protocol::git::VERSION,
            git,
            "Git is not available on this server.",
        ),
        (
            "monitor",
            newport_protocol::monitor::VERSION,
            cfg!(target_os = "linux"),
            "Monitoring requires Linux.",
        ),
        (
            "integration",
            newport_protocol::integration::VERSION,
            true,
            "",
        ),
        (
            newport_protocol::terminal::holder::CAPABILITY,
            newport_protocol::terminal::holder::VERSION,
            cfg!(target_os = "linux"),
            "Persistent terminals require Linux.",
        ),
        ("gui-notices", 1, true, ""),
        (
            newport_protocol::browser::CAPABILITY,
            newport_protocol::browser::VERSION,
            true,
            "",
        ),
        (
            newport_protocol::worker::admission::CAPABILITY,
            newport_protocol::worker::admission::VERSION,
            true,
            "",
        ),
        ("lsp", crate::lsp::VERSION, true, ""),
    ] {
        capabilities.insert(
            name.into(),
            protocol::Capability {
                versions: vec![version],
                unavailable: (!available).then(|| reason.into()),
                contract_id: protocol::service_contract_id(name).map(str::to_owned),
            },
        );
    }
    let terminal = cfg!(target_os = "linux");
    capabilities.insert(
        "terminal".into(),
        protocol::Capability {
            contract_id: None,
            versions: crate::terminal::VERSIONS.to_vec(),
            unavailable: (!terminal).then(|| "Persistent terminals require Linux.".into()),
        },
    );
    capabilities.insert(
        newport_protocol::clipboard::SERVICE_CAPABILITY.into(),
        protocol::Capability {
            contract_id: None,
            versions: newport_protocol::clipboard::SERVICE_VERSIONS.to_vec(),
            unavailable: None,
        },
    );
    capabilities.insert(
        newport_protocol::clipboard::exports::CAPABILITY.into(),
        protocol::Capability {
            contract_id: None,
            versions: newport_protocol::clipboard::exports::VERSIONS.to_vec(),
            unavailable: None,
        },
    );
    Ok(protocol::Hello {
        product: variant,
        versions: vec![protocol::VERSION],
        instance_id: uuid::Uuid::new_v4().to_string(),
        build_sha256: build_identity()?,
        agent_version: Some(crate::VERSION.into()),
        worker_socket: String::new(),
        services: capabilities,
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
    use protocol::{Operation, ReplyValue};
    let mut stop = false;
    let result = if request.runtime_id != hello.instance_id {
        stop = true;
        Err(common::Error {
            code: "stale_runtime".into(),
            message: "Request belongs to an obsolete runtime identity.".into(),
            retry: common::Retry::No,
        })
    } else {
        match request.operation {
            Operation::Initialize {
                version,
                session_id,
                ..
            } if !*initialized && version == protocol::VERSION => {
                broker.initialize(session_id);
                *initialized = true;
                Ok(ReplyValue::Accepted)
            }
            Operation::Permissions {
                worker_id,
                permissions,
            } if *initialized => broker
                .permissions(worker_id, permissions)
                .map(ReplyValue::Permissions)
                .map_err(control_error),
            Operation::PrepareWorkerStream {
                worker_id,
                service,
                version,
                contract_id,
            } if *initialized => hello
                .require_contract(&service, version, contract_id.as_deref())
                .and_then(|_| require(hello, &service, version))
                .and_then(|_| {
                    broker
                        .prepare_stream(&worker_id, &service, version, contract_id.as_deref())
                        .map(ReplyValue::WorkerStream)
                        .map_err(control_error)
                }),
            Operation::ClipboardRoute(route) if *initialized => hello
                .require(newport_protocol::clipboard::SERVICE_CAPABILITY, 1)
                .and_then(|_| {
                    broker
                        .clipboard_route(route)
                        .map(ReplyValue::ClipboardRoute)
                        .map_err(control_error)
                }),
            Operation::Ping if *initialized => Ok(ReplyValue::Accepted),
            Operation::Shutdown if *initialized => {
                stop = true;
                Ok(ReplyValue::Accepted)
            }
            _ => {
                stop = true;
                Err(common::Error {
                    code: "invalid_operation".into(),
                    message: "Invalid runtime initialization or operation.".into(),
                    retry: common::Retry::No,
                })
            }
        }
    };
    (
        protocol::Reply {
            id: request.id,
            runtime_id: hello.instance_id.clone(),
            result,
        },
        stop,
    )
}

fn control_error(error: io::Error) -> common::Error {
    let (code, retry) = match error.kind() {
        io::ErrorKind::WouldBlock => ("busy", common::Retry::Yes),
        io::ErrorKind::TimedOut => ("timed_out", common::Retry::Unknown),
        io::ErrorKind::PermissionDenied => ("permission_denied", common::Retry::No),
        _ => ("control_failed", common::Retry::Unknown),
    };
    common::Error {
        code: code.into(),
        message: error.to_string(),
        retry,
    }
}

pub fn serve() -> io::Result<()> {
    serve_with(&[])
}

pub fn serve_with(services: &[&dyn crate::composition::Service]) -> io::Result<()> {
    serve_product(newport_protocol::product::Variant::Core, services)
}

pub fn serve_product(
    variant: newport_protocol::product::Variant,
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
        });
    }
    // Lingering Newport turned on goes once no display or held terminal runs.
    std::thread::spawn(crate::linger::reconcile);
    let mut hello = describe(variant, services)?;
    let broker = crate::worker_session::Broker::new(
        &hello.instance_id,
        &hello.build_sha256,
        crate::composition::transports(services)?,
    )?;
    hello.worker_socket = broker
        .path()
        .to_str()
        .ok_or_else(|| io::Error::other("Invalid worker socket path"))?
        .into();
    let stdout = File::from(io::stdout().as_fd().try_clone_to_owned()?);
    crate::transport::set_nonblocking(stdout.as_fd())?;
    let mut output = crate::transport::DeadlineWriter {
        inner: stdout,
        timeout: Duration::from_secs(5),
        cancellation: crate::transport::Cancellation::default(),
    };
    wire::write(&mut output, &hello)?;
    let (send, receive) = mpsc::sync_channel(1);
    std::thread::spawn(move || {
        let mut input = io::stdin().lock();
        loop {
            let frame = wire::read::<protocol::Request>(&mut input).map_err(io::Error::from);
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
        wire::write(&mut output, &reply)?;
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
    if !newport_protocol::product::valid_build(expected_build)
        || build_identity()? != expected_build
    {
        return Err(io::Error::other(
            "Needle build changed. Reconnect before starting this service.",
        ));
    }
    if args.len() < 4 {
        return Err(io::Error::other("Needle worker registration is required"));
    }
    let mut lease = crate::worker_session::Lease::attach(
        std::path::Path::new(&args[1]),
        newport_protocol::runtime::Attach {
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
        "terminal" if args.len() == 4 => crate::terminal::serve_bound(cancellation),
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
            contract_id: None,
            versions: vec![1],
            unavailable: Some(reason.into()),
        };
        let hello = protocol::Hello {
            product: newport_protocol::product::Variant::Core,
            versions: vec![protocol::VERSION],
            instance_id: uuid::Uuid::new_v4().to_string(),
            build_sha256: "0".repeat(64),
            agent_version: None,
            worker_socket: "/tmp/workers.sock".into(),
            services: [
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
            require(&hello, "monitor", 1).unwrap_err().message,
            "Monitoring requires Linux."
        );
    }
}
