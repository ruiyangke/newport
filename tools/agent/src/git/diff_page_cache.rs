//! Bounded native-byte captures for diff continuations. File/hunk
//! metadata is retained once; line pieces are compressed in seekable blocks.
use super::*;
use std::io::Write;
const BLOCK_BYTES: usize = 64 * 1024;
const CAPTURE_BYTES: usize = 8 * 1024 * 1024;
const HEADER_BYTES: usize = 7 * 8 + 3;

#[derive(Default)]
pub(in super::super) struct Cache(Vec<(String, Capture)>);
struct Block {
    first: usize,
    count: usize,
    decoded: usize,
    data: Vec<u8>,
}
struct File {
    value: Value,
    hunks: Vec<Value>,
}
pub(super) struct Capture {
    files: Vec<File>,
    blocks: Vec<Block>,
    pending: Vec<u8>,
    pending_count: usize,
    total: usize,
    bytes: usize,
    enabled: bool,
    metadata: Value,
}
impl Default for Capture {
    fn default() -> Self {
        Self {
            files: Vec::new(),
            blocks: Vec::new(),
            pending: Vec::new(),
            pending_count: 0,
            total: 0,
            bytes: 2 * BLOCK_BYTES,
            enabled: true,
            metadata: Value::Null,
        }
    }
}
impl Capture {
    fn account(&mut self, bytes: usize) {
        self.bytes = self.bytes.saturating_add(bytes);
        if self.bytes > CAPTURE_BYTES {
            self.enabled = false;
            self.files = Vec::new();
            self.blocks = Vec::new();
            self.pending = Vec::new();
        }
    }
    pub(super) fn file(&mut self, value: &Value) -> Result<(), Error> {
        if self.enabled {
            self.account(
                serde_json::to_vec(value).map_err(|_| limit())?.len() * 8
                    + 2 * std::mem::size_of::<File>(),
            );
            if self.enabled {
                self.files.push(File {
                    value: value.clone(),
                    hunks: Vec::new(),
                });
            }
        }
        Ok(())
    }
    pub(super) fn hunk(&mut self, value: &Value) -> Result<(), Error> {
        if self.enabled {
            self.account(
                serde_json::to_vec(value).map_err(|_| limit())?.len() * 8
                    + 2 * std::mem::size_of::<Value>(),
            );
            if self.enabled {
                self.files
                    .last_mut()
                    .ok_or_else(limit)?
                    .hunks
                    .push(value.clone());
            }
        }
        Ok(())
    }
    fn flush(&mut self) -> Result<(), Error> {
        if !self.enabled || self.pending_count == 0 {
            return Ok(());
        }
        let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::fast());
        encoder.write_all(&self.pending).map_err(io_error)?;
        let data = encoder.finish().map_err(io_error)?;
        self.account(data.capacity() + 2 * std::mem::size_of::<Block>());
        if self.enabled {
            self.blocks.push(Block {
                first: self.total - self.pending_count,
                count: self.pending_count,
                decoded: self.pending.len(),
                data,
            });
            self.pending.clear();
            self.pending_count = 0;
        }
        Ok(())
    }
    pub(super) fn unit(
        &mut self,
        line: Option<(&git2::DiffLine<'_>, usize, usize, &[u8], bool)>,
        id: Option<&str>,
    ) -> Result<(), Error> {
        if !self.enabled {
            return Ok(());
        }
        if id.is_some_and(|id| id.len() != 64 || !id.bytes().all(|b| b.is_ascii_hexdigit())) {
            return Err(limit());
        }
        let id = id.unwrap_or_default().as_bytes();
        let payload = line.as_ref().map_or(&[][..], |(_, _, _, bytes, _)| *bytes);
        if self.pending.len() + HEADER_BYTES + id.len() + payload.len() > BLOCK_BYTES {
            self.flush()?;
        }
        if !self.enabled {
            return Ok(());
        }
        let file = self.files.len().checked_sub(1).ok_or_else(limit)?;
        let (hunk, index, offset, old, new, origin, complete) = match line {
            Some((line, index, offset, _, complete)) => (
                self.files[file]
                    .hunks
                    .len()
                    .checked_sub(1)
                    .ok_or_else(limit)? as u64,
                index as u64,
                offset as u64,
                line.old_lineno().map_or(u64::MAX, u64::from),
                line.new_lineno().map_or(u64::MAX, u64::from),
                line.origin() as u8,
                complete,
            ),
            None => (u64::MAX, 0, 0, u64::MAX, u64::MAX, 0, false),
        };
        for value in [
            file as u64,
            hunk,
            index,
            offset,
            old,
            new,
            payload.len() as u64,
        ] {
            self.pending.extend_from_slice(&value.to_le_bytes());
        }
        self.pending
            .extend_from_slice(&[origin, u8::from(complete), id.len() as u8]);
        self.pending.extend_from_slice(id);
        self.pending.extend_from_slice(payload);
        self.pending_count += 1;
        self.total += 1;
        Ok(())
    }
    pub(super) fn finish(mut self, metadata: &Value) -> Result<Option<Self>, Error> {
        self.flush()?;
        if self.enabled {
            self.account(serde_json::to_vec(metadata).map_err(|_| limit())?.len() * 8);
        }
        if !self.enabled {
            return Ok(None);
        }
        self.metadata = metadata.clone();
        self.pending = Vec::new();
        Ok(Some(self))
    }
    fn fill(&self, window: &mut Window) -> Result<(), Error> {
        if window.start > self.total {
            return Err(Error::invalid("Cursor is past the end of this diff."));
        }
        for block in &self.blocks {
            if block.first + block.count <= window.start {
                continue;
            }
            if window.full || window.accepted == window.count {
                break;
            }
            if block.decoded > BLOCK_BYTES {
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
            for position in block.first..block.first + block.count {
                let header = rest.get(..HEADER_BYTES).ok_or_else(limit)?;
                let mut fields = [0u64; 7];
                for (target, raw) in fields
                    .iter_mut()
                    .zip(header[..56].as_chunks::<8>().0.iter())
                {
                    *target = u64::from_le_bytes(*raw);
                }
                let [file, hunk, index, offset, old, new, len] = fields;
                let len = usize::try_from(len).map_err(|_| limit())?;
                if len > PIECE_BYTES {
                    return Err(limit());
                }
                rest = &rest[HEADER_BYTES..];
                let id_len = usize::from(header[58]);
                if !matches!(id_len, 0 | 64) {
                    return Err(limit());
                }
                let id = std::str::from_utf8(rest.get(..id_len).ok_or_else(limit)?)
                    .map_err(|_| limit())?;
                rest = &rest[id_len..];
                let content = rest.get(..len).ok_or_else(limit)?;
                rest = &rest[len..];
                if position < window.start {
                    continue;
                }
                if window.full || window.accepted == window.count {
                    break;
                }
                let file = self
                    .files
                    .get(usize::try_from(file).map_err(|_| limit())?)
                    .ok_or_else(limit)?;
                window.position = position;
                if hunk == u64::MAX {
                    window.emit(&file.value, None, None)?;
                } else {
                    let hunk = file
                        .hunks
                        .get(usize::try_from(hunk).map_err(|_| limit())?)
                        .ok_or_else(limit)?;
                    let mut value = json!({"lineIndex":index,"byteOffset":offset,"lineComplete":header[57] != 0,"origin":(header[56] as char).to_string(),"oldLine":(old != u64::MAX).then_some(old),"newLine":(new != u64::MAX).then_some(new),"contentBytesB64":STANDARD.encode(content)});
                    if hunk.get("id").is_some() {
                        value["id"] = json!((!id.is_empty()).then_some(id));
                    }
                    window.emit(&file.value, Some(hunk), Some(value))?;
                }
            }
        }
        window.position = self.total;
        Ok(())
    }
}
impl Cache {
    pub(super) fn render(
        &mut self,
        cursor: &CursorRef,
        mut window: Window,
    ) -> Result<Option<Value>, Error> {
        let token = cursor.s.encode();
        let Some(index) = self.0.iter().position(|(key, _)| *key == token) else {
            return Ok(None);
        };
        let entry = self.0.remove(index);
        self.0.push(entry);
        let capture = &self.0.last().unwrap().1;
        capture.fill(&mut window)?;
        render(window, &cursor.s, &capture.metadata).map(Some)
    }
    pub(super) fn insert(&mut self, snapshot: &SnapshotRef, capture: Capture) {
        let token = snapshot.encode();
        self.0.retain(|(key, _)| *key != token);
        self.0.push((token, capture));
        while self.0.len() > CACHE_ENTRIES
            || self.0.iter().map(|(_, c)| c.bytes).sum::<usize>() > CACHE_BYTES
        {
            self.0.remove(0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn oversized_metadata_discards_capture_instead_of_limiting_results() {
        let mut capture = Capture::default();
        capture.file(&json!({"fileIndex":0,"hunks":[]})).unwrap();
        capture.unit(None, None).unwrap();
        capture.account(CAPTURE_BYTES);
        assert!(!capture.enabled);
        assert!(
            capture.files.is_empty() && capture.blocks.is_empty() && capture.pending.is_empty()
        );
        capture.file(&json!({"fileIndex":1,"hunks":[]})).unwrap();
        capture.unit(None, None).unwrap();
        assert!(capture.finish(&json!({"totalUnits":2})).unwrap().is_none());
    }

    #[test]
    fn lru_eviction_obeys_both_entry_and_memory_bounds() {
        let temp = tempfile::tempdir().unwrap();
        let repo = Repository::init(temp.path()).unwrap();
        let meta = fs::metadata(repo.path()).unwrap();
        let reference = RepoRef::new(repo.path(), meta.dev(), meta.ino());
        let mut cache = Cache::default();
        for index in 0..CACHE_ENTRIES + 2 {
            let snapshot = SnapshotRef {
                r: reference.clone(),
                q: index.to_string(),
                f: "test".into(),
                p: None,
            };
            let capture = Capture {
                bytes: CAPTURE_BYTES,
                ..Capture::default()
            };
            cache.insert(&snapshot, capture);
        }
        assert_eq!(cache.0.len(), CACHE_BYTES / CAPTURE_BYTES);
        assert_eq!(
            SnapshotRef::decode(&cache.0.last().unwrap().0).unwrap().q,
            (CACHE_ENTRIES + 1).to_string()
        );
        assert_eq!(
            SnapshotRef::decode(&cache.0.first().unwrap().0).unwrap().q,
            (CACHE_ENTRIES + 2 - CACHE_BYTES / CAPTURE_BYTES).to_string()
        );
    }
}
