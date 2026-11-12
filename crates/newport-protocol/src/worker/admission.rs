//! Single-use admission to a registered worker of the announced build.
use crate::{
    common,
    runtime::{self, Invalid},
};

pub const CAPABILITY: &str = "worker-stream";
pub const VERSION: u32 = 1;
pub const MAX_ARGUMENTS: usize = 128;
pub const MAX_ARGUMENT_BYTES: usize = 64 * 1024;
pub const LIMITS: common::Limits = common::Limits {
    message: 72 * 1024,
    metadata: 72 * 1024,
};

#[derive(Clone, Debug)]
pub struct Endpoint {
    pub path: String,
    pub ticket: common::Secret,
}

impl Endpoint {
    pub fn validate(&self) -> Result<(), Invalid> {
        if !runtime::socket_path(&self.path) || !valid_ticket(&self.ticket) {
            return Err(Invalid("worker endpoint"));
        }
        Ok(())
    }
}

#[derive(Clone, Debug)]
pub struct Open {
    pub version: u32,
    pub runtime_id: String,
    pub worker_id: String,
    pub build_sha256: String,
    pub ticket: common::Secret,
    pub service: String,
    pub service_version: u32,
    pub contract_id: Option<String>,
    pub arguments: Vec<String>,
}

impl Open {
    pub fn validate(&self) -> Result<(), Invalid> {
        if self.version != VERSION
            || !runtime::identifier(&self.runtime_id)
            || !runtime::identifier(&self.worker_id)
            || !crate::product::valid_build(&self.build_sha256)
            || !valid_ticket(&self.ticket)
            || !runtime::service_name(&self.service)
            || self.service_version == 0
            || self
                .contract_id
                .as_deref()
                .is_some_and(|id| !common::valid_contract_id(id))
            || self.arguments.len() > MAX_ARGUMENTS
            || self
                .arguments
                .iter()
                .any(|value| value.len() > 16384 || value.contains('\0'))
            || self.arguments.iter().map(String::len).sum::<usize>() > MAX_ARGUMENT_BYTES
        {
            return Err(Invalid("worker admission"));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Started {
    pub runtime_id: String,
    pub worker_id: String,
    pub build_sha256: String,
}

impl Started {
    pub fn validate(&self) -> Result<(), Invalid> {
        if !runtime::identifier(&self.runtime_id)
            || !runtime::identifier(&self.worker_id)
            || !crate::product::valid_build(&self.build_sha256)
        {
            return Err(Invalid("worker admission result"));
        }
        Ok(())
    }

    pub fn validate_for(&self, open: &Open) -> Result<(), Invalid> {
        open.validate()?;
        self.validate()?;
        if self.runtime_id != open.runtime_id
            || self.worker_id != open.worker_id
            || self.build_sha256 != open.build_sha256
        {
            return Err(Invalid("worker admission result identity"));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Admission {
    Started(Started),
    Refused(common::Error),
}

impl Admission {
    pub fn validate(&self) -> Result<(), Invalid> {
        match self {
            Self::Started(started) => started.validate(),
            Self::Refused(error) => runtime::validate_error(error),
        }
    }

    pub fn validate_for(&self, open: &Open) -> Result<(), Invalid> {
        open.validate()?;
        match self {
            Self::Started(started) => started.validate_for(open),
            Self::Refused(error) => runtime::validate_error(error),
        }
    }
}

fn valid_ticket(ticket: &common::Secret) -> bool {
    ticket.value.len() == 64 && ticket.value.iter().all(u8::is_ascii_hexdigit)
}
