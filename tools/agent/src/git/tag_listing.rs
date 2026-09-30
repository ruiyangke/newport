//! Lazy tag object reads over bounded compact reference snapshots. A sort-key
//! cursor keeps overflow pages bounded even when display ordering differs from
//! the native byte ordering of reference names.
use super::*;
use std::{cmp::Ordering, collections::BinaryHeap};
#[cfg(not(test))]
const INDEX_BYTES: usize = 8 * 1024 * 1024;
#[cfg(test)]
const INDEX_BYTES: usize = 32 * 1024;
#[derive(Default)]
pub(super) struct Cache(Vec<(String, Arc<Index>)>);
struct Index {
    refs: Vec<Ref>,
    bytes: usize,
}
#[derive(Clone)]
struct Ref {
    key: (String, String),
    reference: Vec<u8>,
    object: Option<git2::Oid>,
    symbolic: Option<Vec<u8>>,
}
impl PartialEq for Ref {
    fn eq(&self, other: &Self) -> bool {
        self.key == other.key
    }
}
impl Eq for Ref {}
impl PartialOrd for Ref {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for Ref {
    fn cmp(&self, other: &Self) -> Ordering {
        self.key.cmp(&other.key)
    }
}
impl Ref {
    fn capture(reference: git2::Reference<'_>) -> Result<Self, Error> {
        let name = reference
            .name_bytes()
            .strip_prefix(b"refs/tags/")
            .ok_or_else(|| Error::invalid("Invalid tag reference."))?;
        Ok(Self {
            key: (
                String::from_utf8_lossy(name).into_owned(),
                STANDARD.encode(name),
            ),
            reference: reference.name_bytes().to_vec(),
            object: reference.target(),
            symbolic: reference.symbolic_target_bytes().map(<[u8]>::to_vec),
        })
    }
    fn bytes(&self) -> usize {
        2 * std::mem::size_of::<Self>()
            + self.key.0.capacity()
            + self.key.1.capacity()
            + self.reference.capacity()
            + self.symbolic.as_ref().map_or(0, Vec::capacity)
    }
    fn row(&self, repo: &Repository, odb: &git2::Odb<'_>, budget: usize) -> Result<Value, Error> {
        let mut row = super::super::tags::details(repo, odb, self.object, budget)?;
        row["name"] = json!(WirePath::new(&self.reference[10..]));
        row["reference"] = json!(WirePath::new(&self.reference));
        row["symbolicTarget"] = json!(self.symbolic.as_deref().map(WirePath::new));
        Ok(row)
    }
}
pub(super) fn query(budget: Option<usize>) -> Result<String, Error> {
    let budget = budget.unwrap_or(16384);
    if !(1..=16384).contains(&budget) {
        return Err(Error::invalid("messageBytes must be between 1 and 16384."));
    }
    Ok(if budget == 16384 {
        "tags".into()
    } else {
        format!("tags_summary:{budget}")
    })
}
fn key(value: &str) -> Result<(String, String), Error> {
    let parsed: (String, String) =
        serde_json::from_str(value).map_err(|_| Error::invalid("Invalid tag cursor key."))?;
    let bytes = STANDARD
        .decode(&parsed.1)
        .map_err(|_| Error::invalid("Invalid tag cursor key."))?;
    if parsed.0 != String::from_utf8_lossy(&bytes) || bytes.is_empty() || bytes.contains(&0) {
        return Err(Error::invalid("Invalid tag cursor key."));
    }
    Ok(parsed)
}
fn expired() -> Error {
    Error::new(
        "SNAPSHOT_EXPIRED",
        "Tags changed. Restart from the first page.",
    )
}
fn render(
    repo: &Repository,
    snapshot: SnapshotRef,
    refs: &[Ref],
    offset: usize,
    total: usize,
    count: usize,
    budget: usize,
) -> Result<Value, Error> {
    let odb = repo.odb().map_err(engine)?;
    let mut rows = Vec::new();
    let mut bytes = 0;
    let mut last = None;
    for reference in refs.iter().take(count) {
        let row = reference.row(repo, &odb, budget)?;
        let size = serde_json::to_vec(&row).map_err(|_| limit())?.len();
        if size > MAX_FRAME / 2 {
            return Err(limit());
        }
        if bytes + size > MAX_FRAME / 2 {
            break;
        }
        bytes += size;
        rows.push(row);
        last = Some(&reference.key);
    }
    let end = offset + rows.len();
    let token = snapshot.encode();
    let next = if end < total {
        Some(
            CursorRef {
                s: snapshot,
                o: end,
                k: Some(serde_json::to_string(last.ok_or_else(limit)?).map_err(|_| limit())?),
            }
            .encode(),
        )
    } else {
        None
    };
    Ok(json!({"snapshot":token,"entries":rows,"nextCursor":next,"metadata":{"totalEntries":total}}))
}
pub(super) fn page(
    cache: &mut Cache,
    repository: RepoRef,
    query: String,
    count: usize,
    snapshot: Option<SnapshotRef>,
    offset: usize,
    cursor_key: Option<String>,
) -> Result<Value, Error> {
    let budget = if query == "tags" {
        16384
    } else {
        query
            .strip_prefix("tags_summary:")
            .and_then(|n| n.parse().ok())
            .ok_or_else(|| Error::invalid("Invalid tag query."))?
    };
    let after = cursor_key.as_deref().map(key).transpose()?;
    if (offset == 0) != after.is_none() {
        return Err(Error::invalid("Tag cursor requires its previous sort key."));
    }
    let repo = repository.open()?;
    if let Some(snapshot) = &snapshot {
        let token = snapshot.encode();
        if let Some(position) = cache.0.iter().position(|(t, _)| t == &token) {
            let cached = cache.0.remove(position);
            let index = cached.1.clone();
            cache.0.push(cached);
            if offset > index.refs.len()
                || (offset > 0 && after.as_ref() != Some(&index.refs[offset - 1].key))
            {
                return Err(Error::invalid("Tag cursor offset does not match its key."));
            }
            return render(
                &repo,
                snapshot.clone(),
                &index.refs[offset..],
                offset,
                index.refs.len(),
                count,
                budget,
            );
        }
    }
    let mut captured = Some(Index {
        refs: Vec::new(),
        bytes: 0,
    });
    let mut selected = BinaryHeap::new();
    let mut total = 0usize;
    let mut before = 0usize;
    let mut found = false;
    // Commutative multiset fingerprint: loose/packed ref iteration order must
    // not change a cursor. Ref names are unique; hash every name/target pair.
    let mut digest = [0u64; 4];
    for reference in repo.references_glob("refs/tags/*").map_err(engine)? {
        let reference = Ref::capture(reference.map_err(engine)?)?;
        total += 1;
        let mut hash = Sha256::new();
        for bytes in [
            reference.reference.as_slice(),
            reference.symbolic.as_deref().unwrap_or_default(),
        ] {
            hash.update((bytes.len() as u64).to_le_bytes());
            hash.update(bytes);
        }
        hash.update(reference.object.map(|o| o.to_string()).unwrap_or_default());
        for (sum, bytes) in digest
            .iter_mut()
            .zip(hash.finalize().as_slice().as_chunks::<8>().0)
        {
            *sum = sum.wrapping_add(u64::from_le_bytes(*bytes));
        }
        if after.as_ref().is_some_and(|k| reference.key <= *k) {
            before += 1;
            found |= after.as_ref() == Some(&reference.key);
        } else if selected.len() < count
            || selected.peek().is_some_and(|last: &Ref| reference < *last)
        {
            selected.push(reference.clone());
            if selected.len() > count {
                selected.pop();
            }
        }
        if let Some(captured) = &mut captured {
            captured.bytes += reference.bytes();
            captured.refs.push(reference);
        }
        if captured.as_ref().is_some_and(|i| i.bytes > INDEX_BYTES) {
            captured = None;
        }
    }
    let fingerprint =
        super::super::journal::hash(&serde_json::to_vec(&(digest, total)).map_err(|_| limit())?);
    if snapshot.as_ref().is_some_and(|s| s.f != fingerprint) {
        return Err(expired());
    }
    if offset > total || (offset > 0 && (!found || before != offset)) {
        return Err(Error::invalid("Tag cursor offset does not match its key."));
    }
    let snapshot = snapshot.unwrap_or(SnapshotRef {
        r: repository,
        q: query,
        f: fingerprint,
        p: None,
    });
    if let Some(mut captured) = captured {
        captured.refs.sort();
        captured.refs.shrink_to_fit();
        let token = snapshot.encode();
        captured.bytes += token.len();
        cache.0.retain(|(t, _)| t != &token);
        cache.0.push((token, Arc::new(captured)));
        while cache.0.len() > CACHE_ENTRIES
            || cache.0.iter().map(|(_, i)| i.bytes).sum::<usize>() > CACHE_BYTES
        {
            cache.0.remove(0);
        }
    }
    render(
        &repo,
        snapshot,
        &selected.into_sorted_vec(),
        offset,
        total,
        count,
        budget,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    fn open(service: &mut Service, path: &Path) -> String {
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
    fn page(
        service: &mut Service,
        id: &str,
        budget: Option<usize>,
        cursor: Option<String>,
    ) -> Result<Value, Error> {
        let Output::Json(v) = service.request(Request::Tags {
            repo_id: id.into(),
            page_size: 2,
            cursor,
            message_bytes: budget,
        })?
        else {
            panic!()
        };
        Ok(v)
    }
    fn fixture(count: usize) -> (tempfile::TempDir, Repository) {
        let dir = tempfile::tempdir().unwrap();
        let repo = Repository::init(dir.path()).unwrap();
        let blob = repo.blob(b"tag target").unwrap();
        let mut packed = String::from("# pack-refs with: peeled fully-peeled sorted\n");
        for n in 0..count {
            packed.push_str(&format!("{blob} refs/tags/v{n:05}\n"));
        }
        fs::write(repo.path().join("packed-refs"), packed).unwrap();
        (dir, repo)
    }
    #[test]
    fn unbounded_tags_continue_and_validate_keys_in_cached_and_overflow_paths() {
        for count in [5, 10_011] {
            let (dir, repo) = fixture(count);
            let mut service = Service::default();
            let id = open(&mut service, dir.path());
            let first = page(&mut service, &id, None, None).unwrap();
            assert_eq!(first["metadata"]["totalEntries"], count);
            assert_eq!(service.tag_cache.0.is_empty(), count > 5);
            let valid = CursorRef::decode(first["nextCursor"].as_str().unwrap()).unwrap();
            let mut wrong_query = valid.clone();
            wrong_query.s.q = "history:HEAD".into();
            assert!(
                matches!(service.request(Request::History {repo_id:id.clone(),revision:"HEAD".into(),page_size:2,cursor:Some(wrong_query.encode()),message_bytes:None}),Err(Error{code,..}) if code=="INVALID_REQUEST")
            );

            let second = page(&mut service, &id, None, Some(valid.encode())).unwrap();
            assert_eq!(second["entries"][0]["name"]["display"], "v00002");
            for cached in [true, false] {
                let mut fresh = Service::default();
                let target = if cached { &mut service } else { &mut fresh };
                for (offset, k) in [
                    (usize::MAX, valid.k.clone()),
                    (1, valid.k.clone()),
                    (2, None),
                    (2, Some("invalid json".into())),
                    (
                        2,
                        Some(json!(["mismatch", STANDARD.encode(b"v00001")]).to_string()),
                    ),
                ] {
                    let bad = CursorRef {
                        s: valid.s.clone(),
                        o: offset,
                        k,
                    };
                    assert_eq!(
                        page(target, &id, None, Some(bad.encode()))
                            .unwrap_err()
                            .code,
                        "INVALID_REQUEST"
                    );
                }
            }
            if count > 5 {
                let late = CursorRef {
                    s: valid.s.clone(),
                    o: 10_000,
                    k: Some(json!(["v09999", STANDARD.encode(b"v09999")]).to_string()),
                };
                let result = page(&mut service, &id, None, Some(late.encode())).unwrap();
                assert_eq!(result["entries"][0]["name"]["display"], "v10000");
            }
            repo.find_reference("refs/tags/v00002")
                .unwrap()
                .delete()
                .unwrap();
            if count == 5 {
                assert_eq!(
                    page(&mut service, &id, None, Some(valid.encode())).unwrap()["entries"][0]
                        ["name"]["display"],
                    "v00002"
                );
            }
            assert_eq!(
                page(&mut Service::default(), &id, None, Some(valid.encode()))
                    .unwrap_err()
                    .code,
                "SNAPSHOT_EXPIRED"
            );
        }
    }
    #[test]
    fn large_annotations_are_paged_summarized_and_read_directly() {
        let (dir, repo) = fixture(0);
        let object = repo.find_object(repo.blob(b"blob").unwrap(), None).unwrap();
        let sig = git2::Signature::now("Tagger", "tagger@example.test").unwrap();
        let annotated = repo
            .tag_annotation_create("v", &object, &sig, &"x".repeat(16_385))
            .unwrap();
        let mut packed = String::new();
        for n in 0..1200 {
            packed.push_str(&format!("{annotated} refs/tags/v{n:05}\n"));
        }
        fs::write(repo.path().join("packed-refs"), packed).unwrap();
        let mut service = Service::default();
        let id = open(&mut service, dir.path());
        let summary = page(&mut service, &id, Some(512), None).unwrap();
        assert_eq!(
            summary["entries"][0]["message"]["display"]
                .as_str()
                .unwrap()
                .len(),
            512
        );
        assert_eq!(summary["entries"][0]["messageTruncated"], true);
        let cursor = summary["nextCursor"].as_str().unwrap().into();
        assert_eq!(
            page(&mut service, &id, None, Some(cursor))
                .unwrap_err()
                .code,
            "INVALID_REQUEST"
        );
        let full = page(&mut service, &id, None, None).unwrap();
        let Output::Json(mut detail) = service
            .request(Request::Tag {
                repo_id: id.clone(),
                oid: annotated.to_string(),
            })
            .unwrap()
        else {
            panic!()
        };
        assert_eq!(detail["message"]["display"].as_str().unwrap().len(), 16384);
        assert_eq!(detail["messageTruncated"], true);
        for field in ["name", "reference", "symbolicTarget"] {
            assert!(detail.get(field).is_none());
            detail[field] = full["entries"][0][field].clone();
        }
        assert_eq!(detail, full["entries"][0]);
        for budget in [0, 16_385, usize::MAX] {
            assert_eq!(
                page(&mut service, &id, Some(budget), None)
                    .unwrap_err()
                    .code,
                "INVALID_REQUEST"
            );
        }
        let wire = serde_json::to_value(Request::Tags {
            repo_id: id,
            page_size: 2,
            cursor: None,
            message_bytes: None,
        })
        .unwrap();
        assert!(wire["params"].get("messageBytes").is_none());
    }
    #[test]
    fn invalid_utf8_sort_keys_roundtrip_without_skips() {
        let (dir, repo) = fixture(0);
        let blob = repo.blob(b"blob").unwrap();
        let mut packed = Vec::new();
        for name in [b"a".as_slice(), b"\xfe", b"\xff"] {
            packed.extend_from_slice(format!("{blob} refs/tags/").as_bytes());
            packed.extend_from_slice(name);
            packed.push(b'\n');
        }
        fs::write(repo.path().join("packed-refs"), packed).unwrap();
        let mut service = Service::default();
        let id = open(&mut service, dir.path());
        let first = page(&mut service, &id, None, None).unwrap();
        let second = page(
            &mut Service::default(),
            &id,
            None,
            first["nextCursor"].as_str().map(str::to_owned),
        )
        .unwrap();
        let mut actual = first["entries"]
            .as_array()
            .unwrap()
            .iter()
            .chain(second["entries"].as_array().unwrap())
            .map(|e| {
                (
                    e["name"]["display"].as_str().unwrap().to_owned(),
                    e["name"]["bytesB64"].as_str().unwrap().to_owned(),
                )
            })
            .collect::<Vec<_>>();
        let mut expected = actual.clone();
        expected.sort();
        expected.dedup();
        assert_eq!(actual, expected);
        assert_eq!(actual.len(), 3);
        actual.clear();
    }
    #[test]
    fn sha256_symbolic_and_nested_details_keep_object_safety_bounds() {
        let dir = tempfile::tempdir().unwrap();
        let mut options = git2::RepositoryInitOptions::new();
        options.object_format(git2::ObjectFormat::Sha256);
        let repo = Repository::init_opts(dir.path(), &options).unwrap();
        let blob = repo.blob(b"blob").unwrap();
        let sig = git2::Signature::now("Tagger", "tagger@example.test").unwrap();
        let annotation = repo
            .tag(
                "annotated",
                &repo.find_object(blob, None).unwrap(),
                &sig,
                "annotation",
                false,
            )
            .unwrap();
        repo.reference_symbolic("refs/tags/symbolic", "refs/tags/annotated", false, "")
            .unwrap();
        let mut service = Service::default();
        let id = open(&mut service, dir.path());
        let page = page(&mut service, &id, None, None).unwrap();
        assert_eq!(page["entries"][0]["oid"]["format"], "sha256");
        assert_eq!(page["entries"][1]["oid"], Value::Null);
        assert_eq!(
            page["entries"][1]["symbolicTarget"]["display"],
            "refs/tags/annotated"
        );
        let Output::Json(detail) = service
            .request(Request::Tag {
                repo_id: id.clone(),
                oid: annotation.to_string(),
            })
            .unwrap()
        else {
            panic!()
        };
        assert_eq!(detail["peeledOid"]["hex"], blob.to_string());
        assert_eq!(detail["messageTruncated"], false);
        let mut nested = annotation;
        for _ in 0..16 {
            nested = repo
                .tag_annotation_create(
                    "nested",
                    &repo.find_object(nested, None).unwrap(),
                    &sig,
                    "nested",
                )
                .unwrap();
        }
        let Output::Json(deep) = service
            .request(Request::Tag {
                repo_id: id.clone(),
                oid: nested.to_string(),
            })
            .unwrap()
        else {
            panic!()
        };
        assert_eq!(deep["detailsOmitted"], true);
        assert!(deep.get("peeledOid").is_none());
        let large = repo
            .tag_annotation_create(
                "large",
                &repo.find_object(blob, None).unwrap(),
                &sig,
                &"x".repeat(1024 * 1024 + 1),
            )
            .unwrap();
        let Output::Json(large) = service
            .request(Request::Tag {
                repo_id: id,
                oid: large.to_string(),
            })
            .unwrap()
        else {
            panic!()
        };
        assert_eq!(large["detailsOmitted"], true);
        assert!(large.get("message").is_none());
    }
}
