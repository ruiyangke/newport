//! Windows identity and private DACLs for pipes, directories and files.
use newport_win32::{
    pipe,
    security::{Descriptor, Token, verify_same_session},
};
use std::{io, os::windows::io::BorrowedHandle};
use windows_sys::Win32::{
    Foundation::HANDLE,
    Security::{
        DACL_SECURITY_INFORMATION, PROTECTED_DACL_SECURITY_INFORMATION, SECURITY_ATTRIBUTES,
    },
};

pub(super) fn current_user() -> io::Result<String> {
    Token::current()?.user()
}

/// Validate the server identity before sending any protocol or credential data.
/// The client uses SECURITY_IDENTIFICATION (Tokio's default), never impersonation.
pub(super) fn verify_server(pipe: HANDLE) -> io::Result<()> {
    verify_same_session(peer_pid(pipe, false)?)
}

pub(super) fn verify_client(pipe: HANDLE) -> io::Result<()> {
    verify_same_session(peer_pid(pipe, true)?)
}

pub(super) fn peer_pid(pipe: HANDLE, server: bool) -> io::Result<u32> {
    // SAFETY: callers pass a pipe handle they hold for the call.
    pipe::peer_pid(unsafe { BorrowedHandle::borrow_raw(pipe) }, server)
}

/// Build the descriptor for each pipe instance. No inherited/default ACEs:
/// only this logon session can connect or create further instances.
pub(super) fn with_attributes<T>(
    f: impl FnOnce(*mut std::ffi::c_void) -> io::Result<T>,
) -> io::Result<T> {
    let logon = Token::current()?.logon()?;
    with_descriptor(&format!("D:P(A;;GA;;;{logon})"), f)
}

pub(super) fn create_private_directory(path: &std::path::Path) -> io::Result<()> {
    use windows_sys::Win32::Storage::FileSystem::CreateDirectoryW;
    let user = current_user()?;
    let wide = wide_path(path)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "Directory contains NUL"))?;
    // User SID (not transient logon SID) keeps files readable after logging in
    // again. Descendants inherit this protected private directory's ACL.
    with_descriptor(
        &format!("D:P(A;OICI;FA;;;{user})(A;OICI;FA;;;SY)"),
        |attributes| {
            // SAFETY: `wide` is NUL-terminated, and `attributes` is the descriptor's
            // SECURITY_ATTRIBUTES, alive inside `with_descriptor`.
            if unsafe { CreateDirectoryW(wide.as_ptr(), attributes.cast()) } == 0 {
                Err(io::Error::last_os_error())
            } else {
                Ok(())
            }
        },
    )
}

pub(super) fn wide_path(path: &std::path::Path) -> io::Result<Vec<u16>> {
    newport_win32::wide(path.as_os_str())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "Path contains NUL"))
}
pub(super) fn protect_path(path: &std::path::Path, inherit: &str) -> io::Result<()> {
    use windows_sys::Win32::Security::SetFileSecurityW;
    let path = wide_path(path)?;
    let user = current_user()?;
    let descriptor = Descriptor::from_sddl(&format!(
        "D:P(A;{inherit};FA;;;{user})(A;{inherit};FA;;;SY)"
    ))?;
    // SAFETY: `path` is NUL-terminated, and `descriptor` is alive for the call.
    if unsafe {
        SetFileSecurityW(
            path.as_ptr(),
            DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
            descriptor.as_ptr(),
        )
    } == 0
    {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

fn with_descriptor<T>(
    sddl: &str,
    f: impl FnOnce(*mut std::ffi::c_void) -> io::Result<T>,
) -> io::Result<T> {
    let descriptor = Descriptor::from_sddl(sddl)?;
    let mut attributes = descriptor.attributes(false);
    f((&mut attributes as *mut SECURITY_ATTRIBUTES).cast())
}
