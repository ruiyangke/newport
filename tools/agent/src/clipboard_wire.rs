//! Metadata-only offers and bounded, revision-tagged clipboard replies.
use std::io;
pub const CHUNK: usize = 64 * 1024;
pub const LIMIT: usize = 32 * 1024 * 1024;
#[derive(Clone, Debug)]
pub struct Offer {
    pub revision: i64,
    pub formats: Vec<String>,
}
#[derive(Clone, Debug)]
pub struct Request {
    pub id: u64,
    pub revision: i64,
    pub format: String,
}
fn invalid() -> io::Error {
    io::Error::other("invalid clipboard message")
}
pub fn valid_format(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 256
        && !s.chars().any(char::is_control)
        && !s.split('/').any(|s| s.is_empty() || s == "." || s == "..")
}
impl Offer {
    pub fn encode(&self) -> Vec<u8> {
        format!("{}\n{}", self.revision, self.formats.join("\n")).into_bytes()
    }
    pub fn decode(bytes: &[u8]) -> io::Result<Self> {
        if bytes.len() > 66000 {
            return Err(invalid());
        }
        let text = std::str::from_utf8(bytes).map_err(|_| invalid())?;
        let (revision, names) = text.split_once('\n').ok_or_else(invalid)?;
        let formats: Vec<String> = if names.is_empty() {
            vec![]
        } else {
            names.split('\n').map(str::to_owned).collect()
        };
        if formats.len() > 256 || formats.iter().any(|s| !valid_format(s)) {
            return Err(invalid());
        }
        let mut unique = formats.clone();
        unique.sort();
        unique.dedup();
        if unique.len() != formats.len() {
            return Err(invalid());
        }
        Ok(Self {
            revision: revision.parse().map_err(|_| invalid())?,
            formats,
        })
    }
}
impl Request {
    pub fn encode(&self) -> Vec<u8> {
        format!("{}\n{}\n{}", self.id, self.revision, self.format).into_bytes()
    }
    pub fn decode(bytes: &[u8]) -> io::Result<Self> {
        let text = std::str::from_utf8(bytes).map_err(|_| invalid())?;
        let mut fields = text.splitn(3, '\n');
        let id = fields
            .next()
            .ok_or_else(invalid)?
            .parse()
            .map_err(|_| invalid())?;
        let revision = fields
            .next()
            .ok_or_else(invalid)?
            .parse()
            .map_err(|_| invalid())?;
        let format = fields.next().ok_or_else(invalid)?.to_owned();
        if !valid_format(&format) {
            return Err(invalid());
        }
        Ok(Self {
            id,
            revision,
            format,
        })
    }
}
#[allow(dead_code)] // Encoder is also compiled into the desktop client.
pub fn reply(request: &Request, status: u8, done: bool, bytes: &[u8]) -> Vec<u8> {
    let mut result = Vec::with_capacity(18 + bytes.len());
    result.extend(request.id.to_be_bytes());
    result.extend(request.revision.to_be_bytes());
    result.extend([status, u8::from(done)]);
    result.extend(bytes);
    result
}
pub fn parse_reply(bytes: &[u8]) -> io::Result<(u64, i64, u8, bool, &[u8])> {
    if bytes.len() < 18 || bytes.len() > CHUNK + 18 || bytes[16] > 2 || bytes[17] > 1 {
        return Err(invalid());
    }
    Ok((
        u64::from_be_bytes(bytes[..8].try_into().unwrap()),
        i64::from_be_bytes(bytes[8..16].try_into().unwrap()),
        bytes[16],
        bytes[17] != 0,
        &bytes[18..],
    ))
}

/// Independent zlib chunks keep decoding bounded and other SSH messages responsive.
/// Status 0 is raw, 1 is a failed read, and 2 is compressed content.
#[allow(dead_code)] // Used by the desktop sender.
pub fn compress_chunk(bytes: &[u8]) -> io::Result<(u8, Vec<u8>)> {
    use std::io::Write;
    if bytes.len() > CHUNK {
        return Err(invalid());
    }
    if bytes.len() >= 1024 {
        let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::fast());
        encoder.write_all(bytes)?;
        let compressed = encoder.finish()?;
        if compressed.len() < bytes.len() {
            return Ok((2, compressed));
        }
    }
    Ok((0, bytes.to_vec()))
}
pub fn decompress_chunk(status: u8, bytes: &[u8]) -> io::Result<Vec<u8>> {
    if bytes.len() > CHUNK {
        return Err(invalid());
    }
    if status == 0 {
        return Ok(bytes.to_vec());
    }
    if status != 2 {
        return Err(invalid());
    }
    let mut decoder = flate2::Decompress::new(true);
    let mut output = vec![0; CHUNK + 1];
    let result = decoder
        .decompress(bytes, &mut output, flate2::FlushDecompress::Finish)
        .map_err(|_| invalid())?;
    if result != flate2::Status::StreamEnd
        || decoder.total_in() != bytes.len() as u64
        || decoder.total_out() > CHUNK as u64
    {
        return Err(invalid());
    }
    output.truncate(decoder.total_out() as usize);
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn compression_is_lossless_and_only_used_when_smaller() {
        let text = vec![b'a'; CHUNK];
        let (status, encoded) = compress_chunk(&text).unwrap();
        assert_eq!(status, 2);
        assert!(encoded.len() < 1024);
        assert_eq!(decompress_chunk(status, &encoded).unwrap(), text);
        let mut state = 123456789u32;
        let noise: Vec<_> = (0..CHUNK)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 17;
                state ^= state << 5;
                state as u8
            })
            .collect();
        let (status, encoded) = compress_chunk(&noise).unwrap();
        assert_eq!(status, 0);
        assert_eq!(encoded, noise);
        for bytes in [b"".as_slice(), b"small"] {
            let (status, encoded) = compress_chunk(bytes).unwrap();
            assert_eq!(status, 0);
            assert_eq!(decompress_chunk(status, &encoded).unwrap(), bytes);
        }
    }
    #[test]
    fn rejects_corrupt_truncated_trailing_and_expanding_streams() {
        use std::io::Write;
        let (_, encoded) = compress_chunk(&vec![b'a'; CHUNK]).unwrap();
        assert!(decompress_chunk(2, &encoded[..encoded.len() - 1]).is_err());
        let mut trailing = encoded.clone();
        trailing.push(0);
        assert!(decompress_chunk(2, &trailing).is_err());
        let mut corrupt = encoded;
        corrupt[0] ^= 255;
        assert!(decompress_chunk(2, &corrupt).is_err());
        let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::fast());
        encoder.write_all(&vec![b'a'; CHUNK + 1]).unwrap();
        assert!(decompress_chunk(2, &encoder.finish().unwrap()).is_err());
    }
    #[test]
    fn rejects_malformed_offers_requests_and_oversized_chunks() {
        for bytes in [
            b"1\nimage/png\nimage/png".as_slice(),
            b"1\n../text",
            b"1\nimage/\0",
            b"invalid\ntext/plain",
        ] {
            assert!(Offer::decode(bytes).is_err());
        }
        assert!(Request::decode(b"2\n3\ntext/plain\nextra").is_err());
        assert!(parse_reply(&[0; 17]).is_err());
        assert!(parse_reply(&vec![0; CHUNK + 19]).is_err());
        let request = Request {
            id: 42,
            revision: -12,
            format: "image/png".into(),
        };
        let bytes = reply(&request, 0, true, b"image");
        assert_eq!(
            parse_reply(&bytes).unwrap(),
            (42, -12, 0, true, b"image".as_slice())
        );
    }
}
