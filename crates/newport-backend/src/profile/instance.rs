//! Process ownership shared by the legacy and native application hosts.
use fs2::FileExt;
use std::{
    fs::{File, OpenOptions},
    io,
    path::{Path, PathBuf},
    sync::Mutex,
};

/// Keep this guard alive until all services have stopped. The lock file stays
/// on disk: removing it could allow two processes to lock different inodes.
pub struct Instance(Vec<File>, Mutex<bool>);

impl Drop for Instance {
    fn drop(&mut self) {
        // A concurrently spawned child may briefly inherit this open file
        // description before exec. Explicit unlock avoids extending ownership
        // beyond this guard while that child still has a descriptor open.
        if let Err(error) = self.release_for_restart() {
            log::error!("Cannot release Newport instance lock: {error}");
        }
    }
}

impl Instance {
    /// `None` means another application owns this profile. Other failures are
    /// errors, so a permissions or filesystem failure cannot start a second host.
    pub fn acquire(directory: &Path, isolated: bool) -> io::Result<Option<Self>> {
        // Always exclude old explicit-profile launches. Production also takes
        // the released app's global lock, including an override that names the
        // actual production directory (or a symlink alias of it).
        let production = !isolated
            || super::default_directory()
                .ok()
                .and_then(|path| path.canonicalize().ok())
                .zip(directory.canonicalize().ok())
                .is_some_and(|(expected, actual)| expected == actual);
        let mut paths = vec![lock_path(directory, true)];
        if production {
            let legacy = lock_path(directory, false);
            if !paths.contains(&legacy) {
                paths.push(legacy);
            }
        }
        Self::acquire_paths(&paths)
    }

    fn acquire_paths(paths: &[PathBuf]) -> io::Result<Option<Self>> {
        let mut owner = Self(Vec::new(), Mutex::new(true));
        for path in paths {
            let file = OpenOptions::new()
                .create(true)
                .truncate(false)
                .read(true)
                .write(true)
                .open(path)?;
            match file.try_lock_exclusive() {
                Ok(()) => owner.0.push(file),
                Err(error)
                    if error.raw_os_error() == fs2::lock_contended_error().raw_os_error() =>
                {
                    return Ok(None);
                }
                Err(error) => return Err(error),
            }
        }
        Ok(Some(owner))
    }

    /// Release only after draining services, immediately before handing off to
    /// a replacement process. Normal shutdown releases ownership on drop.
    pub fn release_for_restart(&self) -> io::Result<()> {
        let mut held = self.1.lock().unwrap_or_else(|error| error.into_inner());
        if *held {
            for file in self.0.iter().rev() {
                FileExt::unlock(file)?;
            }
            *held = false;
        }
        Ok(())
    }
}

fn lock_path(directory: &Path, isolated: bool) -> PathBuf {
    // Retain the original Swift/Tauri lock location for production upgrades.
    if isolated {
        directory.join(super::legacy::INSTANCE_LOCK)
    } else {
        std::env::temp_dir().join(super::legacy::INSTANCE_LOCK)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contention_and_drop_preserve_single_ownership() {
        let directory = tempfile::tempdir().unwrap();
        let first = Instance::acquire(directory.path(), true).unwrap().unwrap();
        assert!(Instance::acquire(directory.path(), true).unwrap().is_none());
        drop(first);
        let second = Instance::acquire(directory.path(), true).unwrap().unwrap();
        assert!(lock_path(directory.path(), true).exists());
        drop(second);
    }

    #[test]
    fn restart_handoff_does_not_release_replacement_ownership() {
        let directory = tempfile::tempdir().unwrap();
        let first = Instance::acquire(directory.path(), true).unwrap().unwrap();
        first.release_for_restart().unwrap();
        first.release_for_restart().unwrap();
        let replacement = Instance::acquire(directory.path(), true).unwrap().unwrap();
        drop(first);
        assert!(Instance::acquire(directory.path(), true).unwrap().is_none());
        drop(replacement);
    }

    #[test]
    fn owner_drop_unlocks_even_while_a_child_has_an_inherited_descriptor() {
        let directory = tempfile::tempdir().unwrap();
        let owner = Instance::acquire(directory.path(), true).unwrap().unwrap();
        // A duplicate models a child between fork and exec. Closing only the
        // parent's descriptor leaves its flock held by that duplicate on Unix.
        let _inherited = owner.0[0].try_clone().unwrap();
        drop(owner);
        assert!(Instance::acquire(directory.path(), true).unwrap().is_some());
    }

    #[test]
    fn isolated_profiles_can_run_together_and_io_errors_remain_errors() {
        let first = tempfile::tempdir().unwrap();
        let second = tempfile::tempdir().unwrap();
        let _first = Instance::acquire(first.path(), true).unwrap().unwrap();
        let _second = Instance::acquire(second.path(), true).unwrap().unwrap();
        assert!(Instance::acquire(&first.path().join("missing"), true).is_err());
        assert_eq!(
            lock_path(first.path(), false),
            std::env::temp_dir().join(super::super::legacy::INSTANCE_LOCK)
        );
    }

    #[test]
    fn native_ownership_excludes_both_released_lock_modes_and_unwinds_contention() {
        let root = tempfile::tempdir().unwrap();
        let profile = root.path().join("profile.lock");
        let legacy = root.path().join("released.lock");
        let both = [profile.clone(), legacy.clone()];
        for old_path in [&profile, &legacy] {
            let old = Instance::acquire_paths(std::slice::from_ref(old_path))
                .unwrap()
                .unwrap();
            assert!(Instance::acquire_paths(&both).unwrap().is_none());
            drop(old);
            let native = Instance::acquire_paths(&both).unwrap().unwrap();
            for path in &both {
                assert!(
                    Instance::acquire_paths(std::slice::from_ref(path))
                        .unwrap()
                        .is_none()
                );
            }
            native.release_for_restart().unwrap();
            let replacement = Instance::acquire_paths(&both).unwrap().unwrap();
            drop(native);
            assert!(Instance::acquire_paths(&both).unwrap().is_none());
            drop(replacement);
        }
    }
}
