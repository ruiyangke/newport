//! Private agent state paths and atomic file writes.
use std::{
    env, fs,
    io::{self, Write},
    os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt},
    path::{Path, PathBuf},
};

/// Never follow a substituted directory or loosen an existing directory's mode.
pub(crate) fn private_home_directory(home: &Path, relative: &str) -> io::Result<PathBuf> {
    let mut path = home.canonicalize()?;
    for component in Path::new(relative).components() {
        path.push(component);
        match fs::DirBuilder::new().mode(0o700).create(&path) {
            Ok(()) => (),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => (),
            Err(e) => {
                return Err(io::Error::new(
                    e.kind(),
                    format!("Cannot create {}: {e}", path.display()),
                ))
            }
        }
        let m = fs::symlink_metadata(&path)?;
        if !m.is_dir() || m.uid() != unsafe { libc::geteuid() } || m.mode() & 0o022 != 0 {
            return Err(io::Error::other(format!(
                "Needle directory is not safely owned by this account: {}",
                path.display()
            )));
        }
    }
    if fs::symlink_metadata(&path)?.mode() & 0o077 != 0 {
        return Err(io::Error::other(format!(
            "Needle directory must be private (mode 700): {}",
            path.display()
        )));
    }
    Ok(path)
}

/// Socket paths and temporary data are private to the account, never global /tmp.
pub(crate) fn runtime() -> io::Result<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(root) = env::var_os("XDG_RUNTIME_DIR") {
        candidates.push(PathBuf::from(root));
    }
    #[cfg(target_os = "linux")]
    candidates.push(PathBuf::from(format!("/run/user/{}", unsafe {
        libc::geteuid()
    })));
    // macOS has no XDG runtime directory. Its OS-provided per-user temporary
    // directory is private and keeps Unix socket names below SUN_LEN.
    #[cfg(target_os = "macos")]
    candidates.push(env::temp_dir());
    for root in candidates {
        if root.is_absolute()
            && fs::symlink_metadata(&root).is_ok_and(|m| {
                m.is_dir() && m.uid() == unsafe { libc::geteuid() } && m.mode() & 0o077 == 0
            })
        {
            return private_home_directory(&root, "newport-ipc");
        }
    }
    let home = env::var_os("HOME").ok_or_else(|| io::Error::other("HOME is required"))?;
    private_home_directory(Path::new(&home), ".cache/newport/run")
}

/// The clipboard root a desktop publishes to when it runs without a session registry.
pub(crate) fn standalone_root(home: &Path) -> PathBuf {
    home.join(".cache/newport/clipboard")
}

pub(crate) fn integration_display_path(value: &str) -> bool {
    let Ok(root) = runtime() else {
        return false;
    };
    let Ok(relative) = Path::new(value).strip_prefix(&root) else {
        return false;
    };
    let parts: Vec<_> = relative
        .components()
        .map(|c| c.as_os_str().to_string_lossy())
        .collect();
    match parts.as_slice() {
        [directory, _] => directory.starts_with("wl-"),
        [runtime, integration, _] => runtime.starts_with("r-") && integration.starts_with("i-"),
        _ => false,
    }
}

pub fn root() -> io::Result<PathBuf> {
    let home =
        PathBuf::from(env::var_os("HOME").ok_or_else(|| io::Error::other("HOME is required"))?);
    let root = standalone_root(&home);
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(&root)?;
    let meta = fs::symlink_metadata(&root)?;
    if !meta.is_dir() || meta.uid() != unsafe { libc::geteuid() } || meta.mode() & 0o077 != 0 {
        return Err(io::Error::other(
            "clipboard directory must be owned by this account with mode 700",
        ));
    }
    Ok(root)
}
pub fn private_write(path: &std::path::Path, bytes: &[u8]) -> io::Result<()> {
    let mut file = tempfile::NamedTempFile::new_in(path.parent().unwrap())?;
    file.as_file()
        .set_permissions(fs::Permissions::from_mode(0o600))?;
    file.write_all(bytes)?;
    file.persist(path).map_err(|e| e.error)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn private_directories_preserve_parents_and_refuse_links_or_public_state() {
        let home = tempfile::tempdir().unwrap();
        let root = private_home_directory(home.path(), ".cache/newport/run").unwrap();
        assert_eq!(fs::metadata(&root).unwrap().mode() & 0o777, 0o700);
        let parent = home.path().join(".cache");
        fs::set_permissions(&parent, fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(
            private_home_directory(home.path(), ".cache/newport/run").unwrap(),
            root
        );
        assert_eq!(fs::metadata(parent).unwrap().mode() & 0o777, 0o755);
        fs::set_permissions(&root, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(private_home_directory(home.path(), ".cache/newport/run").is_err());
        fs::remove_dir(&root).unwrap();
        let other = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(other.path(), &root).unwrap();
        assert!(private_home_directory(home.path(), ".cache/newport/run").is_err());
        assert_eq!(fs::read_dir(other.path()).unwrap().count(), 0);
    }
}
