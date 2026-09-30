//! History pages hydrate only requested commits. Cursors anchor the original
//! tip, never a moving branch. No whole-history JSON listing is retained.
use super::*;

// Topological sorting already visits the graph before yielding its first oid.
// Cache only that compact ordering, so subsequent pages don't repeat the walk.
// A bounded prefix is an optimisation, never a history truncation boundary.
#[derive(Default)]
pub(super) struct Cache(Vec<(String, Arc<Vec<git2::Oid>>, bool)>);
const INDEX_LIMIT: usize = 8 * 1024 * 1024 / std::mem::size_of::<git2::Oid>();

pub(super) fn query(revision: &str, message_bytes: Option<usize>) -> Result<String, Error> {
    if revision.len() > 1024 || revision.contains('\0') {
        return Err(Error::invalid("Invalid revision."));
    }
    let budget = message_bytes.unwrap_or(16384);
    if !(1..=16384).contains(&budget) {
        return Err(Error::invalid("messageBytes must be between 1 and 16384."));
    }
    Ok(if budget == 16384 {
        format!("history:{revision}")
    } else {
        format!("history_summary:{}", json!([revision, budget]))
    })
}
fn row(commit: &git2::Commit<'_>, message_bytes: usize) -> Value {
    let message = commit.message_bytes();
    json!({"oid":oid(commit.id()),"parents":commit.parent_ids().map(oid).collect::<Vec<_>>(),"message":WirePath::new(&message[..message.len().min(message_bytes)]),"messageTruncated":message.len()>message_bytes,"author":{"name":String::from_utf8_lossy(commit.author().name_bytes()),"email":String::from_utf8_lossy(commit.author().email_bytes())},"time":commit.time().seconds(),"offsetMinutes":commit.time().offset_minutes()})
}
pub(super) fn commit(repository: RepoRef, commit_oid: &str) -> Result<Value, Error> {
    let repo = repository.open()?;
    let format = repo.object_format();
    let length = if format == git2::ObjectFormat::Sha1 {
        40
    } else {
        64
    };
    if commit_oid.len() != length {
        return Err(Error::invalid("A complete commit ID is required."));
    }
    let id = git2::Oid::from_str_ext(commit_oid, format)
        .map_err(|_| Error::invalid("Invalid commit ID."))?;
    let commit = repo.find_commit(id).map_err(engine)?;
    let result = row(&commit, 16384);
    if serde_json::to_vec(&result).map_err(|_| limit())?.len() > MAX_FRAME / 2 {
        return Err(limit());
    }
    Ok(result)
}

pub(super) fn page(
    cache: &mut Cache,
    repository: RepoRef,
    query: String,
    count: usize,
    snapshot: Option<SnapshotRef>,
    offset: usize,
) -> Result<Value, Error> {
    let (revision, message_bytes) = if let Some(revision) = query.strip_prefix("history:") {
        (revision.to_owned(), 16384)
    } else {
        serde_json::from_str::<(String, usize)>(
            query
                .strip_prefix("history_summary:")
                .ok_or_else(|| Error::invalid("Invalid history query."))?,
        )
        .map_err(|_| Error::invalid("Invalid history query."))?
    };
    let repo = repository.open()?;
    let tip = match snapshot.as_ref() {
        Some(snapshot) => snapshot
            .p
            .as_deref()
            .map(|value| {
                let format = repo.object_format();
                let length = if format == git2::ObjectFormat::Sha1 {
                    40
                } else {
                    64
                };
                if value.len() != length {
                    return Err(Error::invalid("Invalid history anchor."));
                }
                git2::Oid::from_str_ext(value, format)
                    .map_err(|_| Error::invalid("Invalid history anchor."))
            })
            .transpose()?,
        None => match repo.revparse_single(&revision) {
            Ok(object) => Some(object.peel_to_commit().map_err(engine)?.id()),
            Err(_)
                if revision == "HEAD"
                    && repo
                        .head()
                        .is_err_and(|error| error.code() == git2::ErrorCode::UnbornBranch) =>
            {
                None
            }
            Err(error) => return Err(engine(error)),
        },
    };
    // Shallow boundaries can change even with an unchanged tip after a fetch.
    let mut hash = Sha256::new();
    hash.update(b"history-page-v1");
    hash.update(tip.map(|tip| tip.to_string()).unwrap_or_default());
    let mut budget = MAX_INDEX_BYTES;
    hash_file(&repo.commondir().join("shallow"), &mut hash, &mut budget)?;
    let fingerprint = hex(&hash.finalize());
    if snapshot
        .as_ref()
        .is_some_and(|snapshot| snapshot.f != fingerprint)
    {
        return Err(Error::new(
            "SNAPSHOT_EXPIRED",
            "History changed. Restart it from the first page.",
        ));
    }
    let snapshot = snapshot.unwrap_or(SnapshotRef {
        r: repository,
        q: query,
        f: fingerprint,
        p: tip.map(|tip| tip.to_string()),
    });
    let token = snapshot.encode();
    let Some(tip) = tip else {
        if offset != 0 {
            return Err(Error::invalid("Cursor is past the end of history."));
        }
        return Ok(json!({"snapshot":token,"entries":[],"nextCursor":null,"metadata":{}}));
    };
    let (ids, complete) = if let Some(index) = cache.0.iter().position(|(key, _, _)| key == &token)
    {
        let entry = cache.0.remove(index);
        let result = (entry.1.clone(), entry.2);
        cache.0.push(entry);
        result
    } else {
        let mut walk = repo.revwalk().map_err(engine)?;
        walk.set_sorting(git2::Sort::TOPOLOGICAL | git2::Sort::TIME)
            .map_err(engine)?;
        walk.push(tip).map_err(engine)?;
        let mut ids = Vec::new();
        for id in walk.by_ref().take(INDEX_LIMIT) {
            ids.push(id.map_err(engine)?);
        }
        let complete = walk.next().transpose().map_err(engine)?.is_none();
        ids.shrink_to_fit();
        let ids = Arc::new(ids);
        cache.0.push((token.clone(), ids.clone(), complete));
        while cache.0.len() > CACHE_ENTRIES
            || cache
                .0
                .iter()
                .map(|(_, ids, _)| ids.capacity() * std::mem::size_of::<git2::Oid>())
                .sum::<usize>()
                > CACHE_BYTES
        {
            cache.0.remove(0);
        }
        (ids, complete)
    };
    if complete && offset > ids.len() {
        return Err(Error::invalid("Cursor is past the end of history."));
    }
    let mut walk;
    let mut iter: Box<dyn Iterator<Item = Result<git2::Oid, git2::Error>> + '_> =
        if complete || offset.saturating_add(count) < ids.len() {
            Box::new(ids.iter().skip(offset).copied().map(Ok))
        } else {
            walk = repo.revwalk().map_err(engine)?;
            walk.set_sorting(git2::Sort::TOPOLOGICAL | git2::Sort::TIME)
                .map_err(engine)?;
            walk.push(tip).map_err(engine)?;
            for _ in 0..offset {
                walk.next()
                    .ok_or_else(|| Error::invalid("Cursor is past the end of history."))?
                    .map_err(engine)?;
            }
            Box::new(walk)
        };
    let mut rows = Vec::with_capacity(count);
    let mut bytes = 0;
    let mut more = false;
    for id in iter.by_ref() {
        let id = id.map_err(engine)?;
        if rows.len() == count {
            more = true;
            break;
        }
        let commit = repo.find_commit(id).map_err(engine)?;
        let row = row(&commit, message_bytes);
        let size = serde_json::to_vec(&row).map_err(|_| limit())?.len();
        if size > MAX_FRAME / 2 {
            return Err(limit());
        }
        if bytes + size > MAX_FRAME / 2 {
            more = true;
            break;
        }
        bytes += size;
        rows.push(row);
    }
    let next = more.then(|| {
        CursorRef {
            k: None,
            s: snapshot,
            o: offset + rows.len(),
        }
        .encode()
    });
    Ok(
        json!({"snapshot":token,"entries":rows,"nextCursor":next,"metadata":{"resolvedRevision":oid(tip),"truncated":false}}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    fn request(
        service: &mut Service,
        id: &str,
        budget: Option<usize>,
        cursor: Option<&str>,
    ) -> Result<Value, Error> {
        match service.request(Request::History {
            repo_id: id.into(),
            revision: "HEAD".into(),
            page_size: 1,
            cursor: cursor.map(str::to_owned),
            message_bytes: budget,
        })? {
            Output::Json(value) => Ok(value),
            _ => panic!(),
        }
    }
    #[test]
    fn summaries_are_bounded_cursor_bound_and_detail_is_direct() {
        let dir = tempfile::tempdir().unwrap();
        let repo = Repository::init(dir.path()).unwrap();
        let message = "é".repeat(9000);
        super::super::tests::commit(&repo, &message);
        let root = repo.head().unwrap().target().unwrap();
        super::super::tests::commit(&repo, &message);
        let tip = repo.head().unwrap().target().unwrap();
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
        let full = request(&mut service, &id, None, None).unwrap();
        let summary = request(&mut service, &id, Some(511), None).unwrap();
        let entry = &summary["entries"][0];
        assert_eq!(entry["oid"], full["entries"][0]["oid"]);
        assert_eq!(entry["parents"], full["entries"][0]["parents"]);
        assert_eq!(entry["messageTruncated"], true);
        assert_eq!(
            STANDARD
                .decode(entry["message"]["bytesB64"].as_str().unwrap())
                .unwrap()
                .len(),
            511
        );
        let cursor = summary["nextCursor"].as_str().unwrap();
        assert_eq!(
            request(&mut service, &id, None, Some(cursor))
                .unwrap_err()
                .code,
            "INVALID_REQUEST"
        );
        assert_eq!(
            request(&mut service, &id, Some(512), Some(cursor))
                .unwrap_err()
                .code,
            "INVALID_REQUEST"
        );
        let next = request(&mut Service::default(), &id, Some(511), Some(cursor)).unwrap();
        assert_eq!(next["entries"][0]["oid"]["hex"], root.to_string());
        assert!(next["nextCursor"].is_null());
        for budget in [0, 16385, usize::MAX] {
            assert_eq!(
                request(&mut service, &id, Some(budget), None)
                    .unwrap_err()
                    .code,
                "INVALID_REQUEST"
            );
        }
        let legacy = serde_json::to_value(Request::History {
            repo_id: id.clone(),
            revision: "HEAD".into(),
            page_size: 1,
            cursor: None,
            message_bytes: None,
        })
        .unwrap();
        assert!(legacy["params"].get("messageBytes").is_none());
        // Resolve directly by object ID even after HEAD moves or disappears.
        repo.set_head("refs/heads/unborn").unwrap();
        let Output::Json(detail) = service
            .request(Request::Commit {
                repo_id: id,
                commit_oid: tip.to_string(),
            })
            .unwrap()
        else {
            panic!()
        };
        assert_eq!(detail, full["entries"][0]);
        assert_eq!(detail["messageTruncated"], true);
        assert_eq!(
            STANDARD
                .decode(detail["message"]["bytesB64"].as_str().unwrap())
                .unwrap()
                .len(),
            16384
        );
    }
    #[test]
    fn summary_queries_do_not_confuse_revision_delimiters() {
        let revision = "refs/heads/topic:with,[punctuation]";
        let encoded = query(revision, Some(512)).unwrap();
        let (decoded, budget): (String, usize) =
            serde_json::from_str(encoded.strip_prefix("history_summary:").unwrap()).unwrap();
        assert_eq!(decoded, revision);
        assert_eq!(budget, 512);
        assert_eq!(
            query("HEAD", Some(16384)).unwrap(),
            query("HEAD", None).unwrap()
        );
    }
}
