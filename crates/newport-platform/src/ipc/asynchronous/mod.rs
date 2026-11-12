//! Async host transport with OS-authenticated local peers. Bind inside the
//! host's Tokio runtime; no hidden runtime or blocking socket I/O is used.
#[cfg(unix)]
mod unix;
#[cfg(unix)]
pub use unix::*;
#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use windows::*;

/// A private socket directory must be an ordinary directory, not a symlink.
/// Existing directories retain the host profile's persistent access policy.
fn directory(path: &std::path::Path) -> std::io::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| std::io::Error::other("Missing endpoint directory"))?;
    match crate::filesystem::create_private_directory(parent) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error),
    }
    let metadata = std::fs::symlink_metadata(parent)?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "Local endpoint directory must be a real directory",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests;
