#![allow(dead_code)]
use serde::{Serialize, de::DeserializeOwned};
use std::io::{self, Read, Write};

pub fn write(output: &mut impl Write, value: &impl Serialize) -> io::Result<()> {
    let body = serde_json::to_vec(value).map_err(io::Error::other)?;
    let length = u32::try_from(body.len()).map_err(io::Error::other)?;
    output.write_all(&length.to_be_bytes())?;
    output.write_all(&body)?;
    output.flush()
}

pub fn read<T: DeserializeOwned>(input: &mut impl Read) -> io::Result<T> {
    let mut prefix = [0; 4];
    input.read_exact(&mut prefix)?;
    let length = u32::from_be_bytes(prefix) as usize;
    if length > 16 * 1024 {
        return Err(io::Error::other("Fixture message exceeds limit"));
    }
    let mut body = vec![0; length];
    input.read_exact(&mut body)?;
    serde_json::from_slice(&body).map_err(io::Error::other)
}
