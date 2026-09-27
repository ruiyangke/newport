//! One-time UI data adoption before any webview or window-state plugin opens files.
use std::{fs, io, path::Path};

fn copy_tree(source: &Path, target: &Path) -> io::Result<()> {
    crate::platform::filesystem::create_private_directory(target)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        let destination = target.join(entry.file_name());
        if kind.is_dir() {
            copy_tree(&entry.path(), &destination)?;
        } else if kind.is_file() {
            fs::copy(entry.path(), &destination)?;
        } else {
            return Err(io::Error::other(
                "Cannot migrate linked or special UI data files",
            ));
        }
    }
    Ok(())
}

fn adopt(source: &Path, target: &Path) -> io::Result<()> {
    match fs::symlink_metadata(target) {
        Ok(_) => return Ok(()), // Never replace Newport data, even if unreadable.
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }
    match fs::symlink_metadata(source) {
        Ok(m) if m.is_dir() => {}
        Ok(_) => return Err(io::Error::other("Legacy UI data is not a directory")),
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e),
    }
    let parent = target
        .parent()
        .ok_or_else(|| io::Error::other("Missing UI data directory"))?;
    fs::create_dir_all(parent)?;
    let staging = tempfile::Builder::new()
        .prefix(".newport-migration-")
        .tempdir_in(parent)?;
    copy_tree(source, staging.path())?;
    fs::rename(staging.path(), target)?;
    Ok(())
}

pub fn prepare() -> io::Result<()> {
    if super::data_directory_override().is_some() {
        return Ok(());
    }
    // Retain the originals for rollback; the shared instance lock prevents a
    // Porthop process from writing its webview databases during this copy.
    if let Some(base) = dirs::config_dir() {
        adopt(&base.join(super::legacy::APP_ID), &base.join("app.newport"))?;
    }
    #[cfg(target_os = "windows")]
    if let Some(base) = dirs::data_local_dir() {
        adopt(&base.join(super::legacy::APP_ID), &base.join("app.newport"))?;
    }
    #[cfg(target_os = "macos")]
    if let Some(home) = dirs::home_dir() {
        let base = home.join("Library/WebKit");
        adopt(&base.join(super::legacy::APP_ID), &base.join("app.newport"))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn copies_ui_preferences_once_and_preserves_originals() {
        let root = tempfile::tempdir().unwrap();
        let old = root.path().join("old");
        let new = root.path().join("new");
        fs::create_dir_all(old.join("WebsiteData")).unwrap();
        fs::write(old.join("WebsiteData/preferences"), "dark").unwrap();
        adopt(&old, &new).unwrap();
        assert_eq!(
            fs::read(new.join("WebsiteData/preferences")).unwrap(),
            b"dark"
        );
        fs::write(new.join("WebsiteData/preferences"), "light").unwrap();
        adopt(&old, &new).unwrap();
        assert_eq!(
            fs::read(new.join("WebsiteData/preferences")).unwrap(),
            b"light"
        );
        assert_eq!(
            fs::read(old.join("WebsiteData/preferences")).unwrap(),
            b"dark"
        );
    }
    #[cfg(unix)]
    #[test]
    fn refuses_symlinks_without_publishing_partial_data() {
        let root = tempfile::tempdir().unwrap();
        let old = root.path().join("old");
        let new = root.path().join("new");
        fs::create_dir(&old).unwrap();
        std::os::unix::fs::symlink("/", old.join("escape")).unwrap();
        assert!(adopt(&old, &new).is_err());
        assert!(!new.exists());
    }
}
