//! Blocking local byte streams for client workers and contract fixtures.
//!
//! Unix retains the existing socket behavior. Windows uses overlapped named
//! pipes, with an explicit logon-session ACL, same-user server verification,
//! deadlines and cancellation. Neither implementation retries request bytes.
//! Call these blocking APIs on worker threads, not inside an async executor.
pub mod asynchronous;
pub mod framed;
#[cfg(unix)]
pub use std::os::unix::net::{UnixListener as Listener, UnixStream as Stream};
#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use windows::{Listener, Stream};

/// Keep the shipped endpoint for ordinary profiles. Unix socket addresses are
/// bounded by `sockaddr_un`, independently of the filesystem's path limit.
/// A long profile uses a stable, per-user private directory under /tmp instead.
pub fn profile_endpoint(profile: &std::path::Path) -> std::path::PathBuf {
    #[cfg(unix)]
    let profile = profile
        .canonicalize()
        .unwrap_or_else(|_| profile.to_owned());
    let endpoint = profile.join("native-client/desktop.sock");
    #[cfg(unix)]
    {
        use sha2::{Digest, Sha256};
        use std::os::unix::ffi::OsStrExt;
        if endpoint.as_os_str().as_bytes().len() >= 104 {
            let hash: String = Sha256::digest(profile.as_os_str().as_bytes())
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect();
            // A fixed short root is necessary: the user's temporary directory
            // can itself exceed SUN_LEN. The listener validates ownership and
            // permissions, and clients validate them before connecting.
            // SAFETY: geteuid has no arguments or preconditions.
            let uid = unsafe { libc::geteuid() };
            return std::path::PathBuf::from(format!("/tmp/newport-{uid}-{hash}/s"));
        }
    }
    endpoint
}

/// Connect to Newport's private RPC endpoint, including long-profile fallbacks.
/// Do not use this for OS-owned SSH agent sockets.
pub fn connect_private(path: impl AsRef<std::path::Path>) -> std::io::Result<Stream> {
    connect_private_until(
        path,
        std::time::Instant::now() + newport_protocol::desktop::ADMISSION_TIMEOUT,
    )
}

/// Connection establishment consumes the caller's existing admission deadline.
pub fn connect_private_until(
    path: impl AsRef<std::path::Path>,
    deadline: std::time::Instant,
) -> std::io::Result<Stream> {
    let path = path.as_ref();
    framed::remaining(deadline)?;
    if !path.is_absolute() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "Local endpoint must be absolute",
        ));
    }
    #[cfg(unix)]
    {
        use std::os::{fd::OwnedFd, unix::fs::MetadataExt};
        if let Some(parent) = path.parent()
            && parent.parent() == Some(std::path::Path::new("/tmp"))
            && parent
                .file_name()
                .is_some_and(|name| name.as_encoded_bytes().starts_with(b"newport-"))
        {
            let metadata = std::fs::symlink_metadata(parent)?;
            if !metadata.is_dir()
                // SAFETY: geteuid has no arguments or preconditions.
                || metadata.uid() != unsafe { libc::geteuid() }
                || metadata.mode() & 0o077 != 0
            {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    "Local endpoint directory is not private to this user",
                ));
            }
        }
        let socket = socket2::Socket::new(socket2::Domain::UNIX, socket2::Type::STREAM, None)?;
        socket.connect_timeout(
            &socket2::SockAddr::unix(path)?,
            framed::remaining(deadline)?,
        )?;
        let fd: OwnedFd = socket.into();
        let stream = Stream::from(fd);
        verify_peer(&stream)?;
        framed::remaining(deadline)?;
        Ok(stream)
    }
    #[cfg(windows)]
    {
        Stream::connect_until(path, deadline)
    }
}

#[cfg(unix)]
fn verify_peer(stream: &Stream) -> std::io::Result<()> {
    use std::os::fd::AsRawFd;
    #[cfg(any(target_os = "linux", target_os = "android"))]
    let uid = {
        let mut credentials = std::mem::MaybeUninit::<libc::ucred>::uninit();
        let mut length = libc::socklen_t::try_from(std::mem::size_of::<libc::ucred>())
            .map_err(std::io::Error::other)?;
        // SAFETY: `credentials` has room for a `ucred`, and `length` is its size.
        let result = unsafe {
            libc::getsockopt(
                stream.as_raw_fd(),
                libc::SOL_SOCKET,
                libc::SO_PEERCRED,
                credentials.as_mut_ptr().cast(),
                &mut length,
            )
        };
        if result != 0 {
            return Err(std::io::Error::last_os_error());
        }
        if usize::try_from(length).ok() != Some(std::mem::size_of::<libc::ucred>()) {
            return Err(std::io::Error::other("Invalid local peer credentials"));
        }
        // SAFETY: getsockopt succeeded and filled exactly one `ucred` (length checked).
        unsafe { credentials.assume_init().uid }
    };
    #[cfg(not(any(target_os = "linux", target_os = "android")))]
    let uid = {
        let (mut uid, mut gid) = (0, 0);
        // SAFETY: the descriptor is live, and both ids are writable.
        if unsafe { libc::getpeereid(stream.as_raw_fd(), &mut uid, &mut gid) } != 0 {
            return Err(std::io::Error::last_os_error());
        }
        uid
    };
    // SAFETY: geteuid has no arguments or preconditions.
    if uid != unsafe { libc::geteuid() } {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "Local backend belongs to another user",
        ));
    }
    Ok(())
}

/// Agent endpoints are OS-owned, rather than Newport's profile endpoints.
/// Windows OpenSSH and 1Password expose the same system named pipe.
pub fn connect_agent(path: impl AsRef<std::path::Path>) -> std::io::Result<Stream> {
    #[cfg(unix)]
    {
        Stream::connect(path)
    }
    #[cfg(windows)]
    {
        Stream::connect_agent(path.as_ref())
    }
}

#[cfg(test)]
mod tests;
