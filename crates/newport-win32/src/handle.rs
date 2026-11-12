//! Owned handles from raw Win32 results.
use std::{
    io,
    os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle},
    ptr,
};
use windows_sys::Win32::{
    Foundation::{
        HANDLE, HANDLE_FLAG_INHERIT, INVALID_HANDLE_VALUE, SetHandleInformation, WAIT_OBJECT_0,
        WAIT_TIMEOUT,
    },
    Security::SECURITY_ATTRIBUTES,
    Storage::FileSystem::{CreateFileW, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING},
    System::Threading::WaitForSingleObject,
};

/// Takes ownership of a handle a Win32 call returned, or its last error for
/// a null or invalid one.
/// # Safety
/// `raw` is null, invalid, or a handle the caller owns and no one else closes.
pub unsafe fn owned(raw: HANDLE) -> io::Result<OwnedHandle> {
    if raw.is_null() || raw == INVALID_HANDLE_VALUE {
        Err(io::Error::last_os_error())
    } else {
        // SAFETY: `raw` is valid and owned by the caller, per this function's contract.
        Ok(unsafe { OwnedHandle::from_raw_handle(raw) })
    }
}

/// The last error for a Win32 call that returned FALSE.
pub fn check(result: i32) -> io::Result<()> {
    if result == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

/// Lets child processes inherit `handle`. They get it only through an
/// explicit handle list (see [`crate::process`]).
pub fn set_inheritable(handle: &impl AsRawHandle) -> io::Result<()> {
    // SAFETY: the handle is open for the call.
    check(unsafe {
        SetHandleInformation(
            handle.as_raw_handle(),
            HANDLE_FLAG_INHERIT,
            HANDLE_FLAG_INHERIT,
        )
    })
}

/// An inheritable handle to the `NUL` device with `access`.
pub fn null_device(access: u32) -> io::Result<OwnedHandle> {
    let name = crate::wide("NUL".as_ref())?;
    let attributes = SECURITY_ATTRIBUTES {
        nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: ptr::null_mut(),
        bInheritHandle: 1,
    };
    // SAFETY: `name` is NUL-terminated and `attributes` lives past the call; the
    // returned handle is new and owned.
    unsafe {
        owned(CreateFileW(
            name.as_ptr(),
            access,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            &attributes,
            OPEN_EXISTING,
            0,
            ptr::null_mut(),
        ))
    }
}

/// Whether the process or other waitable object is signaled, without waiting.
pub fn signaled(object: &impl AsRawHandle) -> io::Result<bool> {
    // SAFETY: the handle is open for the call.
    match unsafe { WaitForSingleObject(object.as_raw_handle(), 0) } {
        WAIT_OBJECT_0 => Ok(true),
        WAIT_TIMEOUT => Ok(false),
        _ => Err(io::Error::last_os_error()),
    }
}
