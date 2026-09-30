//! Bounded compressed row blocks shared by immutable paginated listings.
use super::*;
use std::io::Write;
pub(super) const BLOCK_ROWS: usize = 64;
pub(super) const BLOCK_BYTES: usize = 256 * 1024;
pub(super) struct Block {
    pub(super) first: usize,
    pub(super) count: usize,
    pub(super) decoded: usize,
    pub(super) data: Vec<u8>,
}
pub(super) struct Capture {
    pub(super) blocks: Vec<Block>,
    pub(super) bytes: usize,
    pub(super) total: usize,
    pub(super) metadata: Value,
}
pub(super) fn add(rows: &mut Vec<Value>, bytes: &mut usize, encoded: &[u8]) -> Result<bool, Error> {
    if encoded.len() > MAX_FRAME / 2 {
        return Err(limit());
    }
    if *bytes + encoded.len() > MAX_FRAME / 2 {
        return Ok(false);
    }
    *bytes += encoded.len();
    rows.push(serde_json::from_slice(encoded).map_err(|_| limit())?);
    Ok(true)
}
impl Capture {
    pub(super) fn page(&self, offset: usize, count: usize) -> Result<Vec<Value>, Error> {
        let (mut rows, mut bytes) = (Vec::new(), 0);
        let first = self
            .blocks
            .partition_point(|block| block.first + block.count <= offset);
        for block in &self.blocks[first..] {
            if block.first >= offset.saturating_add(count) {
                break;
            }
            if block.decoded > BLOCK_BYTES || block.count > BLOCK_ROWS {
                return Err(limit());
            }
            let mut decoded = Vec::with_capacity(block.decoded);
            flate2::read::ZlibDecoder::new(block.data.as_slice())
                .take(block.decoded as u64 + 1)
                .read_to_end(&mut decoded)
                .map_err(io_error)?;
            if decoded.len() != block.decoded {
                return Err(limit());
            }
            let mut rest = decoded.as_slice();
            for index in block.first..block.first + block.count {
                let len = u32::from_be_bytes(
                    rest.get(..4)
                        .ok_or_else(limit)?
                        .try_into()
                        .map_err(|_| limit())?,
                ) as usize;
                rest = &rest[4..];
                let row = rest.get(..len).ok_or_else(limit)?;
                rest = &rest[len..];
                if index >= offset && rows.len() < count && !add(&mut rows, &mut bytes, row)? {
                    return Ok(rows);
                }
            }
            if !rest.is_empty() {
                return Err(limit());
            }
        }
        Ok(rows)
    }
}
pub(super) fn flush(
    capture: &mut Capture,
    pending: &mut Vec<u8>,
    count: &mut usize,
) -> Result<(), Error> {
    if *count == 0 {
        return Ok(());
    }
    let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::fast());
    encoder.write_all(pending).map_err(io_error)?;
    let data = encoder.finish().map_err(io_error)?;
    capture.bytes += data.capacity() + 2 * std::mem::size_of::<Block>();
    capture.blocks.push(Block {
        first: capture.total - *count,
        count: *count,
        decoded: pending.len(),
        data,
    });
    pending.clear();
    *count = 0;
    Ok(())
}
