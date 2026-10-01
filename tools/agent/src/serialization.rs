//! Bounded MessagePack decoding shared by both ends of agent connections.
use serde::{de::DeserializeOwned, Serialize};
use std::io::{self, Cursor};

pub fn encode(value: &impl Serialize) -> io::Result<Vec<u8>> {
    rmp_serde::to_vec(value).map_err(io::Error::other)
}
pub fn decode<T: DeserializeOwned>(bytes: &[u8], limit: usize) -> io::Result<T> {
    if bytes.len() > limit {
        return Err(io::Error::other("MessagePack message exceeds limit"));
    }
    let mut decoder = rmp_serde::Deserializer::new(Cursor::new(bytes));
    decoder.set_max_depth(32);
    let value = T::deserialize(&mut decoder).map_err(io::Error::other)?;
    if decoder.position() != bytes.len() as u64 {
        return Err(io::Error::other("trailing MessagePack data"));
    }
    Ok(value)
}
