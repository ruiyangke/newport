//! Remote pickers need names, not credentials, URLs and refspec tokens for every
//! remote. Capture a sorted name index once; hydrate only the chosen remote via
//! repo.remote. Oversized indexes are rescanned and verified on continuation.
use super::*;
const INDEX_BYTES: usize = 8 * 1024 * 1024;
#[derive(Default)]
pub(super) struct Cache(Vec<(String, Arc<Vec<String>>, usize)>);
pub(super) fn query(filter: &str) -> Result<String, Error> {
    if filter.len() > 1024 || filter.contains('\0') {
        return Err(Error::invalid("Invalid remote filter."));
    }
    Ok(format!(
        "remote_names:{}",
        serde_json::to_string(&filter.to_lowercase()).map_err(|_| limit())?
    ))
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
        return Err(Error::invalid("Invalid remote names snapshot."));
    }
    if let Some(snapshot) = &snapshot {
        let token = snapshot.encode();
        if let Some(index) = cache.0.iter().position(|(key, _, _)| *key == token) {
            let entry = cache.0.remove(index);
            let names = entry.1.clone();
            cache.0.push(entry);
            return render(snapshot.clone(), &names, offset, count);
        }
    }
    let filter: String =
        serde_json::from_str(query.strip_prefix("remote_names:").ok_or_else(limit)?)
            .map_err(|_| limit())?;
    let native = repo.remotes().map_err(engine)?;
    let mut names = Vec::new();
    for name in native.iter() {
        let name = name
            .map_err(engine)?
            .ok_or_else(|| Error::invalid("A remote name is not UTF-8."))?;
        if filter.is_empty() || name.to_lowercase().contains(&filter) {
            names.push(name.to_owned());
        }
    }
    names.sort_unstable();
    names.dedup();
    let mut hash = Sha256::new();
    for name in &names {
        hash.update((name.len() as u64).to_le_bytes());
        hash.update(name.as_bytes());
    }
    let fingerprint = hex(&hash.finalize());
    if snapshot.as_ref().is_some_and(|s| s.f != fingerprint) {
        return Err(Error::new(
            "SNAPSHOT_EXPIRED",
            "Remote names changed. Refresh the listing.",
        ));
    }
    let snapshot = SnapshotRef {
        r: repository,
        q: query,
        f: fingerprint,
        p: None,
    };
    let result = render(snapshot.clone(), &names, offset, count)?;
    let bytes = names.capacity() * std::mem::size_of::<String>()
        + names.iter().map(String::capacity).sum::<usize>()
        + snapshot.encode().len();
    if bytes <= cache_limit {
        let token = snapshot.encode();
        cache.0.retain(|(key, _, _)| *key != token);
        cache.0.push((token, Arc::new(names), bytes));
        while cache.0.len() > CACHE_ENTRIES
            || cache.0.iter().map(|(_, _, size)| size).sum::<usize>() > CACHE_BYTES
        {
            cache.0.remove(0);
        }
    }
    Ok(result)
}
fn render(
    snapshot: SnapshotRef,
    names: &[String],
    offset: usize,
    count: usize,
) -> Result<Value, Error> {
    if offset > names.len() {
        return Err(Error::invalid("Cursor is past the end of this listing."));
    }
    let (mut rows, mut bytes) = (Vec::new(), 0);
    for name in names.iter().skip(offset).take(count) {
        let encoded = serde_json::to_vec(&json!({"name":name})).map_err(|_| limit())?;
        if !paged_rows::add(&mut rows, &mut bytes, &encoded)? {
            break;
        }
    }
    let end = offset + rows.len();
    let next = (end < names.len()).then(|| {
        CursorRef {
            s: snapshot.clone(),
            o: end,
            k: None,
        }
        .encode()
    });
    Ok(
        json!({"snapshot":snapshot.encode(),"entries":rows,"nextCursor":next,"metadata":{"totalEntries":names.len()}}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture(count: usize) -> (tempfile::TempDir, Repository, RepoRef) {
        let temp = tempfile::tempdir().unwrap();
        let repo = Repository::init(temp.path()).unwrap();
        let path = repo.path().join("config");
        let mut config = fs::read_to_string(&path).unwrap();
        for i in (0..count).rev() {
            config.push_str(&format!(
                "\n[remote \"remote-{i:06}\"]\nurl=https://user:secret@example.test/{i}\n"
            ));
        }
        fs::write(path, config).unwrap();
        let admin = repo.path().canonicalize().unwrap();
        let meta = fs::metadata(&admin).unwrap();
        let id = RepoRef::new(&admin, meta.dev(), meta.ino());
        (temp, repo, id)
    }
    #[test]
    fn pages_walk_all_names_search_and_survive_reconnection() {
        let (_temp, repo, id) = fixture(2001);
        let mut cache = Cache::default();
        let q = query("").unwrap();
        let first = page(&mut cache, id.clone(), q.clone(), 200, None, 0).unwrap();
        assert_eq!(first["entries"].as_array().unwrap().len(), 200);
        assert_eq!(first["metadata"]["totalEntries"], 2001);
        assert!(!first.to_string().contains("secret"));
        let mut current = first.clone();
        let mut names = Vec::new();
        loop {
            names.extend(
                current["entries"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|v| v["name"].as_str().unwrap().to_owned()),
            );
            let Some(cursor) = current["nextCursor"].as_str() else {
                break;
            };
            let c = CursorRef::decode(cursor).unwrap();
            current = page(&mut cache, id.clone(), q.clone(), 200, Some(c.s), c.o).unwrap();
        }
        assert_eq!(
            names,
            (0..2001)
                .map(|i| format!("remote-{i:06}"))
                .collect::<Vec<_>>()
        );
        let c = CursorRef::decode(first["nextCursor"].as_str().unwrap()).unwrap();
        let warm = page(
            &mut cache,
            id.clone(),
            q.clone(),
            200,
            Some(c.s.clone()),
            c.o,
        )
        .unwrap();
        let cold = page(
            &mut Cache::default(),
            id.clone(),
            q.clone(),
            200,
            Some(c.s.clone()),
            c.o,
        )
        .unwrap();
        assert_eq!(warm, cold);
        let mut uncached = Cache::default();
        assert_eq!(
            page_with_limit(
                &mut uncached,
                id.clone(),
                q.clone(),
                200,
                Some(c.s.clone()),
                c.o,
                0
            )
            .unwrap(),
            cold
        );
        assert!(uncached.0.is_empty());
        let filtered = page(
            &mut cache,
            id.clone(),
            query("REMOTE-002000").unwrap(),
            20,
            None,
            0,
        )
        .unwrap();
        assert_eq!(filtered["entries"], json!([{"name":"remote-002000"}]));
        assert!(filtered["nextCursor"].is_null());
        repo.remote("added", "https://example.test/added").unwrap();
        assert_eq!(
            page(
                &mut cache,
                id.clone(),
                q.clone(),
                200,
                Some(c.s.clone()),
                c.o
            )
            .unwrap(),
            warm
        );
        assert_eq!(
            page(&mut Cache::default(), id, q, 200, Some(c.s), c.o)
                .unwrap_err()
                .code,
            "SNAPSHOT_EXPIRED"
        );
    }
    #[test]
    fn empty_searches_end_and_cached_reads_verify_repository_identity() {
        let (temp, repo, id) = fixture(0);
        let mut cache = Cache::default();
        let empty = page(&mut cache, id.clone(), query("").unwrap(), 20, None, 0).unwrap();
        assert_eq!(empty["entries"], json!([]));
        assert_eq!(empty["metadata"]["totalEntries"], 0);
        assert!(empty["nextCursor"].is_null());
        repo.remote("origin", "https://example.test/repo").unwrap();
        let unmatched = page(
            &mut cache,
            id.clone(),
            query("absent").unwrap(),
            20,
            None,
            0,
        )
        .unwrap();
        assert_eq!(unmatched["entries"], json!([]));
        assert!(unmatched["nextCursor"].is_null());
        let first = page(&mut cache, id.clone(), query("").unwrap(), 20, None, 0).unwrap();
        let snapshot = SnapshotRef::decode(first["snapshot"].as_str().unwrap()).unwrap();
        let admin = repo.path().to_owned();
        drop(repo);
        fs::rename(admin, temp.path().join("old-admin")).unwrap();
        Repository::init(temp.path()).unwrap();
        assert!(page(&mut cache, id, query("").unwrap(), 20, Some(snapshot), 0).is_err());
    }

    #[test]
    fn service_rejects_cross_query_cross_repository_and_invalid_cursors() {
        let (_temp, _repo, id) = fixture(3);
        let (_other_temp, _other_repo, other) = fixture(3);
        let mut service = Service::default();
        let q = query("").unwrap();
        let first = service.page(id.encode(), q.clone(), 1, None).unwrap();
        let cursor = first["nextCursor"].as_str().unwrap().to_owned();
        assert!(service
            .page(
                id.encode(),
                query("remote").unwrap(),
                1,
                Some(cursor.clone())
            )
            .is_err());
        assert!(service
            .page(other.encode(), q.clone(), 1, Some(cursor.clone()))
            .is_err());
        let mut c = CursorRef::decode(&cursor).unwrap();
        c.o = 99;
        assert!(service
            .page(id.encode(), q.clone(), 1, Some(c.encode()))
            .is_err());
        c.o = 1;
        c.k = Some("bad".into());
        assert!(service
            .page(id.encode(), q.clone(), 1, Some(c.encode()))
            .is_err());
        for size in [0, 201] {
            assert!(service.page(id.encode(), q.clone(), size, None).is_err());
        }
        assert!(query("bad\0filter").is_err());
        assert!(query(&"a".repeat(1025)).is_err());
    }
}
