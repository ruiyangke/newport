//! One bounded catalogue result for a runtime-admitted discovery worker.
use super::{Application, Catalogue, DesktopAction, Invalid, Reply, SERVER_LIMITS};
use crate::{
    common,
    encoding::protobuf::{self, FrameError, fields, generated::newport::remote::catalogue as pb},
};
use bytes::Bytes;
use std::io::{Read, Write};

#[cfg(test)]
mod tests;

pub const MAX_FRAMED_BYTES: usize = SERVER_LIMITS.message + 10;
const MAX_FIELDS: usize = 256 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Frame(#[from] FrameError),
    #[error(transparent)]
    Invalid(#[from] Invalid),
    #[error(transparent)]
    Control(#[from] crate::worker::wire::Error),
    #[error("Catalogue worker ended without a result")]
    Ended,
    #[error("Unexpected data after the catalogue result")]
    Trailing,
}

pub fn encode(reply: &Reply) -> Result<Vec<u8>, Error> {
    reply.validate()?;
    let message = match reply {
        Reply::Catalogue(value) => pb::server_frame::Message::Catalogue(pb::Catalogue {
            apps: value
                .apps
                .iter()
                .map(|app| pb::Application {
                    id: app.id.clone(),
                    name: app.name.clone(),
                    description: app.description.clone(),
                    keywords: app.keywords.clone(),
                    icon: app.icon.clone(),
                    actions: app
                        .actions
                        .iter()
                        .map(|action| pb::DesktopAction {
                            id: action.id.clone(),
                            name: action.name.clone(),
                        })
                        .collect(),
                })
                .collect(),
        }),
        Reply::Failed(error) => pb::server_frame::Message::Error(protobuf::Error {
            code: error.code.clone(),
            message: error.message.clone(),
            retry: match error.retry {
                common::Retry::Unknown => 0,
                common::Retry::Yes => 1,
                common::Retry::No => 2,
            },
        }),
    };
    let mut output = Vec::new();
    protobuf::write_message(
        &mut output,
        &pb::ServerFrame {
            message: Some(message),
        },
        &SERVER_LIMITS,
    )?;
    Ok(output)
}

pub fn decode(body: Bytes) -> Result<Reply, Error> {
    crate::worker::wire::reject_controls(&body, SERVER_LIMITS.message)?;
    let mut remaining = MAX_FIELDS;
    scan(&body, Shape::Frame, &mut remaining)?;
    let frame: pb::ServerFrame = protobuf::decode(body, &SERVER_LIMITS)?;
    let reply = match frame.message.ok_or(Invalid("Missing catalogue result"))? {
        pb::server_frame::Message::Catalogue(value) => Reply::Catalogue(Catalogue {
            apps: value
                .apps
                .into_iter()
                .map(|app| Application {
                    id: app.id,
                    name: app.name,
                    description: app.description,
                    keywords: app.keywords,
                    icon: app.icon,
                    actions: app
                        .actions
                        .into_iter()
                        .map(|action| DesktopAction {
                            id: action.id,
                            name: action.name,
                        })
                        .collect(),
                })
                .collect(),
        }),
        pb::server_frame::Message::Error(error) => Reply::Failed(common::Error {
            code: error.code,
            message: error.message,
            retry: match error.retry {
                0 => common::Retry::Unknown,
                1 => common::Retry::Yes,
                2 => common::Retry::No,
                _ => return Err(Invalid("Invalid catalogue retry value").into()),
            },
        }),
        pb::server_frame::Message::WorkerControl(_) => {
            return Err(crate::worker::wire::Error::UnexpectedControl.into());
        }
    };
    reply.validate()?;
    Ok(reply)
}

pub fn read(input: &mut impl Read) -> Result<Reply, Error> {
    let body = protobuf::read_frame(input, SERVER_LIMITS.message)?.ok_or(Error::Ended)?;
    decode(body)
}

/// A completed command must contain exactly one result, including its framing.
pub fn decode_complete(mut encoded: &[u8]) -> Result<Reply, Error> {
    let reply = read(&mut encoded)?;
    if !encoded.is_empty() {
        return Err(Error::Trailing);
    }
    Ok(reply)
}

pub fn write(output: &mut impl Write, reply: &Reply) -> Result<(), Error> {
    output
        .write_all(&encode(reply)?)
        .map_err(FrameError::from)?;
    Ok(())
}

#[derive(Clone, Copy)]
enum Shape {
    Frame,
    Catalogue,
    Application,
    Action,
    Error,
}

fn scan(mut body: &[u8], shape: Shape, remaining: &mut usize) -> Result<(), Error> {
    let mut counts = [0usize; 7];
    while !body.is_empty() {
        *remaining = remaining
            .checked_sub(1)
            .ok_or(Invalid("Too many catalogue fields"))?;
        let field = fields::field(&mut body)?;
        let number = field.number;
        let (maximum, limit, child) = match (shape, number) {
            (Shape::Frame, 1 | 2) => {
                counts[0] += 1;
                if counts[0] > 1 {
                    return Err(Invalid("Duplicate catalogue result").into());
                }
                (
                    1,
                    SERVER_LIMITS.message,
                    Some(if number == 1 {
                        Shape::Catalogue
                    } else {
                        Shape::Error
                    }),
                )
            }
            (Shape::Catalogue, 1) => (
                super::MAX_APPLICATIONS,
                SERVER_LIMITS.message,
                Some(Shape::Application),
            ),
            (Shape::Application, 1) => (1, super::MAX_ID, None),
            (Shape::Application, 2) => (1, super::MAX_NAME, None),
            (Shape::Application, 3) => (1, super::MAX_DESCRIPTION, None),
            (Shape::Application, 4) => (super::MAX_KEYWORDS, super::MAX_KEYWORD, None),
            (Shape::Application, 5) => (1, super::MAX_CATALOGUE_BYTES, None),
            (Shape::Application, 6) => (
                super::MAX_ACTIONS,
                SERVER_LIMITS.message,
                Some(Shape::Action),
            ),
            (Shape::Action, 1) => (1, super::MAX_ACTION_ID, None),
            (Shape::Action, 2) => (1, super::MAX_NAME, None),
            (Shape::Error, 1) => (1, 64, None),
            (Shape::Error, 2) => (1, super::MAX_ERROR_MESSAGE, None),
            (Shape::Error, 3) => {
                counts[3] += 1;
                if counts[3] > 1 || field.u32()? > 2 {
                    return Err(Invalid("Invalid catalogue retry value").into());
                }
                continue;
            }
            _ => continue,
        };
        counts[number as usize] += 1;
        if counts[number as usize] > maximum {
            return Err(Invalid("Too many catalogue values").into());
        }
        let bytes = field.bytes()?;
        if bytes.len() > limit {
            return Err(Invalid("Catalogue field exceeds its byte limit").into());
        }
        if let Some(child) = child {
            scan(bytes, child, remaining)?;
        }
    }
    Ok(())
}
