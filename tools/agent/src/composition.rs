//! A host executable can register services without adding private dependencies
//! to the core agent. The host owns early helper/supervisor dispatch; call the
//! shared CLI only after that dispatch, before starting unrelated threads.
use crate::{runtime_protocol::Capability, worker_session::Lease};
use std::{collections::BTreeMap, io, path::Path};

/// A compiled service. Availability describes host support, not account access
/// or sharing permission: each service must enforce those before doing work.
pub trait Service: Sync {
    fn name(&self) -> &'static str;
    fn capability(&self) -> Capability;
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
    "gui-notices",
    "gui-launch",
    "gui-attachment-check",
    "worker-stream",
    "lsp",
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
            || capability.reason.as_ref().is_some_and(|s| s.len() > 512)
            || (!capability.available && capability.reason.as_deref().is_none_or(str::is_empty))
        {
            return Err(io::Error::other(format!(
                "Invalid or duplicate agent service: {name}"
            )));
        }
        result.insert(name.into(), capability);
    }
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
        fn run(&self, _: WorkerContext, _: &[String]) -> io::Result<()> {
            unreachable!()
        }
    }
    #[test]
    fn extensions_cannot_replace_core_services_or_publish_invalid_capabilities() {
        let ready = Capability {
            versions: vec![1],
            available: true,
            reason: None,
        };
        for name in [
            "",
            "git",
            "integration",
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
                available: false,
                ..ready
            },
        ] {
            assert!(capabilities(&[&Fixture("fixture", value)]).is_err());
        }
        assert!(capabilities(&[&fixture]).unwrap()["fixture"].available);
    }
}
