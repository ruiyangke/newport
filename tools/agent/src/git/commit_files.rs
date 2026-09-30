//! Commit-file pages render only requested rows. The native tree comparison
//! runs once per cached snapshot. Oversized captures fall back to recomputing
//! and validating the same snapshot, without truncating the file list.
use super::paged_rows::{flush, Capture, BLOCK_BYTES, BLOCK_ROWS};
use super::*;
const CAPTURE_BYTES: usize = 8 * 1024 * 1024;
#[derive(Default)]
pub(super) struct Cache(Vec<(String, Arc<Capture>)>);

pub(super) fn page(
    cache: &mut Cache,
    repository: RepoRef,
    query: String,
    count: usize,
    snapshot: Option<SnapshotRef>,
    offset: usize,
) -> Result<Value, Error> {
    page_with_limit(
        cache,
        repository,
        query,
        count,
        snapshot,
        offset,
        CAPTURE_BYTES,
    )
}
fn page_with_limit(
    cache: &mut Cache,
    repository: RepoRef,
    query: String,
    count: usize,
    snapshot: Option<SnapshotRef>,
    offset: usize,
    cache_limit: usize,
) -> Result<Value, Error> {
    let repo = repository.open()?;
    if snapshot.as_ref().is_some_and(|s| s.p.is_some()) {
        return Err(Error::invalid("Invalid commit files snapshot."));
    }
    if let Some(snapshot) = &snapshot {
        let token = snapshot.encode();
        if let Some(index) = cache.0.iter().position(|(key, _)| *key == token) {
            let entry = cache.0.remove(index);
            let capture = entry.1.clone();
            cache.0.push(entry);
            if offset > capture.total {
                return Err(Error::invalid("Cursor is past the end of this listing."));
            }
            let rows = capture.page(offset, count)?;
            return render(
                snapshot,
                &capture.metadata,
                offset,
                capture.total,
                rows.into(),
            );
        }
    }
    let mut capture = Capture {
        blocks: Vec::new(),
        bytes: 0,
        total: 0,
        metadata: Value::Null,
    };
    let mut pending = Vec::new();
    let mut pending_count = 0;
    let mut cached = cache_limit > 0;
    let (commit, parent) = query
        .strip_prefix("commit_files:")
        .and_then(|selection| selection.rsplit_once(':'))
        .ok_or_else(|| Error::invalid("Invalid commit selection."))?;
    let parent = parent
        .parse::<usize>()
        .map_err(|_| Error::invalid("Invalid parent selection."))?;
    let mut fingerprint = String::new();
    let mut metadata = commit_comparison(&repo, commit, parent, 0, |diff| {
        let total = diff.deltas().len();
        if offset > total {
            return Err(Error::invalid("Cursor is past the end of this listing."));
        }
        // Include the actual comparison so changed rename/configuration or
        // shallow boundaries cannot silently reorder continuation pages.
        let mut hash = Sha256::new();
        hash.update(b"commit-files-page-v1");
        let mut entries = Vec::new();
        let mut bytes = 0;
        let mut end = offset;
        for (index, delta) in diff.deltas().enumerate() {
            hash.update((delta.status() as u32).to_le_bytes());
            for file in [delta.old_file(), delta.new_file()] {
                let path = file
                    .path()
                    .map(|p| p.as_os_str().as_bytes())
                    .unwrap_or_default();
                hash.update((path.len() as u64).to_le_bytes());
                hash.update(path);
                hash.update(file.id().as_bytes());
                hash.update(i32::from(file.mode()).to_le_bytes());
            }
            let wanted = index == end && entries.len() < count;
            if wanted || cached {
                let row = delta_metadata(&delta);
                let encoded = serde_json::to_vec(&row).map_err(|_| limit())?;
                if wanted {
                    if encoded.len() > MAX_FRAME / 2 {
                        return Err(limit());
                    }
                    if bytes + encoded.len() <= MAX_FRAME / 2 {
                        bytes += encoded.len();
                        entries.push(row);
                        end += 1;
                    }
                }
                if cached {
                    if encoded.len() + 4 > BLOCK_BYTES {
                        cached = false;
                    } else {
                        if pending_count == BLOCK_ROWS
                            || pending.len() + encoded.len() + 4 > BLOCK_BYTES
                        {
                            flush(&mut capture, &mut pending, &mut pending_count)?;
                        }
                        pending.extend_from_slice(&(encoded.len() as u32).to_be_bytes());
                        pending.extend_from_slice(&encoded);
                        pending_count += 1;
                        cached =
                            capture.bytes + pending.capacity() + 2 * BLOCK_BYTES <= cache_limit;
                    }
                    if !cached {
                        capture.blocks = Vec::new();
                        pending = Vec::new();
                        pending_count = 0;
                    }
                }
            }
            capture.total += 1;
        }
        fingerprint = hex(&hash.finalize());
        Ok(json!({"entries":entries,"totalFiles":total,"truncated":false}))
    })?;
    let entries = metadata
        .as_object_mut()
        .expect("comparison object")
        .remove("entries")
        .expect("page rows");
    // Parent metadata is part of the snapshot too (for example after a
    // shallow boundary changes), independently of identical file deltas.
    fingerprint = super::super::journal::hash(
        &serde_json::to_vec(&json!([fingerprint, metadata])).map_err(|_| limit())?,
    );
    if snapshot.as_ref().is_some_and(|s| s.f != fingerprint) {
        return Err(Error::new(
            "SNAPSHOT_EXPIRED",
            "The commit comparison changed. Restart from the first page.",
        ));
    }
    let snapshot = snapshot.unwrap_or(SnapshotRef {
        r: repository,
        q: query,
        f: fingerprint,
        p: None,
    });
    let result = render(&snapshot, &metadata, offset, capture.total, entries)?;
    if cached {
        flush(&mut capture, &mut pending, &mut pending_count)?;
        capture.bytes +=
            2 * BLOCK_BYTES + serde_json::to_vec(&metadata).map_err(|_| limit())?.len();
        if capture.bytes <= cache_limit {
            capture.metadata = metadata;
            let token = snapshot.encode();
            cache.0.retain(|(key, _)| *key != token);
            cache.0.push((token, Arc::new(capture)));
            while cache.0.len() > CACHE_ENTRIES
                || cache.0.iter().map(|(_, c)| c.bytes).sum::<usize>() > CACHE_BYTES
            {
                cache.0.remove(0);
            }
        }
    }
    Ok(result)
}
fn render(
    snapshot: &SnapshotRef,
    metadata: &Value,
    offset: usize,
    total: usize,
    entries: Value,
) -> Result<Value, Error> {
    let end = offset + entries.as_array().ok_or_else(limit)?.len();
    let next = (end < total).then(|| {
        CursorRef {
            k: None,
            s: snapshot.clone(),
            o: end,
        }
        .encode()
    });
    Ok(
        json!({"snapshot":snapshot.encode(),"entries":entries,"nextCursor":next,"metadata":metadata}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture(count: usize) -> (tempfile::TempDir, Repository, RepoRef, String, git2::Oid) {
        let temp = tempfile::tempdir().unwrap();
        let repo = Repository::init_bare(temp.path()).unwrap();
        let mut builder = repo.treebuilder(None).unwrap();
        let blob = repo.blob(b"content\n").unwrap();
        for index in 0..count {
            builder
                .insert(format!("file-{index:06}"), blob, 0o100644)
                .unwrap();
        }
        let tree_id = builder.write().unwrap();
        drop(builder);
        let tree = repo.find_tree(tree_id).unwrap();
        let sig = git2::Signature::now("Test", "test@example.test").unwrap();
        let commit = repo
            .commit(Some("HEAD"), &sig, &sig, "files", &tree, &[])
            .unwrap();
        let meta = fs::metadata(repo.path()).unwrap();
        let reference = RepoRef::new(repo.path(), meta.dev(), meta.ino());
        let query = format!("commit_files:{commit}:0");
        drop(tree);
        (temp, repo, reference, query, commit)
    }
    #[test]
    fn fifty_thousand_files_walk_without_reloading_commit_and_resume_after_eviction() {
        let (_temp, repo, reference, query, commit) = fixture(50_005);
        let mut cache = Cache::default();
        let mut current = page(&mut cache, reference.clone(), query.clone(), 200, None, 0).unwrap();
        assert_eq!(cache.0.len(), 1);
        assert!(cache.0[0].1.bytes < CAPTURE_BYTES);
        let object = repo
            .path()
            .join("objects")
            .join(&commit.to_string()[..2])
            .join(&commit.to_string()[2..]);
        let bytes = fs::read(&object).unwrap();
        fs::remove_file(&object).unwrap();
        let mut seen = HashSet::new();
        let mut last_cursor = None;
        loop {
            for row in current["entries"].as_array().unwrap() {
                assert!(seen.insert(row["newPath"]["bytesB64"].as_str().unwrap().to_owned()));
            }
            let Some(cursor) = current["nextCursor"].as_str() else {
                break;
            };
            let cursor = CursorRef::decode(cursor).unwrap();
            last_cursor = Some(cursor.clone());
            current = page(
                &mut cache,
                reference.clone(),
                query.clone(),
                200,
                Some(cursor.s),
                cursor.o,
            )
            .unwrap();
        }
        assert_eq!(seen.len(), 50_005);
        fs::write(object, bytes).unwrap();
        cache.0.clear();
        let cursor = last_cursor.unwrap();
        assert_eq!(
            page(&mut cache, reference, query, 200, Some(cursor.s), cursor.o).unwrap(),
            current
        );
    }
    #[test]
    fn uncached_fallback_preserves_snapshot_order_and_page_content() {
        let (_temp, _repo, reference, query, _commit) = fixture(205);
        let mut cache = Cache::default();
        let first = page(&mut cache, reference.clone(), query.clone(), 100, None, 0).unwrap();
        let mut disabled = Cache::default();
        assert_eq!(
            page_with_limit(
                &mut disabled,
                reference.clone(),
                query.clone(),
                100,
                None,
                0,
                0
            )
            .unwrap(),
            first
        );
        assert!(disabled.0.is_empty());
        let snapshot = SnapshotRef::decode(first["snapshot"].as_str().unwrap()).unwrap();
        for offset in [100, 200, 205] {
            let warm = page(
                &mut cache,
                reference.clone(),
                query.clone(),
                100,
                Some(snapshot.clone()),
                offset,
            )
            .unwrap();
            let cold = page_with_limit(
                &mut disabled,
                reference.clone(),
                query.clone(),
                100,
                Some(snapshot.clone()),
                offset,
                0,
            )
            .unwrap();
            assert_eq!(warm, cold);
        }
        assert!(page(&mut cache, reference, query, 100, Some(snapshot), 206).is_err());
    }
}
