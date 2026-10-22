//! Blocking local byte streams for client workers and contract fixtures.
//!
//! Unix retains the existing socket behavior. Windows uses overlapped named
//! pipes, with an explicit logon-session ACL, same-user server verification,
//! deadlines and cancellation. Neither implementation retries request bytes.
//! Call these blocking APIs on worker threads, not inside an async executor.
pub mod asynchronous;
pub mod protocol;
#[cfg(unix)]
pub use std::os::unix::net::{UnixListener as Listener, UnixStream as Stream};
#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use windows::{Listener, Stream};

/// Keep the shipped endpoint for ordinary profiles. Unix socket addresses are
/// bounded (104 bytes on macOS), independently of the filesystem's path limit.
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
            let uid = unsafe { libc::geteuid() };
            return std::path::PathBuf::from(format!("/tmp/newport-{uid}-{hash}/s"));
        }
    }
    endpoint
}

/// Connect to Newport's private RPC endpoint, including long-profile fallbacks.
/// Do not use this for OS-owned SSH agent sockets.
pub fn connect_private(path: impl AsRef<std::path::Path>) -> std::io::Result<Stream> {
    let path = path.as_ref();
    #[cfg(unix)]
    if let Some(parent) = path.parent()
        && parent.parent() == Some(std::path::Path::new("/tmp"))
        && parent
            .file_name()
            .is_some_and(|name| name.as_encoded_bytes().starts_with(b"newport-"))
    {
        use std::os::unix::fs::MetadataExt;
        let metadata = std::fs::symlink_metadata(parent)?;
        if !metadata.is_dir()
            || metadata.uid() != unsafe { libc::geteuid() }
            || metadata.mode() & 0o077 != 0
        {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "Local endpoint directory is not private to this user",
            ));
        }
    }
    Stream::connect(path)
}

pub fn bind_private(path: impl AsRef<std::path::Path>) -> std::io::Result<Listener> {
    let listener = Listener::bind(path.as_ref())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }
    // On Windows Listener::bind creates its protected DACL atomically.
    Ok(listener)
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
