//! Historical and working-tree diff pages. Cursor offsets count bounded line pieces,
//! so a single large hunk or long line never creates an unreachable tail.
//! Raw content is sent once as base64; clients join pieces before decoding text.
//! Working hunk IDs identify complete native hunks, but a client must only offer
//! hunk actions once all `totalLines` have arrived. Line IDs appear only on the
//! final piece; clients must assemble every preceding byte before selecting one.
use super::*;
#[path = "diff_page_cache.rs"]
mod cache;
pub(super) use cache::Cache;
const PIECE_BYTES: usize = 4096;
const TEXT_LIMIT: u64 = 2 * 1024 * 1024;

#[derive(Clone)]
struct Window {
    compact: bool,
    start: usize,
    count: usize,
    budget: usize,
    position: usize,
    accepted: usize,
    bytes: usize,
    full: bool,
    files: Vec<Value>,
}
impl Window {
    fn emit(
        &mut self,
        file: &Value,
        hunk: Option<&Value>,
        line: Option<Value>,
    ) -> Result<(), Error> {
        // tuple_v1: index, byte offset, complete, origin, old line, new
        // line, base64 bytes; working diffs append their optional mutation ID.
        let line = line.map(|mut line| {
            if !self.compact {
                return line;
            }
            let working = line.get("id").is_some();
            let mut fields: Vec<_> = [
                "lineIndex",
                "byteOffset",
                "lineComplete",
                "origin",
                "oldLine",
                "newLine",
                "contentBytesB64",
            ]
            .into_iter()
            .map(|field| line[field].take())
            .collect();
            if working {
                fields.push(line["id"].take());
            }
            Value::Array(fields)
        });
        let position = self.position;
        self.position += 1;
        if position < self.start || self.full || self.accepted == self.count {
            return Ok(());
        }
        let new_file = self
            .files
            .last()
            .is_none_or(|f| f["fileIndex"] != file["fileIndex"]);
        let new_hunk = hunk.is_some_and(|h| {
            new_file
                || self.files.last().unwrap()["hunks"]
                    .as_array()
                    .unwrap()
                    .last()
                    .is_none_or(|last| last["index"] != h["index"])
        });
        let size = if new_file {
            serde_json::to_vec(file).map_err(|_| limit())?.len()
        } else {
            0
        } + if new_hunk {
            serde_json::to_vec(hunk.unwrap())
                .map_err(|_| limit())?
                .len()
        } else {
            0
        } + line
            .as_ref()
            .map(|l| {
                serde_json::to_vec(l)
                    .map(|b| b.len() + 1)
                    .map_err(|_| limit())
            })
            .transpose()?
            .unwrap_or(0)
            + 2;
        if self.bytes + size > self.budget {
            if self.accepted == 0 {
                return Err(limit());
            }
            self.full = true;
            return Ok(());
        }
        self.bytes += size;
        self.accepted += 1;
        if new_file {
            self.files.push(file.clone());
        }
        if let Some(hunk) = hunk {
            let hunks = self.files.last_mut().unwrap()["hunks"]
                .as_array_mut()
                .unwrap();
            if new_hunk {
                hunks.push(hunk.clone());
            }
            if let Some(line) = line {
                hunks.last_mut().unwrap()["lines"]
                    .as_array_mut()
                    .unwrap()
                    .push(line);
            }
        }
        Ok(())
    }
}
#[allow(clippy::too_many_arguments)]
pub(super) fn read_cached(
    cache: &mut Cache,
    compact: bool,
    repository: RepoRef,
    commit: &str,
    path: WirePath,
    parent: usize,
    context: u32,
    count: usize,
    budget: Option<usize>,
    cursor: Option<String>,
) -> Result<Value, Error> {
    let budget = budget.unwrap_or(256 * 1024);
    if !(1..=5000).contains(&count) || !(64 * 1024..=MAX_FRAME / 2).contains(&budget) {
        return Err(Error::invalid(
            "pageSize must be 1–5000 and maxBytes must be 65536–524288.",
        ));
    }
    let commit = super::super::branches::oid(commit)?.to_string();
    let selected = path.decode()?;
    if selected.is_empty() {
        return Err(Error::invalid("Select a changed file."));
    }
    let query = format!(
        "commit_diff_page:{}",
        serde_json::to_string(&(&commit, parent, context, STANDARD.encode(&selected)))
            .map_err(|_| limit())?
    );
    let cursor = cursor.map(|value| CursorRef::decode(&value)).transpose()?;
    if cursor
        .as_ref()
        .is_some_and(|c| c.s.r != repository || c.s.q != query || c.s.p.is_some() || c.k.is_some())
    {
        return Err(Error::invalid("Cursor does not match this diff selection."));
    }
    let mut window = Window {
        compact,
        start: cursor.as_ref().map_or(0, |c| c.o),
        count,
        budget,
        position: 0,
        accepted: 0,
        bytes: 0,
        full: false,
        files: Vec::new(),
    };
    let repo = repository.open()?;
    if let Some(cursor) = &cursor {
        if let Some(page) = cache.render(cursor, window.clone())? {
            return Ok(page);
        }
    }
    let mut capture = cache::Capture::default();
    let mut hash = Sha256::new();
    hash.update(b"historical-diff-page-v1");
    let mut matched = false;
    let mut omitted = false;
    let metadata = commit_comparison_for_path(
        &repo,
        &commit,
        parent,
        context,
        Some(&selected),
        |diff| {
            let (found, omissions, _) = collect(
                diff,
                Some(&selected),
                false,
                &mut window,
                &mut capture,
                &mut hash,
            )?;
            matched = found;
            omitted = omissions;
            Ok(
                json!({"contextLines":context,"selectedPath":WirePath::new(&selected),"readOnly":true,"hasOmissions":omitted,"totalUnits":window.position}),
            )
        },
    )?;
    if !matched {
        return Err(Error::new(
            "FILE_NOT_CHANGED",
            "The selected path is not changed in this comparison.",
        ));
    }
    hash.update(serde_json::to_vec(&metadata).map_err(|_| limit())?);
    let fingerprint = hex(&hash.finalize());
    if cursor.as_ref().is_some_and(|c| c.s.f != fingerprint) {
        return Err(Error::new(
            "SNAPSHOT_EXPIRED",
            "The diff changed. Restart from its first page.",
        ));
    }
    if window.start > window.position {
        return Err(Error::invalid("Cursor is past the end of this diff."));
    }
    let snapshot = SnapshotRef {
        r: repository,
        q: query,
        f: fingerprint,
        p: None,
    };
    let result = render(window, &snapshot, &metadata)?;
    if let Some(capture) = capture.finish(&metadata)? {
        cache.insert(&snapshot, capture);
    }
    Ok(result)
}

/// Continuations read a frozen capture. Mutations still validate sourceSnapshot
/// against the current repository; a page snapshot is never a write token.
#[allow(clippy::too_many_arguments)]
pub(super) fn read_working_cached(
    cache: &mut Cache,
    compact: bool,
    repository: RepoRef,
    source: SnapshotRef,
    entry: &str,
    side: Side,
    context: u32,
    count: usize,
    budget: Option<usize>,
    cursor: Option<String>,
) -> Result<Value, Error> {
    let budget = budget.unwrap_or(256 * 1024);
    if !(1..=5000).contains(&count)
        || !(64 * 1024..=MAX_FRAME / 2).contains(&budget)
        || context > 100
    {
        return Err(Error::invalid(
            "Invalid diff page size, byte budget, or context.",
        ));
    }
    if source.r != repository || source.q != "status" || source.p.is_some() {
        return Err(Error::invalid(
            "Diff requires a status snapshot from this repository.",
        ));
    }
    EntryRef::decode(entry)?.paths()?;
    // Bind to the complete source snapshot and comparison, not a mutable path.
    let query = format!(
        "working_diff_page:{}",
        hex(&Sha256::digest(
            serde_json::to_vec(&(&source.encode(), entry, &side, context)).map_err(|_| limit())?
        ))
    );
    let cursor = cursor.map(|value| CursorRef::decode(&value)).transpose()?;
    if cursor
        .as_ref()
        .is_some_and(|c| c.s.r != repository || c.s.q != query || c.s.p.is_some() || c.k.is_some())
    {
        return Err(Error::invalid("Cursor does not match this diff selection."));
    }
    let mut window = Window {
        compact,
        start: cursor.as_ref().map_or(0, |c| c.o),
        count,
        budget,
        position: 0,
        accepted: 0,
        bytes: 0,
        full: false,
        files: Vec::new(),
    };
    repository.open()?;
    if let Some(cursor) = &cursor {
        if let Some(page) = cache.render(cursor, window.clone())? {
            return Ok(page);
        }
    }
    let mut capture = cache::Capture::default();
    let mut hash = Sha256::new();
    hash.update(b"working-diff-page-v1");
    let metadata = working_comparison(
        &repository,
        &source,
        entry,
        side.clone(),
        context,
        |diff| {
            let (_, omitted, total_files) =
                collect(diff, None, true, &mut window, &mut capture, &mut hash)?;
            Ok(
                json!({"sourceSnapshot":source.encode(),"entryId":entry,"side":side,"contextLines":context,
            "readOnly":false,"hasOmissions":omitted,"totalFiles":total_files,"totalUnits":window.position}),
            )
        },
    )?;
    hash.update(serde_json::to_vec(&metadata).map_err(|_| limit())?);
    let fingerprint = hex(&hash.finalize());
    if cursor.as_ref().is_some_and(|c| c.s.f != fingerprint) {
        return Err(Error::new(
            "SNAPSHOT_EXPIRED",
            "The diff changed. Restart from its first page.",
        ));
    }
    if window.start > window.position {
        return Err(Error::invalid("Cursor is past the end of this diff."));
    }
    let snapshot = SnapshotRef {
        r: repository,
        q: query,
        f: fingerprint,
        p: None,
    };
    let result = render(window, &snapshot, &metadata)?;
    // working_comparison's final fingerprint must pass before publishing.
    if let Some(capture) = capture.finish(&metadata)? {
        cache.insert(&snapshot, capture);
    }
    Ok(result)
}

fn collect(
    diff: &git2::Diff<'_>,
    selected: Option<&[u8]>,
    mutation_ids: bool,
    window: &mut Window,
    capture: &mut cache::Capture,
    hash: &mut Sha256,
) -> Result<(bool, bool, usize), Error> {
    let mut matched = false;
    let mut omitted = false;
    let mut total_files = 0;
    for index in 0..diff.deltas().len() {
        let delta = diff.get_delta(index).ok_or_else(limit)?;
        if selected.is_some_and(|selected| {
            delta.old_file().path_bytes() != Some(selected)
                && delta.new_file().path_bytes() != Some(selected)
        }) {
            continue;
        }
        matched = true;
        total_files += 1;
        let patch = git2::Patch::from_diff(diff, index).map_err(engine)?;
        let delta = diff.get_delta(index).ok_or_else(limit)?;
        let mut file = delta_metadata(&delta);
        let too_large =
            delta.old_file().size() > TEXT_LIMIT || delta.new_file().size() > TEXT_LIMIT;
        let binary = delta.old_file().is_binary() || delta.new_file().is_binary();
        let (_, additions, deletions) = patch
            .as_ref()
            .map(|p| p.line_stats().map_err(engine))
            .transpose()?
            .unwrap_or((0, 0, 0));
        // A diff attribute can force text even above max_size. In that
        // case libgit2 returned real hunks; do not label their content omitted.
        let reason = if too_large && binary {
            Some("file_size_limit")
        } else if binary {
            Some("binary")
        } else {
            None
        };
        omitted |= reason.is_some();
        file["fileIndex"] = index.into();
        file["binary"] = binary.into();
        file["omissionReason"] = json!(reason);
        file["additions"] = additions.into();
        file["deletions"] = deletions.into();
        file["hunks"] = json!([]);
        hash.update(serde_json::to_vec(&file).map_err(|_| limit())?);
        capture.file(&file)?;
        let Some(patch) = patch.filter(|p| p.num_hunks() != 0) else {
            capture.unit(None, None)?;
            window.emit(&file, None, None)?;
            continue;
        };
        for h in 0..patch.num_hunks() {
            let (header, lines) = patch.hunk(h).map_err(engine)?;
            let mut hunk = json!({"index":h,"oldStart":header.old_start(),"oldLines":header.old_lines(),"newStart":header.new_start(),"newLines":header.new_lines(),"lines":[]});
            let hunk_id = if mutation_ids {
                Some(super::super::hunks::id(&patch, h)?)
            } else {
                None
            };
            if let Some(id) = &hunk_id {
                hunk["id"] = id.clone().into();
                hunk["totalLines"] = lines.into();
            }
            hash.update(serde_json::to_vec(&hunk).map_err(|_| limit())?);
            capture.hunk(&hunk)?;
            for l in 0..lines {
                let line = patch.line_in_hunk(h, l).map_err(engine)?;
                hash.update((line.origin() as u32).to_le_bytes());
                hash.update(line.old_lineno().unwrap_or(u32::MAX).to_le_bytes());
                hash.update(line.new_lineno().unwrap_or(u32::MAX).to_le_bytes());
                hash.update((line.content().len() as u64).to_le_bytes());
                hash.update(line.content());
                let line_id = hunk_id
                    .as_ref()
                    .and_then(|id| super::super::hunks::line_id(id, l, &line));
                let chunks = line.content().len().div_ceil(PIECE_BYTES).max(1);
                for piece in 0..chunks {
                    let offset = piece * PIECE_BYTES;
                    let bytes =
                        &line.content()[offset..line.content().len().min(offset + PIECE_BYTES)];
                    capture.unit(
                        Some((&line, l, offset, bytes, piece + 1 == chunks)),
                        line_id.as_deref().filter(|_| piece + 1 == chunks),
                    )?;
                    // Only encode the requested window. Hashing the native
                    // patch verifies reconnects without building its JSON.
                    if window.position >= window.start
                        && !window.full
                        && window.accepted < window.count
                    {
                        let mut value = json!({"lineIndex":l,"byteOffset":offset,"lineComplete":piece+1==chunks,"origin":line.origin().to_string(),"oldLine":line.old_lineno(),"newLine":line.new_lineno(),"contentBytesB64":STANDARD.encode(bytes)});
                        if mutation_ids {
                            value["id"] = json!(line_id.as_ref().filter(|_| piece + 1 == chunks));
                        }
                        window.emit(&file, Some(&hunk), Some(value))?;
                    } else {
                        window.position += 1;
                    }
                }
            }
        }
    }
    Ok((matched, omitted, total_files))
}

fn render(mut window: Window, snapshot: &SnapshotRef, metadata: &Value) -> Result<Value, Error> {
    // Include tokens and metadata in the requested wire budget. Trimming only
    // trailing units keeps the next cursor contiguous even for long paths.
    loop {
        let end = window.start + window.accepted;
        let next = (end < window.position).then(|| {
            CursorRef {
                s: snapshot.clone(),
                o: end,
                k: None,
            }
            .encode()
        });
        let result = json!({"snapshot":snapshot.encode(),"entries":window.files,"nextCursor":next,"metadata":metadata});
        if serde_json::to_vec(&result).map_err(|_| limit())?.len() <= window.budget {
            return Ok(result);
        }
        if window.accepted <= 1 {
            return Err(limit());
        }
        let file = window.files.last_mut().ok_or_else(limit)?;
        let hunks = file["hunks"].as_array_mut().ok_or_else(limit)?;
        if let Some(hunk) = hunks.last_mut() {
            let lines = hunk["lines"].as_array_mut().ok_or_else(limit)?;
            lines.pop();
            if lines.is_empty() {
                hunks.pop();
            }
        }
        if hunks.is_empty() {
            window.files.pop();
        }
        window.accepted -= 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[allow(clippy::too_many_arguments)]
    fn read(
        repository: RepoRef,
        commit: &str,
        path: WirePath,
        parent: usize,
        context: u32,
        count: usize,
        budget: Option<usize>,
        cursor: Option<String>,
    ) -> Result<Value, Error> {
        read_cached(
            &mut Cache::default(),
            false,
            repository,
            commit,
            path,
            parent,
            context,
            count,
            budget,
            cursor,
        )
    }

    fn fixture(bytes: &[u8]) -> (tempfile::TempDir, RepoRef, String) {
        let temp = tempfile::tempdir().unwrap();
        let repo = Repository::init(temp.path()).unwrap();
        fs::write(temp.path().join("file.txt"), bytes).unwrap();
        super::super::tests::commit(&repo, "content");
        let metadata = fs::metadata(repo.path()).unwrap();
        let reference = RepoRef::new(repo.path(), metadata.dev(), metadata.ino());
        let commit = repo.head().unwrap().target().unwrap().to_string();
        (temp, reference, commit)
    }

    #[test]
    fn compact_rows_preserve_pages_and_ids_across_cold_and_warm_reads() {
        let (temp, reference, commit) = fixture(&b"original\n".repeat(40));
        fs::write(temp.path().join("file.txt"), b"changed\n".repeat(40)).unwrap();
        let source = SnapshotRef {
            r: reference.clone(),
            q: "status".into(),
            f: fingerprint(&reference.open().unwrap()).unwrap(),
            p: None,
        };
        let entry = EntryRef::new(&[b"file.txt".to_vec()]).encode();
        fn expand(mut page: Value) -> Value {
            for file in page["entries"].as_array_mut().unwrap() {
                for hunk in file["hunks"].as_array_mut().unwrap() {
                    for line in hunk["lines"].as_array_mut().unwrap() {
                        let row = line.as_array().unwrap();
                        let keys = [
                            "lineIndex",
                            "byteOffset",
                            "lineComplete",
                            "origin",
                            "oldLine",
                            "newLine",
                            "contentBytesB64",
                            "id",
                        ];
                        *line = Value::Object(
                            keys.iter()
                                .zip(row)
                                .map(|(k, v)| ((*k).into(), v.clone()))
                                .collect(),
                        );
                    }
                }
            }
            page
        }
        for working in [false, true] {
            let mut cache = Cache::default();
            let mut cursor = None;
            loop {
                let mut request = |compact, cold| {
                    let mut empty = Cache::default();
                    let cache = if cold { &mut empty } else { &mut cache };
                    if working {
                        read_working_cached(
                            cache,
                            compact,
                            reference.clone(),
                            source.clone(),
                            &entry,
                            Side::IndexToWorktree,
                            3,
                            7,
                            Some(65536),
                            cursor.clone(),
                        )
                        .unwrap()
                    } else {
                        read_cached(
                            cache,
                            compact,
                            reference.clone(),
                            &commit,
                            WirePath::new(b"file.txt"),
                            0,
                            3,
                            7,
                            Some(65536),
                            cursor.clone(),
                        )
                        .unwrap()
                    }
                };
                let original = request(false, false);
                let compact = request(true, false);
                assert_eq!(expand(compact.clone()), original);
                assert_eq!(request(true, true), compact);
                assert!(
                    serde_json::to_vec(&compact).unwrap().len()
                        < serde_json::to_vec(&original).unwrap().len()
                );
                cursor = original["nextCursor"].as_str().map(str::to_owned);
                if cursor.is_none() {
                    break;
                }
            }
        }
    }

    #[test]
    fn working_pages_preserve_raw_lines_and_mutation_ids_for_every_side() {
        let (temp, reference, _) = fixture(b"original\n");
        let repo = reference.open().unwrap();
        let file = temp.path().join("file.txt");
        fs::write(&file, "staged\n".repeat(200)).unwrap();
        let mut index = repo.index().unwrap();
        index.add_path(Path::new("file.txt")).unwrap();
        index.write().unwrap();
        let mut content = vec![b'x'; 9000];
        content.extend_from_slice(b"\nlast line without newline");
        fs::write(&file, &content).unwrap();
        let source = SnapshotRef {
            r: reference.clone(),
            q: "status".into(),
            f: fingerprint(&repo).unwrap(),
            p: None,
        };
        let entry = EntryRef::new(&[b"file.txt".to_vec()]).encode();
        let index_before = fs::read(repo.path().join("index")).unwrap();
        for side in [
            Side::HeadToIndex,
            Side::IndexToWorktree,
            Side::HeadToWorktree,
        ] {
            let legacy = working_comparison(&reference, &source, &entry, side.clone(), 3, |diff| {
                diff_value(diff, None)
            })
            .unwrap();
            let mut cache = Cache::default();
            let mut cursor = None;
            let mut pieces = Vec::new();
            let mut snapshot = None;
            loop {
                let read = |cache: &mut Cache| {
                    read_working_cached(
                        cache,
                        false,
                        reference.clone(),
                        source.clone(),
                        &entry,
                        side.clone(),
                        3,
                        7,
                        Some(65536),
                        cursor.clone(),
                    )
                    .unwrap()
                };
                let page = read(&mut cache);
                assert_eq!(
                    page,
                    read(&mut Cache::default()),
                    "warm and recomputed pages differ"
                );
                assert!(serde_json::to_vec(&page).unwrap().len() <= 65536);
                assert_eq!(page["metadata"]["totalFiles"], 1);
                assert_eq!(page["metadata"]["sourceSnapshot"], source.encode());
                if let Some(previous) = &snapshot {
                    assert_eq!(previous, &page["snapshot"]);
                }
                snapshot = Some(page["snapshot"].clone());
                for file in page["entries"].as_array().unwrap() {
                    for hunk in file["hunks"].as_array().unwrap() {
                        let original =
                            &legacy["files"][0]["hunks"][hunk["index"].as_u64().unwrap() as usize];
                        assert_eq!(hunk["id"], original["id"]);
                        assert_eq!(
                            hunk["totalLines"].as_u64().unwrap() as usize,
                            original["lines"].as_array().unwrap().len()
                        );
                        for line in hunk["lines"].as_array().unwrap() {
                            pieces.push((hunk["index"].as_u64().unwrap() as usize, line.clone()));
                        }
                    }
                }
                cursor = page["nextCursor"].as_str().map(str::to_owned);
                if cursor.is_none() {
                    assert_eq!(
                        pieces.len(),
                        page["metadata"]["totalUnits"].as_u64().unwrap() as usize
                    );
                    break;
                }
            }
            let mut raw = Vec::new();
            let mut completed = 0;
            for (h, piece) in pieces {
                assert_eq!(raw.len(), piece["byteOffset"].as_u64().unwrap() as usize);
                raw.extend(
                    STANDARD
                        .decode(piece["contentBytesB64"].as_str().unwrap())
                        .unwrap(),
                );
                if piece["lineComplete"] == true {
                    let original = &legacy["files"][0]["hunks"][h]["lines"]
                        [piece["lineIndex"].as_u64().unwrap() as usize];
                    assert_eq!(
                        raw,
                        STANDARD
                            .decode(original["content"]["bytesB64"].as_str().unwrap())
                            .unwrap()
                    );
                    assert_eq!(piece["id"], original["id"]);
                    raw.clear();
                    completed += 1;
                } else {
                    assert!(piece["id"].is_null());
                }
            }
            assert!(raw.is_empty());
            assert_eq!(
                completed,
                legacy["files"][0]["hunks"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|h| h["lines"].as_array().unwrap().len())
                    .sum::<usize>()
            );
        }
        assert_eq!(fs::read(repo.path().join("index")).unwrap(), index_before);
        assert_eq!(fs::read(file).unwrap(), content);
    }

    #[test]
    fn working_cursor_binds_selection_and_frozen_cache_never_authorizes_writes() {
        let (temp, reference, _) = fixture(b"original\n");
        let repo = reference.open().unwrap();
        fs::write(temp.path().join("file.txt"), "changed\n".repeat(100)).unwrap();
        let source = SnapshotRef {
            r: reference.clone(),
            q: "status".into(),
            f: fingerprint(&repo).unwrap(),
            p: None,
        };
        let entry = EntryRef::new(&[b"file.txt".to_vec()]).encode();
        let mut cache = Cache::default();
        let first = read_working_cached(
            &mut cache,
            false,
            reference.clone(),
            source.clone(),
            &entry,
            Side::IndexToWorktree,
            3,
            2,
            None,
            None,
        )
        .unwrap();
        let cursor = first["nextCursor"].as_str().unwrap().to_string();
        let read = |cache: &mut Cache, side, context| {
            read_working_cached(
                cache,
                false,
                reference.clone(),
                source.clone(),
                &entry,
                side,
                context,
                2,
                None,
                Some(cursor.clone()),
            )
        };
        assert_eq!(
            read(&mut cache, Side::HeadToIndex, 3).unwrap_err().code,
            "INVALID_REQUEST"
        );
        assert_eq!(
            read(&mut cache, Side::IndexToWorktree, 4).unwrap_err().code,
            "INVALID_REQUEST"
        );
        let second = read(&mut cache, Side::IndexToWorktree, 3).unwrap();
        fs::write(temp.path().join("file.txt"), "newer edit\n").unwrap();
        assert_eq!(read(&mut cache, Side::IndexToWorktree, 3).unwrap(), second);
        assert_eq!(
            read(&mut Cache::default(), Side::IndexToWorktree, 3)
                .unwrap_err()
                .code,
            "STALE_SNAPSHOT"
        );
        assert_eq!(
            working_comparison(
                &reference,
                &source,
                &entry,
                Side::IndexToWorktree,
                3,
                |_| panic!("stale source accepted")
            )
            .unwrap_err()
            .code,
            "STALE_SNAPSHOT"
        );
        let page_snapshot = SnapshotRef::decode(first["snapshot"].as_str().unwrap()).unwrap();
        assert_eq!(
            working_comparison(
                &reference,
                &page_snapshot,
                &entry,
                Side::IndexToWorktree,
                3,
                |_| panic!("page token accepted as status")
            )
            .unwrap_err()
            .code,
            "INVALID_REQUEST"
        );
    }

    #[test]
    fn pages_reconstruct_long_lines_and_respect_total_wire_budget() {
        // Split a multi-byte character at the 4096-byte piece boundary and
        // include non-UTF8 text: consumers must concatenate before decoding.
        let mut source = vec![b'a'; 4095];
        source.extend_from_slice("界".as_bytes());
        source.push(0xff);
        source.extend(vec![b'b'; 150_000]);
        source.push(b'\n');
        for _ in 0..2000 {
            source.extend_from_slice(b"short line\n");
        }
        let (_temp, repo, commit) = fixture(&source);
        let mut cache = Cache::default();
        let mut cursor = None;
        let mut reconstructed = Vec::new();
        let mut expected_line = 0;
        let mut expected_offset = 0;
        let mut pages = 0;
        let mut snapshot = None;
        loop {
            let page = read_cached(
                &mut cache,
                false,
                repo.clone(),
                &commit,
                WirePath::new(b"file.txt"),
                0,
                3,
                5000,
                Some(65536),
                cursor.clone(),
            )
            .unwrap();
            let cold = read(
                repo.clone(),
                &commit,
                WirePath::new(b"file.txt"),
                0,
                3,
                5000,
                Some(65536),
                cursor.clone(),
            )
            .unwrap();
            assert_eq!(page, cold, "warm and reconnected pages must be identical");
            assert!(serde_json::to_vec(&page).unwrap().len() <= 65536);
            assert_eq!(page["metadata"]["readOnly"], true);
            assert_eq!(page["metadata"]["hasOmissions"], false);
            if let Some(ref previous) = snapshot {
                assert_eq!(previous, &page["snapshot"]);
            }
            snapshot = Some(page["snapshot"].clone());
            for file in page["entries"].as_array().unwrap() {
                for hunk in file["hunks"].as_array().unwrap() {
                    for line in hunk["lines"].as_array().unwrap() {
                        assert_eq!(line["origin"], "+");
                        assert_eq!(line["lineIndex"], expected_line);
                        assert_eq!(line["byteOffset"], expected_offset);
                        assert!(line.get("id").is_none());
                        let bytes = STANDARD
                            .decode(line["contentBytesB64"].as_str().unwrap())
                            .unwrap();
                        expected_offset += bytes.len();
                        reconstructed.extend(bytes);
                        if line["lineComplete"] == true {
                            expected_line += 1;
                            expected_offset = 0;
                        }
                    }
                }
            }
            pages += 1;
            assert!(pages < 100);
            cursor = page["nextCursor"].as_str().map(str::to_owned);
            if cursor.is_none() {
                break;
            }
        }
        assert!(pages > 2);
        assert_eq!(expected_offset, 0);
        assert_eq!(reconstructed, source);
    }

    #[test]
    fn warm_continuation_reads_capture_without_reopening_commit_objects() {
        let (_temp, repo, commit) = fixture(b"first\nsecond\nthird\n");
        let mut cache = Cache::default();
        let first = read_cached(
            &mut cache,
            false,
            repo.clone(),
            &commit,
            WirePath::new(b"file.txt"),
            0,
            3,
            1,
            None,
            None,
        )
        .unwrap();
        let cursor = first["nextCursor"].as_str().unwrap().to_owned();
        let expected = read(
            repo.clone(),
            &commit,
            WirePath::new(b"file.txt"),
            0,
            3,
            1,
            None,
            Some(cursor.clone()),
        )
        .unwrap();
        let repository = repo.open().unwrap();
        // Only this disposable fixture is modified. A warm capture must remain
        // readable without loading a commit or regenerating its comparison.
        fs::remove_file(
            repository
                .path()
                .join("objects")
                .join(&commit[..2])
                .join(&commit[2..]),
        )
        .unwrap();
        let warm = read_cached(
            &mut cache,
            false,
            repo.clone(),
            &commit,
            WirePath::new(b"file.txt"),
            0,
            3,
            1,
            None,
            Some(cursor.clone()),
        )
        .unwrap();
        assert_eq!(warm, expected);
        assert_eq!(
            read(
                repo,
                &commit,
                WirePath::new(b"file.txt"),
                0,
                3,
                1,
                None,
                Some(cursor)
            )
            .unwrap_err()
            .code,
            "COMMIT_NOT_FOUND"
        );
    }

    #[test]
    fn binary_and_oversized_files_have_explicit_omissions() {
        for (source, reason) in [
            (vec![0, 1, 2], "binary"),
            (vec![b'x'; TEXT_LIMIT as usize + 1], "file_size_limit"),
        ] {
            let (_temp, repo, commit) = fixture(&source);
            let page = read(
                repo,
                &commit,
                WirePath::new(b"file.txt"),
                0,
                3,
                1,
                None,
                None,
            )
            .unwrap();
            assert_eq!(page["metadata"]["hasOmissions"], true);
            assert_eq!(page["entries"][0]["omissionReason"], reason);
            assert_eq!(page["entries"][0]["hunks"], json!([]));
            assert_eq!(page["nextCursor"], Value::Null);
        }
    }

    #[test]
    fn explicitly_text_large_files_do_not_report_omitted_content() {
        let temp = tempfile::tempdir().unwrap();
        let repo = Repository::init(temp.path()).unwrap();
        fs::write(temp.path().join(".gitattributes"), "file.txt diff\n").unwrap();
        let bytes = vec![b'x'; TEXT_LIMIT as usize + 1];
        fs::write(temp.path().join("file.txt"), &bytes).unwrap();
        super::super::tests::commit(&repo, "large text");
        let metadata = fs::metadata(repo.path()).unwrap();
        let reference = RepoRef::new(repo.path(), metadata.dev(), metadata.ino());
        let commit = repo.head().unwrap().target().unwrap().to_string();
        fn walk(mut request: impl FnMut(Option<String>) -> Value) -> (Vec<u8>, Vec<u8>) {
            let (mut added, mut removed) = (Vec::new(), Vec::new());
            let mut cursor = None;
            let mut pages = 0;
            loop {
                let page = request(cursor.clone());
                assert!(serde_json::to_vec(&page).unwrap().len() <= 65536);
                assert_eq!(page["metadata"]["hasOmissions"], false);
                for file in page["entries"].as_array().unwrap() {
                    assert_eq!(file["omissionReason"], Value::Null);
                    for hunk in file["hunks"].as_array().unwrap() {
                        for line in hunk["lines"].as_array().unwrap() {
                            let bytes = STANDARD
                                .decode(line["contentBytesB64"].as_str().unwrap())
                                .unwrap();
                            match line["origin"].as_str().unwrap() {
                                "+" => added.extend(bytes),
                                "-" => removed.extend(bytes),
                                _ => {}
                            }
                        }
                    }
                }
                pages += 1;
                let next = page["nextCursor"].as_str().map(str::to_owned);
                if next.is_none() {
                    break;
                }
                assert_ne!(next, cursor);
                cursor = next;
            }
            assert!(pages > 1);
            (added, removed)
        }
        let mut cache = Cache::default();
        let (added, removed) = walk(|cursor| {
            read_cached(
                &mut cache,
                false,
                reference.clone(),
                &commit,
                WirePath::new(b"file.txt"),
                0,
                3,
                5000,
                Some(65536),
                cursor,
            )
            .unwrap()
        });
        assert_eq!(added, bytes);
        assert!(removed.is_empty());
        fs::write(temp.path().join("file.txt"), vec![b'y'; bytes.len()]).unwrap();
        let source = SnapshotRef {
            r: reference.clone(),
            q: "status".into(),
            f: fingerprint(&repo).unwrap(),
            p: None,
        };
        let entry = EntryRef::new(&[b"file.txt".to_vec()]).encode();
        let mut cache = Cache::default();
        let (added, removed) = walk(|cursor| {
            read_working_cached(
                &mut cache,
                false,
                reference.clone(),
                source.clone(),
                &entry,
                Side::IndexToWorktree,
                3,
                5000,
                Some(65536),
                cursor,
            )
            .unwrap()
        });
        assert_eq!(added, vec![b'y'; bytes.len()]);
        assert_eq!(removed, bytes);
    }

    #[test]
    fn missing_final_newline_preserves_git_line_markers() {
        let (_temp, repo, commit) = fixture(b"unterminated");
        let repository = repo.open().unwrap();
        let legacy = commit_diff(&repository, &commit, 0, 3, Some(b"file.txt")).unwrap();
        let page = read(
            repo,
            &commit,
            WirePath::new(b"file.txt"),
            0,
            3,
            5000,
            None,
            None,
        )
        .unwrap();
        let expected = legacy["files"][0]["hunks"][0]["lines"].as_array().unwrap();
        let actual = page["entries"][0]["hunks"][0]["lines"].as_array().unwrap();
        assert_eq!(expected.len(), actual.len());
        for (old, new) in expected.iter().zip(actual) {
            assert_eq!(old["origin"], new["origin"]);
            assert_eq!(old["oldLine"], new["oldLine"]);
            assert_eq!(old["newLine"], new["newLine"]);
        }
        assert_eq!(
            STANDARD
                .decode(actual[0]["contentBytesB64"].as_str().unwrap())
                .unwrap(),
            b"unterminated"
        );
    }

    #[test]
    fn cursors_bind_selection_and_reject_tampered_offset() {
        let (_temp, repo, commit) = fixture(b"first\nsecond\nthird\n");
        let first = read(
            repo.clone(),
            &commit,
            WirePath::new(b"file.txt"),
            0,
            3,
            1,
            None,
            None,
        )
        .unwrap();
        let cursor = first["nextCursor"].as_str().unwrap().to_owned();
        for (path, context) in [(b"other".as_slice(), 3), (b"file.txt".as_slice(), 0)] {
            assert!(read(
                repo.clone(),
                &commit,
                WirePath::new(path),
                0,
                context,
                1,
                None,
                Some(cursor.clone())
            )
            .is_err());
        }
        let mut forged = CursorRef::decode(&cursor).unwrap();
        forged.o = 1000;
        assert!(read(
            repo,
            &commit,
            WirePath::new(b"file.txt"),
            0,
            3,
            1,
            None,
            Some(forged.encode())
        )
        .is_err());
    }
}
