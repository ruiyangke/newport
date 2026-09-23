//! Ownership and recovery of private Unix socket paths.
use fs2::FileExt;
use socket2::{Domain, SockAddr, Socket, Type};
use std::{
    fs::{self, File, OpenOptions},
    io,
    os::unix::{
        fs::{DirBuilderExt, FileTypeExt, MetadataExt, OpenOptionsExt, PermissionsExt},
        net::UnixListener,
    },
    path::{Path, PathBuf},
};

pub(crate) struct Endpoint {
    path: PathBuf,
    identity: (u64, u64),
    // Keep the stable lock inode for the entire listener lifetime. Never unlink it.
    _lock: Option<File>,
    _temporary: Option<tempfile::TempDir>,
}
impl Endpoint {
    pub(crate) fn temporary() -> io::Result<(UnixListener, Self)> {
        let directory = tempfile::Builder::new()
            .prefix("porthop-wl-")
            .permissions(fs::Permissions::from_mode(0o700))
            .tempdir_in("/tmp")?;
        Self::bind(directory.path().join("clipboard"), None, Some(directory))
    }
    pub(crate) fn fixed(path: &Path) -> io::Result<(UnixListener, Self)> {
        if !path.is_absolute() {
            return Err(io::Error::other("the Wayland socket path must be absolute"));
        }
        let parent = path
            .parent()
            .ok_or_else(|| io::Error::other("invalid socket path"))?;
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(parent)?;
        let metadata = fs::symlink_metadata(parent)?;
        let uid = unsafe { libc::geteuid() };
        if !metadata.is_dir() || metadata.uid() != uid || metadata.mode() & 0o077 != 0 {
            return Err(io::Error::other("the socket directory must belong to this account, have mode 700, and not be a symlink"));
        }
        let mut lock_name = path.as_os_str().to_owned();
        lock_name.push(".lock");
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(PathBuf::from(lock_name))?;
        let m = lock.metadata()?;
        if !m.is_file() || m.uid() != uid || m.nlink() != 1 || m.mode() & 0o077 != 0 {
            return Err(io::Error::other("invalid Wayland socket lock"));
        }
        FileExt::try_lock_exclusive(&lock).map_err(|e| {
            if e.kind() == io::ErrorKind::WouldBlock {
                already_running()
            } else {
                e
            }
        })?;
        match fs::symlink_metadata(path) {
            Ok(m) => {
                if !m.file_type().is_socket() || m.uid() != uid {
                    return Err(io::Error::other(
                        "refusing to replace a file or symlink at the socket path",
                    ));
                }
                // Nonblocking connect cannot hang on an unrelated listener's full backlog.
                let probe = Socket::new(Domain::UNIX, Type::STREAM, None)?;
                probe.set_nonblocking(true)?;
                match probe.connect(&SockAddr::unix(path)?) {
                    Ok(()) => return Err(already_running()),
                    Err(e) if e.kind() == io::ErrorKind::ConnectionRefused => {
                        let current = fs::symlink_metadata(path)?;
                        if (current.dev(), current.ino()) != (m.dev(), m.ino()) {
                            return Err(io::Error::other("socket changed during recovery; retry"));
                        }
                        fs::remove_file(path)?;
                    }
                    Err(e) if e.kind() == io::ErrorKind::NotFound => (),
                    Err(e) => {
                        return Err(io::Error::other(format!(
                            "cannot confirm the socket is stale: {e}"
                        )))
                    }
                }
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => (),
            Err(e) => return Err(e),
        }
        Self::bind(path.to_owned(), Some(lock), None)
    }
    fn bind(
        path: PathBuf,
        lock: Option<File>,
        temporary: Option<tempfile::TempDir>,
    ) -> io::Result<(UnixListener, Self)> {
        let listener = UnixListener::bind(&path)?;
        let m = fs::symlink_metadata(&path)?;
        let endpoint = Self {
            path,
            identity: (m.dev(), m.ino()),
            _lock: lock,
            _temporary: temporary,
        };
        fs::set_permissions(&endpoint.path, fs::Permissions::from_mode(0o600))?;
        listener.set_nonblocking(true)?;
        Ok((listener, endpoint))
    }
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }
}
impl Drop for Endpoint {
    fn drop(&mut self) {
        if fs::symlink_metadata(&self.path).is_ok_and(|m| (m.dev(), m.ino()) == self.identity) {
            let _ = fs::remove_file(&self.path);
        }
    }
}
fn already_running() -> io::Error {
    io::Error::new(
        io::ErrorKind::AddrInUse,
        "a Porthop service is already listening at this socket",
    )
}
