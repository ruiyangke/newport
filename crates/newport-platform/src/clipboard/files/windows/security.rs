//! Owner-only ACLs are supplied at creation, before any payload is written.
use std::{
    fs::File,
    io,
    mem::size_of,
    os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle},
};
use windows::{
    Win32::{
        Foundation::{HANDLE, HLOCAL, LocalFree},
        Security::{Authorization::*, *},
        Storage::FileSystem::FILE_ALL_ACCESS,
        System::Threading::{GetCurrentProcess, OpenProcessToken},
    },
    core::{PCWSTR, PWSTR},
};

fn error(error: impl std::fmt::Display) -> io::Error {
    io::Error::other(error.to_string())
}

struct Allocation(*mut core::ffi::c_void);
impl Drop for Allocation {
    fn drop(&mut self) {
        // SAFETY: the pointer is a LocalAlloc allocation (or null) this value owns.
        unsafe {
            let _ = LocalFree(Some(HLOCAL(self.0)));
        }
    }
}

struct User {
    _token: OwnedHandle,
    buffer: Vec<usize>,
}
impl User {
    fn current() -> io::Result<Self> {
        let mut token = HANDLE::default();
        // SAFETY: the pseudo-handle is always valid, and `token` is writable.
        unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) }.map_err(error)?;
        // SAFETY: OpenProcessToken succeeded, so the token handle is new and owned.
        let token = unsafe { OwnedHandle::from_raw_handle(token.0) };
        let mut size = 0;
        // SAFETY: a size query with no buffer; the token is open.
        unsafe {
            let _ =
                GetTokenInformation(HANDLE(token.as_raw_handle()), TokenUser, None, 0, &mut size);
        }
        if size < size_of::<TOKEN_USER>() as u32 || size > 64 * 1024 {
            return Err(io::Error::other("Invalid current-user token size"));
        }
        let mut buffer = vec![0usize; (size as usize).div_ceil(size_of::<usize>())];
        // SAFETY: `buffer` is pointer-aligned and at least `size` bytes.
        unsafe {
            GetTokenInformation(
                HANDLE(token.as_raw_handle()),
                TokenUser,
                Some(buffer.as_mut_ptr().cast()),
                size,
                &mut size,
            )
        }
        .map_err(error)?;
        Ok(Self {
            _token: token,
            buffer,
        })
    }
    fn sid(&self) -> PSID {
        // SAFETY: `current` filled the buffer with a TOKEN_USER, which it keeps alive.
        unsafe { (*self.buffer.as_ptr().cast::<TOKEN_USER>()).User.Sid }
    }
}

pub(crate) struct PrivateSecurity {
    allocation: Allocation,
}
impl PrivateSecurity {
    pub(crate) fn new() -> io::Result<Self> {
        let user = User::current()?;
        let mut text = PWSTR::null();
        // SAFETY: the SID is valid; on success `text` is a LocalAlloc string.
        unsafe { ConvertSidToStringSidW(user.sid(), &mut text) }.map_err(error)?;
        let _sid_allocation = Allocation(text.0.cast());
        // SAFETY: `text` is NUL-terminated and alive until `_sid_allocation` drops.
        let sid = unsafe { text.to_string() }.map_err(error)?;
        let sddl: Vec<u16> = format!("O:{sid}D:P(A;OICI;FA;;;{sid})")
            .encode_utf16()
            .chain(Some(0))
            .collect();
        let mut descriptor = PSECURITY_DESCRIPTOR::default();
        // SAFETY: `sddl` is NUL-terminated; on success `descriptor` is a LocalAlloc
        // allocation that `Allocation` frees.
        unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                PCWSTR(sddl.as_ptr()),
                SDDL_REVISION_1,
                &mut descriptor,
                None,
            )
        }
        .map_err(error)?;
        Ok(Self {
            allocation: Allocation(descriptor.0),
        })
    }
    pub(crate) fn attributes(&self) -> SECURITY_ATTRIBUTES {
        SECURITY_ATTRIBUTES {
            nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: self.allocation.0,
            bInheritHandle: false.into(),
        }
    }
}

/// Accept only the current user's ACL, not an arbitrary directory with our name.
pub(crate) fn verify_private(file: &File, require_protected: bool) -> io::Result<()> {
    let user = User::current()?;
    let mut owner = PSID::default();
    let mut acl = std::ptr::null_mut();
    let mut descriptor = PSECURITY_DESCRIPTOR::default();
    // SAFETY: the file handle is open, and every out-pointer is writable. On
    // success `owner` and `acl` point into `descriptor`, a LocalAlloc allocation
    // freed below, after the last use of either.
    let result = unsafe {
        GetSecurityInfo(
            super::filesystem::handle(file),
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
            Some(&mut owner),
            None,
            Some(&mut acl),
            None,
            Some(&mut descriptor),
        )
    };
    if result.0 != 0 {
        return Err(io::Error::from_raw_os_error(result.0 as i32));
    }
    let _allocation = Allocation(descriptor.0);
    // SAFETY: both SIDs are valid and outlive the call.
    unsafe { EqualSid(owner, user.sid()) }
        .map_err(|_| io::Error::other("Staging directory belongs to another user"))?;
    let mut control = 0;
    let mut revision = 0;
    // SAFETY: `descriptor` is valid, and both out-values are writable.
    unsafe { GetSecurityDescriptorControl(descriptor, &mut control, &mut revision) }
        .map_err(error)?;
    if acl.is_null() || require_protected && control & SE_DACL_PROTECTED.0 == 0 {
        return Err(io::Error::other("Staging ACL is not private"));
    }
    let mut size = ACL_SIZE_INFORMATION::default();
    // SAFETY: `acl` is non-null and valid; `size` is the structure the class names.
    unsafe {
        GetAclInformation(
            acl,
            (&mut size as *mut ACL_SIZE_INFORMATION).cast(),
            size_of::<ACL_SIZE_INFORMATION>() as u32,
            AclSizeInformation,
        )
    }
    .map_err(error)?;
    if size.AceCount != 1 {
        return Err(io::Error::other(
            "Staging ACL permits additional principals",
        ));
    }
    let mut ace = std::ptr::null_mut();
    // SAFETY: `acl` holds exactly one ACE, so index 0 exists.
    unsafe { GetAce(acl, 0, &mut ace) }.map_err(error)?;
    // SAFETY: GetAce returned a pointer to an ACE inside `acl`, which starts with a header.
    let header = unsafe { &*ace.cast::<ACE_HEADER>() };
    if usize::from(header.AceSize) < size_of::<ACCESS_ALLOWED_ACE>() {
        return Err(io::Error::other("Staging ACL has an invalid allow rule"));
    }
    // SAFETY: the header's size covers a whole ACCESS_ALLOWED_ACE (checked above).
    let ace = unsafe { &*ace.cast::<ACCESS_ALLOWED_ACE>() };
    // ACCESS_ALLOWED_ACE_TYPE is the standard discretionary allow ACE (zero).
    if ace.Header.AceType != 0
        || ace.Header.AceFlags as u32 & INHERIT_ONLY_ACE.0 != 0
        || ace.Mask != FILE_ALL_ACCESS.0
    {
        return Err(io::Error::other(
            "Staging ACL is not an owner-only allow rule",
        ));
    }
    let sid = PSID((&ace.SidStart as *const u32).cast_mut().cast());
    // SAFETY: the ACE's SID is valid inside `acl`, and `user` is alive.
    unsafe { EqualSid(sid, user.sid()) }
        .map_err(|_| io::Error::other("Staging ACL belongs to another principal"))
}
