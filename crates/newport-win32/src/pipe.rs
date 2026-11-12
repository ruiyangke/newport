//! Local named-pipe ends and their peers.
use crate::handle::owned;
use std::{
    io,
    os::windows::io::{AsRawHandle, BorrowedHandle, OwnedHandle},
    ptr,
};
use windows_sys::Win32::{
    Security::SECURITY_ATTRIBUTES,
    Storage::FileSystem::{
        CreateFileW, FILE_FLAGS_AND_ATTRIBUTES, OPEN_EXISTING, SECURITY_IDENTIFICATION,
        SECURITY_SQOS_PRESENT,
    },
    System::Pipes::{GetNamedPipeClientProcessId, GetNamedPipeServerProcessId},
};

/// Opens the client end of the pipe `name` (NUL-terminated) with `access`
/// and `flags`. The server may identify the client but never impersonate it.
pub fn open_client(
    name: &[u16],
    access: u32,
    flags: FILE_FLAGS_AND_ATTRIBUTES,
    inheritable: bool,
) -> io::Result<OwnedHandle> {
    if name.last() != Some(&0) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Pipe name must be NUL-terminated",
        ));
    }
    let attributes = SECURITY_ATTRIBUTES {
        nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: ptr::null_mut(),
        bInheritHandle: inheritable.into(),
    };
    // SAFETY: `name` is NUL-terminated (checked above) and `attributes` lives
    // past the call; the returned handle is new and owned.
    unsafe {
        owned(CreateFileW(
            name.as_ptr(),
            access,
            0,
            &attributes,
            OPEN_EXISTING,
            flags | SECURITY_SQOS_PRESENT | SECURITY_IDENTIFICATION,
            ptr::null_mut(),
        ))
    }
}

/// The process at the other end: the client's from a server end, the
/// server's from a client end.
pub fn peer_pid(pipe: BorrowedHandle<'_>, server: bool) -> io::Result<u32> {
    let pipe = pipe.as_raw_handle();
    let mut pid = 0;
    // SAFETY: `pipe` is a pipe handle borrowed for the call, and `pid` is
    // writable.
    let ok = unsafe {
        if server {
            GetNamedPipeClientProcessId(pipe, &mut pid)
        } else {
            GetNamedPipeServerProcessId(pipe, &mut pid)
        }
    };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(pid)
}
