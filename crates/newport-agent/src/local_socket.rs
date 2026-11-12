//! Ownership and recovery of private Unix socket paths.
use fs2::FileExt;
use socket2::{Domain, SockAddr, Socket, Type};
use std::{
    fs::{self, File, OpenOptions},
    io,
    os::unix::{
        fs::{DirBuilderExt, FileTypeExt, MetadataExt, OpenOptionsExt, PermissionsExt},
        net::{UnixListener, UnixStream},
    },
    path::{Path, PathBuf},
    time::Duration,
};

pub(crate) fn read_timeout(stream: &UnixStream, timeout: Duration) -> io::Result<()> {
    timeout_result(stream, stream.set_read_timeout(Some(timeout)))
}

pub(crate) fn write_timeout(stream: &UnixStream, timeout: Duration) -> io::Result<()> {
    timeout_result(stream, stream.set_write_timeout(Some(timeout)))
}

fn timeout_result(stream: &UnixStream, result: io::Result<()>) -> io::Result<()> {
    // Darwin rejects timeout changes after disconnect, while buffered bytes,
    // EOF and write errors remain immediately available on the socket.
    if cfg!(target_os = "macos")
        && result
            .as_ref()
            .is_err_and(|error| error.raw_os_error() == Some(libc::EINVAL))
        && peer_unavailable(stream)
    {
        Ok(())
    } else {
        result
    }
}

fn peer_unavailable(stream: &UnixStream) -> bool {
    stream
        .peer_addr()
        .is_err_and(|error| matches!(error.raw_os_error(), Some(libc::EINVAL | libc::ENOTCONN)))
}

pub(crate) fn hangup_is_disconnect(stream: &UnixStream) -> bool {
    // Darwin also reports POLLHUP for an intentional request half-close.
    !cfg!(target_os = "macos") || peer_unavailable(stream)
}

pub(crate) fn shutdown(stream: &UnixStream) {
    if cfg!(target_os = "macos") {
        // A peer half-close can make SHUT_RDWR fail before closing our writer.
        let _ = stream.shutdown(std::net::Shutdown::Write);
        let _ = stream.shutdown(std::net::Shutdown::Read);
    } else {
        let _ = stream.shutdown(std::net::Shutdown::Both);
    }
}

struct Lock {
    file: File,
}
impl Lock {
    fn acquire(file: File) -> io::Result<Self> {
        FileExt::try_lock_exclusive(&file).map_err(|e| {
            if e.kind() == io::ErrorKind::WouldBlock {
                already_running()
            } else {
                e
            }
        })?;
        Ok(Self { file })
    }
}
impl Drop for Lock {
    fn drop(&mut self) {
        // A parallel fork can retain the open file description until exec.
        // Release ownership even when endpoint setup fails before binding.
        let _ = FileExt::unlock(&self.file);
    }
}

pub(crate) struct Endpoint {
    path: PathBuf,
    identity: (u64, u64),
    // Keep the stable lock inode for the entire listener lifetime. Never unlink it.
    _lock: Option<Lock>,
    _temporary: Option<crate::owned_directory::Directory>,
}
impl Endpoint {
    pub(crate) fn temporary() -> io::Result<(UnixListener, Self)> {
        let directory = crate::owned_directory::Directory::new(&crate::paths::runtime()?, "wl-")?;
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
        let uid = crate::paths::effective_uid();
        if !metadata.is_dir() || metadata.uid() != uid || metadata.mode() & 0o077 != 0 {
            return Err(io::Error::other(
                "the socket directory must belong to this account, have mode 700, and not be a symlink",
            ));
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
        Self::recover_and_bind(path, uid, Lock::acquire(lock)?)
    }
    fn recover_and_bind(path: &Path, uid: u32, lock: Lock) -> io::Result<(UnixListener, Self)> {
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
                        )));
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
        lock: Option<Lock>,
        temporary: Option<crate::owned_directory::Directory>,
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
        // The lock field releases ownership after socket cleanup, including
        // when another process still holds an inherited descriptor.
    }
}
fn already_running() -> io::Error {
    io::Error::new(
        io::ErrorKind::AddrInUse,
        "a Newport service is already listening at this socket",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disconnected_peers_preserve_buffered_bytes_eof_and_write_errors() {
        use std::io::{Read, Write};
        let (mut stream, mut peer) = UnixStream::pair().unwrap();
        peer.write_all(b"ready").unwrap();
        drop(peer);
        read_timeout(&stream, Duration::from_secs(1)).unwrap();
        let mut bytes = [0; 5];
        stream.read_exact(&mut bytes).unwrap();
        assert_eq!(&bytes, b"ready");
        read_timeout(&stream, Duration::from_secs(1)).unwrap();
        assert_eq!(stream.read(&mut bytes).unwrap(), 0);
        write_timeout(&stream, Duration::from_secs(1)).unwrap();
        let error = stream.write_all(b"next").unwrap_err();
        assert!(matches!(
            error.kind(),
            io::ErrorKind::BrokenPipe | io::ErrorKind::ConnectionReset
        ));
        assert!(error.raw_os_error().is_some());
        assert_eq!(
            read_timeout(&stream, Duration::ZERO).unwrap_err().kind(),
            io::ErrorKind::InvalidInput
        );
    }

    #[test]
    fn shutdown_closes_the_writer_after_a_peer_half_close() {
        use std::io::Read;
        let (mut stream, mut peer) = UnixStream::pair().unwrap();
        peer.shutdown(std::net::Shutdown::Write).unwrap();
        assert_eq!(stream.read(&mut [0]).unwrap(), 0);
        shutdown(&stream);
        shutdown(&stream);
        read_timeout(&peer, Duration::from_secs(1)).unwrap();
        assert_eq!(peer.read(&mut [0]).unwrap(), 0);
    }

    #[test]
    fn shutdown_releases_fixed_socket_lock_with_an_inherited_descriptor() {
        let directory = tempfile::Builder::new()
            .permissions(fs::Permissions::from_mode(0o700))
            .tempdir_in("/tmp")
            .unwrap();
        let socket = directory.path().join("wayland.sock");
        let (listener, endpoint) = Endpoint::fixed(&socket).unwrap();
        // A parallel fork shares this open file description until exec.
        let inherited = endpoint._lock.as_ref().unwrap().file.try_clone().unwrap();
        assert!(matches!(Endpoint::fixed(&socket), Err(e) if e.kind() == io::ErrorKind::AddrInUse));
        drop(listener);
        drop(endpoint);
        assert!(!socket.exists());
        let (listener, endpoint) = Endpoint::fixed(&socket).unwrap();
        drop(inherited);
        // Closing the old descriptor must not release the new owner's lock.
        assert!(matches!(Endpoint::fixed(&socket), Err(e) if e.kind() == io::ErrorKind::AddrInUse));
        drop(listener);
        drop(endpoint);
    }

    #[test]
    fn failed_setup_releases_its_lock_without_disturbing_a_live_listener() {
        let directory = tempfile::Builder::new()
            .permissions(fs::Permissions::from_mode(0o700))
            .tempdir_in("/tmp")
            .unwrap();
        let socket = directory.path().join("wayland.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let before = fs::symlink_metadata(&socket).unwrap();
        let lock_path = socket.with_file_name("wayland.sock.lock");
        let open_lock = || {
            OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .truncate(false)
                .mode(0o600)
                .open(&lock_path)
                .unwrap()
        };
        let lock = Lock::acquire(open_lock()).unwrap();
        // This shares the locked open file description, exactly as fork does.
        let inherited = lock.file.try_clone().unwrap();
        assert!(matches!(
            Endpoint::recover_and_bind(&socket, crate::paths::effective_uid(), lock),
            Err(error) if error.kind() == io::ErrorKind::AddrInUse
        ));
        let current = fs::symlink_metadata(&socket).unwrap();
        assert_eq!((current.dev(), current.ino()), (before.dev(), before.ino()));
        assert!(std::os::unix::net::UnixStream::connect(&socket).is_ok());

        let next = Lock::acquire(open_lock()).unwrap();
        drop(inherited);
        // Closing a failed owner's inherited descriptor cannot release this
        // separate open file description's new ownership.
        assert!(
            matches!(Lock::acquire(open_lock()), Err(error) if error.kind() == io::ErrorKind::AddrInUse)
        );
        drop(next);
        drop(listener);
    }
}
