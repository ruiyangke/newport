//! Recoverable publication of a verified development export. This lock only
//! serializes this destination's publication, not builds or manifest edits.
use anyhow::{Context, Result, bail, ensure};
use std::{
    fs::{self, File, OpenOptions},
    io,
    path::{Path, PathBuf},
};

fn sibling(destination: &Path, suffix: &str) -> Result<PathBuf> {
    let mut name = destination
        .file_name()
        .context("Export destination has no name")?
        .to_os_string();
    name.push(suffix);
    Ok(destination.with_file_name(name))
}

fn entry(path: &Path) -> io::Result<Option<fs::Metadata>> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => Ok(Some(metadata)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

fn lock(destination: &Path) -> Result<File> {
    let path = sibling(destination, ".publish.lock")?;
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&path)
        .with_context(|| format!("Cannot open export publication lock {}", path.display()))?;
    file.try_lock().with_context(|| {
        format!(
            "Cannot acquire export publication lock {}; another publisher may be active",
            path.display()
        )
    })?;
    // Never unlink this file: another process may already have opened its inode.
    Ok(file)
}

pub(super) fn publish(
    fresh: &Path,
    destination: &Path,
    validate: impl FnOnce(&Path) -> Result<PathBuf>,
) -> Result<PathBuf> {
    publish_with(fresh, destination, validate, |from, to| {
        fs::rename(from, to)
    })
}

fn publish_with(
    fresh: &Path,
    destination: &Path,
    validate: impl FnOnce(&Path) -> Result<PathBuf>,
    mut rename: impl FnMut(&Path, &Path) -> io::Result<()>,
) -> Result<PathBuf> {
    let _lock = lock(destination)?;
    let previous = sibling(destination, ".previous")?;
    ensure!(
        entry(&previous)?.is_none(),
        "Unfinished export publication at {}; inspect it before retrying",
        previous.display()
    );
    ensure!(
        fs::symlink_metadata(fresh)?.file_type().is_dir(),
        "Staged export must be a real directory"
    );
    let old = entry(destination)?;
    ensure!(
        old.as_ref()
            .is_none_or(|metadata| metadata.file_type().is_dir()),
        "Existing export must be a real directory"
    );
    let had_previous = old.is_some();
    if had_previous {
        rename(destination, &previous).with_context(|| {
            format!(
                "Cannot preserve previous export {} at {}",
                destination.display(),
                previous.display()
            )
        })?;
    }
    if let Err(error) = rename(fresh, destination) {
        let publication = anyhow::Error::new(error).context(format!(
            "Cannot publish export {} at {}",
            fresh.display(),
            destination.display()
        ));
        return Err(recover(
            publication,
            &previous,
            destination,
            had_previous,
            &mut rename,
        ));
    }
    let verified = match validate(destination) {
        Ok(verified) => verified,
        Err(error) => {
            let validation = error.context("Published export failed final validation");
            // Return only our rejected replacement to its now-vacant staging
            // path before restoring the old export. Never remove a destination
            // or a pre-existing staging entry to force recovery.
            let returned = (|| -> Result<()> {
                ensure!(entry(fresh)?.is_none(), "Staging path is no longer vacant");
                rename(destination, fresh).context("Cannot return rejected export to staging")
            })();
            if let Err(rollback) = returned {
                if had_previous {
                    bail!(
                        "{validation:#}; recovery failed: {rollback:#}; previous export preserved at {}",
                        previous.display()
                    );
                }
                bail!(
                    "{validation:#}; recovery failed: {rollback:#}; rejected export remains at {}",
                    destination.display()
                );
            }
            return Err(recover(
                validation,
                &previous,
                destination,
                had_previous,
                &mut rename,
            ));
        }
    };
    if had_previous {
        fs::remove_dir_all(&previous).with_context(|| {
            format!(
                "New export is published and verified at {}, but previous-export cleanup failed at {}",
                destination.display(),
                previous.display()
            )
        })?;
    }
    Ok(verified)
}

fn recover(
    failure: anyhow::Error,
    previous: &Path,
    destination: &Path,
    had_previous: bool,
    rename: &mut impl FnMut(&Path, &Path) -> io::Result<()>,
) -> anyhow::Error {
    if !had_previous {
        return failure;
    }
    let restored = (|| -> Result<()> {
        ensure!(
            entry(destination)?.is_none(),
            "Destination is no longer vacant; refusing to overwrite it"
        );
        rename(previous, destination).context("Cannot restore previous export")
    })();
    match restored {
        Ok(()) => failure.context(format!(
            "Previous export restored at {}",
            destination.display()
        )),
        Err(rollback) => anyhow::anyhow!(
            "{failure:#}; recovery failed: {rollback:#}; previous export preserved at {}",
            previous.display()
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fixture {
        root: tempfile::TempDir,
        stage: tempfile::TempDir,
        fresh: PathBuf,
        destination: PathBuf,
    }
    impl Fixture {
        fn new(old: bool) -> Self {
            let root = tempfile::tempdir().unwrap();
            let stage = tempfile::tempdir_in(root.path()).unwrap();
            let fresh = stage.path().join("resources");
            let destination = root.path().join("desktop-resources");
            fs::create_dir(&fresh).unwrap();
            fs::write(fresh.join("binary"), "new binary").unwrap();
            fs::write(fresh.join("provenance"), "new provenance").unwrap();
            if old {
                fs::create_dir(&destination).unwrap();
                fs::write(destination.join("binary"), "known good binary").unwrap();
                fs::write(destination.join("stale"), "known good metadata").unwrap();
            }
            Self {
                root,
                stage,
                fresh,
                destination,
            }
        }
        fn previous(&self) -> PathBuf {
            sibling(&self.destination, ".previous").unwrap()
        }
        fn old_at(path: &Path) {
            assert_eq!(fs::read(path.join("binary")).unwrap(), b"known good binary");
            assert_eq!(
                fs::read(path.join("stale")).unwrap(),
                b"known good metadata"
            );
        }
    }
    fn valid(path: &Path) -> Result<PathBuf> {
        assert_eq!(fs::read(path.join("binary"))?, b"new binary");
        assert_eq!(fs::read(path.join("provenance"))?, b"new provenance");
        Ok(path.to_owned())
    }
    fn injected(message: &str) -> io::Error {
        io::Error::other(message)
    }

    #[test]
    fn replaces_complete_export_and_keeps_backup_until_validation() {
        for old in [false, true] {
            let f = Fixture::new(old);
            let result = publish(&f.fresh, &f.destination, |path| {
                if old {
                    Fixture::old_at(&f.previous());
                }
                valid(path)
            })
            .unwrap();
            assert_eq!(result, f.destination);
            assert!(!f.fresh.exists());
            assert!(!f.previous().exists());
            assert!(!f.destination.join("stale").exists());
            assert!(sibling(&f.destination, ".publish.lock").unwrap().is_file());
        }
    }

    #[test]
    fn initial_backup_failure_leaves_old_and_staged_exports_untouched() {
        let f = Fixture::new(true);
        let error = publish_with(
            &f.fresh,
            &f.destination,
            |_| panic!("must not validate"),
            |_, _| Err(injected("initial move failed")),
        )
        .unwrap_err();
        assert!(format!("{error:#}").contains("initial move failed"));
        Fixture::old_at(&f.destination);
        valid(&f.fresh).unwrap();
        assert!(!f.previous().exists());
    }

    #[test]
    fn publication_failure_restores_previous_export() {
        let f = Fixture::new(true);
        let error = publish_with(
            &f.fresh,
            &f.destination,
            |_| panic!("must not validate"),
            |from, to| {
                if from == f.fresh {
                    return Err(injected("publish failed"));
                }
                fs::rename(from, to)
            },
        )
        .unwrap_err();
        assert!(format!("{error:#}").contains("publish failed"));
        Fixture::old_at(&f.destination);
        valid(&f.fresh).unwrap();
        assert!(!f.previous().exists());
    }

    #[test]
    fn failed_publication_and_rollback_preserve_backup_after_stage_drop() {
        let f = Fixture::new(true);
        let previous = f.previous();
        let error = publish_with(
            &f.fresh,
            &f.destination,
            |_| panic!("must not validate"),
            |from, to| {
                if from == f.fresh {
                    return Err(injected("publish failed"));
                }
                if from == previous {
                    return Err(injected("restore failed"));
                }
                fs::rename(from, to)
            },
        )
        .unwrap_err();
        let message = format!("{error:#}");
        assert!(message.contains("publish failed") && message.contains("restore failed"));
        assert!(message.contains(previous.to_str().unwrap()));
        drop(f.stage);
        Fixture::old_at(&previous);
        assert!(!f.destination.exists());
        assert!(f.root.path().exists());
    }

    #[test]
    fn final_validation_failure_restores_old_or_removes_first_rejected_publication() {
        for old in [false, true] {
            let f = Fixture::new(old);
            let error =
                publish(&f.fresh, &f.destination, |_| bail!("validation failed")).unwrap_err();
            assert!(format!("{error:#}").contains("validation failed"));
            if old {
                Fixture::old_at(&f.destination);
            } else {
                assert!(!f.destination.exists());
            }
            valid(&f.fresh).unwrap();
            assert!(!f.previous().exists());
        }
    }

    #[test]
    fn final_validation_recovery_failures_keep_previous_outside_stage() {
        for fail_return in [false, true] {
            let f = Fixture::new(true);
            let previous = f.previous();
            let mut published = false;
            let error = publish_with(
                &f.fresh,
                &f.destination,
                |_| bail!("validation failed"),
                |from, to| {
                    if from == f.fresh {
                        published = true;
                    }
                    if published
                        && ((fail_return && from == f.destination)
                            || (!fail_return && from == previous))
                    {
                        return Err(injected("recovery move failed"));
                    }
                    fs::rename(from, to)
                },
            )
            .unwrap_err();
            let message = format!("{error:#}");
            assert!(
                message.contains("validation failed") && message.contains("recovery move failed")
            );
            assert!(message.contains(previous.to_str().unwrap()));
            drop(f.stage);
            Fixture::old_at(&previous);
            if fail_return {
                valid(&f.destination).unwrap();
            } else {
                assert!(!f.destination.exists());
            }
            assert!(f.root.path().exists());
        }
    }

    #[test]
    fn preexisting_backup_file_or_directory_refuses_without_changes() {
        for directory in [false, true] {
            let f = Fixture::new(true);
            let previous = f.previous();
            if directory {
                fs::create_dir(&previous).unwrap();
            } else {
                fs::write(&previous, "keep").unwrap();
            }
            let error =
                publish(&f.fresh, &f.destination, |_| panic!("must not validate")).unwrap_err();
            assert!(error.to_string().contains("inspect it before retrying"));
            Fixture::old_at(&f.destination);
            valid(&f.fresh).unwrap();
            if directory {
                assert!(previous.is_dir());
            } else {
                assert_eq!(fs::read(&previous).unwrap(), b"keep");
            }
        }
    }

    #[cfg(unix)]
    #[test]
    fn dangling_backup_symlink_is_preserved_and_refused() {
        let f = Fixture::new(true);
        std::os::unix::fs::symlink("missing", f.previous()).unwrap();
        let error = publish(&f.fresh, &f.destination, |_| panic!("must not validate")).unwrap_err();
        assert!(error.to_string().contains("inspect it before retrying"));
        assert_eq!(fs::read_link(f.previous()).unwrap(), Path::new("missing"));
        Fixture::old_at(&f.destination);
        valid(&f.fresh).unwrap();
    }

    #[test]
    fn contending_publisher_refuses_and_lock_file_persists_for_reuse() {
        let f = Fixture::new(true);
        let held = lock(&f.destination).unwrap();
        let lock_path = sibling(&f.destination, ".publish.lock").unwrap();
        let error = publish(&f.fresh, &f.destination, |_| panic!("must not validate")).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("another publisher may be active")
        );
        Fixture::old_at(&f.destination);
        valid(&f.fresh).unwrap();
        assert!(lock_path.is_file());
        drop(held);
        publish(&f.fresh, &f.destination, valid).unwrap();
        assert!(lock_path.is_file());
    }
}
