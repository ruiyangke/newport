use super::*;

type Result<T> = std::result::Result<T, Error>;
fn required<T>(value: Option<T>, field: &'static str) -> Result<T> {
    value.ok_or_else(|| Invalid(field).into())
}
fn size(value: usize) -> Result<u32> {
    u32::try_from(value).map_err(|_| Invalid("runtime limit").into())
}
fn native_size(value: u32) -> Result<usize> {
    usize::try_from(value).map_err(|_| Invalid("runtime limit").into())
}

pub(crate) fn error(value: &common::Error) -> protobuf::Error {
    protobuf::Error {
        code: value.code.clone(),
        message: value.message.clone(),
        retry: match value.retry {
            common::Retry::Unknown => 0,
            common::Retry::Yes => 1,
            common::Retry::No => 2,
        },
    }
}
pub(crate) fn from_error(value: protobuf::Error) -> Result<common::Error> {
    Ok(common::Error {
        code: value.code,
        message: value.message,
        retry: match value.retry {
            0 => common::Retry::Unknown,
            1 => common::Retry::Yes,
            2 => common::Retry::No,
            _ => return Err(Invalid("retry value").into()),
        },
    })
}
fn secret(value: &common::Secret) -> protobuf::Secret {
    protobuf::Secret {
        value: value.value.clone(),
    }
}
fn from_secret(value: Option<protobuf::Secret>) -> Result<common::Secret> {
    Ok(common::Secret {
        value: required(value, "secret")?.value,
    })
}

pub(crate) fn hello(value: &Hello) -> Result<pb::Hello> {
    Ok(pb::Hello {
        versions: value.versions.clone(),
        instance_id: value.instance_id.clone(),
        build_sha256: value.build_sha256.clone(),
        agent_version: value.agent_version.clone(),
        worker_socket: value.worker_socket.clone(),
        product: match value.product {
            product::Variant::Core => pb::Product::Core,
            product::Variant::Full => pb::Product::Full,
        }
        .into(),
        services: value
            .services
            .iter()
            .map(|(name, service)| {
                (
                    name.clone(),
                    pb::Capability {
                        versions: service.versions.clone(),
                        unavailable: service.unavailable.clone(),
                        contract_id: service.contract_id.clone(),
                    },
                )
            })
            .collect(),
        dependencies: value.dependencies.clone().into_iter().collect(),
        limits: Some(pb::Limits {
            max_control_bytes: size(value.limits.max_control_bytes)?,
            max_inflight: size(value.limits.max_inflight)?,
            max_workers: size(value.limits.max_workers)?,
            max_registrations: size(value.limits.max_registrations)?,
        }),
    })
}
pub(crate) fn from_hello(value: pb::Hello) -> Result<Hello> {
    let limits = required(value.limits, "runtime limits")?;
    Ok(Hello {
        versions: value.versions,
        instance_id: value.instance_id,
        build_sha256: value.build_sha256,
        agent_version: value.agent_version,
        worker_socket: value.worker_socket,
        product: match pb::Product::try_from(value.product) {
            Ok(pb::Product::Core) => product::Variant::Core,
            Ok(pb::Product::Full) => product::Variant::Full,
            _ => return Err(Invalid("runtime product").into()),
        },
        services: value
            .services
            .into_iter()
            .map(|(name, service)| {
                (
                    name,
                    Capability {
                        versions: service.versions,
                        unavailable: service.unavailable,
                        contract_id: service.contract_id,
                    },
                )
            })
            .collect(),
        dependencies: value.dependencies.into_iter().collect(),
        limits: Limits {
            max_control_bytes: native_size(limits.max_control_bytes)?,
            max_inflight: native_size(limits.max_inflight)?,
            max_workers: native_size(limits.max_workers)?,
            max_registrations: native_size(limits.max_registrations)?,
        },
    })
}
pub(crate) fn permission_values(value: &Permissions) -> pb::Permissions {
    pb::Permissions {
        revision: value.revision,
        clipboard: value.clipboard,
        browser: value.browser,
    }
}
pub(crate) fn from_permission_values(value: Option<pb::Permissions>) -> Result<Permissions> {
    let value = required(value, "permissions")?;
    Ok(Permissions {
        revision: value.revision,
        clipboard: value.clipboard,
        browser: value.browser,
    })
}
pub(crate) fn applied(value: &Applied) -> Result<pb::Applied> {
    Ok(pb::Applied {
        permissions: Some(permission_values(&value.permissions)),
        error: value.error.as_ref().map(error),
    })
}
pub(crate) fn from_applied(value: pb::Applied) -> Result<Applied> {
    Ok(Applied {
        permissions: from_permission_values(value.permissions)?,
        error: value.error.map(from_error).transpose()?,
    })
}

pub(super) fn request(value: &Request) -> Result<pb::Request> {
    use pb::request::Operation as Op;
    let operation = match &value.operation {
        Operation::Initialize {
            version,
            session_id,
            transport_generation,
        } => Op::Initialize(pb::request::Initialize {
            version: *version,
            session_id: session_id.clone(),
            transport_generation: *transport_generation,
        }),
        Operation::PrepareWorkerStream {
            worker_id,
            service,
            version,
            contract_id,
        } => Op::PrepareWorkerStream(pb::request::PrepareWorkerStream {
            worker_id: worker_id.clone(),
            service: service.clone(),
            version: *version,
            contract_id: contract_id.clone(),
        }),
        Operation::Permissions {
            worker_id,
            permissions,
        } => Op::Permissions(pb::request::WorkerPermissionUpdate {
            worker_id: worker_id.clone(),
            permissions: Some(permission_values(permissions)),
        }),
        Operation::ClipboardRoute(value) => Op::ClipboardRoute(pb::ClipboardRoute {
            revision: value.revision,
            worker_id: value.worker_id.clone(),
            exports: value.exports,
            focused: value.focused,
        }),
        Operation::Ping => Op::Ping(protobuf::Present {}),
        Operation::Shutdown => Op::Shutdown(protobuf::Present {}),
    };
    Ok(pb::Request {
        id: value.id,
        runtime_id: value.runtime_id.clone(),
        operation: Some(operation),
    })
}
pub(super) fn from_request(value: pb::Request) -> Result<Request> {
    use pb::request::Operation as Op;
    let operation = match required(value.operation, "runtime operation")? {
        Op::Initialize(value) => Operation::Initialize {
            version: value.version,
            session_id: value.session_id,
            transport_generation: value.transport_generation,
        },
        Op::PrepareWorkerStream(value) => Operation::PrepareWorkerStream {
            worker_id: value.worker_id,
            service: value.service,
            version: value.version,
            contract_id: value.contract_id,
        },
        Op::Permissions(value) => Operation::Permissions {
            worker_id: value.worker_id,
            permissions: from_permission_values(value.permissions)?,
        },
        Op::ClipboardRoute(value) => Operation::ClipboardRoute(clipboard::Route {
            revision: value.revision,
            worker_id: value.worker_id,
            exports: value.exports,
            focused: value.focused,
        }),
        Op::Ping(_) => Operation::Ping,
        Op::Shutdown(_) => Operation::Shutdown,
    };
    Ok(Request {
        id: value.id,
        runtime_id: value.runtime_id,
        operation,
    })
}
pub(super) fn reply(value: &Reply) -> Result<pb::Reply> {
    use pb::reply::Result as R;
    let result = match &value.result {
        Ok(ReplyValue::Accepted) => R::Accepted(protobuf::Present {}),
        Ok(ReplyValue::WorkerStream(value)) => R::WorkerStream(pb::Endpoint {
            path: value.path.clone(),
            ticket: Some(secret(&value.ticket)),
        }),
        Ok(ReplyValue::Permissions(value)) => R::Permissions(applied(value)?),
        Ok(ReplyValue::ClipboardRoute(value)) => R::ClipboardRoute(pb::RouteReply {
            revision: value.revision,
            worker_id: value.worker_id.clone(),
            outcome: match value.outcome {
                clipboard::RouteOutcome::Applied => pb::RouteOutcome::Applied,
                clipboard::RouteOutcome::Retired => pb::RouteOutcome::Retired,
            }
            .into(),
        }),
        Err(value) => R::Error(error(value)),
    };
    Ok(pb::Reply {
        id: value.id,
        runtime_id: value.runtime_id.clone(),
        result: Some(result),
    })
}
pub(super) fn from_reply(value: pb::Reply) -> Result<Reply> {
    use pb::reply::Result as R;
    let result = match required(value.result, "runtime reply result")? {
        R::Accepted(_) => Ok(ReplyValue::Accepted),
        R::WorkerStream(value) => Ok(ReplyValue::WorkerStream(admission::Endpoint {
            path: value.path,
            ticket: from_secret(value.ticket)?,
        })),
        R::Permissions(value) => Ok(ReplyValue::Permissions(from_applied(value)?)),
        R::ClipboardRoute(value) => Ok(ReplyValue::ClipboardRoute(clipboard::RouteReply {
            revision: value.revision,
            worker_id: value.worker_id,
            outcome: match pb::RouteOutcome::try_from(value.outcome) {
                Ok(pb::RouteOutcome::Applied) => clipboard::RouteOutcome::Applied,
                Ok(pb::RouteOutcome::Retired) => clipboard::RouteOutcome::Retired,
                _ => return Err(Invalid("clipboard route outcome").into()),
            },
        })),
        R::Error(value) => Err(from_error(value)?),
    };
    Ok(Reply {
        id: value.id,
        runtime_id: value.runtime_id,
        result,
    })
}
pub(super) fn attach(value: &Attach) -> Result<pb::Attach> {
    Ok(pb::Attach {
        runtime_id: value.runtime_id.clone(),
        worker_id: value.worker_id.clone(),
        build_sha256: value.build_sha256.clone(),
        service: value.service.clone(),
    })
}
pub(super) fn from_attach(value: pb::Attach) -> Result<Attach> {
    Ok(Attach {
        runtime_id: value.runtime_id,
        worker_id: value.worker_id,
        build_sha256: value.build_sha256,
        service: value.service,
    })
}
pub(super) fn attached(value: &Attached) -> Result<pb::Attached> {
    use pb::attached::Result as R;
    let result = match &value.result {
        Ok(value) => R::Lease(pb::Lease {
            session_id: value.session_id.clone(),
            clipboard: value
                .clipboard
                .as_ref()
                .map(crate::clipboard::service::wire::access_to_wire)
                .transpose()?,
        }),
        Err(value) => R::Error(error(value)),
    };
    Ok(pb::Attached {
        runtime_id: value.runtime_id.clone(),
        worker_id: value.worker_id.clone(),
        result: Some(result),
    })
}
pub(super) fn from_attached(value: pb::Attached) -> Result<Attached> {
    use pb::attached::Result as R;
    let result = match required(value.result, "worker lease result")? {
        R::Lease(value) => Ok(Lease {
            session_id: value.session_id,
            clipboard: value
                .clipboard
                .map(crate::clipboard::service::wire::access_from_wire)
                .transpose()?,
        }),
        R::Error(value) => Err(from_error(value)?),
    };
    Ok(Attached {
        runtime_id: value.runtime_id,
        worker_id: value.worker_id,
        result,
    })
}
pub(super) fn permissions(value: &WorkerPermissions) -> Result<pb::WorkerPermissions> {
    Ok(pb::WorkerPermissions {
        runtime_id: value.runtime_id.clone(),
        worker_id: value.worker_id.clone(),
        permissions: Some(permission_values(&value.permissions)),
    })
}
pub(super) fn from_permissions(value: pb::WorkerPermissions) -> Result<WorkerPermissions> {
    Ok(WorkerPermissions {
        runtime_id: value.runtime_id,
        worker_id: value.worker_id,
        permissions: from_permission_values(value.permissions)?,
    })
}
pub(super) fn open(value: &admission::Open) -> Result<pb::Open> {
    Ok(pb::Open {
        version: value.version,
        runtime_id: value.runtime_id.clone(),
        worker_id: value.worker_id.clone(),
        build_sha256: value.build_sha256.clone(),
        ticket: Some(secret(&value.ticket)),
        service: value.service.clone(),
        service_version: value.service_version,
        contract_id: value.contract_id.clone(),
        arguments: value.arguments.clone(),
    })
}
pub(super) fn from_open(value: pb::Open) -> Result<admission::Open> {
    Ok(admission::Open {
        version: value.version,
        runtime_id: value.runtime_id,
        worker_id: value.worker_id,
        build_sha256: value.build_sha256,
        ticket: from_secret(value.ticket)?,
        service: value.service,
        service_version: value.service_version,
        contract_id: value.contract_id,
        arguments: value.arguments,
    })
}
