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
    if clone.is_none() {
        let branch = branch.as_deref().ok_or_else(failure)?;
        if branch.starts_with('-')
            || command::run(
                Path::new("/"),
                &["check-ref-format", &format!("refs/heads/{branch}")],
            )?
            .is_none()
        {
            return Err(Error::invalid("Invalid initial branch."));
        }
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
    let mut made = MadeFolder(None);
    let (root, leaf) = if clone.is_some() {
        (
            destination.parent().ok_or_else(failure)?,
            destination.file_name().ok_or_else(failure)?,
        )
    } else {
        // As `git init <dir>` does, a new project's folder is made when its
        // parent exists; an existing folder keeps its files.
        match fs::create_dir(destination) {
            Ok(()) => made.0 = Some(destination),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Err(Error::new(
                    "DIRECTORY_REQUIRED",
                    "Choose a folder whose parent folder exists.",
                ));
            }
            Err(error) if error.kind() != std::io::ErrorKind::AlreadyExists => {
                return Err(io_error(error));
            }
            Err(_) => {}
        }
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
            if clone.is_some() {
                "PATH_EXISTS"
            } else {
                "ALREADY_REPOSITORY"
            },
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
    // From here the outcome is journaled, so the folder stays with it.
    made.0 = None;
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
            args.extend([
                "init".into(),
                "--template=".into(),
                format!("--initial-branch={branch}").into(),
                "--".into(),
                prepared.as_os_str().into(),
            ]);
        }
        if let Err(reason) = command::run_os_reason(&root, &args, vec![])? {
            return Err(Error::new(
                "GIT_ERROR",
                git_failure(clone.is_some(), &reason),
            ));
        }
        let cloned_oid = if clone.is_some() {
            command::run(&prepared, &["rev-parse", "--verify", "HEAD"])?
                .map(|b| String::from_utf8_lossy(trim_line(&b)).into_owned())
        } else {
            None
        };
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
            Ok(json!({"cloned":true,"path":path,"bare":bare,"oid":cloned_oid,"openRequired":true}))
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

/// Git's own reason, so a server problem (no CA certificates, no network, a
/// wrong URL) can be fixed there.
fn git_failure(clone: bool, reason: &str) -> String {
    let action = if clone { "clone" } else { "create" };
    let mut message = if reason.is_empty() {
        format!("Git couldn't {action} the repository.")
    } else {
        format!("Git couldn't {action} the repository: {reason}")
    };
    if ["SSL CA cert", "certificate verification", "CAfile"]
        .iter()
        .any(|sign| reason.contains(sign))
    {
        message.push_str(" Install the ca-certificates package on the server, then try again.");
    }
    message
}

/// A folder made for `repo.init`, removed again if the request is refused
/// before it is journaled, so a refusal changes nothing. Only an empty folder
/// is removed.
struct MadeFolder<'a>(Option<&'a Path>);
impl Drop for MadeFolder<'_> {
    fn drop(&mut self) {
        if let Some(folder) = self.0 {
            let _ = fs::remove_dir(folder);
        }
    }
}

#[cfg(test)]
mod failure_tests {
    #[test]
    fn a_missing_certificate_store_is_named_with_its_fix() {
        let message = super::git_failure(
            true,
            "unable to access 'https://github.com/octocat/Spoon-Knife.git/': Problem with the SSL CA cert (path? access rights?)",
        );
        assert!(message.starts_with("Git couldn't clone the repository: unable to access"));
        assert!(
            message.ends_with("Install the ca-certificates package on the server, then try again.")
        );
        assert_eq!(
            super::git_failure(false, ""),
            "Git couldn't create the repository."
        );
    }
}
