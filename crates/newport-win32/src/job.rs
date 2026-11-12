//! Job objects that own a supervised process tree.
use crate::handle::{check, owned};
use std::{
    io,
    os::windows::io::{AsRawHandle, OwnedHandle, RawHandle},
    ptr,
};
use windows_sys::Win32::System::JobObjects::{
    CreateJobObjectW, JOB_OBJECT_LIMIT_ACTIVE_PROCESS, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    JOBOBJECT_BASIC_ACCOUNTING_INFORMATION, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
    JobObjectBasicAccountingInformation, JobObjectExtendedLimitInformation,
    QueryInformationJobObject, SetInformationJobObject, TerminateJobObject,
};

/// An unnamed job whose processes end when its last handle closes.
pub struct Job(OwnedHandle);

impl Job {
    /// With `active`, the job admits at most that many live processes.
    pub fn new(active: Option<u32>) -> io::Result<Self> {
        // SAFETY: a new unnamed job; the handle is owned by the result.
        let job = unsafe { owned(CreateJobObjectW(ptr::null(), ptr::null())) }?;
        // SAFETY: the structure is plain data, valid when zeroed.
        let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        if let Some(active) = active {
            limits.BasicLimitInformation.LimitFlags |= JOB_OBJECT_LIMIT_ACTIVE_PROCESS;
            limits.BasicLimitInformation.ActiveProcessLimit = active;
        }
        // SAFETY: the job is open, and `limits` is the structure the information
        // class names, with its size.
        check(unsafe {
            SetInformationJobObject(
                job.as_raw_handle(),
                JobObjectExtendedLimitInformation,
                (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                size_of_val(&limits) as u32,
            )
        })?;
        Ok(Self(job))
    }

    /// Ends every process in the job with `code`. False when Windows refused.
    pub fn terminate(&self, code: u32) -> bool {
        // SAFETY: the job handle is open for the call.
        unsafe { TerminateJobObject(self.0.as_raw_handle(), code) != 0 }
    }

    pub fn accounting(&self) -> io::Result<JOBOBJECT_BASIC_ACCOUNTING_INFORMATION> {
        // SAFETY: the structure is plain data, valid when zeroed.
        let mut information: JOBOBJECT_BASIC_ACCOUNTING_INFORMATION = unsafe { std::mem::zeroed() };
        // SAFETY: the job is open, and `information` is the structure the
        // information class names, with its size.
        check(unsafe {
            QueryInformationJobObject(
                self.0.as_raw_handle(),
                JobObjectBasicAccountingInformation,
                (&mut information as *mut JOBOBJECT_BASIC_ACCOUNTING_INFORMATION).cast(),
                size_of_val(&information) as u32,
                ptr::null_mut(),
            )
        })?;
        Ok(information)
    }
}

impl AsRawHandle for Job {
    fn as_raw_handle(&self) -> RawHandle {
        self.0.as_raw_handle()
    }
}
