//! Windows identity and the private pipe DACL. Handles and LocalAlloc buffers
//! never escape the checked Win32 calls that own them.
use std::{io, ptr};
use windows_sys::Win32::{
    Foundation::{CloseHandle, HANDLE, LocalFree},
    Security::{Authorization::*, *},
    System::{
        Pipes::{GetNamedPipeClientProcessId, GetNamedPipeServerProcessId},
        Threading::*,
    },
};

struct Handle(HANDLE);
impl Drop for Handle {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.0);
        }
    }
}
struct Allocation(*mut std::ffi::c_void);
impl Drop for Allocation {
    fn drop(&mut self) {
        unsafe {
            LocalFree(self.0);
        }
    }
}

fn token(process: HANDLE) -> io::Result<Handle> {
    let mut handle = ptr::null_mut();
    if unsafe { OpenProcessToken(process, TOKEN_QUERY, &mut handle) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(Handle(handle))
}

fn sid(token: &Handle, logon: bool) -> io::Result<String> {
    let class = if logon { TokenLogonSid } else { TokenUser };
    let mut needed = 0;
    unsafe {
        GetTokenInformation(token.0, class, ptr::null_mut(), 0, &mut needed);
    }
    if needed == 0 {
        return Err(io::Error::last_os_error());
    }
    // TOKEN_USER/TOKEN_GROUPS contain pointers and need pointer alignment.
    let mut buffer = vec![0usize; (needed as usize).div_ceil(size_of::<usize>())];
    if unsafe {
        GetTokenInformation(
            token.0,
            class,
            buffer.as_mut_ptr().cast(),
            needed,
            &mut needed,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    let raw = unsafe {
        if logon {
            let groups = &*buffer.as_ptr().cast::<TOKEN_GROUPS>();
            if groups.GroupCount != 1 {
                return Err(io::Error::other("Expected one Windows logon SID"));
            }
            groups.Groups[0].Sid
        } else {
            (*buffer.as_ptr().cast::<TOKEN_USER>()).User.Sid
        }
    };
    let mut text = ptr::null_mut();
    if unsafe { ConvertSidToStringSidW(raw, &mut text) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let _allocation = Allocation(text.cast());
    let mut length = 0;
    unsafe {
        while *text.add(length) != 0 {
            length += 1;
        }
    }
    Ok(String::from_utf16_lossy(unsafe {
        std::slice::from_raw_parts(text, length)
    }))
}

pub(super) fn current_user() -> io::Result<String> {
    sid(&token(unsafe { GetCurrentProcess() })?, false)
}

/// Validate the server identity before sending any protocol or credential data.
/// The client uses SECURITY_IDENTIFICATION (Tokio's default), never impersonation.
pub(super) fn verify_server(pipe: HANDLE) -> io::Result<()> {
    verify_process(peer_pid(pipe, false)?)
}

pub(super) fn verify_client(pipe: HANDLE) -> io::Result<()> {
    verify_process(peer_pid(pipe, true)?)
}

pub(super) fn peer_pid(pipe: HANDLE, server: bool) -> io::Result<u32> {
    let mut pid = 0;
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

fn verify_process(pid: u32) -> io::Result<()> {
    let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if process.is_null() {
        return Err(io::Error::last_os_error());
    }
    let process = Handle(process);
    let own = token(unsafe { GetCurrentProcess() })?;
    let peer = token(process.0)?;
    if sid(&own, false)? != sid(&peer, false)? || sid(&own, true)? != sid(&peer, true)? {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "Local peer belongs to another user or logon session",
        ));
    }
    Ok(())
}

/// Build the descriptor for each pipe instance. No inherited/default ACEs:
/// only this logon session can connect or create further instances.
pub(super) fn with_attributes<T>(
    f: impl FnOnce(*mut std::ffi::c_void) -> io::Result<T>,
) -> io::Result<T> {
    let logon = sid(&token(unsafe { GetCurrentProcess() })?, true)?;
    with_descriptor(&format!("D:P(A;;GA;;;{logon})"), f)
}

pub(super) fn create_private_directory(path: &std::path::Path) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::CreateDirectoryW;
    let user = current_user()?;
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    if wide[..wide.len() - 1].contains(&0) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Directory contains NUL",
        ));
    }
    // User SID (not transient logon SID) keeps files readable after logging in
    // again. Descendants inherit this protected private directory's ACL.
    with_descriptor(
        &format!("D:P(A;OICI;FA;;;{user})(A;OICI;FA;;;SY)"),
        |attributes| {
            if unsafe { CreateDirectoryW(wide.as_ptr(), attributes.cast()) } == 0 {
                Err(io::Error::last_os_error())
            } else {
                Ok(())
            }
        },
    )
}

pub(super) fn wide_path(path: &std::path::Path) -> io::Result<Vec<u16>> {
    use std::os::windows::ffi::OsStrExt;
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    if wide[..wide.len() - 1].contains(&0) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Path contains NUL",
        ));
    }
    Ok(wide)
}
pub(super) fn protect_path(path: &std::path::Path, inherit: &str) -> io::Result<()> {
    let path = wide_path(path)?;
    let user = current_user()?;
    with_descriptor(
        &format!("D:P(A;{inherit};FA;;;{user})(A;{inherit};FA;;;SY)"),
        |attributes| {
            let descriptor =
                unsafe { (*attributes.cast::<SECURITY_ATTRIBUTES>()).lpSecurityDescriptor };
            if unsafe {
                SetFileSecurityW(
                    path.as_ptr(),
                    DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
                    descriptor,
                )
            } == 0
            {
                Err(io::Error::last_os_error())
            } else {
                Ok(())
            }
        },
    )
}

fn with_descriptor<T>(
    sddl: &str,
    f: impl FnOnce(*mut std::ffi::c_void) -> io::Result<T>,
) -> io::Result<T> {
    let sddl: Vec<u16> = sddl.encode_utf16().chain(Some(0)).collect();
    let mut descriptor = ptr::null_mut();
    if unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            sddl.as_ptr(),
            SDDL_REVISION_1,
            &mut descriptor,
            ptr::null_mut(),
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    let _allocation = Allocation(descriptor);
    let mut attributes = SECURITY_ATTRIBUTES {
        nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: descriptor,
        bInheritHandle: 0,
    };
    f((&mut attributes as *mut SECURITY_ATTRIBUTES).cast())
}
