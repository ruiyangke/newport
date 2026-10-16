//! Recorded ownership of a public X11 address. Bind privately, then publish a
//! non-overwriting hard link; the retained private inode proves socket identity.
use crate::owned_directory::Directory;
use serde::{Deserialize, Serialize};
use socket2::{Domain, SockAddr, Socket, Type};
use std::{
    fs::{self, OpenOptions},
    io,
    os::unix::{
        fs::{DirBuilderExt, FileTypeExt, MetadataExt, OpenOptionsExt, PermissionsExt},
        net::UnixListener,
    },
    path::{Path, PathBuf},
};

const RECORD: &str = ".newport-x11.json";
const SOCKET: &str = "x11.sock";
const PREFIX: &str = "newport-x11-";
const PUBLIC: &str = "/tmp/.X11-unix";

// Standalone clipboard displays have no desktop browser. Recognize their
// authority location even when a shell retains it after the display closes.
pub(crate) fn is_authority_path(value: &str) -> bool {
    let Some(directory) = Path::new(value).parent() else {
        return false;
    };
    directory.parent() == Some(Path::new(PUBLIC))
        && directory
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with(PREFIX))
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Record {
    version: u32,
    display: u16,
}
pub(crate) struct Endpoint {
    pub(crate) display: u16,
    directory: Directory,
}
impl Endpoint {
    pub(crate) fn bind(requested: Option<u16>) -> io::Result<(UnixListener, Self)> {
        let public = Path::new(PUBLIC);
        match fs::DirBuilder::new().mode(0o1777).create(public) {
            Ok(()) => fs::set_permissions(public, fs::Permissions::from_mode(0o1777))?,
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => (),
            Err(e) => return Err(e),
        }
        check_public(public)?;
        // Keep the private inode on the public directory's filesystem, including
        // when /tmp/.X11-unix is a separate mount. Its directory remains 0700.
        let directory = Directory::new(public, PREFIX)?;
        let private = directory.path().join(SOCKET);
        let listener = UnixListener::bind(&private)?;
        fs::set_permissions(&private, fs::Permissions::from_mode(0o600))?;
        let numbers: Box<dyn Iterator<Item = u16>> = match requested {
            Some(n) => Box::new(std::iter::once(n)),
            None => Box::new(100..1000),
        };
        for display in numbers {
            // An existing real server lock (including a broken symlink) reserves
            // the display. Never unlink or change another application's lock.
            if fs::symlink_metadata(lock_path(Path::new("/tmp"), display)).is_ok() {
                continue;
            }
            crate::paths::private_write(
                &directory.path().join(RECORD),
                &serde_json::to_vec(&Record {
                    version: 1,
                    display,
                })?,
            )?;
            // The ownership record is published before the public name appears.
            // If publication fails or a process crashes before it, recovery sees
            // either no public socket or a different inode and leaves it alone.
            match fs::hard_link(&private, public.join(format!("X{display}"))) {
                Ok(()) => return Ok((listener, Self { display, directory })),
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => (),
                Err(e) => return Err(e),
            }
        }
        Err(io::Error::other("no unused X display available"))
    }
    pub(crate) fn directory(&self) -> &Path {
        self.directory.path()
    }
}
fn lock_path(parent: &Path, display: u16) -> PathBuf {
    parent.join(format!(".X{display}-lock"))
}
fn check_public(path: &Path) -> io::Result<()> {
    let m = fs::symlink_metadata(path)?;
    let uid = unsafe { libc::geteuid() };
    if !m.is_dir()
        || (m.uid() != 0 && m.uid() != uid)
        || (m.mode() & 0o022 != 0 && m.mode() & 0o1000 == 0)
    {
        return Err(io::Error::other("invalid X socket directory"));
    }
    Ok(())
}
fn same_socket(path: &Path, private: &fs::Metadata) -> bool {
    fs::symlink_metadata(path).is_ok_and(|m| {
        m.file_type().is_socket()
            && m.uid() == unsafe { libc::geteuid() }
            && (m.dev(), m.ino()) == (private.dev(), private.ino())
    })
}
/// Called only with exclusive directory ownership. False retains the directory
/// and its ownership proof for a later attempt; no arbitrary paths are read from
/// the record. Unrecognized or ambiguous resources are never deleted.
pub(crate) fn reclaim(directory: &Path) -> bool {
    reclaim_in(directory, Path::new(PUBLIC), Path::new("/tmp"))
}
fn reclaim_in(directory: &Path, public: &Path, locks: &Path) -> bool {
    let file = match OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(directory.join(RECORD))
    {
        Ok(file) => file,
        Err(e) => return e.kind() == io::ErrorKind::NotFound,
    };
    if !file.metadata().is_ok_and(|m| {
        m.is_file()
            && m.uid() == unsafe { libc::geteuid() }
            && m.mode() & 0o077 == 0
            && m.nlink() == 1
            && m.len() <= 256
    }) {
        return false;
    }
    let Ok(record) = serde_json::from_reader::<_, Record>(file) else {
        return false;
    };
    if record.version != 1 {
        return false;
    }
    let private = match fs::symlink_metadata(directory.join(SOCKET)) {
        Ok(m) if m.file_type().is_socket() && m.uid() == unsafe { libc::geteuid() } => m,
        _ => return false,
    };
    let socket = public.join(format!("X{}", record.display));
    match fs::symlink_metadata(&socket) {
        Ok(_) => (),
        Err(e) if e.kind() == io::ErrorKind::NotFound => return true,
        Err(_) => return false,
    }
    if !same_socket(&socket, &private) {
        // A different public inode belongs to someone else. Our private files
        // can still be removed, but must never be used to delete that new socket.
        return true;
    }
    if check_public(public).is_err()
        || fs::symlink_metadata(lock_path(locks, record.display)).is_ok()
    {
        return false;
    }
    let probe = (|| -> io::Result<()> {
        let socket_probe = Socket::new(Domain::UNIX, Type::STREAM, None)?;
        socket_probe.set_nonblocking(true)?;
        socket_probe.connect(&SockAddr::unix(&socket)?)
    })();
    match probe {
        Err(e) if e.kind() == io::ErrorKind::NotFound => true,
        Err(e) if e.kind() == io::ErrorKind::ConnectionRefused => {
            if !same_socket(&socket, &private) {
                return true;
            }
            fs::remove_file(socket).is_ok()
        }
        // Includes an active server, a full backlog, and unexpected errors.
        _ => false,
    }
}
pub(crate) fn sweep() {
    let public = Path::new(PUBLIC);
    if check_public(public).is_ok() {
        crate::owned_directory::sweep(public, PREFIX);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Fixture {
        root: tempfile::TempDir,
        public: tempfile::TempDir,
        listener: Option<UnixListener>,
    }
    impl Fixture {
        fn new() -> Self {
            let root = tempfile::tempdir().unwrap();
            let public = tempfile::tempdir().unwrap();
            let listener = UnixListener::bind(root.path().join(SOCKET)).unwrap();
            fs::hard_link(root.path().join(SOCKET), public.path().join("X400")).unwrap();
            crate::paths::private_write(
                &root.path().join(RECORD),
                &serde_json::to_vec(&Record {
                    version: 1,
                    display: 400,
                })
                .unwrap(),
            )
            .unwrap();
            Self {
                root,
                public,
                listener: Some(listener),
            }
        }
        fn reclaim(&self) -> bool {
            reclaim_in(self.root.path(), self.public.path(), self.public.path())
        }
        fn await_reclaim(&self) {
            // Concurrent fork/exec can briefly retain a copy of the listener.
            // Recovery must wait for connection refusal, never delete a socket
            // while an inherited listener is still alive.
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
            while !self.reclaim() {
                assert!(
                    std::time::Instant::now() < deadline,
                    "dead socket was not reclaimed"
                );
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
        }
    }
    #[test]
    fn preserves_listeners_and_real_server_locks_then_reclaims_dead_owned_socket() {
        let mut f = Fixture::new();
        assert!(!f.reclaim());
        f.listener.take();
        let lock = lock_path(f.public.path(), 400);
        fs::write(&lock, "another server").unwrap();
        assert!(!f.reclaim());
        assert!(f.public.path().join("X400").exists());
        fs::remove_file(lock).unwrap();
        f.await_reclaim();
        assert!(!f.public.path().join("X400").exists());
    }
    #[test]
    fn preserves_replacement_socket_and_refuses_unrecognized_ownership() {
        let mut f = Fixture::new();
        f.listener.take();
        let public = f.public.path().join("X400");
        fs::remove_file(&public).unwrap();
        let _replacement = UnixListener::bind(&public).unwrap();
        assert!(f.reclaim());
        assert!(public.exists());
        crate::paths::private_write(
            &f.root.path().join(RECORD),
            b"{\"version\":2,\"display\":400}",
        )
        .unwrap();
        assert!(!f.reclaim());
        assert!(public.exists());
    }
}
