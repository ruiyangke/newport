//! Contract admission precedes generated decoding of a helper's observations.
use super::{Reply, Request};
use crate::{
    common,
    encoding::protobuf::{self, FrameError, fields, generated::newport::discovery as pb},
    monitor::{
        self, Invalid,
        ports::discovery::{Snapshot, wire::Record},
        wire::scan,
    },
};
use bytes::Bytes;
use prost::Message;
use std::io::{self, Read, Write};

pub const REQUEST_LIMITS: common::Limits = common::Limits {
    message: monitor::SERVER_LIMITS.message,
    metadata: monitor::SERVER_LIMITS.metadata,
};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Wire(#[from] monitor::wire::Error),
    #[error(transparent)]
    Refused(#[from] common::Error),
}

impl From<FrameError> for Error {
    fn from(error: FrameError) -> Self {
        Self::Wire(error.into())
    }
}
impl From<Invalid> for Error {
    fn from(error: Invalid) -> Self {
        Self::Wire(error.into())
    }
}
impl From<Error> for io::Error {
    fn from(error: Error) -> Self {
        match error {
            Error::Wire(error) => error.into(),
            Error::Refused(error) => Self::new(io::ErrorKind::InvalidData, error),
        }
    }
}

pub fn require_contract_id(actual: Option<&str>) -> Result<(), common::Error> {
    if actual == Some(monitor::CONTRACT_ID) {
        Ok(())
    } else {
        Err(common::Error {
            code: "incompatible".into(),
            message: "The discovery helper does not match Needle. Update Needle and retry.".into(),
            retry: common::Retry::No,
        })
    }
}

fn bound(body: &[u8], limits: &common::Limits) -> Result<(), Error> {
    if body.len() > limits.message {
        return Err(FrameError::TooLarge {
            length: u64::try_from(body.len()).unwrap_or(u64::MAX),
            limit: limits.message,
        }
        .into());
    }
    Ok(())
}

fn encode<M: Message + protobuf::Layout>(
    value: &M,
    limits: &common::Limits,
    check: impl FnOnce(&[u8]) -> Result<(), monitor::wire::Error>,
) -> Result<Vec<u8>, Error> {
    if value.encoded_len() > limits.message {
        return Err(FrameError::TooLarge {
            length: u64::try_from(value.encoded_len()).unwrap_or(u64::MAX),
            limit: limits.message,
        }
        .into());
    }
    check(&value.encode_to_vec())?;
    let mut frame = Vec::new();
    protobuf::write_message(&mut frame, value, limits)?;
    Ok(frame)
}

pub fn encode_request(value: &Request) -> Result<Vec<u8>, Error> {
    value.validate()?;
    encode(
        &pb::HelperRequest {
            contract_id: value.contract_id.clone(),
            generation: value.generation,
            projects: value
                .projects
                .as_ref()
                .map(|snapshot| Box::new(snapshot.to_wire())),
        },
        &REQUEST_LIMITS,
        |body| scan::check_helper_request(body, true),
    )
}

pub fn decode_request(body: Bytes) -> Result<Request, Error> {
    bound(&body, &REQUEST_LIMITS)?;
    scan::check_helper_request(&body, false)?;
    let mut envelope = body.as_ref();
    let mut contract = None;
    let mut projects = false;
    while !envelope.is_empty() {
        let field = fields::field(&mut envelope)?;
        match field.number {
            1 => {
                contract = Some(
                    std::str::from_utf8(field.bytes()?)
                        .map_err(|_| Invalid("discovery helper contract identifier"))?,
                )
            }
            3 => projects = true,
            _ => {}
        }
    }
    if projects {
        require_contract_id(contract)?;
    }
    scan::check_helper_request(&body, true)?;
    let value: pb::HelperRequest = protobuf::decode(body, &REQUEST_LIMITS)?;
    let request = Request {
        contract_id: value.contract_id,
        generation: value.generation,
        projects: value
            .projects
            .map(|snapshot| Snapshot::from_wire(*snapshot).map(Box::new))
            .transpose()?,
    };
    request.validate()?;
    Ok(request)
}

pub fn encode_reply(value: &Reply, request: &Request) -> Result<Vec<u8>, Error> {
    value.validate_for(request)?;
    require_contract_id(value.contract_id.as_deref())?;
    use pb::helper_reply::Result as ResultKind;
    let result = match &value.result {
        Ok(snapshot) => ResultKind::Snapshot(Box::new(snapshot.to_wire())),
        Err(error) => ResultKind::Error(protobuf::Error {
            code: error.code.clone(),
            message: error.message.clone(),
            retry: match error.retry {
                common::Retry::Unknown => 0,
                common::Retry::Yes => 1,
                common::Retry::No => 2,
            },
        }),
    };
    encode(
        &pb::HelperReply {
            contract_id: value.contract_id.clone(),
            generation: value.generation,
            result: Some(result),
        },
        &monitor::SERVER_LIMITS,
        |body| scan::check_helper_reply(body, true),
    )
}

fn admit(mut body: &[u8]) -> Result<(), Error> {
    scan::check_helper_reply(body, false)?;
    let mut contract = None;
    while !body.is_empty() {
        let field = fields::field(&mut body)?;
        if field.number == 1 {
            contract = Some(
                std::str::from_utf8(field.bytes()?)
                    .map_err(|_| Invalid("discovery helper contract identifier"))?,
            );
        }
    }
    require_contract_id(contract)?;
    Ok(())
}

pub fn decode_reply(body: Bytes, request: &Request) -> Result<Reply, Error> {
    request.validate()?;
    bound(&body, &monitor::SERVER_LIMITS)?;
    admit(&body)?;
    scan::check_helper_reply(&body, true)?;
    let value: pb::HelperReply = protobuf::decode(body, &monitor::SERVER_LIMITS)?;
    use pb::helper_reply::Result as ResultKind;
    let result = match value.result.ok_or(Invalid("discovery helper result"))? {
        ResultKind::Snapshot(snapshot) => Ok(Box::new(Snapshot::from_wire(*snapshot)?)),
        ResultKind::Error(error) => Err(common::Error {
            code: error.code,
            message: error.message,
            retry: match error.retry {
                0 => common::Retry::Unknown,
                1 => common::Retry::Yes,
                2 => common::Retry::No,
                _ => return Err(Invalid("discovery helper retry value").into()),
            },
        }),
    };
    let reply = Reply {
        contract_id: value.contract_id,
        generation: value.generation,
        result,
    };
    reply.validate_for(request)?;
    Ok(reply)
}

pub fn read_request(input: &mut impl Read) -> Result<Request, Error> {
    decode_request(
        protobuf::read_frame(input, REQUEST_LIMITS.message)?.ok_or(monitor::wire::Error::Ended)?,
    )
}

pub fn read_reply(input: &mut impl Read, request: &Request) -> Result<Reply, Error> {
    decode_reply(
        protobuf::read_frame(input, monitor::SERVER_LIMITS.message)?
            .ok_or(monitor::wire::Error::Ended)?,
        request,
    )
}

pub fn write_reply(output: &mut impl Write, value: &Reply, request: &Request) -> Result<(), Error> {
    output
        .write_all(&encode_reply(value, request)?)
        .map_err(FrameError::from)?;
    output.flush().map_err(FrameError::from)?;
    Ok(())
}

#[cfg(test)]
mod tests;
