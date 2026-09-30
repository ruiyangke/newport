//! Branch pages hydrate requested rows from bounded compact reference snapshots.
//! Very large snapshots fall back to stateless scans, never listing truncation.
use super::*;
use std::collections::{BTreeSet, HashMap};

#[cfg(not(test))]
const INDEX_BYTES: usize = 8 * 1024 * 1024;
// Exercise the stateless overflow path without allocating an 8 MiB fixture.
#[cfg(test)]
const INDEX_BYTES: usize = 32 * 1024;
#[derive(Default)]
pub(super) struct Cache(Vec<(String, Index)>);
struct Branch {
    reference: Vec<u8>,
    oid: Option<git2::Oid>,
    local: bool,
}
impl Branch {
    fn name(&self) -> &[u8] {
        &self.reference[if self.local { 11 } else { 13 }..]
    }
}
struct Index {
    entries: Vec<Branch>,
    references: BTreeSet<Vec<u8>>,
    current: Option<Vec<u8>>,
    config_hash: String,
    bytes: usize,
}
fn expired() -> Error {
    Error::new(
        "SNAPSHOT_EXPIRED",
        "The branch listing changed. Restart from the first page.",
    )
}
pub(super) fn query(filter: &str, kind: Option<&str>) -> Result<String, Error> {
    if filter.len() > 1024 || filter.contains('\0') {
        return Err(Error::invalid("Invalid branch filter."));
    }
    let kind = kind.unwrap_or("all");
    if !matches!(kind, "all" | "local" | "remote") {
        return Err(Error::invalid("Invalid branchKind."));
    }
    Ok(format!("branches:{}", json!([filter.to_lowercase(), kind])))
}
fn config_hash(config: &git2::Config) -> Result<String, Error> {
    let mut hash = Sha256::new();
    let mut entries = config.entries(None).map_err(engine)?;
    while let Some(entry) = entries.next() {
        let entry = entry.map_err(engine)?;
        if entry.name_bytes().starts_with(b"branch.") || entry.name_bytes().starts_with(b"remote.")
        {
            hash.update(format!("{:?}", entry.level()));
            hash.update(entry.include_depth().to_le_bytes());
            hash.update([u8::from(entry.has_value())]);
            for bytes in [entry.name_bytes(), entry.value_bytes()] {
                hash.update((bytes.len() as u64).to_le_bytes());
                hash.update(bytes);
            }
        }
    }
    Ok(hex(&hash.finalize()))
}
fn row<'r>(
    repo: &'r Repository,
    config: &git2::Config,
    remotes: &mut HashMap<String, Option<git2::Remote<'r>>>,
    branch: &Branch,
    current: Option<&[u8]>,
    references: Option<&BTreeSet<Vec<u8>>>,
) -> Result<Value, Error> {
    let name = std::str::from_utf8(branch.name()).ok();
    let tracking = if branch.local {
        name.map(|n| super::super::branches::tracking_in(config, n))
            .transpose()?
    } else {
        None
    };
    let upstream = match (branch.local, name) {
        (false, _) => None,
        (true, Some(name)) => match references {
            Some(refs) => super::super::branches::upstream_target_in(repo, config, remotes, name)
                .filter(|target| refs.contains(target)),
            None => super::super::branches::upstream_in(repo, config, remotes, name),
        },
        // Invalid UTF-8 cannot be represented as a config key. Such uncommon
        // snapshots are not cached; retain libgit2's original fallback.
        (true, None) => {
            repo.branches(Some(git2::BranchType::Local))
                .ok()
                .and_then(|mut branches| {
                    branches.find_map(|b| {
                        b.ok()
                            .filter(|(b, _)| b.get().name_bytes() == branch.reference)
                            .and_then(|(b, _)| {
                                b.upstream().ok().map(|b| b.get().name_bytes().to_vec())
                            })
                    })
                })
        }
    };
    Ok(
        json!({"tracking":tracking,"name":WirePath::new(branch.name()),"reference":WirePath::new(&branch.reference),"oid":branch.oid.map(oid),"remote":!branch.local,"current":current==Some(branch.reference.as_slice()),"upstream":upstream.map(|b|WirePath::new(&b))}),
    )
}
fn add_row(rows: &mut Vec<Value>, bytes: &mut usize, row: Value) -> Result<bool, Error> {
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
fn response(snapshot: SnapshotRef, entries: Vec<Value>, end: usize, total: usize) -> Value {
    let token = snapshot.encode();
    let next = (end < total).then(|| {
        CursorRef {
            k: None,
            s: snapshot,
            o: end,
        }
        .encode()
    });
    json!({"snapshot":token,"entries":entries,"nextCursor":next,"metadata":{"totalEntries":total}})
}
pub(super) fn page(
    cache: &mut Cache,
    repository: RepoRef,
    query: String,
    count: usize,
    snapshot: Option<SnapshotRef>,
    offset: usize,
) -> Result<Value, Error> {
    let (filter, kind): (String, String) = serde_json::from_str(
        query
            .strip_prefix("branches:")
            .ok_or_else(|| Error::invalid("Invalid branch query."))?,
    )
    .map_err(|_| Error::invalid("Invalid branch query."))?;
    // Always reopen: a cached snapshot never conceals repository replacement.
    let repo = repository.open()?;
    let config = repo
        .config()
        .and_then(|mut c| c.snapshot())
        .map_err(engine)?;
    let config_hash = config_hash(&config)?;
    let mut remotes = HashMap::new();
    if let Some(snapshot) = &snapshot {
        let token = snapshot.encode();
        if let Some(position) = cache.0.iter().position(|(key, _)| key == &token) {
            let cached = cache.0.remove(position);
            cache.0.push(cached);
            let index = &cache.0.last().expect("inserted").1;
            if index.config_hash != config_hash {
                return Err(expired());
            }
            if offset > index.entries.len() {
                return Err(Error::invalid("Cursor is past the end of this listing."));
            }
            let mut rows = Vec::new();
            let mut bytes = 0;
            for branch in index.entries.iter().skip(offset).take(count) {
                if !add_row(
                    &mut rows,
                    &mut bytes,
                    row(
                        &repo,
                        &config,
                        &mut remotes,
                        branch,
                        index.current.as_deref(),
                        Some(&index.references),
                    )?,
                )? {
                    break;
                }
            }
            let end = offset + rows.len();
            return Ok(response(snapshot.clone(), rows, end, index.entries.len()));
        }
    }
    let current = repo
        .head()
        .ok()
        .filter(|h| h.is_branch())
        .map(|h| h.name_bytes().to_vec());
    let mut hash = Sha256::new();
    hash.update(b"branch-pages-v2");
    hash.update(&config_hash);
    hash.update(current.as_deref().unwrap_or_default());
    let mut index = Some(Index {
        entries: Vec::new(),
        references: BTreeSet::new(),
        current: current.clone(),
        config_hash,
        bytes: 0,
    });
    let mut rows = Vec::new();
    let mut bytes = 0;
    let mut total = 0;
    let mut end = offset;
    for reference in repo.references().map_err(engine)? {
        let reference = reference.map_err(engine)?;
        let refname = reference.name_bytes();
        for value in [
            refname,
            reference.symbolic_target_bytes().unwrap_or_default(),
        ] {
            hash.update((value.len() as u64).to_le_bytes());
            hash.update(value);
        }
        hash.update(
            reference
                .target()
                .map(|oid| oid.to_string())
                .unwrap_or_default(),
        );
        if let Some(index) = &mut index {
            index.bytes += refname.len() + 96;
            index.references.insert(refname.to_vec());
        }
        if index.as_ref().is_some_and(|i| i.bytes > INDEX_BYTES) {
            index = None;
        }
        let local = refname.starts_with(b"refs/heads/");
        if !local && !refname.starts_with(b"refs/remotes/") {
            continue;
        }
        let branch = Branch {
            reference: refname.to_vec(),
            oid: reference.target(),
            local,
        };
        if (kind == "local" && !local)
            || (kind == "remote" && local)
            || (!filter.is_empty()
                && !String::from_utf8_lossy(branch.name())
                    .to_lowercase()
                    .contains(&filter))
        {
            continue;
        }
        let position = total;
        total += 1;
        if position == end
            && rows.len() < count
            && add_row(
                &mut rows,
                &mut bytes,
                row(
                    &repo,
                    &config,
                    &mut remotes,
                    &branch,
                    current.as_deref(),
                    None,
                )?,
            )?
        {
            end += 1;
        }
        if local && std::str::from_utf8(branch.name()).is_err() {
            index = None;
        }
        if let Some(index) = &mut index {
            index.bytes += refname.len() + 2 * std::mem::size_of::<Branch>();
            index.entries.push(branch);
        }
        if index.as_ref().is_some_and(|i| i.bytes > INDEX_BYTES) {
            index = None;
        }
    }
    if offset > total {
        return Err(Error::invalid("Cursor is past the end of this listing."));
    }
    let fingerprint = hex(&hash.finalize());
    if snapshot.as_ref().is_some_and(|s| s.f != fingerprint) {
        return Err(expired());
    }
    let snapshot = snapshot.unwrap_or(SnapshotRef {
        r: repository,
        q: query,
        f: fingerprint,
        p: None,
    });
    if let Some(mut index) = index {
        index.entries.shrink_to_fit();
        let token = snapshot.encode();
        index.bytes += token.len();
        cache.0.retain(|(key, _)| key != &token);
        cache.0.push((token, index));
        while cache.0.len() > CACHE_ENTRIES
            || cache.0.iter().map(|(_, i)| i.bytes).sum::<usize>() > CACHE_BYTES
        {
            cache.0.remove(0);
        }
    }
    Ok(response(snapshot, rows, end, total))
}
#[cfg(test)]
mod tests {
    use super::*;

    fn request(
        service: &mut Service,
        id: &str,
        filter: &str,
        kind: &str,
        cursor: Option<&str>,
    ) -> Result<Value, Error> {
        match service.request(serde_json::from_value(json!({"method":"repo.branches", "params":{"repoId":id,"pageSize":2,"filter":filter,"branchKind":kind,"cursor":cursor}})).unwrap())? {
            Output::Json(value) => Ok(value),
            _ => panic!("expected JSON"),
        }
    }

    #[test]
    fn filtered_pages_cover_large_refsets_and_bind_cursors() {
        let dir = tempfile::tempdir().unwrap();
        let repo = Repository::init(dir.path()).unwrap();
        super::super::tests::commit(&repo, "initial");
        let oid = repo.head().unwrap().target().unwrap();
        // Packed refs exercise >10,000 branches without thousands of file writes.
        let mut packed = String::from("# pack-refs with: peeled fully-peeled sorted\n");
        for index in 0..10_010 {
            packed.push_str(&format!("{oid} refs/heads/topic-{index:05}\n"));
        }
        packed.push_str(&format!("{oid} refs/remotes/origin/topic-10009\n"));
        fs::write(repo.path().join("packed-refs"), packed).unwrap();
        let mut service = Service::default();
        let id = match service
            .request(Request::Open {
                path: wire_path(dir.path()),
            })
            .unwrap()
        {
            Output::Json(value) => value["repoId"].as_str().unwrap().to_owned(),
            _ => panic!(),
        };
        let first = request(&mut service, &id, "TOPIC-1000", "local", None).unwrap();
        assert_eq!(first["metadata"]["totalEntries"], 10);
        assert!(
            service.branch_cache.0.is_empty(),
            "oversized compact indexes are not retained"
        );
        assert_eq!(first["entries"][0]["name"]["display"], "topic-10000");
        let cursor = first["nextCursor"].as_str().unwrap();
        let second = request(
            &mut Service::default(),
            &id,
            "topic-1000",
            "local",
            Some(cursor),
        )
        .unwrap();
        assert_eq!(second["entries"][0]["name"]["display"], "topic-10002");
        assert_eq!(
            request(&mut service, &id, "other", "local", Some(cursor))
                .unwrap_err()
                .code,
            "INVALID_REQUEST"
        );
        assert_eq!(
            request(&mut service, &id, "topic-1000", "all", Some(cursor))
                .unwrap_err()
                .code,
            "INVALID_REQUEST"
        );
        let all = request(&mut service, &id, "", "all", None).unwrap();
        assert_eq!(all["metadata"]["totalEntries"], 10_012);
        let remote = request(&mut service, &id, "TOPIC", "remote", None).unwrap();
        assert_eq!(remote["entries"].as_array().unwrap().len(), 1);
        assert!(remote["nextCursor"].is_null());
        repo.reference("refs/heads/topic-10010", oid, false, "test")
            .unwrap();
        assert_eq!(
            request(
                &mut Service::default(),
                &id,
                "topic-1000",
                "local",
                Some(cursor)
            )
            .unwrap_err()
            .code,
            "SNAPSHOT_EXPIRED"
        );
    }
    #[test]
    fn cached_pages_preserve_refs_and_head_but_expire_config_and_eviction() {
        let dir = tempfile::tempdir().unwrap();
        let repo = Repository::init(dir.path()).unwrap();
        super::super::tests::commit(&repo, "initial");
        let oid = repo.head().unwrap().target().unwrap();
        for name in ["a", "b", "c", "d"] {
            repo.reference(&format!("refs/heads/{name}"), oid, false, "")
                .unwrap();
        }
        repo.remote("origin", "https://example.test/repo").unwrap();
        repo.reference("refs/remotes/origin/c", oid, false, "")
            .unwrap();
        repo.config()
            .unwrap()
            .set_str("branch.c.remote", "origin")
            .unwrap();
        repo.config()
            .unwrap()
            .set_str("branch.c.merge", "refs/heads/c")
            .unwrap();
        repo.set_head("refs/heads/c").unwrap();
        let mut service = Service::default();
        let id = match service
            .request(Request::Open {
                path: wire_path(dir.path()),
            })
            .unwrap()
        {
            Output::Json(v) => v["repoId"].as_str().unwrap().to_owned(),
            _ => panic!(),
        };
        let first = request(&mut service, &id, "", "local", None).unwrap();
        let cursor = first["nextCursor"].as_str().unwrap();
        repo.find_reference("refs/heads/c")
            .unwrap()
            .rename("refs/heads/renamed", false, "")
            .unwrap();
        repo.set_head("refs/heads/a").unwrap();
        repo.find_reference("refs/remotes/origin/c")
            .unwrap()
            .delete()
            .unwrap();
        let old = request(&mut service, &id, "", "local", Some(cursor)).unwrap();
        assert_eq!(old["entries"][0]["name"]["display"], "c");
        assert_eq!(old["entries"][0]["current"], true);
        assert_eq!(
            old["entries"][0]["upstream"]["display"],
            "refs/remotes/origin/c"
        );
        assert_eq!(
            request(&mut Service::default(), &id, "", "local", Some(cursor))
                .unwrap_err()
                .code,
            "SNAPSHOT_EXPIRED"
        );
        let fresh = request(&mut service, &id, "renamed", "local", None).unwrap();
        assert_eq!(fresh["entries"][0]["name"]["display"], "renamed");
        repo.config()
            .unwrap()
            .set_str("branch.c.remote", "other")
            .unwrap();
        assert_eq!(
            request(&mut service, &id, "", "local", Some(cursor))
                .unwrap_err()
                .code,
            "SNAPSHOT_EXPIRED"
        );
        repo.config()
            .unwrap()
            .set_str("branch.c.remote", "origin")
            .unwrap();
        for n in 0..CACHE_ENTRIES {
            request(&mut service, &id, &format!("no-match-{n}"), "all", None).unwrap();
        }
        assert_eq!(
            request(&mut service, &id, "", "local", Some(cursor))
                .unwrap_err()
                .code,
            "SNAPSHOT_EXPIRED"
        );
    }
}
