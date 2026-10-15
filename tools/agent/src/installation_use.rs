//! Cross-process admission for services and future destructive maintenance.
//! This coordination file is never unlinked: replacing a live lock would split
//! admission between two inodes. It contains no installation or shell state.
use fs2::FileExt;
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    os::unix::{
        ffi::OsStrExt,
        fs::{DirBuilderExt, MetadataExt, OpenOptionsExt},
    },
    path::{Path, PathBuf},
};

const FORMAT: &[u8] = b"Newport installation usage v1\n";

struct Lease {
    root: PathBuf,
    directory: (u64, u64),
    file: File,
}

impl Drop for Lease {
    fn drop(&mut self) {
        // Closing alone leaves flock held by duplicates inherited during a
        // concurrent fork, until that child execs or exits. The lease belongs
        // to this owner; participating workers acquire independent descriptors.
        let _ = FileExt::unlock(&self.file);
    }
}

pub struct ServiceLease {
    _lease: Lease,
    _legacy: Option<Lease>,
}

pub struct MaintenanceLease(Lease, Option<Lease>);

fn identity(metadata: &fs::Metadata) -> (u64, u64) {
    (metadata.dev(), metadata.ino())
}
fn private(metadata: &fs::Metadata) -> bool {
    metadata.uid() == unsafe { libc::geteuid() } && metadata.mode() & 0o077 == 0
}
fn directory(home: &Path) -> io::Result<PathBuf> {
    crate::paths::private_home_directory(home, ".local/state/newport/usage")
}

fn legacy_directory(home: &Path) -> io::Result<PathBuf> {
    let home = home.canonicalize()?;
    if !home.is_dir() {
        return Err(io::Error::other("Needle HOME is not a directory"));
    }
    let hash: String = Sha256::digest(home.as_os_str().as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    Ok(Path::new("/tmp").join(format!("newport-use-{}-{hash}", unsafe { libc::geteuid() })))
}

impl ServiceLease {
    /// Every service holds its own descriptor, independent of its controller.
    pub fn acquire() -> io::Result<Self> {
        let home = std::env::var_os("HOME").ok_or_else(|| io::Error::other("HOME is required"))?;
        let lease = Lease::acquire(Path::new(&home), false)?;
        crate::deployment::require_service_ready(Path::new(&home))?;
        let legacy = Lease::legacy(Path::new(&home), false)?;
        Ok(Self {
            _lease: lease,
            _legacy: legacy,
        })
    }
}

impl MaintenanceLease {
    /// The caller must retain this lease throughout destructive maintenance and
    /// separately establish compatibility of older installed/running builds.
    /// A successful lease alone cannot account for pre-lease agent versions.
    pub fn acquire(home: &Path) -> io::Result<Self> {
        Ok(Self(
            Lease::acquire(home, true)?,
            Lease::legacy(home, true)?,
        ))
    }

    pub fn validate(&self) -> io::Result<()> {
        self.0.validate()?;
        if let Some(legacy) = &self.1 {
            legacy.validate()?;
        }
        Ok(())
    }
}

impl Lease {
    fn acquire(home: &Path, exclusive: bool) -> io::Result<Self> {
        Self::at(directory(home)?, exclusive)
    }

    // Existing older processes must keep sharing their original inode. Never
    // create a new global temporary path, delete one, or ignore an access error.
    fn legacy(home: &Path, exclusive: bool) -> io::Result<Option<Self>> {
        let root = legacy_directory(home)?;
        match fs::symlink_metadata(&root) {
            Ok(_) => Self::at(root, exclusive).map(Some),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e),
        }
    }

    fn at(root: PathBuf, exclusive: bool) -> io::Result<Self> {
        match fs::DirBuilder::new().mode(0o700).create(&root) {
            Ok(()) => (),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => (),
            Err(error) => return Err(error),
        }
        let metadata = fs::symlink_metadata(&root)?;
        if !metadata.is_dir() || !private(&metadata) {
            return Err(io::Error::other("Invalid Needle usage directory"));
        }
        let directory = identity(&metadata);
        let path = root.join("lease");
        if !path.try_exists()? {
            // Publish initialized bytes atomically so concurrent first users
            // cannot mistake an empty, partially initialized file for corruption.
            let mut temporary = tempfile::NamedTempFile::new_in(&root)?;
            temporary.write_all(FORMAT)?;
            temporary.as_file().sync_all()?;
            match temporary.persist_noclobber(&path) {
                Ok(_) => (),
                Err(error) if error.error.kind() == io::ErrorKind::AlreadyExists => (),
                Err(error) => return Err(error.error),
            }
        }
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(&path)?;
        let metadata = file.metadata()?;
        if !metadata.is_file() || !private(&metadata) || metadata.len() != FORMAT.len() as u64 {
            return Err(io::Error::other("Invalid Needle usage lease"));
        }
        // persist_noclobber may publish by linking and then unlinking its
        // private temporary name. Allow that bounded initialization window,
        // without accepting a permanently multiply-linked coordination file.
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(100);
        while file.metadata()?.nlink() != 1 {
            if std::time::Instant::now() >= deadline {
                return Err(io::Error::other("Needle usage lease has additional links"));
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        let result = if exclusive {
            FileExt::try_lock_exclusive(&file)
        } else {
            FileExt::try_lock_shared(&file)
        };
        result.map_err(|error| {
            if error.kind() == io::ErrorKind::WouldBlock {
                io::Error::new(io::ErrorKind::WouldBlock, if exclusive {
                    "Needle is in use by another runtime or service. Disconnect active clients before removal."
                } else {
                    "Needle maintenance is in progress. Retry after it finishes."
                })
            } else { error }
        })?;
        let mut lease = Self {
            root,
            directory,
            file,
        };
        let mut format = vec![0; FORMAT.len()];
        lease.file.read_exact(&mut format)?;
        if format != FORMAT {
            return Err(io::Error::other("Unsupported Needle usage lease format"));
        }
        lease.validate()?;
        Ok(lease)
    }

    /// Check immediately before destructive writes; a replaced coordination
    /// path is an error, never an invitation to recreate it under a held lease.
    fn validate(&self) -> io::Result<()> {
        let root = fs::symlink_metadata(&self.root)?;
        let path = fs::symlink_metadata(self.root.join("lease"))?;
        if !root.is_dir()
            || !private(&root)
            || identity(&root) != self.directory
            || !path.is_file()
            || !private(&path)
            || path.nlink() != 1
            || identity(&path) != identity(&self.file.metadata()?)
        {
            return Err(io::Error::other("Needle usage ownership changed"));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::{symlink, PermissionsExt};

    #[test]
    fn fresh_installation_uses_home_without_creating_a_legacy_lock() {
        let home = tempfile::tempdir().unwrap();
        let legacy = legacy_directory(home.path()).unwrap();
        let lease = MaintenanceLease::acquire(home.path()).unwrap();
        assert!(home
            .path()
            .join(".local/state/newport/usage/lease")
            .is_file());
        assert!(!legacy.exists());
        lease.validate().unwrap();
    }

    #[test]
    fn maintenance_honors_an_existing_legacy_service_lock() {
        let home = tempfile::tempdir().unwrap();
        let legacy = legacy_directory(home.path()).unwrap();
        let old_service = Lease::at(legacy.clone(), false).unwrap();
        assert_eq!(
            MaintenanceLease::acquire(home.path()).err().unwrap().kind(),
            io::ErrorKind::WouldBlock
        );
        drop(old_service);
        let maintenance = MaintenanceLease::acquire(home.path()).unwrap();
        assert_eq!(
            Lease::at(legacy.clone(), false).err().unwrap().kind(),
            io::ErrorKind::WouldBlock
        );
        maintenance.validate().unwrap();
        drop(maintenance);
        fs::remove_dir_all(legacy).unwrap();
    }

    #[test]
    fn concurrent_first_users_share_one_lock_inode() {
        for _ in 0..8 {
            let home = tempfile::tempdir().unwrap();
            let start = std::sync::Barrier::new(8);
            let acquired = std::sync::Barrier::new(8);
            std::thread::scope(|scope| {
                let tasks: Vec<_> = (0..8)
                    .map(|_| {
                        scope.spawn(|| {
                            start.wait();
                            let result = Lease::acquire(home.path(), false);
                            acquired.wait();
                            result.map(|lease| identity(&lease.file.metadata().unwrap()))
                        })
                    })
                    .collect();
                let identities: Vec<_> = tasks
                    .into_iter()
                    .map(|task| task.join().unwrap().unwrap())
                    .collect();
                assert!(identities.iter().all(|identity| *identity == identities[0]));
            });
            fs::remove_dir_all(directory(home.path()).unwrap()).unwrap();
        }
    }

    #[test]
    fn dropping_owner_releases_lease_even_with_an_inherited_descriptor() {
        let home = tempfile::tempdir().unwrap();
        for exclusive in [false, true] {
            let lease = Lease::acquire(home.path(), exclusive).unwrap();
            // A concurrent fork can temporarily inherit the open file
            // description before exec closes its CLOEXEC descriptor.
            let inherited = lease.file.try_clone().unwrap();
            drop(lease);
            let next = MaintenanceLease::acquire(home.path()).unwrap();
            next.validate().unwrap();
            drop(next);
            drop(inherited);
        }
        fs::remove_dir_all(directory(home.path()).unwrap()).unwrap();
    }

    #[test]
    fn shared_users_exclude_maintenance_and_release_independently() {
        let home = tempfile::tempdir().unwrap();
        let first = Lease::acquire(home.path(), false).unwrap();
        let second = Lease::acquire(home.path(), false).unwrap();
        assert_eq!(
            MaintenanceLease::acquire(home.path()).err().unwrap().kind(),
            io::ErrorKind::WouldBlock
        );
        drop(first);
        assert!(MaintenanceLease::acquire(home.path()).is_err());
        drop(second);
        let maintenance = MaintenanceLease::acquire(home.path()).unwrap();
        assert_eq!(
            Lease::acquire(home.path(), false).err().unwrap().kind(),
            io::ErrorKind::WouldBlock
        );
        let alias = home.path().join("alias");
        symlink(home.path(), &alias).unwrap();
        assert!(Lease::acquire(&alias, false).is_err());
        maintenance.validate().unwrap();
        drop(maintenance);
        assert!(Lease::acquire(home.path(), false).is_ok());
        fs::remove_dir_all(directory(home.path()).unwrap()).unwrap();
    }

    #[test]
    fn refuses_foreign_symlink_and_replaced_coordination_paths() {
        let home = tempfile::tempdir().unwrap();
        let lease = MaintenanceLease::acquire(home.path()).unwrap();
        let root = directory(home.path()).unwrap();
        let path = root.join("lease");
        fs::rename(&path, root.join("original")).unwrap();
        fs::write(&path, FORMAT).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        assert!(lease.validate().is_err());
        drop(lease);
        fs::remove_file(&path).unwrap();
        symlink(root.join("original"), &path).unwrap();
        assert!(MaintenanceLease::acquire(home.path()).is_err());
        fs::remove_file(&path).unwrap();
        fs::rename(root.join("original"), &path).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(MaintenanceLease::acquire(home.path()).is_err());
        fs::remove_dir_all(root).unwrap();
    }
}
