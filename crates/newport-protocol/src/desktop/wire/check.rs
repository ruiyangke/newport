use super::*;

pub(super) fn request(value: &crate::desktop::Request) -> Result<(), Error> {
    match &value.operation {
        Operation::Services(value) => value
            .service_request()
            .validate()
            .map_err(crate::monitor::InvalidRequest::Services)?,
        Operation::Containers(value) => value.service_request().validate()?,
        Operation::Account(_)
        | Operation::Connection(_)
        | Operation::Configuration(_)
        | Operation::Files(_)
        | Operation::Metrics(_)
        | Operation::Preferences(_)
        | Operation::Git(_)
        | Operation::Applications(_)
        | Operation::Host(_)
        | Operation::Events(_)
        | Operation::Terminal(_)
        | Operation::Stream(_) => {}
    }
    Ok(())
}

pub fn validate_server_for(
    value: &ServerMessage,
    request: &crate::desktop::Request,
) -> Result<(), Error> {
    match value {
        ServerMessage::Progress(value) => value.validate_for(request)?,
        ServerMessage::Response(value) => {
            value.validate_for(request)?;
            if let Outcome::Completed(reply) = &value.outcome {
                match (reply.as_ref(), &request.operation) {
                    (Reply::Services(reply), Operation::Services(request)) => reply
                        .service_reply()
                        .validate_for(&request.service_request())?,
                    (Reply::Containers(reply), Operation::Containers(request)) => {
                        let call = crate::monitor::Call {
                            id: 1,
                            request: request.service_request(),
                        };
                        let response = crate::monitor::Response {
                            id: 1,
                            reply: reply.service_reply(),
                        };
                        response.validate_for(&call)?;
                    }
                    _ => {}
                }
            }
        }
        ServerMessage::Ready(_) | ServerMessage::Refused(_) => {
            return Err(Invalid("server operation phase").into());
        }
    }
    Ok(())
}

pub fn decode_server_for(
    body: Bytes,
    request: &crate::desktop::Request,
) -> Result<ServerMessage, Error> {
    let value = decode_server(body)?;
    validate_server_for(&value, request)?;
    Ok(value)
}
pub fn encode_server_for(
    value: &ServerMessage,
    request: &crate::desktop::Request,
) -> Result<Vec<u8>, Error> {
    validate_server_for(value, request)?;
    encode_server(value)
}
