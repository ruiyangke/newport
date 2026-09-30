//! Tag references are changed under native locks, with exact object-ID guards.
use super::{
    operations,
    protocol::{Action, Error, Path as WirePath},
    repository,
};
use git2::{ObjectType, Oid, Repository};
use serde_json::{json, Value};

const MAX_OBJECT: usize = 1024 * 1024;
fn engine(error: git2::Error) -> Error {
    match error.code() {
        git2::ErrorCode::Locked => Error::new(
            "REPOSITORY_BUSY",
            "Another operation holds the tag reference lock.",
        ),
        git2::ErrorCode::NotFound => Error::new(
            "TAG_NOT_FOUND",
            "The tag or target object no longer exists.",
        ),
        _ => Error::new("GIT_ERROR", "The tag could not be prepared."),
    }
}
pub(super) fn oid(value: &str) -> Result<Oid, Error> {
    if value.len() != 40 {
        return Err(Error::invalid("A complete SHA-1 object ID is required."));
    }
    Oid::from_str(value).map_err(|_| Error::invalid("Invalid object ID."))
}
fn wire_oid(oid: Oid) -> Value {
    json!({"format":oid.object_format().str(),"hex":oid.to_string()})
}
pub(super) fn reference(name: &str) -> Result<String, Error> {
    if name.len() > 1024 || !git2::Tag::is_valid_name(name) {
        return Err(Error::invalid(
            "Supply a valid tag name of at most 1024 bytes.",
        ));
    }
    Ok(format!("refs/tags/{name}"))
}

pub(super) fn details(
    repo: &Repository,
    odb: &git2::Odb<'_>,
    object: Option<Oid>,
    message_bytes: usize,
) -> Result<Value, Error> {
    let mut row = json!({"oid":object.map(wire_oid),"annotated":false,"detailsOmitted":false});
    if let Some(mut current) = object {
        // Bound both nested tags and object reads. A large external tag can
        // still be displayed/deleted without loading its entire annotation.
        let mut object_bytes: usize = 0;
        for depth in 0..16 {
            let (size, kind) = odb.read_header(current).map_err(engine)?;
            if depth == 0 {
                row["objectType"] = json!(kind.str());
                row["annotated"] = json!(kind == ObjectType::Tag);
                if kind != ObjectType::Tag {
                    row["targetOid"] = wire_oid(current);
                }
            }
            if kind != ObjectType::Tag {
                row["peeledOid"] = wire_oid(current);
                row["peeledType"] = json!(kind.str());
                break;
            }
            object_bytes = object_bytes.saturating_add(size);
            if object_bytes > MAX_OBJECT {
                row["detailsOmitted"] = true.into();
                break;
            }
            let tag = repo.find_tag(current).map_err(engine)?;
            if depth == 0 {
                let message = tag.message_bytes().unwrap_or_default();
                row["targetOid"] = wire_oid(tag.target_id());
                row["message"] = json!(WirePath::new(&message[..message.len().min(message_bytes)]));
                row["messageTruncated"] = json!(message.len() > message_bytes);
                row["tagger"] = tag.tagger().map(|s| json!({"name":String::from_utf8_lossy(s.name_bytes()),"email":String::from_utf8_lossy(s.email_bytes()),"time":s.when().seconds(),"offsetMinutes":s.when().offset_minutes()})).unwrap_or(Value::Null);
            }
            current = tag.target_id();
            if depth == 15 {
                row["detailsOmitted"] = true.into();
            }
        }
    }
    Ok(row)
}

pub(super) fn detail(repo: &Repository, value: &str) -> Result<Value, Error> {
    let format = repo.object_format();
    let length = if format == git2::ObjectFormat::Sha1 {
        40
    } else {
        64
    };
    if value.len() != length {
        return Err(Error::invalid("A complete object ID is required."));
    }
    let oid = Oid::from_str_ext(value, format).map_err(|_| Error::invalid("Invalid object ID."))?;
    details(repo, &repo.odb().map_err(engine)?, Some(oid), 16384)
}

#[cfg(test)]
fn list(repo: &Repository) -> Result<Vec<Value>, Error> {
    let odb = repo.odb().map_err(engine)?;
    let mut rows = Vec::new();
    for reference in repo.references_glob("refs/tags/*").map_err(engine)? {
        let reference = reference.map_err(engine)?;
        let mut row = details(repo, &odb, reference.target(), 16384)?;
        row["name"] = json!(WirePath::new(&reference.name_bytes()[10..]));
        row["reference"] = json!(WirePath::new(reference.name_bytes()));
        row["symbolicTarget"] = json!(reference.symbolic_target_bytes().map(WirePath::new));
        rows.push(row);
    }
    rows.sort_by(|a, b| {
        a["name"]["display"]
            .as_str()
            .cmp(&b["name"]["display"].as_str())
            .then_with(|| {
                a["name"]["bytesB64"]
                    .as_str()
                    .cmp(&b["name"]["bytesB64"].as_str())
            })
    });
    Ok(rows)
}

pub fn apply(repo: &Repository, action: &Action, expected: &str) -> Result<Value, Error> {
    let name = match action {
        Action::TagCreate { name, .. } | Action::TagDelete { name, .. } => name,
        _ => return Err(Error::invalid("Not a tag operation.")),
    };
    let reference_name = reference(name)?;
    let mut tx = repo.transaction().map_err(engine)?;
    tx.lock_ref(&reference_name).map_err(engine)?;
    if repository::fingerprint(repo)? != expected {
        return Err(Error::new(
            "STALE_SNAPSHOT",
            "Refresh repository status before changing tags.",
        ));
    }
    let result = match action {
        Action::TagCreate {
            target_oid,
            annotation,
            ..
        } => {
            match repo.find_reference(&reference_name) {
                Ok(_) => {
                    return Err(Error::new(
                        "TAG_EXISTS",
                        "A tag with this name already exists.",
                    ))
                }
                Err(e) if e.code() == git2::ErrorCode::NotFound => {}
                Err(e) => return Err(engine(e)),
            }
            match repo.config().map_err(engine)?.get_bool("tag.gpgsign") {
                Ok(true) => {
                    return Err(Error::new(
                        "UNSUPPORTED_SIGNING",
                        "Tag signing is configured but is not supported yet.",
                    ))
                }
                Ok(false) => {}
                Err(e) if e.code() == git2::ErrorCode::NotFound => {}
                Err(e) => return Err(engine(e)),
            }
            let target = oid(target_oid)?;
            let (size, _) = repo
                .odb()
                .map_err(engine)?
                .read_header(target)
                .map_err(engine)?;
            if annotation.is_some() && size > MAX_OBJECT {
                return Err(Error::new(
                    "LIMIT_EXCEEDED",
                    "The tag target object exceeds the supported size.",
                ));
            }
            let tagger = annotation
                .as_ref()
                .map(|a| operations::signature(repo, a.author.as_ref()))
                .transpose()?;
            let new_oid = if let Some(annotation) = annotation {
                if annotation.message.trim().is_empty()
                    || annotation.message.len() > 65536
                    || annotation.message.contains('\0')
                {
                    return Err(Error::invalid(
                        "An annotation must be nonempty, NUL-free and at most 64 KiB.",
                    ));
                }
                repo.tag_annotation_create(
                    name,
                    &repo.find_object(target, None).map_err(engine)?,
                    tagger.as_ref().expect("annotation identity"),
                    &annotation.message,
                )
                .map_err(engine)?
            } else {
                target
            };
            tx.set_target(&reference_name, new_oid, tagger.as_ref(), "tag: Newport")
                .map_err(engine)?;
            json!({"name":name,"oid":new_oid.to_string(),"targetOid":target.to_string(),"annotated":annotation.is_some(),"refreshRequired":true})
        }
        Action::TagDelete { expected_oid, .. } => {
            let old = repo.find_reference(&reference_name).map_err(engine)?;
            let expected = oid(expected_oid)?;
            if old.target() != Some(expected) {
                return Err(Error::new(
                    "STALE_REFERENCE",
                    "The tag changed. Refresh tags before deleting it.",
                ));
            }
            tx.remove(&reference_name).map_err(engine)?;
            json!({"deleted":name,"previousOid":expected.to_string(),"refreshRequired":true})
        }
        _ => unreachable!(),
    };
    tx.commit().map_err(|_| {
        Error::new(
            "OUTCOME_UNKNOWN",
            "The tag update could not be confirmed. Inspect the operation before retrying.",
        )
    })?;
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::super::{
        journal::Journal,
        protocol::{Author, Request, TagAnnotation},
        repository::{Output, Service},
    };
    use super::*;
    use std::{fs, os::unix::ffi::OsStrExt};
    use uuid::Uuid;
    fn fixture() -> (tempfile::TempDir, Repository, Oid) {
        let temp = tempfile::tempdir().unwrap();
        let repo = Repository::init(temp.path()).unwrap();
        repo.config()
            .unwrap()
            .set_bool("tag.gpgsign", false)
            .unwrap();
        fs::write(temp.path().join("file"), "base").unwrap();
        let mut index = repo.index().unwrap();
        index.add_path(std::path::Path::new("file")).unwrap();
        index.write().unwrap();
        let tree = repo.find_tree(index.write_tree().unwrap()).unwrap();
        let sig = git2::Signature::now("Fixture", "fixture@example.test").unwrap();
        let oid = repo
            .commit(Some("HEAD"), &sig, &sig, "base", &tree, &[])
            .unwrap();
        drop(tree);
        (temp, repo, oid)
    }
    fn create(name: &str, oid: Oid, message: Option<&str>) -> Action {
        Action::TagCreate {
            name: name.into(),
            target_oid: oid.to_string(),
            annotation: message.map(|message| TagAnnotation {
                message: message.into(),
                author: Some(Author {
                    name: "Fixture".into(),
                    email: "fixture@example.test".into(),
                }),
            }),
        }
    }
    fn run(repo: &Repository, action: &Action) -> Result<Value, Error> {
        operations::apply(repo, action, &[], &repository::fingerprint(repo).unwrap())
    }
    #[test]
    fn lightweight_tags_accept_large_blobs_without_loading_the_target() {
        let (_temp, repo, _) = fixture();
        let blob = repo.blob(&vec![b'x'; MAX_OBJECT + 1]).unwrap();
        let created = run(&repo, &create("large-blob", blob, None)).unwrap();
        assert_eq!(created["oid"], blob.to_string());
        assert_eq!(
            repo.find_reference("refs/tags/large-blob")
                .unwrap()
                .target(),
            Some(blob)
        );
        // Annotated tags still require a bounded native object allocation.
        assert_eq!(
            run(&repo, &create("large-annotation", blob, Some("note")))
                .unwrap_err()
                .code,
            "LIMIT_EXCEEDED"
        );
        assert!(repo.find_reference("refs/tags/large-annotation").is_err());
        assert!(run(&repo, &create("missing", Oid::ZERO_SHA1, None)).is_err());
        assert!(repo.find_reference("refs/tags/missing").is_err());
    }

    #[test]
    fn lightweight_annotated_and_nested_tags_preserve_index_and_files() {
        let (temp, repo, commit) = fixture();
        let index = fs::read(repo.path().join("index")).unwrap();
        fs::write(temp.path().join("file"), "local work").unwrap();
        let light = run(&repo, &create("release/light", commit, None)).unwrap();
        assert_eq!(light["oid"], commit.to_string());
        let annotated = run(
            &repo,
            &create("release/annotated", commit, Some("Release notes")),
        )
        .unwrap();
        let annotation = oid(annotated["oid"].as_str().unwrap()).unwrap();
        assert_ne!(annotation, commit);
        run(&repo, &create("release/nested", annotation, Some("Nested"))).unwrap();
        let rows = list(&repo).unwrap();
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0]["name"]["display"], "release/annotated");
        assert_eq!(rows[0]["message"]["display"], "Release notes");
        assert_eq!(rows[0]["tagger"]["email"], "fixture@example.test");
        assert_eq!(rows[2]["peeledOid"]["hex"], commit.to_string());
        assert_eq!(rows[2]["targetOid"]["hex"], annotation.to_string());
        assert_eq!(fs::read(repo.path().join("index")).unwrap(), index);
        assert_eq!(fs::read(temp.path().join("file")).unwrap(), b"local work");
        assert_eq!(repo.head().unwrap().target(), Some(commit));
        assert_eq!(
            run(&repo, &create("release/light", commit, None))
                .unwrap_err()
                .code,
            "TAG_EXISTS"
        );
        repo.refdb().unwrap().compress().unwrap();
        run(
            &repo,
            &Action::TagDelete {
                name: "release/annotated".into(),
                expected_oid: annotation.to_string(),
            },
        )
        .unwrap();
        assert!(repo.find_reference("refs/tags/release/annotated").is_err());
        assert_eq!(list(&repo).unwrap().len(), 2);
    }
    #[test]
    fn guards_exact_tag_object_stale_worktree_locks_names_and_signing() {
        let (temp, repo, commit) = fixture();
        let tag = run(&repo, &create("v1", commit, Some("first"))).unwrap();
        assert_eq!(
            run(
                &repo,
                &Action::TagDelete {
                    name: "v1".into(),
                    expected_oid: commit.to_string()
                }
            )
            .unwrap_err()
            .code,
            "STALE_REFERENCE"
        );
        let old = tag["oid"].as_str().unwrap();
        let sig = git2::Signature::now("Fixture", "fixture@example.test").unwrap();
        let new = repo
            .tag(
                "v1",
                &repo.find_object(commit, None).unwrap(),
                &sig,
                "replacement",
                true,
            )
            .unwrap();
        assert_eq!(
            run(
                &repo,
                &Action::TagDelete {
                    name: "v1".into(),
                    expected_oid: old.into()
                }
            )
            .unwrap_err()
            .code,
            "STALE_REFERENCE"
        );
        assert_eq!(
            repo.find_reference("refs/tags/v1").unwrap().target(),
            Some(new)
        );
        let stale = repository::fingerprint(&repo).unwrap();
        fs::write(temp.path().join("file"), "changed").unwrap();
        assert_eq!(
            apply(&repo, &create("v2", commit, None), &stale)
                .unwrap_err()
                .code,
            "STALE_SNAPSHOT"
        );
        let mut lock = repo.transaction().unwrap();
        lock.lock_ref("refs/tags/v2").unwrap();
        assert_eq!(
            run(&repo, &create("v2", commit, None)).unwrap_err().code,
            "REPOSITORY_BUSY"
        );
        drop(lock);
        for name in ["", "../escape", "bad:name", "bad name", "bad.lock"] {
            assert_eq!(
                run(&repo, &create(name, commit, None)).unwrap_err().code,
                "INVALID_REQUEST"
            );
        }
        assert_eq!(
            run(&repo, &create("v2", commit, Some("  ")))
                .unwrap_err()
                .code,
            "INVALID_REQUEST"
        );
        repo.config()
            .unwrap()
            .set_bool("tag.gpgsign", true)
            .unwrap();
        assert_eq!(
            run(&repo, &create("v2", commit, Some("signed")))
                .unwrap_err()
                .code,
            "UNSUPPORTED_SIGNING"
        );
        assert!(repo.find_reference("refs/tags/v2").is_err());
    }
    #[test]
    fn bare_blob_tags_and_large_external_annotations_are_bounded() {
        let temp = tempfile::tempdir().unwrap();
        let repo = Repository::init_bare(temp.path()).unwrap();
        repo.config()
            .unwrap()
            .set_bool("tag.gpgsign", false)
            .unwrap();
        let blob = repo.blob(b"small blob").unwrap();
        run(&repo, &create("blob", blob, None)).unwrap();
        let sig = git2::Signature::now("Fixture", "fixture@example.test").unwrap();
        repo.tag(
            "large",
            &repo.find_object(blob, None).unwrap(),
            &sig,
            &"x".repeat(MAX_OBJECT + 1),
            false,
        )
        .unwrap();
        let rows = list(&repo).unwrap();
        assert_eq!(rows[0]["peeledType"], "blob");
        assert_eq!(rows[1]["annotated"], true);
        assert_eq!(rows[1]["detailsOmitted"], true);
        assert!(rows[1]["message"].is_null());
        let large_blob = repo.blob(&vec![0; MAX_OBJECT + 1]).unwrap();
        assert_eq!(
            run(&repo, &create("too-large", large_blob, Some("annotation")))
                .unwrap_err()
                .code,
            "LIMIT_EXCEEDED"
        );
        run(
            &repo,
            &Action::TagDelete {
                name: "blob".into(),
                expected_oid: blob.to_string(),
            },
        )
        .unwrap();
    }
    fn request(service: &mut Service, request: Request) -> Value {
        match service.request(request).unwrap() {
            Output::Json(v) => v,
            _ => panic!("expected JSON"),
        }
    }
    #[test]
    fn pagination_is_anchored_and_journal_replays_after_reconnect() {
        let (temp, repo, commit) = fixture();
        let records = tempfile::tempdir().unwrap();
        let journal =
            Journal::open(records.path().join("journal"), Uuid::new_v4().to_string()).unwrap();
        let mut service = Service::with_journal(journal.clone());
        let open = || Request::Open {
            path: WirePath::new(temp.path().as_os_str().as_bytes()),
        };
        let handle = request(&mut service, open())["repoId"]
            .as_str()
            .unwrap()
            .to_owned();
        let status = request(
            &mut service,
            Request::Status {
                filter: None,
                repo_id: handle.clone(),
                page_size: 100,
                cursor: None,
            },
        );
        let mut create = Request::Start {
            operation_id: Uuid::new_v4().to_string(),
            repo_id: handle.clone(),
            expected_snapshot: status["snapshot"].as_str().unwrap().into(),
            action: create("b", commit, Some("recorded")),
        };
        let result = request(&mut service, create.clone());
        assert_eq!(result["state"], "succeeded", "{result}");
        run(&repo, &self::create("a", commit, None)).unwrap();
        let first = request(
            &mut service,
            Request::Tags {
                message_bytes: None,
                repo_id: handle.clone(),
                page_size: 1,
                cursor: None,
            },
        );
        run(
            &repo,
            &Action::TagDelete {
                name: "b".into(),
                expected_oid: result["result"]["oid"].as_str().unwrap().into(),
            },
        )
        .unwrap();
        let second = request(
            &mut service,
            Request::Tags {
                message_bytes: None,
                repo_id: handle,
                page_size: 1,
                cursor: Some(first["nextCursor"].as_str().unwrap().into()),
            },
        );
        assert_eq!(second["entries"][0]["name"]["display"], "b");
        assert_eq!(second["snapshot"], first["snapshot"]);
        let mut reconnected = Service::with_journal(journal);
        let handle = request(&mut reconnected, open())["repoId"]
            .as_str()
            .unwrap()
            .to_owned();
        if let Request::Start { repo_id, .. } = &mut create {
            *repo_id = handle;
        }
        assert_eq!(request(&mut reconnected, create), result);
        assert!(
            repo.find_reference("refs/tags/b").is_err(),
            "replay must not recreate the deleted tag"
        );
    }
}
