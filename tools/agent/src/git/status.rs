//! Full working-tree snapshots with lazy row serialization and bounded caches.
//! Cache overflow falls back to re-reading, never truncating repository status.
use super::*;

/// Display-only counts: no changed-file rows, cache capture or write snapshot.
pub(super) fn summary(repo: &Repository) -> Result<Value, Error> {
    if repo.is_bare() {
        return Err(Error::new(
            "UNSUPPORTED_CAPABILITY",
            "Bare repositories have no working tree.",
        ));
    }
    let scan = statuses(repo)?;
    metadata(repo, &scan)
}

pub(super) fn query(
    filter: Option<&super::super::protocol::StatusFilter>,
) -> Result<String, Error> {
    let Some(filter) = filter else {
        return Ok("status".into());
    };
    if filter.text.len() > 1024 || filter.text.contains('\0') {
        return Err(Error::invalid("Invalid changed-file filter."));
    }
    let group = filter.group.as_deref().unwrap_or("all");
    if !matches!(
        group,
        "all" | "staged" | "unstaged" | "untracked" | "conflicted"
    ) {
        return Err(Error::invalid("Invalid changed-file group."));
    }
    let text = filter.text.trim().to_lowercase();
    if text.is_empty() && group == "all" {
        return Ok("status".into());
    }
    Ok(format!("status:{}", json!([text, group])))
}
struct Filter {
    text: String,
    group: String,
}
impl Filter {
    fn all(&self) -> bool {
        self.text.is_empty() && self.group == "all"
    }
    fn matches(&self, entry: &Entry) -> bool {
        let flags = entry.flags;
        let group = match self.group.as_str() {
            "all" => true,
            "conflicted" => flags.is_conflicted(),
            "untracked" => !flags.is_conflicted() && flags.is_wt_new(),
            "staged" => {
                !flags.is_conflicted()
                    && flags.intersects(
                        git2::Status::INDEX_NEW
                            | git2::Status::INDEX_MODIFIED
                            | git2::Status::INDEX_DELETED
                            | git2::Status::INDEX_RENAMED
                            | git2::Status::INDEX_TYPECHANGE,
                    )
            }
            "unstaged" => {
                !flags.is_conflicted()
                    && !flags.is_wt_new()
                    && flags.intersects(
                        git2::Status::WT_MODIFIED
                            | git2::Status::WT_DELETED
                            | git2::Status::WT_RENAMED
                            | git2::Status::WT_TYPECHANGE,
                    )
            }
            _ => false,
        };
        group
            && (self.text.is_empty()
                || format!(
                    "{} {}",
                    String::from_utf8_lossy(entry.path.as_deref().unwrap_or_default()),
                    String::from_utf8_lossy(entry.old_path.as_deref().unwrap_or_default())
                )
                .to_lowercase()
                .contains(&self.text))
    }
}

#[cfg(not(test))]
const INDEX_BYTES: usize = 8 * 1024 * 1024;
#[cfg(test)]
const INDEX_BYTES: usize = 32 * 1024;
#[derive(Default)]
pub(super) struct Cache(Vec<(String, Arc<Capture>)>);
struct Capture {
    entries: Vec<Entry>,
    metadata: Value,
    bytes: usize,
}
struct Entry {
    paths: Vec<Vec<u8>>,
    path: Option<Vec<u8>>,
    old_path: Option<Vec<u8>>,
    flags: git2::Status,
    conflict: Option<Value>,
}
impl Entry {
    fn capture(entry: &git2::StatusEntry<'_>, index: &git2::Index) -> Self {
        let paths = entry_paths(entry);
        let flags = entry.status();
        let conflict = if flags.is_conflicted() {
            paths.iter().find_map(|path|index.conflict_get(Path::new(OsStr::from_bytes(path))).ok()).map(|c|{
                let side=|entry: Option<git2::IndexEntry>|entry.map(|e|json!({"oid":oid(e.id),"path":WirePath::new(&e.path),"mode":e.mode}));
                json!({"base":side(c.ancestor),"ours":side(c.our),"theirs":side(c.their)})
            })
        } else {
            None
        };
        Self {
            paths,
            path: entry
                .index_to_workdir()
                .or_else(|| entry.head_to_index())
                .and_then(|d| d.new_file().path())
                .map(|p| p.as_os_str().as_bytes().to_vec()),
            old_path: entry
                .head_to_index()
                .and_then(|d| d.old_file().path())
                .map(|p| p.as_os_str().as_bytes().to_vec()),
            flags,
            conflict,
        }
    }
    fn bytes(&self) -> usize {
        // Account for vector growth, path allocations and JSON tree overhead.
        2 * std::mem::size_of::<Self>()
            + self.paths.capacity() * std::mem::size_of::<Vec<u8>>()
            + self.paths.iter().map(Vec::capacity).sum::<usize>()
            + self.path.as_ref().map_or(0, Vec::capacity)
            + self.old_path.as_ref().map_or(0, Vec::capacity)
            + self
                .conflict
                .as_ref()
                .map_or(0, |v| serde_json::to_vec(v).map_or(0, |b| b.len() * 8))
    }
    fn json(&self) -> Value {
        let flags = self.flags;
        json!({"entryId":EntryRef::new(&self.paths).encode(),"path":self.path.as_deref().map(WirePath::new),"flags":flags.bits(),"staged":flags.intersects(git2::Status::INDEX_NEW|git2::Status::INDEX_MODIFIED|git2::Status::INDEX_DELETED|git2::Status::INDEX_RENAMED|git2::Status::INDEX_TYPECHANGE),"unstaged":flags.intersects(git2::Status::WT_MODIFIED|git2::Status::WT_DELETED|git2::Status::WT_RENAMED|git2::Status::WT_TYPECHANGE),"untracked":flags.is_wt_new(),"conflicted":flags.is_conflicted(),"conflict":self.conflict,"oldPath":self.old_path.as_deref().map(WirePath::new)})
    }
}
fn metadata(repo: &Repository, scan: &StatusScan<'_>) -> Result<Value, Error> {
    let (mut staged, mut unstaged, mut untracked, mut conflicted) = (0, 0, 0, 0);
    for entry in scan.iter() {
        let flags = entry.status();
        if flags.is_conflicted() {
            conflicted += 1;
            continue;
        }
        if flags.intersects(
            git2::Status::INDEX_NEW
                | git2::Status::INDEX_MODIFIED
                | git2::Status::INDEX_DELETED
                | git2::Status::INDEX_RENAMED
                | git2::Status::INDEX_TYPECHANGE,
        ) {
            staged += 1;
        }
        if flags.is_wt_new() {
            untracked += 1;
        } else if flags.intersects(
            git2::Status::WT_MODIFIED
                | git2::Status::WT_DELETED
                | git2::Status::WT_RENAMED
                | git2::Status::WT_TYPECHANGE,
        ) {
            unstaged += 1;
        }
    }
    let total = scan.total;
    let tracking = repo
        .head()
        .ok()
        .filter(|h| h.is_branch())
        .and_then(|h| {
            h.shorthand()
                .ok()
                .and_then(|s| repo.find_branch(s, git2::BranchType::Local).ok())
        })
        .and_then(|b| b.upstream().ok());
    let upstream = tracking.as_ref().and_then(|b| b.get().target());
    let counts = repo
        .head()
        .ok()
        .and_then(|h| h.target())
        .zip(upstream)
        .and_then(|(a, b)| repo.graph_ahead_behind(a, b).ok());
    Ok(
        json!({"head":head(repo)?,"operationState":format!("{:?}",repo.state()),"integration":super::super::integration::status(repo),"ahead":counts.map(|c|c.0),"behind":counts.map(|c|c.1),"basis":"stored_refs","upstreamRef":tracking.as_ref().map(|b|WirePath::new(b.get().name_bytes())),"truncated":false,"totalEntries":total,"groupCounts":{"staged":staged,"unstaged":unstaged,"untracked":untracked,"conflicted":conflicted}}),
    )
}
fn add(rows: &mut Vec<Value>, bytes: &mut usize, entry: &Entry) -> Result<bool, Error> {
    let row = entry.json();
    let size = serde_json::to_vec(&row).map_err(|_| limit())?.len();
    if size > MAX_FRAME / 2 {
        return Err(limit());
    }
    if *bytes + size > MAX_FRAME / 2 {
        return Ok(false);
    }
    *bytes += size;
    rows.push(row);
    Ok(true)
}
fn response(
    snapshot: SnapshotRef,
    query: &str,
    rows: Vec<Value>,
    offset: usize,
    total: usize,
    metadata: &Value,
) -> Value {
    let token = snapshot.encode();
    let end = offset + rows.len();
    let next = (end < total).then(|| {
        let mut snapshot = snapshot;
        snapshot.q = query.into();
        CursorRef {
            k: None,
            s: snapshot,
            o: end,
        }
        .encode()
    });
    let mut metadata = metadata.clone();
    metadata["matchedEntries"] = json!(total);
    json!({"snapshot":token,"entries":rows,"nextCursor":next,"metadata":metadata})
}
pub(super) fn page(
    cache: &mut Cache,
    repository: RepoRef,
    query: String,
    count: usize,
    snapshot: Option<SnapshotRef>,
    offset: usize,
) -> Result<Value, Error> {
    let (text, group) = if query == "status" {
        (String::new(), "all".into())
    } else {
        serde_json::from_str(
            query
                .strip_prefix("status:")
                .ok_or_else(|| Error::invalid("Invalid status query."))?,
        )
        .map_err(|_| Error::invalid("Invalid status query."))?
    };
    let filter = Filter { text, group };
    // Writes always use the full-repository status snapshot. Only cursors
    // carry the filter identity; cached captures are shared across filters.
    let snapshot = snapshot.map(|mut s| {
        s.q = "status".into();
        s
    });
    let repo = repository.open()?;
    if repo.is_bare() {
        return Err(Error::new(
            "UNSUPPORTED_CAPABILITY",
            "Bare repositories have no working tree.",
        ));
    }
    if let Some(snapshot) = &snapshot {
        let token = snapshot.encode();
        if let Some(position) = cache.0.iter().position(|(key, _)| key == &token) {
            let cached = cache.0.remove(position);
            let captured = cached.1.clone();
            cache.0.push(cached);
            let total = captured
                .entries
                .iter()
                .filter(|entry| filter.matches(entry))
                .count();
            if offset > total {
                return Err(Error::invalid("Cursor is past the end of this listing."));
            }
            let mut rows = Vec::new();
            let mut bytes = 0;
            for entry in captured
                .entries
                .iter()
                .filter(|entry| filter.matches(entry))
                .skip(offset)
                .take(count)
            {
                if !add(&mut rows, &mut bytes, entry)? {
                    break;
                }
            }
            return Ok(response(
                snapshot.clone(),
                &query,
                rows,
                offset,
                total,
                &captured.metadata,
            ));
        }
    }
    let scan = statuses(&repo)?;
    let fingerprint = fingerprint_scan(&repo, &repo.path().join("index"), Some(&scan))?;
    if snapshot.as_ref().is_some_and(|s| s.f != fingerprint) {
        return Err(Error::new(
            "SNAPSHOT_EXPIRED",
            "Working tree changed. Restart status from the first page.",
        ));
    }
    if offset > scan.total {
        return Err(Error::invalid("Cursor is past the end of this listing."));
    }
    let index = repo.index().map_err(engine)?;
    let metadata = metadata(&repo, &scan)?;
    let mut captured = Some(Capture {
        entries: Vec::new(),
        metadata: metadata.clone(),
        bytes: serde_json::to_vec(&metadata).map_err(|_| limit())?.len() * 8,
    });
    let mut rows = Vec::new();
    let mut bytes = 0;
    let mut matched = 0;
    for (position, entry) in scan.iter().enumerate() {
        // Beyond the cache budget, materialize only requested rows.
        let selected = position == offset + rows.len() && rows.len() < count;
        if captured.is_none() && filter.all() && !selected {
            matched += 1;
            continue;
        }
        let entry = Entry::capture(&entry, &index);
        if filter.matches(&entry) {
            if matched == offset + rows.len() && rows.len() < count {
                add(&mut rows, &mut bytes, &entry)?;
            }
            matched += 1;
        }
        if let Some(captured) = &mut captured {
            captured.bytes += entry.bytes();
            captured.entries.push(entry);
        }
        if captured.as_ref().is_some_and(|c| c.bytes > INDEX_BYTES) {
            captured = None;
        }
    }
    if fingerprint_scan(&repo, &repo.path().join("index"), None)? != fingerprint {
        return Err(Error::new(
            "REPOSITORY_BUSY",
            "Working tree changed while reading status.",
        ));
    }
    let snapshot = snapshot.unwrap_or(SnapshotRef {
        r: repository,
        q: "status".into(),
        f: fingerprint,
        p: None,
    });
    if let Some(mut captured) = captured {
        captured.entries.shrink_to_fit();
        let token = snapshot.encode();
        captured.bytes += token.len();
        cache.0.retain(|(key, _)| key != &token);
        cache.0.push((token, Arc::new(captured)));
        while cache.0.len() > CACHE_ENTRIES
            || cache.0.iter().map(|(_, c)| c.bytes).sum::<usize>() > CACHE_BYTES
        {
            cache.0.remove(0);
        }
    }
    if offset > matched {
        return Err(Error::invalid("Cursor is past the end of this listing."));
    }
    Ok(response(snapshot, &query, rows, offset, matched, &metadata))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn summary_matches_counts_without_rows_or_write_tokens() {
        let temp = tempfile::tempdir().unwrap();
        let repo = Repository::init(temp.path()).unwrap();
        for n in 0..250 {
            fs::write(temp.path().join(format!("file-{n}")), "data").unwrap();
        }
        let mut service = Service::default();
        let id = opened(&mut service, temp.path());
        let before = fingerprint(&repo).unwrap();
        let Output::Json(summary) = service
            .request(Request::StatusSummary {
                repo_id: Some(id.clone()),
                path: None,
            })
            .unwrap()
        else {
            panic!()
        };
        let full = request(&mut service, &id, None).unwrap();
        for field in [
            "head",
            "totalEntries",
            "groupCounts",
            "ahead",
            "behind",
            "integration",
        ] {
            assert_eq!(summary[field], full["metadata"][field]);
        }
        fs::create_dir(temp.path().join("nested")).unwrap();
        let Output::Json(by_path) = service
            .request(Request::StatusSummary {
                repo_id: None,
                path: Some(wire_path(&temp.path().join("nested"))),
            })
            .unwrap()
        else {
            panic!()
        };
        assert_eq!(summary, by_path);
        for (repo_id, path) in [
            (None, None),
            (Some(id.clone()), Some(wire_path(temp.path()))),
            (None, Some(WirePath::new(b"relative"))),
        ] {
            assert_eq!(
                service
                    .request(Request::StatusSummary { repo_id, path })
                    .err()
                    .unwrap()
                    .code,
                "INVALID_REQUEST"
            );
        }
        assert_eq!(summary["totalEntries"], 250);
        assert!(summary.get("entries").is_none());
        assert!(summary.get("snapshot").is_none());
        assert!(summary.get("nextCursor").is_none());
        assert_eq!(fingerprint(&repo).unwrap(), before);
    }

    #[test]
    fn filtered_pages_keep_global_snapshots_and_bind_cursors_to_the_query() {
        for count in [20, 200] {
            let temp = tempfile::tempdir().unwrap();
            Repository::init(temp.path()).unwrap();
            let mut expected = Vec::new();
            for n in 0..count {
                let name = format!("{n:04}-{}.txt", if n % 4 == 0 { "keep" } else { "skip" });
                fs::write(temp.path().join(&name), "data").unwrap();
                if n % 4 == 0 {
                    expected.push(name);
                }
            }
            let mut service = Service::default();
            let id = opened(&mut service, temp.path());
            let full = request(&mut service, &id, None).unwrap();
            let read = |service: &mut Service, text: &str, cursor: Option<String>| {
                let Output::Json(value) = service.request(Request::Status {
                    repo_id: id.clone(),
                    page_size: 3,
                    cursor,
                    filter: Some(super::super::super::protocol::StatusFilter {
                        text: text.into(),
                        group: Some("untracked".into()),
                    }),
                })?
                else {
                    panic!()
                };
                Ok::<_, Error>(value)
            };
            let first = read(&mut service, " KEEP ", None).unwrap();
            assert_eq!(first["snapshot"], full["snapshot"]);
            assert_eq!(first["metadata"]["totalEntries"], count);
            assert_eq!(first["metadata"]["matchedEntries"], expected.len());
            let cursor = first["nextCursor"].as_str().unwrap().to_owned();
            assert_eq!(
                read(&mut service, "skip", Some(cursor.clone()))
                    .unwrap_err()
                    .code,
                "INVALID_REQUEST"
            );
            let mut collected = Vec::new();
            let mut page = first;
            loop {
                assert_eq!(page["snapshot"], full["snapshot"]);
                collected.extend(
                    page["entries"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|e| e["path"]["display"].as_str().unwrap().to_owned()),
                );
                let Some(cursor) = page["nextCursor"].as_str() else {
                    break;
                };
                // A reconnect drops every cache; pagination must still work.
                service = Service::default();
                page = read(&mut service, "keep", Some(cursor.into())).unwrap();
            }
            assert_eq!(collected, expected);
            assert_eq!(
                read(&mut service, "no-match", None).unwrap()["nextCursor"],
                Value::Null
            );
            fs::write(temp.path().join("unrelated"), "changed").unwrap();
            assert_eq!(
                read(&mut Service::default(), "keep", Some(cursor))
                    .unwrap_err()
                    .code,
                "SNAPSHOT_EXPIRED"
            );
        }
    }

    fn opened(service: &mut Service, path: &Path) -> String {
        let Output::Json(v) = service
            .request(Request::Open {
                path: wire_path(path),
            })
            .unwrap()
        else {
            panic!()
        };
        v["repoId"].as_str().unwrap().into()
    }
    fn request(service: &mut Service, id: &str, cursor: Option<String>) -> Result<Value, Error> {
        match service.request(Request::Status {
            filter: None,
            repo_id: id.into(),
            page_size: 2,
            cursor,
        })? {
            Output::Json(v) => Ok(v),
            _ => panic!(),
        }
    }
    #[test]
    fn pages_beyond_ten_thousand_and_late_changes_preserve_write_guards() {
        let dir = tempfile::tempdir().unwrap();
        let repo = Repository::init(dir.path()).unwrap();
        super::super::tests::commit(&repo, "initial");
        for n in 0..10_011 {
            fs::write(dir.path().join(format!("file-{n:05}")), "a").unwrap();
        }
        let mut service = Service::default();
        let id = opened(&mut service, dir.path());
        let first = request(&mut service, &id, None).unwrap();
        assert_eq!(first["metadata"]["totalEntries"], 10_011);
        assert_eq!(first["metadata"]["groupCounts"]["untracked"], 10_011);
        assert_eq!(first["metadata"]["truncated"], false);
        assert!(
            service.status_cache.0.is_empty(),
            "over-budget compact snapshots fall back to scanning"
        );
        let mut cursor = CursorRef::decode(first["nextCursor"].as_str().unwrap()).unwrap();
        cursor.o = 10_000;
        let late = request(&mut Service::default(), &id, Some(cursor.encode())).unwrap();
        assert_eq!(late["entries"][0]["path"]["display"], "file-10000");
        let entry = late["entries"][0]["entryId"].as_str().unwrap();
        assert_eq!(
            Service::entry_paths_now(&repo, entry).unwrap(),
            vec![b"file-10000".to_vec()]
        );
        let old = fingerprint(&repo).unwrap();
        fs::write(dir.path().join("file-10000"), "b").unwrap();
        assert_ne!(
            old,
            fingerprint(&repo).unwrap(),
            "same-size late rewrite invalidates the complete snapshot"
        );
        assert_eq!(
            request(&mut service, &id, Some(cursor.encode()))
                .unwrap_err()
                .code,
            "SNAPSHOT_EXPIRED"
        );
        assert!(
            matches!(service.diff(&id,first["snapshot"].as_str().unwrap(),entry,Side::IndexToWorktree,3),Err(Error{code,..}) if code=="STALE_SNAPSHOT")
        );
    }
    #[test]
    fn frozen_cache_paginates_without_gaps_and_expires_after_eviction() {
        let dir = tempfile::tempdir().unwrap();
        let repo = Repository::init(dir.path()).unwrap();
        super::super::tests::commit(&repo, "initial");
        for n in 0..9 {
            fs::write(dir.path().join(format!("file-{n:02}")), "a").unwrap();
        }
        let mut service = Service::default();
        let id = opened(&mut service, dir.path());
        let first = request(&mut service, &id, None).unwrap();
        let first_cursor = first["nextCursor"].as_str().unwrap().to_owned();
        fs::rename(dir.path().join("file-03"), dir.path().join("renamed")).unwrap();
        let mut names = first["entries"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| e["path"]["display"].as_str().unwrap().to_owned())
            .collect::<Vec<_>>();
        let mut cursor = Some(first_cursor.clone());
        while let Some(next) = cursor {
            let page = request(&mut service, &id, Some(next)).unwrap();
            names.extend(
                page["entries"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|e| e["path"]["display"].as_str().unwrap().to_owned()),
            );
            cursor = page["nextCursor"].as_str().map(str::to_owned);
        }
        assert_eq!(
            names,
            (0..9).map(|n| format!("file-{n:02}")).collect::<Vec<_>>()
        );
        assert_eq!(
            request(&mut Service::default(), &id, Some(first_cursor.clone()))
                .unwrap_err()
                .code,
            "SNAPSHOT_EXPIRED"
        );
        for n in 0..CACHE_ENTRIES {
            fs::write(dir.path().join(format!("new-{n}")), "x").unwrap();
            request(&mut service, &id, None).unwrap();
        }
        assert_eq!(
            request(&mut service, &id, Some(first_cursor))
                .unwrap_err()
                .code,
            "SNAPSHOT_EXPIRED"
        );
    }
    #[test]
    fn group_counts_cover_the_whole_snapshot_not_only_the_page() {
        let dir = tempfile::tempdir().unwrap();
        let repo = Repository::init(dir.path()).unwrap();
        fs::write(dir.path().join("tracked"), "a").unwrap();
        super::super::tests::commit(&repo, "initial");
        fs::write(dir.path().join("tracked"), "b").unwrap();
        let mut index = repo.index().unwrap();
        index.add_path(Path::new("tracked")).unwrap();
        index.write().unwrap();
        fs::write(dir.path().join("tracked"), "c").unwrap();
        for n in 0..3 {
            fs::write(dir.path().join(format!("new-{n}")), "x").unwrap();
        }
        let mut service = Service::default();
        let id = opened(&mut service, dir.path());
        let page = request(&mut service, &id, None).unwrap();
        assert_eq!(page["entries"].as_array().unwrap().len(), 2);
        assert_eq!(
            page["metadata"]["groupCounts"],
            json!({"staged":1,"unstaged":1,"untracked":3,"conflicted":0})
        );
    }
}
