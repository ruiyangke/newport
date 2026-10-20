//! Explicit, immutable deployment. Normal runtime startup never calls this module.
use sha2::{Digest, Sha256};
#[path = "deployment_operations.rs"]
mod operations;
#[path = "deployment_removal.rs"]
mod removal;

pub(crate) fn require_service_ready(home: &Path) -> io::Result<()> {
    removal::require_ready(home)
}
use std::{
    env, fs, io,
    io::Read,
    os::unix::fs::{symlink, DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

fn checksum(path: &Path) -> io::Result<String> {
    let mut file = fs::File::open(path)?;
    let mut digest = Sha256::new();
    let mut buffer = [0; 65536];
    loop {
        let size = file.read(&mut buffer)?;
        if size == 0 {
            break;
        }
        digest.update(&buffer[..size]);
    }
    Ok(digest
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
}
pub(crate) fn private_file(path: &Path) -> io::Result<fs::File> {
    let file = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file()
        || metadata.uid() != unsafe { libc::geteuid() }
        || metadata.mode() & 0o077 != 0
        || metadata.nlink() != 1
    {
        return Err(io::Error::other(
            "Needle operation journal is not a private regular file",
        ));
    }
    Ok(file)
}
pub(crate) fn private_directory(path: &Path) -> io::Result<()> {
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(path)?;
    check_directory(path)
}
fn check_directory(path: &Path) -> io::Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_dir()
        || metadata.uid() != unsafe { libc::geteuid() }
        || metadata.mode() & 0o077 != 0
    {
        return Err(io::Error::other(
            "Needle installation directory is not private to this account",
        ));
    }
    Ok(())
}
pub(crate) fn valid_hash(value: &str) -> io::Result<()> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
    {
        return Err(io::Error::other("Invalid Needle build identity"));
    }
    Ok(())
}
struct Installation {
    root: PathBuf,
    active: PathBuf,
    _lock: fs::File,
}
impl Installation {
    fn inspect(home: &Path) -> io::Result<Self> {
        let root = home.join(".local/share/newport/needle");
        check_directory(&root)?;
        let lock = private_file(&root.join("activation.lock"))?;
        acquire_lock(&lock)?;
        Ok(Self {
            root,
            active: home.join(".local/bin/newport-agent"),
            _lock: lock,
        })
    }
    fn open(home: &Path) -> io::Result<Self> {
        let root = home.join(".local/share/newport/needle");
        private_directory(&root)?;
        let lock = fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(root.join("activation.lock"))?;
        let metadata = lock.metadata()?;
        if !metadata.is_file()
            || metadata.uid() != unsafe { libc::geteuid() }
            || metadata.mode() & 0o077 != 0
        {
            return Err(io::Error::other("Invalid Needle activation lock"));
        }
        acquire_lock(&lock)?;
        private_directory(&root.join("builds"))?;
        fs::create_dir_all(home.join(".local/bin"))?;
        Ok(Self {
            root,
            active: home.join(".local/bin/newport-agent"),
            _lock: lock,
        })
    }
    fn binary(&self, hash: &str) -> PathBuf {
        self.root.join("builds").join(hash).join("newport-agent")
    }
    fn verify(&self, hash: &str) -> io::Result<PathBuf> {
        valid_hash(hash)?;
        let path = self.binary(hash);
        let metadata = fs::symlink_metadata(&path)?;
        if !metadata.is_file()
            || metadata.uid() != unsafe { libc::geteuid() }
            || metadata.mode() & 0o077 != 0
            || metadata.mode() & 0o100 == 0
            || checksum(&path)? != hash
        {
            return Err(io::Error::other(
                "Needle immutable build failed verification",
            ));
        }
        Ok(path)
    }
    fn retain(&self, source: &Path, hash: &str) -> io::Result<PathBuf> {
        valid_hash(hash)?;
        let path = self.binary(hash);
        private_directory(path.parent().unwrap())?;
        if fs::symlink_metadata(&path).is_ok() {
            return self.verify(hash);
        }
        let mut temporary = tempfile::NamedTempFile::new_in(path.parent().unwrap())?;
        let mut source = fs::File::open(source)?;
        io::copy(&mut source, temporary.as_file_mut())?;
        temporary
            .as_file()
            .set_permissions(fs::Permissions::from_mode(0o500))?;
        temporary.as_file().sync_all()?;
        if checksum(temporary.path())? != hash {
            return Err(io::Error::other(
                "Needle build checksum changed while staging",
            ));
        }
        temporary
            .persist_noclobber(&path)
            .map_err(io::Error::other)?;
        fs::File::open(path.parent().unwrap())?.sync_all()?;
        Ok(path)
    }
    fn current(&self) -> io::Result<String> {
        let metadata = match fs::symlink_metadata(&self.active) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok("missing".into()),
            Err(error) => return Err(error),
        };
        if metadata.is_symlink() {
            let target = fs::read_link(&self.active)?;
            let hash = target
                .parent()
                .and_then(Path::file_name)
                .and_then(|s| s.to_str())
                .ok_or_else(|| io::Error::other("Unrelated Needle command symlink"))?;
            valid_hash(hash)?;
            if target != self.binary(hash) {
                return Err(io::Error::other(
                    "Cannot replace an unrelated newport-agent symlink",
                ));
            }
            self.verify(hash)?;
            return Ok(hash.into());
        }
        Err(io::Error::other("Cannot replace unrelated newport-agent"))
    }
    fn replace(&self, hash: &str) -> io::Result<()> {
        if hash == "missing" {
            fs::remove_file(&self.active)?;
        } else {
            let target = self.verify(hash)?;
            let staging = tempfile::Builder::new()
                .prefix(".needle-activate-")
                .tempdir_in(self.active.parent().unwrap())?;
            let link = staging.path().join("newport-agent");
            symlink(target, &link)?;
            fs::rename(link, &self.active)?;
        }
        fs::File::open(self.active.parent().unwrap())?.sync_all()
    }
    fn activate(&self, candidate: &str, expected: &str) -> io::Result<()> {
        operations::require_settled(self)?;
        self.switch(candidate, expected)
    }
    fn switch(&self, candidate: &str, expected: &str) -> io::Result<()> {
        self.verify(candidate)?;
        let current = self.current()?;
        if current != expected {
            return Err(io::Error::other(
                "Needle changed since inspection; inspect again before activating",
            ));
        }
        // Retain the previous build, and a recovery record, before the atomic
        // entry-point switch. An interrupted caller can inspect this receipt.
        let mut receipt = tempfile::NamedTempFile::new_in(&self.root)?;
        serde_json::to_writer(
            receipt.as_file_mut(),
            &serde_json::json!({"previous": current, "candidate": candidate}),
        )?;
        receipt.as_file().sync_all()?;
        receipt
            .persist(self.root.join("activation.json"))
            .map_err(io::Error::other)?;
        fs::File::open(&self.root)?.sync_all()?;
        self.replace(candidate)
    }
}
pub(crate) fn acquire_lock(lock: &fs::File) -> io::Result<()> {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match fs2::FileExt::try_lock_exclusive(lock) {
            Ok(()) => return Ok(()),
            Err(error)
                if error.kind() == io::ErrorKind::WouldBlock && Instant::now() < deadline =>
            {
                std::thread::sleep(Duration::from_millis(25));
            }
            Err(error) => {
                return Err(io::Error::other(format!(
                    "Needle installation is busy: {error}"
                )));
            }
        }
    }
}

pub fn run(args: &[String]) -> io::Result<()> {
    if args == ["protocol"] {
        println!(
            "{}",
            serde_json::json!({"version": 2, "helperOperations": true, "helperReview": true, "helperHistory": 1, "standaloneHelperRecovery": 1, "helperRemovalPlan": true, "usageLease": 2, "removalOperations": true, "removalCancellation": true, "removalReview": true, "buildSha256": crate::runtime::build_identity()?})
        );
        return Ok(());
    }
    let home =
        PathBuf::from(env::var_os("HOME").ok_or_else(|| io::Error::other("HOME is required"))?);
    if args == ["helper-history"] {
        // Standalone installation may predate any deployment journal or lock.
        // Observe helpers first: managed deployment intent is persisted before
        // its helper record, so the subsequent owner read filters those IDs.
        let (ownership_recorded, mut operations) =
            crate::helper_transaction::history(&home, &std::collections::HashSet::new())?;
        let owned = operations::helper_owners_at(&home.join(".local/share/newport/needle"))?;
        operations.retain(|row| !owned.contains(&row.operation_id));
        println!(
            "{}",
            serde_json::to_string(&crate::deployment_protocol::HelperHistory {
                version: 1,
                build_sha256: crate::runtime::build_identity()?,
                ownership_recorded,
                operations,
            })?
        );
        return Ok(());
    }
    if !args.first().is_some_and(|s| {
        matches!(
            s.as_str(),
            "remove"
                | "review-removal"
                | "keep-removal"
                | "restore-removal"
                | "removal"
                | "operation"
                | "review"
                | "helper-removal-plan"
                | "review-helper"
        )
    }) {
        removal::require_ready(&home)?;
    }
    let installation = if args.first().is_some_and(|s| {
        matches!(
            s.as_str(),
            "operation"
                | "review"
                | "keep"
                | "helper-removal-plan"
                | "review-helper"
                | "remove"
                | "review-removal"
                | "keep-removal"
                | "restore-removal"
                | "removal"
        )
    }) {
        Installation::inspect(&home)?
    } else {
        Installation::open(&home)?
    };
    // Recheck after acquiring the activation lock: a removal could have
    // completed while this request was waiting for that lock.
    if !args.first().is_some_and(|s| {
        matches!(
            s.as_str(),
            "remove"
                | "review-removal"
                | "keep-removal"
                | "restore-removal"
                | "removal"
                | "operation"
                | "review"
                | "helper-removal-plan"
                | "review-helper"
        )
    }) {
        removal::require_ready(&home)?;
    }
    match args {
        [operation, id, fingerprint] if operation == "restore-helper" => {
            let _usage = crate::installation_use::ServiceLease::acquire()?;
            let id = uuid::Uuid::parse_str(id).map_err(io::Error::other)?;
            let owned = operations::helper_owners(&installation)?;
            println!(
                "{}",
                serde_json::to_string(&crate::helper_transaction::recover_standalone(
                    &home,
                    id,
                    fingerprint,
                    &owned
                )?)?
            );
            Ok(())
        }
        [operation, id, fingerprint] if operation == "review-helper" => {
            let id = uuid::Uuid::parse_str(id).map_err(io::Error::other)?;
            let owned = operations::helper_owners(&installation)?;
            println!(
                "{}",
                serde_json::to_string(&crate::helper_transaction::review_standalone(
                    &home,
                    id,
                    fingerprint,
                    &owned,
                    None
                )?)?
            );
            Ok(())
        }
        [operation, id, fingerprint, index, file_fingerprint] if operation == "keep-helper" => {
            let _usage = crate::installation_use::ServiceLease::acquire()?;
            let id = uuid::Uuid::parse_str(id).map_err(io::Error::other)?;
            let index = index.parse::<usize>().map_err(io::Error::other)?;
            valid_hash(file_fingerprint)?;
            let owned = operations::helper_owners(&installation)?;
            println!(
                "{}",
                serde_json::to_string(&crate::helper_transaction::review_standalone(
                    &home,
                    id,
                    fingerprint,
                    &owned,
                    Some((index, file_fingerprint))
                )?)?
            );
            Ok(())
        }
        [operation, build, flag, id] if operation == "remove" && flag == "--operation" => {
            let id = uuid::Uuid::parse_str(id).map_err(io::Error::other)?;
            println!(
                "{}",
                serde_json::to_string(&removal::remove(&installation, &home, build, id)?)?
            );
            Ok(())
        }
        [operation, id, flag, build] if operation == "review-removal" && flag == "--build" => {
            let id = uuid::Uuid::parse_str(id).map_err(io::Error::other)?;
            println!(
                "{}",
                serde_json::to_string(&removal::review(&installation, &home, id, build, None)?)?
            );
            Ok(())
        }
        [operation, id, flag, build, index, fingerprint]
            if operation == "keep-removal" && flag == "--build" =>
        {
            let id = uuid::Uuid::parse_str(id).map_err(io::Error::other)?;
            let index = index.parse::<usize>().map_err(io::Error::other)?;
            valid_hash(fingerprint)?;
            println!(
                "{}",
                serde_json::to_string(&removal::review(
                    &installation,
                    &home,
                    id,
                    build,
                    Some((index, fingerprint))
                )?)?
            );
            Ok(())
        }
        [operation, id] if operation == "restore-removal" => {
            let id = uuid::Uuid::parse_str(id).map_err(io::Error::other)?;
            println!(
                "{}",
                serde_json::to_string(&removal::restore(&installation, &home, id)?)?
            );
            Ok(())
        }
        [operation, id, flag, build] if operation == "restore-removal" && flag == "--build" => {
            let id = uuid::Uuid::parse_str(id).map_err(io::Error::other)?;
            println!(
                "{}",
                serde_json::to_string(&removal::restore_or_cancel(
                    &installation,
                    &home,
                    id,
                    build
                )?)?
            );
            Ok(())
        }
        [operation, id] if operation == "removal" => {
            let id = uuid::Uuid::parse_str(id).map_err(io::Error::other)?;
            println!(
                "{}",
                serde_json::to_string(&removal::observe(&installation, id)?)?
            );
            Ok(())
        }
        [operation] if operation == "helper-removal-plan" => {
            operations::require_settled(&installation)?;
            println!(
                "{}",
                serde_json::to_string(&crate::helper_transaction::removal_plan(&home)?)?
            );
            Ok(())
        }
        [operation, id] if operation == "review" => {
            let id = uuid::Uuid::parse_str(id).map_err(io::Error::other)?;
            println!(
                "{}",
                serde_json::to_string(&operations::review_helpers(&installation, id, None)?)?
            );
            Ok(())
        }
        [operation, id, index, fingerprint] if operation == "keep" => {
            let id = uuid::Uuid::parse_str(id).map_err(io::Error::other)?;
            let index = index.parse::<usize>().map_err(io::Error::other)?;
            valid_hash(fingerprint)?;
            println!(
                "{}",
                serde_json::to_string(&operations::review_helpers(
                    &installation,
                    id,
                    Some((index, fingerprint))
                )?)?
            );
            Ok(())
        }
        [operation, hash, flag, id] if operation == "helpers" && flag == "--operation" => {
            valid_hash(hash)?;
            if crate::runtime::build_identity()? != *hash {
                return Err(io::Error::other(
                    "Needle helper executable identity changed",
                ));
            }
            let id = uuid::Uuid::parse_str(id).map_err(io::Error::other)?;
            let intent = crate::deployment_protocol::Intent {
                kind: crate::deployment_protocol::Kind::Helpers,
                operation_id: id,
                candidate: hash.clone(),
                previous: hash.clone(),
                integrations: true,
            };
            println!(
                "{}",
                serde_json::to_string(&operations::activate(&installation, intent, || {
                    crate::install::install_quiet(id)
                })?)?
            );
            Ok(())
        }
        [operation, id] if operation == "operation" => {
            let id = uuid::Uuid::parse_str(id).map_err(io::Error::other)?;
            println!(
                "{}",
                serde_json::to_string(&operations::observe(&installation, id)?)?
            );
            Ok(())
        }
        [operation, id] if operation == "recover" => {
            let id = uuid::Uuid::parse_str(id).map_err(io::Error::other)?;
            println!(
                "{}",
                serde_json::to_string(&operations::recover(&installation, id)?)?
            );
            Ok(())
        }
        [operation, id] if operation == "reconcile" => {
            let id = uuid::Uuid::parse_str(id).map_err(io::Error::other)?;
            println!(
                "{}",
                serde_json::to_string(&operations::reconcile(&installation, id)?)?
            );
            Ok(())
        }
        [operation, hash, expected, flag, id, rest @ ..]
            if operation == "activate"
                && flag == "--operation"
                && (rest.is_empty() || rest == ["--integrations"]) =>
        {
            let intent = crate::deployment_protocol::Intent {
                kind: crate::deployment_protocol::Kind::Deployment,
                operation_id: uuid::Uuid::parse_str(id).map_err(io::Error::other)?,
                candidate: hash.clone(),
                previous: expected.clone(),
                integrations: !rest.is_empty(),
            };
            let operation_id = intent.operation_id;
            println!(
                "{}",
                serde_json::to_string(&operations::activate(&installation, intent, || {
                    crate::install::install_quiet(operation_id)
                })?)?
            );
            Ok(())
        }
        [operation, hash] if operation == "prepare" => {
            valid_hash(hash)?;
            if crate::runtime::build_identity()? != *hash {
                return Err(io::Error::other("Needle candidate checksum mismatch"));
            }
            installation.retain(&env::current_exe()?, hash).map(|_| ())
        }
        [operation, hash, expected] if operation == "activate" => {
            installation.activate(hash, expected)
        }
        [operation, hash, expected, integration]
            if operation == "activate" && integration == "--integrations" =>
        {
            installation.activate(hash, expected)?;
            if let Err(error) = crate::install::install() {
                return Err(match installation.replace(expected) {
                    Ok(()) => io::Error::other(format!(
                        "Integration setup failed; previous Needle build restored: {error}"
                    )),
                    Err(rollback) => io::Error::other(format!(
                        "Integration setup failed: {error}; rollback could not be confirmed: {rollback}"
                    )),
                });
            }
            Ok(())
        }
        [operation, hash, previous] if operation == "rollback" => {
            operations::require_settled(&installation)?;
            if installation.current()? != *hash {
                return Err(io::Error::other(
                    "Needle changed; refusing to roll back another installation",
                ));
            }
            installation.replace(previous)
        }
        _ => Err(io::Error::other(
            "Expected deployment prepare HASH, activate HASH EXPECTED, or rollback HASH PREVIOUS",
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidate(installation: &Installation, bytes: &[u8]) -> String {
        let file = tempfile::NamedTempFile::new().unwrap();
        fs::write(file.path(), bytes).unwrap();
        let hash = checksum(file.path()).unwrap();
        installation.retain(file.path(), &hash).unwrap();
        hash
    }

    #[test]
    fn activation_retains_previous_build_and_rollback_keeps_both_builds() {
        let home = tempfile::tempdir().unwrap();
        let install = Installation::open(home.path()).unwrap();
        fs::write(&install.active, b"unrelated").unwrap();
        assert!(install.current().is_err());
        fs::remove_file(&install.active).unwrap();
        let before = candidate(&install, b"previous build");
        install.activate(&before, "missing").unwrap();
        let next = candidate(&install, b"new build");
        install.activate(&next, &before).unwrap();
        assert_eq!(install.current().unwrap(), next);
        install.replace(&before).unwrap();
        assert_eq!(install.current().unwrap(), before);
        assert!(
            install.verify(&next).is_ok(),
            "rollback retains the candidate too"
        );
        let receipt: serde_json::Value =
            serde_json::from_slice(&fs::read(install.root.join("activation.json")).unwrap())
                .unwrap();
        assert_eq!(receipt["previous"], before);
        assert_eq!(receipt["candidate"], next);
    }

    #[test]
    fn concurrent_activation_has_one_winner_and_does_not_delete_retained_builds() {
        let home = tempfile::tempdir().unwrap();
        let install = Installation::open(home.path()).unwrap();
        let a = candidate(&install, b"candidate a");
        let b = candidate(&install, b"candidate b");
        drop(install);
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
        let results = std::thread::scope(|scope| {
            let first = scope.spawn(|| {
                barrier.wait();
                Installation::open(home.path())
                    .unwrap()
                    .activate(&a, "missing")
            });
            let second = scope.spawn(|| {
                barrier.wait();
                Installation::open(home.path())
                    .unwrap()
                    .activate(&b, "missing")
            });
            [first.join().unwrap(), second.join().unwrap()]
        });
        assert_eq!(results.iter().filter(|r| r.is_ok()).count(), 1);
        let install = Installation::open(home.path()).unwrap();
        assert!(install.verify(&a).is_ok() && install.verify(&b).is_ok());
        let winner = install.current().unwrap();
        assert_eq!(winner, if results[0].is_ok() { a } else { b });
    }

    #[test]
    fn corrupt_candidates_and_unrelated_commands_never_replace_the_active_entry() {
        let home = tempfile::tempdir().unwrap();
        let install = Installation::open(home.path()).unwrap();
        let first = candidate(&install, b"known good");
        install.activate(&first, "missing").unwrap();
        let bad = candidate(&install, b"candidate later corrupted");
        fs::set_permissions(install.binary(&bad), fs::Permissions::from_mode(0o700)).unwrap();
        fs::write(install.binary(&bad), b"corrupt").unwrap();
        assert!(install.activate(&bad, &first).is_err());
        assert_eq!(install.current().unwrap(), first);
        fs::remove_file(&install.active).unwrap();
        fs::write(&install.active, b"unrelated").unwrap();
        assert!(install.activate(&first, "missing").is_err());
        assert_eq!(fs::read(&install.active).unwrap(), b"unrelated");
        fs::remove_file(&install.active).unwrap();
        symlink("/unrelated/location", &install.active).unwrap();
        assert!(install.activate(&first, "missing").is_err());
        assert_eq!(
            fs::read_link(&install.active).unwrap(),
            Path::new("/unrelated/location")
        );
    }
}
