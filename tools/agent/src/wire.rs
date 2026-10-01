// Shared by the Linux agent and desktop. Positional MessagePack with native bytes.
use crate::serialization;
use std::io::{self, Read, Write};
pub const MAX_FRAME: usize = 33 * 1024 * 1024;
pub const VERSION: &str = "newport-agent/6";
const OVERHEAD: usize = 16;
pub fn encode(kind: u8, data: &[u8]) -> io::Result<Vec<u8>> {
    if data.len() > MAX_FRAME {
        return Err(io::Error::other("agent frame exceeds limit"));
    }
    let payload = serialization::encode(&(kind, serde_bytes::Bytes::new(data)))?;
    let mut frame = Vec::with_capacity(payload.len() + 5);
    frame.push(b'M');
    frame.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    frame.extend_from_slice(&payload);
    Ok(frame)
}
pub fn payload_length(header: &[u8; 5], limit: usize) -> io::Result<usize> {
    let len = u32::from_be_bytes(header[1..].try_into().unwrap()) as usize;
    if header[0] != b'M' {
        return Err(io::Error::other(
            "incompatible agent protocol; update the agent",
        ));
    }
    if len == 0 || len > limit.min(MAX_FRAME) + OVERHEAD {
        return Err(io::Error::other("agent frame exceeds limit"));
    }
    Ok(len)
}
pub fn decode(bytes: &[u8], limit: usize) -> io::Result<(u8, Vec<u8>)> {
    let (kind, data): (u8, serde_bytes::ByteBuf) =
        serialization::decode(bytes, limit.min(MAX_FRAME) + OVERHEAD)?;
    if data.len() > limit.min(MAX_FRAME) {
        return Err(io::Error::other("agent frame exceeds limit"));
    }
    Ok((kind, data.into_vec()))
}
pub fn read_limited(input: &mut impl Read, limit: usize) -> io::Result<(u8, Vec<u8>)> {
    let mut header = [0; 5];
    input.read_exact(&mut header)?;
    let len = payload_length(&header, limit)?;
    let mut data = vec![0; len];
    input.read_exact(&mut data)?;
    decode(&data, limit)
}
pub fn read(input: &mut impl Read) -> io::Result<(u8, Vec<u8>)> {
    read_limited(input, MAX_FRAME)
}
pub fn write(output: &mut impl Write, kind: u8, data: &[u8]) -> io::Result<()> {
    output.write_all(&encode(kind, data)?)?;
    output.flush()
}
/// Browser request: [request ID, URL]. Reply: [request ID, success, message].
pub fn browser_request(id: u64, url: &str) -> io::Result<Vec<u8>> {
    serialization::encode(&(id, url))
}
pub fn parse_browser_request(bytes: &[u8]) -> io::Result<(u64, String)> {
    let value: (u64, String) = serialization::decode(bytes, 8224)?;
    if web_url(&value.1).is_none() {
        return Err(io::Error::other("invalid browser URL"));
    }
    Ok(value)
}
pub fn browser_reply(id: u64, success: bool, message: Option<&str>) -> io::Result<Vec<u8>> {
    serialization::encode(&(id, success, message))
}
pub fn parse_browser_reply(bytes: &[u8]) -> io::Result<(u64, bool, Option<String>)> {
    serialization::decode(bytes, 8224)
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

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn frames_are_positional_messagepack_with_native_bytes() {
        let frame = encode(b'D', &[0, 255]).unwrap();
        assert_eq!(frame, [b'M', 0, 0, 0, 6, 0x92, b'D', 0xc4, 2, 0, 255]);
        assert_eq!(read(&mut frame.as_slice()).unwrap(), (b'D', vec![0, 255]));
        for end in 0..frame.len() {
            assert!(read(&mut &frame[..end]).is_err());
        }
        let mut joined = frame.clone();
        joined.extend(encode(b'H', &[]).unwrap());
        let mut input = joined.as_slice();
        assert_eq!(read(&mut input).unwrap().1, [0, 255]);
        assert_eq!(read(&mut input).unwrap(), (b'H', vec![]));
        assert!(input.is_empty());
    }
    #[test]
    fn rejects_legacy_oversized_trailing_and_invalid_browser_messages() {
        for marker in *b"JCO" {
            assert!(payload_length(&[marker, 0, 0, 0, 1], MAX_FRAME).is_err());
        }
        assert!(payload_length(&[b'M', 255, 255, 255, 255], MAX_FRAME).is_err());
        assert!(read_limited(&mut encode(b'D', &[0; 17]).unwrap().as_slice(), 16).is_err());
        let mut payload = encode(b'H', &[]).unwrap()[5..].to_vec();
        payload.push(0);
        assert!(decode(&payload, MAX_FRAME).is_err());
        let request = browser_request(42, "https://example.com/a?b=c").unwrap();
        assert_eq!(
            parse_browser_request(&request).unwrap(),
            (42, "https://example.com/a?b=c".into())
        );
        assert!(parse_browser_request(&browser_request(1, "file:///tmp/a").unwrap()).is_err());
        let reply = browser_reply(42, true, Some("warning")).unwrap();
        assert_eq!(
            parse_browser_reply(&reply).unwrap(),
            (42, true, Some("warning".into()))
        );
    }
}
