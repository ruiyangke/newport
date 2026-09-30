//! Immutable blob byte pages. Wire budgets include tokens and metadata.
//! Small objects are cached within an LRU budget. Loose objects above the cache
//! limit stream initially, then use a bounded disk cache for continuations.
//! Packed objects need a whole-object decode on a cache miss. Anonymous-file
//! windows serve continuations, including blobs larger than an entry's budget.
use super::*;
use std::io::Write;

use std::os::unix::fs::FileExt;
const DISK_ENTRY_BYTES: usize = 64 * 1024 * 1024;
const DISK_CACHE_BYTES: usize = 256 * 1024 * 1024;
const CAPTURE_BYTES: usize = 8 * 1024 * 1024;
struct DiskEntry {
    key: String,
    file: fs::File,
    size: usize,
    offset: usize,
    length: usize,
}
#[derive(Default)]
pub(super) struct Cache {
    memory: Vec<(String, Arc<[u8]>)>,
    disk: Vec<DiskEntry>,
}
impl Cache {
    fn disk_size(&mut self, key: &str) -> Option<usize> {
        let index = self.disk.iter().position(|entry| entry.key == key)?;
        let entry = self.disk.remove(index);
        let size = entry.size;
        self.disk.push(entry);
        Some(size)
    }
    fn disk_read(
        &self,
        key: &str,
        offset: usize,
        count: usize,
    ) -> Option<std::io::Result<Vec<u8>>> {
        let (file, relative) = self.disk_range(key, offset, count)?;
        let mut bytes = vec![0; count];
        Some(
            file.read_exact_at(&mut bytes, relative as u64)
                .map(|()| bytes),
        )
    }
    fn disk_range(&self, key: &str, offset: usize, count: usize) -> Option<(&fs::File, usize)> {
        let entry = self.disk.iter().find(|entry| entry.key == key)?;
        let relative = offset.checked_sub(entry.offset)?;
        if relative.checked_add(count)? > entry.length {
            return None;
        }
        Some((&entry.file, relative))
    }
    fn capture_disk(&mut self, key: String, content: &[u8]) {
        self.capture_with(key, content.len(), 0, content.len(), |file| {
            file.write_all(content)
        });
    }
    fn capture_stream(&mut self, key: String, size: usize, mut content: impl Read) {
        self.capture_range(key, size, 0, size, &mut content);
    }
    fn capture_range(
        &mut self,
        key: String,
        size: usize,
        offset: usize,
        length: usize,
        mut content: impl Read,
    ) {
        self.capture_with(key, size, offset, length, |file| {
            let copied = std::io::copy(&mut (&mut content).take(length as u64), file)?;
            if copied != length as u64 {
                return Err(std::io::ErrorKind::UnexpectedEof.into());
            }
            Ok(())
        });
    }
    fn capture_with(
        &mut self,
        key: String,
        size: usize,
        offset: usize,
        length: usize,
        write: impl FnOnce(&mut fs::File) -> std::io::Result<()>,
    ) {
        if length > DISK_ENTRY_BYTES || offset > size || length > size - offset {
            return;
        }
        // Evict before writing so peak disk use also respects the budget.
        self.disk.retain(|entry| entry.key != key);
        while self.disk.len() >= CACHE_ENTRIES
            || self.disk.iter().map(|entry| entry.length).sum::<usize>() + length > DISK_CACHE_BYTES
        {
            self.disk.remove(0);
        }
        // Optional acceleration: a full/unavailable temp directory must not
        // prevent a preview. tempfile() is private and removed on close.
        if let Ok(mut file) = tempfile::tempfile() {
            if write(&mut file).is_ok() {
                self.disk.push(DiskEntry {
                    key,
                    file,
                    size,
                    offset,
                    length,
                });
            }
        }
    }
    fn get(&mut self, key: &str) -> Option<Arc<[u8]>> {
        let index = self.memory.iter().position(|(name, _)| name == key)?;
        let entry = self.memory.remove(index);
        let content = entry.1.clone();
        self.memory.push(entry);
        Some(content)
    }
    fn insert(&mut self, key: String, content: Arc<[u8]>) {
        self.memory.retain(|(name, _)| name != &key);
        self.memory.push((key, content));
        while self.memory.len() > CACHE_ENTRIES
            || self
                .memory
                .iter()
                .map(|(_, bytes)| bytes.len())
                .sum::<usize>()
                > CACHE_BYTES
        {
            self.memory.remove(0);
        }
    }
}
fn envelope(
    snapshot: &SnapshotRef,
    object: git2::Oid,
    size: usize,
    offset: usize,
    bytes: &[u8],
) -> Value {
    let end = offset + bytes.len();
    let next = (end < size).then(|| {
        CursorRef {
            s: snapshot.clone(),
            o: end,
            k: None,
        }
        .encode()
    });
    let entries = if bytes.is_empty() {
        vec![]
    } else {
        vec![json!({"offset":offset,"bytesB64":STANDARD.encode(bytes)})]
    };
    json!({"snapshot":snapshot.encode(),"entries":entries,"nextCursor":next,"metadata":{"oid":oid(object),"size":size}})
}
pub(super) fn read(
    cache: &mut Cache,
    repository: RepoRef,
    object: &str,
    budget: Option<usize>,
    cursor: Option<String>,
) -> Result<Value, Error> {
    let budget = budget.unwrap_or(64 * 1024);
    if !(4096..=MAX_FRAME / 2).contains(&budget) {
        return Err(Error::invalid("maxBytes must be 4096–524288."));
    }
    let object = super::super::branches::oid(object)?;
    let query = format!("blob:{object}");
    let cursor = cursor.map(|c| CursorRef::decode(&c)).transpose()?;
    if cursor
        .as_ref()
        .is_some_and(|c| c.s.r != repository || c.s.q != query || c.s.p.is_some() || c.k.is_some())
    {
        return Err(Error::invalid("Cursor does not match this blob."));
    }
    let repo = repository.open()?;
    let key = format!("{}:{object}", repository.encode());
    let cached = cache.get(&key);
    let disk_size = cache.disk_size(&key);
    let odb = repo.odb().map_err(engine)?;
    let size = if let Some(bytes) = &cached {
        bytes.len()
    } else if let Some(size) = disk_size {
        size
    } else {
        let (size, kind) = odb.read_header(object).map_err(engine)?;
        if kind != git2::ObjectType::Blob {
            return Err(Error::invalid("The object is not a file blob."));
        }
        size
    };
    let snapshot = SnapshotRef {
        r: repository,
        q: query,
        f: format!("{object}:{size}"),
        p: None,
    };
    if cursor.as_ref().is_some_and(|c| c.s.f != snapshot.f) {
        return Err(Error::invalid("Blob snapshot does not match."));
    }
    let offset = cursor.as_ref().map_or(0, |c| c.o);
    if offset > size {
        return Err(Error::invalid("Cursor is past the end of this blob."));
    }
    // Reserve the longest offset/cursor plus a nonempty row before base64.
    let mut skeleton = envelope(&snapshot, object, size, size.saturating_sub(1), &[]);
    skeleton["entries"] = json!([{"offset":size,"bytesB64":""}]);
    let overhead = serde_json::to_vec(&skeleton).map_err(|_| limit())?.len();
    let capacity = budget.checked_sub(overhead).ok_or_else(limit)? / 4 * 3;
    if capacity == 0 {
        return Err(limit());
    }
    let count = capacity.min(size - offset);
    if count == 0 {
        return Ok(envelope(&snapshot, object, size, offset, &[]));
    }
    // A preview is often the only page opened. Loose objects can stream just
    // that prefix regardless of whether the whole blob fits our memory cache.
    // Packed objects do not support this reader and retain the existing cache.
    let preview = if cached.is_none() && disk_size.is_none() && offset == 0 && count < size {
        odb.reader(object).ok()
    } else {
        None
    };
    let bytes = if let Some(cached) = cached {
        cached[offset..offset + count].to_vec()
    } else if let Some(bytes) = cache.disk_read(&key, offset, count) {
        bytes.map_err(io_error)?
    } else if let Some((mut stream, actual_size, kind)) = preview {
        if actual_size != size || kind != git2::ObjectType::Blob {
            return Err(limit());
        }
        let mut bytes = vec![0; count];
        stream.read_exact(&mut bytes).map_err(io_error)?;
        bytes
    } else if size <= CAPTURE_BYTES {
        let blob = repo.find_blob(object).map_err(engine)?;
        let bytes = blob
            .content()
            .get(offset..offset + count)
            .ok_or_else(limit)?
            .to_vec();
        cache.insert(key, Arc::from(blob.content()));
        bytes
    } else if let Ok((mut stream, actual_size, kind)) = odb.reader(object) {
        if actual_size != size || kind != git2::ObjectType::Blob {
            return Err(limit());
        }
        // Leave the first preview streaming. Once a continuation is requested, spool
        // bounded objects without holding their contents in RAM, avoiding a
        // repeated decompression of every earlier page. A final page alone
        // does not justify populating the cache.
        if offset > 0 && offset + count < size {
            if size <= DISK_ENTRY_BYTES {
                cache.capture_stream(key.clone(), size, &mut stream);
            } else {
                // A bounded forward window also accelerates objects too large
                // to cache whole. Cache offsets are absolute blob offsets.
                let skipped =
                    std::io::copy(&mut (&mut stream).take(offset as u64), &mut std::io::sink())
                        .map_err(io_error)?;
                if skipped != offset as u64 {
                    return Err(limit());
                }
                cache.capture_range(
                    key.clone(),
                    size,
                    offset,
                    DISK_ENTRY_BYTES.min(size - offset),
                    &mut stream,
                );
            }
            if cache.disk_range(&key, offset, count).is_none() {
                // Temp storage is optional; retry the ordinary streaming path.
                stream = odb.reader(object).map_err(engine)?.0;
            }
        }
        if let Some(bytes) = cache.disk_read(&key, offset, count) {
            bytes.map_err(io_error)?
        } else {
            let skipped =
                std::io::copy(&mut (&mut stream).take(offset as u64), &mut std::io::sink())
                    .map_err(io_error)?;
            if skipped != offset as u64 {
                return Err(limit());
            }
            let mut bytes = vec![0; count];
            stream.read_exact(&mut bytes).map_err(io_error)?;
            bytes
        }
    } else {
        let blob = repo.find_blob(object).map_err(engine)?;
        let bytes = blob
            .content()
            .get(offset..offset + count)
            .ok_or_else(limit)?
            .to_vec();
        // Avoid a write when this request already consumes the complete blob.
        if offset + count < size {
            if size <= DISK_ENTRY_BYTES {
                cache.capture_disk(key, blob.content());
            } else {
                // Keep the initial preview's disk write small. A continuation
                // beyond this prefix earns a larger forward window.
                let window = if offset == 0 {
                    CAPTURE_BYTES
                } else {
                    DISK_ENTRY_BYTES
                };
                let length = window.min(size - offset);
                cache.capture_range(
                    key,
                    size,
                    offset,
                    length,
                    &blob.content()[offset..offset + length],
                );
            }
        }
        bytes
    };
    let result = envelope(&snapshot, object, size, offset, &bytes);
    if serde_json::to_vec(&result).map_err(|_| limit())?.len() > budget {
        return Err(limit());
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture(content: &[u8]) -> (tempfile::TempDir, RepoRef, git2::Oid) {
        let temp = tempfile::tempdir().unwrap();
        let repo = Repository::init_bare(temp.path()).unwrap();
        let object = repo.blob(content).unwrap();
        let meta = fs::metadata(repo.path()).unwrap();
        let reference = RepoRef::new(repo.path(), meta.dev(), meta.ino());
        (temp, reference, object)
    }
    #[test]
    fn pages_cover_empty_binary_and_large_streamed_blobs_with_strict_budgets() {
        for size in [0, 513, 600_000, CAPTURE_BYTES + 19] {
            let content: Vec<_> = (0..size).map(|i| (i % 251) as u8).collect();
            let (_temp, repository, object) = fixture(&content);
            let mut cache = Cache::default();
            let mut cursor = None;
            let mut bytes = Vec::new();
            loop {
                let page = read(
                    &mut cache,
                    repository.clone(),
                    &object.to_string(),
                    Some(524288),
                    cursor,
                )
                .unwrap();
                assert!(serde_json::to_vec(&page).unwrap().len() <= 524288);
                assert_eq!(page["metadata"]["size"], size);
                for entry in page["entries"].as_array().unwrap() {
                    assert_eq!(entry["offset"], bytes.len());
                    bytes.extend(
                        STANDARD
                            .decode(entry["bytesB64"].as_str().unwrap())
                            .unwrap(),
                    );
                }
                cursor = page["nextCursor"].as_str().map(str::to_owned);
                if cursor.is_none() {
                    break;
                }
            }
            assert_eq!(bytes, content);
            assert!(cache.memory.iter().map(|(_, c)| c.len()).sum::<usize>() <= CACHE_BYTES);
            if size > CAPTURE_BYTES {
                assert!(cache.memory.is_empty());
            }
        }
    }
    #[test]
    fn small_loose_previews_defer_capture_until_continuation() {
        for size in [262144, CAPTURE_BYTES] {
            let content: Vec<_> = (0..size).map(|i| (i % 251) as u8).collect();
            let (_temp, reference, object) = fixture(&content);
            let mut cache = Cache::default();
            let first = read(
                &mut cache,
                reference.clone(),
                &object.to_string(),
                Some(65536),
                None,
            )
            .unwrap();
            let first_bytes = STANDARD
                .decode(first["entries"][0]["bytesB64"].as_str().unwrap())
                .unwrap();
            assert_eq!(first_bytes, content[..first_bytes.len()]);
            assert!(cache.memory.is_empty());
            assert!(cache.disk.is_empty());
            let second = read(
                &mut cache,
                reference,
                &object.to_string(),
                Some(65536),
                first["nextCursor"].as_str().map(str::to_owned),
            )
            .unwrap();
            let second_bytes = STANDARD
                .decode(second["entries"][0]["bytesB64"].as_str().unwrap())
                .unwrap();
            assert_eq!(
                second_bytes,
                content[first_bytes.len()..first_bytes.len() + second_bytes.len()]
            );
            assert_eq!(cache.memory.len(), 1);
            assert_eq!(cache.memory[0].1.len(), size);
            assert_eq!(first["snapshot"], second["snapshot"]);
        }
    }

    #[test]
    fn loose_blob_cache_starts_only_after_the_initial_preview() {
        let content = vec![b'x'; CAPTURE_BYTES + 19];
        let (_temp, reference, object) = fixture(&content);
        let mut cache = Cache::default();
        let first = read(
            &mut cache,
            reference.clone(),
            &object.to_string(),
            Some(65536),
            None,
        )
        .unwrap();
        assert!(cache.disk.is_empty());
        let second = read(
            &mut cache,
            reference,
            &object.to_string(),
            Some(524288),
            first["nextCursor"].as_str().map(str::to_owned),
        )
        .unwrap();
        assert_eq!(cache.disk.len(), 1);
        assert_eq!(cache.disk[0].size, content.len());
        assert!(cache.memory.is_empty());
        assert!(serde_json::to_vec(&second).unwrap().len() <= 524288);
    }

    #[test]
    fn oversized_blobs_cache_windows_with_correct_boundaries_and_backward_reads() {
        let content: Vec<_> = (0..DISK_ENTRY_BYTES + 600_000)
            .map(|i| (i % 251) as u8)
            .collect();
        for packed in [false, true] {
            let (temp, reference, object) = fixture(&content);
            if packed {
                let repo = reference.open().unwrap();
                let pack = temp.path().join("objects/pack");
                fs::create_dir_all(&pack).unwrap();
                let mut builder = repo.packbuilder().unwrap();
                builder.insert_object(object, None).unwrap();
                builder.write(&pack, 0).unwrap();
                let hex = object.to_string();
                fs::remove_file(temp.path().join("objects").join(&hex[..2]).join(&hex[2..]))
                    .unwrap();
            }
            let mut cache = Cache::default();
            let first = read(
                &mut cache,
                reference.clone(),
                &object.to_string(),
                Some(65536),
                None,
            )
            .unwrap();
            let first_cursor = first["nextCursor"].as_str().unwrap().to_owned();
            let second = read(
                &mut cache,
                reference.clone(),
                &object.to_string(),
                Some(65536),
                Some(first_cursor.clone()),
            )
            .unwrap();
            assert_eq!(cache.disk.len(), 1);
            assert_eq!(cache.disk[0].size, content.len());
            assert!(cache.disk[0].length <= DISK_ENTRY_BYTES);
            let boundary = cache.disk[0].offset + cache.disk[0].length;
            let mut cursor = CursorRef::decode(&first_cursor).unwrap();
            cursor.o = boundary - 17;
            let crossing = read(
                &mut cache,
                reference.clone(),
                &object.to_string(),
                Some(65536),
                Some(cursor.encode()),
            )
            .unwrap();
            let bytes = STANDARD
                .decode(crossing["entries"][0]["bytesB64"].as_str().unwrap())
                .unwrap();
            assert_eq!(bytes, content[cursor.o..cursor.o + bytes.len()]);
            assert!(bytes.len() > 17);
            assert_eq!(cache.disk[0].offset, cursor.o);
            assert!(cache.disk[0].length <= DISK_ENTRY_BYTES);
            // A cursor is independent of the cache window; backwards requests
            // reconstruct bytes rather than returning data at a relative offset.
            assert_eq!(
                read(
                    &mut cache,
                    reference,
                    &object.to_string(),
                    Some(65536),
                    Some(first_cursor)
                )
                .unwrap(),
                second
            );
            assert!(cache.memory.is_empty());
        }
    }

    #[test]
    fn disk_windows_account_for_captured_bytes_and_reject_uncovered_ranges() {
        let mut cache = Cache::default();
        cache.capture_range("huge".into(), usize::MAX, 100, 3, &b"abc"[..]);
        assert_eq!(cache.disk_size("huge"), Some(usize::MAX));
        assert_eq!(cache.disk[0].length, 3);
        assert_eq!(cache.disk_read("huge", 100, 3).unwrap().unwrap(), b"abc");
        assert!(cache.disk_read("huge", 99, 1).is_none());
        assert!(cache.disk_read("huge", 101, 3).is_none());
        assert!(cache.disk_read("huge", 100, usize::MAX).is_none());
        cache.capture_range("short".into(), 1000, 100, 4, &b"abc"[..]);
        assert!(cache.disk_size("short").is_none());
    }

    #[test]
    fn incomplete_or_oversized_streams_are_not_cached() {
        let mut cache = Cache::default();
        cache.capture_stream("short".into(), 10, &b"abc"[..]);
        cache.capture_stream("large".into(), DISK_ENTRY_BYTES + 1, &b"abc"[..]);
        assert!(cache.disk.is_empty());
    }

    #[test]
    fn packed_blob_pages_survive_pruning_and_cache_eviction() {
        let content: Vec<_> = (0..CAPTURE_BYTES + 19).map(|i| (i % 251) as u8).collect();
        let (temp, reference, object) = fixture(&content);
        let repo = reference.open().unwrap();
        let pack_dir = temp.path().join("objects/pack");
        fs::create_dir_all(&pack_dir).unwrap();
        let mut builder = repo.packbuilder().unwrap();
        builder.insert_object(object, None).unwrap();
        builder.write(&pack_dir, 0).unwrap();
        let hex = object.to_string();
        fs::remove_file(temp.path().join("objects").join(&hex[..2]).join(&hex[2..])).unwrap();
        drop(builder);
        drop(repo);
        let mut cache = Cache::default();
        let first = read(&mut cache, reference.clone(), &hex, Some(4096), None).unwrap();
        assert_eq!(cache.disk.len(), 1);
        assert!(cache.memory.is_empty());
        let cursor = first["nextCursor"].as_str().unwrap().to_owned();
        let next = |cache: &mut Cache| {
            read(
                cache,
                reference.clone(),
                &hex,
                Some(4096),
                Some(cursor.clone()),
            )
        };
        let cold = next(&mut Cache::default()).unwrap();
        assert_eq!(next(&mut cache).unwrap(), cold);
        // A continuation reads only its byte range from the captured object,
        // even if Git has since pruned the original pack.
        fs::remove_dir_all(&pack_dir).unwrap();
        assert_eq!(next(&mut cache).unwrap(), cold);
        assert!(next(&mut Cache::default()).is_err());
        let entry = &cold["entries"][0];
        let bytes = STANDARD
            .decode(entry["bytesB64"].as_str().unwrap())
            .unwrap();
        let offset = entry["offset"].as_u64().unwrap() as usize;
        assert_eq!(bytes, content[offset..offset + bytes.len()]);
        for index in 0..CACHE_ENTRIES {
            cache.capture_disk(format!("evict-{index}"), b"other");
        }
        assert_eq!(cache.disk.len(), CACHE_ENTRIES);
        assert!(next(&mut cache).is_err());
    }
    #[test]
    fn disk_cache_evicts_before_exceeding_byte_budget() {
        let mut cache = Cache::default();
        for index in 0..DISK_CACHE_BYTES / DISK_ENTRY_BYTES {
            let file = tempfile::tempfile().unwrap();
            file.set_len(DISK_ENTRY_BYTES as u64).unwrap();
            cache.disk.push(DiskEntry {
                key: index.to_string(),
                file,
                size: DISK_ENTRY_BYTES,
                offset: 0,
                length: DISK_ENTRY_BYTES,
            });
        }
        assert_eq!(cache.disk_size("0"), Some(DISK_ENTRY_BYTES));
        cache.capture_disk("new".into(), b"bytes");
        assert!(cache.disk_size("1").is_none());
        assert!(cache.disk_size("0").is_some());
        assert!(cache.disk.iter().map(|entry| entry.length).sum::<usize>() <= DISK_CACHE_BYTES);
        assert_eq!(cache.disk_read("new", 1, 3).unwrap().unwrap(), b"yte");
    }
    #[test]
    fn warm_and_cold_pages_match_and_cursor_binds_repository_object_size_and_offset() {
        let (temp, repository, object) = fixture(&vec![0xff; 12_000]);
        let mut cache = Cache::default();
        let first = read(
            &mut cache,
            repository.clone(),
            &object.to_string(),
            Some(4096),
            None,
        )
        .unwrap();
        let cursor = first["nextCursor"].as_str().unwrap().to_owned();
        let next = |cache: &mut Cache, cursor: String| {
            read(
                cache,
                repository.clone(),
                &object.to_string(),
                Some(4096),
                Some(cursor),
            )
        };
        assert_eq!(
            next(&mut cache, cursor.clone()).unwrap(),
            next(&mut Cache::default(), cursor.clone()).unwrap()
        );
        for mutate in [0, 1, 2, 3] {
            let mut token = CursorRef::decode(&cursor).unwrap();
            match mutate {
                0 => token.o = 12_001,
                1 => token.s.f.push('0'),
                2 => token.s.q = "blob:other".into(),
                _ => token.k = Some("other".into()),
            }
            assert_eq!(
                next(&mut cache, token.encode()).unwrap_err().code,
                "INVALID_REQUEST"
            );
        }
        let (_other, reference, _) = fixture(b"other");
        assert_eq!(
            read(
                &mut cache,
                reference,
                &object.to_string(),
                None,
                Some(cursor.clone())
            )
            .unwrap_err()
            .code,
            "INVALID_REQUEST"
        );
        // Captured immutable data survives object pruning, but a reconnect
        // cannot reconstruct a now-missing object; no fabricated content.
        let hex = object.to_string();
        fs::remove_file(temp.path().join("objects").join(&hex[..2]).join(&hex[2..])).unwrap();
        assert!(next(&mut cache, cursor.clone()).is_ok());
        assert!(next(&mut Cache::default(), cursor).is_err());
    }
    #[test]
    fn refuses_non_blob_objects_and_invalid_budgets() {
        let (_temp, reference, object) = fixture(b"text");
        let repo = reference.open().unwrap();
        let tree = repo.treebuilder(None).unwrap().write().unwrap();
        assert_eq!(
            read(
                &mut Cache::default(),
                reference.clone(),
                &tree.to_string(),
                None,
                None
            )
            .unwrap_err()
            .code,
            "INVALID_REQUEST"
        );
        for budget in [0, 4095, 524289] {
            assert_eq!(
                read(
                    &mut Cache::default(),
                    reference.clone(),
                    &object.to_string(),
                    Some(budget),
                    None
                )
                .unwrap_err()
                .code,
                "INVALID_REQUEST"
            );
        }
    }
}
