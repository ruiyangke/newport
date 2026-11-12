//! Child processes created inside a job, inheriting exactly the handles they
//! are given.
use crate::{
    handle::{check, owned},
    job::Job,
};
use std::{
    ffi::OsStr,
    io,
    os::windows::io::{AsRawHandle, OwnedHandle},
    ptr,
};
use windows_sys::Win32::{
    Foundation::HANDLE,
    System::Threading::{
        CREATE_UNICODE_ENVIRONMENT, CreateProcessW, DeleteProcThreadAttributeList,
        EXTENDED_STARTUPINFO_PRESENT, InitializeProcThreadAttributeList,
        PROC_THREAD_ATTRIBUTE_HANDLE_LIST, PROC_THREAD_ATTRIBUTE_JOB_LIST, PROCESS_CREATION_FLAGS,
        PROCESS_INFORMATION, STARTF_USESTDHANDLES, STARTUPINFOEXW, UpdateProcThreadAttribute,
    },
};

/// What [`spawn`] starts. The standard handles must be inheritable and among
/// `inherit`; nothing else the parent holds reaches the child.
pub struct Spawn<'a> {
    /// The absolute executable.
    pub application: &'a OsStr,
    /// From [`crate::command_line`].
    pub command_line: Vec<u16>,
    /// The job the process is created in, before any of its code runs.
    pub job: &'a Job,
    pub stdin: HANDLE,
    pub stdout: HANDLE,
    pub stderr: HANDLE,
    pub inherit: Vec<HANDLE>,
    /// Creation flags besides the extended startup information.
    pub flags: PROCESS_CREATION_FLAGS,
    /// A double-NUL-terminated environment block; the parent's without one.
    pub environment: Option<Vec<u16>>,
    /// A NUL-terminated working directory; the parent's without one.
    pub directory: Option<Vec<u16>>,
    /// A NUL-terminated `station\desktop`; the parent's without one.
    pub desktop: Option<Vec<u16>>,
}

pub struct Child {
    pub process: OwnedHandle,
    pub thread: OwnedHandle,
    pub id: u32,
}

/// A process-and-thread attribute list. The arrays it points to live with it.
struct Attributes {
    list: Vec<usize>,
    _inherit: Box<[HANDLE]>,
    _jobs: Box<[HANDLE]>,
}

impl Attributes {
    fn new(inherit: Box<[HANDLE]>, jobs: Box<[HANDLE]>) -> io::Result<Self> {
        let mut length = 0;
        // SAFETY: a size query; a null list is allowed.
        unsafe {
            InitializeProcThreadAttributeList(ptr::null_mut(), 2, 0, &mut length);
        }
        if length == 0 || length > 64 * 1024 {
            return Err(io::Error::other("Invalid process attribute list size"));
        }
        let mut list = vec![0usize; length.div_ceil(size_of::<usize>())];
        // SAFETY: `list` is pointer-aligned and at least `length` bytes.
        check(unsafe {
            InitializeProcThreadAttributeList(list.as_mut_ptr().cast(), 2, 0, &mut length)
        })?;
        let mut attributes = Self {
            list,
            _inherit: inherit,
            _jobs: jobs,
        };
        for (key, value) in [
            (PROC_THREAD_ATTRIBUTE_HANDLE_LIST, &attributes._inherit),
            (PROC_THREAD_ATTRIBUTE_JOB_LIST, &attributes._jobs),
        ] {
            // SAFETY: the list is initialized, and the value arrays are owned by
            // `attributes` until it drops, after the process is created.
            check(unsafe {
                UpdateProcThreadAttribute(
                    attributes.list.as_mut_ptr().cast(),
                    0,
                    key as usize,
                    value.as_ptr().cast(),
                    size_of_val(&**value),
                    ptr::null_mut(),
                    ptr::null(),
                )
            })?;
        }
        Ok(attributes)
    }
}

impl Drop for Attributes {
    fn drop(&mut self) {
        // SAFETY: `new` initialized the list, and it is deleted once.
        unsafe {
            DeleteProcThreadAttributeList(self.list.as_mut_ptr().cast());
        }
    }
}

/// Creates the process. Its job membership is atomic with creation.
pub fn spawn(options: Spawn) -> io::Result<Child> {
    let Spawn {
        application,
        mut command_line,
        job,
        stdin,
        stdout,
        stderr,
        inherit,
        mut flags,
        environment,
        directory,
        mut desktop,
    } = options;
    if command_line.len() > 32767 || command_line.last() != Some(&0) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Invalid child process command line",
        ));
    }
    let terminated =
        |text: &Option<Vec<u16>>| text.as_ref().is_none_or(|text| text.last() == Some(&0));
    if !terminated(&directory)
        || !terminated(&desktop)
        || environment
            .as_ref()
            .is_some_and(|block| !block.ends_with(&[0, 0]))
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Child process strings must be NUL-terminated",
        ));
    }
    let application = crate::wide(application)?;
    let mut attributes =
        Attributes::new(inherit.into_boxed_slice(), Box::new([job.as_raw_handle()]))?;
    // SAFETY: the structure is plain data, valid when zeroed.
    let mut startup: STARTUPINFOEXW = unsafe { std::mem::zeroed() };
    startup.StartupInfo.cb = size_of::<STARTUPINFOEXW>() as u32;
    startup.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
    startup.StartupInfo.hStdInput = stdin;
    startup.StartupInfo.hStdOutput = stdout;
    startup.StartupInfo.hStdError = stderr;
    if let Some(desktop) = desktop.as_mut() {
        startup.StartupInfo.lpDesktop = desktop.as_mut_ptr();
    }
    startup.lpAttributeList = attributes.list.as_mut_ptr().cast();
    if environment.is_some() {
        flags |= CREATE_UNICODE_ENVIRONMENT;
    }
    // SAFETY: the structure is plain data, valid when zeroed.
    let mut information: PROCESS_INFORMATION = unsafe { std::mem::zeroed() };
    // SAFETY: every string is NUL-terminated and, with the attribute list and
    // the handles it names, outlives the call; `command_line` is writable.
    check(unsafe {
        CreateProcessW(
            application.as_ptr(),
            command_line.as_mut_ptr(),
            ptr::null(),
            ptr::null(),
            1,
            EXTENDED_STARTUPINFO_PRESENT | flags,
            environment
                .as_ref()
                .map_or(ptr::null(), |block| block.as_ptr().cast()),
            directory.as_ref().map_or(ptr::null(), |path| path.as_ptr()),
            &startup.StartupInfo,
            &mut information,
        )
    })?;
    // SAFETY: CreateProcessW succeeded, so both handles are new and owned.
    Ok(Child {
        process: unsafe { owned(information.hProcess) }?,
        thread: unsafe { owned(information.hThread) }?,
        id: information.dwProcessId,
    })
}
