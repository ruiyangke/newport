//! Crash-recoverable transient directories. A shared lease protects every user;
//! only an exclusive lease permits deletion. Unrecognized resources are retained.
use fs2::FileExt;
use std::{
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
};

const MARKER: &str = ".newport-lifetime";
const FORMAT: &[u8] = b"Newport transient directory v1\n";

pub(crate) struct Directory {
    path: PathBuf,
    identity: (u64, u64),
    lock: File,
}
fn identity(m: &fs::Metadata) -> (u64, u64) {
    (m.dev(), m.ino())
}
fn private(m: &fs::Metadata) -> bool {
    m.uid() == unsafe { libc::geteuid() } && m.mode() & 0o077 == 0
}
impl Directory {
    pub(crate) fn new(parent: &Path, prefix: &str) -> io::Result<Self> {
        sweep(parent, prefix);
        let directory = tempfile::Builder::new()
            .prefix(prefix)
            .permissions(fs::Permissions::from_mode(0o700))
            .tempdir_in(parent)?;
        // Publish the marker only after taking its lock. A concurrent sweep can
        // never mistake a partially initialized directory for a crashed owner.
        let mut marker = tempfile::NamedTempFile::new_in(directory.path())?;
        FileExt::try_lock_shared(marker.as_file())?;
        marker.write_all(FORMAT)?;
        let marker = marker
            .persist_noclobber(directory.path().join(MARKER))
            .map_err(|e| e.error)?;
        let identity = identity(&fs::symlink_metadata(directory.path())?);
        Ok(Self {
            path: directory.keep(),
            identity,
            lock: marker,
        })
    }
    pub(crate) fn join(path: &Path) -> io::Result<Self> {
        let m = fs::symlink_metadata(path)?;
        if !m.is_dir() || !private(&m) {
            return Err(io::Error::other("Invalid Needle transient directory"));
        }
        let mut lock = OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(path.join(MARKER))?;
        let marker = lock.metadata()?;
        if !marker.is_file()
            || !private(&marker)
            || marker.nlink() != 1
            || marker.len() != FORMAT.len() as u64
        {
            return Err(io::Error::other("Invalid Needle directory lifetime marker"));
        }
        FileExt::try_lock_shared(&lock)?;
        let mut format = vec![0; FORMAT.len()];
        lock.read_exact(&mut format)?;
        if format != FORMAT {
            return Err(io::Error::other("Unknown Needle directory lifetime format"));
        }
        let value = Self {
            path: path.into(),
            identity: identity(&m),
            lock,
        };
        if !value.matches() {
            // No mutation: publication changed while its lease was acquired.
            return Err(io::Error::other(
                "Needle directory changed during attachment",
            ));
        }
        Ok(value)
    }
    fn matches(&self) -> bool {
        fs::symlink_metadata(&self.path).is_ok_and(|m| m.is_dir() && identity(&m) == self.identity)
            && self.lock.metadata().is_ok_and(|held| {
                fs::symlink_metadata(self.path.join(MARKER))
                    .is_ok_and(|m| m.is_file() && identity(&m) == identity(&held))
            })
    }
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }
}
impl Drop for Directory {
    fn drop(&mut self) {
        if FileExt::try_lock_exclusive(&self.lock).is_ok()
            && self.matches()
            && crate::x_endpoint::reclaim(&self.path)
        {
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}
pub(crate) fn sweep(parent: &Path, prefix: &str) {
    let Ok(entries) = fs::read_dir(parent) else {
        return;
    };
    // Keep startup work bounded. Never infer ownership from a directory name
    // alone: join validates the private marker and Drop requires exclusive access.
    for entry in entries.take(4096).flatten() {
        if entry.file_name().to_string_lossy().starts_with(prefix) {
            if let Ok(directory) = Directory::join(&entry.path()) {
                drop(directory);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn directory_survives_owner_exit_until_last_worker_releases_it() {
        let parent = tempfile::tempdir().unwrap();
        let runtime = Directory::new(parent.path(), "runtime-").unwrap();
        let path = runtime.path().to_owned();
        fs::write(path.join("snapshot"), b"content").unwrap();
        let worker = Directory::join(&path).unwrap();
        drop(runtime);
        sweep(parent.path(), "runtime-");
        assert!(path.join("snapshot").exists());
        drop(worker);
        // Concurrent subprocess creation can briefly inherit the CLOEXEC lock
        // descriptor until exec. A busy lease must defer deletion, not bypass
        // ownership; the next sweep reclaims it once the kernel releases it.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
        while path.exists() && std::time::Instant::now() < deadline {
            sweep(parent.path(), "runtime-");
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        assert!(!path.exists());
    }
    #[test]
    fn sweeper_reclaims_abandoned_directories_but_preserves_live_and_unrecognized_data() {
        let parent = tempfile::tempdir().unwrap();
        let live = Directory::new(parent.path(), "runtime-").unwrap();
        let dead = Directory::new(parent.path(), "runtime-").unwrap();
        let dead_path = dead.path.clone();
        // Model the OS releasing a crashed process's lease before Rust cleanup.
        FileExt::unlock(&dead.lock).unwrap();
        let unknown = parent.path().join("runtime-unrecognized");
        fs::create_dir(&unknown).unwrap();
        fs::write(unknown.join("keep"), b"user data").unwrap();
        let link = parent.path().join("runtime-link");
        std::os::unix::fs::symlink(&unknown, &link).unwrap();
        sweep(parent.path(), "runtime-");
        assert!(!dead_path.exists());
        assert!(live.path().exists());
        assert!(unknown.join("keep").exists());
        assert!(link.is_symlink());
    }
}
