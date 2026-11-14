//! Runtime announcements, control and authenticated worker leases.
//! Validation checks message shape; the runtime owns authorization and lifetime.
use crate::{clipboard, common, product, worker::admission};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, fmt};

#[cfg(feature = "protobuf")]
pub mod wire;

pub const VERSION: u32 = 1;
pub const MAX_WORKERS: usize = 16;
pub const MAX_REGISTRATIONS: usize = 4096;
pub const MAX_FRAME: usize = 16 * 1024;
pub const LIMITS: common::Limits = common::Limits {
    message: MAX_FRAME,
    metadata: MAX_FRAME,
};

/// The names Needle announces its services by and leases workers for. A
/// feature module with a capability of its own names it there, such as
/// [`crate::browser::CAPABILITY`].
pub mod service {
    pub const GIT: &str = "git";
    pub const MONITOR: &str = "monitor";
    pub const TERMINAL: &str = "terminal";
    pub const LSP: &str = "lsp";
    pub const INTEGRATION: &str = "integration";
    pub const REMOTE_APPS: &str = "remote-apps";
    pub const GUI_NOTICES: &str = "gui-notices";
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Capability {
    pub versions: Vec<u32>,
    pub unavailable: Option<String>,
    pub contract_id: Option<String>,
}

#[cfg(feature = "protobuf")]
pub fn service_contract_id(service: &str) -> Option<&'static str> {
    match service {
        service::MONITOR => Some(crate::monitor::CONTRACT_ID),
        _ => None,
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Limits {
    pub max_control_bytes: usize,
    pub max_inflight: usize,
    pub max_workers: usize,
    pub max_registrations: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_control_bytes: MAX_FRAME,
            max_inflight: 1,
            max_workers: MAX_WORKERS,
            max_registrations: MAX_REGISTRATIONS,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Hello {
    pub versions: Vec<u32>,
    pub instance_id: String,
    pub build_sha256: String,
    /// Semantic product version; absence means it has not been reported.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_version: Option<String>,
    pub worker_socket: String,
    pub product: product::Variant,
    pub services: BTreeMap<String, Capability>,
    pub dependencies: BTreeMap<String, bool>,
    pub limits: Limits,
}

impl Hello {
    /// Validate identity and announced values without deciding whether this
    /// desktop supports the announced versions.
    pub fn validate(&self) -> Result<(), Invalid> {
        if !versions(&self.versions)
            || !identifier(&self.instance_id)
            || !product::valid_build(&self.build_sha256)
            || self
                .agent_version
                .as_deref()
                .is_some_and(|version| !product::valid_version(version))
            || !socket_path(&self.worker_socket)
            || self.services.len() > 15
            || self.services.is_empty()
            || self.dependencies.len() > 16
            || self.services.iter().any(|(name, value)| {
                !service_name(name)
                    || !versions(&value.versions)
                    || value
                        .contract_id
                        .as_deref()
                        .is_some_and(|id| !common::valid_contract_id(id))
                    || value.unavailable.as_ref().is_some_and(|reason| {
                        reason.is_empty() || reason.len() > 512 || reason.contains('\0')
                    })
            })
            || self.dependencies.keys().any(|name| !service_name(name))
            || self.limits.max_control_bytes == 0
            || self.limits.max_control_bytes > MAX_FRAME
            || self.limits.max_inflight != 1
            || self.limits.max_workers == 0
            || self.limits.max_workers > MAX_WORKERS
            || self.limits.max_registrations == 0
            || self.limits.max_registrations > MAX_REGISTRATIONS
            || (self.product == product::Variant::Core
                && self.services.contains_key(service::REMOTE_APPS))
        {
            return Err(Invalid("runtime announcement"));
        }
        Ok(())
    }

    /// Availability can change after the announcement; version compatibility
    /// alone is enough to attempt a service whose dependency was just installed.
    pub fn require_compatible(
        &self,
        name: &str,
        version: u32,
    ) -> Result<&Capability, common::Error> {
        self.services
            .get(name)
            .filter(|service| service.versions.contains(&version))
            .ok_or_else(|| common::Error {
                code: "incompatible".into(),
                message: format!(
                    "Needle does not support {name} version {version}. Update Needle."
                ),
                retry: common::Retry::No,
            })
    }

    pub fn require(&self, name: &str, version: u32) -> Result<(), common::Error> {
        let service = self.require_compatible(name, version)?;
        match &service.unavailable {
            Some(reason) => Err(common::Error {
                code: "unavailable".into(),
                message: reason.clone(),
                retry: common::Retry::No,
            }),
            None => Ok(()),
        }
    }

    /// Check the schema before allocating a worker or exchanging service data.
    pub fn require_contract(
        &self,
        name: &str,
        version: u32,
        contract_id: Option<&str>,
    ) -> Result<&Capability, common::Error> {
        let capability = self.require_compatible(name, version)?;
        if capability.contract_id.as_deref() != contract_id {
            return Err(common::Error {
                code: "incompatible".into(),
                message: format!(
                    "Newport and Needle use different {name} protocols. Update Newport and Needle."
                ),
                retry: common::Retry::No,
            });
        }
        Ok(capability)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Permissions {
    pub revision: u64,
    pub clipboard: bool,
    pub browser: bool,
}

impl Permissions {
    pub fn validate(&self) -> Result<(), Invalid> {
        if self.revision == 0 {
            return Err(Invalid("permission revision"));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Applied {
    pub permissions: Permissions,
    pub error: Option<common::Error>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Operation {
    Initialize {
        version: u32,
        session_id: String,
        transport_generation: u64,
    },
    PrepareWorkerStream {
        worker_id: String,
        service: String,
        version: u32,
        contract_id: Option<String>,
    },
    Permissions {
        worker_id: String,
        permissions: Permissions,
    },
    ClipboardRoute(clipboard::Route),
    Ping,
    Shutdown,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Request {
    pub id: u64,
    pub runtime_id: String,
    pub operation: Operation,
}

impl Request {
    pub fn validate(&self) -> Result<(), Invalid> {
        correlation(self.id, &self.runtime_id)?;
        match &self.operation {
            Operation::Initialize {
                version,
                session_id,
                transport_generation,
            } if *version == 0 || !identifier(session_id) || *transport_generation == 0 => {
                Err(Invalid("runtime initialization"))
            }
            Operation::PrepareWorkerStream {
                worker_id,
                service,
                version,
                contract_id,
            } if !identifier(worker_id)
                || !service_name(service)
                || *version == 0
                || contract_id
                    .as_deref()
                    .is_some_and(|id| !common::valid_contract_id(id)) =>
            {
                Err(Invalid("worker registration"))
            }
            Operation::Permissions {
                worker_id,
                permissions,
            } => {
                if !identifier(worker_id) {
                    return Err(Invalid("worker identity"));
                }
                permissions.validate()
            }
            Operation::ClipboardRoute(route) => validate_route(route),
            _ => Ok(()),
        }
    }
}

#[derive(Clone, Debug)]
pub enum ReplyValue {
    Accepted,
    WorkerStream(admission::Endpoint),
    Permissions(Applied),
    ClipboardRoute(clipboard::RouteReply),
}

#[derive(Clone, Debug)]
pub struct Reply {
    pub id: u64,
    pub runtime_id: String,
    pub result: Result<ReplyValue, common::Error>,
}

impl Reply {
    pub fn validate(&self) -> Result<(), Invalid> {
        correlation(self.id, &self.runtime_id)?;
        match &self.result {
            Ok(ReplyValue::Accepted) => Ok(()),
            Ok(ReplyValue::WorkerStream(endpoint)) => endpoint.validate(),
            Ok(ReplyValue::Permissions(applied)) => applied.validate(),
            Ok(ReplyValue::ClipboardRoute(route)) => {
                if route.revision == 0
                    || route.worker_id.as_ref().is_some_and(|id| !identifier(id))
                    || (route.outcome == clipboard::RouteOutcome::Retired
                        && route.worker_id.is_none())
                {
                    return Err(Invalid("clipboard route reply"));
                }
                Ok(())
            }
            Err(error) => validate_error(error),
        }
    }

    /// A valid reply still has to answer this request and its expected result.
    pub fn validate_for(&self, request: &Request) -> Result<(), Invalid> {
        request.validate()?;
        self.validate()?;
        if self.id != request.id || self.runtime_id != request.runtime_id {
            return Err(Invalid("runtime reply identity"));
        }
        let Ok(value) = &self.result else {
            return Ok(());
        };
        match (&request.operation, value) {
            (
                Operation::Initialize { .. } | Operation::Ping | Operation::Shutdown,
                ReplyValue::Accepted,
            ) => Ok(()),
            (Operation::PrepareWorkerStream { .. }, ReplyValue::WorkerStream(_)) => Ok(()),
            (Operation::Permissions { permissions, .. }, ReplyValue::Permissions(applied)) => {
                applied.validate_for(permissions)
            }
            (Operation::ClipboardRoute(route), ReplyValue::ClipboardRoute(reply))
                if route.revision == reply.revision && route.worker_id == reply.worker_id =>
            {
                Ok(())
            }
            _ => Err(Invalid("runtime reply result")),
        }
    }
}

impl Applied {
    pub fn validate(&self) -> Result<(), Invalid> {
        self.permissions.validate()?;
        if let Some(error) = &self.error {
            validate_error(error)?;
        }
        Ok(())
    }

    pub fn validate_for(&self, requested: &Permissions) -> Result<(), Invalid> {
        requested.validate()?;
        self.validate()?;
        if self.permissions.revision != requested.revision
            || self.permissions.clipboard && !requested.clipboard
            || self.permissions.browser && !requested.browser
        {
            return Err(Invalid("permission acknowledgement"));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Attach {
    pub runtime_id: String,
    pub worker_id: String,
    pub build_sha256: String,
    pub service: String,
}

impl Attach {
    pub fn validate(&self) -> Result<(), Invalid> {
        if !identifier(&self.runtime_id)
            || !identifier(&self.worker_id)
            || !product::valid_build(&self.build_sha256)
            || !service_name(&self.service)
        {
            return Err(Invalid("worker attachment"));
        }
        Ok(())
    }
}

#[derive(Clone, Debug)]
pub struct Lease {
    pub session_id: String,
    pub clipboard: Option<clipboard::Access>,
}

#[derive(Clone, Debug)]
pub struct Attached {
    pub runtime_id: String,
    pub worker_id: String,
    pub result: Result<Lease, common::Error>,
}

impl Attached {
    pub fn validate_for(&self, request: &Attach, worker_socket: &str) -> Result<(), Invalid> {
        request.validate()?;
        self.validate()?;
        if self.runtime_id != request.runtime_id || self.worker_id != request.worker_id {
            return Err(Invalid("worker lease response identity"));
        }
        if let Ok(lease) = &self.result
            && let Some(access) = &lease.clipboard
            && access.path.to_str().and_then(|path| path.rsplit_once('/'))
                != worker_socket
                    .rsplit_once('/')
                    .map(|(directory, _)| (directory, "clipboard.sock"))
        {
            return Err(Invalid("clipboard access endpoint"));
        }
        Ok(())
    }

    pub fn validate(&self) -> Result<(), Invalid> {
        if !identifier(&self.runtime_id) || !identifier(&self.worker_id) {
            return Err(Invalid("worker lease identity"));
        }
        match &self.result {
            Ok(lease) => {
                if !identifier(&lease.session_id) {
                    return Err(Invalid("worker lease session"));
                }
                if let Some(access) = &lease.clipboard
                    && (!access.path.to_str().is_some_and(socket_path)
                        || access.runtime != self.runtime_id
                        || access.worker != self.worker_id
                        || !identifier(&access.token))
                {
                    return Err(Invalid("clipboard access"));
                }
                Ok(())
            }
            Err(error) => validate_error(error),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkerPermissions {
    pub runtime_id: String,
    pub worker_id: String,
    pub permissions: Permissions,
}

impl WorkerPermissions {
    pub fn validate(&self) -> Result<(), Invalid> {
        if !identifier(&self.runtime_id) || !identifier(&self.worker_id) {
            return Err(Invalid("worker permission identity"));
        }
        self.permissions.validate()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Invalid(pub(crate) &'static str);
impl fmt::Display for Invalid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "invalid {}", self.0)
    }
}
impl std::error::Error for Invalid {}

pub(crate) fn identifier(value: &str) -> bool {
    uuid::Uuid::parse_str(value).is_ok_and(|id| !id.is_nil())
}
pub(crate) fn service_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && !value.starts_with("product-")
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}
pub(crate) fn socket_path(value: &str) -> bool {
    value.starts_with('/')
        && value.len() <= 1024
        && !value.chars().any(char::is_control)
        && !value[1..]
            .split('/')
            .any(|part| matches!(part, "" | "." | ".."))
}
fn versions(values: &[u32]) -> bool {
    !values.is_empty()
        && values.len() <= 8
        && !values.contains(&0)
        && values
            .iter()
            .enumerate()
            .all(|(index, value)| !values[..index].contains(value))
}
fn correlation(id: u64, runtime: &str) -> Result<(), Invalid> {
    if id == 0 || !identifier(runtime) {
        return Err(Invalid("runtime request identity"));
    }
    Ok(())
}
fn validate_route(route: &clipboard::Route) -> Result<(), Invalid> {
    if route.revision == 0
        || route
            .worker_id
            .as_ref()
            .is_some_and(|worker| !identifier(worker))
        || (route.worker_id.is_none() && (route.exports || route.focused))
    {
        return Err(Invalid("clipboard route"));
    }
    Ok(())
}
pub(crate) fn validate_error(error: &common::Error) -> Result<(), Invalid> {
    if error.code.is_empty()
        || error.code.len() > 64
        || error.message.len() > 4096
        || error.code.chars().any(char::is_control)
        || error.message.contains('\0')
    {
        return Err(Invalid("runtime error"));
    }
    Ok(())
}

#[cfg(test)]
mod service_names {
    use super::service;

    /// The names are on the wire: a renamed constant must keep its value.
    #[test]
    fn service_names_keep_their_wire_values() {
        assert_eq!(
            [
                service::GIT,
                service::MONITOR,
                service::TERMINAL,
                service::LSP,
                service::INTEGRATION,
                service::REMOTE_APPS,
                service::GUI_NOTICES,
            ],
            [
                "git",
                "monitor",
                "terminal",
                "lsp",
                "integration",
                "remote-apps",
                "gui-notices"
            ]
        );
    }
}
