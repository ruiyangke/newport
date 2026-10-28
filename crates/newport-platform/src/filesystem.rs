//! Private application files and durable replacement for native services.
use std::{fs::OpenOptions, io, path::Path};

/// Ensure a private directory hierarchy exists, preserving the host's existing
/// SSH-directory behavior. Unlike create_private_directory, this is idempotent.
pub fn create_private_directory_all(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(path)
    }
    #[cfg(windows)]
    {
        std::fs::create_dir_all(path)?;
        protect_directory(path)
    }
}

/// Create a new directory, failing if it exists; apply privacy at creation.
pub fn create_private_directory(path: impl AsRef<Path>) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        std::fs::DirBuilder::new().mode(0o700).create(path)
    }
    #[cfg(windows)]
    {
        crate::security::create_private_directory(path.as_ref())
    }
}

/// Files must be created inside a private directory. Windows inherits its DACL.
pub fn private_file_options() -> OpenOptions {
    #[allow(unused_mut)]
    let mut options = OpenOptions::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options
}

pub fn protect_directory(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
    }
    #[cfg(windows)]
    {
        crate::security::protect_path(path, "OICI")
    }
}
pub fn protect_file(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
    }
    #[cfg(windows)]
    {
        crate::security::protect_path(path, "")
    }
}
pub fn replace_file(source: &Path, destination: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        std::fs::rename(source, destination)
    }
    #[cfg(windows)]
    {
        use windows_sys::Win32::Storage::FileSystem::{
            MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
        };
        let source = crate::security::wide_path(source)?;
        let destination = crate::security::wide_path(destination)?;
        if unsafe {
            MoveFileExW(
                source.as_ptr(),
                destination.as_ptr(),
                MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
            )
        } == 0
        {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    }
}
pub fn sync_directory(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        std::fs::File::open(path)?.sync_all()
    }
    #[cfg(windows)]
    {
        // Windows replacement is flushed by MOVEFILE_WRITE_THROUGH.
        let _ = path;
        Ok(())
    }
}

/// Publish a completed recovery directory on the same filesystem. The caller
/// holds profile ownership and has verified that the destination is absent.
pub fn publish_directory(source: &Path, destination: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        std::fs::rename(source, destination)
    }
    #[cfg(windows)]
    {
        use windows_sys::Win32::Storage::FileSystem::{MOVEFILE_WRITE_THROUGH, MoveFileExW};
        let source = crate::security::wide_path(source)?;
        let destination = crate::security::wide_path(destination)?;
        // sync_directory is a no-op on Windows. Flush publication here instead;
        // omit REPLACE_EXISTING so an existing recovery directory is preserved.
        if unsafe {
            MoveFileExW(
                source.as_ptr(),
                destination.as_ptr(),
                MOVEFILE_WRITE_THROUGH,
            )
        } == 0
        {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, path::PathBuf};

    struct Directory(PathBuf);

    impl Drop for Directory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    // Applying the private DACL must not prevent atomic replacement or
    // subsequent reads.
    #[test]
    fn protected_files_can_be_replaced_and_read() {
        let path = std::env::temp_dir().join(format!("newport-files-{}", uuid::Uuid::new_v4()));
        create_private_directory(&path).unwrap();
        let directory = Directory(path);
        protect_directory(&directory.0).unwrap();
        let target = directory.0.join("vault");
        let staged = directory.0.join("staged");
        fs::write(&target, b"old").unwrap();
        fs::write(&staged, b"new").unwrap();
        protect_file(&staged).unwrap();
        replace_file(&staged, &target).unwrap();
        assert_eq!(fs::read(&target).unwrap(), b"new");
        assert!(!staged.exists());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&target).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }
}
