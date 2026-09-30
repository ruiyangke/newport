//! Bounded worktree snapshots. Hash the canonical array incrementally so
//! pagination and filtering never weaken existing mutation snapshot guards.
use super::*;
use std::{collections::BinaryHeap, ffi::OsString, io::Write};
#[cfg(not(test))]
const INDEX_BYTES: usize = 8 * 1024 * 1024;
#[cfg(test)]
const INDEX_BYTES: usize = 1024 * 1024;
const BLOCK_BYTES: usize = 256 * 1024;
const BLOCK_ROWS: usize = 64;
const SORT_CHUNK: usize = 1024;
#[derive(Default)]
pub(super) struct Cache(Vec<(String, Arc<Capture>)>);
impl Cache {
    fn get(&mut self, token: &str) -> Option<Arc<Capture>> {
        let index = self.0.iter().position(|(key, _)| key == token)?;
        let cached = self.0.remove(index);
        let result = cached.1.clone();
        self.0.push(cached);
        Some(result)
    }
}
struct Capture {
    blocks: Vec<Block>,
    pending: Vec<u8>,
    pending_rows: usize,
    total: usize,
    metadata: Value,
    bytes: usize,
}
struct Block {
    first: usize,
    rows: usize,
    decoded_bytes: usize,
    compressed: Vec<u8>,
}
impl Block {
    fn visit(
        &self,
        mut visit: impl FnMut(usize, &[u8]) -> Result<bool, Error>,
    ) -> Result<bool, Error> {
        if self.decoded_bytes > BLOCK_BYTES || self.rows > BLOCK_ROWS {
            return Err(limit());
        }
        let mut decoded = Vec::with_capacity(self.decoded_bytes);
        flate2::read::ZlibDecoder::new(self.compressed.as_slice())
            .take(self.decoded_bytes as u64 + 1)
            .read_to_end(&mut decoded)
            .map_err(io_error)?;
        if decoded.len() != self.decoded_bytes {
            return Err(limit());
        }
        let mut rest = decoded.as_slice();
        for i in 0..self.rows {
            let prefix = rest.get(..4).ok_or_else(limit)?;
            let len = u32::from_be_bytes(prefix.try_into().map_err(|_| limit())?) as usize;
            rest = &rest[4..];
            let row = rest.get(..len).ok_or_else(limit)?;
            rest = &rest[len..];
            if !visit(self.first + i, row)? {
                return Ok(false);
            }
        }
        if !rest.is_empty() {
            return Err(limit());
        }
        Ok(true)
    }
}
impl Capture {
    fn new() -> Self {
        Self {
            blocks: Vec::new(),
            pending: Vec::new(),
            pending_rows: 0,
            total: 0,
            metadata: Value::Null,
            bytes: 0,
        }
    }
    fn flush(&mut self) -> Result<(), Error> {
        if self.pending_rows == 0 {
            return Ok(());
        }
        let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::fast());
        encoder.write_all(&self.pending).map_err(io_error)?;
        let compressed = encoder.finish().map_err(io_error)?;
        self.bytes += compressed.capacity() + 2 * std::mem::size_of::<Block>();
        self.blocks.push(Block {
            first: self.total - self.pending_rows,
            rows: self.pending_rows,
            decoded_bytes: self.pending.len(),
            compressed,
        });
        self.pending.clear();
        self.pending_rows = 0;
        Ok(())
    }
    fn push(&mut self, encoded: &[u8]) -> Result<bool, Error> {
        if encoded.len() + 4 > BLOCK_BYTES {
            return Ok(false);
        }
        if self.pending.len() + encoded.len() + 4 > BLOCK_BYTES || self.pending_rows == BLOCK_ROWS {
            self.flush()?;
        }
        self.pending
            .extend_from_slice(&(encoded.len() as u32).to_be_bytes());
        self.pending.extend_from_slice(encoded);
        self.pending_rows += 1;
        self.total += 1;
        // Include the builder and one decode/compression window, not merely
        // compressed bytes. A large cache must never conceal large transients.
        Ok(self.bytes + self.pending.capacity() + 2 * BLOCK_BYTES <= INDEX_BYTES)
    }
    fn page(&self, offset: usize, count: usize) -> Result<Vec<Value>, Error> {
        if offset > self.total {
            return Err(Error::invalid("Cursor is past the end of this listing."));
        }
        let (mut rows, mut size) = (Vec::new(), 0);
        for block in &self.blocks {
            if block.first + block.rows <= offset {
                continue;
            }
            if block.first >= offset.saturating_add(count) {
                break;
            }
            if !block.visit(|index, encoded| {
                if index >= offset && rows.len() < count {
                    add(&mut rows, &mut size, encoded)
                } else {
                    Ok(true)
                }
            })? {
                break;
            }
        }
        Ok(rows)
    }
}
pub(super) fn query(
    filter: String,
    branch: Option<String>,
    name: Option<String>,
) -> Result<String, Error> {
    if filter.len() > 1024
        || filter.contains('\0')
        || branch
            .as_ref()
            .is_some_and(|s| s.len() > 1024 || s.contains('\0'))
        || name
            .as_ref()
            .is_some_and(|s| s.len() > 1024 || s.contains('\0'))
    {
        return Err(Error::invalid(
            "Worktree filters must be at most 1024 bytes without NUL.",
        ));
    }
    if filter.is_empty() && branch.is_none() && name.is_none() {
        return Ok("worktrees".into());
    }
    Ok(format!(
        "worktrees:{}",
        serde_json::to_string(&(filter.to_lowercase(), branch, name)).map_err(|_| limit())?
    ))
}
/// Visit names in raw-byte order using a bounded sorting window. Even an
/// oversized registry needs no full name array or full JSON response in memory.
pub(in crate::git) fn scan(
    repo: &Repository,
    mut visit: impl FnMut(Value, &[u8]) -> Result<(), Error>,
) -> Result<String, Error> {
    let common = repo.commondir().canonicalize().map_err(io_error)?;
    let current = repo.path().canonicalize().map_err(io_error)?;
    let registry = common.join("worktrees");
    let mut hash = Sha256::new();
    hash.update(b"[");
    let mut first = true;
    let mut emit = |row: Value| -> Result<(), Error> {
        let encoded = serde_json::to_vec(&row).map_err(|_| limit())?;
        if !first {
            hash.update(b",");
        }
        first = false;
        hash.update(&encoded);
        visit(row, &encoded)
    };
    emit(worktree_main(repo)?)?;
    let mut after: Option<OsString> = None;
    loop {
        let entries = match fs::read_dir(&registry) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => break,
            Err(e) => return Err(io_error(e)),
        };
        let mut names = BinaryHeap::new();
        for entry in entries {
            let name = entry.map_err(io_error)?.file_name();
            if after.as_ref().is_some_and(|key| &name <= key) {
                continue;
            }
            if names.len() < SORT_CHUNK {
                names.push(name);
            } else if names.peek().is_some_and(|largest| &name < largest) {
                names.pop();
                names.push(name);
            }
        }
        if names.is_empty() {
            break;
        }
        let names = names.into_sorted_vec();
        after = names.last().cloned();
        let last = names.len() < SORT_CHUNK;
        for name in names {
            emit(worktree_row(repo, &common, &current, &registry, &name)?)?;
        }
        if last {
            break;
        }
    }
    hash.update(b"]");
    Ok(hex(&hash.finalize()))
}
fn matches(row: &Value, filter: &str, branch: &Option<String>, name: &Option<String>) -> bool {
    let display = |v: &Value| v["display"].as_str().unwrap_or("").to_owned();
    branch
        .as_ref()
        .is_none_or(|b| row["head"]["name"]["bytesB64"] == STANDARD.encode(b.as_bytes()))
        && name
            .as_ref()
            .is_none_or(|n| row["name"]["bytesB64"] == STANDARD.encode(n.as_bytes()))
        && (filter.is_empty()
            || [&row["name"], &row["path"], &row["head"]["name"]]
                .iter()
                .any(|v| display(v).to_lowercase().contains(filter)))
}
fn add(rows: &mut Vec<Value>, size: &mut usize, encoded: &[u8]) -> Result<bool, Error> {
    if encoded.len() > MAX_FRAME / 2 {
        return Err(limit());
    }
    if *size + encoded.len() > MAX_FRAME / 2 {
        return Ok(false);
    }
    rows.push(serde_json::from_slice(encoded).map_err(|_| limit())?);
    *size += encoded.len();
    Ok(true)
}
fn response(snapshot: SnapshotRef, entries: Vec<Value>, offset: usize, metadata: &Value) -> Value {
    let end = offset + entries.len();
    let next = (end < metadata["matchingEntries"].as_u64().unwrap_or(0) as usize).then(|| {
        CursorRef {
            s: snapshot.clone(),
            o: end,
            k: None,
        }
        .encode()
    });
    let mut mutation_snapshot = snapshot;
    mutation_snapshot.q = "worktrees".into();
    json!({"snapshot":mutation_snapshot.encode(),"entries":entries,"nextCursor":next,"metadata":metadata})
}
pub(super) fn anchored_page(
    cache: &mut Cache,
    repository: RepoRef,
    query: String,
    count: usize,
    token: &str,
) -> Result<Value, Error> {
    if !(1..=200).contains(&count) {
        return Err(Error::invalid("pageSize must be between 1 and 200."));
    }
    let mut anchor = SnapshotRef::decode(token)?;
    if anchor.r != repository || anchor.q != "worktrees" || anchor.p.is_some() {
        return Err(Error::invalid(
            "atSnapshot must be an unfiltered worktree snapshot for this repository.",
        ));
    }
    anchor.q = query.clone();
    page(cache, repository, query, count, Some(anchor), 0)
}
pub(super) fn page(
    cache: &mut Cache,
    repository: RepoRef,
    query: String,
    count: usize,
    snapshot: Option<SnapshotRef>,
    offset: usize,
) -> Result<Value, Error> {
    let repo = repository.open()?;
    if let Some(snapshot) = &snapshot {
        let token = snapshot.encode();
        if let Some(data) = cache.get(&token) {
            let rows = data.page(offset, count)?;
            return Ok(response(snapshot.clone(), rows, offset, &data.metadata));
        }
    }
    // A filtered continuation or explicit anchor may reuse the full frozen
    // capture. No implicit anchor is used for ordinary fresh searches.
    let source = snapshot.as_ref().and_then(|snapshot| {
        let mut unfiltered = snapshot.clone();
        unfiltered.q = "worktrees".into();
        cache.get(&unfiltered.encode())
    });
    let (filter, branch, name): (String, Option<String>, Option<String>) = if query == "worktrees" {
        (String::new(), None, None)
    } else {
        serde_json::from_str(
            query
                .strip_prefix("worktrees:")
                .ok_or_else(|| Error::invalid("Invalid worktree query."))?,
        )
        .map_err(|_| Error::invalid("Invalid worktree query."))?
    };
    let (mut rows, mut size, mut total, mut matching) = (Vec::new(), 0, 0, 0);
    let (mut main, mut current) = (Value::Null, Value::Null);
    let mut captured = Some(Capture::new());
    let mut visit = |row: Value, encoded: &[u8]| {
        total += 1;
        if row["kind"] != "linked" {
            main = row.clone();
        }
        if row["current"] == true {
            current = row.clone();
        }
        if matches(&row, &filter, &branch, &name) {
            if matching == offset + rows.len() && rows.len() < count {
                add(&mut rows, &mut size, encoded)?;
            }
            matching += 1;
            if let Some(cache) = &mut captured {
                if !cache.push(encoded)? {
                    captured = None;
                }
            }
        }
        Ok(())
    };
    let fingerprint = if let Some(source) = source {
        for block in &source.blocks {
            block.visit(|_, encoded| {
                visit(
                    serde_json::from_slice(encoded).map_err(|_| limit())?,
                    encoded,
                )?;
                Ok(true)
            })?;
        }
        source.metadata["listToken"]
            .as_str()
            .ok_or_else(limit)?
            .to_owned()
    } else {
        scan(&repo, visit)?
    };
    if snapshot.as_ref().is_some_and(|s| s.f != fingerprint) {
        return Err(Error::new(
            "SNAPSHOT_EXPIRED",
            "Worktrees changed. Restart from the first page.",
        ));
    }
    if offset > matching {
        return Err(Error::invalid("Cursor is past the end of this listing."));
    }
    let metadata = json!({"listToken":fingerprint,"totalEntries":total,"matchingEntries":matching,"current":current,"main":main});
    let snapshot = snapshot.unwrap_or(SnapshotRef {
        r: repository,
        q: query,
        f: fingerprint,
        p: None,
    });
    if let Some(mut captured) = captured {
        captured.flush()?;
        captured.pending = Vec::new();
        captured.bytes += BLOCK_BYTES; // one bounded decompression window
        captured.metadata = metadata.clone();
        captured.bytes += serde_json::to_vec(&metadata).map_err(|_| limit())?.len() * 8;
        let token = snapshot.encode();
        captured.bytes += token.len();
        if captured.bytes <= INDEX_BYTES {
            cache.0.retain(|(key, _)| key != &token);
            cache.0.push((token, Arc::new(captured)));
            while cache.0.len() > CACHE_ENTRIES
                || cache.0.iter().map(|(_, c)| c.bytes).sum::<usize>() > CACHE_BYTES
            {
                cache.0.remove(0);
            }
        }
    }
    Ok(response(snapshot, rows, offset, &metadata))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture(root: &Path, count: usize) -> Repository {
        let repo = Repository::init(root.join("main")).unwrap();
        super::super::tests::commit(&repo, "base");
        let oid = repo.head().unwrap().target().unwrap();
        for i in 0..count {
            let name = format!("tree-{i:05}");
            let admin = repo.path().join("worktrees").join(&name);
            let checkout = root.join(&name);
            fs::create_dir_all(&admin).unwrap();
            fs::create_dir_all(&checkout).unwrap();
            fs::write(admin.join("commondir"), "../..\n").unwrap();
            fs::write(
                admin.join("gitdir"),
                format!("{}\n", checkout.join(".git").display()),
            )
            .unwrap();
            fs::write(
                checkout.join(".git"),
                format!("gitdir: {}\n", admin.display()),
            )
            .unwrap();
            fs::write(admin.join("HEAD"), format!("ref: refs/heads/{name}\n")).unwrap();
            repo.reference(&format!("refs/heads/{name}"), oid, false, "fixture")
                .unwrap();
        }
        repo
    }
    fn request(
        service: &mut Service,
        repo: &Repository,
        cursor: Option<String>,
        filter: &str,
        branch: Option<&str>,
        name: Option<&str>,
    ) -> Result<Value, Error> {
        let stat = fs::metadata(repo.path()).unwrap();
        let id =
            RepoRef::new(&repo.path().canonicalize().unwrap(), stat.dev(), stat.ino()).encode();
        match service.request(Request::Worktrees {
            at_snapshot: None,
            repo_id: id,
            page_size: 100,
            cursor,
            filter: filter.into(),
            branch: branch.map(str::to_owned),
            name: name.map(str::to_owned),
        })? {
            Output::Json(v) => Ok(v),
            _ => panic!(),
        }
    }
    #[test]
    fn complete_pages_exact_filters_and_metadata_preserve_mutation_token() {
        let temp = tempfile::tempdir().unwrap();
        let repo = fixture(temp.path(), 1201);
        let mut service = Service::default();
        let first = request(&mut service, &repo, None, "", None, None).unwrap();
        assert_eq!(first["metadata"]["totalEntries"], 1202);
        assert_eq!(first["metadata"]["main"], first["entries"][0]);
        assert_eq!(first["metadata"]["current"], first["entries"][0]);
        let rows = worktree_rows(&repo).unwrap();
        let old_token = crate::git::journal::hash(&serde_json::to_vec(&rows).unwrap());
        assert_eq!(first["metadata"]["listToken"], old_token);
        let mut all = first["entries"].as_array().unwrap().clone();
        let mut page = first.clone();
        while let Some(cursor) = page["nextCursor"].as_str() {
            page = request(&mut service, &repo, Some(cursor.into()), "", None, None).unwrap();
            assert_eq!(page["metadata"], first["metadata"]);
            all.extend(page["entries"].as_array().unwrap().iter().cloned());
        }
        assert_eq!(all, rows);
        let filtered = request(
            &mut service,
            &repo,
            None,
            "TREE-01200",
            Some("refs/heads/tree-01200"),
            Some("tree-01200"),
        )
        .unwrap();
        assert_eq!(filtered["entries"].as_array().unwrap().len(), 1);
        assert_eq!(filtered["metadata"]["matchingEntries"], 1);
        assert_eq!(filtered["metadata"]["totalEntries"], 1202);
        assert_eq!(filtered["metadata"]["main"], first["entries"][0]);
        assert_eq!(filtered["snapshot"], first["snapshot"]);
        let cursor = first["nextCursor"].as_str().unwrap().to_owned();
        assert!(request(
            &mut service,
            &repo,
            Some(cursor.clone()),
            "other",
            None,
            None
        )
        .is_err());
        let mut reconnect = Service::default();
        assert_eq!(
            request(&mut reconnect, &repo, Some(cursor.clone()), "", None, None).unwrap()
                ["entries"],
            serde_json::to_value(&rows[100..200]).unwrap()
        );
        fs::write(repo.path().join("worktrees/tree-01200/locked"), "new lock").unwrap();
        // Cached pages retain the captured snapshot; reconnect/new reads verify all rows.
        assert_eq!(
            request(&mut service, &repo, Some(cursor.clone()), "", None, None).unwrap()["snapshot"],
            first["snapshot"]
        );
        assert_eq!(
            request(&mut Service::default(), &repo, Some(cursor), "", None, None)
                .unwrap_err()
                .code,
            "SNAPSHOT_EXPIRED"
        );
        assert_ne!(
            request(&mut service, &repo, None, "", None, None).unwrap()["snapshot"],
            first["snapshot"]
        );
    }
    fn anchored_request(
        service: &mut Service,
        repo: &Repository,
        snapshot: &str,
        cursor: Option<String>,
        filter: &str,
    ) -> Result<Value, Error> {
        let stat = fs::metadata(repo.path()).unwrap();
        let repo_id =
            RepoRef::new(&repo.path().canonicalize().unwrap(), stat.dev(), stat.ino()).encode();
        match service.request(Request::Worktrees {
            repo_id,
            page_size: 100,
            cursor,
            filter: filter.into(),
            branch: None,
            name: None,
            at_snapshot: Some(snapshot.into()),
        })? {
            Output::Json(v) => Ok(v),
            _ => panic!(),
        }
    }
    #[test]
    fn anchored_search_preserves_captured_order_and_expires_after_eviction() {
        let temp = tempfile::tempdir().unwrap();
        let repo = fixture(temp.path(), 230);
        let mut service = Service::with_journal(
            Journal::open(
                temp.path().join("journal"),
                uuid::Uuid::new_v4().to_string(),
            )
            .unwrap(),
        );
        let first = request(&mut service, &repo, None, "", None, None).unwrap();
        let snapshot = first["snapshot"].as_str().unwrap();
        let rows = worktree_rows(&repo).unwrap();
        // Changes cannot leak into a search of the explicitly requested snapshot.
        fs::write(repo.path().join("worktrees/tree-00229/locked"), "new lock").unwrap();
        let mut filtered = anchored_request(&mut service, &repo, snapshot, None, "tree-").unwrap();
        assert_eq!(filtered["snapshot"], first["snapshot"]);
        assert_eq!(filtered["metadata"]["matchingEntries"], 230);
        let mut entries = filtered["entries"].as_array().unwrap().clone();
        while let Some(cursor) = filtered["nextCursor"].as_str() {
            filtered = request(
                &mut service,
                &repo,
                Some(cursor.into()),
                "tree-",
                None,
                None,
            )
            .unwrap();
            entries.extend(filtered["entries"].as_array().unwrap().iter().cloned());
        }
        assert_eq!(entries, rows[1..]);
        assert_ne!(
            crate::git::worktrees::token(&repo).unwrap(),
            first["metadata"]["listToken"].as_str().unwrap()
        );
        let state = SnapshotRef::decode(snapshot).unwrap();
        let operation_id = uuid::Uuid::new_v4().to_string();
        let result = service.request(Request::Start {
            operation_id: operation_id.clone(),
            repo_id: state.r.encode(),
            expected_snapshot: snapshot.into(),
            action: Action::WorktreeUnlock {
                name: "tree-00229".into(),
            },
        });
        assert_eq!(result.err().unwrap().code, "STALE_SNAPSHOT");
        assert_eq!(
            fs::read(repo.path().join("worktrees/tree-00229/locked")).unwrap(),
            b"new lock"
        );
        assert_eq!(
            service
                .journal
                .as_ref()
                .unwrap()
                .get(&operation_id)
                .unwrap_err()
                .code,
            "OPERATION_NOT_FOUND"
        );
        // A fresh search deliberately bypasses the frozen source.
        let fresh = request(&mut service, &repo, None, "tree-00229", None, None).unwrap();
        assert_eq!(fresh["entries"][0]["locked"], true);
        assert_ne!(fresh["snapshot"], first["snapshot"]);
        service.worktree_cache.0.clear();
        assert_eq!(
            anchored_request(&mut service, &repo, snapshot, None, "tree-")
                .unwrap_err()
                .code,
            "SNAPSHOT_EXPIRED"
        );
        // Unchanged repositories can reconstruct an anchor after reconnect.
        let restored = anchored_request(
            &mut service,
            &repo,
            fresh["snapshot"].as_str().unwrap(),
            None,
            "tree-00229",
        )
        .unwrap();
        assert_eq!(restored["entries"], fresh["entries"]);
    }
    #[test]
    fn anchors_reject_cross_repository_query_parameters_and_cursor_combinations() {
        let temp = tempfile::tempdir().unwrap();
        let repo = fixture(temp.path(), 2);
        let mut service = Service::default();
        let first = request(&mut service, &repo, None, "", None, None).unwrap();
        let snapshot = first["snapshot"].as_str().unwrap();
        assert!(anchored_request(&mut service, &repo, snapshot, Some("bad".into()), "").is_err());
        assert!(anchored_request(&mut service, &repo, "not a token", None, "").is_err());
        let other = tempfile::tempdir().unwrap();
        let other_repo = fixture(other.path(), 0);
        assert!(anchored_request(&mut service, &other_repo, snapshot, None, "").is_err());
        let mut token = SnapshotRef::decode(snapshot).unwrap();
        for q in ["status", "worktrees:[\"filter\",null,null]"] {
            token.q = q.into();
            assert!(anchored_request(&mut service, &repo, &token.encode(), None, "").is_err());
        }
        token.q = "worktrees".into();
        token.p = Some("unexpected".into());
        assert!(anchored_request(&mut service, &repo, &token.encode(), None, "").is_err());
        let captured = service.worktree_cache.0.last_mut().unwrap();
        let capture = Arc::get_mut(&mut captured.1).unwrap();
        capture.blocks[0].decoded_bytes = BLOCK_BYTES + 1;
        assert!(anchored_request(&mut service, &repo, snapshot, None, "tree-").is_err());
    }
    #[test]
    fn linked_current_descriptor_survives_nonmatching_filters() {
        let temp = tempfile::tempdir().unwrap();
        let main = fixture(temp.path(), 2);
        let linked = Repository::open(temp.path().join("tree-00001")).unwrap();
        let page = request(
            &mut Service::default(),
            &linked,
            None,
            "nothing matches",
            None,
            None,
        )
        .unwrap();
        assert!(page["entries"].as_array().unwrap().is_empty());
        assert_eq!(page["metadata"]["matchingEntries"], 0);
        assert_eq!(page["metadata"]["totalEntries"], 3);
        assert_eq!(page["metadata"]["current"]["name"]["display"], "tree-00001");
        assert_eq!(
            page["metadata"]["main"]["path"],
            serde_json::to_value(wire_path(main.workdir().unwrap())).unwrap()
        );
        assert_eq!(page["metadata"]["current"]["current"], true);
    }
    #[test]
    fn compressed_blocks_reject_corruption_and_bound_decoding() {
        let mut capture = Capture::new();
        for i in 0..130 {
            assert!(capture
                .push(&serde_json::to_vec(&json!({"index":i})).unwrap())
                .unwrap());
        }
        capture.flush().unwrap();
        assert_eq!(
            capture.page(63, 3).unwrap(),
            vec![
                json!({"index":63}),
                json!({"index":64}),
                json!({"index":65})
            ]
        );
        assert_eq!(capture.page(129, 100).unwrap(), vec![json!({"index":129})]);
        assert!(capture.page(usize::MAX, 100).is_err());
        capture.blocks[0].decoded_bytes = BLOCK_BYTES + 1;
        assert!(capture.page(0, 1).is_err());
        capture.blocks[0].decoded_bytes = 100;
        capture.blocks[0].compressed = vec![0; 32];
        assert!(capture.page(0, 1).is_err());
    }
    #[test]
    fn empty_defaults_preserve_wire_and_filters_reject_unbounded_inputs() {
        let request: Request =
            serde_json::from_value(json!({"method":"repo.worktrees","params":{"repoId":"repo"}}))
                .unwrap();
        let encoded = serde_json::to_value(request).unwrap();
        assert!(encoded["params"].get("filter").is_none());
        assert!(encoded["params"].get("branch").is_none());
        assert!(encoded["params"].get("name").is_none());
        assert!(encoded["params"].get("atSnapshot").is_none());
        assert!(query("x".repeat(1025), None, None).is_err());
        assert!(query("\0".into(), None, None).is_err());
    }
    #[test]
    fn oversized_capture_falls_back_without_truncation() {
        let temp = tempfile::tempdir().unwrap();
        let repo = fixture(temp.path(), 80);
        for i in 0..80 {
            fs::write(repo.path().join(format!("worktrees/tree-{i:05}/locked")), {
                let mut seed = i as u64 + 1;
                (0..16 * 1024)
                    .map(|_| {
                        seed ^= seed << 13;
                        seed ^= seed >> 7;
                        seed ^= seed << 17;
                        b' ' + (seed % 95) as u8
                    })
                    .collect::<Vec<u8>>()
            })
            .unwrap();
        }
        let mut service = Service::default();
        let mut page = request(&mut service, &repo, None, "", None, None).unwrap();
        assert!(service.worktree_cache.0.is_empty());
        let mut count = page["entries"].as_array().unwrap().len();
        while let Some(cursor) = page["nextCursor"].as_str() {
            page = request(&mut service, &repo, Some(cursor.into()), "", None, None).unwrap();
            count += page["entries"].as_array().unwrap().len();
        }
        assert_eq!(count, 81);
    }
}
