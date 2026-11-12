//! A host executable can register services without adding private dependencies
//! to the core agent. The host owns early helper/supervisor dispatch; call the
//! shared CLI only after that dispatch, before starting unrelated threads.
use crate::worker_session::Lease;
use newport_protocol::{runtime::Capability, worker::Transport};
use std::{collections::BTreeMap, io, path::Path};

/// A compiled service. Availability describes host support, not account access
/// or sharing permission: each service must enforce those before doing work.
pub trait Service: Sync {
    fn name(&self) -> &'static str;
    fn capability(&self) -> Capability;
    /// The compiled service owns its framing; client arguments cannot select it.
    fn transport(&self) -> Transport;
    /// Runs only after the runtime admits this exact worker, service and build.
    /// The implementation must bound I/O and observe context cancellation.
    fn run(&self, context: WorkerContext, args: &[String]) -> io::Result<()>;
}

/// The admitted worker retains its runtime lease until its service returns.
/// Dropping this context does not cancel other workers or the runtime.
pub struct WorkerContext {
    lease: Lease,
    identity: crate::routing::Scope,
    worker: String,
    build: String,
}
impl WorkerContext {
    pub(crate) fn new(lease: Lease, worker: &str, build: &str) -> Self {
        Self {
            identity: lease.scope(),
            lease,
            worker: worker.into(),
            build: build.into(),
        }
    }
    pub fn session_id(&self) -> &str {
        &self.identity.session
    }
    pub fn runtime_id(&self) -> &str {
        &self.identity.runtime
    }
    pub fn worker_id(&self) -> &str {
        &self.worker
    }
    pub fn build_sha256(&self) -> &str {
        &self.build
    }
    /// Private to this runtime and kept alive by the worker lease.
    pub fn runtime_directory(&self) -> &Path {
        &self.identity.directory
    }
    pub fn cancellation(&self) -> Cancellation {
        Cancellation(self.lease.cancellation())
    }
    /// Access the runtime's host imports or local native selection. This grants no host
    /// export permission and creates no display or SSH channel. Reads must run
    /// outside a compositor/event-loop thread; the worker lease fences access.
    pub fn clipboard(&self) -> io::Result<crate::clipboard_service::Reader> {
        self.lease.cancellation().check()?;
        Ok(crate::clipboard_service::Reader::new(
            self.lease.clipboard()?,
        ))
    }
    /// This worker's clipboard admission, for a process it relays a display
    /// to that outlives it (a stable display): that process reads and
    /// publishes as this worker, while this worker is admitted.
    pub fn clipboard_access(&self) -> io::Result<crate::clipboard_service::Access> {
        self.lease.cancellation().check()?;
        self.lease.clipboard()
    }
    /// Open native-provider IPC. Publication remains disabled until the owning
    /// runtime control connection assigns this worker. This grants no export.
    pub fn native_clipboard(&self) -> io::Result<crate::clipboard_service::NativePublisher> {
        self.lease.cancellation().check()?;
        crate::clipboard_service::NativePublisher::new(self.lease.clipboard()?)
    }
}

/// Read-only cancellation from the runtime's lifetime lease. A service owns
/// unblocking its input/output and cleaning up its children after cancellation.
#[derive(Clone)]
pub struct Cancellation(crate::transport::Cancellation);
impl Cancellation {
    pub fn check(&self) -> io::Result<()> {
        self.0.check()
    }
    pub fn is_cancelled(&self) -> bool {
        self.check().is_err()
    }
}

const CORE_NAMES: &[&str] = &[
    "clipboard-service",
    "clipboard-export",
    "git",
    "monitor",
    "integration",
    newport_protocol::browser::CAPABILITY,
    "gui-notices",
    "worker-stream",
    "lsp",
    "terminal",
    newport_protocol::terminal::holder::CAPABILITY,
];

pub(crate) fn capabilities(services: &[&dyn Service]) -> io::Result<BTreeMap<String, Capability>> {
    let mut result = BTreeMap::new();
    if services.len() + CORE_NAMES.len() + 1 > 16 {
        return Err(io::Error::other("Too many agent services"));
    }
    for service in services {
        let name = service.name();
        let capability = service.capability();
        if name.is_empty()
            || name.len() > 64
            || !name
                .bytes()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-')
            || CORE_NAMES.contains(&name)
            || name.starts_with("product-")
            || result.contains_key(name)
            || capability.versions.is_empty()
            || capability.versions.len() > 8
            || capability.versions.contains(&0)
            || capability
                .unavailable
                .as_ref()
                .is_some_and(|s| s.is_empty() || s.len() > 512 || s.contains('\0'))
        {
            return Err(io::Error::other(format!(
                "Invalid or duplicate agent service: {name}"
            )));
        }
        result.insert(name.into(), capability);
    }
    Ok(result)
}

pub(crate) fn transports(services: &[&dyn Service]) -> io::Result<BTreeMap<String, Transport>> {
    capabilities(services)?;
    let mut result = BTreeMap::from([
        ("git".into(), Transport::GIT),
        ("monitor".into(), Transport::MONITOR),
        ("terminal".into(), Transport::TERMINAL),
        ("integration".into(), Transport::INTEGRATION),
        ("lsp".into(), Transport::Opaque),
    ]);
    result.extend(
        services
            .iter()
            .map(|service| (service.name().into(), service.transport())),
    );
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Fixture(&'static str, Capability);
    impl Service for Fixture {
        fn name(&self) -> &'static str {
            self.0
        }
        fn capability(&self) -> Capability {
            self.1.clone()
        }
        fn transport(&self) -> Transport {
            Transport::Opaque
        }
        fn run(&self, _: WorkerContext, _: &[String]) -> io::Result<()> {
            unreachable!()
        }
    }
    #[test]
    fn registered_services_choose_their_transport_independently_of_client_input() {
        let fixture = Fixture(
            "foreign",
            Capability {
                contract_id: None,
                versions: vec![1],
                unavailable: None,
            },
        );
        let transports = transports(&[&fixture]).unwrap();
        assert_eq!(transports["git"], Transport::GIT);
        assert_eq!(transports["monitor"], Transport::MONITOR);
        assert_eq!(transports["terminal"], Transport::TERMINAL);
        assert_eq!(transports["integration"], Transport::INTEGRATION);
        assert_eq!(transports["lsp"], Transport::Opaque);
        assert_eq!(transports["foreign"], Transport::Opaque);
        assert!(!transports.contains_key("unknown"));
    }
    #[test]
    fn extensions_cannot_replace_core_services_or_publish_invalid_capabilities() {
        let ready = Capability {
            contract_id: None,
            versions: vec![1],
            unavailable: None,
        };
        for name in [
            "",
            "git",
            "integration",
            newport_protocol::browser::CAPABILITY,
            newport_protocol::terminal::holder::CAPABILITY,
            "worker-stream",
            "space name",
            "../worker",
        ] {
            assert!(capabilities(&[&Fixture(name, ready.clone())]).is_err());
        }
        let fixture = Fixture("fixture", ready.clone());
        assert!(capabilities(&[&fixture, &fixture]).is_err());
        for value in [
            Capability {
                versions: vec![],
                ..ready.clone()
            },
            Capability {
                versions: vec![0],
                ..ready.clone()
            },
            Capability {
                unavailable: Some(String::new()),
                ..ready
            },
        ] {
            assert!(capabilities(&[&Fixture("fixture", value)]).is_err());
        }
        assert!(
            capabilities(&[&fixture]).unwrap()["fixture"]
                .unavailable
                .is_none()
        );
    }
}
