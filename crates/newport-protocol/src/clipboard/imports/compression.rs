use super::{CHUNK, Encoding};
use std::io;
fn invalid() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, "Invalid clipboard chunk")
}

/// Independent zlib chunks keep decoding bounded and other SSH messages responsive.
pub fn compress_chunk(bytes: &[u8]) -> io::Result<(Encoding, Vec<u8>)> {
    use std::io::Write;
    if bytes.len() > CHUNK {
        return Err(invalid());
    }
    if bytes.len() >= 1024 {
        let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::fast());
        encoder.write_all(bytes)?;
        let compressed = encoder.finish()?;
        if compressed.len() < bytes.len() {
            return Ok((Encoding::Zlib, compressed));
        }
    }
    Ok((Encoding::Raw, bytes.to_vec()))
}
pub fn decompress_chunk(status: Encoding, bytes: &[u8]) -> io::Result<Vec<u8>> {
    if bytes.len() > CHUNK {
        return Err(invalid());
    }
    if status == Encoding::Raw {
        return Ok(bytes.to_vec());
    }
    if status != Encoding::Zlib {
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
        assert_eq!(status, Encoding::Zlib);
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
        assert_eq!(status, Encoding::Raw);
        assert_eq!(encoded, noise);
        for bytes in [b"".as_slice(), b"small"] {
            let (status, encoded) = compress_chunk(bytes).unwrap();
            assert_eq!(status, Encoding::Raw);
            assert_eq!(decompress_chunk(status, &encoded).unwrap(), bytes);
        }
    }
    #[test]
    fn rejects_corrupt_truncated_trailing_and_expanding_streams() {
        use std::io::Write;
        let (_, encoded) = compress_chunk(&vec![b'a'; CHUNK]).unwrap();
        assert!(decompress_chunk(Encoding::Zlib, &encoded[..encoded.len() - 1]).is_err());
        let mut trailing = encoded.clone();
        trailing.push(0);
        assert!(decompress_chunk(Encoding::Zlib, &trailing).is_err());
        let mut corrupt = encoded;
        corrupt[0] ^= 255;
        assert!(decompress_chunk(Encoding::Zlib, &corrupt).is_err());
        let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::fast());
        encoder.write_all(&vec![b'a'; CHUNK + 1]).unwrap();
        assert!(decompress_chunk(Encoding::Zlib, &encoder.finish().unwrap()).is_err());
    }
}
