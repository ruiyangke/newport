//! Shared protobuf records inside the native transport's bounded datagrams.
use super::*;
use crate::encoding::protobuf::generated::newport::desktop::stub as pb;
use crate::encoding::protobuf::{self, FrameError};
mod convert;
mod scan;
#[cfg(test)]
mod tests;
#[cfg(test)]
pub(crate) use convert::enum_maps;
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Frame(#[from] FrameError),
    #[error(transparent)]
    Invalid(#[from] Invalid),
}
pub fn encode_host(value: &ToHost) -> Result<Vec<u8>, Error> {
    value.validate()?;
    encode(&convert::to_host_to(value), scan::Shape::Host)
}
pub fn encode_stub(value: &ToStub) -> Result<Vec<u8>, Error> {
    value.validate()?;
    encode(&convert::to_stub_to(value), scan::Shape::Stub)
}
pub fn decode_host(frame: &[u8]) -> Result<ToHost, Error> {
    let value: pb::HostFrame = decode(frame, scan::Shape::Host)?;
    let value = convert::to_host_from(value)?;
    value.validate()?;
    Ok(value)
}
pub fn decode_stub(frame: &[u8]) -> Result<ToStub, Error> {
    let value: pb::StubFrame = decode(frame, scan::Shape::Stub)?;
    let value = convert::to_stub_from(value)?;
    value.validate()?;
    Ok(value)
}
fn encode<T: prost::Message + protobuf::Layout>(
    value: &T,
    shape: scan::Shape,
) -> Result<Vec<u8>, Error> {
    let mut frame = Vec::new();
    protobuf::write_message(&mut frame, value, &LIMITS)?;
    scan::check(body(&frame)?, shape)?;
    Ok(frame)
}
fn decode<T: prost::Message + protobuf::Layout + Default>(
    frame: &[u8],
    shape: scan::Shape,
) -> Result<T, Error> {
    let body = body(frame)?;
    scan::check(body, shape)?;
    Ok(protobuf::decode(
        bytes::Bytes::copy_from_slice(body),
        &LIMITS,
    )?)
}
fn body(frame: &[u8]) -> Result<&[u8], Error> {
    if frame.len() > MAX_MESSAGE_BYTES {
        return Err(Invalid("datagram size").into());
    }
    let mut prefix = protobuf::LengthDecoder::default();
    for (index, &byte) in frame.iter().enumerate() {
        if let Some(length) = prefix.push(byte, LIMITS.message)? {
            let body = &frame[index + 1..];
            if body.len() != length {
                return Err(Invalid("truncated or trailing datagram bytes").into());
            }
            return Ok(body);
        }
    }
    Err(FrameError::Truncated.into())
}
