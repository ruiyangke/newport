use super::super::Failure;
use std::{
    ffi::{CString, OsStr},
    fs::{File, OpenOptions},
    io,
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::{ffi::OsStrExt, fs::OpenOptionsExt},
    },
    path::Path,
};

const FILEID_KERNFS: i32 = 0xfe;

pub(super) struct Directory {
    pub file: File,
    pub identity: u64,
    pub mount: i32,
}

impl Directory {
    pub fn open(path: &Path) -> Result<Self, Failure> {
        let file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
            .open(path)?;
        Self::checked(file)
    }

    pub fn child(&self, name: &OsStr) -> Result<Self, Failure> {
        if name.as_bytes().contains(&b'/') || matches!(name.to_str(), Some("" | "." | "..")) {
            return Err(Failure::Invalid("Invalid cgroup directory component"));
        }
        let name = CString::new(name.as_bytes())
            .map_err(|_| Failure::Invalid("Invalid cgroup directory component"))?;
        // SAFETY: the parent stays open and `name` is NUL-terminated; the single
        // component cannot traverse a symlink or parent.
        let descriptor = unsafe {
            libc::openat(
                self.file.as_raw_fd(),
                name.as_ptr(),
                libc::O_RDONLY
                    | libc::O_DIRECTORY
                    | libc::O_NOFOLLOW
                    | libc::O_NONBLOCK
                    | libc::O_CLOEXEC,
            )
        };
        if descriptor < 0 {
            return Err(io::Error::last_os_error().into());
        }
        // SAFETY: a successful openat returns a fresh descriptor nothing else owns.
        let file = unsafe { File::from_raw_fd(descriptor) };
        let child = Self::checked(file)?;
        if child.mount != self.mount {
            return Err(Failure::Invalid(
                "Cgroup traversal crossed a mount boundary",
            ));
        }
        Ok(child)
    }

    fn checked(file: File) -> Result<Self, Failure> {
        let mut filesystem = std::mem::MaybeUninit::<libc::statfs>::uninit();
        // SAFETY: fstatfs fills the output on success; the file stays open.
        if unsafe { libc::fstatfs(file.as_raw_fd(), filesystem.as_mut_ptr()) } < 0 {
            return Err(io::Error::last_os_error().into());
        }
        // SAFETY: the successful fstatfs initialized this output.
        let filesystem = unsafe { filesystem.assume_init() };
        if i128::from(filesystem.f_type) != i128::from(libc::CGROUP2_SUPER_MAGIC) {
            return Err(Failure::Invalid(
                "Discovered mount is not a cgroup-v2 filesystem",
            ));
        }
        let (identity, mount) = handle(&file)?;
        Ok(Self {
            file,
            identity,
            mount,
        })
    }
}

#[repr(C)]
struct Handle {
    header: libc::file_handle,
    bytes: [u8; 8],
}

fn handle(file: &File) -> Result<(u64, i32), Failure> {
    let mut value = Handle {
        header: libc::file_handle {
            handle_bytes: 8,
            handle_type: 0,
            f_handle: [],
        },
        bytes: [0; 8],
    };
    let mut mount = 0;
    // SAFETY: repr(C) places the byte storage immediately after file_handle's
    // flexible array, `handle_bytes` is that storage's size, every output
    // pointer is writable, and AT_EMPTY_PATH refers to the borrowed fd.
    let result = unsafe {
        libc::name_to_handle_at(
            file.as_raw_fd(),
            c"".as_ptr(),
            std::ptr::addr_of_mut!(value).cast(),
            &mut mount,
            libc::AT_EMPTY_PATH,
        )
    };
    if result < 0 {
        return Err(io::Error::last_os_error().into());
    }
    if value.header.handle_bytes != 8 || value.header.handle_type != FILEID_KERNFS || mount <= 0 {
        return Err(Failure::Invalid("Unsupported cgroup file handle"));
    }
    let identity = u64::from_ne_bytes(value.bytes);
    if identity == 0 {
        return Err(Failure::Invalid("Empty cgroup file handle"));
    }
    Ok((identity, mount))
}
