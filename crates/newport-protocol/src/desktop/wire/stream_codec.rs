use super::*;
use crate::desktop::stream as contract;

pub fn encode_terminal_client(value: &contract::TerminalClient) -> Result<Vec<u8>, Error> {
    value.validate()?;
    encode(value, &TERMINAL_CLIENT_LIMITS, scan::Shape::TerminalClient)
}
pub fn decode_terminal_client(body: Bytes) -> Result<contract::TerminalClient, Error> {
    let value: contract::TerminalClient =
        decode(body, &TERMINAL_CLIENT_LIMITS, scan::Shape::TerminalClient)?;
    value.validate()?;
    Ok(value)
}
pub fn encode_terminal_server(value: &contract::TerminalServer) -> Result<Vec<u8>, Error> {
    value.validate()?;
    encode(value, &TERMINAL_SERVER_LIMITS, scan::Shape::TerminalServer)
}
pub fn decode_terminal_server(body: Bytes) -> Result<contract::TerminalServer, Error> {
    let value: contract::TerminalServer =
        decode(body, &TERMINAL_SERVER_LIMITS, scan::Shape::TerminalServer)?;
    value.validate()?;
    Ok(value)
}
pub fn encode_opaque_client(value: &contract::OpaqueClient) -> Result<Vec<u8>, Error> {
    value.validate()?;
    encode(value, &OPAQUE_LIMITS, scan::Shape::OpaqueClient)
}
pub fn decode_opaque_client(body: Bytes) -> Result<contract::OpaqueClient, Error> {
    let value: contract::OpaqueClient = decode(body, &OPAQUE_LIMITS, scan::Shape::OpaqueClient)?;
    value.validate()?;
    Ok(value)
}
pub fn encode_opaque_server(value: &contract::OpaqueServer) -> Result<Vec<u8>, Error> {
    value.validate()?;
    encode(value, &OPAQUE_LIMITS, scan::Shape::OpaqueServer)
}
pub fn decode_opaque_server(body: Bytes) -> Result<contract::OpaqueServer, Error> {
    let value: contract::OpaqueServer = decode(body, &OPAQUE_LIMITS, scan::Shape::OpaqueServer)?;
    value.validate()?;
    Ok(value)
}
