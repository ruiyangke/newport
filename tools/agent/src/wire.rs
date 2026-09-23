// Shared by the Linux agent and the Mac SSH client. Version 1, bounded binary frames.
use std::io::{self, Read, Write};
pub const MAX_FRAME: usize = 33 * 1024 * 1024;
pub const VERSION: &str = "porthop-agent/1";
pub fn encode(kind: u8, data: &[u8]) -> io::Result<Vec<u8>> {
    if data.len() > MAX_FRAME {
        return Err(io::Error::other("agent frame exceeds limit"));
    }
    let mut frame = Vec::with_capacity(data.len() + 5);
    frame.push(kind);
    frame.extend_from_slice(&(data.len() as u32).to_be_bytes());
    frame.extend_from_slice(data);
    Ok(frame)
}
pub fn read(input: &mut impl Read) -> io::Result<(u8, Vec<u8>)> {
    let mut header = [0; 5];
    input.read_exact(&mut header)?;
    let len = u32::from_be_bytes(header[1..].try_into().unwrap()) as usize;
    if len > MAX_FRAME {
        return Err(io::Error::other("agent frame exceeds limit"));
    }
    let mut data = vec![0; len];
    input.read_exact(&mut data)?;
    Ok((header[0], data))
}
pub fn write(output: &mut impl Write, kind: u8, data: &[u8]) -> io::Result<()> {
    output.write_all(&encode(kind, data)?)?;
    output.flush()
}
pub fn web_url(value: &str) -> Option<url::Url> {
    if value.len() > 8192 || value.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return None;
    }
    let url = url::Url::parse(value).ok()?;
    (matches!(url.scheme(), "http" | "https")
        && url.host_str().is_some()
        && url.username().is_empty()
        && url.password().is_none())
    .then_some(url)
}
