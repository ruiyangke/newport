//! Remote advertisements are captured once into bounded compressed blocks.
//! libgit2 still owns a complete upstream advertisement; we never additionally
//! retain a complete JSON tree. An oversized cache falls back to recapturing
//! and verifying the same snapshot, without imposing a result-count cutoff.
use super::paged_rows::{add, flush, Capture, BLOCK_BYTES, BLOCK_ROWS};
use super::*;
const INDEX_BYTES: usize = 8 * 1024 * 1024;
#[derive(Default)]
pub(super) struct Cache(Vec<(String, Arc<Capture>)>);
pub(super) fn query(
    remote: &str,
    token: &str,
    for_push: bool,
    filter: &str,
) -> Result<String, Error> {
    if filter.len() > 1024 || filter.contains('\0') {
        return Err(Error::invalid("Invalid remote reference filter."));
    }
    let filter = filter.trim().to_lowercase();
    // Preserve existing unfiltered cursor identities.
    let encoded = if filter.is_empty() {
        serde_json::to_string(&(remote, token, for_push))
    } else {
        serde_json::to_string(&(remote, token, for_push, filter))
    }
    .map_err(|_| limit())?;
    Ok(format!("remote_refs:{encoded}"))
}
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
        INDEX_BYTES,
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
        return Err(Error::invalid("Invalid remote listing snapshot."));
    }
    if let Some(snapshot) = &snapshot {
        let token = snapshot.encode();
        if let Some(index) = cache.0.iter().position(|(key, _)| *key == token) {
            let entry = cache.0.remove(index);
            let captured = entry.1.clone();
            cache.0.push(entry);
            return render(
                snapshot.clone(),
                &captured,
                offset,
                captured.page(offset, count)?,
            );
        }
    }
    let payload = query.strip_prefix("remote_refs:").ok_or_else(limit)?;
    let (remote, token, for_push, filter): (String, String, bool, String) =
        serde_json::from_str(payload)
            .or_else(|_| {
                serde_json::from_str::<(String, String, bool)>(payload)
                    .map(|(r, t, p)| (r, t, p, String::new()))
            })
            .map_err(|_| Error::invalid("Invalid remote listing query."))?;
    let mut capture = Capture {
        blocks: Vec::new(),
        bytes: 0,
        total: 0,
        metadata: Value::Null,
    };
    let (mut pending, mut pending_count) = (Vec::new(), 0);
    let (mut rows, mut row_bytes) = (Vec::new(), 0);
    let mut cached = true;
    let mut page_full = false;
    let mut hash = Sha256::new();
    // Preserve the original canonical [rows, metadata] fingerprint.
    hash.update(b"[[");
    let metadata =
        super::super::remotes::visit_references(&repo, &remote, &token, for_push, |row| {
            if !filter.is_empty()
                && !row["reference"]["display"]
                    .as_str()
                    .is_some_and(|name| name.to_lowercase().contains(&filter))
            {
                return Ok(());
            }
            let encoded = serde_json::to_vec(&row).map_err(|_| limit())?;
            if capture.total != 0 {
                hash.update(b",");
            }
            hash.update(&encoded);
            if capture.total >= offset && rows.len() < count && !page_full {
                page_full = !add(&mut rows, &mut row_bytes, &encoded)?;
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
                    cached = capture.bytes + pending.capacity() + 2 * BLOCK_BYTES <= cache_limit;
                }
                if !cached {
                    capture.blocks = Vec::new();
                    pending = Vec::new();
                    pending_count = 0;
                }
            }
            capture.total += 1;
            Ok(())
        })?;
    hash.update(b"],");
    hash.update(serde_json::to_vec(&metadata).map_err(|_| limit())?);
    hash.update(b"]");
    let fingerprint = hex(&hash.finalize());
    if snapshot.as_ref().is_some_and(|s| s.f != fingerprint) {
        return Err(Error::new(
            "SNAPSHOT_EXPIRED",
            "Remote references changed. Refresh the listing.",
        ));
    }
    let snapshot = SnapshotRef {
        r: repository,
        q: query,
        f: fingerprint,
        p: None,
    };
    capture.metadata = metadata;
    let result = render(snapshot.clone(), &capture, offset, rows)?;
    if cached {
        flush(&mut capture, &mut pending, &mut pending_count)?;
        capture.bytes += 2 * BLOCK_BYTES
            + serde_json::to_vec(&capture.metadata)
                .map_err(|_| limit())?
                .len();
        if capture.bytes <= cache_limit {
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
    snapshot: SnapshotRef,
    capture: &Capture,
    offset: usize,
    rows: Vec<Value>,
) -> Result<Value, Error> {
    if offset > capture.total {
        return Err(Error::invalid("Cursor is past the end of this listing."));
    }
    let end = offset + rows.len();
    let next = (end < capture.total).then(|| {
        CursorRef {
            s: snapshot.clone(),
            o: end,
            k: None,
        }
        .encode()
    });
    Ok(
        json!({"snapshot":snapshot.encode(),"entries":rows,"nextCursor":next,"metadata":capture.metadata}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture(count: usize) -> (tempfile::TempDir, Repository, RepoRef, String) {
        let temp = tempfile::tempdir().unwrap();
        let source = Repository::init_bare(temp.path().join("source.git")).unwrap();
        let tree = source.treebuilder(None).unwrap().write().unwrap();
        let tree = source.find_tree(tree).unwrap();
        let sig = git2::Signature::now("Test", "test@example.test").unwrap();
        let oid = source.commit(None, &sig, &sig, "base", &tree, &[]).unwrap();
        let refs = (0..count)
            .map(|i| format!("{oid} refs/heads/topic-{i:06}\n"))
            .collect::<String>();
        fs::write(
            source.path().join("packed-refs"),
            format!("# pack-refs with: sorted\n{refs}"),
        )
        .unwrap();
        let local = Repository::init(temp.path().join("local")).unwrap();
        local
            .remote("origin", source.path().to_str().unwrap())
            .unwrap();
        let admin = local.path().canonicalize().unwrap();
        let meta = fs::metadata(&admin).unwrap();
        let repo = RepoRef::new(&admin, meta.dev(), meta.ino());
        let token =
            super::super::super::remotes::token(&local.find_remote("origin").unwrap()).unwrap();
        let query = format!(
            "remote_refs:{}",
            serde_json::to_string(&("origin", token, false)).unwrap()
        );
        drop(tree);
        (temp, source, repo, query)
    }
    #[test]
    fn filtered_pages_match_late_refs_and_resume_without_a_cache() {
        let (_temp, source, repo, original) = fixture(205);
        let (remote, token, push): (String, String, bool) =
            serde_json::from_str(original.strip_prefix("remote_refs:").unwrap()).unwrap();
        let filtered = query(&remote, &token, push, " TOPIC-00020 ").unwrap();
        assert_eq!(
            filtered,
            query(&remote, &token, push, "topic-00020").unwrap()
        );
        assert_eq!(original, query(&remote, &token, push, " ").unwrap());
        assert!(query(&remote, &token, push, &"x".repeat(1025)).is_err());
        assert!(query(&remote, &token, push, "bad\0filter").is_err());
        let first = page_with_limit(
            &mut Cache::default(),
            repo.clone(),
            filtered.clone(),
            2,
            None,
            0,
            0,
        )
        .unwrap();
        assert_eq!(
            first["entries"][0]["reference"]["display"],
            "refs/heads/topic-000200"
        );
        let cursor = CursorRef::decode(first["nextCursor"].as_str().unwrap()).unwrap();
        let second = page_with_limit(
            &mut Cache::default(),
            repo.clone(),
            filtered.clone(),
            2,
            Some(cursor.s.clone()),
            cursor.o,
            0,
        )
        .unwrap();
        assert_eq!(
            second["entries"][0]["reference"]["display"],
            "refs/heads/topic-000202"
        );
        let last = CursorRef::decode(second["nextCursor"].as_str().unwrap()).unwrap();
        let end = page(
            &mut Cache::default(),
            repo.clone(),
            filtered.clone(),
            2,
            Some(last.s),
            last.o,
        )
        .unwrap();
        assert_eq!(end["entries"].as_array().unwrap().len(), 1);
        assert!(end["nextCursor"].is_null());
        source
            .find_reference("refs/heads/topic-000204")
            .unwrap()
            .delete()
            .unwrap();
        assert_eq!(
            page(
                &mut Cache::default(),
                repo.clone(),
                filtered,
                2,
                Some(cursor.s),
                cursor.o
            )
            .unwrap_err()
            .code,
            "SNAPSHOT_EXPIRED"
        );
        let empty = page(
            &mut Cache::default(),
            repo,
            query(&remote, &token, push, "missing").unwrap(),
            2,
            None,
            0,
        )
        .unwrap();
        assert!(empty["entries"].as_array().unwrap().is_empty());
        assert!(empty["nextCursor"].is_null());
    }

    #[test]
    fn large_advertisement_walk_is_complete_cached_and_reconnectable() {
        let (_temp, source, repo, query) = fixture(50_005);
        let mut cache = Cache::default();
        let first = page(&mut cache, repo.clone(), query.clone(), 200, None, 0).unwrap();
        let snapshot = SnapshotRef::decode(first["snapshot"].as_str().unwrap()).unwrap();
        assert_eq!(cache.0.len(), 1);
        assert!(cache.0[0].1.bytes <= INDEX_BYTES);
        let mut seen = HashSet::new();
        let mut current = first;
        let mut cursor_to_resume = None;
        loop {
            for row in current["entries"].as_array().unwrap() {
                assert!(seen.insert(row["reference"]["bytesB64"].as_str().unwrap().to_owned()));
            }
            let Some(cursor) = current["nextCursor"].as_str() else {
                break;
            };
            let cursor = CursorRef::decode(cursor).unwrap();
            cursor_to_resume = Some(cursor.clone());
            current = page(
                &mut cache,
                repo.clone(),
                query.clone(),
                200,
                Some(cursor.s),
                cursor.o,
            )
            .unwrap();
        }
        assert_eq!(seen.len(), 50_005);
        let cursor = cursor_to_resume.unwrap();
        cache.0.clear();
        let continued = page(
            &mut cache,
            repo.clone(),
            query.clone(),
            200,
            Some(cursor.s.clone()),
            cursor.o,
        )
        .unwrap();
        assert_eq!(continued, current);
        fs::write(
            source.path().join("packed-refs"),
            "# pack-refs with: sorted\n",
        )
        .unwrap();
        // Warm continuation is still the exact captured advertisement.
        assert_eq!(
            page(
                &mut cache,
                repo.clone(),
                query.clone(),
                200,
                Some(cursor.s),
                cursor.o
            )
            .unwrap(),
            current
        );
        cache.0.clear();
        assert_eq!(
            page(&mut cache, repo, query, 200, Some(snapshot), 200)
                .unwrap_err()
                .code,
            "SNAPSHOT_EXPIRED"
        );
    }
    #[test]
    fn uncached_pages_revalidate_and_preserve_canonical_fingerprint() {
        let (_temp, source, repo, query) = fixture(205);
        let mut cache = Cache::default();
        let first =
            page_with_limit(&mut cache, repo.clone(), query.clone(), 100, None, 0, 0).unwrap();
        assert!(cache.0.is_empty());
        let snapshot = SnapshotRef::decode(first["snapshot"].as_str().unwrap()).unwrap();
        let second = page_with_limit(
            &mut cache,
            repo.clone(),
            query.clone(),
            100,
            Some(snapshot.clone()),
            100,
            0,
        )
        .unwrap();
        let third = page_with_limit(
            &mut cache,
            repo.clone(),
            query.clone(),
            100,
            Some(snapshot.clone()),
            200,
            0,
        )
        .unwrap();
        let rows = [
            first["entries"].as_array().unwrap(),
            second["entries"].as_array().unwrap(),
            third["entries"].as_array().unwrap(),
        ]
        .into_iter()
        .flatten()
        .cloned()
        .collect::<Vec<_>>();
        assert_eq!(
            snapshot.f,
            super::super::super::journal::hash(
                &serde_json::to_vec(&json!([rows, first["metadata"]])).unwrap()
            )
        );
        fs::write(source.path().join("packed-refs"), "").unwrap();
        assert_eq!(
            page_with_limit(&mut cache, repo, query, 100, Some(snapshot), 100, 0)
                .unwrap_err()
                .code,
            "SNAPSHOT_EXPIRED"
        );
    }
    #[test]
    fn cached_pages_reject_removed_and_replaced_repositories() {
        let (temp, _source, repository, query) = fixture(205);
        let mut cache = Cache::default();
        let first = page(&mut cache, repository.clone(), query.clone(), 100, None, 0).unwrap();
        let cursor = CursorRef::decode(first["nextCursor"].as_str().unwrap()).unwrap();
        assert!(!cache.0.is_empty());
        let local = temp.path().join("local");
        fs::rename(local.join(".git"), temp.path().join("old-admin")).unwrap();
        let gone = page(
            &mut cache,
            repository.clone(),
            query.clone(),
            100,
            Some(cursor.s.clone()),
            cursor.o,
        );
        assert_eq!(gone.unwrap_err().code, "REPO_NOT_FOUND");
        Repository::init(&local).unwrap();
        let replaced = page(&mut cache, repository, query, 100, Some(cursor.s), cursor.o);
        assert_eq!(replaced.unwrap_err().code, "REPO_REPLACED");
    }

    #[test]
    fn corrupted_block_is_rejected() {
        let mut capture = Capture {
            blocks: Vec::new(),
            bytes: 0,
            total: 1,
            metadata: Value::Null,
        };
        let mut pending = vec![0, 0, 0, 2, b'{', b'}'];
        let mut count = 1;
        flush(&mut capture, &mut pending, &mut count).unwrap();
        assert_eq!(capture.page(0, 1).unwrap(), vec![json!({})]);
        capture.blocks[0].decoded += 1;
        assert!(capture.page(0, 1).is_err());
        capture.blocks[0].decoded = BLOCK_BYTES + 1;
        assert!(capture.page(0, 1).is_err());
    }
}
