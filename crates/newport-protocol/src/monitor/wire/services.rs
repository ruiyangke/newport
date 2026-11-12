use super::super::services as contract;
use super::{Invalid, Record, pb, required};

impl Record for contract::Scope {
    type Wire = i32;
    fn to_wire(&self) -> i32 {
        match self {
            Self::System => 1,
            Self::User => 2,
        }
    }
    fn from_wire(value: i32) -> Result<Self, Invalid> {
        Ok(match value {
            1 => Self::System,
            2 => Self::User,
            _ => return Err(Invalid("services Scope")),
        })
    }
}

impl Record for contract::Privilege {
    type Wire = i32;
    fn to_wire(&self) -> i32 {
        match self {
            Self::Root => 1,
            Self::Sudo => 2,
            Self::Password => 3,
            Self::None => 4,
        }
    }
    fn from_wire(value: i32) -> Result<Self, Invalid> {
        Ok(match value {
            1 => Self::Root,
            2 => Self::Sudo,
            3 => Self::Password,
            4 => Self::None,
            _ => return Err(Invalid("services Privilege")),
        })
    }
}

impl Record for contract::Verb {
    type Wire = i32;
    fn to_wire(&self) -> i32 {
        match self {
            Self::Start => 1,
            Self::Stop => 2,
            Self::Restart => 3,
            Self::Reload => 4,
            Self::Enable => 5,
            Self::Disable => 6,
        }
    }
    fn from_wire(value: i32) -> Result<Self, Invalid> {
        Ok(match value {
            1 => Self::Start,
            2 => Self::Stop,
            3 => Self::Restart,
            4 => Self::Reload,
            5 => Self::Enable,
            6 => Self::Disable,
            _ => return Err(Invalid("services Verb")),
        })
    }
}

impl Record for contract::Priority {
    type Wire = i32;
    fn to_wire(&self) -> i32 {
        match self {
            Self::Err => 1,
            Self::Warning => 2,
        }
    }
    fn from_wire(value: i32) -> Result<Self, Invalid> {
        Ok(match value {
            1 => Self::Err,
            2 => Self::Warning,
            _ => return Err(Invalid("services Priority")),
        })
    }
}

impl Record for contract::ExitKind {
    type Wire = i32;
    fn to_wire(&self) -> i32 {
        match self {
            Self::Exited => 1,
            Self::Killed => 2,
            Self::Dumped => 3,
        }
    }
    fn from_wire(value: i32) -> Result<Self, Invalid> {
        Ok(match value {
            1 => Self::Exited,
            2 => Self::Killed,
            3 => Self::Dumped,
            _ => return Err(Invalid("services ExitKind")),
        })
    }
}

impl Record for contract::LogsRequest {
    type Wire = pb::ServicesLogsRequest;
    fn to_wire(&self) -> Self::Wire {
        Self::Wire {
            scope: self.scope.to_wire(),
            unit: self.unit.clone(),
            priority: self.priority.as_ref().map(Record::to_wire),
            cursor: self.cursor.clone(),
            lines: self.lines,
        }
    }
    fn from_wire(value: Self::Wire) -> Result<Self, Invalid> {
        Ok(Self {
            scope: contract::Scope::from_wire(value.scope)?,
            unit: value.unit,
            priority: value
                .priority
                .map(contract::Priority::from_wire)
                .transpose()?,
            cursor: value.cursor,
            lines: value.lines,
        })
    }
}

impl Record for contract::Service {
    type Wire = pb::ServicesService;
    fn to_wire(&self) -> Self::Wire {
        Self::Wire {
            name: self.name.clone(),
            scope: self.scope.to_wire(),
            description: self.description.clone(),
            load: self.load.clone(),
            active: self.active.clone(),
            sub: self.sub.clone(),
            enabled: self.enabled.clone(),
            main_pid: self.main_pid,
            since: self.since,
            memory: self.memory,
            cpu: self.cpu,
            restarts: self.restarts,
            result: self.result.clone(),
            exit_code: self.exit_code,
            exit_kind: self.exit_kind.as_ref().map(Record::to_wire),
            fragment: self.fragment.clone(),
            drop_ins: self.drop_ins.clone(),
            triggered_by: self.triggered_by.clone(),
            user: self.user.clone(),
        }
    }
    fn from_wire(value: Self::Wire) -> Result<Self, Invalid> {
        Ok(Self {
            name: value.name,
            scope: contract::Scope::from_wire(value.scope)?,
            description: value.description,
            load: value.load,
            active: value.active,
            sub: value.sub,
            enabled: value.enabled,
            main_pid: value.main_pid,
            since: value.since,
            memory: value.memory,
            cpu: value.cpu,
            restarts: value.restarts,
            result: value.result,
            exit_code: value.exit_code,
            exit_kind: value
                .exit_kind
                .map(contract::ExitKind::from_wire)
                .transpose()?,
            fragment: value.fragment,
            drop_ins: value.drop_ins,
            triggered_by: value.triggered_by,
            user: value.user,
        })
    }
}

impl Record for contract::Timer {
    type Wire = pb::ServicesTimer;
    fn to_wire(&self) -> Self::Wire {
        Self::Wire {
            name: self.name.clone(),
            scope: self.scope.to_wire(),
            description: self.description.clone(),
            active: self.active.clone(),
            next: self.next,
            last: self.last,
            unit: self.unit.clone(),
        }
    }
    fn from_wire(value: Self::Wire) -> Result<Self, Invalid> {
        Ok(Self {
            name: value.name,
            scope: contract::Scope::from_wire(value.scope)?,
            description: value.description,
            active: value.active,
            next: value.next,
            last: value.last,
            unit: value.unit,
        })
    }
}

impl Record for contract::UserManager {
    type Wire = pb::ServicesUserManager;
    fn to_wire(&self) -> Self::Wire {
        Self::Wire {
            available: self.available,
            reason: self.reason.clone(),
        }
    }
    fn from_wire(value: Self::Wire) -> Result<Self, Invalid> {
        Ok(Self {
            available: value.available,
            reason: value.reason,
        })
    }
}

impl Record for contract::Snapshot {
    type Wire = pb::ServicesSnapshot;
    fn to_wire(&self) -> Self::Wire {
        Self::Wire {
            services: self.services.iter().map(Record::to_wire).collect(),
            timers: self.timers.iter().map(Record::to_wire).collect(),
            user: Some(self.user.to_wire()),
            privilege: self.privilege.to_wire(),
        }
    }
    fn from_wire(value: Self::Wire) -> Result<Self, Invalid> {
        Ok(Self {
            services: value
                .services
                .into_iter()
                .map(contract::Service::from_wire)
                .collect::<Result<_, _>>()?,
            timers: value
                .timers
                .into_iter()
                .map(contract::Timer::from_wire)
                .collect::<Result<_, _>>()?,
            user: contract::UserManager::from_wire(required(value.user)?)?,
            privilege: contract::Privilege::from_wire(value.privilege)?,
        })
    }
}

impl Record for contract::Entry {
    type Wire = pb::ServicesEntry;
    fn to_wire(&self) -> Self::Wire {
        Self::Wire {
            at: self.at,
            priority: self.priority.map(u32::from),
            pid: self.pid,
            identifier: self.identifier.clone(),
            message: self.message.clone(),
        }
    }
    fn from_wire(value: Self::Wire) -> Result<Self, Invalid> {
        Ok(Self {
            at: value.at,
            priority: value
                .priority
                .map(|v| u8::try_from(v).map_err(|_| Invalid("services priority")))
                .transpose()?,
            pid: value.pid,
            identifier: value.identifier,
            message: value.message,
        })
    }
}

impl Record for contract::Logs {
    type Wire = pb::ServicesLogs;
    fn to_wire(&self) -> Self::Wire {
        Self::Wire {
            entries: self.entries.iter().map(Record::to_wire).collect(),
            cursor: self.cursor.clone(),
        }
    }
    fn from_wire(value: Self::Wire) -> Result<Self, Invalid> {
        Ok(Self {
            entries: value
                .entries
                .into_iter()
                .map(contract::Entry::from_wire)
                .collect::<Result<_, _>>()?,
            cursor: value.cursor,
        })
    }
}

impl Record for contract::UnitFile {
    type Wire = pb::ServicesUnitFile;
    fn to_wire(&self) -> Self::Wire {
        Self::Wire {
            text: self.text.clone(),
        }
    }
    fn from_wire(value: Self::Wire) -> Result<Self, Invalid> {
        Ok(Self { text: value.text })
    }
}

impl Record for contract::ActionReply {
    type Wire = pb::ServicesActionReply;
    fn to_wire(&self) -> Self::Wire {
        Self::Wire {
            ok: self.ok,
            needs_password: self.needs_password,
            command: self.command.clone(),
        }
    }
    fn from_wire(value: Self::Wire) -> Result<Self, Invalid> {
        Ok(Self {
            ok: value.ok,
            needs_password: value.needs_password,
            command: value.command,
        })
    }
}

impl Record for contract::Request {
    type Wire = pb::ServicesRequest;
    fn to_wire(&self) -> Self::Wire {
        use pb::services_request::Operation;
        let operation = match self {
            Self::Snapshot => Operation::Snapshot(crate::encoding::protobuf::Present {}),
            Self::Logs(v) => Operation::Logs(v.to_wire()),
            Self::Unit { scope, unit } => Operation::Unit(pb::ServicesUnit {
                scope: scope.to_wire(),
                unit: unit.clone(),
            }),
            Self::Action { scope, unit, verb } => Operation::Action(pb::ServicesAction {
                scope: scope.to_wire(),
                unit: unit.clone(),
                verb: verb.to_wire(),
            }),
        };
        Self::Wire {
            operation: Some(operation),
        }
    }
    fn from_wire(value: Self::Wire) -> Result<Self, Invalid> {
        use pb::services_request::Operation;
        Ok(match required(value.operation)? {
            Operation::Snapshot(_) => Self::Snapshot,
            Operation::Logs(v) => Self::Logs(contract::LogsRequest::from_wire(v)?),
            Operation::Unit(v) => Self::Unit {
                scope: contract::Scope::from_wire(v.scope)?,
                unit: v.unit,
            },
            Operation::Action(v) => Self::Action {
                scope: contract::Scope::from_wire(v.scope)?,
                unit: v.unit,
                verb: contract::Verb::from_wire(v.verb)?,
            },
        })
    }
}
impl Record for contract::Reply {
    type Wire = pb::ServicesReply;
    fn to_wire(&self) -> Self::Wire {
        use pb::services_reply::Result;
        let result = match self {
            Self::Snapshot(v) => Result::Snapshot(v.to_wire()),
            Self::Logs(v) => Result::Logs(v.to_wire()),
            Self::Unit(v) => Result::Unit(v.to_wire()),
            Self::Action(v) => Result::Action(v.to_wire()),
        };
        Self::Wire {
            result: Some(result),
        }
    }
    fn from_wire(value: Self::Wire) -> std::result::Result<Self, Invalid> {
        use pb::services_reply::Result;
        Ok(match required(value.result)? {
            Result::Snapshot(v) => Self::Snapshot(contract::Snapshot::from_wire(v)?),
            Result::Logs(v) => Self::Logs(contract::Logs::from_wire(v)?),
            Result::Unit(v) => Self::Unit(contract::UnitFile::from_wire(v)?),
            Result::Action(v) => Self::Action(contract::ActionReply::from_wire(v)?),
        })
    }
}
