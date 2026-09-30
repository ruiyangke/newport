//! Share publication and journaling primitives; prepare repositories with Git.
use super::super::{bootstrap, cloning};
use super::*;
use std::{ffi::OsString, path::Component};
pub(super) fn create(
    service: &Service,
    id: &str,
    path: WirePath,
    branch: Option<String>,
    clone: Option<(String, bool)>,
) -> Result<Value, Error> {
    let journal = writes::journal(service)?;
    if let Some((url, _)) = &clone {
        super::super::remotes::validate_url(url)?;
    }
    let payload = pages::hash(&json!({"engine":"cli","path":path,"branch":branch,"clone":clone}))?;
    if journal.existing(id, &payload)?.is_some() {
        return serde_json::to_value(journal.get(id)?).map_err(|_| failure());
    }
    let bytes = path.decode()?;
    let destination = Path::new(OsStr::from_bytes(&bytes));
    if !destination.is_absolute()
        || destination
            .components()
            .any(|c| matches!(c, Component::ParentDir))
    {
        return Err(Error::invalid(
            "Choose an absolute destination without parent traversal.",
        ));
    }
    let (root, leaf) = if clone.is_some() {
        (
            destination.parent().ok_or_else(failure)?,
            destination.file_name().ok_or_else(failure)?,
        )
    } else {
        (destination, OsStr::new(".git"))
    };
    let (root, directory, parent_identity) =
        bootstrap::root(&WirePath::new(root.as_os_str().as_bytes()))?;
    let identity = super::super::journal::hash(
        &[
            if clone.is_some() {
                b"clone:".as_slice()
            } else {
                b"init:".as_slice()
            },
            parent_identity.as_bytes(),
            leaf.as_bytes(),
        ]
        .concat(),
    );
    let _lock = journal.lock_repository(&identity)?;
    if let Some(record) = journal.existing(id, &payload)? {
        return serde_json::to_value(record).map_err(|_| failure());
    }
    if fs::symlink_metadata(root.join(leaf)).is_ok() {
        return Err(Error::new(
            "PATH_EXISTS",
            "The destination already exists and will not be replaced.",
        ));
    }
    if clone.is_none() && command::run(&root, &["rev-parse", "--git-dir"])?.is_some() {
        return Err(Error::new(
            "ALREADY_REPOSITORY",
            "The selected directory is already inside a repository.",
        ));
    }
    let mut record = journal.begin(id, payload, identity)?;
    let result = (|| {
        let temp = tempfile::Builder::new()
            .prefix(".newport-cli-create-")
            .tempdir_in(&root)
            .map_err(io_error)?;
        let prepared = temp.path().join("repository");
        let mut args = Vec::<OsString>::new();
        if let Some((url, bare)) = &clone {
            if url.is_empty() || url.len() > 8192 || url.contains(['\0', '\n', '\r']) {
                return Err(Error::invalid("Invalid clone URL."));
            }
            args.push("clone".into());
            args.push("--no-recurse-submodules".into());
            if *bare {
                args.push("--bare".into());
            }
            if let Some(branch) = &branch {
                args.push(format!("--branch={branch}").into());
            }
            args.push("--".into());
            args.push(url.into());
            args.push(prepared.as_os_str().into());
        } else {
            let branch = branch.as_deref().ok_or_else(failure)?;
            if branch.starts_with('-')
                || command::run(
                    &root,
                    &["check-ref-format", &format!("refs/heads/{branch}")],
                )?
                .is_none()
            {
                return Err(Error::invalid("Invalid initial branch."));
            }
            args.extend([
                "init".into(),
                "--template=".into(),
                format!("--initial-branch={branch}").into(),
                "--".into(),
                prepared.as_os_str().into(),
            ]);
        }
        if command::run_os(&root, &args, vec![])?.is_none() {
            return Err(Error::new(
                "GIT_ERROR",
                "Git could not prepare the repository. The destination was not published.",
            ));
        }
        let source = if clone.is_some() {
            prepared.clone()
        } else {
            prepared.join(".git")
        };
        cloning::sync_tree(&source, &mut 0)?;
        let previous = directory.metadata().map_err(io_error)?;
        let current = fs::metadata(&root).map_err(io_error)?;
        if previous.dev() != current.dev() || previous.ino() != current.ino() {
            return Err(Error::new(
                "PATH_CHANGED",
                "The destination directory changed.",
            ));
        }
        let source = source.strip_prefix(&root).map_err(|_| failure())?;
        bootstrap::publish_to(&directory, source, Path::new(leaf))?;
        if let Some((_, bare)) = &clone {
            Ok(json!({"cloned":true,"path":path,"bare":bare,"openRequired":true}))
        } else {
            Ok(
                json!({"initialized":true,"path":path,"initialBranch":branch,"bare":false,"openRequired":true}),
            )
        }
    })();
    match result {
        Ok(value) => {
            record.state = "succeeded".into();
            record.result = Some(value);
        }
        Err(e) => {
            record.state = if e.code == "OUTCOME_UNKNOWN" {
                "outcome_unknown"
            } else {
                "failed"
            }
            .into();
            record.error = Some(e);
        }
    }
    record.seq += 1;
    journal.save(&record).map_err(|_| {
        Error::new(
            "OUTCOME_UNKNOWN",
            "Creation completed but its outcome could not be saved.",
        )
    })?;
    serde_json::to_value(record).map_err(|_| failure())
}
