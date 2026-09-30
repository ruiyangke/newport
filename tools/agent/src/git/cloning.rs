//! Journaled clone into a private sibling, followed by no-replace publication.
use super::{
    bootstrap,
    journal::{self, Journal},
    protocol::{Error, Path as WirePath},
    remotes,
};
use serde_json::{json, Value};
use std::{
    ffi::OsStr,
    fs::{self, File},
    os::unix::{ffi::OsStrExt, fs::MetadataExt},
    path::{Component, Path},
};

fn io_error(_: std::io::Error) -> Error {
    Error::new("IO_ERROR", "Cannot prepare the clone destination.")
}
fn unknown() -> Error {
    Error::new(
        "OUTCOME_UNKNOWN",
        "The clone may have been published. Inspect the operation and destination before retrying.",
    )
}
fn absent(path: &Path) -> Result<(), Error> {
    match fs::symlink_metadata(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(io_error(e)),
        Ok(_) => Err(Error::new(
            "PATH_EXISTS",
            "The clone destination already exists and will not be replaced.",
        )),
    }
}
pub(super) fn sync_tree(path: &Path, count: &mut usize) -> Result<(), Error> {
    *count += 1;
    if *count > 250_000 {
        return Err(Error::new(
            "LIMIT_EXCEEDED",
            "The clone exceeds the publication entry limit.",
        ));
    }
    let meta = fs::symlink_metadata(path).map_err(io_error)?;
    if meta.is_symlink() {
        return Ok(());
    }
    if meta.is_dir() {
        for entry in fs::read_dir(path).map_err(io_error)? {
            sync_tree(&entry.map_err(io_error)?.path(), count)?;
        }
    } else if !meta.is_file() {
        return Err(Error::new(
            "UNSUPPORTED_CAPABILITY",
            "The clone contains an unsupported special file.",
        ));
    }
    File::open(path)
        .and_then(|f| f.sync_all())
        .map_err(io_error)
}
pub fn clone(
    journal: &Journal,
    operation_id: &str,
    url: &str,
    path: &WirePath,
    branch: Option<&str>,
    bare: bool,
) -> Result<Value, Error> {
    remotes::validate_url(url)?;
    if let Some(branch) = branch {
        super::branches::name(branch)?;
    }
    let hash = journal::hash(&serde_json::to_vec(&json!({"method":"repo.clone","url":url,"path":path.bytes_b64,"branch":branch,"bare":bare})).map_err(|_| Error::invalid("Invalid clone request."))?);
    if journal.existing(operation_id, &hash)?.is_some() {
        return serde_json::to_value(journal.get(operation_id)?).map_err(|_| unknown());
    }
    let bytes = path.decode()?;
    let requested = Path::new(OsStr::from_bytes(&bytes));
    if !requested.is_absolute()
        || requested
            .components()
            .any(|c| matches!(c, Component::ParentDir))
    {
        return Err(Error::invalid(
            "Choose an absolute destination without parent traversal.",
        ));
    }
    let leaf = requested
        .file_name()
        .ok_or_else(|| Error::invalid("Choose a new directory name."))?;
    let parent = requested
        .parent()
        .ok_or_else(|| Error::invalid("Choose a destination parent."))?;
    let (parent, directory, parent_identity) =
        bootstrap::root(&WirePath::new(parent.as_os_str().as_bytes()))?;
    let destination = parent.join(leaf);
    let identity = journal::hash(
        &[
            b"clone:".as_slice(),
            parent_identity.as_bytes(),
            leaf.as_bytes(),
        ]
        .concat(),
    );
    let _lock = journal.lock_repository(&identity)?;
    if let Some(record) = journal.existing(operation_id, &hash)? {
        return serde_json::to_value(record).map_err(|_| unknown());
    }
    absent(&destination)?;
    let mut record = journal.begin(operation_id, hash, identity)?;
    let prepare = || -> Result<Value, Error> {
        let temporary = tempfile::Builder::new()
            .prefix(".newport-clone-")
            .tempdir_in(&parent)
            .map_err(io_error)?;
        let prepared = temporary.path().join("checkout");
        let mut result = remotes::clone_into(url, &prepared, branch, bare)?;
        sync_tree(&prepared, &mut 0)?;
        let before = directory.metadata().map_err(io_error)?;
        let now = fs::symlink_metadata(&parent).map_err(io_error)?;
        if before.dev() != now.dev() || before.ino() != now.ino() {
            return Err(Error::new(
                "PATH_CHANGED",
                "The destination parent moved during cloning.",
            ));
        }
        absent(&destination)?;
        let source = Path::new(
            temporary
                .path()
                .file_name()
                .ok_or_else(|| Error::invalid("Invalid temporary clone path."))?,
        )
        .join("checkout");
        bootstrap::publish_to(&directory, &source, Path::new(leaf)).map_err(|e| {
            if e.code == "ALREADY_REPOSITORY" {
                Error::new(
                    "PATH_EXISTS",
                    "The destination appeared during cloning and was not replaced.",
                )
            } else {
                e
            }
        })?;
        let repo = git2::Repository::open(&destination).map_err(|_| unknown())?;
        if repo.is_bare() != bare
            || (!bare
                && repo.workdir().and_then(|p| p.canonicalize().ok()) != Some(destination.clone()))
        {
            return Err(unknown());
        }
        result["path"] = json!(WirePath::new(destination.as_os_str().as_bytes()));
        result["cloned"] = true.into();
        result["openRequired"] = true.into();
        Ok(result)
    };
    match prepare() {
        Ok(result) => {
            record.state = "succeeded".into();
            record.result = Some(result);
        }
        Err(error) => {
            record.state = if error.code == "OUTCOME_UNKNOWN" {
                "outcome_unknown"
            } else {
                "failed"
            }
            .into();
            record.error = Some(error);
        }
    }
    record.seq += 1;
    journal.save(&record).map_err(|_| unknown())?;
    serde_json::to_value(record).map_err(|_| unknown())
}

#[cfg(test)]
mod tests {
    use super::*;
    use git2::{Repository, Signature};
    use uuid::Uuid;
    fn commit(repo: &Repository, text: &str) -> git2::Oid {
        fs::write(repo.workdir().unwrap().join("a"), text).unwrap();
        let mut index = repo.index().unwrap();
        index
            .add_all(["*"], git2::IndexAddOption::DEFAULT, None)
            .unwrap();
        index.write().unwrap();
        let oid = index.write_tree().unwrap();
        let tree = repo.find_tree(oid).unwrap();
        let sig = Signature::now("Fixture", "fixture@example.test").unwrap();
        let parent = repo.head().ok().and_then(|h| h.peel_to_commit().ok());
        repo.commit(
            Some("HEAD"),
            &sig,
            &sig,
            text,
            &tree,
            &parent.iter().collect::<Vec<_>>(),
        )
        .unwrap()
    }
    fn journal(root: &Path) -> Journal {
        Journal::open(root.join("journal"), Uuid::new_v4().to_string()).unwrap()
    }
    fn path(path: &Path) -> WirePath {
        WirePath::new(path.as_os_str().as_bytes())
    }
    #[test]
    fn clones_default_selected_bare_and_replays_after_move() {
        let temp = tempfile::tempdir().unwrap();
        let source = Repository::init(temp.path().join("source")).unwrap();
        source.set_head("refs/heads/main").unwrap();
        let first = commit(&source, "first");
        source
            .branch("feature", &source.find_commit(first).unwrap(), false)
            .unwrap();
        let latest = commit(&source, "latest");
        let tag = source
            .tag(
                "v1",
                &source.find_object(first, None).unwrap(),
                &Signature::now("Fixture", "fixture@example.test").unwrap(),
                "notes",
                false,
            )
            .unwrap();
        let journal = journal(temp.path());
        let destination = temp.path().join("clone");
        let operation = Uuid::new_v4().to_string();
        let url = source.path().to_str().unwrap();
        let cloned = clone(&journal, &operation, url, &path(&destination), None, false).unwrap();
        assert_eq!(cloned["state"], "succeeded", "{cloned}");
        let repo = Repository::open(&destination).unwrap();
        assert_eq!(repo.head().unwrap().target(), Some(latest));
        assert_eq!(repo.head().unwrap().name().unwrap(), "refs/heads/main");
        assert_eq!(
            repo.find_reference("refs/tags/v1").unwrap().target(),
            Some(tag)
        );
        assert_eq!(
            repo.find_branch("main", git2::BranchType::Local)
                .unwrap()
                .upstream()
                .unwrap()
                .get()
                .name()
                .unwrap(),
            "refs/remotes/origin/main"
        );
        assert_eq!(fs::read(destination.join("a")).unwrap(), b"latest");
        assert!(!repo.path().join("objects/info/alternates").exists());
        fs::rename(&destination, temp.path().join("moved")).unwrap();
        assert_eq!(
            clone(&journal, &operation, url, &path(&destination), None, false).unwrap(),
            cloned
        );
        assert!(!destination.exists());
        let selected = temp.path().join("selected");
        let result = clone(
            &journal,
            &Uuid::new_v4().to_string(),
            url,
            &path(&selected),
            Some("feature"),
            false,
        )
        .unwrap();
        assert_eq!(result["state"], "succeeded", "{result}");
        assert_eq!(fs::read(selected.join("a")).unwrap(), b"first");
        let bare = temp.path().join("bare.git");
        let result = clone(
            &journal,
            &Uuid::new_v4().to_string(),
            url,
            &path(&bare),
            None,
            true,
        )
        .unwrap();
        assert_eq!(result["state"], "succeeded", "{result}");
        let bare = Repository::open(&bare).unwrap();
        assert!(bare.is_bare());
        assert_eq!(bare.head().unwrap().target(), Some(latest));
        assert_eq!(
            bare.find_reference("refs/heads/feature").unwrap().target(),
            Some(first)
        );
        assert_eq!(
            bare.find_reference("refs/tags/v1").unwrap().target(),
            Some(tag)
        );
    }
    #[test]
    fn failures_do_not_publish_or_replace_directories_and_empty_clones_work() {
        let temp = tempfile::tempdir().unwrap();
        let source = Repository::init(temp.path().join("source")).unwrap();
        source.set_head("refs/heads/main").unwrap();
        let journal = journal(temp.path());
        let url = source.path().to_str().unwrap();
        let empty = temp.path().join("empty");
        let result = clone(
            &journal,
            &Uuid::new_v4().to_string(),
            url,
            &path(&empty),
            None,
            false,
        )
        .unwrap();
        assert_eq!(result["state"], "succeeded", "{result}");
        assert_eq!(result["result"]["unborn"], true);
        assert_eq!(
            Repository::open(&empty)
                .unwrap()
                .head()
                .err()
                .unwrap()
                .code(),
            git2::ErrorCode::UnbornBranch
        );
        fs::write(empty.join("keep"), "keep").unwrap();
        assert_eq!(
            clone(
                &journal,
                &Uuid::new_v4().to_string(),
                url,
                &path(&empty),
                None,
                false
            )
            .unwrap_err()
            .code,
            "PATH_EXISTS"
        );
        assert_eq!(fs::read(empty.join("keep")).unwrap(), b"keep");
        commit(&source, "first");
        let missing = temp.path().join("missing");
        let result = clone(
            &journal,
            &Uuid::new_v4().to_string(),
            url,
            &path(&missing),
            Some("absent"),
            false,
        )
        .unwrap();
        assert_eq!(result["error"]["code"], "BRANCH_NOT_FOUND");
        assert!(!missing.exists());
        fs::write(
            source.workdir().unwrap().join(".gitattributes"),
            "a filter=custom\n",
        )
        .unwrap();
        commit(&source, "filtered");
        let filtered = temp.path().join("filtered");
        let result = clone(
            &journal,
            &Uuid::new_v4().to_string(),
            url,
            &path(&filtered),
            None,
            false,
        )
        .unwrap();
        assert_eq!(result["error"]["code"], "UNSUPPORTED_FILTER", "{result}");
        assert!(!filtered.exists());
        assert!(!fs::read_dir(temp.path()).unwrap().any(|e| e
            .unwrap()
            .file_name()
            .as_bytes()
            .starts_with(b".newport-clone-")));
    }
}
