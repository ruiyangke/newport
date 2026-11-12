//! Staging directories a successful installer handoff leaves behind. The
//! installer needs its directory after the app exits, so nothing removes it
//! then; the next start does.
use std::{
    fs, io,
    path::Path,
    time::{Duration, SystemTime},
};

/// Prefix of every staging directory created inside the user's temporary folder.
pub const STAGING_PREFIX: &str = "newport-update-";

/// How long a staging directory may sit untouched before it counts as
/// abandoned. An installer that is still running holds a newer one.
pub const STALE_AFTER: Duration = Duration::from_secs(24 * 60 * 60);

/// Remove abandoned staging directories directly inside `temporary` and return
/// how many went. Only real directories carrying [`STAGING_PREFIX`] qualify:
/// links and files are never followed or touched. A directory the installer
/// still has open fails to delete and stays; any failure is skipped.
pub fn remove_stale_staging(temporary: &Path, now: SystemTime) -> io::Result<usize> {
    let mut removed = 0;
    for entry in fs::read_dir(temporary)? {
        let Ok(entry) = entry else { continue };
        if !entry
            .file_name()
            .to_str()
            .is_some_and(|name| name.starts_with(STAGING_PREFIX))
        {
            continue;
        }
        let Ok(metadata) = fs::symlink_metadata(entry.path()) else {
            continue;
        };
        let abandoned = metadata.is_dir()
            && metadata
                .modified()
                .ok()
                .and_then(|modified| now.duration_since(modified).ok())
                .is_some_and(|age| age >= STALE_AFTER);
        if abandoned && fs::remove_dir_all(entry.path()).is_ok() {
            removed += 1;
        }
    }
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_old_staging_directories_are_removed() {
        let temporary = tempfile::tempdir().unwrap();
        let staging = temporary.path().join(format!("{STAGING_PREFIX}abc"));
        let unrelated = temporary.path().join("other-abc");
        let file = temporary.path().join(format!("{STAGING_PREFIX}file"));
        for directory in [&staging, &unrelated] {
            fs::create_dir(directory).unwrap();
            fs::write(directory.join("newport-setup.exe"), "installer").unwrap();
        }
        fs::write(&file, "not a directory").unwrap();

        let now = SystemTime::now();
        assert_eq!(remove_stale_staging(temporary.path(), now).unwrap(), 0);
        assert!(staging.exists());

        let later = now + STALE_AFTER + Duration::from_secs(1);
        assert_eq!(remove_stale_staging(temporary.path(), later).unwrap(), 1);
        assert!(!staging.exists());
        assert!(unrelated.exists());
        assert!(file.exists());
    }

    #[test]
    #[cfg(unix)]
    fn a_linked_staging_directory_is_left_alone() {
        let temporary = tempfile::tempdir().unwrap();
        let target = tempfile::tempdir().unwrap();
        fs::write(target.path().join("keep"), "data").unwrap();
        let link = temporary.path().join(format!("{STAGING_PREFIX}link"));
        std::os::unix::fs::symlink(target.path(), &link).unwrap();
        let later = SystemTime::now() + STALE_AFTER + Duration::from_secs(1);
        assert_eq!(remove_stale_staging(temporary.path(), later).unwrap(), 0);
        assert!(target.path().join("keep").exists());
    }
}
