use std::{
    io,
    os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt},
    path::{Path, PathBuf},
};
pub use tokio::net::UnixStream as Stream;

struct SocketGuard {
    path: PathBuf,
    device: u64,
    inode: u64,
}
impl Drop for SocketGuard {
    fn drop(&mut self) {
        if std::fs::symlink_metadata(&self.path).is_ok_and(|m| {
            m.file_type().is_socket() && m.dev() == self.device && m.ino() == self.inode
        }) {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}
pub struct Listener {
    listener: tokio::net::UnixListener,
    _guard: SocketGuard,
}
impl Listener {
    /// Refuses to replace existing endpoints. Useful for isolated fixtures.
    pub fn bind(path: impl AsRef<Path>) -> io::Result<Self> {
        Self::bind_inner(path.as_ref(), false)
    }
    /// The caller must hold the exclusive profile instance lock for this
    /// listener's entire lifetime. Only an owned stale socket may be replaced.
    pub fn bind_owned(path: impl AsRef<Path>) -> io::Result<Self> {
        Self::bind_inner(path.as_ref(), true)
    }
    fn bind_inner(path: &Path, replace: bool) -> io::Result<Self> {
        if !path.is_absolute() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Local endpoint must be absolute",
            ));
        }
        super::directory(path)?;
        let parent = path.parent().expect("validated parent");
        let uid = unsafe { libc::geteuid() };
        if std::fs::symlink_metadata(parent)?.uid() != uid {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "Local endpoint directory belongs to another user",
            ));
        }
        std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700))?;
        if replace {
            match std::fs::symlink_metadata(path) {
                Ok(previous) if previous.file_type().is_socket() && previous.uid() == uid => {
                    std::fs::remove_file(path)?
                }
                Ok(_) => {
                    return Err(io::Error::new(
                        io::ErrorKind::PermissionDenied,
                        "Refusing to replace an unexpected endpoint",
                    ));
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(error),
            }
        }
        let listener = tokio::net::UnixListener::bind(path)?;
        let metadata = std::fs::symlink_metadata(path)?;
        let guard = SocketGuard {
            path: path.to_owned(),
            device: metadata.dev(),
            inode: metadata.ino(),
        };
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
        Ok(Self {
            listener,
            _guard: guard,
        })
    }
    pub async fn accept(&mut self) -> io::Result<Stream> {
        loop {
            let (stream, _) = self.listener.accept().await?;
            if stream
                .peer_cred()
                .is_ok_and(|peer| peer.uid() == unsafe { libc::geteuid() })
            {
                return Ok(stream);
            }
        }
    }
}
pub fn peer_process_id(stream: &Stream) -> io::Result<u32> {
    stream
        .peer_cred()?
        .pid()
        .and_then(|pid| u32::try_from(pid).ok())
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::PermissionDenied,
                "Local transport requires a kernel peer PID",
            )
        })
}
pub async fn connect_agent(path: impl AsRef<Path>) -> io::Result<Stream> {
    Stream::connect(path).await
}
pub async fn pair() -> io::Result<(Stream, Stream)> {
    Stream::pair()
}
