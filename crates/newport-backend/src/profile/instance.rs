//! Process ownership of one profile directory.
use fs2::FileExt;
use std::{
    fs::{File, OpenOptions},
    io,
    path::{Path, PathBuf},
    sync::Mutex,
};

/// The per-profile lock file. It stays on disk: removing it could allow two
/// processes to lock different inodes.
pub(super) const LOCK_FILE: &str = "newport.lock";

/// Keep this guard alive until all services have stopped.
pub struct Instance(File, Mutex<bool>);

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
    pub fn acquire(directory: &Path) -> io::Result<Option<Self>> {
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(lock_path(directory))?;
        match file.try_lock_exclusive() {
            Ok(()) => Ok(Some(Self(file, Mutex::new(true)))),
            Err(error) if error.raw_os_error() == fs2::lock_contended_error().raw_os_error() => {
                Ok(None)
            }
            Err(error) => Err(error),
        }
    }

    /// Release only after draining services, immediately before handing off to
    /// a replacement process. Normal shutdown releases ownership on drop.
    pub fn release_for_restart(&self) -> io::Result<()> {
        let mut held = self.1.lock().unwrap_or_else(|error| error.into_inner());
        if *held {
            FileExt::unlock(&self.0)?;
            *held = false;
        }
        Ok(())
    }
}

fn lock_path(directory: &Path) -> PathBuf {
    directory.join(LOCK_FILE)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contention_and_drop_preserve_single_ownership() {
        let directory = tempfile::tempdir().unwrap();
        let first = Instance::acquire(directory.path()).unwrap().unwrap();
        assert!(Instance::acquire(directory.path()).unwrap().is_none());
        drop(first);
        let second = Instance::acquire(directory.path()).unwrap().unwrap();
        assert!(directory.path().join("newport.lock").exists());
        drop(second);
    }

    #[test]
    fn restart_handoff_does_not_release_replacement_ownership() {
        let directory = tempfile::tempdir().unwrap();
        let first = Instance::acquire(directory.path()).unwrap().unwrap();
        first.release_for_restart().unwrap();
        first.release_for_restart().unwrap();
        let replacement = Instance::acquire(directory.path()).unwrap().unwrap();
        drop(first);
        assert!(Instance::acquire(directory.path()).unwrap().is_none());
        drop(replacement);
    }

    #[test]
    fn owner_drop_unlocks_even_while_a_child_has_an_inherited_descriptor() {
        let directory = tempfile::tempdir().unwrap();
        let owner = Instance::acquire(directory.path()).unwrap().unwrap();
        // A duplicate models a child between fork and exec. Closing only the
        // parent's descriptor leaves its flock held by that duplicate on Unix.
        let _inherited = owner.0.try_clone().unwrap();
        drop(owner);
        assert!(Instance::acquire(directory.path()).unwrap().is_some());
    }

    #[test]
    fn separate_profiles_can_run_together_and_io_errors_remain_errors() {
        let first = tempfile::tempdir().unwrap();
        let second = tempfile::tempdir().unwrap();
        let _first = Instance::acquire(first.path()).unwrap().unwrap();
        let _second = Instance::acquire(second.path()).unwrap().unwrap();
        assert!(Instance::acquire(&first.path().join("missing")).is_err());
    }
}
