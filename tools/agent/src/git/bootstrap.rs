//! Repository creation before a repository handle exists. Publish prepared
//! metadata with a no-replace directory rename; never reinitialize a project.
use super::{
    journal,
    protocol::{Error, Path as WirePath},
};
use std::{
    ffi::{CString, OsStr},
    fs::{self, File, OpenOptions},
    io,
    os::{
        fd::AsRawFd,
        unix::{
            ffi::OsStrExt,
            fs::{MetadataExt, OpenOptionsExt},
        },
    },
    path::{Path, PathBuf},
};
fn io_error(_: io::Error) -> Error {
    Error::new("IO_ERROR", "Cannot prepare the repository directory.")
}
fn unknown() -> Error {
    Error::new("OUTCOME_UNKNOWN","Repository metadata may have been published. Query the operation and inspect the directory before retrying.")
}
pub(super) fn root(path: &WirePath) -> Result<(PathBuf, File, String), Error> {
    let bytes = path.decode()?;
    let normalized: PathBuf = Path::new(OsStr::from_bytes(&bytes)).components().collect();
    let path = normalized.as_path();
    if !path.is_absolute() {
        return Err(Error::invalid("Choose an absolute directory path."));
    }
    let meta = fs::symlink_metadata(path)
        .map_err(|_| Error::new("DIRECTORY_REQUIRED", "Choose an existing directory."))?;
    if !meta.is_dir() {
        return Err(Error::new(
            "DIRECTORY_REQUIRED",
            "Choose a directory, not a file or symbolic link.",
        ));
    }
    let canonical = path.canonicalize().map_err(io_error)?;
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW)
        .open(&canonical)
        .map_err(io_error)?;
    let actual = file.metadata().map_err(io_error)?;
    if actual.dev() != meta.dev() || actual.ino() != meta.ino() {
        return Err(Error::new(
            "PATH_CHANGED",
            "The selected directory changed.",
        ));
    }
    let identity = journal::hash(
        &[
            b"bootstrap:".as_slice(),
            canonical.as_os_str().as_bytes(),
            &actual.dev().to_be_bytes(),
            &actual.ino().to_be_bytes(),
        ]
        .concat(),
    );
    Ok((canonical, file, identity))
}
/// Both paths are relative to a held directory descriptor. The destination
/// must remain absent even if another process creates it after preflight.
pub(super) fn publish_to(directory: &File, source: &Path, destination: &Path) -> Result<(), Error> {
    let source = CString::new(source.as_os_str().as_bytes())
        .map_err(|_| Error::invalid("Invalid metadata path."))?;
    let destination = CString::new(destination.as_os_str().as_bytes())
        .map_err(|_| Error::invalid("Invalid destination path."))?;
    // SAFETY: live directory descriptors and NUL-terminated paths; libc retains
    // neither pointer. NOREPLACE/EXCL guarantees no existing entry is replaced.
    #[cfg(target_os = "linux")]
    let result = unsafe {
        libc::syscall(
            libc::SYS_renameat2,
            directory.as_raw_fd(),
            source.as_ptr(),
            directory.as_raw_fd(),
            destination.as_ptr(),
            libc::RENAME_NOREPLACE,
        )
    };
    #[cfg(target_os = "macos")]
    let result = unsafe {
        libc::renameatx_np(
            directory.as_raw_fd(),
            source.as_ptr(),
            directory.as_raw_fd(),
            destination.as_ptr(),
            libc::RENAME_EXCL,
        )
    };
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    let result = -1;
    if result != 0 {
        let error = io::Error::last_os_error();
        return Err(if error.kind() == io::ErrorKind::AlreadyExists {
            Error::new(
                "ALREADY_REPOSITORY",
                "Git metadata appeared during initialization; it was not replaced.",
            )
        } else {
            io_error(error)
        });
    }
    directory.sync_all().map_err(|_| unknown())
}
