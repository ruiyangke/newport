//! Token identities and security descriptors. Handles and LocalAlloc buffers
//! never escape the checked Win32 calls that own them.
use crate::handle::{check, owned};
use std::{
    io,
    os::windows::io::{AsHandle, AsRawHandle, BorrowedHandle, OwnedHandle},
    ptr,
};
use windows_sys::Win32::{
    Foundation::LocalFree,
    Security::{
        Authorization::{
            ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW,
            SDDL_REVISION_1,
        },
        GetTokenInformation, PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES, TOKEN_GROUPS, TOKEN_QUERY,
        TOKEN_USER, TokenLogonSid, TokenUser,
    },
    System::Threading::{
        GetCurrentProcess, OpenProcess, OpenProcessToken, PROCESS_QUERY_LIMITED_INFORMATION,
    },
};

struct Local(*mut std::ffi::c_void);
impl Drop for Local {
    fn drop(&mut self) {
        // SAFETY: the pointer is a LocalAlloc allocation (or null) this value owns.
        unsafe {
            LocalFree(self.0);
        }
    }
}

/// A process's access token, open for queries.
pub struct Token(OwnedHandle);

impl Token {
    pub fn current() -> io::Result<Self> {
        // SAFETY: the current-process pseudo-handle is always valid and never closed.
        Self::of(unsafe { BorrowedHandle::borrow_raw(GetCurrentProcess()) })
    }

    pub fn of(process: BorrowedHandle<'_>) -> io::Result<Self> {
        let mut handle = ptr::null_mut();
        // SAFETY: the process handle is open, and `handle` is writable.
        check(unsafe { OpenProcessToken(process.as_raw_handle(), TOKEN_QUERY, &mut handle) })?;
        // SAFETY: OpenProcessToken succeeded, so the token handle is new and owned.
        unsafe { owned(handle) }.map(Self)
    }

    /// The user's SID as a string.
    pub fn user(&self) -> io::Result<String> {
        self.sid(false)
    }

    /// The logon session's SID as a string.
    pub fn logon(&self) -> io::Result<String> {
        self.sid(true)
    }

    fn sid(&self, logon: bool) -> io::Result<String> {
        let class = if logon { TokenLogonSid } else { TokenUser };
        let token = self.0.as_raw_handle();
        let mut needed = 0;
        // SAFETY: a size query with a null buffer; the token is open.
        unsafe {
            GetTokenInformation(token, class, ptr::null_mut(), 0, &mut needed);
        }
        if needed == 0 {
            return Err(io::Error::last_os_error());
        }
        let smallest = if logon {
            size_of::<TOKEN_GROUPS>()
        } else {
            size_of::<TOKEN_USER>()
        };
        if (needed as usize) < smallest || needed > 64 * 1024 {
            return Err(io::Error::other("Invalid Windows token information size"));
        }
        // TOKEN_USER/TOKEN_GROUPS contain pointers and need pointer alignment.
        let mut buffer = vec![0usize; (needed as usize).div_ceil(size_of::<usize>())];
        // SAFETY: `buffer` is pointer-aligned and at least `needed` bytes.
        check(unsafe {
            GetTokenInformation(
                token,
                class,
                buffer.as_mut_ptr().cast(),
                needed,
                &mut needed,
            )
        })?;
        // SAFETY: the call succeeded and the buffer holds at least one whole
        // structure of the class; the SID points into the buffer, which outlives it.
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
        // SAFETY: `raw` is a valid SID; on success `text` is a LocalAlloc string.
        check(unsafe { ConvertSidToStringSidW(raw, &mut text) })?;
        let _allocation = Local(text.cast());
        let mut length = 0;
        // SAFETY: `text` is NUL-terminated and alive until `_allocation` drops.
        unsafe {
            while *text.add(length) != 0 {
                length += 1;
            }
        }
        // SAFETY: `length` units of `text` precede its NUL.
        Ok(String::from_utf16_lossy(unsafe {
            std::slice::from_raw_parts(text, length)
        }))
    }
}

/// Refuses process `pid` unless it runs as this process's user in this
/// process's logon session.
pub fn verify_same_session(pid: u32) -> io::Result<()> {
    // SAFETY: OpenProcess returns a new handle, or null on failure.
    let process = unsafe { owned(OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid)) }?;
    let own = Token::current()?;
    let peer = Token::of(process.as_handle())?;
    if own.user()? != peer.user()? || own.logon()? != peer.logon()? {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "Local peer belongs to another user or logon session",
        ));
    }
    Ok(())
}

/// A security descriptor built from SDDL.
pub struct Descriptor(Local);

impl Descriptor {
    pub fn from_sddl(sddl: &str) -> io::Result<Self> {
        let sddl: Vec<u16> = sddl.encode_utf16().chain(Some(0)).collect();
        let mut descriptor = ptr::null_mut();
        // SAFETY: `sddl` is NUL-terminated; on success `descriptor` is a LocalAlloc
        // allocation that `Local` frees.
        check(unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                sddl.as_ptr(),
                SDDL_REVISION_1,
                &mut descriptor,
                ptr::null_mut(),
            )
        })?;
        Ok(Self(Local(descriptor)))
    }

    pub fn as_ptr(&self) -> PSECURITY_DESCRIPTOR {
        self.0.0
    }

    /// Attributes carrying this descriptor. They are valid while it lives.
    pub fn attributes(&self, inheritable: bool) -> SECURITY_ATTRIBUTES {
        SECURITY_ATTRIBUTES {
            nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: self.as_ptr(),
            bInheritHandle: inheritable.into(),
        }
    }
}
