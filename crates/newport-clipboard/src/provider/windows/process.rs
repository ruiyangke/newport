//! Exact-process ownership and private, overlapped clipboard IPC.
use super::{Files, OpenFiles, WorkerExecutable, worker};
use newport_win32::{
    command_line,
    handle::{null_device, set_inheritable, signaled},
    job::Job,
    pipe,
    process::{self, Spawn},
    security::{Descriptor, Token, verify_same_session},
};
use std::{
    io,
    os::windows::io::{AsHandle, AsRawHandle, BorrowedHandle, OwnedHandle},
    ptr,
    sync::{
        Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tokio::net::windows::named_pipe::{NamedPipeClient, NamedPipeServer, ServerOptions};
use windows_sys::Win32::{
    Foundation::*,
    Security::SECURITY_ATTRIBUTES,
    Storage::FileSystem::*,
    System::{Console::*, StationsAndDesktops::*, Threading::*},
};
pub const ARGUMENT: &str = "--clipboard-provider-windows";
const GRACE: Duration = Duration::from_millis(100);
const REAP: Duration = Duration::from_millis(500);
fn object_name(handle: HANDLE) -> io::Result<Vec<u16>> {
    let mut needed = 0;
    // SAFETY: A null buffer of length zero only asks for the required size.
    unsafe {
        GetUserObjectInformationW(handle, UOI_NAME, ptr::null_mut(), 0, &mut needed);
    }
    if needed == 0 || needed > 4096 {
        return Err(io::Error::other("Invalid worker desktop name"));
    }
    let mut value = vec![0u16; (needed as usize).div_ceil(2)];
    // SAFETY: `value` holds at least `needed` bytes and both stay valid for the call.
    if unsafe {
        GetUserObjectInformationW(
            handle,
            UOI_NAME,
            value.as_mut_ptr().cast(),
            needed,
            &mut needed,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    while value.last() == Some(&0) {
        value.pop();
    }
    Ok(value)
}
/// Validate the server identity before sending any protocol or credential data.
/// The client uses SECURITY_IDENTIFICATION (Tokio's default), never impersonation.
fn verify_server(pipe: BorrowedHandle<'_>) -> io::Result<()> {
    verify_same_session(pipe::peer_pid(pipe, false)?)
}
static ACTIVE: AtomicBool = AtomicBool::new(false);
struct Slot;
impl Drop for Slot {
    fn drop(&mut self) {
        ACTIVE.store(false, Ordering::Release);
    }
}
struct Quarantined {
    child: Child,
    _files: Vec<Files>,
}
static QUARANTINED: Mutex<Option<Quarantined>> = Mutex::new(None);
fn reserve() -> io::Result<Slot> {
    let mut quarantined = QUARANTINED
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(previous) = quarantined.as_mut() {
        if previous.child.exited()? {
            quarantined.take();
        } else {
            return Err(io::Error::other(
                "Previous clipboard helper has not been reaped",
            ));
        }
    }
    ACTIVE
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .map_err(|_| io::Error::other("A clipboard helper already owns this process slot"))?;
    Ok(Slot)
}
pub(super) fn quarantine(child: Child, files: Vec<Files>) {
    let mut previous = QUARANTINED
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    assert!(
        previous.is_none(),
        "only the active clipboard process can enter quarantine"
    );
    *previous = Some(Quarantined {
        child,
        _files: files,
    });
}
pub(super) struct Child {
    process: OwnedHandle,
    job: Job,
    reaped: bool,
    slot: Option<Slot>,
}
impl Child {
    fn exited(&mut self) -> io::Result<bool> {
        let exited = signaled(&self.process)?;
        self.reaped |= exited;
        Ok(exited)
    }
    pub(super) async fn finish(&mut self) -> Result<(), String> {
        let deadline = tokio::time::Instant::now() + GRACE;
        while !self.exited().map_err(|e| e.to_string())? && tokio::time::Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        if self.reaped {
            return Ok(());
        }
        if !self.job.terminate(1) {
            let error = io::Error::last_os_error();
            if !self.exited().map_err(|e| e.to_string())? {
                return Err(format!("Cannot terminate clipboard helper: {error}"));
            }
        }
        let deadline = tokio::time::Instant::now() + REAP;
        loop {
            if self.exited().map_err(|e| e.to_string())? {
                return Ok(());
            }
            if tokio::time::Instant::now() >= deadline {
                return Err("Clipboard helper termination was not reaped".into());
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }
}
impl Drop for Child {
    fn drop(&mut self) {
        if !self.reaped {
            self.job.terminate(1);
            if !self.exited().unwrap_or(false) {
                // Unexpected unwinding must never admit a successor without reap.
                if let Some(slot) = self.slot.take() {
                    std::mem::forget(slot);
                }
            }
        }
    }
}
pub(super) async fn spawn(executable: &WorkerExecutable) -> io::Result<(Child, NamedPipeServer)> {
    let slot = reserve()?;
    if !executable.path.is_absolute() {
        return Err(io::Error::other("Clipboard worker path must be absolute"));
    }
    let name = format!(r"\\.\pipe\newport-clipboard-{}", uuid::Uuid::new_v4());
    // No inherited or default ACEs: only this logon session can connect or
    // create further instances.
    let descriptor =
        Descriptor::from_sddl(&format!("D:P(A;;GA;;;{})", Token::current()?.logon()?))?;
    let mut attributes = descriptor.attributes(false);
    // SAFETY: `attributes` points into `descriptor`, and both outlive the call.
    let server = unsafe {
        ServerOptions::new()
            .first_pipe_instance(true)
            .reject_remote_clients(true)
            .create_with_security_attributes_raw(
                &name,
                (&mut attributes as *mut SECURITY_ATTRIBUTES).cast(),
            )
    }?;
    // Register only the server with this process's IOCP. The child registers the
    // inherited client; a handle already registered here cannot migrate IOCPs.
    let client = pipe::open_client(
        &command_line::wide(name.as_ref())?,
        GENERIC_READ | GENERIC_WRITE,
        FILE_FLAG_OVERLAPPED,
        false,
    )?;
    verify_server(client.as_handle())?;
    server.connect().await?;
    if pipe::peer_pid(server.as_handle(), true)? != std::process::id() {
        return Err(io::Error::other(
            "Clipboard pipe was claimed by another process",
        ));
    }
    set_inheritable(&client)?;
    let null = null_device(GENERIC_WRITE)?;
    let job = Job::new(Some(1))?;
    // SAFETY: Both calls take no arguments and return pseudo-handles that need no close.
    let mut desktop = object_name(unsafe { GetProcessWindowStation() })?;
    desktop.push(b'\\' as u16);
    // SAFETY: Takes the current thread's own id and returns a handle the thread keeps.
    desktop.extend(object_name(unsafe {
        GetThreadDesktop(GetCurrentThreadId())
    })?);
    desktop.push(0);
    let spawned = process::spawn(Spawn {
        application: executable.path.as_os_str(),
        command_line: command_line::command_line(
            executable.path.as_os_str(),
            executable
                .arguments
                .iter()
                .map(|argument| argument.as_ref()),
        )?,
        job: &job,
        stdin: client.as_raw_handle(),
        stdout: null.as_raw_handle(),
        stderr: null.as_raw_handle(),
        inherit: vec![client.as_raw_handle(), null.as_raw_handle()],
        flags: CREATE_SUSPENDED | CREATE_NO_WINDOW,
        environment: None,
        directory: None,
        desktop: Some(desktop),
    })?;
    let mut child = Child {
        process: spawned.process,
        job,
        reaped: false,
        slot: Some(slot),
    };
    // SAFETY: The handle is the suspended primary thread owned by `spawned`.
    if unsafe { ResumeThread(spawned.thread.as_raw_handle()) } == u32::MAX {
        let error = io::Error::last_os_error();
        if child.finish().await.is_err() {
            quarantine(child, Vec::new());
        }
        return Err(error);
    }
    Ok((child, server))
}
/// Called before profile, credential, GUI, or account initialization.
pub fn run_from_stdio(open: OpenFiles) -> Result<(), String> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .map_err(|e| e.to_string())?;
    let result = runtime.block_on(async {
        // SAFETY: Takes a constant and returns a process-owned handle or an error value.
        let raw = unsafe { GetStdHandle(STD_INPUT_HANDLE) };
        if raw.is_null() || raw == INVALID_HANDLE_VALUE {
            return Err("Missing clipboard worker pipe".into());
        }
        // SAFETY: The standard input handle stays open for the life of the worker.
        verify_server(unsafe { BorrowedHandle::borrow_raw(raw) }).map_err(|e| e.to_string())?;
        // SAFETY: Ownership transfers once; the standard library never reads this handle,
        // and the parent opened it as an overlapped pipe client.
        let pipe = unsafe { NamedPipeClient::from_raw_handle(raw) }.map_err(|e| e.to_string())?;
        worker::run(pipe, open).await
    });
    runtime.shutdown_timeout(Duration::from_millis(50));
    result
}
