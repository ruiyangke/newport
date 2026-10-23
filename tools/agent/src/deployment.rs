//! Installing Needle: stage a verified build, then point the entry point at
//! it. That is the whole lifecycle on this side. Installing again converges;
//! nothing is journaled. Uninstalling is Newport's job and needs nothing from
//! here (it removes everything this writes).
use sha2::{Digest, Sha256};
use std::{
    collections::HashSet,
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

/// The lock that serializes installs, kept in Needle's folder.
fn lock_file(root: &Path) -> io::Result<fs::File> {
    let lock = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(root.join("install.lock"))?;
    let metadata = lock.metadata()?;
    if !metadata.is_file()
        || metadata.uid() != unsafe { libc::geteuid() }
        || metadata.mode() & 0o077 != 0
    {
        return Err(io::Error::other("Invalid Needle install lock"));
    }
    Ok(lock)
}

impl Installation {
    fn open(home: &Path) -> io::Result<Self> {
        let root = home.join(".local/share/newport/needle");
        private_directory(&root)?;
        let lock = lock_file(&root)?;
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
            // Its build was deleted: nothing runs there, and installing
            // replaces the link.
            if fs::symlink_metadata(&target).is_err() {
                return Ok("missing".into());
            }
            self.verify(hash)?;
            return Ok(hash.into());
        }
        // An earlier Needle installed as a plain file, by hand or before
        // builds were managed: installing replaces it too.
        if legacy_needle(&self.active, &metadata)? {
            return Ok("missing".into());
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

    /// Points the entry point at `hash`, then removes what an install no
    /// longer needs: other builds (see `prune`) and the journals earlier
    /// Needle versions kept.
    fn install(&self, hash: &str) -> io::Result<()> {
        let current = self.current()?;
        if current != hash {
            self.replace(hash)?;
        }
        for retired in [
            "operations.json",
            "removals.json",
            "activation.json",
            "activation.lock",
            "helpers",
        ] {
            let path = self.root.join(retired);
            match fs::symlink_metadata(&path) {
                Ok(metadata) if metadata.is_dir() => fs::remove_dir_all(&path)?,
                Ok(_) => fs::remove_file(&path)?,
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(error),
            }
        }
        self.prune(&[hash], running_builds().as_ref());
        Ok(())
    }
    /// Removes builds nothing needs any more. Kept: the active build, builds a
    /// process still runs (a runtime keeps launching workers from its own
    /// build, and a replaced one drains), and builds staged recently, which
    /// another Newport may be about to install. Without a list of running
    /// builds nothing is removed. Cleanup is best effort.
    fn prune(&self, keep: &[&str], running: Option<&HashSet<String>>) {
        let Some(running) = running else {
            return;
        };
        let Ok(entries) = fs::read_dir(self.root.join("builds")) else {
            return;
        };
        for entry in entries.flatten() {
            let Some(hash) = entry.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            if valid_hash(&hash).is_err()
                || keep.contains(&hash.as_str())
                || running.contains(&hash)
            {
                continue;
            }
            let Ok(metadata) = fs::symlink_metadata(entry.path()) else {
                continue;
            };
            let recent = metadata
                .modified()
                .ok()
                .and_then(|time| time.elapsed().ok())
                .is_none_or(|age| age < RECENTLY_STAGED);
            if metadata.is_dir() && metadata.uid() == unsafe { libc::geteuid() } && !recent {
                let _ = fs::remove_dir_all(entry.path());
            }
        }
    }
}

/// Whether a plain file is a Needle executable: it names Needle's protocol
/// (`newport-agent/N`). Read, never run, to tell.
fn legacy_needle(path: &Path, metadata: &fs::Metadata) -> io::Result<bool> {
    const LIMIT: u64 = 128 * 1024 * 1024;
    if !metadata.is_file() || metadata.uid() != unsafe { libc::geteuid() } || metadata.len() > LIMIT
    {
        return Ok(false);
    }
    let mut bytes = Vec::new();
    fs::File::open(path)?.take(LIMIT).read_to_end(&mut bytes)?;
    Ok(bytes
        .windows(15)
        .any(|s| s.starts_with(b"newport-agent/") && s[14].is_ascii_digit()))
}

/// How long a staged build is safe from pruning before it's activated.
const RECENTLY_STAGED: Duration = Duration::from_secs(60 * 60);

/// The builds this account's processes are running, from `/proc`. `None`
/// where they can't be listed, so pruning keeps everything.
fn running_builds() -> Option<HashSet<String>> {
    #[cfg(target_os = "linux")]
    {
        let mut builds = HashSet::new();
        for entry in fs::read_dir("/proc").ok()?.flatten() {
            if !entry
                .file_name()
                .to_str()
                .is_some_and(|name| name.bytes().all(|b| b.is_ascii_digit()))
            {
                continue;
            }
            // Other accounts' processes can't be read, and can't run this
            // account's private builds either.
            let Ok(executable) = fs::read_link(entry.path().join("exe")) else {
                continue;
            };
            if let Some(hash) = build_of(&executable) {
                builds.insert(hash);
            }
        }
        Some(builds)
    }
    #[cfg(not(target_os = "linux"))]
    None
}

/// The build a `…/needle/builds/HASH/newport-agent` path runs, also once
/// pruned or replaced (`/proc` then appends " (deleted)").
#[cfg_attr(not(any(test, target_os = "linux")), allow(dead_code))]
fn build_of(executable: &Path) -> Option<String> {
    let name = executable.file_name()?.to_str()?;
    if name != "newport-agent" && name != "newport-agent (deleted)" {
        return None;
    }
    let build = executable.parent()?;
    let builds = build.parent()?;
    let hash = build.file_name()?.to_str()?;
    (builds.file_name()? == "builds"
        && builds.parent()?.file_name()? == "needle"
        && valid_hash(hash).is_ok())
    .then(|| hash.to_owned())
}
/// Cleanup when a runtime starts, so a replaced build that was still running
/// at its update goes once it stopped. Skipped when an install holds the lock;
/// it never waits.
pub(crate) fn prune_retained(home: &Path) {
    let root = home.join(".local/share/newport/needle");
    if check_directory(&root).is_err() {
        return;
    }
    let Ok(lock) = lock_file(&root) else {
        return;
    };
    if fs2::FileExt::try_lock_exclusive(&lock).is_err() {
        return;
    }
    let installation = Installation {
        root,
        active: home.join(".local/bin/newport-agent"),
        _lock: lock,
    };
    if let Ok(active) = installation.current() {
        if active != "missing" {
            installation.prune(&[&active], running_builds().as_ref());
        }
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
            serde_json::json!({"version": 1, "install": true, "buildSha256": crate::runtime::build_identity()?})
        );
        return Ok(());
    }
    let home =
        PathBuf::from(env::var_os("HOME").ok_or_else(|| io::Error::other("HOME is required"))?);
    let installation = Installation::open(&home)?;
    match args {
        [operation, hash] if operation == "prepare" => {
            valid_hash(hash)?;
            if crate::runtime::build_identity()? != *hash {
                return Err(io::Error::other("Needle candidate checksum mismatch"));
            }
            installation.retain(&env::current_exe()?, hash).map(|_| ())
        }
        [operation, hash, rest @ ..]
            if operation == "install" && (rest.is_empty() || rest == ["--integrations"]) =>
        {
            valid_hash(hash)?;
            installation.verify(hash)?;
            installation.install(hash)?;
            let notice = if rest.is_empty() {
                let notices = crate::shell_setup::remove_porthop(&home)?;
                crate::legacy_install::remove(&home)?;
                crate::install::setup_notice(notices)
            } else {
                crate::install::apply()?
            };
            println!("{}", serde_json::json!({"build": hash, "notice": notice}));
            Ok(())
        }
        _ => Err(io::Error::other(
            "Expected deployment prepare HASH or install HASH [--integrations]",
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

    fn age(install: &Installation, hash: &str) {
        let staged = std::time::SystemTime::now() - RECENTLY_STAGED - Duration::from_secs(60);
        fs::File::open(install.root.join("builds").join(hash))
            .unwrap()
            .set_modified(staged)
            .unwrap();
    }

    #[test]
    fn installing_points_the_entry_at_the_build_and_again_changes_nothing() {
        let home = tempfile::tempdir().unwrap();
        let install = Installation::open(home.path()).unwrap();
        let first = candidate(&install, b"first build");
        install.install(&first).unwrap();
        assert_eq!(install.current().unwrap(), first);
        install.install(&first).unwrap();
        assert_eq!(install.current().unwrap(), first);
        let next = candidate(&install, b"next build");
        install.install(&next).unwrap();
        assert_eq!(install.current().unwrap(), next);
    }

    #[test]
    fn installing_replaces_an_earlier_plain_needle_or_a_link_to_a_deleted_build() {
        let home = tempfile::tempdir().unwrap();
        let install = Installation::open(home.path()).unwrap();
        // A Needle installed by hand: a plain file naming Needle's protocol.
        fs::write(&install.active, b"\x7fELF ... newport-agent/5 ...").unwrap();
        let build = candidate(&install, b"managed build");
        install.install(&build).unwrap();
        assert_eq!(install.current().unwrap(), build);
        // Its build deleted under the link.
        fs::remove_dir_all(install.root.join("builds").join(&build)).unwrap();
        assert_eq!(install.current().unwrap(), "missing");
        let next = candidate(&install, b"next build");
        install.install(&next).unwrap();
        assert_eq!(install.current().unwrap(), next);
        // Anything else at the entry point is someone else's.
        fs::remove_file(&install.active).unwrap();
        fs::write(&install.active, b"#!/bin/sh\necho mine\n").unwrap();
        assert!(install.install(&next).is_err());
        assert_eq!(
            fs::read(&install.active).unwrap(),
            b"#!/bin/sh\necho mine\n"
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn an_install_removes_old_builds_and_earlier_journals() {
        let home = tempfile::tempdir().unwrap();
        let install = Installation::open(home.path()).unwrap();
        let old = candidate(&install, b"old build");
        install.install(&old).unwrap();
        for journal in ["operations.json", "removals.json", "activation.json"] {
            fs::write(install.root.join(journal), b"{}").unwrap();
        }
        fs::create_dir(install.root.join("helpers")).unwrap();
        let new = candidate(&install, b"new build");
        age(&install, &old);
        install.install(&new).unwrap();
        assert!(
            install.verify(&old).is_err(),
            "the replaced build is removed"
        );
        assert!(install.verify(&new).is_ok());
        for gone in [
            "operations.json",
            "removals.json",
            "activation.json",
            "helpers",
        ] {
            assert!(!install.root.join(gone).exists(), "{gone} is left");
        }
    }

    #[test]
    fn pruning_keeps_active_running_and_recent_builds() {
        let home = tempfile::tempdir().unwrap();
        let install = Installation::open(home.path()).unwrap();
        let [old, running, recent, active] =
            ["old", "running", "recent", "active"].map(|name| candidate(&install, name.as_bytes()));
        for hash in [&old, &running, &active] {
            age(&install, hash);
        }
        let unrelated = install.root.join("builds/not-a-build");
        fs::create_dir(&unrelated).unwrap();
        install.prune(&[&active], Some(&HashSet::from([running.clone()])));
        assert!(
            install.verify(&old).is_err(),
            "an unused old build is removed"
        );
        for hash in [&running, &recent, &active] {
            assert!(install.verify(hash).is_ok());
        }
        assert!(unrelated.exists(), "only build folders are touched");
        // Not knowing what runs keeps everything.
        let other = candidate(&install, b"other");
        age(&install, &other);
        install.prune(&[&active], None);
        assert!(install.verify(&other).is_ok());
    }

    #[test]
    fn running_builds_are_recognized_by_their_path() {
        let hash = "a".repeat(64);
        let build = PathBuf::from(format!(
            "/home/me/.local/share/newport/needle/builds/{hash}"
        ));
        assert_eq!(build_of(&build.join("newport-agent")), Some(hash.clone()));
        assert_eq!(build_of(&build.join("newport-agent (deleted)")), Some(hash));
        assert_eq!(build_of(Path::new("/usr/bin/newport-agent")), None);
        assert_eq!(build_of(&build.join("other")), None);
        assert_eq!(
            build_of(Path::new("/home/me/builds/short/newport-agent")),
            None
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn running_builds_lists_a_process_that_runs_from_a_build() {
        use std::os::unix::process::CommandExt;
        // Any idle program works as the "build": copy `sleep` into one.
        let sleep = env::split_paths(&env::var_os("PATH").unwrap())
            .map(|directory| directory.join("sleep"))
            .find(|path| path.is_file())
            .expect("sleep on PATH");
        let home = tempfile::tempdir().unwrap();
        let hash = "b".repeat(64);
        let build = home
            .path()
            .join(".local/share/newport/needle/builds")
            .join(&hash);
        fs::create_dir_all(&build).unwrap();
        fs::copy(&sleep, build.join("newport-agent")).unwrap();
        // BusyBox picks the program by the name it's started as.
        let mut process = std::process::Command::new(build.join("newport-agent"))
            .arg0("sleep")
            .arg("30")
            .spawn()
            .unwrap();
        let listed = running_builds().unwrap().contains(&hash);
        // Removing the build doesn't hide the process that still runs it.
        fs::remove_dir_all(&build).unwrap();
        let after_removal = running_builds().unwrap().contains(&hash);
        process.kill().unwrap();
        process.wait().unwrap();
        assert!(listed && after_removal);
        assert!(!running_builds().unwrap().contains(&hash));
    }

    #[test]
    fn concurrent_installs_are_serialized_and_one_build_ends_up_active() {
        let home = tempfile::tempdir().unwrap();
        let install = Installation::open(home.path()).unwrap();
        let a = candidate(&install, b"candidate a");
        let b = candidate(&install, b"candidate b");
        drop(install);
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
        std::thread::scope(|scope| {
            for hash in [&a, &b] {
                let barrier = barrier.clone();
                let home = home.path();
                scope.spawn(move || {
                    barrier.wait();
                    Installation::open(home).unwrap().install(hash).unwrap();
                });
            }
        });
        let install = Installation::open(home.path()).unwrap();
        let active = install.current().unwrap();
        assert!(active == a || active == b);
        assert!(install.verify(&active).is_ok());
    }

    #[test]
    fn corrupt_builds_and_unrelated_commands_never_replace_the_entry() {
        let home = tempfile::tempdir().unwrap();
        let install = Installation::open(home.path()).unwrap();
        let first = candidate(&install, b"known good");
        install.install(&first).unwrap();
        let bad = candidate(&install, b"candidate later corrupted");
        fs::set_permissions(install.binary(&bad), fs::Permissions::from_mode(0o700)).unwrap();
        fs::write(install.binary(&bad), b"corrupt").unwrap();
        assert!(install.verify(&bad).is_err());
        assert_eq!(install.current().unwrap(), first);
        fs::remove_file(&install.active).unwrap();
        fs::write(&install.active, b"unrelated").unwrap();
        assert!(install.install(&first).is_err());
        assert_eq!(fs::read(&install.active).unwrap(), b"unrelated");
        fs::remove_file(&install.active).unwrap();
        symlink("/unrelated/location", &install.active).unwrap();
        assert!(install.install(&first).is_err());
        assert_eq!(
            fs::read_link(&install.active).unwrap(),
            Path::new("/unrelated/location")
        );
    }
}
